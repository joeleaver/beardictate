#!/usr/bin/env bash
# Puts Microsoft's Visual C++ runtime DLLs into a folder, to ship next to the
# exes (DESIGN.md "Windows install and first run": llama.cpp's DLLs share the
# DLL C runtime). Installed app-local, they need no redistributable installer
# and no admin rights, and Windows prefers them to an older copy in System32.
#
#   scripts/windows-vc-runtime.sh <dir>
#
# The source is pinned: Visual Studio 2022's x64 CRT redist package, taken
# from the release channel manifest (https://aka.ms/vs/17/release/channel ->
# VisualStudio.vsman, package Microsoft.VC.14.44.17.14.CRT.Redist.X64.base).
# It must be at least as new as the MSVC headers the code is compiled with
# (cargo-xwin's: _MSVC_STL_UPDATE 202503, VS 17.14); an older msvcp140.dll
# crashes newer STL code. Redistributable under the Visual Studio license
# terms; see packaging/THIRD-PARTY.txt.
set -euo pipefail
dest="${1:?usage: windows-vc-runtime.sh <dir>}"
version=14.44.35211
url=https://download.visualstudio.microsoft.com/download/pr/45d3b8dd-bced-4b37-9974-142f748d710c/4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5/Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix
sha256=4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5
dlls=(vcruntime140.dll vcruntime140_1.dll msvcp140.dll)

cache="${XDG_CACHE_HOME:-$HOME/.cache}/beardictate/vc-runtime-$version"
vsix="$cache/redist.vsix"
mkdir -p "$cache"
if ! { test -f "$vsix" && echo "$sha256  $vsix" | sha256sum -c --status; }; then
    curl -fsSL -o "$vsix.part" "$url"
    echo "$sha256  $vsix.part" | sha256sum -c --quiet
    mv "$vsix.part" "$vsix"
fi
mkdir -p "$dest"
for dll in "${dlls[@]}"; do
    unzip -p "$vsix" "Contents/VC/Redist/MSVC/*/x64/Microsoft.VC143.CRT/$dll" >"$dest/$dll"
    test -s "$dest/$dll" || { echo "$dll missing from the package" >&2; exit 1; }
done
echo "VC++ runtime $version: ${dlls[*]}"
