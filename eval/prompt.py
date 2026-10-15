"""Load a cleanup prompt file and build chat messages from it.

Prompt file format (shared with crates/bd-cleanup, which loads the same file
with include_str!):

    === SYSTEM ===
    ...system text, may use {style_rule}...
    === USER ===
    ...user text with {context} and {raw}...

An empty context becomes the literal "(none: this is the start of the
passage)", as in the runtime.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
RUNTIME_PROMPT = REPO / "crates" / "bd-cleanup" / "prompts" / "generic-v1.txt"
BASELINE_PROMPT = REPO / "eval" / "prompts" / "generic-v1.txt"
EMPTY_CONTEXT = "(none: this is the start of the passage)"

MAX_CONTEXT_CHARS = 1200  # bd_cleanup::PromptOptions default


def style_rule(style: str) -> str:
    """Mirror of bd_cleanup::prompt::style_rule (us = curly double + inside,
    uk = curly single + logical)."""
    if style == "uk":
        o, c, kind = "\u2018", "\u2019", "curly single quotation marks"
        placement = (f"A comma or period goes inside the closing mark only when it belongs to the speech, as with a "
                     f"dialog tag: {o}To the river,{c} he said. A quoted fragment keeps it outside: she called it {o}the end{c}.")
    else:
        o, c, kind = "\u201c", "\u201d", "curly double quotation marks"
        placement = f"A comma or period that ends a quoted line always goes inside the closing mark: {o}To the river,{c} he said."
    return (f"Quote dialog with {kind}: {o} to open and {c} to close. "
            f"Write apostrophes as plain ' (don't, Mara's). {placement}")


def trim_context(context: str, max_chars: int = MAX_CONTEXT_CHARS) -> str:
    """Mirror of bd_cleanup::prompt::trim_context."""
    if len(context) <= max_chars:
        return context
    tail = context[len(context) - max_chars :]
    i = tail.find("\n\n")
    cut = i + 2 if i >= 0 else (tail.find(". ") + 2 if tail.find(". ") >= 0 else 0)
    return tail[cut:].lstrip()


def substitute(text: str, values: dict[str, str]) -> str:
    """Single-pass {name} substitution, like the Rust `substitute`."""
    return re.sub(r"\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)


def default_prompt_path() -> Path:
    """The runtime prompt when bd-cleanup has published one, else the eval baseline."""
    return RUNTIME_PROMPT if RUNTIME_PROMPT.exists() else BASELINE_PROMPT


MARKS = {"us": ("\u201c", "\u201d"), "uk": ("\u2018", "\u2019")}
ROLES = {"=== SYSTEM ===": "system", "=== USER ===": "user", "=== ASSISTANT ===": "assistant"}


@dataclass(frozen=True)
class Prompt:
    """Patched mirror of bd_cleanup::prompt: any number of SYSTEM/USER/ASSISTANT
    sections, and {open}/{close} substituted."""
    sections: tuple
    path: Path

    @classmethod
    def load(cls, path: Path | None = None) -> "Prompt":
        path = path or default_prompt_path()
        out, role, buf = [], None, []
        for line in path.read_text(encoding="utf-8").split("\n"):
            if line in ROLES:
                if role:
                    out.append((role, "\n".join(buf)))
                role, buf = ROLES[line], []
            else:
                buf.append(line)
        if role:
            out.append((role, "\n".join(buf)))
        secs = tuple((r, t.rstrip("\n") if r == "assistant" else t.strip("\n")) for r, t in out)
        return cls(secs, path)

    def messages(self, context: str, raw: str, style: str = "us") -> list[dict[str, str]]:
        ctx = trim_context(context)
        o, c = MARKS[style]
        values = {"style_rule": style_rule(style), "context": ctx if ctx.strip() else EMPTY_CONTEXT, "raw": raw,
                  "open": o, "close": c}
        return [{"role": r, "content": substitute(t, values)} for r, t in self.sections]
