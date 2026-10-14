"""Property tests for the simulator and segmenter: the target must always be
recoverable by allowed edits (the word guardrail must accept it)."""

from __future__ import annotations

import random
from collections import Counter

import commands
from segment import segments, split_sentences
from simulate import NoiseConfig, simulate
from textnorm import check_edits

PARS = [
    "“I don't know,” she said. “Maybe tomorrow, if the rain stops.”",
    "He laughed—a short, ugly sound—and turned away. The door slammed behind him.",
    "“Where are you going?” asked Mr. Brown.",
    "“To the river.” He folded the map. “Don't wait up, and don't tell your mother, whatever she asks you.”",
    "It was a long, long way down, and the rope was old; nobody spoke.",
    "“Well!” cried the old woman. “Of all the things to say to me, in my own kitchen, on a Sunday!”",
]


def test_guardrail_accepts_every_simulated_pair() -> None:
    rng = random.Random(0)
    stats: Counter[str] = Counter()
    noisy = NoiseConfig.scaled(4.0)
    noisy.command_span = noisy.command_para = 0.3
    for _ in range(300):
        for seg in segments(PARS, rng):
            raw = simulate(seg.expected, rng, noisy, stats)
            model_in = commands.apply(raw, seg.in_quote_at_start).text
            chk = check_edits(model_in, seg.expected)
            assert not chk.added, (seg.expected, raw, model_in, chk.added)
    assert stats["fillers"] and stats["repeats"] and stats["cmd_quote"] and stats["merges"]


def test_simulator_removes_quotes_and_breaks() -> None:
    rng = random.Random(1)
    for _ in range(50):
        raw = simulate("\n\n“Go.” He went.\n\n“Fine.”", rng, NoiseConfig(command_span=0, command_para=0), Counter())
        assert "“" not in raw and "\n" not in raw


def test_seeded_is_deterministic() -> None:
    a = simulate(PARS[3], random.Random(5), NoiseConfig(), Counter())
    b = simulate(PARS[3], random.Random(5), NoiseConfig(), Counter())
    assert a == b


def test_segments_close_quotes_at_sentence_boundaries() -> None:
    for seed in range(30):
        for seg in segments(PARS, random.Random(seed)):
            text = seg.expected
            depth = 1 if seg.in_quote_at_start else 0
            depth += text.count("“") - text.count("”")
            if not text.rstrip().endswith((",", "—", ";", ":")):
                assert depth == 0, text


def test_split_sentences_respects_abbreviations() -> None:
    assert split_sentences("Mr. Brown came. He sat.") == ["Mr. Brown came.", "He sat."]
    assert split_sentences("“Go!” He went.") == ["“Go!”", "He went."]
