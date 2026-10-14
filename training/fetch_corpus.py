#!/usr/bin/env python3
"""Fetch a modest set of public-domain novels from Project Gutenberg.

Politeness: one request at a time, a delay between requests, a descriptive
User-Agent, and an on-disk cache so a book is never fetched twice.

Output layout (default root /mnt/data/beardictate-dev/corpus):
  raw/pg<ID>.txt      exactly as downloaded (cache)
  clean/pg<ID>.txt    header/footer stripped, unwrapped into one paragraph
                      per line, quotes normalized (curly double quotes,
                      straight apostrophes), italics/illustrations removed
  manifest.jsonl      one row per book: id, title, author, death year,
                      dialog style detected, paragraph/word counts, status
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from pathlib import Path

from corpus_books import BOOKS, Book

USER_AGENT = "beardictate-corpus/0.1 (+https://github.com/joeleaver/beardictate; offline dictation research)"
URL = "https://www.gutenberg.org/cache/epub/{id}/pg{id}.txt"

START_RE = re.compile(r"^\*\*\* ?START OF (THE|THIS) PROJECT GUTENBERG EBOOK.*$", re.M | re.I)
END_RE = re.compile(r"^\*\*\* ?END OF (THE|THIS) PROJECT GUTENBERG EBOOK.*$", re.M | re.I)


@dataclass
class ManifestRow:
    id: int
    expected_title: str
    title: str
    author: str
    died: int
    style: str
    paragraphs: int
    words: int
    status: str


def fetch(book_id: int, raw_dir: Path, delay: float) -> tuple[str, bool]:
    """Return (text, fetched_now). Uses the cache when present."""
    path = raw_dir / f"pg{book_id}.txt"
    if path.exists() and path.stat().st_size > 0:
        return path.read_text(encoding="utf-8", errors="replace"), False
    req = urllib.request.Request(URL.format(id=book_id), headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=60) as resp:
        data = resp.read()
    text = data.decode("utf-8", errors="replace")
    tmp = path.with_suffix(".part")
    tmp.write_text(text, encoding="utf-8")
    tmp.rename(path)
    time.sleep(delay)
    return text, True


def header_field(text: str, name: str) -> str:
    m = re.search(rf"^{name}:\s*(.+)$", text[:5000], re.M)
    return m.group(1).strip() if m else ""


def strip_boilerplate(text: str) -> str:
    text = text.replace("\r\n", "\n").lstrip("﻿")
    s = START_RE.search(text)
    e = END_RE.search(text)
    body = text[s.end() if s else 0 : e.start() if e else len(text)]
    # Producer credits sometimes sit right after the START line.
    body = re.sub(r"^\s*(Produced|Transcribed|E-text prepared) by[^\n]*(\n[^\n]+)*\n", "", body, flags=re.I)
    return body


HEADING_RE = re.compile(
    r"^(chapter|book|part|volume|vol\.|stave|letter|preface|contents|the end|finis)\b.*$|^[IVXLC]+\.?$|^[\dIVXLC]+\.\s*$",
    re.I,
)


def is_heading(par: str) -> bool:
    if HEADING_RE.match(par):
        return True
    letters = [c for c in par if c.isalpha()]
    # ALL-CAPS short lines are headings/titles.
    return len(par) < 80 and bool(letters) and all(c.isupper() for c in letters)


def normalize_double_quotes(par: str) -> str:
    """Straight " -> curly, by position; existing curly quotes are kept."""
    out: list[str] = []
    for i, ch in enumerate(par):
        if ch != '"':
            out.append(ch)
            continue
        prev = par[i - 1] if i > 0 else " "
        opening = prev.isspace() or prev in "([{—-–‘"
        out.append("“" if opening else "”")
    return "".join(out)


ARCHAIC = {
    "to-day": "today", "To-day": "Today", "to-morrow": "tomorrow", "To-morrow": "Tomorrow",
    "to-night": "tonight", "To-night": "Tonight", "good-bye": "goodbye", "Good-bye": "Goodbye",
}


