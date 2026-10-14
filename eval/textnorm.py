"""Text normalization and word alignment shared by the scorer, the case
builder and the training-pair generator.

A "word" is what the guardrail compares: lowercased, curly apostrophes
mapped to straight ones, surrounding punctuation and quote marks stripped,
and dashes treated as spaces (so "well-known" and "well known" compare
equal; hyphenation counts as punctuation).
"""

from __future__ import annotations

import re
from dataclasses import dataclass

OPEN_MARKS = "“‘"  # “ ‘
CLOSE_MARKS = "”’"  # ” ’
QUOTE_CHARS = '"“”‘’„«»'
DASHES = "—–-"
FILLERS = frozenset({"um", "uh", "er", "erm", "ah", "eh", "hmm", "mm", "mhm", "uhm", "umm", "uhh"})

_APOS = str.maketrans({"’": "'", "‘": "'", "ʼ": "'"})
_DASH_RE = re.compile(r"[—–]|(?<=\w)-(?=\w)|--+")


def words(text: str) -> list[str]:
    """Normalized word list of `text`."""
    out: list[str] = []
    for raw in _DASH_RE.sub(" ", text).split():
        w = raw.translate(_APOS).lower()
        w = w.strip(".,;:!?…\"'()[]{}*_" + QUOTE_CHARS + DASHES)
        if w:
            out.append(w)
    return out


@dataclass(frozen=True)
class Tok:
    """A word with its position in the original text and quote state."""

    word: str  # normalized
    surface: str  # as written (with attached punctuation)
    start: int  # char offset of surface in the text
    in_quote: bool  # inside a quotation when the word starts
    span: int = -1  # id of the outermost quotation the word is in (-1: none)
    brk: str = ""  # "para" / "line" if a paragraph / line break precedes the word
    punct: str = ""  # punctuation after the word, before the next word (no quote marks)


def lcs_pairs(a: list[str], b: list[str]) -> list[tuple[int, int]]:
    """Index pairs of a longest common subsequence of `a` and `b`."""
    n, m = len(a), len(b)
    dp = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n - 1, -1, -1):
        ai, row, nxt = a[i], dp[i], dp[i + 1]
        for j in range(m - 1, -1, -1):
            row[j] = nxt[j + 1] + 1 if ai == b[j] else max(nxt[j], row[j + 1])
    pairs: list[tuple[int, int]] = []
    i = j = 0
    while i < n and j < m:
        if a[i] == b[j]:
            pairs.append((i, j))
            i += 1
            j += 1
        elif dp[i + 1][j] >= dp[i][j + 1]:
            i += 1
        else:
            j += 1
    return pairs


def edit_distance(a: list[str], b: list[str]) -> int:
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i] + [0] * len(b)
        for j, y in enumerate(b, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y))
        prev = cur
    return prev[-1]


@dataclass
class EditCheck:
    """Word-level comparison of a cleaned output against its raw input."""

    added: list[str]  # output words with no counterpart in raw (additions or substitutions)
    deleted: list[str]  # raw words dropped
    suspicious_deletions: list[str]  # dropped words that are not fillers or nearby repeats

    @property
    def forbidden(self) -> int:
        return len(self.added)


def check_edits(raw: str, output: str, window: int = 6) -> EditCheck:
    """Which words `output` added or dropped relative to `raw`.

    Allowed: dropping fillers, repeats and false starts. A dropped word that
    is neither a filler nor equal to a word within `window` positions of it
    in raw is reported as suspicious (a likely content deletion).
    """
    rw, ow = words(raw), words(output)
    pairs = lcs_pairs(rw, ow)
    kept_r = {i for i, _ in pairs}
    kept_o = {j for _, j in pairs}
    added = [w for j, w in enumerate(ow) if j not in kept_o]
    deleted, suspicious = [], []
    for i, w in enumerate(rw):
        if i in kept_r:
            continue
        deleted.append(w)
        near = rw[max(0, i - window) : i] + rw[i + 1 : i + 1 + window]
        if w not in FILLERS and w not in near:
            suspicious.append(w)
    return EditCheck(added, deleted, suspicious)


