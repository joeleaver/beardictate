# Boundary labelling instructions

These are the instructions given to the labeller (a Claude agent, or a
person) for `boundaries.jsonl` (`boundaries.py`). Labels are written to `labels.jsonl` in the
data directory, one line per boundary: `{"id": "...", "label": "..."}`.

Each boundary is a pause in someone's dictation, between segment A and
segment B. You get the segment before A (`before`), A (`a_text`), B
(`b_text`) and the one after B (`after`). The recognizer's punctuation at the
end of A is **not evidence**: when the pause was long, the app added a full
stop there itself. Judge the words.

Normal dictation (`list_mode` false):

- `mid`: B carries on the sentence A started. A alone is not a finished
  sentence, or B only makes sense attached to it ("I went to the" / "shop
  on Friday"; "When the rain stops," / "we'll walk down"). A trailing
  afterthought that grammatically attaches to A ("…the parcel." / "By
  courier.") counts as `mid` only
  when a careful writer would join them with a comma, not a full stop.
- `end`: A is a complete sentence or question and B starts a new one, or B
  is a separate fragment a writer would set apart.
- `unsure`: you can't tell, or the text is too garbled to judge.

List mode (`list_mode` true), where each utterance becomes a bullet:

- `mid`: B is the rest of the same item as A (the speaker paused inside one
  item).
- `end`: B is a new item, or a command.
- `unsure`: as above.

Judge from the words alone, quickly; don't overthink borderline cases, mark
them `unsure`.
