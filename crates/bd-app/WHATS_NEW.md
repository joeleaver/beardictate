# What's new in beardictate

Shown once in the app, at the first start of a new version (see
`src/whats_new.rs`). Each release is a `## <version>` heading followed by
`- ` lines, newest first; write them for the people using the app, not for
us. `site/publish.sh add` refuses a version with no section here.

## 0.1.24

- If your microphone is on but sends no sound (some USB microphones do for a while after the computer starts), beardictate notices, keeps reopening it and picks it up as soon as it sends sound. Meanwhile the bear glows amber; press your shortcut or click the bear to try straight away.
- In a list, when you pause for a moment in the middle of an item and carry on, the rest goes on the same item instead of a new line. Writing › Pauses turns it off.

## 0.1.23

- When your microphone stops working (unplugged, or Windows resets it), beardictate now gets it back by itself. Meanwhile the bear glows amber; press your shortcut or click the bear to try straight away.
- If another program holds your shortcut, beardictate keeps trying and takes it as soon as it’s free.
- Words that couldn’t be typed (say, while you held Ctrl) are no longer lost: they wait for you to let go, and anything left over is kept together on the clipboard. A small “!” on the bear tells you to press Ctrl+V.
- Say “hyphen” or “dash” for a hyphen: “well hyphen known” gives well-known.
- With Writing › Numbers set to the style guide, dates and years come out as numbers: “November sixth, twenty twenty six” gives November 6, 2026.
- No capital letter after a comma when you carry on in a new dictation, in your app and in the Bear Pad.
- Special words can start or end with a space: type the space, or write <space>.
- New in the last versions, in case you missed them: “bear, start a list” / “bear, end the list”, and “bear, title case”, “bear, all caps”, “bear, lowercase”, “bear, normal case”. They’re in the bear’s menu too.