def clean_paragraph(par: str) -> str:
    par = " ".join(par.split())
    par = re.sub(r"\[(Illustration|Footnote|Sidenote)[^\]]*\]", "", par, flags=re.I)
    par = re.sub(r"_([^_]+)_", r"\1", par)  # _italics_
    par = par.replace("_", "")
    par = re.sub(r"\s*--+\s*", "—", par)
    par = par.replace("— ", "—").replace(" —", "—")
    par = par.replace("``", '"').replace("''", '"')
    par = normalize_double_quotes(par)
    # Apostrophes stay straight (that is what the ASR emits); a ’ between
    # letters, or after s at a word end, is an apostrophe.
    par = re.sub(r"(?<=\w)’(?=\w)", "'", par)
    par = re.sub(r"(?<=s)’(?=[\s,.;:!?—]|$)", "'", par)
    for k, v in ARCHAIC.items():
        par = re.sub(rf"\b{re.escape(k)}\b", v, par)
    return " ".join(par.split())


def detect_style(pars: list[str]) -> str:
    """'us' if dialog is in double quotes, 'uk' if in single quotes."""
    dbl = sum(p.count("“") for p in pars)
    sgl = sum(len(re.findall(r"(^|\s)[‘']", p)) for p in pars)
    return "us" if dbl >= sgl else "uk"


CHAPTER_RE = re.compile(r"^(chapter|stave|book)\s+(the\s+)?(i|1|one|first)\b", re.I)


def skip_front_matter(blocks: list[str]) -> list[str]:
    """Drop prefaces, contents and introductions before the first chapter.

    Tables of contents also list "Chapter I", so take the first chapter-one
    heading (within the first 20% of the book) that is followed by real
    prose rather than by more headings.
    """
    limit = max(1, len(blocks) // 5)
    for i, b in enumerate(blocks[:limit]):
        if not CHAPTER_RE.match(b.strip()):
            continue
        following = blocks[i + 1 : i + 4]
        if any(len(f.split()) >= 40 for f in following):
            return blocks[i + 1 :]
    return blocks


def to_paragraphs(body: str) -> list[str]:
    pars: list[str] = []
    blocks = [" ".join(ln.strip() for ln in b.split("\n") if ln.strip()) for b in re.split(r"\n\s*\n", body)]
    blocks = skip_front_matter([b for b in blocks if b])
    for block in blocks:
        lines = [block]
        par = clean_paragraph(" ".join(lines))
        if not par or is_heading(par) or len(par.split()) < 3:
            continue
        pars.append(par)
    return pars


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", type=Path, default=Path("/mnt/data/beardictate-dev/corpus"))
    ap.add_argument("--delay", type=float, default=3.0, help="seconds between network requests")
    ap.add_argument("--max-died", type=int, default=1915, help="only authors who died in or before this year")
    ap.add_argument("--limit", type=int, default=0, help="fetch at most N books (0 = all)")
    args = ap.parse_args()

    raw_dir, clean_dir = args.root / "raw", args.root / "clean"
    raw_dir.mkdir(parents=True, exist_ok=True)
    clean_dir.mkdir(parents=True, exist_ok=True)

    books: list[Book] = [b for b in BOOKS if b.died <= args.max_died]
    if args.limit:
        books = books[: args.limit]
    rows: list[ManifestRow] = []
    for b in books:
        try:
            text, fetched = fetch(b.id, raw_dir, args.delay)
        except (urllib.error.URLError, TimeoutError) as e:
            print(f"pg{b.id}: FETCH FAILED {e}", file=sys.stderr)
            rows.append(ManifestRow(b.id, b.title, "", "", b.died, "", 0, 0, f"fetch-failed: {e}"))
            continue
        title, author = header_field(text, "Title"), header_field(text, "Author")
        pars = to_paragraphs(strip_boilerplate(text))
        style = detect_style(pars)
        key = b.title.lower().split()[0 if b.title.lower().split()[0] not in ("the", "a") else 1]
        status = "ok" if key in title.lower() else "title-mismatch"
        (clean_dir / f"pg{b.id}.txt").write_text("\n".join(pars) + "\n", encoding="utf-8")
        words = sum(len(p.split()) for p in pars)
        rows.append(ManifestRow(b.id, b.title, title, author, b.died, style, len(pars), words, status))
        print(f"pg{b.id:<6} {'net' if fetched else 'cache'} {status:<14} {style} {words:>7}w  {title} / {author}")

    with (args.root / "manifest.jsonl").open("w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(asdict(r), ensure_ascii=False) + "\n")
    ok = [r for r in rows if r.status == "ok"]
    print(f"\n{len(ok)}/{len(rows)} books ok, {sum(r.words for r in ok):,} words; "
          f"styles: us={sum(r.style == 'us' for r in ok)} uk={sum(r.style == 'uk' for r in ok)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
