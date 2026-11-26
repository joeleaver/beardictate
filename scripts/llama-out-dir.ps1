# Prints the build folder of llama-cpp-sys-2 (the "out" folder holding
# llama.cpp's bin\*.dll and backends\*.dll) for one cargo build, from that
# build's own messages:
#
#   cargo build ... --message-format=json-render-diagnostics > messages.jsonl
#   pwsh scripts/llama-out-dir.ps1 messages.jsonl
#
# Cargo says where each build script wrote ("build-script-executed",
# "out_dir"), fresh or not, so this doesn't depend on how Cargo lays out its
# build folder: Rust 1.100 moved build\<pkg>-<hash>\out to
# build\<pkg>\<hash>\out, which a glob for the old layout missed (CI,
# 2026-11-26). It fails when the build had no vulkan-dl llama.cpp.
param([Parameter(Mandatory)][string]$Messages)
$ErrorActionPreference = "Stop"
$dirs = @(Get-Content $Messages |
    Where-Object { $_.StartsWith("{") } |
    ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq "build-script-executed" -and $_.package_id -match "#llama-cpp-sys-2@" } |
    ForEach-Object { $_.out_dir } |
    Sort-Object -Unique)
if ($dirs.Count -ne 1) { throw "expected one llama-cpp-sys-2 build in $Messages, found $($dirs.Count): $($dirs -join ', ')" }
$out = $dirs[0]
if (-not (Test-Path "$out\backends\ggml-vulkan.dll")) { throw "llama.cpp in $out was built without the Vulkan backend (feature vulkan-dl)" }
$out
