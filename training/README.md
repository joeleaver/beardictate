# training

Synthetic training data for the "Bear" cleanup fine-tunes (DESIGN.md
milestone 5), plus a recommended fine-tune recipe. Nothing here trains a
model yet.

Large files live under `/mnt/data/beardictate-dev/` and never in the repo.
Use the venv at `/mnt/data/venvs/beardictate` (`uv venv --python 3.12`, then
`uv pip install pytest sherpa-onnx soundfile numpy`).

| file | what it does |
|---|---|
| `corpus_books.py` | the candidate list: ~60 dialog-heavy Gutenberg novels with author death years |
| `fetch_corpus.py` | polite download (3 s delay, UA, on-disk cache), strips the Gutenberg header, footer and license and the front matter, unwraps paragraphs, normalizes quotes (curly double, straight apostrophes), removes `_italics_` and illustrations |
| `segment.py` | cuts a book into dictation segments with running context |
| `simulate.py` | the Parakeet noise model (`NoiseConfig`, all rates configurable) |
| `make_pairs.py` | corpus → SFT chat JSONL + eval-format pairs + `report.md` |
| `calibrate_asr.py` | TTS → real Parakeet → compares its output style with the simulator |
| `tests/` | property tests: every generated pair passes the word guardrail |

## Regenerate

```sh
PY=/mnt/data/venvs/beardictate/bin/python
$PY training/fetch_corpus.py            # ~60 books, ~9.3M words, ~100 MB; cached, so a rerun only reprocesses
$PY training/make_pairs.py              # ~8 min; writes /mnt/data/beardictate-dev/pairs/
$PY -m pytest -q training/tests eval/tests
```

`fetch_corpus.py --max-died 1915` (the default) keeps only authors who died
more than ~110 years ago. Doyle, Wharton and Hardy are listed but excluded by
that rule. They are public domain in the US and life+70 countries, so
`--max-died 1940` adds them. Ten Dickens and Alcott editions are typeset with
single-quote dialog. The manifest marks them `style: uk` and `make_pairs.py`
skips them unless you pass `--include-uk-books`. UK-style training examples
are made by re-rendering whole US-typeset books with ‘ ’ instead
(`--uk-fraction`, default 0.1).

`make_pairs.py` options worth knowing:

- `--seed`. Everything is seeded per book, so output is reproducible.
- `--val-books N` / `--val-ids ...` hold out whole books, so there is no leakage.
- `--per-book 1500` caps examples per book, so Dickens doesn't dominate.
- `--narration-keep 0.5` downsamples segments that have no dialog.
- `--noise-scale 1.5` multiplies all disfluency rates. Any single rate can be
  set with `--noise-<name>`, for example `--noise-filler 0.04` or
  `--noise-command-span 0.1`.
- `--clean-fraction 0.05` adds examples with no disfluencies, so the model learns to leave clean text alone.
- `--prompt FILE` sets the prompt template. The default is the runtime prompt
  `crates/bd-cleanup/prompts/generic-v1.txt` when it exists, else
  `eval/prompts/generic-v1.txt`.

### Output (`/mnt/data/beardictate-dev/pairs/`)

- `train.jsonl`, `val.jsonl`: `{"id","book","style","messages":[system,user,assistant]}`.
  The system and user messages are built with the same template, style rule
  and context trimming as `bd_cleanup::prompt` (mirrored in `eval/prompt.py`).
  The user message carries the ASR text **after the command pass**
  (`eval/commands.py` mirrors `crates/bd-commands`), because that is what the
  runtime LLM sees. The assistant message is the target.
- `train_pairs.jsonl`, `val_pairs.jsonl`: eval format
  `{id, category, context, raw, expected, style}`. Here `raw` is the simulated
  ASR text, which may contain spoken commands. You can score `val_pairs.jsonl`
  directly: `eval/run_eval.py --cases .../val_pairs.jsonl --limit 500 ...`.
- `report.md`: counts by split, category and style, noise event rates, and 10 random examples.

### How a pair is made

