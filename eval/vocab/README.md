# vocab: invented names through the ASR

Eight sentences full of invented names (`sentences.txt`), read by the Piper
voice, through Parakeet with and without the custom-words feature.
`words.txt` is the list in `bd --words` format; its "heard" forms were taken
from these same transcripts, so the full-list result only shows the fix-up
pass works, not how well it generalizes.

```bash
M=/mnt/data/beardictate-dev/models
/mnt/data/venvs/beardictate/bin/python -I eval/vocab/tts_lines.py $M/vits-piper-en_US-ryan-medium /tmp/names eval/vocab/sentences.txt
bd file /tmp/names/n*.wav --cleanup off                                  # baseline
cut -d: -f1 eval/vocab/words.txt > /tmp/plain.words
bd file /tmp/names/n*.wav --cleanup off --words /tmp/plain.words         # hotwords only
bd file /tmp/names/n*.wav --cleanup off --words eval/vocab/words.txt     # hotwords + fix-ups
```

2026-10-14, 22 names: greedy 4 right; hotwords at 2.0 about 10, at 2.5
12 (but "whispered" became "whisper"), at 3.0 15 (more such damage);
hotwords plus the fix-up list 22. Hotwords also capitalize a few ordinary
words that start like a name ("Fog", "Twins"). Default boost: 2.0.
