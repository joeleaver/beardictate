"""Python mirror of crates/bd-commands (the deterministic spoken-command pass).

The runtime runs this pass on ASR text BEFORE the cleanup LLM, so the LLM
sees quote marks and breaks, never the words "open quote". run_eval.py and
training/make_pairs.py use this mirror to feed models the same input the
runtime would. Only the structural commands are mirrored. Literal punctuation
("comma", "period") is off by default in the runtime and is not mirrored.

Kept in sync with the Rust test vectors by eval/tests/test_commands.py, which
parses crates/bd-commands/src/tests.rs and tests_prose.rs directly.
"""

from __future__ import annotations

from dataclasses import dataclass, field

OPEN, CLOSE, PARA, LINE, SCRATCH = "open", "close", "para", "line", "scratch"

# Longest phrases first, so "end of quote" wins over a shorter prefix.
PHRASES: list[tuple[tuple[str, ...], str]] = [
    (("end", "of", "quote"), CLOSE),
    (("open", "quote"), OPEN),
    (("open", "quotes"), OPEN),
    (("begin", "quote"), OPEN),
    (("close", "quote"), CLOSE),
    (("close", "quotes"), CLOSE),
    (("end", "quote"), CLOSE),
    (("end", "quotes"), CLOSE),
    (("unquote",), CLOSE),
    (("new", "paragraph"), PARA),
    (("new", "line"), LINE),
    (("scratch", "that"), SCRATCH),
]
PHRASE_SET = {p for p, _ in PHRASES}

QUOTE_BLOCKERS = {"a", "an", "the", "this", "these", "those", "each", "every", "another", "no", "any"}
NOUN_BLOCKERS = {
    "a", "an", "the", "this", "that", "these", "those", "my", "your", "his", "her", "its",
    "our", "their", "each", "every", "another", "no", "any", "some", "one", "whose", "which",
    "what", "first", "last", "next",
}
MODIFIERS = {
    "whole", "brand", "entire", "single", "second", "third", "final", "fresh", "short", "long",
    "separate", "very", "lone", "stray", "extra", "missing", "same", "little", "tiny",
}
NOUN_FOLLOWERS = {
    "of", "mark", "marks", "character", "characters", "symbol", "symbols", "splice", "splices",
    "break", "breaks", "drama", "dramas", "piece", "pieces", "key", "keys",
}
BARE_QUOTE_BLOCKERS = {
    "to", "a", "an", "the", "this", "that", "his", "her", "my", "your", "our", "their", "its",
    "one", "favorite", "favourite", "famous",
}
ASR_PUNCT = ",.;:!?…"


@dataclass
class Token:
    lead: str
    core: str
    trail: str

    @property
    def lower(self) -> str:
        return self.core.replace("’", "'").replace("‘", "'").replace("ʼ", "'").lower()

    @property
    def text(self) -> str:
        return self.lead + self.core + self.trail

    @property
    def bare(self) -> bool:
        return not self.core

    def ends_sentence(self) -> bool:
        return any(c in ".!?…" for c in self.trail)


def joined(a: Token, b: Token) -> bool:
    return not a.trail and not b.lead and not a.bare and not b.bare


def _split(word: str) -> Token:
    idx = [i for i, c in enumerate(word) if c.isalnum()]
    if not idx:
        return Token(word, "", "")
    return Token(word[: idx[0]], word[idx[0] : idx[-1] + 1], word[idx[-1] + 1 :])


def tokenize(text: str) -> list[Token]:
    out: list[Token] = []
    for word in text.split():
        tok = _split(word)
        parts = tok.lower.split("-")
        if len(parts) > 1 and all(parts) and tuple(parts) in PHRASE_SET:
            cores = tok.core.split("-")
            for k, part in enumerate(cores):
                out.append(Token(tok.lead if k == 0 else "", part, tok.trail if k == len(cores) - 1 else ""))
        else:
            out.append(tok)
    return out


def _prev_words(toks: list[Token], i: int, n: int) -> list[str]:
    out: list[str] = []
    k = i
    while k > 0 and len(out) < n and joined(toks[k - 1], toks[k]):
        out.append(toks[k - 1].lower)
        k -= 1
    return out


def _noun_blocked(toks: list[Token], i: int) -> bool:
    prev = _prev_words(toks, i, 2)
    if prev and prev[0] in NOUN_BLOCKERS:
        return True
    return len(prev) == 2 and prev[0] in MODIFIERS and prev[1] in NOUN_BLOCKERS


def _preceded_by_bare_quote(toks: list[Token], i: int) -> bool:
    if i == 0 or toks[i - 1].lower != "quote":
        return False
    p = toks[i - 1]
    return (not p.lead and joined(p, toks[i])) or p.trail == ","


def _words_match(toks: list[Token], i: int, words: tuple[str, ...]) -> bool:
    if i + len(words) > len(toks):
        return False
    span = toks[i : i + len(words)]
    return all(t.lower == w for t, w in zip(span, words)) and all(
        joined(span[k], span[k + 1]) for k in range(len(span) - 1)
    )