1. **Segments** (`segment.py`) are 1–4 sentence units, capped at 70 words.
   About 8% of long sentences are split at a comma (a mid-sentence pause).
   With p=0.35, short dialog paragraphs are joined into one segment (a quick
   exchange), so the target contains `\n\n`.
   - **Context** is the last two paragraphs of what the app would already have typed.
   - **Targets** start with `\n\n` when the segment begins a new paragraph.
   - **Quote convention** (same as the eval and the runtime prompt): every
     quotation is closed at the end of a segment unless the segment stops
     mid-sentence. If the original speech continues past a sentence-final
     segment boundary, the target closes the quote and the next segment
     reopens it. A segment that ends mid-sentence inside a quote leaves it
     open, which gives the "continue an open quote" examples.
   - **Skipped paragraphs**: those with unbalanced quotes (multi-paragraph
     speeches), digits, brackets or non-Latin characters. They still appear
     in context.
2. **Modernize** the source first. Semicolons become a comma or a sentence
   break, and a colon before a lowercase word becomes a comma. Nobody dictates
   a semicolon, and a target the model can't infer only teaches it to invent
   punctuation. `--keep-old-punct` turns this off.
3. **Simulate Parakeet** (`simulate.py`). It removes all quotation marks and
   paragraph breaks, turns dashes into a comma, a period or nothing, and
   turns `!` mostly into `.` or `,`. Some sentences are merged (mostly as
   comma splices) and some are split at commas. Commas are dropped or
   inserted, sentence starts are capitalized (including dialog tags after
   `?`), and a few are lowercased. Fillers (`um`, `uh`, `Um,` at a sentence
   start), stutter repeats, 2–4 word restarts and false starts with a
   changed word are injected. Sometimes the final period is dropped. Spoken
   commands appear at a low rate: `open quote … close quote` around about 4%
   of quoted spans and `new paragraph` at about 6% of breaks.
4. **Command pass, then guardrail check.** The simulated ASR goes through the
   command pass. Every target word must then appear in the model input in
   order. Pairs that fail are dropped and counted (60 of about 171k).
   Duplicates (same input and target) are dropped.

### Calibration against real Parakeet

`calibrate_asr.py` synthesizes 24 eval cases (with both a fluent and a
disfluent reading) using two Piper voices through sherpa-onnx's TTS. The
voices are in `/mnt/data/beardictate-dev/models/vits-piper-*`. No system TTS
was installed. It then transcribes them with Parakeet TDT 0.6B v3 int8 and
writes `transcripts.jsonl` and `summary.json` to
`/mnt/data/beardictate-dev/calibration/`. TTS reads more regularly than a
person dictating, so the run calibrates punctuation and segmentation habits
but not disfluency rates. These findings changed the defaults (2026-10-14):

- **Comma splices are common.** Parakeet writes 27% fewer sentence ends than
  the source and about 50% more commas. Examples: "smaller than she
  remembered, the porch sagged" and "the drive home, I remember". The merge
  rate went up (0.07 → 0.18, 70% of merges as a comma, 30% keep the capital).
- **`!` almost never survives** ("Stop!" → "Stop" / "Stop,"), and repeated
  questions become commas ("Hello? Hello?" → "Hello, hello,"). Added
  `exclaim_to_comma` and `question_to_comma`.
- **Segment-final punctuation is sometimes missing** ("Only me don't shoot").
  Added `no_final_punct`.
