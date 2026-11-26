# Findings: SAVA and Voice Isolator on Will's recordings

2026-11-26. Will's telemetry (install `1e61263e…`, 0.1.10–0.1.23), offline,
CPU (this machine: Ryzen AI 9 HX 370, shared with other builds at a load
of 30–150 on 24 threads, so every time below is pessimistic). Segment ids
are `session prefix:segment`; no transcripts here. How to rerun:
[README.md](README.md).

## Summary

| model | verdict | why |
|---|---|---|
| SAVA | **drop for sentence ends; list mode is not worth it on its own** | On his normal dictation SAVA's `turn` barely tells a thinking pause from a finished sentence (AUC 0.57–0.59). Every rule where SAVA decides makes 3–5× *more* false splits than ours. Used only to lengthen our wait, it prevents 4 of our 18 false splits, the same as raising our 1.5 s wait to 1.8 s, which adds less delay. In list mode it helps (10 of 22 false item splits left, against 15 for a flat wait at the same 0.9 s mean), but on 22 pauses, and a longer list-mode wait gets most of it. ONNX export works. |
| Voice Isolator | **drop** | It doesn't touch the runaway repeats: they come from the live decoder, not the audio (the worst, 9913149c:71, doesn't happen offline at all; the 7 of 16 that do are all still there after isolation). It keeps the supper conversation, which is voices (149 → 152 words). It makes quiet speech worse. Across 64 segments, 17 transcripts got worse, 6 better, 41 unchanged. It isn't realtime here on CPU (about 12 CPU-seconds per audio second) or on the Radeon 890M (4 s per audio second, bound by kernel launches, not arithmetic). |

## 1. SAVA: thinking pause or sentence end?

### The set

- **377 boundaries** (pauses our VAD ended an utterance on) from 111 runs
  of consecutive segments: 312 in normal dictation from six sessions
  (8731bdaf 11/12, 2e98972e 11/06, fb1e2f3e 11/21, 9913149c 11/26, 8f495fd2
  10/21, 56b2be93 11/22) and **65 in list mode: every list-mode run he
  has** (da122f63, effd6629 11/10; 99dc6817 11/11; 9afdefb3 11/13;
  7ea6bb56 10/20). The brief's "11/12 list-mode session" is 8731bdaf,
  which has list mode off in its settings; the list-mode sessions are
  11/10–11/13, all included.
- **Streams** rebuild each run: the segments' audio, with each real gap
  (`pause_ms`, the silence between the padded segments) filled with white
  noise at that segment's own noise floor (its quietest 10% of 20 ms
  windows). Gaps over 10 s are not rebuilt (every rule ends the utterance
  long before); runs are cut there. The filler is not his room's real
  noise; that's the main caveat on the stream.
