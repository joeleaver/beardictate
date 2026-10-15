#!/usr/bin/env bash
# Cross-builds the Windows release on Linux with cargo-xwin and stages what
# ships into one folder, for `package-windows.ps1 -Stage <dir>` on a Windows
# machine (DESIGN.md "Windows install and first run"). Cleanup runs on the
# GPU through llama.cpp's Vulkan backend, loaded at run time (`vulkan-dl`).
#
#   scripts/cross-build-windows.sh <stage-dir>
#
# Needs cargo-xwin, LLVM (clang-cl, lld-link, llvm-lib, llvm-dlltool,
# llvm-readobj) and ninja on PATH, plus what windows-vulkan-sdk.sh needs.
# VULKAN_SDK, when set, names a stand-in SDK made by that script; otherwise
# one is made under the target folder. CARGO_TARGET_DIR is honoured.
set -euo pipefail
stage="${1:?usage: cross-build-windows.sh <stage-dir>}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}"
triple=x86_64-pc-windows-msvc
if [ -z "${VULKAN_SDK:-}" ]; then
    export VULKAN_SDK="$target/vulkan-sdk-windows"
    "$repo/scripts/windows-vulkan-sdk.sh" "$VULKAN_SDK"
fi

# llama.cpp's DLLs hand C runtime objects to each other (a FILE* from
# ggml_fopen), so they must share one C runtime: the DLL one (/MD), unlike
# .cargo/config.toml's static default. RUSTFLAGS replaces that file's flags.
export LLAMA_STATIC_CRT=0 RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=-crt-static"
# A clang-cl that compiles every CPU variant (see scripts/xwin/clang-cl).
export PATH="$repo/scripts/xwin:$PATH"
# cargo-xwin's CMake toolchain drops the Windows libraries CMake links by
# default, which llama.cpp's DLLs need (the registry, CommandLineToArgvW).
# lld-link, like link.exe, takes extra options from LINK; the host compiler
# building ggml's shader tool doesn't.
for lib in kernel32 user32 shell32 ole32 uuid advapi32; do
    LINK="${LINK:-} /defaultlib:$lib"
done
export LINK
cd "$repo"
cargo xwin build --release --locked --target "$triple" -p bd-cli -p bd-app \
    --features bd-cli/vulkan-dl,bd-app/vulkan-dl
release="$target/$triple/release"
# llama-cpp-sys hard-links its DLLs next to the exes only when none is there
# yet, so take them from its newest vulkan-dl build folder instead (other
# feature sets, e.g. a clippy run without vulkan-dl, leave folders without
# backends).
out="$(ls -td "$release"/build/llama-cpp-sys-2-*/out/backends/ggml-vulkan.dll 2>/dev/null | head -1)"
out="${out%/backends/ggml-vulkan.dll}"
test -n "$out" || { echo "no vulkan-dl build of llama.cpp in $release" >&2; exit 1; }

rm -rf "$stage"
mkdir -p "$stage"
cp "$release"/{beardictate.exe,bd.exe,sherpa-onnx-c-api.dll,onnxruntime.dll,onnxruntime_providers_shared.dll} "$stage/"
# llama.cpp's libraries (not llama-common.dll, which nothing here imports)
# and its backends: ggml-cpu-<variant>.dll for each x86 level, ggml-vulkan.dll.
cp "$out"/bin/{llama,ggml,ggml-base}.dll "$out"/backends/*.dll "$stage/"
"$repo/scripts/windows-vc-runtime.sh" "$stage"

# Every DLL a binary imports must ship, or come with Windows (no VC++
# redistributable); only the Vulkan backend may import the Vulkan loader,
# which only GPU drivers install.
readobj="$(command -v llvm-readobj || echo /usr/lib/llvm-21/bin/llvm-readobj)"
for f in "$stage"/*.exe "$stage"/*.dll; do
    imports="$("$readobj" --coff-imports "$f" | sed -n 's/^ *Name: //p')"
    for dll in $(grep -iE '^(msvcp|vcruntime|concrt|vcomp)' <<<"$imports"); do
        if ! ls "$stage" | grep -qixF "$dll"; then echo "$f needs $dll, which doesn't ship" >&2; exit 1; fi
    done
    if [ "$(basename "$f")" != ggml-vulkan.dll ] && grep -qi '^vulkan-1' <<<"$imports"; then
        echo "$f imports vulkan-1.dll" >&2
        exit 1
    fi
done
ls -l "$stage"
