#!/usr/bin/env bash
# Builds beardictate and installs it for this user under ~/.local:
#   ~/.local/lib/beardictate/   the app and the libraries it loads (sherpa-onnx, onnxruntime)
#   ~/.local/bin/beardictate    a link to it
#   ~/.local/share/applications/beardictate.desktop   (KDE's shortcut portal needs it)
#   ~/.local/share/icons/hicolor/scalable/apps/beardictate.svg
# With --autostart, it also starts at login. Models are not copied: set
# models_dir in ~/.config/beardictate/settings.toml, or download them with `bd`.
#
#   scripts/install-linux.sh [--autostart] [--no-build]
set -euo pipefail

autostart=false
build=true
for arg in "$@"; do
    case "$arg" in
        --autostart) autostart=true ;;
        --no-build) build=false ;;
        *) echo "usage: $0 [--autostart] [--no-build]" >&2; exit 2 ;;
    esac
done

repo="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}"
features="${BD_FEATURES:-vulkan,pipewire}"
# Every build is an alpha build for now (Joe, 2026-10-19): telemetry on,
# with the shared key (docs/telemetry.md), which isn't in the repo.
BD_TELEMETRY_KEY="${BD_TELEMETRY_KEY:-$(cat "$HOME/.config/beardictate-release/telemetry-key" 2>/dev/null || true)}"
[ -n "$BD_TELEMETRY_KEY" ] || { echo "no telemetry key: set BD_TELEMETRY_KEY or create ~/.config/beardictate-release/telemetry-key" >&2; exit 1; }
export BD_TELEMETRY_KEY
# Our sherpa-onnx build (v1.13.8 plus Nemotron beam search and hotwords).
SHERPA_ONNX_LIB_DIR="$("$repo/scripts/build-sherpa-onnx.sh" linux)"
export SHERPA_ONNX_LIB_DIR
if $build; then
    (cd "$repo" && cargo build --release -p bd-app --features "$features,nemotron-hotwords,alpha")
fi

lib="$HOME/.local/lib/beardictate"
bin="$HOME/.local/bin"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
mkdir -p "$lib" "$bin" "$apps" "$icons"

# The binary finds its shared libraries next to itself ($ORIGIN rpath).
install -m 755 "$target/release/beardictate" "$lib/beardictate"
for so in "$SHERPA_ONNX_LIB_DIR"/lib{sherpa-onnx-c-api,onnxruntime}.so; do
    install -m 644 "$so" "$lib/"
done
ln -sf "$lib/beardictate" "$bin/beardictate"

sed "s|@EXEC@|$lib/beardictate|" "$repo/packaging/linux/beardictate.desktop" > "$apps/beardictate.desktop"
install -m 644 "$repo/design/logo/mark.svg" "$icons/beardictate.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true

if $autostart; then
    mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
    cp "$apps/beardictate.desktop" "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/"
fi

echo "Installed. Start it from the app menu, or run: $lib/beardictate"
