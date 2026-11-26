# Copies the runtime DLLs in -From next to every executable one cargo build
# made, as its own messages name them:
#
#   cargo test ... --no-run --message-format=json-render-diagnostics > messages.jsonl
#   pwsh scripts/dlls-beside-executables.ps1 messages.jsonl C:\t\release
#
# Windows looks for a DLL beside the exe before System32, and System32 holds
# Windows ML's own, older onnxruntime.dll: a test binary without ours beside
# it loads that one and crashes (0xc0000005). Rust 1.100 moved test binaries
# from target\<profile>\deps to build\<pkg>\<hash>\out (CI, 2026-11-26), so
# their folders come from cargo ("executable"), not from the layout.
param(
    [Parameter(Mandatory)][string]$Messages,
    [Parameter(Mandatory)][string]$From
)
$ErrorActionPreference = "Stop"
$dlls = @(Get-ChildItem "$From\*.dll")
if ($dlls.Count -eq 0) { throw "no DLLs in $From" }
$dirs = @(Get-Content $Messages |
    Where-Object { $_.StartsWith("{") } |
    ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq "compiler-artifact" -and $_.executable } |
    ForEach-Object { Split-Path -Parent $_.executable } |
    Sort-Object -Unique)
if ($dirs.Count -eq 0) { throw "no executables in $Messages" }
$source = [IO.Path]::GetFullPath($From).TrimEnd('\', '/')
foreach ($dir in $dirs) {
    # bd.exe and the other bins are in -From already.
    if ([IO.Path]::GetFullPath($dir).TrimEnd('\', '/') -ne $source) { Copy-Item $dlls.FullName $dir }
}
"$($dlls.Count) DLLs beside the executables in $($dirs.Count) folders"
