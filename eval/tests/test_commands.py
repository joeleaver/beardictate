"""Checks the Python command-pass mirror against the Rust test vectors."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

import commands

SRC = Path(__file__).resolve().parents[2] / "crates" / "bd-commands" / "src"
STR = r'"((?:[^"\\]|\\.)*)"'


def _unescape(s: str) -> str:
    return s.replace("\\n", "\n").replace('\\"', '"').replace("\\\\", "\\")


def _table(file: str, name: str) -> str:
    text = (SRC / file).read_text(encoding="utf-8")
    m = re.search(rf"const {name}: &\[[^\]]*\] = &\[(.*?)\n\];", text, re.S)
    assert m, f"{name} not found in {file}"
    return m.group(1)


def _pairs(file: str, name: str) -> list[tuple[str, str]]:
    body = _table(file, name)
    return [(_unescape(a), _unescape(b)) for a, b in re.findall(rf"\(\s*{STR},\s*{STR},?\s*\)", body)]


def _strings(file: str, name: str) -> list[str]:
    return [_unescape(s) for s in re.findall(STR, _table(file, name))]


needs_src = pytest.mark.skipif(not SRC.exists(), reason="crates/bd-commands not present")


@needs_src
def test_structural_vectors() -> None:
    pairs = _pairs("tests.rs", "STRUCTURAL")
    assert len(pairs) > 20
    for raw, want in pairs:
        got = commands.apply(raw, quote_open=raw.startswith("I'm not going")).text
        assert got == want, raw


@needs_src
def test_prose_stays_literal() -> None:
    prose = _strings("tests_prose.rs", "STRUCTURAL_PROSE")
    assert len(prose) > 20
    for s in prose:
        out = commands.apply(s)
        assert out.text == s and not out.actions, s


@needs_src
def test_mixed_vectors() -> None:
    for raw, want in _pairs("tests_prose.rs", "MIXED"):
        assert commands.apply(raw).text == want, raw


def test_quote_state_carries() -> None:
    first = commands.apply("He said, open quote, I'm not going.")
    assert first.quote_open
    second = commands.apply("I won't quote him on that, end quote.", first.quote_open)
    assert second.text == "I won't quote him on that.”"
    assert commands.apply("He said quote run end quote").text == "He said “run”"


def test_scratch_previous() -> None:
    assert commands.apply("Scratch that. Scratch that.").scratch_previous == 2
