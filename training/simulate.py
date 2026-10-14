"""Simulate Parakeet TDT 0.6B v3 output from clean prose.

Parakeet already capitalizes and punctuates, but it never emits quotation
marks or paragraph breaks, rarely emits semicolons or dashes, gets some
sentence boundaries wrong, and transcribes fillers ("um", "uh") and repeated
words. The simulator starts from the clean target and degrades it in those
ways, so the target stays exactly recoverable by the allowed edits (delete
fillers, repeats and false starts; fix case and punctuation; add marks).
"""

from __future__ import annotations

import random
import re
from collections import Counter
from dataclasses import dataclass, fields

QUOTE_MARKS = "“”‘’"
SENT_END = ".?!…"
COMMON_STARTERS = {
    "the", "he", "she", "it", "they", "we", "but", "and", "then", "there", "so", "you", "a", "in",
    "when", "as", "his", "her", "this", "that", "what", "if", "now", "yes", "no", "well", "oh",
}
# Placeholders keep spoken commands intact through the word-level noise.
OQ, CQ, NP = "\u27e6OQ\u27e7", "\u27e6CQ\u27e7", "\u27e6NP\u27e7"
SPOKEN = {OQ: "open quote", CQ: "close quote", NP: "new paragraph"}
SUBSTITUTES = ["went", "was", "had", "said", "the", "a", "just", "saw", "took", "got"]


@dataclass
class NoiseConfig:
    """Rates are per opportunity (per word gap, per comma, per boundary...)."""

    filler: float = 0.025  # insert um/uh per word gap
    repeat: float = 0.018  # stutter-repeat a word
    restart: float = 0.05  # per segment: repeat a 2-4 word run (restart)
    false_start: float = 0.015  # per segment: abandoned phrase with a changed word
    merge: float = 0.18  # per sentence boundary: ASR merges sentences (calibrated: comma splices are common)
    merge_comma: float = 0.7  # a merge leaves a comma (else nothing)
    merge_keep_cap: float = 0.3  # a merged sentence keeps its capital ("remembered, The porch")
    split: float = 0.04  # per comma: ASR ends a sentence early
    comma_drop: float = 0.12
    comma_insert: float = 0.01  # per word gap
    lower_start: float = 0.05  # lowercase a sentence start
    tag_period: float = 0.3  # "I'm going, she said" -> "I'm going. She said."
    exclaim_to_period: float = 0.35  # "!" -> "." (calibrated: Parakeet rarely writes "!")
    exclaim_to_comma: float = 0.3  # "!" -> "," ; the rest of the time "!" is dropped or kept
    question_to_comma: float = 0.2  # non-final "?" -> "," ("Hello, hello, can you hear me?")
    no_final_punct: float = 0.12  # segment ends with no punctuation at all ("Only me don't shoot")
    hyphen_to_space: float = 0.3
    keep_ellipsis: float = 0.3
    command_span: float = 0.04  # per quoted span: author dictates "open quote ... close quote"
    command_para: float = 0.06  # per paragraph break: author says "new paragraph"
    trailing_period: float = 0.85  # segment ending mid-clause still gets a "."

    @classmethod
    def scaled(cls, factor: float) -> "NoiseConfig":
        """All disfluency/boundary rates multiplied by `factor`."""
        base = cls()
        scalable = {"filler", "repeat", "restart", "false_start", "merge", "split", "comma_drop", "comma_insert",
                    "lower_start", "no_final_punct", "question_to_comma"}
        return cls(**{f.name: min(1.0, getattr(base, f.name) * (factor if f.name in scalable else 1.0)) for f in fields(cls)})


def _cap(w: str) -> str:
    for i, c in enumerate(w):
        if c.isalpha():
            return w[:i] + c.upper() + w[i + 1 :]
    return w


def _lower(w: str) -> str:
    for i, c in enumerate(w):
        if c.isalpha():
            return w[:i] + c.lower() + w[i + 1 :]
    return w


def _core(w: str) -> str:
    return w.strip(".,;:!?…—\"'()" + QUOTE_MARKS).lower()


