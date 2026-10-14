# eval

The eval set and scorer for the cleanup model (DESIGN.md milestone 0). Every
case is original prose written for this set.

| file | what it is |
|---|---|
| `cases.jsonl` | 151 US-style cases `{id, category, context, raw, expected, notes?}` |
| `cases_uk.jsonl` | 20 UK-style cases (single curly quotes, logical punctuation), with `"style": "uk"` |
| `cases_src/*.txt` | the readable source of both files. Edit these, then run `build_cases.py` |
| `build_cases.py` | builds the JSONL files and validates every case against the word guardrail |
| `score.py` | the scorer: cases + outputs.jsonl → metrics and a markdown report |
| `run_eval.py` | the runner: any OpenAI-compatible endpoint (or `--launch` a GGUF with llama-server) → outputs.jsonl → score |
| `prompts/generic-v1.txt` | the baseline prompt (same two-section format as `crates/bd-cleanup/prompts/`) |
| `prompt.py` | renders a prompt file exactly as `bd_cleanup::prompt` does (style rule, context trim, empty-context sentinel) |
| `commands.py` | Python mirror of `crates/bd-commands`; `tests/test_commands.py` checks it against the Rust test tables |
| `textnorm.py` | word normalization, LCS alignment, the guardrail check, quote/paragraph tokenization |
| `results/<model>.<prompt>/` | `report.md` and `outputs.jsonl` for each baseline run |

## Conventions the cases encode

- **Raw** is Parakeet-style. It already has casing and punctuation, but never
  quotation marks. Its sentence boundaries are imperfect (`Where's the key? He
  asked.`), and it contains fillers and repeats.
- **Spoken commands** appear in `raw` ("Open quote. … Close quote."). By
  default the runner applies the command pass first, as the runtime does, so
  the model sees marks. `--no-command-pass` sends them verbatim.
- **Paragraphs.** The output is the cleaned segment only. It starts with
  `\n\n` when the segment begins a new paragraph relative to the context. A
  "new paragraph" command gives `\n\n` (even at the very end), and "new line"
  gives `\n`.
- **Quotes.** US uses “ ”, with ‘ ’ inside. UK uses ‘ ’, with “ ” inside.
  Apostrophes stay straight `'`, as the ASR writes them. Close every
  quotation by the end of the segment unless the segment stops mid-sentence.
  If the context ends inside an open quote, continue it without a new
  opening mark. An output may then *begin* with the closing mark (`cont-06`,
  `cont-08`), which the pipeline joins without a space.
- **Thoughts, indirect speech and names are not quoted.** Rhetorical
  questions in narration aren't either.
- **`expected` changes no words.** `build_cases.py` fails if any expected
  word is not in the command-passed raw, in order. Dropped words that are
  neither fillers nor nearby repeats are listed for review. Only `narr-06`
  (a false start with a changed verb) and `cont-07` (re-said words across
  segments) are flagged, both deliberately.

Categories (151): narration_fillers 12, dialog_tagged 12, dialog_untagged 10,
dialog_inferred 12, dialog_continuation 10, action_beats 10, embedded_quotes 8,
spoken_commands 12, command_words_in_prose 8, repetition 11,
names_odd_words 10, missegmentation 10, questions_exclamations 10,
interruptions 8, long_segments 8.

## Metrics (`score.py`)

| metric | meaning |
|---|---|
| exact | output == expected, ignoring straight vs curly quote glyphs and spacing |
| word acc | 1 − WER on normalized words (case, punctuation and quotes ignored) |
| quotes ok | same quoted spans (by word boundaries, aligned to expected), per case |
| quote F1 | micro F1 over quoted spans |
| breaks ok | same paragraph and line breaks at the same word positions, including a leading or trailing break |
| punct F1 | micro F1 over (word, mark) for `, . ; : ! ? — …` |
| forbidden | words added or substituted relative to the command-passed raw (what the runtime guardrail rejects). Should be 0 |
| p50/p95 ms | request latency |

```sh
PY=/mnt/data/venvs/beardictate/bin/python
$PY eval/build_cases.py
$PY -m pytest -q eval/tests
# score existing outputs
$PY eval/score.py eval/cases.jsonl eval/cases_uk.jsonl results/x/outputs.jsonl --report results/x/report.md
# run a model (one llama-server at a time; the runner starts and kills it)
$PY eval/run_eval.py --launch /mnt/data/models/Qwen3-4B/Qwen3-4B-Q4_K_M.gguf --no-think \
    --prompt eval/prompts/generic-v1.txt --out eval/results/qwen3-4b-q4km.eval-generic-v1
# or any running OpenAI-compatible server
$PY eval/run_eval.py --base-url http://127.0.0.1:11434/v1 --model qwen3:4b --out /tmp/ollama-run
```

The default `--prompt` is `crates/bd-cleanup/prompts/generic-v1.txt` when that
file exists, else `eval/prompts/generic-v1.txt`. Each report records which
prompt it used.
