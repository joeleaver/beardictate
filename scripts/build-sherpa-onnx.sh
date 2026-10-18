#!/usr/bin/env bash
# Builds our own sherpa-onnx shared libraries: v1.13.8 plus upstream PR #3895
# (sherpa-onnx-3895.patch, merged after v1.13.8), which adds
# modified_beam_search, and so hotwords, to the streaming NeMo transducer that
# Nemotron uses. The C API is unchanged from v1.13.8, so the `sherpa-onnx`
# 1.13.8 crate (feature `shared`) links them through SHERPA_ONNX_LIB_DIR.
#
#   scripts/build-sherpa-onnx.sh linux|windows
#
# Prints the lib folder, laid out like the crate's prebuilt download:
#   linux:   $target/sherpa-onnx/v1.13.8-bd1-linux-x64/lib (lib*.so)
#   windows: $target/sherpa-onnx/v1.13.8-bd1-win-x64/lib (*.dll, *.lib; /MT, as upstream's shared-MT build)
# and skips the build when that folder already exists. Progress goes to stderr.
# Linux builds in upstream's release container (manylinux2014: glibc 2.17),
# so needs docker. Windows cross-builds with cargo-xwin's clang-cl toolchain,
# so needs cargo-xwin, LLVM and ninja, as cross-build-windows.sh does.
# CARGO_TARGET_DIR is honoured.
set -euo pipefail
platform="${1:?usage: build-sherpa-onnx.sh linux|windows}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}"
version=v1.13.8
build_id=bd1  # bump when the patch or the build options change
case "$platform" in
    linux) name="$version-$build_id-linux-x64" ;;
    windows) name="$version-$build_id-win-x64" ;;
    *) echo "usage: build-sherpa-onnx.sh linux|windows" >&2; exit 2 ;;
esac
root="$target/sherpa-onnx"
out="$root/$name"
if [ -d "$out/lib" ]; then echo "$out/lib"; exit 0; fi

# A failed build's folder is reused, so a rerun resumes it.
work="$root/build-$name"
if [ ! -f "$work/patched" ]; then
    rm -rf "$work"
    mkdir -p "$work"
    git -c advice.detachedHead=false clone -q --depth 1 --branch "$version" \
        https://github.com/k2-fsa/sherpa-onnx "$work/src" >&2
    git -C "$work/src" apply "$repo/scripts/sherpa-onnx-3895.patch"
    touch "$work/patched"
fi

# Only the libraries: no binaries, examples, microphone or websocket code.
# TTS and speaker diarization stay on, as in upstream's build, so the C API
# exports the same symbols.
options=(
    -D CMAKE_BUILD_TYPE=Release
    -D BUILD_SHARED_LIBS=ON
    -D SHERPA_ONNX_ENABLE_BINARY=OFF
    -D SHERPA_ONNX_BUILD_C_API_EXAMPLES=OFF
    -D SHERPA_ONNX_ENABLE_PORTAUDIO=OFF
    -D SHERPA_ONNX_ENABLE_WEBSOCKET=OFF
    -D SHERPA_ONNX_ENABLE_TESTS=OFF
    -D SHERPA_ONNX_ENABLE_PYTHON=OFF
    -D SHERPA_ONNX_ENABLE_TTS=ON
    -D SHERPA_ONNX_ENABLE_SPEAKER_DIARIZATION=ON
    -D BUILD_ESPEAK_NG_EXE=OFF
)
rm -rf "$out.tmp"
mkdir -p "$out.tmp/lib"

case "$platform" in
linux)
    docker run --rm --user "$(id -u):$(id -g)" -e HOME=/work -v "$work:/work" -w /work/src \
        quay.io/pypa/manylinux2014_x86_64 \
        bash -c "cmake -S . -B build ${options[*]} -D CMAKE_INSTALL_PREFIX=/work/src/install >&2 &&
                 cmake --build build -j$(nproc) >&2 && cmake --install build >&2"
    # Installed, not built, libraries: installing sets their rpath to $ORIGIN.
    cp -L "$work/src/install/lib"/lib{sherpa-onnx-c-api,sherpa-onnx-cxx-api,onnxruntime}.so "$out.tmp/lib/"
    ;;
windows)
    # cargo-xwin's environment: clang-cl, lld-link and the MSVC CRT and
    # Windows SDK it downloaded, plus its CMake toolchain file. sherpa-onnx
    # picks its onnxruntime by the Visual Studio platform, which Ninja leaves
    # unset.
    eval "$(cd "$repo" && cargo xwin env --target x86_64-pc-windows-msvc)"
    command -v llvm-lib >/dev/null || PATH="$PATH:/usr/lib/llvm-21/bin"
    # The toolchain drops the Windows libraries CMake links by default, which
    # espeak-ng needs (the registry); lld-link takes them from LINK.
    for lib in kernel32 user32 shell32 ole32 uuid advapi32; do
        LINK="${LINK:-} /defaultlib:$lib"
    done
    export LINK
    cmake -S "$work/src" -B "$work/src/build" -G Ninja \
        -D CMAKE_TOOLCHAIN_FILE="$CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_msvc" \
        -D CMAKE_VS_PLATFORM_NAME=x64 -D SHERPA_ONNX_USE_STATIC_CRT=ON "${options[@]}" >&2
    cmake --build "$work/src/build" >&2
    b="$work/src/build"
    cp "$b"/bin/sherpa-onnx-{c,cxx}-api.dll "$b"/lib/sherpa-onnx-{c,cxx}-api.lib \
        "$b"/_deps/onnxruntime-src/lib/{onnxruntime.dll,onnxruntime_providers_shared.dll,onnxruntime.lib} \
        "$out.tmp/lib/"
    ;;
esac
mv "$out.tmp" "$out"
rm -rf "$work"
echo "$out/lib"