@dataclass
class Simulator:
    cfg: NoiseConfig
    rng: random.Random
    stats: Counter[str]

    def p(self, rate: float) -> bool:
        return self.rng.random() < rate

    # -- stage 1: marks the ASR cannot produce --------------------------------
    def strip_marks(self, text: str, open_mark: str, close_mark: str) -> str:
        """Quotes -> removed (or spoken), paragraph breaks -> spaces (or spoken)."""
        out: list[str] = []
        lead_break = text.startswith("\n")
        body = text.lstrip("\n")
        if lead_break:
            if self.p(self.cfg.command_para):
                out.append(f"{NP}. ")
                self.stats["cmd_new_paragraph"] += 1
        spoken_span = False
        k = 0
        while k < len(body):
            ch = body[k]
            if ch == "\n":
                j = k
                while j < len(body) and body[j] == "\n":
                    j += 1
                if self.p(self.cfg.command_para):
                    out.append(f" {NP}. ")
                    self.stats["cmd_new_paragraph"] += 1
                else:
                    out.append(" ")
                self.stats["para_breaks_removed"] += 1
                k = j
                continue
            prev_alnum = k > 0 and body[k - 1].isalnum()
            next_alnum = k + 1 < len(body) and body[k + 1].isalnum()
            if ch == open_mark and not (prev_alnum and next_alnum):
                spoken_span = self.p(self.cfg.command_span)
                if spoken_span:
                    out.append(f" {OQ}, ")
                    self.stats["cmd_quote"] += 1
                self.stats["quote_spans"] += 1
            elif ch == close_mark and not (prev_alnum and next_alnum):
                if spoken_span:
                    out.append(f", {CQ}, ")
                spoken_span = False
            elif ch in QUOTE_MARKS:
                pass  # nested marks are always dropped
            else:
                out.append(ch)
            k += 1
        return re.sub(r"[ \t]+", " ", "".join(out)).strip()

    # -- stage 2: punctuation the ASR would write differently ----------------
    def asr_punct(self, text: str) -> str:
        def dash(m: re.Match[str]) -> str:
            self.stats["dashes"] += 1
            r = self.rng.random()
            return ", " if r < 0.5 else (". " if r < 0.7 else " ")

        text = re.sub(r"\s*[—–]\s*", dash, text)
        text = re.sub(r";", lambda m: "," if self.p(0.6) else ".", text)
        text = re.sub(r":(?=\s)", lambda m: self.rng.choices([",", "", ".", ":"], [0.3, 0.4, 0.2, 0.1])[0], text)
        text = re.sub(r"[()\[\]]", "", text)
        text = re.sub(r"…|\.\.\.", lambda m: "..." if self.p(self.cfg.keep_ellipsis) else ".", text)
        def excl(m: re.Match[str]) -> str:
            r = self.rng.random()
            c = self.cfg
            return "." if r < c.exclaim_to_period else ("," if r < c.exclaim_to_period + c.exclaim_to_comma else
                                                        ("" if r < c.exclaim_to_period + c.exclaim_to_comma + 0.15 else "!"))

        text = re.sub(r"!", excl, text)
        text = re.sub(r"\?(?=\s)", lambda m: "," if self.p(self.cfg.question_to_comma) else "?", text)
        text = re.sub(r"(?<=\w)-(?=\w)", lambda m: " " if self.p(self.cfg.hyphen_to_space) else "-", text)
        text = re.sub(r"\s+([,.?!])", r"\1", text)
        text = re.sub(r"([,.?!])[,.]+", r"\1", text)
        return re.sub(r"\s+", " ", text).strip(" ,")

    # -- stage 3: word-level noise ----------------------------------------------
    def word_noise(self, text: str) -> str:
        toks = text.split()
        out: list[str] = []
        for i, w in enumerate(toks):
            prev = out[-1] if out else ""
            sent_start = not out or prev[-1:] in SENT_END
            # sentence boundary errors on the previous token
            if out and prev[-1:] == "." and not prev.lower().endswith(("mr.", "mrs.", "dr.", "st.", "ms.")):
                if self.p(self.cfg.merge):
                    out[-1] = prev[:-1] + ("," if self.p(self.cfg.merge_comma) else "")
                    self.stats["merges"] += 1
                    sent_start = False
                    if w[0].isupper() and _core(w) in COMMON_STARTERS and not self.p(self.cfg.merge_keep_cap):
                        w = _lower(w)
            elif out and prev.endswith(",") and self.p(self.cfg.split):
                out[-1] = prev[:-1] + "."
                self.stats["splits"] += 1
                sent_start = True
            elif out and prev.endswith(",") and self.p(self.cfg.comma_drop):
                out[-1] = prev[:-1]
                self.stats["comma_drops"] += 1
            elif out and prev[-1:].isalnum() and self.p(self.cfg.comma_insert):
                out[-1] = prev + ","
                self.stats["comma_inserts"] += 1
            # Parakeet capitalizes after a sentence end, e.g. the tag in "Where? He asked."
            if sent_start:
                w = _cap(w)
                if out and _core(w) in COMMON_STARTERS and self.p(self.cfg.lower_start):
                    w = _lower(w)
                    self.stats["lower_starts"] += 1
            # fillers before the word
            if self.p(self.cfg.filler):
                f = self.rng.choices(["um", "uh", "er", "ah"], [0.5, 0.4, 0.05, 0.05])[0]
                self.stats["fillers"] += 1
                if sent_start:
                    out.append(_cap(f) + ",")
                    w = _lower(w) if _core(w) in COMMON_STARTERS else w
                elif self.p(0.5) and out and out[-1][-1:].isalnum():
                    out[-1] += ","
                    out.append(f + ",")
                else:
                    out.append(f)
            # stutter repeat
            bare = w.rstrip(".,?!;:")
            if bare[-1:].isalnum() and self.p(self.cfg.repeat):
                out.append(bare + ("," if self.p(0.3) else ""))
                self.stats["repeats"] += 1
                if sent_start and _core(w) in COMMON_STARTERS:
                    w = _lower(w)
            out.append(w)
        return " ".join(out)

    def restarts(self, text: str) -> str:
        toks = text.split()
        if len(toks) < 6:
            return text
        if self.p(self.cfg.restart):
            i = self.rng.randrange(0, len(toks) - 3)
            k = self.rng.randint(2, 4)
            run = [t.rstrip(".,?!") for t in toks[i : i + k] if t]
            if run and all(r[-1:].isalnum() for r in run):
                filler = [self.rng.choice(["uh,", "um,"])] if self.p(0.5) else []
                run[-1] += ","
                toks[i:i] = run + filler
                self.stats["restarts"] += 1
        elif self.p(self.cfg.false_start):
            i = self.rng.randrange(0, len(toks) - 3)
            k = self.rng.randint(1, 3)
            run = [t.rstrip(".,?!") for t in toks[i : i + k]]
            sub = self.rng.choice(SUBSTITUTES)
            if all(r[-1:].isalnum() for r in run) and sub not in (_core(t) for t in toks[i : i + k + 1]):
                toks[i:i] = run + [sub + ","]
                self.stats["false_starts"] += 1
        return " ".join(toks)

    def finish(self, text: str) -> str:
        text = text.strip()
        if not text:
            return text
        text = _cap(text) if self.p(0.97) else text
        if text[-1] in ",;:":
            text = text[:-1] + ("." if self.p(self.cfg.trailing_period) else ",")
        elif text[-1].isalnum() or text[-1] in "'":
            text += "."
        if text[-1] in ".!?" and not text.endswith("...") and self.p(self.cfg.no_final_punct):
            text = text[:-1]
            self.stats["no_final_punct"] += 1
        return text

    def tag_periods(self, text: str) -> str:
        """'going, she said' -> 'going. She said' sometimes (Parakeet's sentence-end habit)."""
        def rep(m: re.Match[str]) -> str:
            if self.p(self.cfg.tag_period):
                self.stats["tag_periods"] += 1
                return ". " + _cap(m.group(1))
            return m.group(0)

        return re.sub(r", ((?:he|she|they|I|we|you)\s+(?:said|asked|replied|cried|whispered|answered|shouted))\b", rep, text)


def simulate(expected: str, rng: random.Random, cfg: NoiseConfig, stats: Counter[str],
             marks: tuple[str, str] = ("“", "”")) -> str:
    """Raw Parakeet-style text for the clean segment `expected`."""
    sim = Simulator(cfg, rng, stats)
    text = sim.strip_marks(expected, *marks)
    text = sim.asr_punct(text)
    text = sim.tag_periods(text)
    text = sim.word_noise(text)
    text = sim.restarts(text)
    text = sim.finish(text)
    for ph, spoken in SPOKEN.items():
        text = re.sub(rf"(^|[.?!]\s+){re.escape(ph)}", lambda m: m.group(1) + spoken.capitalize(), text)
        text = text.replace(ph, spoken)
    text = re.sub(r"\s+", " ", text).strip()
    stats["segments"] += 1
    stats["words"] += len(expected.split())
    return text
