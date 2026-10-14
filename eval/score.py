#!/usr/bin/env python3
"""Score cleanup-model outputs against the eval cases.

    score.py cases.jsonl outputs.jsonl [--report report.md] [--details scores.jsonl]

outputs.jsonl rows: {"id", "output", "latency_ms"}.

Metrics, per category and overall:
  exact        output == expected after canonicalizing quote glyphs
               (straight/curly) and spaces; breaks and punctuation count
  word_acc     1 - WER on normalized words (case, punctuation, quotes ignored)
  quote_ok     same set of quoted spans, compared by word boundaries in
               expected-word coordinates (case-level, all-or-nothing)
  quote_f1     F1 over quoted spans (micro)
  para_ok      same paragraph/line breaks at the same word positions
               (including a leading or trailing break)
  punct_f1     F1 over (word, mark) pairs for , . ; : ! ? — …  (micro)
  forbidden    words added or substituted relative to the raw input after
               the command pass (should be 0; what the guardrail rejects)
  latency      p50 / p95 of latency_ms
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

import commands
from textnorm import Tokenized, check_edits, edit_distance, ends_in_open_quote, lcs_pairs, tokenize, words

MARKS = {"us": ("“", "”"), "uk": ("‘", "’")}
_CANON = str.maketrans({"“": '"', "”": '"', "‘": "'", "’": "'", "ʼ": "'"})


def canon(text: str) -> str:
    text = text.translate(_CANON).replace("...", "…")
    text = re.sub(r"[ \t]+", " ", text)
    return re.sub(r" ?\n ?", "\n", text).strip(" ")


@dataclass
class Counts:
    tp: int = 0
    fp: int = 0
    fn: int = 0

    def add(self, gold: set[tuple[object, ...]], pred: set[tuple[object, ...]]) -> None:
        self.tp += len(gold & pred)
        self.fp += len(pred - gold)
        self.fn += len(gold - pred)

    @property
    def f1(self) -> float:
        if self.tp + self.fp + self.fn == 0:
            return 1.0
        return 2 * self.tp / (2 * self.tp + self.fp + self.fn)


@dataclass
class CaseScore:
    id: str
    category: str
    exact: bool
    word_acc: float
    quote_ok: bool
    para_ok: bool
    forbidden: int
    added: list[str]
    latency_ms: float | None
    style: str = "us"
    quote: Counts = field(default_factory=Counts)
    punct: Counts = field(default_factory=Counts)


def _mapped(exp: Tokenized, out: Tokenized) -> dict[int, int]:
    """Output token index -> expected token index, via word LCS."""
    pairs = lcs_pairs([t.word for t in exp.toks], [t.word for t in out.toks])
    return {j: i for i, j in pairs}


def quote_spans(tz: Tokenized, index: dict[int, int] | None = None) -> set[tuple[int, int]]:
    """(first, last) word index of each quoted span; output tokens are mapped
    into expected coordinates through `index` (unaligned words are ignored)."""
    groups: dict[int, list[int]] = {}
    for k, t in enumerate(tz.toks):
        if t.span < 0:
            continue
        pos = k if index is None else index.get(k)
        if pos is not None:
            groups.setdefault(t.span, []).append(pos)
    return {(min(v), max(v)) for v in groups.values()}


def breaks(tz: Tokenized, index: dict[int, int] | None, n_expected: int) -> set[tuple[int, str]]:
    out: set[tuple[int, str]] = set()
    for k, t in enumerate(tz.toks):
        if t.brk:
            pos = k if index is None else index.get(k)
            out.add((pos if pos is not None else -1 - k, t.brk))
    if tz.trailing_break:
        out.add((n_expected, tz.trailing_break))
    return out


def puncts(tz: Tokenized, index: dict[int, int] | None) -> set[tuple[int, str]]:
    out: set[tuple[int, str]] = set()
    for k, t in enumerate(tz.toks):
        pos = k if index is None else index.get(k)
        for ch in t.punct:
            out.add((pos if pos is not None else -1 - k, ch))
    return out


def score_case(case: dict[str, str], output: str, latency_ms: float | None) -> CaseScore:
    style = case.get("style", "us")
    in_quote = ends_in_open_quote(case.get("context", ""))
    expected = case["expected"]
    exp = tokenize(expected, in_quote)
    out = tokenize(output, in_quote)
    idx = _mapped(exp, out)
    n = len(exp.toks)

    ew, ow = words(expected), words(output)
    word_acc = max(0.0, 1.0 - edit_distance(ew, ow) / max(1, len(ew)))

    q = Counts()
    gold_q, pred_q = quote_spans(exp), quote_spans(out, idx)
    q.add(gold_q, pred_q)  # type: ignore[arg-type]
    p = Counts()
    p.add(puncts(exp, None), puncts(out, idx))  # type: ignore[arg-type]

    raw_after = commands.apply(case["raw"], in_quote, MARKS[style]).text
    chk = check_edits(raw_after, output)
    return CaseScore(
        id=case["id"],
        category=case["category"],
        exact=canon(output) == canon(expected),
        word_acc=word_acc,
        quote_ok=gold_q == pred_q,
        para_ok=breaks(exp, None, n) == breaks(out, idx, n),
        forbidden=chk.forbidden,
        added=chk.added,
        latency_ms=latency_ms,
        style=style,
        quote=q,
        punct=p,
    )


def pct(xs: list[float], q: float) -> float:
    if not xs:
        return float("nan")
    xs = sorted(xs)
    k = (len(xs) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def aggregate(scores: list[CaseScore]) -> dict[str, float]:
    q, p = Counts(), Counts()
    for s in scores:
        q.tp, q.fp, q.fn = q.tp + s.quote.tp, q.fp + s.quote.fp, q.fn + s.quote.fn
        p.tp, p.fp, p.fn = p.tp + s.punct.tp, p.fp + s.punct.fp, p.fn + s.punct.fn
    lat = [s.latency_ms for s in scores if s.latency_ms is not None]
    n = max(1, len(scores))
    return {
        "n": len(scores),
        "exact": sum(s.exact for s in scores) / n,
        "word_acc": statistics.fmean(s.word_acc for s in scores) if scores else 0.0,
        "quote_ok": sum(s.quote_ok for s in scores) / n,
        "quote_f1": q.f1,
        "para_ok": sum(s.para_ok for s in scores) / n,
        "punct_f1": p.f1,
        "forbidden": float(sum(s.forbidden for s in scores)),
        "forbidden_cases": float(sum(s.forbidden > 0 for s in scores)),
        "p50_ms": pct(lat, 0.5),  # type: ignore[arg-type]
        "p95_ms": pct(lat, 0.95),  # type: ignore[arg-type]
    }


COLS = [
    ("n", "n", "{:.0f}"), ("exact", "exact", "{:.0%}"), ("word_acc", "word acc", "{:.1%}"),
    ("quote_ok", "quotes ok", "{:.0%}"), ("quote_f1", "quote F1", "{:.2f}"), ("para_ok", "breaks ok", "{:.0%}"),
    ("punct_f1", "punct F1", "{:.2f}"), ("forbidden", "forbidden words", "{:.0f}"),
    ("forbidden_cases", "cases w/ forbidden", "{:.0f}"), ("p50_ms", "p50 ms", "{:.0f}"), ("p95_ms", "p95 ms", "{:.0f}"),
]


def table(rows: list[tuple[str, dict[str, float]]]) -> str:
    head = "| | " + " | ".join(c[1] for c in COLS) + " |"
    sep = "|---|" + "---:|" * len(COLS)
    body = ["| " + name + " | " + " | ".join(fmt.format(m[k]) for k, _, fmt in COLS) + " |" for name, m in rows]
    return "\n".join([head, sep, *body])


def report(title: str, cases: dict[str, dict[str, str]], scores: list[CaseScore], outputs: dict[str, str],
           max_failures: int = 15) -> str:
    cats = sorted({s.category for s in scores})
    styles = sorted({s.style for s in scores})
    style_rows = [(f"_style {st}_", aggregate([s for s in scores if s.style == st])) for st in styles] if len(styles) > 1 else []
    rows = [("**overall**", aggregate(scores))] + style_rows + [(c, aggregate([s for s in scores if s.category == c])) for c in cats]
    lines = [f"# {title}", "", table(rows), ""]
    bad = [s for s in scores if not s.exact]
    bad.sort(key=lambda s: (-s.forbidden, s.quote_ok, s.para_ok, s.word_acc))
    if bad:
        lines += [f"## Failures (showing {min(max_failures, len(bad))} of {len(bad)})", ""]
        for s in bad[:max_failures]:
            flags = [f for f, ok in (("quotes", s.quote_ok), ("breaks", s.para_ok)) if not ok]
            if s.forbidden:
                flags.append(f"forbidden {s.added}")
            lines += [f"**{s.id}** ({s.category}) {', '.join(flags) or 'punctuation/case'}", "",
                      "```", "expected: " + cases[s.id]["expected"].replace("\n", "⏎"),
                      "output:   " + outputs.get(s.id, "").replace("\n", "⏎"), "```", ""]
    return "\n".join(lines)


def load_jsonl(path: Path) -> list[dict[str, object]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def score_files(cases_path: list[Path], outputs_path: Path) -> tuple[dict[str, dict[str, str]], list[CaseScore], dict[str, str]]:
    cases = {str(c["id"]): c for p in cases_path for c in load_jsonl(p)}  # type: ignore[misc]
    outs = {str(o["id"]): o for o in load_jsonl(outputs_path)}
    scores: list[CaseScore] = []
    for cid, case in cases.items():
        o = outs.get(cid)
        if o is None:
            continue
        lat = o.get("latency_ms")
        scores.append(score_case(case, str(o.get("output", "")), float(lat) if lat is not None else None))  # type: ignore[arg-type]
    return cases, scores, {k: str(v.get("output", "")) for k, v in outs.items()}  # type: ignore[return-value]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cases", type=Path, nargs="+", help="one or more cases JSONL files")
    ap.add_argument("outputs", type=Path)
    ap.add_argument("--report", type=Path, help="write a markdown report here")
    ap.add_argument("--details", type=Path, help="write per-case scores (JSONL) here")
    ap.add_argument("--title", default="Cleanup eval")
    ap.add_argument("--meta", default="", help="a line printed under the title (prompt, settings)")
    args = ap.parse_args()

    cases, scores, outputs = score_files(args.cases, args.outputs)
    missing = len(cases) - len(scores)
    md = report(args.title, cases, scores, outputs)
    if args.meta:
        head, _, rest = md.partition("\n")
        md = f"{head}\n\n{args.meta}\n{rest}"
    if missing:
        md += f"\n\n_{missing} case(s) had no output and were not scored._\n"
    print(table([("overall", aggregate(scores))]))
    if args.report:
        args.report.write_text(md + "\n", encoding="utf-8")
    if args.details:
        with args.details.open("w", encoding="utf-8") as f:
            for s in scores:
                f.write(json.dumps({"id": s.id, "category": s.category, "exact": s.exact, "word_acc": s.word_acc,
                                    "quote_ok": s.quote_ok, "para_ok": s.para_ok, "forbidden": s.forbidden,
                                    "added": s.added, "latency_ms": s.latency_ms}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
