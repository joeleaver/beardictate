#!/usr/bin/env bash
# End-to-end test of the tray app, isolated from your desktop: a nested
# `kwin_wayland --virtual` under its own D-Bus, a separate rootless Xwayland
# inside it (XTEST input from X clients doesn't reach the Xwayland KWin
# starts), an xterm to type into, and a WAV standing in for the microphone.
# Presses the hotkey to start, `beardictate --toggle` to stop, then prints
# what was typed. Config, data and logs go to a temp dir.
#
#   scripts/e2e-nested.sh <dictation.wav> [seconds-to-listen]
#
# Needs: kwin_wayland, Xwayland, xterm, xdotool, dbus-run-session, a release
# build of bd-app, and models (BD_MODELS_DIR, default the dev store).
set -euo pipefail
wav="$(realpath "$1")"
listen="${2:-25}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$repo/target}/release/beardictate"
models="${BD_MODELS_DIR:-/mnt/data/beardictate-dev/models}"
work="$(mktemp -d)"
mkdir -p "$work/config/beardictate"
printf 'models_dir = "%s"\nparagraphs = "auto"\nnotifications = false\n' "$models" > "$work/config/beardictate/settings.toml"

cat > "$work/inner.sh" <<INNER
#!/usr/bin/env bash
Xwayland :99 -rootless -noreset > "$work/xwayland.log" 2>&1 &
sleep 2
export DISPLAY=:99
unset WAYLAND_DISPLAY
: > "$work/typed.txt"
xterm -geometry 100x20+0+0 -e bash -c "stty -icanon -echo; cat > '$work/typed.txt'" &
sleep 2
XDG_SESSION_TYPE=x11 XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data" XDG_STATE_HOME="$work/state" \
    BD_TEST_WAV="$wav" "$bin" > "$work/app.log" 2>&1 &
app=\$!
for i in \$(seq 1 120); do grep -q "models loaded" "$work/app.log" && break; sleep 1; done
xdotool search --class xterm windowfocus; sleep 1
xdotool key F24; sleep 0.5          # the first XTEST key on a fresh server is lost
xdotool key ctrl+alt+space          # start (the global hotkey)
sleep $listen
XDG_RUNTIME_DIR="\$XDG_RUNTIME_DIR" "$bin" --toggle   # stop
sleep 25                            # let the last sentences be cleaned and typed
kill \$app; sleep 2; kill %1 %2 2>/dev/null || true
INNER
chmod +x "$work/inner.sh"
timeout $((listen + 200)) dbus-run-session -- kwin_wayland --virtual --xwayland --socket "wayland-bde2e-$$" \
    --exit-with-session "$work/inner.sh" > "$work/kwin.log" 2>&1 || true
echo "== typed ($work/typed.txt):"
cat "$work/typed.txt"; echo
echo "== app log: $work/app.log"
grep -E "hotkey|toggle|typing|repaired|rejected|WARN|ERROR" "$work/app.log" | grep -v control-looking || true
