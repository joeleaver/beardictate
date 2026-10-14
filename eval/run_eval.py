import json, sys, time, urllib.request

PORT = sys.argv[1]
LABEL = sys.argv[2]

SYSTEM = """You clean up dictated prose for a novelist. The input is raw speech-to-text of the author narrating their manuscript.
Rules:
- Remove filler words (um, uh, like-as-filler, you know), stutters, repeated words, false starts, and noise tags like [cough].
- Fix capitalization and punctuation.
- When a character speaks, put their words in double quotation marks with correct dialog punctuation (comma inside the quote before a dialog tag, new paragraph for a new speaker).
- Spoken commands: "open quote"/"close quote" insert quotation marks; "new paragraph" starts a new paragraph.
- Do NOT change, add, or reword any other words. Do not summarize. Keep the author's voice exactly.
Output only the cleaned text."""

CASES = [
    {
        "context": "",
        "raw": "the rain had not stopped for three days and um Mara was was starting to think it never would",
    },
    {
        "context": "The rain had not stopped for three days, and Mara was starting to think it never would.",
        "raw": "where are you going she asked uh he didn't look up from the the map I told you he said to the river",
    },
    {
        "context": "\"Where are you going?\" she asked.\n\nHe didn't look up from the map. \"I told you,\" he said. \"To the river.\"",
        "raw": "the river's flooded you know that it's been flooded since Tuesday so what I'll swim he folded the map and and stood up",
    },
    {
        "context": "",
        "raw": "open quote get down close quote Tom hissed new paragraph she dropped to her knees um just as the [cough] the headlights swept across the field",
    },
]


def call(context, raw):
    user = (f"Previous text (for context only, do not output it):\n{context}\n\n" if context else "") + f"Raw dictation:\n{raw}"
    body = {
        "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": user}],
        "temperature": 0.0,
        "max_tokens": 300,
        "chat_template_kwargs": {"enable_thinking": False},
    }
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/v1/chat/completions", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    t = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=300))
    dt = time.time() - t
    out = r["choices"][0]["message"]["content"].strip()
    tm = r.get("timings", {})
    return out, dt, tm


call("", "warm up")  # warm-up
lat = []
for c in CASES:
    out, dt, tm = call(c["context"], c["raw"])
    lat.append(dt)
    print(f"--- [{LABEL}] {dt:.2f}s (prompt {tm.get('prompt_per_second', 0):.0f} t/s, gen {tm.get('predicted_per_second', 0):.0f} t/s)")
    print(out)
print(f"=== [{LABEL}] mean latency {sum(lat)/len(lat):.2f}s, max {max(lat):.2f}s\n")