def _context_ok(toks: list[Token], i: int, words: tuple[str, ...], cmd: str, after_command: bool) -> bool:
    end = i + len(words)
    first, last = toks[i], toks[end - 1]
    nxt = toks[end] if end < len(toks) and joined(last, toks[end]) else None
    next_is_noun = nxt is not None and nxt.lower in NOUN_FOLLOWERS
    if cmd in (OPEN, CLOSE):
        blocked = any(w in QUOTE_BLOCKERS for w in _prev_words(toks, i, 1))
        return not blocked and not next_is_noun and not (words == ("unquote",) and _preceded_by_bare_quote(toks, i))
    if cmd in (PARA, LINE):
        return not _noun_blocked(toks, i) and not next_is_noun
    # scratch that
    left = i == 0 or after_command or toks[i - 1].ends_sentence() or toks[i - 1].bare
    right = (
        end == len(toks) or bool(last.trail) or toks[end].bare or match_at(toks, end, True) is not None
    )
    return left and right and not first.lead


def match_at(toks: list[Token], i: int, after_command: bool) -> tuple[str, int] | None:
    for words, cmd in PHRASES:
        if _words_match(toks, i, words) and _context_ok(toks, i, words, cmd, after_command):
            return cmd, len(words)
    return None


@dataclass
class Item:
    tok: Token | None = None  # a kept word
    cmd: str = ""  # or a command
    trail: str = ""


@dataclass
class CommandOutput:
    text: str
    quote_open: bool
    scratch_previous: int = 0
    actions: list[str] = field(default_factory=list)


def _can_open_bare(tok: Token, prev: Token | None) -> bool:
    return tok.lower == "quote" and not tok.lead and not (
        prev is not None and joined(prev, tok) and prev.lower in BARE_QUOTE_BLOCKERS
    )


def _resolve_bare_quotes(items: list[Item], quote_open: bool) -> None:
    start, is_open = 0, quote_open
    for k, it in enumerate(items):
        if it.cmd == OPEN:
            is_open = True
        elif it.cmd == CLOSE:
            if not is_open:
                for q in range(k - 1, start - 1, -1):
                    t = items[q].tok
                    if t is None:
                        continue
                    prev = items[q - 1].tok if q > 0 else None
                    nxt = items[q + 1].tok if q + 1 < len(items) else None
                    if _can_open_bare(t, prev) and not (nxt is not None and nxt.lower == "unquote"):
                        items[q] = Item(cmd=OPEN)
                        break
            is_open = False
            start = k + 1


def _end_sentence(out: str, mark: str) -> str:
    if not out or out[-1] in ".!?…" or not (out[-1].isalnum() or out[-1] in ",;:"):
        return out
    return out.rstrip(",;:") + mark


def _cap_first(word: str) -> str:
    for k, c in enumerate(word):
        if c.isalpha():
            return word[:k] + c.upper() + word[k + 1 :]
    return word


def _render(items: list[Item], marks: tuple[str, str]) -> tuple[str, list[str]]:
    out, actions = "", []
    space = capitalize = last_word = False
    for it in items:
        if it.tok is not None:
            text = it.tok.text
            out += (" " if space else "") + (_cap_first(text) if capitalize else text)
            if capitalize:
                capitalize = not any(c.isalpha() for c in text)
            space = last_word = True
            continue
        terminal = next((c for c in it.trail if c in ".!?"), None)
        if it.cmd == OPEN:
            out += (" " if space else "") + marks[0]
            space = False
            actions.append(OPEN)
        elif it.cmd == CLOSE:
            if terminal and last_word:
                out = _end_sentence(out, terminal)
            out = out.rstrip(" ") + marks[1]
            space = True
            actions.append(CLOSE)
        else:
            if terminal and last_word:
                out = _end_sentence(out, terminal)
            out = out.rstrip(" ") + ("\n\n" if it.cmd == PARA else "\n")
            space, capitalize = False, True
            actions.append(it.cmd)
        last_word = False
    return out, actions


def apply(asr_text: str, quote_open: bool = False, marks: tuple[str, str] = ("“", "”")) -> CommandOutput:
    """Apply spoken commands to one segment, like bd_commands::CommandPass::apply."""
    toks = tokenize(asr_text)
    items: list[Item] = []
    scratch_prev = 0
    i = 0
    while i < len(toks):
        after_command = bool(items) and items[-1].tok is None
        m = match_at(toks, i, after_command)
        if m is not None:
            cmd, n = m
            if cmd == SCRATCH:
                if items:
                    items.clear()
                else:
                    scratch_prev += 1
            else:
                items.append(Item(cmd=cmd, trail=toks[i + n - 1].trail))
            i += n
            continue
        items.append(Item(tok=toks[i]))
        i += 1
    _resolve_bare_quotes(items, quote_open)
    text, actions = _render(items, marks)
    is_open = quote_open
    for a in actions:
        if a == OPEN:
            is_open = True
        elif a == CLOSE:
            is_open = False
    return CommandOutput(text, is_open, scratch_prev, actions)


def context_quote_open(context: str, marks: tuple[str, str] = ("“", "”")) -> bool:
    """Whether the context's last paragraph ends inside an open quotation."""
    last = context.rstrip("\n").split("\n")[-1] if context.strip() else ""
    return last.count(marks[0]) > last.count(marks[1]) if marks[0] != marks[1] else last.count(marks[0]) % 2 == 1
