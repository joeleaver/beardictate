# bd-platform: Windows manual test checklist

The Windows path compiles and passes clippy under `cargo xwin` but has not yet
run on a real Windows box. Use `cargo run -p bd-platform --example inject -- --delay 3`
then focus the target app.

1. Notepad, Word, Chrome (Google Docs), Scrivener, VS Code: type
   `“Go,” she said—café 😀` + newlines; emoji arrives as one character.
2. Slack/Teams with `newline = ShiftEnter`: the message is not sent.
3. RDP, a VM, a game / Electron app that drops typing: try `PasteOnly` and a
   higher `batch_delay`.
4. Paste with restore: copy rich text from Word plus an image, dictate with
   `PasteOnly`, paste manually afterwards: the original comes back; the
   dictated text is absent from Win+V history.
5. Restore timing: in a slow app / over RDP the dictated text (not the old
   clipboard) is pasted; raise `paste_settle` if not.
6. Elevated target (admin Notepad, Task Manager, regedit): `LeftOnClipboard`
   with a reason. Running beardictate as admin: types normally.
7. UAC prompt or lock screen during injection: `LeftOnClipboard`.
8. Hold Ctrl during injection: after 2 s text is on the clipboard, nothing
   typed, no Ctrl+Enter sent.
9. Alt-Tab mid-injection: typing stops, the remainder is on the clipboard.
10. `foreground_app()` returns `WINWORD.EXE`, `slack.exe`, …; per-app
    override applies.
11. Chord variants: Ctrl+Shift+V in Windows Terminal, Shift+Insert in a
    classic console.
12. Non-US layouts (German, French AZERTY): typing unaffected, Ctrl+V pastes.
