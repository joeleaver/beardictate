# Rejoining a sentence the listener closed at a thinking pause

2026-11-26. Can the app take back the full stop its listener adds when the
speaker pauses mid-sentence for longer than its deadline, by a text rule,
by the cleanup model (Bear, as shipped), or by stock Gemma 4 E2B? And, in
list mode, should an utterance said before the deadline go on the same
item? This is what `crates/bd-pipeline/src/stages/rejoin.rs` was built on.

## The set

The Windows tester's telemetry (install `1e61263e…`): 110 runs of
consecutive segments from six sessions of normal dictation and every
session's list mode (`segments.pick`, seed 7), 373 pauses between two
segments, each under 10 s. Each pause's
text on both sides (with a sentence of context) was labelled `mid` (the
sentence or list item goes on), `end` or `unsure` by two Claude labellers
with `label_prompt.md`, split between them: normal mode 119 mid, 168
end, 21 unsure; list mode 25 mid, 39 end, 1 unsure.

`closed` says whether today's listener closed the sentence there: its
deadline (0.6 s VAD plus 1.5 s, or 4 s after an UNFINISHED word or a comma)
came before the speaker went on (pause plus 0.4 s of padding). Only closed
pauses get our full stop; at the others Nemotron punctuates knowing what
followed.

| mode | closed: mid | closed: end | went on in time: mid | went on in time: end |
|---|---|---|---|---|
| normal | 18 | 99 (+8 unsure) | | |
| list | 7 | 34 | 18 | 5 |

Not scored: closed pauses after which the speaker only said a mark
("question mark"; normal 6, list 3), since the app already puts a said mark
in place of ours (telemetry `retyped`); and ones whose next words start
with a command (normal 3, list 6, mid and end together). That leaves
**normal 11 mid / 97 end, list 3 mid / 29 end**.

## Results

"Rejoined" is of the mid-sentence pauses, "wrongly joined" of the real ends.

### Normal mode (closed pauses)

| option | rejoined | wrongly joined |
|---|---|---|
| (a) text rule: A ends on a DANGLING word or a comma, or B starts with a CONTINUES word | **1/11** | **0/97** |
| (a') the same with the listener's whole UNFINISHED list | 2/11 | 3/97 |
| (b) Bear, yes/no "does B carry A on?" (P(yes) ≥ 0.5) | 1/11 | 1/97 |
| (b) text rule + Bear yes/no | 2/11 | 1/97 |
| (b) Bear's own cleanup prompt on A+B without our mark ("reads on": no sentence end put back) | 10/11 | 47/97 |
| (c) stock Gemma, yes/no (≥ 0.5) | 1/11 | 2/97 |
| (c) text rule + Gemma yes/no | 2/11 | 2/97 |
| (c) Gemma, choose split or joined, ≥ 0.5 | 10/11 | 91/97 |
| (c) Gemma, choose split or joined, ≥ 0.7 | 0/11 | 2/97 |

### List mode (closed pauses)

| option | rejoined | wrongly joined |
|---|---|---|
| (a) text rule | **1/3** | **0/29** |
| (b) Bear yes/no (≥ 0.5) | 0/3 | 2/29 |
| (b) Bear's cleanup on A+B | 2/3 | 12/29 |
| (c) Gemma yes/no (≥ 0.5) | 0/3 | 1/29 |

### List mode, went on before the deadline

| option | kept whole | wrongly joined |
|---|---|---|
| join every one | 18/18 | 5/5 |
| join unless Nemotron ended A with . ? ! | **17/18** | **1/5** |

That one wrong join is 1 of all 39 item ends in list mode (the closed ones
never join this way). The split that stays split is `s104.2`, the wrong
join `s107.10` (`boundaries.jsonl` ids).

## What it says

- **The models give no signal.** Bear's P(yes) is about 0.01 for mid and
  end pauses alike, stock Gemma's about 0.04; adding either to the text
  rule only adds wrong joins. Asked to choose between the two ways of
  writing it, Gemma answered exactly 0.5 for 23 of 25 mids and 121 of 133
  ends: position bias, nothing more. Bear's own cleanup, given the two as
  one, seldom puts a sentence end back in (47 of 97 real ends ran on).
  Both are in `training/RETRAIN.md` (13, 14).
- **The text rule is safe and small.** It never joined a real end, but only
  1 of 11 mid-sentence pauses ends on a word a sentence can't end on: the
  tester's mid pauses mostly come after a full word, where only meaning can
  tell. The listener's UNFINISHED words that are left out of DANGLING
  ("is", "you", "do you") end this tester's sentences as often as not, and
  a B starting "And", "But", "If" or "When" mostly starts a new sentence.
- **In a list, going on in time is the rest of the item.** Nemotron
  punctuates A knowing B followed; when it put no mark, B was the same
  item 17 times out of 18.

## What was built

- `list_join` (app setting `list_join_pauses`): **on** by default.
- `rejoin` (app setting `rejoin_sentences`), the text rule only: **off** by
  default. It mends 1 or 2 of 11 and wrongly joins none here; it's there to
  be measured by telemetry (`rejoined`) before it's turned on.
- No model in the decision.

Follow-ups: the fillers ("um", "uh") could join the listener's UNFINISHED
words, so it waits the longer deadline after them and fewer pauses close
at all. Once `rejoin_sentences` is on for a tester, count `rejoined`
against a "scratch that" right after it.

## Running it

`segments.py` reads the export and picks the runs; `label_prompt.md` is
what the labellers were given. Keep the export and everything written from
it outside the repo (the scripts refuse a `--data` inside it), and delete
it afterwards: it holds a tester's words.

```
telemetry/pull.sh export $D/will.jsonl --install 1e61263ef559c1873d45ab09a82ba59d
python3 eval/rejoin/boundaries.py --export $D/will.jsonl --data $D
# label boundaries.jsonl into $D/labels.jsonl: id, label (mid/end/unsure)
llama-server -m MODEL.gguf -ngl 99 -c 4096 --port 18431 &
python3 eval/rejoin/ask.py --data $D --name bear-join --mode join      # Bear loaded
python3 eval/rejoin/ask.py --data $D --name bear-tidy --mode tidy
python3 eval/rejoin/ask.py --data $D --name gemma-join --mode join     # stock Gemma loaded
python3 eval/rejoin/ask.py --data $D --name gemma-choose --mode choose
python3 eval/rejoin/score.py --data $D --answers bear-join gemma-join bear-tidy gemma-choose
```

Models: Bear `cleanup-bear-gemma-4-e2b-v4-q4_0`, stock
`cleanup-gemma-4-e2b-q4_0` (Q4_0), llama.cpp Vulkan, all layers on the GPU.
