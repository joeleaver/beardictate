# Qwen3-4B Q4_K_M

prompt `prompts/generic-v1.txt`; command pass on; temperature 0.0; 171 cases

| | n | exact | word acc | quotes ok | quote F1 | breaks ok | punct F1 | forbidden words | cases w/ forbidden | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **overall** | 171 | 26% | 96.6% | 44% | 0.34 | 78% | 0.93 | 28 | 7 | 1346 | 2998 |
| action_beats | 12 | 8% | 99.4% | 8% | 0.08 | 75% | 0.89 | 0 | 0 | 1449 | 1820 |
| command_words_in_prose | 8 | 75% | 100.0% | 75% | 0.00 | 100% | 1.00 | 0 | 0 | 1189 | 1497 |
| dialog_continuation | 11 | 0% | 94.0% | 18% | 0.18 | 73% | 0.93 | 0 | 0 | 1296 | 2158 |
| dialog_inferred | 14 | 0% | 94.2% | 7% | 0.13 | 0% | 0.97 | 9 | 2 | 1686 | 2226 |
| dialog_tagged | 16 | 19% | 97.2% | 31% | 0.51 | 100% | 0.95 | 2 | 1 | 974 | 1396 |
| dialog_untagged | 10 | 30% | 76.7% | 60% | 0.63 | 50% | 0.88 | 12 | 2 | 582 | 1311 |
| embedded_quotes | 13 | 31% | 95.7% | 69% | 0.60 | 100% | 0.95 | 3 | 1 | 1580 | 1947 |
| interruptions | 9 | 0% | 100.0% | 0% | 0.22 | 56% | 0.78 | 0 | 0 | 1460 | 2836 |
| long_segments | 8 | 12% | 99.6% | 12% | 0.21 | 62% | 0.99 | 0 | 0 | 6172 | 7871 |
| missegmentation | 10 | 20% | 100.0% | 50% | 0.00 | 100% | 0.79 | 0 | 0 | 1544 | 2257 |
| names_odd_words | 10 | 40% | 96.5% | 50% | 0.43 | 90% | 0.93 | 2 | 1 | 1461 | 1896 |
| narration_fillers | 13 | 31% | 95.5% | 69% | 0.00 | 100% | 0.91 | 0 | 0 | 1401 | 2213 |
| questions_exclamations | 11 | 18% | 100.0% | 27% | 0.10 | 91% | 0.96 | 0 | 0 | 1952 | 2449 |
| repetition | 12 | 67% | 100.0% | 67% | 0.25 | 100% | 1.00 | 0 | 0 | 1032 | 1303 |
| spoken_commands | 14 | 43% | 100.0% | 100% | 1.00 | 79% | 0.86 | 0 | 0 | 863 | 1845 |

## Failures (showing 15 of 127)

**infer-06** (dialog_inferred) quotes, breaks, forbidden ['is', 'it', 'true', 'about', 'the', 'will', 'asked', 'caroline']

```
expected: ⏎⏎“Every word,” the lawyer replied.⏎⏎“Then I'll contest it.”⏎⏎“You can try.”
output:   “Is it true about the will?” asked Caroline.⏎⏎Every word, the lawyer replied. Then I'll contest it. You can try.
```

**untag-05** (dialog_untagged) forbidden ['is', 'anyone', 'there', 'kit', 'called', 'into', 'the', 'dark']

```
expected: ⏎⏎“Only me. Don't shoot.”
output:   “Is anyone there?” Kit called into the dark.⏎⏎"Only me. Don't shoot."
```

**untag-10** (dialog_untagged) quotes, breaks, forbidden ['said', 'the', 'inspector', 'well']

```
expected: ⏎⏎“Well what? I've told you everything.”
output:   “Well?” said the inspector.⏎⏎"well what? i've told you everything."
```

**embed-08** (embedded_quotes) quotes, forbidden ['said', 'the', 'farmer']

```
expected: “Can't you read?” The farmer pointed at the sign. “It says ‘No trespassing.’”
output:   "Can't you read?" the farmer said. "The farmer pointed at the sign. It says no trespassing."
```

**tag-11** (dialog_tagged) quotes, forbidden ["don't", 'move']

```
expected: “Don't move,” Theo whispered. “There's something in the reeds.”
output:   Theo whispered. "Don't move, there's something in the reeds."
```

**name-03** (names_odd_words) quotes, forbidden ['niching', 'comin']

```
expected: “Ain't nobody coming,” Jessup said. “Not tonight, not ever.”
output:   “AIN'T NICHING COMIN’,” JESSUP SAID. “NOT TONIGHT, NOT EVER.”
```

**infer-12** (dialog_inferred) quotes, breaks, forbidden ['name']

```
expected: ⏎⏎“Peregrine Ashdown.”⏎⏎The clerk's pen paused. “Like the bird?”⏎⏎“Like the bird.”
output:   “Name?”⏎Peregrine Ashdown. The clerk's pen paused. Like the bird? Like the bird.
```

**untag-03** (dialog_untagged) quotes, breaks

```
expected: ⏎⏎“No. Should I?”
output:   "No."
```

**cont-09** (dialog_continuation) quotes, breaks

```
expected: whatever anyone tells you. Promise me.”⏎⏎“I promise,” I said.
output:   "Whatever anyone tells you. Promise me. I promise."
```

**name-08** (names_odd_words) quotes, breaks

```
expected: “Ça va?” asked Mathilde.⏎⏎“Bien, merci,” he lied.
output:   “Ça va?” asked Mathilde. "Uh, bien, merci, he lied."
```

**cont-10** (dialog_continuation) quotes, breaks

```
expected: requesting immediate support. Over.”⏎⏎Base came back at once. “Kestrel, hold your position.”
output:   "Requesting immediate, immediate support. Over. Base came back at once. Kestrel, hold your position."
```

**infer-07** (dialog_inferred) quotes, breaks

```
expected: ⏎⏎“Devendra, do you know what time it is?”⏎⏎“Sorry. I know. I just needed to hear your voice.”
output:   "Devendra, do you know what time it is? Sorry. I know. I just, I needed to hear your voice."
```

**long-06** (long_segments) quotes, breaks

```
expected: ⏎⏎Detective Hwang walked around it twice before she said anything. “No skid marks. No damage. Keys in the ignition.”⏎⏎“Maybe he ran out of gas,” Pruitt offered.⏎⏎Hwang leaned in through the open window and tapped the gauge. “Three quarters full.”⏎⏎“Okay, so maybe he just wanted to walk.”⏎⏎“In January? In his socks?” She held up a pair of shoes from the footwell.
output:   Detective Hwang walked around it twice before she said anything. No skid marks. No damage. Keys in the ignition. Maybe he ran out of gas, Pruitt offered. Hwang leaned in through the open window and tapped the gauge. Three quarters full. Okay, so maybe he, maybe he just wanted to walk. In January? In his socks? She held up a pair of shoes from the footwell.
```

**untag-09** (dialog_untagged) quotes, breaks

```
expected: ⏎⏎“Thanks. I needed this.”
output:   Thanks. I needed this.
```

**infer-01** (dialog_inferred) quotes, breaks

```
expected: “Where are you going?” she asked.⏎⏎He didn't look up from the map. “I told you,” he said. “To the river.”
output:   "Where are you going?" she asked. "He didn't look up from the map. I told you," he said. "To the river."
```