- **A capital is kept after a mid-sentence comma or colon** ("Martin said, We
  should…", "was a note Gone to the coast"). The cleanup has to fix it.
- **No quotes, semicolons, colons, dashes or hyphens appear**, which matches the simulator.
- **Things the simulator deliberately does not do**:
  - word errors: WER is about 5.5% even on clean TTS. Rare names are mangled
    (Ysolde → "Assault", Mykel → "Michael", Kestrel → "Kestral"), there are
    homophones ("come" → "comb"), and US spelling turns UK ("color" →
    "colour").
  - fillers misheard as words ("Um, the" → "On the", "uh" → "Oh").
  - stutters collapsed ("the the" → "the").

  The cleanup model must not repair any of these, because changing words is
  forbidden. If we want the model to be robust to them, the next step is to
  add substitutions to both raw *and* target.

## Fine-tune recipe (recommended, not yet run)

**Base models.** Qwen3-0.6B, 1.7B and 4B, from the base-instruct HF
checkpoints (`Qwen/Qwen3-0.6B` etc., Apache-2.0).

**Prompt.** The fine-tunes don't need a long instruction prompt. It costs
~450 prefill tokens per segment, which matters on CPU. Write a short
`bear-v1.txt` in the same two-section format (one line of system text plus
`{style_rule}`) and generate data with `--prompt`. The model manifest then
ships that prompt next to the weights (DESIGN.md "Cleanup model choices"). To
compare against stock models, keep generic-v1.

**Chat format.** Train with thinking disabled, exactly as inference runs.
Render the prompt with
`tokenizer.apply_chat_template(messages[:-1], add_generation_prompt=True, enable_thinking=False)`,
which ends in an empty `<think>\n\n</think>\n\n`. Append the target and
`<|im_end|>`. Compute the loss on the assistant tokens only. Keep a leading
`\n\n` in targets: it is the paragraph-break signal.

| | Qwen3-0.6B | Qwen3-1.7B | Qwen3-4B |
|---|---|---|---|
| method | full fine-tune | full FT (or LoRA r=64) | LoRA r=32–64, α=2r, dropout 0.05, all linear layers |
| precision | bf16 | bf16 | bf16 base (QLoRA 4-bit if memory-bound) |
| LR | 2e-5, cosine, 3% warmup | 1e-5 (LoRA: 2e-4) | 1e-4–2e-4 |
| epochs | 2 | 2 | 1–2 |
| effective batch | 64 sequences | 64 | 64 |
| max seq len | 2048 (examples are ~700–1,100 tokens with generic-v1, ~400 with a short prompt) | 2048 | 2048 |
| weight decay | 0.0 | 0.0 | 0.0 |
| GPU | 1× 24 GB | 1× 24–48 GB | 1× 24 GB (LoRA) |

Use TRL `SFTTrainer` with a completion-only collator (or Unsloth or axolotl,
which have the same knobs). Turn packing on only if the collator keeps
attention per example. With ~64k examples, one epoch is ~1k steps at batch 64.
The 890M iGPU on the dev box isn't practical for this. Rent a single 24–48 GB
GPU.

**Model selection.**
1. Watch val loss on `val.jsonl` (held-out books).
2. Every half epoch, export and run `eval/run_eval.py` on `eval/cases.jsonl`
   and `cases_uk.jsonl`.
3. The gate (DESIGN.md milestone 5) is to beat Gemma 4 12B on quote and break
   accuracy with **zero forbidden-edit cases**.
4. Also score ~500 rows of `val_pairs.jsonl`.

If quote accuracy plateaus, raise the dialog share (`--narration-keep`) and
the continuation share before adding epochs.

**Export to GGUF.**

```sh
# merge LoRA first if used: PeftModel.from_pretrained(base, adapter).merge_and_unload().save_pretrained("out/merged")
python /mnt/data/llama.cpp/convert_hf_to_gguf.py out/merged --outtype f16  --outfile out/bear-small-f16.gguf
python /mnt/data/llama.cpp/convert_hf_to_gguf.py out/merged --outtype q8_0 --outfile out/bear-small-Q8_0.gguf
# Q4_K_M needs llama-quantize (not built in /mnt/data/llama.cpp yet: cmake --build build --target llama-quantize)
/mnt/data/llama.cpp/build/bin/llama-quantize out/bear-large-f16.gguf out/bear-large-Q4_K_M.gguf Q4_K_M
```

The catalog plan is Small = 0.6B Q8_0, Medium = 1.7B Q8_0 and Large = 4B
Q4_K_M. Re-run the eval on the quantized file, not the HF checkpoint: Q4_K_M
can cost a few points on quote accuracy.
