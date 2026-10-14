"""Unit tests for the scorer."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

import score
from textnorm import check_edits, tokenize, words

EVAL = Path(__file__).resolve().parents[1]

CASE = {
    "id": "t",
    "category": "dialog",
    "context": "",
    "raw": "Um, where's the key? He asked. I don't know, she said.",
    "expected": "“Where's the key?” he asked.\n\n“I don't know,” she said.",
}


def sc(output: str, case: dict[str, str] = CASE) -> score.CaseScore:
    return score.score_case(case, output, 100.0)


def test_perfect_output() -> None:
    s = sc(CASE["expected"])
    assert s.exact and s.quote_ok and s.para_ok and s.forbidden == 0
    assert s.word_acc == 1.0 and s.quote.f1 == 1.0 and s.punct.f1 == 1.0


def test_straight_quotes_still_exact() -> None:
    s = sc('"Where\'s the key?" he asked.\n\n"I don\'t know," she said.')
    assert s.exact and s.quote_ok


def test_missing_quotes() -> None:
    s = sc("Where's the key? he asked.\n\nI don't know, she said.")
    assert not s.exact and not s.quote_ok and s.para_ok
    assert s.word_acc == 1.0 and s.forbidden == 0
    assert s.quote.fn == 2 and s.quote.tp == 0


def test_wrong_quote_boundary() -> None:
    s = sc("“Where's the key? he asked.”\n\n“I don't know,” she said.")
    assert not s.quote_ok
    assert s.quote.tp == 1 and s.quote.fp == 1 and s.quote.fn == 1


def test_merged_adjacent_quotes_are_two_spans() -> None:
    case = {**CASE, "raw": "He did. She did.", "expected": "“He did.”\n\n“She did.”"}
    assert not sc("“He did. She did.”", case).quote_ok
    assert sc("“He did.”\n\n“She did.”", case).quote_ok


def test_missing_paragraph_break() -> None:
    s = sc("“Where's the key?” he asked. “I don't know,” she said.")
    assert s.quote_ok and not s.para_ok and not s.exact


def test_leading_break_counts() -> None:
    case = {**CASE, "context": "He waited.", "raw": "Hello.", "expected": "\n\n“Hello.”"}
    assert sc("\n\n“Hello.”", case).para_ok
    assert not sc("“Hello.”", case).para_ok


def test_added_and_substituted_words_are_forbidden() -> None:
    s = sc("“Where's the door?” he asked.\n\n“I really don't know,” she said.")
    assert s.forbidden == 2 and sorted(s.added) == ["door", "really"]
    assert s.word_acc < 1.0


def test_filler_kept_is_not_forbidden_but_costs_accuracy() -> None:
    s = sc("“Um, where's the key?” he asked.\n\n“I don't know,” she said.")
    assert s.forbidden == 0 and s.word_acc < 1.0 and s.quote_ok


def test_continuation_starts_inside_quote() -> None:
    case = {"id": "c", "category": "cont", "context": "“I gave you the car,", "raw": "and the house. She left.",
            "expected": "and the house.” She left."}
    assert sc("and the house.” She left.", case).quote_ok
    assert not sc("and the house. She left.", case).quote_ok


def test_punctuation_f1() -> None:
    s = sc("“Where's the key” he asked.\n\n“I don't know” she said.")
    assert s.punct.fn == 2 and s.punct.fp == 0


def test_spoken_commands_not_forbidden() -> None:
    case = {"id": "s", "category": "cmd", "context": "", "raw": "Open quote, run, close quote.", "expected": "“Run.”"}
    s = sc("“Run.”", case)
    assert s.exact and s.forbidden == 0


def test_latency_percentiles() -> None:
    scores = [score.score_case(CASE, CASE["expected"], float(ms)) for ms in range(1, 101)]
    agg = score.aggregate(scores)
    assert agg["p50_ms"] == pytest.approx(50.5)
    assert agg["p95_ms"] == pytest.approx(95.05)
    assert agg["exact"] == 1.0


def test_words_normalization() -> None:
    assert words("“Don’t—stop,” he said. Well-known…") == ["don't", "stop", "he", "said", "well", "known"]


def test_check_edits_suspicious() -> None:
    chk = check_edits("um the the cat sat quietly", "The cat sat.")
    assert chk.forbidden == 0 and chk.suspicious_deletions == ["quietly"]


def test_tokenize_apostrophes_are_not_quotes() -> None:
    tz = tokenize("The dogs’ bowls and Sam’s hat. ‘Go,’ she said.")
    assert [t.span >= 0 for t in tz.toks] == [False] * 6 + [True, False, False]


@pytest.mark.parametrize("name", ["cases.jsonl", "cases_uk.jsonl"])
def test_expected_scores_perfect_on_every_case(name: str) -> None:
    path = EVAL / name
    if not path.exists():
        pytest.skip("cases not built")
    for line in path.read_text(encoding="utf-8").splitlines():
        case = json.loads(line)
        s = score.score_case(case, case["expected"], None)
        assert s.exact and s.quote_ok and s.para_ok and s.forbidden == 0, case["id"]
        assert s.punct.f1 == 1.0 and s.quote.f1 == 1.0, case["id"]
