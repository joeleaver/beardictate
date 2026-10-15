# Builds beardictate for Windows and packages it: a portable zip and, when
# Inno Setup is installed, the per-user installer (DESIGN.md "Windows install
# and first run"). Run from anywhere on a Windows machine with Rust (MSVC),
# CMake and the Vulkan SDK (VULKAN_SDK set):
#
#   pwsh scripts/package-windows.ps1 [-NoBuild] [-Stage <dir>]
#
# Output in dist\. It builds the release configuration, as
# scripts/cross-build-windows.sh does on Linux (what releases use): feature
# vulkan-dl, the DLL C runtime, and a stage folder holding the exes and every
# DLL to ship (llama.cpp's libraries and backends, the VC++ runtime pinned in
# scripts/windows-vc-runtime.sh). -NoBuild packages an existing build of that
# configuration (CI builds it in earlier steps); -Stage <dir> packages a stage
# folder made elsewhere (the cross build).
# Code signing: set BD_SIGN_CMD to a signtool command line ending in the file
# to sign, written as $f (e.g. 'signtool sign /fd sha256 /tr http://... /td sha256 /a $f').
param(
    [switch]$NoBuild,
    [string]$Stage = ""
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$version = (Select-String -Path "$repo\Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# The VC++ runtime DLLs, from the package pinned in windows-vc-runtime.sh
# (the one source of its URL and checksum), into $dest.
function Get-VcRuntime([string]$dest) {
    $pin = Get-Content "$repo\scripts\windows-vc-runtime.sh" -Raw
    $field = { param($name) [regex]::Match($pin, "(?m)^$name=(\S+)").Groups[1].Value }
    $url = & $field "url"; $sha256 = & $field "sha256"; $ver = & $field "version"
    $vsix = Join-Path ([IO.Path]::GetTempPath()) "vc-runtime-$ver.zip"
    if (-not (Test-Path $vsix) -or (Get-FileHash $vsix -Algorithm SHA256).Hash -ne $sha256) {
        Invoke-WebRequest -UseBasicParsing -OutFile $vsix $url
        if ((Get-FileHash $vsix -Algorithm SHA256).Hash -ne $sha256) { throw "VC++ runtime package: checksum mismatch" }
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($vsix)
    try {
        foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll") {
            $entry = $zip.Entries | Where-Object { $_.FullName -like "Contents/VC/Redist/MSVC/*/x64/Microsoft.VC143.CRT/$dll" } | Select-Object -First 1
            if (-not $entry) { throw "$dll missing from the VC++ runtime package" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, "$dest\$dll", $true)
        }
    } finally { $zip.Dispose() }
    Write-Host "VC++ runtime $ver"
}

$out = "$repo\packaging\windows\stage"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
if ($Stage) {
    Copy-Item "$Stage\beardictate.exe", "$Stage\bd.exe", "$Stage\*.dll" $out
} else {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$repo\target" }
    if (-not $NoBuild) {
        # llama.cpp's DLLs share C runtime objects, so everything uses the DLL
        # runtime (.cargo/config.toml's static one is for other builds).
        $env:LLAMA_STATIC_CRT = "0"
        $env:RUSTFLAGS = "-C target-feature=-crt-static"
        $cargoArgs = @("build", "--release", "--locked", "-p", "bd-app", "-p", "bd-cli", "--features", "bd-app/vulkan-dl,bd-cli/vulkan-dl")
        Push-Location $repo
        try { & cargo @cargoArgs; if ($LASTEXITCODE) { throw "cargo build failed" } } finally { Pop-Location }
    }
    $release = "$target\release"
    # llama-cpp-sys copies its DLLs next to the exes only when none is there
    # yet, so take them from its newest build folder.
    $llama = Get-ChildItem -Directory "$release\build\llama-cpp-sys-2-*\out" | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not (Test-Path "$($llama.FullName)\backends\ggml-vulkan.dll")) { throw "no vulkan-dl build of llama.cpp in $release" }
    foreach ($f in "beardictate.exe", "bd.exe", "sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll") {
        Copy-Item "$release\$f" $out
    }
    foreach ($f in "llama.dll", "ggml.dll", "ggml-base.dll") { Copy-Item "$($llama.FullName)\bin\$f" $out }
    Copy-Item "$($llama.FullName)\backends\*.dll" $out
    Get-VcRuntime $out
}
& "$out\bd.exe" --version; if ($LASTEXITCODE) { throw "the staged bd.exe doesn't start" }
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
