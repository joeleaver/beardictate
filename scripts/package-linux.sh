#!/usr/bin/env bash
# Builds a Linux release tarball: dist/beardictate-<version>-linux-x86_64.tar.gz,
# holding the app, the sherpa-onnx and onnxruntime libraries it loads, its
# desktop entry and icon, the notices, and install.sh (which installs it for
# the current user). Other libraries come from the system (GTK 3, libxdo,
# Vulkan, PipeWire), and glibc must be at least the build machine's.
#
#   scripts/package-linux.sh [--no-build]
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}"
features="${BD_FEATURES:-vulkan,pipewire}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)"
name="beardictate-$version-linux-x86_64"
# Our sherpa-onnx build (v1.13.8 plus Nemotron beam search and hotwords).
SHERPA_ONNX_LIB_DIR="$("$repo/scripts/build-sherpa-onnx.sh" linux)"
export SHERPA_ONNX_LIB_DIR
if [ "${1:-}" != --no-build ]; then
    (cd "$repo" && cargo build --release --locked -p bd-app --features "$features,nemotron-hotwords")
fi

dist="$repo/dist"
stage="$dist/$name"
rm -rf "$stage" "$dist/$name.tar.gz"
mkdir -p "$stage"
install -m 755 "$target/release/beardictate" "$stage/"
strip "$stage/beardictate"
for so in "$SHERPA_ONNX_LIB_DIR"/lib{sherpa-onnx-c-api,onnxruntime}.so; do
    install -m 644 "$so" "$stage/"
done
install -m 644 "$repo/packaging/linux/beardictate.desktop" "$stage/"
install -m 644 "$repo/design/logo/mark.svg" "$stage/beardictate.svg"
install -m 755 "$repo/packaging/linux/install.sh" "$stage/"
install -m 644 "$repo/packaging/THIRD-PARTY.txt" "$stage/"
glibc="$(objdump -T "$stage/beardictate" "$stage"/*.so* 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1)"
cat > "$stage/README.txt" <<README
beardictate $version for Linux (x86_64)

Install for your user (no root needed):   ./install.sh
Start it at login too:                    ./install.sh --autostart
Remove it:                                ./install.sh --uninstall

Needs: a recent Linux ($glibc or newer) with GTK 3, libxdo, a Vulkan
loader and PipeWire; KDE Plasma or GNOME on Wayland, or X11. The speech
models (about 640 MB) download on first run; cleanup models from the
Cleanup page. https://beardictate.jkbase.app
README
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"
echo "$dist/$name.tar.gz ($(du -h "$dist/$name.tar.gz" | cut -f1); needs $glibc)"
