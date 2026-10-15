#!/usr/bin/env python3
"""Build cases.jsonl / cases_uk.jsonl from the readable sources in cases_src/,
and validate every case against the word-level guardrail.

Source format (one block per case):

    === <id> | <category>
    @context            (optional; previous cleaned output)
    ...
    @raw                (Parakeet-style ASR text, may contain spoken commands)
    ...
    @expected
    ...
    @notes              (optional)
    ...

In @expected, a first line that is just "¶" means the output starts with a
paragraph break ("\\n\\n"); a last line "¶" means it ends with one.

Validation: after the command pass (commands.py, a mirror of bd-commands),
every expected word must be a raw word, in order (no additions or
substitutions; this is a hard error). Dropped words that are neither fillers
nor nearby repeats are listed for review (false starts are legitimate).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import commands
from textnorm import check_edits, ends_in_open_quote

HERE = Path(__file__).resolve().parent
SECTIONS = ("context", "raw", "expected", "notes")
MARKS = {"us": ("“", "”"), "uk": ("‘", "’")}


def parse(text: str) -> list[dict[str, str]]:
    cases: list[dict[str, str]] = []
    for block in re.split(r"^=== ", text, flags=re.M)[1:]:
        header, _, body = block.partition("\n")
        cid, _, cat = (x.strip() for x in header.partition("|"))
        case: dict[str, str] = {"id": cid, "category": cat, "context": ""}
        for m in re.finditer(r"^@(\w+)\n(.*?)(?=^@\w+\n|\Z)", body, flags=re.M | re.S):
            name, val = m.group(1), m.group(2)
            if name not in SECTIONS:
                raise ValueError(f"{cid}: unknown section @{name}")
            lines = val.rstrip("\n").split("\n")
            while lines and not lines[-1].strip():
                lines.pop()
            if name == "expected":
                lead = bool(lines) and lines[0] == "¶"
                trail = len(lines) > 1 and lines[-1] == "¶"
                core = lines[1 if lead else 0 : -1 if trail else None]
                val = ("\n\n" if lead else "") + "\n".join(core) + ("\n\n" if trail else "")
            else:
                val = "\n".join(lines)
            case[name] = val
        for req in ("raw", "expected"):
            if req not in case:
                raise ValueError(f"{cid}: missing @{req}")
        cases.append(case)
    return cases


def command_pass(case: dict[str, str], style: str) -> str:
    marks = MARKS[style]
    return commands.apply(case["raw"], ends_in_open_quote(case["context"]), marks).text


def validate(cases: list[dict[str, str]], style: str) -> int:
    errors = 0
    seen: set[str] = set()
    for c in cases:
        if c["id"] in seen:
            print(f"ERROR {c['id']}: duplicate id")
            errors += 1
        seen.add(c["id"])
        chk = check_edits(command_pass(c, style), c["expected"])
        if chk.added:
            print(f"ERROR {c['id']}: expected adds/substitutes words {chk.added}")
            errors += 1
        if chk.suspicious_deletions:
            print(f"review {c['id']}: drops {chk.suspicious_deletions}  ({c.get('notes', '')})")
    return errors


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", type=Path, default=HERE / "cases_src")
    ap.add_argument("--out", type=Path, default=HERE)
    args = ap.parse_args()

    groups = {
        "cases.jsonl": ("us", sorted(args.src.glob("us_*.txt"))),
        "cases_uk.jsonl": ("uk", [args.src / "uk.txt"]),
        # Narration only: counts quotes added where nobody speaks.
        "cases_narr.jsonl": ("us", [args.src / "narr.txt"]),
    }
    errors = 0
    for out_name, (style, files) in groups.items():
        cases = [c for f in files for c in parse(f.read_text(encoding="utf-8"))]
        if style == "uk":
            for c in cases:
                c["style"] = "uk"
        errors += validate(cases, style)
        with (args.out / out_name).open("w", encoding="utf-8") as f:
            for c in cases:
                f.write(json.dumps(c, ensure_ascii=False) + "\n")
        cats: dict[str, int] = {}
        for c in cases:
            cats[c["category"]] = cats.get(c["category"], 0) + 1
        print(f"{out_name}: {len(cases)} cases  " + ", ".join(f"{k}={v}" for k, v in sorted(cats.items())))
    if errors:
        print(f"{errors} error(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
