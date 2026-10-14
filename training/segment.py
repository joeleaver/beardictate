"""Cut a book into dictation segments, the way an author pauses.

A segment is 1-4 consecutive "units" (sentences, or clauses at a comma when
an author would plausibly pause mid-sentence), sometimes running across a
paragraph boundary (a quick back-and-forth). For each segment we produce:

  context   the last ~2 paragraphs of what the app would already have typed
  expected  the segment as it should be typed (may start with "\\n\\n")

Quote convention (shared with the eval and the runtime prompt): a quotation
is closed at the end of every segment unless the segment stops mid-sentence.
When the original text continues the same speech after a sentence-final
segment boundary, the expected text closes the quote and the next segment
reopens it.
"""

from __future__ import annotations

import random
import re
from dataclasses import dataclass
from typing import Iterator

OPEN, CLOSE = "“", "”"
# Sentence end: terminal punctuation, optional closing quote(s), then space and
# an uppercase letter or an opening quote.
SENT_RE = re.compile(r"(?<=[.?!…])[”’]*\s+(?=[“‘]?[A-Z])")
ABBREV_RE = re.compile(r"\b(Mr|Mrs|Ms|Dr|St|Mt|Jr|Sr|Capt|Col|Gen|Rev|No)\.$")
BAD_CHARS = re.compile(r"[\[\]{}<>|#*_=+\\/@^~`\d]{1}|[^\x00-\x7f‘’“”—…À-ſ]")


@dataclass
class Unit:
    text: str
    para: int
    first_in_para: bool
    depth_before: int  # quote depth at the start of the unit (paragraph-local)
    depth_after: int


@dataclass
class Segment:
    context: str
    expected: str
    in_quote_at_start: bool
    crosses_paragraph: bool
    clause_split: bool


def quote_depth(text: str, depth: int = 0) -> int:
    for ch in text:
        if ch == OPEN:
            depth += 1
        elif ch == CLOSE and depth > 0:
            depth -= 1
    return depth


def balanced(par: str) -> bool:
    return par.count(OPEN) == par.count(CLOSE) and quote_depth(par) == 0


def split_sentences(par: str) -> list[str]:
    pieces: list[str] = []
    last = 0
    for m in SENT_RE.finditer(par):
        head = par[last : m.start()]
        if ABBREV_RE.search(head.rstrip()):
            continue
        pieces.append(par[last : m.end()].strip())
        last = m.end()
    pieces.append(par[last:].strip())
    return [p for p in pieces if p]


def split_clause(sent: str, rng: random.Random, min_words: int = 6) -> list[str]:
    """Split a long sentence once at a comma, as a mid-sentence pause."""
    spots = [m.end() for m in re.finditer(r",[”’]? ", sent)]
    good = [s for s in spots if len(sent[:s].split()) >= min_words and len(sent[s:].split()) >= 4]
    if not good:
        return [sent]
    cut = rng.choice(good)
    return [sent[:cut].strip(), sent[cut:].strip()]


def units_of(pars: list[str], rng: random.Random, clause_rate: float) -> list[Unit]:
    units: list[Unit] = []
    for pi, par in enumerate(pars):
        depth = 0
        for si, sent in enumerate(split_sentences(par)):
            parts = split_clause(sent, rng) if len(sent.split()) > 14 and rng.random() < clause_rate else [sent]
            for k, part in enumerate(parts):
                after = quote_depth(part, depth)
                units.append(Unit(part, pi, si == 0 and k == 0, depth, after))
                depth = after
    return units


def tail_context(typed: list[str], max_chars: int) -> str:
    """Last two paragraphs of the typed text, trimmed from the left at a sentence start."""
    text = "".join(typed).lstrip("\n")
    paras = text.split("\n\n")
    ctx = "\n\n".join(paras[-2:])
    if len(ctx) > max_chars:
        cut = ctx[-max_chars:]
        m = re.search(r"(?<=[.?!][” ])\s*(?=[“A-Z])", cut)
        ctx = cut[m.end():] if m else cut[cut.find(" ") + 1 :]
    return ctx


def segments(pars: list[str], rng: random.Random, *, clause_rate: float = 0.08, cross_rate: float = 0.35,
             max_words: int = 70, max_context_chars: int = 900) -> Iterator[Segment]:
    """Yield segments over the whole book in order (context accumulates)."""
    units = units_of(pars, rng, clause_rate)
    typed: list[str] = []  # what the app has typed so far
    reopen = False  # previous segment closed a quote the original kept open
    i = 0
    while i < len(units):
        want = rng.choices([1, 2, 3, 4], [0.42, 0.33, 0.17, 0.08])[0]
        seg = [units[i]]
        j = i + 1
        while j < len(units) and len(seg) < want:
            nxt = units[j]
            if nxt.para != seg[-1].para and not (rng.random() < cross_rate and len(nxt.text.split()) < 25):
                break
            if sum(len(u.text.split()) for u in seg) + len(nxt.text.split()) > max_words:
                break
            seg.append(nxt)
            j += 1

        context = tail_context(typed, max_context_chars)
        pieces: list[str] = []
        for k, u in enumerate(seg):
            sep = "" if k == 0 else ("\n\n" if u.para != seg[k - 1].para else " ")
            pieces.append(sep + u.text)
        body = "".join(pieces)
        first = seg[0]
        reopened = reopen and not first.first_in_para and first.depth_before > 0
        if reopened:
            body = OPEN + body
        last = seg[-1]
        mid_sentence = bool(re.search(r"[,;:—][”’]?$", last.text))
        close_added = last.depth_after > 0 and not mid_sentence and not last.text.endswith(CLOSE)
        if close_added:
            body += CLOSE
        reopen = close_added
        lead = "\n\n" if first.first_in_para and typed else ""
        expected = lead + body
        # Continuation: the context ends in an open quote only if the previous
        # segment stopped mid-sentence inside the quote.
        in_quote = first.depth_before > 0 and not first.first_in_para and not reopened
        ok = all(balanced(pars[p]) for p in {u.para for u in seg}) and not BAD_CHARS.search(body)
        if ok:
            yield Segment(context, expected, in_quote,
                          len({u.para for u in seg}) > 1, any(not re.search(r"[.?!…][”’]*$", u.text) for u in seg))
        sep = (" " if typed and not lead else "")
        typed.append(sep + expected if not expected.startswith("\n") else expected)
        i = j
