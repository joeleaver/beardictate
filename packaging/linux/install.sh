#!/usr/bin/env bash
# Installs beardictate from this unpacked release for the current user,
# under ~/.local (no root):
#   ~/.local/lib/beardictate/   the app and the libraries it loads
#   ~/.local/bin/beardictate    a link to it
#   ~/.local/share/applications/beardictate.desktop   (KDE's shortcut portal needs it)
#   ~/.local/share/icons/hicolor/scalable/apps/beardictate.svg
# The speech models download on first run (about 640 MB).
#
#   ./install.sh [--autostart]      install, optionally starting at login
#   ./install.sh --uninstall        remove it (your settings and models stay)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
lib="$HOME/.local/lib/beardictate"
bin="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor/scalable/apps"
autostart_dir="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

case "${1:-}" in
    --uninstall)
        rm -rf "$lib"
        rm -f "$bin/beardictate" "$apps/beardictate.desktop" "$icons/beardictate.svg" "$autostart_dir/beardictate.desktop"
        echo "Removed. Settings (~/.config/beardictate) and models (~/.local/share/beardictate) are left in place."
        exit 0 ;;
    ""|--autostart) ;;
    *) echo "usage: $0 [--autostart | --uninstall]" >&2; exit 2 ;;
esac

mkdir -p "$lib" "$bin" "$apps" "$icons"
# The app finds its libraries next to itself ($ORIGIN rpath).
install -m 755 "$here/beardictate" "$lib/beardictate"
for so in "$here"/lib*.so*; do
    install -m 644 "$so" "$lib/"
done
ln -sf "$lib/beardictate" "$bin/beardictate"
sed "s|@EXEC@|$lib/beardictate|" "$here/beardictate.desktop" > "$apps/beardictate.desktop"
install -m 644 "$here/beardictate.svg" "$icons/beardictate.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true

if [ "${1:-}" = --autostart ]; then
    mkdir -p "$autostart_dir"
    cp "$apps/beardictate.desktop" "$autostart_dir/"
fi

if ! "$lib/beardictate" --help >/dev/null 2>&1; then
    echo "Installed, but beardictate doesn't start here. It needs a recent Linux (glibc 2.43 or newer)" >&2
    echo "with GTK 3, libxdo, a Vulkan loader and PipeWire. Details: ldd $lib/beardictate" >&2
    exit 1
fi
echo "Installed. Start beardictate from your app menu, or run: $lib/beardictate"
