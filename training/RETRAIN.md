# What the next Bear model must learn

A running list, added to as testers and telemetry show the current model
(Bear Gemma 4 E2B v4, `bear-tags-v1`) getting something wrong. Retrain when
the list is worth a GPU run (Joe, 2026-10-20). Each item says what goes
wrong, the evidence, and what the training data needs. Strike an item
through (with the model that fixed it) rather than deleting it.

Telemetry is the source of real cases: `telemetry/pull.sh export`, then the
`cleaned` events' `raw` / `model_output` / `final` (see CLAUDE.md for where
exports may go). Testers' words never go into the training set as such;
the generator makes cases like them.

## Open

1. **Questions that came out with a full stop.** Nemotron ends about 1 in 7
   real questions with "." (a Windows tester, 2026-10-20: ~5 of ~34, e.g.
   "Didn't Mister Pimble live in Fazzi's thumb."), most often when the
   question runs over a pause and the last piece doesn't sound like one
   ("Is there a word to insert" / "a hard return character in the special
   words place."). Bear fixed none. `simulate.py` only ever turns a
   non-final "?" into ","; add final "?" → "." at about Nemotron's rate,
   and questions split over two segments. Keep statements that start like
   questions ("What you would call it.", "Which I love.") as negatives.
2. **Over-splitting one speaker into two turns.** "No. Should I?" became two
   turns, and a reply by the person named in the context's last line got a
   new paragraph (untagged dialog 100% in v3 → 60% in v4). From the
   generated scenes (`gen_scenes.py`). Report: `eval/research/2026-10-bear-finetune.md`.
3. **Quoting what isn't dialog.** Trained on novels, Bear quoted short lines
   that weren't speech. Dialog is off by default now and the guardrail takes
   out quote marks it added (`keep_dictated_quotes`), but the lasting fix is a
   dialog on/off tag in the prompt, trained both ways. Narration-only and
   dictation-app asides ("Okay, where were we.") as dialog-off cases.
4. **Real-mishearing recall.** Special words fixed from real Parakeet
   mishearings fell from 29/37 (v3) to 23/37 (v4). The mishearing bank is
   Parakeet's; the recognizer is Nemotron now, so rebuild
   `mishear_bank.py` from Nemotron.
5. **Punctuation junk turned into a dash.** Given "A: B:." Bear wrote
   "A: B:—" (a tester's special word, 7 of 7 times). The app no longer sends
   ":." (b9b386e), but doubled or odd marks should come back cleaned to
   one sensible mark, never a new dash.
6. **Closing a sentence the speaker hadn't finished.** Given a piece that
   ends in a comma because more is coming, Bear ends it: "Had the worst sore
   throat I ever had in my life," → "…life." (next: "but it's improving"),
   "…a new message from Hendrick for the pod," → "…pod?" (next: "in the
   pod"). The next piece then starts in lower case after the new full stop
   (a Windows tester, 2026-10-22). Train pieces ending in a comma, "and",
   "the"… to come back still open.
7. **Lower-casing a special word.** "Well, it got golder." with Guldor
   listed came back "Well, it got guldor." (respelled, accepted, typed lower
   case); the tester: "we don't have a feature for all these capitalizing
   yet" (2026-10-22). Respelled words should be written as listed in every
   case. (The app could also force the listed casing after the guardrail.)
8. **A filler that is part of a mishearing.** "send him the link to er
   dictate" (beardictate) came back "the link to dictate": the "er" was
   dropped as a filler and the name lost. Sound-alike spans that start with
   "er"/"uh" should be respelled, not trimmed.
9. **Spoken email addresses.** "Charlie at Charlie Lever dot com." was left
   as words; Bear tried "CharlieLever", which the guardrail refused
   (2026-10-22). Train "<name> at <domain> dot com" → an address, as a
   formatting rewrite the guardrail allows.

Evidence for 3, 2026-10-22, Dialog off: 15 of 109 answers came back wrapped
in curly quotes (“Pick up some deli.”). The guardrail took them all out,
including the one real line of dialog ("Callut said to Guldor, Oh there you
go."), which the tester expected quoted.
