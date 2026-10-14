#!/usr/bin/env python3
"""Build synthetic cleanup training pairs from the public-domain corpus.

For each book (corpus/clean/pg<ID>.txt, one paragraph per line):
  1. cut it into dictation segments with running context (segment.py)
  2. simulate Parakeet output for each segment (simulate.py)
  3. run the simulated ASR text through the command pass (eval/commands.py,
     a mirror of crates/bd-commands), exactly as the runtime does
  4. verify the word guardrail: every expected word is in the model input,
     in order (otherwise the example is dropped and counted)

Whole books are held out for validation. Output (default
/mnt/data/beardictate-dev/pairs):
  train.jsonl / val.jsonl          chat format for SFT:
      {"messages": [system, user, assistant], "id", "book", "style"}
  train_pairs.jsonl / val_pairs.jsonl
      eval format {id, category, context, raw, expected, style}; the val
      file can be scored directly with eval/score.py
  report.md                        counts, noise stats, random examples
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import re
import sys
from collections import Counter
from dataclasses import asdict, fields
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "eval"))

import commands  # noqa: E402
from prompt import Prompt  # noqa: E402
from segment import Segment, segments  # noqa: E402
from simulate import NoiseConfig, simulate  # noqa: E402
from textnorm import check_edits  # noqa: E402

MARKS = {"us": ("“", "”"), "uk": ("‘", "’")}
TO_UK = str.maketrans({"“": "‘", "”": "’", "‘": "“", "’": "”"})


def category(seg: Segment) -> str:
    has_quote = "“" in seg.expected or seg.in_quote_at_start
    if seg.in_quote_at_start:
        return "continuation"
    if seg.crosses_paragraph and has_quote:
        return "dialog_multi"
    if has_quote:
        return "dialog"
    return "narration"


def modernize(par: str, rng: random.Random) -> str:
    """Replace punctuation a dictating author never speaks and the model
    cannot recover: semicolons become a comma or a sentence break, and a
    colon before a lowercase word becomes a comma. Done on the source text,
    so contexts and targets agree."""
    def semi(m: re.Match[str]) -> str:
        nxt = m.group(1)
        return f", {nxt}" if rng.random() < 0.5 else f". {nxt[:1].upper()}{nxt[1:]}"

    par = re.sub(r";\s+([\u201c\u2018]?\w)", semi, par)
    par = re.sub(r":\s+(?=[a-z])", ", ", par)
    return par.replace(";", ",")


def load_books(corpus: Path, include_uk_books: bool) -> list[tuple[int, str, list[str]]]:
    books = []
    for line in (corpus / "manifest.jsonl").read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        if row["status"] not in ("ok",) or (row["style"] != "us" and not include_uk_books):
            continue
        pars = (corpus / "clean" / f"pg{row['id']}.txt").read_text(encoding="utf-8").splitlines()
        books.append((int(row["id"]), str(row["title"]), [p for p in pars if p.strip()]))
    return books


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--corpus", type=Path, default=Path("/mnt/data/beardictate-dev/corpus"))
    ap.add_argument("--out", type=Path, default=Path("/mnt/data/beardictate-dev/pairs"))
    ap.add_argument("--prompt", type=Path, default=None, help="prompt file (default: runtime prompt if present)")
    ap.add_argument("--seed", type=int, default=1234)
    ap.add_argument("--per-book", type=int, default=1500, help="max examples kept per book")
    ap.add_argument("--val-books", type=int, default=5, help="whole books held out for validation")
    ap.add_argument("--val-ids", type=int, nargs="*", default=None, help="explicit Gutenberg ids to hold out")
    ap.add_argument("--narration-keep", type=float, default=0.5, help="keep-probability for narration-only segments")
    ap.add_argument("--uk-fraction", type=float, default=0.1, help="fraction of books rendered in UK quote style")
    ap.add_argument("--noise-scale", type=float, default=1.0, help="multiply all disfluency rates")
    ap.add_argument("--clean-fraction", type=float, default=0.05, help="examples with no disfluencies at all")
    ap.add_argument("--clause-rate", type=float, default=0.08)
    ap.add_argument("--cross-rate", type=float, default=0.35)
    ap.add_argument("--no-context-rate", type=float, default=0.03, help="drop context (start of a session)")
    ap.add_argument("--keep-old-punct", action="store_true", help="keep semicolons/colons from the source text")
    ap.add_argument("--include-uk-books", action="store_true", help="also use single-quote (UK-typeset) books as-is")
    for f in fields(NoiseConfig):
        ap.add_argument(f"--noise-{f.name.replace('_', '-')}", type=float, default=None, dest=f"noise_{f.name}")
    args = ap.parse_args()

    cfg = NoiseConfig.scaled(args.noise_scale)
    for f in fields(NoiseConfig):
        v = getattr(args, f"noise_{f.name}")
        if v is not None:
            setattr(cfg, f.name, v)
    clean_cfg = NoiseConfig(**{f.name: 0.0 for f in fields(NoiseConfig)})
    clean_cfg.trailing_period = cfg.trailing_period
    prompt = Prompt.load(args.prompt)

    books = load_books(args.corpus, args.include_uk_books)
    order = sorted(b[0] for b in books)
    rng0 = random.Random(args.seed)
    val_ids = set(args.val_ids) if args.val_ids else set(rng0.sample(order, min(args.val_books, len(order) - 1)))
    uk_ids = {b for b in order if b not in val_ids and rng0.random() < args.uk_fraction}
    uk_ids |= set(sorted(val_ids)[:1]) if args.uk_fraction > 0 else set()

    args.out.mkdir(parents=True, exist_ok=True)
    files = {name: (args.out / name).open("w", encoding="utf-8")
             for name in ("train.jsonl", "val.jsonl", "train_pairs.jsonl", "val_pairs.jsonl")}
    stats: Counter[str] = Counter()
    counts: Counter[str] = Counter()
    seen: set[str] = set()
    examples: list[dict[str, str]] = []

    for book_id, title, pars in books:
        rng = random.Random(f"{args.seed}-{book_id}")
        if not args.keep_old_punct:
            pars = [modernize(p, rng) for p in pars]
        split = "val" if book_id in val_ids else "train"
        style = "uk" if book_id in uk_ids else "us"
        kept: list[dict[str, str]] = []
        for n, seg in enumerate(segments(pars, rng, clause_rate=args.clause_rate, cross_rate=args.cross_rate)):
            cat = category(seg)
            if cat == "narration" and rng.random() > args.narration_keep:
                continue
            context, expected = seg.context, seg.expected
            if rng.random() < args.no_context_rate:
                context, expected = "", expected.lstrip("\n")
            if style == "uk":
                context, expected = context.translate(TO_UK), expected.translate(TO_UK)
            noise = clean_cfg if rng.random() < args.clean_fraction else cfg
            raw = simulate(expected, rng, noise, stats, MARKS[style])
            model_in = commands.apply(raw, seg.in_quote_at_start, MARKS[style]).text
            chk = check_edits(model_in, expected)
            if chk.added:
                counts["dropped_guardrail"] += 1
                continue
            key = hashlib.sha1(f"{model_in}\x00{expected}".encode()).hexdigest()
            if key in seen:
                counts["dropped_duplicate"] += 1
                continue
            seen.add(key)
            kept.append({"id": f"pg{book_id}-{n}", "book": str(book_id), "category": cat, "style": style,
                         "context": context, "raw": raw, "model_input": model_in, "expected": expected})
        rng.shuffle(kept)
        kept = kept[: args.per_book]
        for ex in kept:
            msgs = prompt.messages(ex["context"], ex["model_input"], ex["style"])
            msgs.append({"role": "assistant", "content": ex["expected"]})
            files[f"{split}.jsonl"].write(json.dumps({"id": ex["id"], "book": ex["book"], "style": ex["style"],
                                                      "messages": msgs}, ensure_ascii=False) + "\n")
            pair = {k: ex[k] for k in ("id", "category", "context", "raw", "expected", "style")}
            files[f"{split}_pairs.jsonl"].write(json.dumps(pair, ensure_ascii=False) + "\n")
            counts[f"{split}"] += 1
            counts[f"{split}:{ex['category']}"] += 1
            counts[f"{split}:{ex['style']}"] += 1
        examples.extend(kept[:20])
        print(f"pg{book_id:<6} {split:<5} {style} {len(kept):>5}  {title}", flush=True)
    for f in files.values():
        f.close()

    sample = random.Random(args.seed).sample(examples, min(10, len(examples)))
    report = render_report(args, cfg, counts, stats, sample, val_ids, uk_ids, prompt.path)
    (args.out / "report.md").write_text(report, encoding="utf-8")
    print(report)
    return 0


def render_report(args: argparse.Namespace, cfg: NoiseConfig, counts: Counter[str], stats: Counter[str],
                  sample: list[dict[str, str]], val_ids: set[int], uk_ids: set[int], prompt_path: Path) -> str:
    words = max(1, stats["words"])
    lines = ["# Synthetic pair generation report", "",
             f"seed {args.seed}, prompt `{prompt_path}`, noise scale {args.noise_scale}", "",
             f"- train examples: {counts['train']}", f"- val examples: {counts['val']} (held-out books: {sorted(val_ids)})",
             f"- UK-style books: {sorted(uk_ids)}",
             f"- dropped by guardrail check: {counts['dropped_guardrail']}, duplicates: {counts['dropped_duplicate']}", "",
             "| split:category/style | count |", "|---|---:|"]
    lines += [f"| {k} | {v} |" for k, v in sorted(counts.items()) if ":" in k]
    lines += ["", "## Noise (over all simulated segments, before sampling)", "",
              f"segments {stats['segments']}, words {stats['words']}", "", "| event | count | per 100 words |", "|---|---:|---:|"]
    lines += [f"| {k} | {v} | {100 * v / words:.2f} |" for k, v in sorted(stats.items()) if k not in ("segments", "words")]
    lines += ["", "## Noise config", "", "```", json.dumps(asdict(cfg), indent=1), "```", "", "## 10 random examples", ""]
    for ex in sample:
        lines += [f"**{ex['id']}** ({ex['category']}, {ex['style']})", "", "```",
                  "context:  " + ex["context"][-300:].replace("\n", "⏎"),
                  "raw:      " + ex["raw"].replace("\n", "⏎"),
                  "input:    " + ex["model_input"].replace("\n", "⏎"),
                  "expected: " + ex["expected"].replace("\n", "⏎"), "```", ""]
    return "\n".join(lines)


if __name__ == "__main__":
    sys.exit(main())
