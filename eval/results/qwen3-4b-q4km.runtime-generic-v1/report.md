# Qwen3-4B Q4_K_M

prompt `../crates/bd-cleanup/prompts/generic-v1.txt`; command pass on; temperature 0.0; 171 cases

| | n | exact | word acc | quotes ok | quote F1 | breaks ok | punct F1 | forbidden words | cases w/ forbidden | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **overall** | 171 | 32% | 95.2% | 47% | 0.38 | 77% | 0.92 | 66 | 12 | 1672 | 4660 |
| action_beats | 12 | 8% | 94.3% | 8% | 0.17 | 75% | 0.82 | 2 | 1 | 2306 | 3218 |
| command_words_in_prose | 8 | 75% | 96.1% | 88% | 0.50 | 100% | 0.97 | 0 | 0 | 2026 | 2532 |
| dialog_continuation | 11 | 9% | 78.8% | 36% | 0.32 | 73% | 0.88 | 36 | 2 | 2101 | 6825 |
| dialog_inferred | 14 | 0% | 93.7% | 0% | 0.05 | 0% | 0.96 | 9 | 1 | 2727 | 4413 |
| dialog_tagged | 16 | 25% | 99.1% | 31% | 0.51 | 100% | 1.00 | 0 | 0 | 1397 | 2127 |
| dialog_untagged | 10 | 0% | 93.3% | 20% | 0.29 | 40% | 0.95 | 4 | 1 | 1073 | 2196 |
| embedded_quotes | 13 | 31% | 95.0% | 54% | 0.48 | 100% | 0.94 | 5 | 2 | 2198 | 3080 |
| interruptions | 9 | 0% | 100.0% | 0% | 0.33 | 56% | 0.78 | 0 | 0 | 1334 | 1619 |
| long_segments | 8 | 38% | 99.6% | 38% | 0.00 | 62% | 1.00 | 0 | 0 | 5815 | 7178 |
| missegmentation | 10 | 30% | 96.4% | 50% | 0.25 | 100% | 0.76 | 2 | 1 | 1736 | 2565 |
| names_odd_words | 10 | 50% | 97.6% | 80% | 0.77 | 90% | 0.97 | 1 | 1 | 2597 | 3384 |
| narration_fillers | 13 | 69% | 96.9% | 92% | 0.00 | 100% | 0.98 | 0 | 0 | 1549 | 1842 |
| questions_exclamations | 11 | 45% | 95.3% | 55% | 0.58 | 91% | 0.93 | 6 | 2 | 1092 | 1592 |
| repetition | 12 | 67% | 93.5% | 67% | 0.44 | 100% | 0.90 | 0 | 0 | 1574 | 1869 |
| spoken_commands | 14 | 36% | 98.2% | 93% | 0.92 | 71% | 0.86 | 1 | 1 | 1485 | 3261 |

## Failures (showing 15 of 117)

**cont-04** (dialog_continuation) quotes, forbidden ['let', 'me', 'tell', 'you', 'about', 'my', 'brother', 'the', 'old', 'woman', 'said', 'he', 'was', 'the', 'clever', 'one', 'the', 'one', 'who', 'could', 'fix', 'anything']

```
expected: a clock, a radio, a broken heart. And then one day he just walked away.” She folded her hands.
output:   “Let me tell you about my brother,” the old woman said. “He was the clever one, the one who could fix anything, a clock, a radio, a broken heart. And then one day he, he just walked away. She folded her hands."
```

**cont-10** (dialog_continuation) quotes, breaks, forbidden ['the', 'radio', 'crackled', 'base', 'this', 'is', 'kestrel', 'we', 'are', 'taking', 'fire', 'from', 'the', 'ridge']

```
expected: requesting immediate support. Over.”⏎⏎Base came back at once. “Kestrel, hold your position.”
output:   The radio crackled. “Base, this is Kestrel. We are taking fire from the ridge, requesting immediate, immediate support. Over. Base came back at once. Kestrel, hold your position.”
```

**infer-09** (dialog_inferred) quotes, breaks, forbidden ['where', 'were', 'you', 'on', 'the', 'night', 'of', 'the', 'ninth']

```
expected: ⏎⏎“At home.”⏎⏎“Alone?”⏎⏎“With my cat.”⏎⏎“Does the cat have a name?”⏎⏎“Biscuit.”
output:   “Where were you on the night of the ninth?”  ⏎At home. Alone? With my cat. Does the cat have a name? Biscuit.
```

**untag-10** (dialog_untagged) quotes, breaks, forbidden ['said', 'the', 'inspector', 'well']

```
expected: ⏎⏎“Well what? I've told you everything.”
output:   “Well?” said the inspector.  ⏎"Well what? I've told you everything."
```

**ques-06** (questions_exclamations) quotes, breaks, forbidden ['he', 'asked', 'he', 'asked']

```
expected: “Did you lock the back door?”⏎⏎“Yes.”⏎⏎“Are you sure?”⏎⏎“Yes, Mom.”
output:   “Did you lock the back door?” he asked. “Yes. Are you sure?” he asked. “Yes, Mom.”
```

**embed-08** (embedded_quotes) quotes, forbidden ['said', 'the', 'farmer']

```
expected: “Can't you read?” The farmer pointed at the sign. “It says ‘No trespassing.’”
output:   “Can't you read?” the farmer said. “The farmer pointed at the sign. It says no trespassing.”
```

**beat-09** (action_beats) quotes, breaks, forbidden ['felix', 'said']

```
expected: ⏎⏎“You can't be serious,” Felix said, setting down his glass.⏎⏎“I'm completely serious.”
output:   Felix said, "You can't be serious." I'm completely serious.
```

**embed-06** (embedded_quotes) quotes, forbidden ['she', 'said']

```
expected: She said that she would be home by nine, but she wasn't.
output:   “she would be home by nine,” she said. “but she wasn't.”
```

**ques-08** (questions_exclamations) quotes, forbidden ['she', 'asked']

```
expected: “Where were you? Where were you all night?” Her voice cracked.
output:   “Where were you?” she asked. “Where were you all night?” her voice cracked.
```

**seg-03** (missegmentation) forbidden ['ruth', 'said']

```
expected: “I told him not to come. He came anyway,” Ruth said.
output:   Ruth said, "I told him. Not to come. He came anyway."
```

**cmd-03** (spoken_commands) quotes, forbidden ['im']

```
expected: “I'm fine,” she lied.
output:   “Im fine.” she lied.
```

**name-03** (names_odd_words) forbidden ['nownever']

```
expected: “Ain't nobody coming,” Jessup said. “Not tonight, not ever.”
output:   “AIN'T NOWNEVER COMING,” JESSUP SAID. “NOT TONIGHT, NOT EVER.”
```

**infer-01** (dialog_inferred) quotes, breaks

```
expected: “Where are you going?” she asked.⏎⏎He didn't look up from the map. “I told you,” he said. “To the river.”
output:   Where are you going? She asked. He didn't look up from the map. I told you. "To the river."
```

**infer-07** (dialog_inferred) quotes, breaks

```
expected: ⏎⏎“Devendra, do you know what time it is?”⏎⏎“Sorry. I know. I just needed to hear your voice.”
output:   Devendra, do you know what time it is? Sorry. I know. I just, I needed to hear your voice.
```

**infer-02** (dialog_inferred) quotes, breaks

```
expected: ⏎⏎“The river's flooded, you know that. It's been flooded since Tuesday.”⏎⏎“So what? I'll swim.” He folded the map and stood up.
output:   The river's flooded, you know that. It's been flooded since Tuesday. So what? I'll swim. He folded the map and and stood up.
```

