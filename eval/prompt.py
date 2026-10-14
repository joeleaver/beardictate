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


@dataclass(frozen=True)
class Prompt:
    system: str
    user: str
    path: Path

    @classmethod
    def load(cls, path: Path | None = None) -> "Prompt":
        path = path or default_prompt_path()
        text = path.read_text(encoding="utf-8")
        if "=== SYSTEM ===" not in text or "=== USER ===" not in text:
            raise ValueError(f"{path}: expected '=== SYSTEM ===' and '=== USER ===' sections")
        system = text.split("=== SYSTEM ===", 1)[1].split("=== USER ===", 1)[0].strip("\n")
        user = text.split("=== USER ===", 1)[1].strip("\n")
        return cls(system, user, path)

    def messages(self, context: str, raw: str, style: str = "us") -> list[dict[str, str]]:
        ctx = trim_context(context)
        values = {"style_rule": style_rule(style), "context": ctx if ctx.strip() else EMPTY_CONTEXT, "raw": raw}
        return [{"role": "system", "content": substitute(self.system, values)},
                {"role": "user", "content": substitute(self.user, values)}]