PUNCT_MARKS = ",.;:!?\u2014\u2026"


@dataclass
class Tokenized:
    toks: list[Tok]
    trailing_break: str  # "para" / "line" / "" after the last word


def tokenize(text: str, start_in_quote: bool = False) -> Tokenized:
    """Words of `text` with quote, paragraph-break and punctuation info.

    Opening marks are “ and ‘; closing marks are ” and ’ (a ’ between
    letters is an apostrophe, as is one after a letter when no quote is
    open). Straight " toggles. Nested quotes belong to the outer span. A
    paragraph that starts with an opening mark keeps a dangling quote open
    (multi-paragraph speech); one that does not closes it. "..." counts as …
    """
    text = text.replace("...", "\u2026").replace("--", "\u2014")
    out: list[Tok] = []
    depth = 1 if start_in_quote else 0
    span = 0 if start_in_quote else -1
    next_span = 1
    n = len(text)
    k = 0
    word_start = -1
    word_quote = False
    word_span = -1
    pending_brk = ""
    punct = ""

    def flush(end: int) -> None:
        nonlocal word_start, pending_brk
        if word_start >= 0:
            ws = words(text[word_start:end])
            for i, w in enumerate(ws):
                out.append(Tok(w, text[word_start:end], word_start, word_quote,
                               word_span if word_quote else -1, pending_brk if i == 0 else ""))
            pending_brk = ""
        word_start = -1

    def add_punct(ch: str) -> None:
        if out and word_start < 0:
            last = out[-1]
            out[-1] = Tok(last.word, last.surface, last.start, last.in_quote, last.span, last.brk, last.punct + ch)

    while k < n:
        ch = text[k]
        prev_alnum = k > 0 and text[k - 1].isalnum()
        next_alnum = k + 1 < n and text[k + 1].isalnum()
        if ch.isalnum() or (ch in "'\u2019" and prev_alnum and next_alnum) or (ch == "'" and prev_alnum):
            if word_start < 0:
                word_start = k
                word_quote = depth > 0
                word_span = span
            k += 1
            continue
        flush(k)
        if ch == "\n":
            j = k
            while j < n and text[j] in "\n \t":
                j += 1
            pending_brk = "para" if text[k:j].count("\n") >= 2 else "line"
            if j < n and text[j] not in OPEN_MARKS + '"' + CLOSE_MARKS:
                depth, span = 0, -1
            k = j
            continue
        if ch in OPEN_MARKS and not (ch == "\u2018" and prev_alnum):
            if depth == 0:
                span, next_span = next_span, next_span + 1
            depth += 1
        elif ch in CLOSE_MARKS:
            if depth > 0:
                depth -= 1
                if depth == 0:
                    span = -1
        elif ch == '"':
            if depth:
                depth, span = 0, -1
            else:
                depth, span, next_span = 1, next_span, next_span + 1
        elif ch in PUNCT_MARKS:
            add_punct(ch)
        k += 1
    flush(n)
    return Tokenized(out, pending_brk)


def tokens_with_quotes(text: str, start_in_quote: bool = False) -> list[Tok]:
    return tokenize(text, start_in_quote).toks


def ends_in_open_quote(context: str) -> bool:
    """True if the context ends inside a quotation (mid-quote continuation)."""
    if not context.strip():
        return False
    last_para = context.rstrip("\n").split("\n")[-1]
    depth = 0
    for k, ch in enumerate(last_para):
        prev_alpha = k > 0 and last_para[k - 1].isalnum()
        next_alpha = k + 1 < len(last_para) and last_para[k + 1].isalnum()
        if ch in OPEN_MARKS and not (prev_alpha and next_alpha):
            depth += 1
        elif ch in CLOSE_MARKS and not (prev_alpha and next_alpha) and depth > 0:
            depth -= 1
        elif ch == '"':
            depth = 0 if depth else 1
    return depth > 0
