#!/usr/bin/env bash
# Builds a stand-in Windows Vulkan SDK from this Linux machine's packages, so
# `cargo xwin` can cross-build llama.cpp's Vulkan backend (DESIGN.md
# "Windows install and first run"). What ggml-vulkan's CMake looks for under
# VULKAN_SDK on a Windows target:
#   include/   the Vulkan and SPIR-V headers (platform-independent)
#   lib/       vulkan-1.lib, the import library for the loader DLL
#   bin/       glslc, run on the build host to compile the shaders
#   share/cmake/SPIRV-Headers/   the SPIR-V headers' CMake package
# CMake's FindVulkan spells the folders in lower case, llama-cpp-sys's build
# script as the real SDK does (Include, Lib, Bin); both are there.
# Needs libvulkan-dev, spirv-headers, glslc and llvm-dlltool (llvm).
#
#   scripts/windows-vulkan-sdk.sh <dir>     then: export VULKAN_SDK=<dir>
set -euo pipefail
sdk="${1:?usage: windows-vulkan-sdk.sh <dir>}"
dlltool="$(command -v llvm-dlltool || echo /usr/lib/llvm-21/bin/llvm-dlltool)"
header=/usr/include/vulkan/vulkan_core.h
for need in "$header" /usr/include/spirv /usr/share/cmake/SPIRV-Headers "$(command -v glslc)" "$dlltool"; do
    test -e "$need" || { echo "missing $need" >&2; exit 1; }
done

rm -rf "$sdk"
mkdir -p "$sdk/Include" "$sdk/Lib" "$sdk/Bin" "$sdk/share/cmake"
for d in vulkan vk_video spirv; do ln -s "/usr/include/$d" "$sdk/Include/$d"; done
ln -s "$(command -v glslc)" "$sdk/Bin/glslc"
# The package config finds the headers relative to itself, in ../../include.
cp -r /usr/share/cmake/SPIRV-Headers "$sdk/share/cmake/"
for d in Include Lib Bin; do ln -s "$d" "$sdk/${d,,}"; done

# The loader exports the core commands (Vulkan 1.0 to 1.4), which sit in the
# header before the first extension. Only the ones a DLL calls end up in its
# import table; ggml-vulkan resolves the rest through vkGetInstanceProcAddr.
{
    echo "LIBRARY vulkan-1.dll"
    echo "EXPORTS"
    sed -n '/^#define VK_VERSION_1_0 1/,/^#define VK_KHR_surface 1/p' "$header" |
        sed -n 's/^VKAPI_ATTR .* VKAPI_CALL \(vk[A-Za-z0-9]*\)(.*/    \1/p'
} >"$sdk/Lib/vulkan-1.def"
"$dlltool" -m i386:x86-64 -d "$sdk/Lib/vulkan-1.def" -l "$sdk/Lib/vulkan-1.lib"
echo "VULKAN_SDK=$sdk ($(grep -c '^    vk' "$sdk/Lib/vulkan-1.def") loader exports)"