- **Labels** (`mid` / `end` / `unsure`) from the words alone, by a Claude
  agent following [label_prompt.md](label_prompt.md), told to ignore the
  punctuation at the pause (the app adds a full stop itself after a long
  pause, so the punctuation is what we're testing, not evidence).
  Normal: 132 mid, 164 end, 16 unsure. List: 22 mid, 39 end, 4 unsure.
- **Hand check:** I labelled a random 40 blind. Exact agreement 36/40;
  34/35 where neither of us said `unsure`. The brief's punctuation rule
  (mid when A has no closing mark) agrees with the labels on 312/357; most
  of the difference is 18 `mid` pauses that carry the app's own full stop,
  which are exactly the false splits this test is about.

### How a rule is scored

Times are from the end of the speech (segment end less the segmenter's
200 ms padding). The speaker resumed after G = `pause_ms` + 0.4 s. A rule
decides at D; it splits if D < G. At a `mid` boundary a split is a
**false split** (a full stop and capital, or a new list item, mid-thought).
At an `end` boundary D is the **wait** before the sentence is final.
SAVA's `turn` for every pause length comes from a copy of the streaming
session fed 10.5 s more of the same noise at each segment end
([run_sava.py](run_sava.py)).

- **current**: what the app does now (`stages/listen.rs`): the VAD stops
  at 0.6 s, then the stream waits 1.5 s more (4 s after a word a sentence
  doesn't end on). D = 2.1 s, or 4.6 s. In list mode every utterance is an
  item, D = 0.6 s. (`sentence_hold` in the tidy stage only delays
  cleanup; it doesn't decide the full stop, so it isn't modelled.)
- **sava t/M**: once the VAD has stopped, end at the first frame with
  `turn` > t, else at M.
- **hold t**: our rule, but SAVA can lengthen it: at our decision time,
  end if `turn` > t, else wait until it is, at most 4.6 s.
- **wait W**: no model, a flat wait W: the baseline a SAVA rule must beat.

### Normal dictation (132 mid, 164 end)

| rule | false splits | wait at ends: median / mean / p90 (s) |
|---|---|---|
| **current** | **18** | 2.10 / 2.13 / 2.10 |
| sava 0.5 / 2.1 | 100 | 0.62 / 0.73 / 0.64 |
| sava 0.9 / 2.1 | 75 | 0.64 / 1.13 / 2.10 |
| sava 0.95 / 2.1 | 56 | 0.86 / 1.31 / 2.10 |
| sava 0.95 / 4.6 | 49 | 0.86 / 1.96 / 4.32 |
| hold 0.9 | 14 | 2.13 / 2.57 / 3.72 |
| hold 0.95 | 13 | 2.13 / 2.81 / 4.58 |
| wait 2.6 | 15 | 2.60 / 2.60 / 2.60 |
| wait 2.8 | 12 | 2.80 / 2.80 / 2.80 |
| current, `WAIT` 1.8 | 14 | 2.40 / 2.43 / 2.40 |
| current, `WAIT` 2.2 | 12 | 2.80 / 2.82 / 2.80 |

(`wait W` is flat, even after an unfinished word; "current, `WAIT` x" is
the app's rule with only its 1.5 s constant changed.)

How well `turn` separates `end` from `mid` while still silent (AUC; 0.5 is
chance): 0.57 at 0.8 s, 0.59 at 1.2 s, 0.59 at 2.0 s after the speech.

**So, at an acceptable latency, how many of his false splits would SAVA
have prevented? Effectively none.** SAVA in charge makes the end of a
sentence fast (0.6–0.9 s median, against our 2.1 s) but splits 37–76% of
his thinking pauses, against our 14%. On his speech `turn` is above 0.9
by 0.7 s into the pause at 47% of mid-sentence pauses and 59% of sentence
ends: barely different (for example
8731bdaf:2740, a short pause before a relative clause, or
8731bdaf:96, 8731bdaf:195). As a brake on our rule it saves 4 of the 18
(2e98972e:488, 2e98972e:2868, 9913149c:108, 56b2be93:112) for 0.44 s more
mean wait; our own rule with `WAIT` 1.8 s instead of 1.5 saves the same 4
for 0.30 s more. Our 18 false
splits are pauses of 2.1–7.7 s (half over 3.3 s); only a longer wait
reaches them, model or no model.

### List mode (22 mid, 39 end)

| rule | false item splits | wait at item ends: median / mean / p90 (s) |
|---|---|---|
| **current** | **22** (all) | 0.60 / 0.60 / 0.60 |
| hold 0.9 | 10 | 0.62 / 0.89 / 1.53 |
| hold 0.95 | 6 | 0.64 / 1.52 / 3.20 |
| wait 0.9 | 15 | 0.90 |
| wait 1.2 | 12 | 1.20 |
| wait 1.5 | 8 | 1.50 |

Here SAVA does read something (AUC 0.74 at 0.8 s): with hold 0.9 a pause
inside an item mostly keeps a low `turn` (9afdefb3:191, 99dc6817:1645,
da122f63:200 stay under 0.1), so it saves 12 of the 22 false item splits
while item ends still close at 0.62 s median. Against a flat wait at the
same mean (0.9 s: 15 left) that's 5 fewer false splits out of 22. Real, but
small, and on one tester's 22 pauses. A list-mode wait of 1.2–1.5 s
(12 or 8 left), a one-line change, gets most of it.

### Cost and ONNX

- **ONNX: yes.** `torch.onnx.export(dynamo=True)` exports the one-frame
  streaming step with its whole state as 19 fixed-shape tensors in and out
  (sliding-window attention caches, Mamba conv and SSM states, the frame
  count), 103 MB with weights. onnxruntime matches PyTorch to 7.6e-6 in
  both probabilities over 1500 frames (60 s) of his audio, state carried
  frame to frame ([export_sava_onnx.py](export_sava_onnx.py)). So
  adopting it in Rust would be an onnxruntime session and 19 buffers, no
  custom ops.
- **CPU:** onnxruntime, one thread, 17.7 ms per 40 ms frame (0.44 of a
  core) on this loaded machine; 11.3 ms (0.28) on an earlier, less loaded
  run. PyTorch frame by frame: 39–45 ms. Inworld quote 3.8 ms on an M3 Max.
  On Will's 7800X3D expect a tenth to a quarter of one core, beside
  Nemotron's 0.2 of real time on 8 threads.

## 2. Voice Isolator: does cleaning change what is transcribed?

### The set and how it's run

64 segments ([isolate.py](isolate.py) `select`): 16 **repeat** (the
live app's transcript has a word 5+ times in a row, the longest first:
9913149c:71's "re" ×40, 00fa4593:26, 99dc6817:1655, …), 12 **supper**
(the 11/25 dinner conversation, 3890918c:635–647), 8 **quiet** (level
under −48 dBFS), 8 **noisy** (the highest noise floors of 200 random),
and 20 **control** (random). Each is upsampled 16 → 48 kHz (soxr), run
through the realtime checkpoint (`mode="streaming"`, 2 lookahead frames,
`isolate()`, which runs the live stream over the clip), and resampled back.
Both versions are transcribed offline by `bd file` (release build, cleanup
off, his word list and hotword boost 2.5 from his settings).

The reference for word error is the cleanup's output where it cleaned that
segment alone, else the live transcript. Both come from the original
audio, so the WER leans towards the original; the iso-vs-orig column
(word edits between the two offline transcripts, over the original's
words) has no such lean.

| group | n | runaway (5+ repeats) orig → iso | words orig → iso | level change (dB) | iso vs orig edits | WER orig → iso |
|---|---|---|---|---|---|---|
| repeat | 16 | 7 → 7 | 227 → 214 | −0.8 | 0.13 | 0.37 → 0.42 |
| supper | 12 | 0 → 0 | 149 → 152 | 0.0 | 0.14 | 0.38 → 0.37 |
| quiet | 8 | 0 → 0 | 40 → 35 | −2.0 | 0.72 | 0.24 → 0.74 |
| noisy | 8 | 0 → 0 | 58 → 57 | −1.8 | 0.04 | 0.55 → 0.59 |
| control | 20 | 0 → 0 | 154 → 154 | −0.5 | 0.03 | 0.11 → 0.09 |

All 64: WER lower for 6, higher for 17, the same for 41.

- **Runaway repeats: no effect, and not an audio problem.** All 16 were
  runaways in the live app. Re-transcribed offline from the same audio,
  only 7 repeat 5+ times (four are "very" ×5+, two laughter, one "wait"),
  and the isolated audio gives the same 7.
  9913149c:71's "re" ×40 is a single "re" offline, even with the three
  segments before it as context. The runaways come from the live
  decoder's state, so a fix belongs in the recognizer stage, not in front
  of it.
- **Supper conversation: not suppressed.** It is people talking, which is
  what the isolator keeps: 149 words before, 152 after, with small
  word changes both ways (3890918c:637 one word better, :644 slightly
  worse). Stopping that needs speaker or attention logic, not denoising.
- **Quiet speech: worse.** 178c1f22:35 loses 8 of 10 words;
  fb1e2f3e:1129 drops 8.7 dB and changes; on 0dca7853:785 (near-silence,
  nothing transcribed before) the isolated audio gives three words.
- **Noisy and control:** almost untouched (3–4% of words change); the
  noisy group's WER is slightly worse (f5393372:44), the control's
  slightly better (8f495fd2:1395).
- **Repeat group regressions:** 05750d61:368 loses 10 of 17 words.

Caveat: Will's recordings are 16 kHz, so the isolator was given audio with
nothing above 8 kHz. In the app it would get the microphone's own 48 kHz.
That could help it, but it can't create speech it doesn't hear: none of the
three findings above depend on the top band.

### Cost, CPU and GPU

- **CPU:** 404 GFLOP per audio second (profiled, one second of audio). That is about 12 CPU-seconds
  per audio second on one thread of this machine, 6.7 s wall at 4 threads
  under load. It can't run live on any desktop CPU.
- **GPU (Radeon 890M iGPU, ROCm 6.4, torch 2.9.1):** 20 ms pushes take
  3.98 s per audio second (`isolate.py cost --device cuda`); a push takes
  60 ms median, 127 ms p95, against the 20 ms it must keep up with.
  `isolate()` on the whole clip is 3.0 s per audio second. The arithmetic
  would take the 890M well under a second; the profile shows the cost is
  **about 1,000 kernel launches per 20 ms frame** (100,187 for 2 s), and
  the host spends 6.8 CPU-seconds per audio second issuing them. So
  realtime needs the frame captured as one graph (CUDA/HIP graphs,
  `torch.compile`, or an ONNX export run on a GPU execution provider) on a
  discrete GPU. Inworld's realtime figures are for NVIDIA datacenter GPUs.
  On Will's Windows RX 5600 XT that means ONNX on DirectML: an untested
  export of a Mamba model, sharing a 6 GB card with the cleanup model.
- **48 kHz path:** the app's audio is 16 kHz from the capture stage on;
  the isolator would have to sit before that resample, on the device's
  native 48 kHz, and hand 16 kHz on. That's a change to the capture code.
  We tested the next best thing, 16 kHz upsampled.

## Recommendation

- **SAVA: drop for normal dictation.** Nothing it does beats a longer
  wait. If the 18 false splits matter more than the extra wait, try
  `WAIT` 1.8 s in `stages/listen.rs` (14 left, as with SAVA's best rule,
  for 0.30 s more mean wait against its 0.44), or 2.2 s (12 left): a
  constant, no model.
- **List mode: try a longer wait first, not SAVA.** Every list item
  that pauses mid-thought becomes two items today (22 of 22). A list-mode
  wait of 1.2–1.5 s (12 or 8 left) is a one-line change. SAVA's hold 0.9
  does better at less wait (10 left at 0.89 s mean), but that's on 22
  pauses from one tester, and costs a 103 MB ONNX model and a tenth to a half
  of a core. Revisit it only if testers still complain after the wait change.
  The ONNX export is here and verified, if that day comes.
- **Voice Isolator: drop.** It doesn't address any of the three problems
  it was hoped for, harms quiet speech, and can't run live on his
  hardware. The runaway repeats are a recognizer-state bug: look for them
  in the recognizer stage (decoder reset, repeat guard), with
  9913149c:71's session as the reproduction to chase, since the audio
  alone doesn't reproduce it.
