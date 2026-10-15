# Builds beardictate for Windows and packages it: a portable zip and, when
# Inno Setup is installed, the per-user installer (DESIGN.md "Windows install
# and first run"). Run from anywhere on a Windows machine with Rust (MSVC),
# CMake, Ninja and LLVM (for llama.cpp):
#
#   pwsh scripts/package-windows.ps1 [-Features vulkan] [-NoBuild] [-Stage <dir>]
#
# Output in dist\. -Stage packages binaries built elsewhere (a cargo-xwin
# cross build from Linux): a folder holding the exes and sherpa-onnx DLLs.
# Code signing: set BD_SIGN_CMD to a signtool command line ending in the file
# to sign, written as $f (e.g. 'signtool sign /fd sha256 /tr http://... /td sha256 /a $f').
param(
    [string]$Features = "",
    [switch]$NoBuild,
    [string]$Stage = ""
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$version = (Select-String -Path "$repo\Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

if (-not $Stage) {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$repo\target" }
    if (-not $NoBuild) {
        $cargoArgs = @("build", "--release", "--locked", "-p", "bd-app", "-p", "bd-cli")
        if ($Features) { $cargoArgs += @("--features", $Features) }
        Push-Location $repo
        try { & cargo @cargoArgs; if ($LASTEXITCODE) { throw "cargo build failed" } } finally { Pop-Location }
    }
    $Stage = "$target\release"
}

$out = "$repo\packaging\windows\stage"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
foreach ($f in "beardictate.exe", "bd.exe", "sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll") {
    Copy-Item "$Stage\$f" $out
}
Copy-Item "$repo\LICENSE" "$out\LICENSE.txt"
Copy-Item "$repo\packaging\THIRD-PARTY.txt" $out

if ($env:BD_SIGN_CMD) {
    foreach ($f in Get-ChildItem "$out\*.exe", "$out\*.dll") {
        $cmd = $env:BD_SIGN_CMD.Replace('$f', "`"$($f.FullName)`"")
        cmd /c $cmd; if ($LASTEXITCODE) { throw "signing $f failed" }
    }
}

New-Item -ItemType Directory -Force "$repo\dist" | Out-Null
$zip = "$repo\dist\beardictate-$version-windows-x64.zip"
Compress-Archive -Force -Path "$out\*" -DestinationPath $zip
Write-Host "portable: $zip"

$iscc = Get-Command iscc -ErrorAction SilentlyContinue
if (-not $iscc) {
    foreach ($p in "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") {
        if (Test-Path $p) { $iscc = $p; break }
    }
}
if (-not $iscc) {
    Write-Warning "Inno Setup 6 not found (winget install JRSoftware.InnoSetup); skipping the installer"
    exit 0
}
$isccArgs = @("/Q", "/DAppVersion=$version", "/DStage=$out")
if ($env:BD_SIGN_CMD) { $isccArgs += @("/DSign", "/Sbdsign=$($env:BD_SIGN_CMD)") }
& $iscc @isccArgs "$repo\packaging\windows\beardictate.iss"
if ($LASTEXITCODE) { throw "iscc failed" }
Write-Host "installer: $repo\dist\beardictate-$version-windows-x64-setup.exe"
