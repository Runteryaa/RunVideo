# Builds the Windows installer and the portable zip for RunVideo.
#
#   .\installer\build.ps1                     # version from Cargo.toml
#   .\installer\build.ps1 -Version 0.2.0
#   .\installer\build.ps1 -FfmpegDir "C:\ffmpeg\bin" -SkipBuild
#
# Both packages bundle FFmpeg (ffmpeg.exe + ffprobe.exe) next to the app, so the
# installed/portable copy does not need FFmpeg on PATH.

param(
    [string]$Version = "",
    [string]$FfmpegDir = "",
    [string]$Iscc = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    # --- Version ---------------------------------------------------------
    if (-not $Version) {
        $cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
        if ($cargo -match '(?m)^version\s*=\s*"([^"]+)"') {
            $Version = $Matches[1]
        } else {
            throw "Could not read the version from Cargo.toml; pass -Version."
        }
    }
    Write-Host "Building RunVideo $Version"

    # --- FFmpeg ----------------------------------------------------------
    if (-not $FfmpegDir) {
        $cmd = Get-Command ffmpeg -ErrorAction SilentlyContinue
        if (-not $cmd) {
            throw "ffmpeg was not found on PATH. Pass -FfmpegDir with the folder that holds ffmpeg.exe and ffprobe.exe."
        }
        $FfmpegDir = Split-Path -Parent $cmd.Source
    }
    $ffmpeg = Join-Path $FfmpegDir "ffmpeg.exe"
    $ffprobe = Join-Path $FfmpegDir "ffprobe.exe"
    if (-not (Test-Path $ffmpeg)) { throw "ffmpeg.exe not found in $FfmpegDir" }
    if (-not (Test-Path $ffprobe)) { throw "ffprobe.exe not found in $FfmpegDir" }

    $vendor = Join-Path $root "vendor\ffmpeg"
    $vendorBin = Join-Path $vendor "bin"
    New-Item -ItemType Directory -Force -Path $vendorBin | Out-Null
    Copy-Item $ffmpeg $vendorBin -Force
    Copy-Item $ffprobe $vendorBin -Force
    Write-Host "Bundled FFmpeg from $FfmpegDir"

    # Keep FFmpeg's own license next to the binaries when the build ships one.
    foreach ($name in @("LICENSE-FFmpeg.txt", "LICENSE.txt", "LICENSE")) {
        $candidate = Join-Path (Split-Path -Parent $FfmpegDir) $name
        if (Test-Path $candidate) {
            Copy-Item $candidate (Join-Path $vendor "LICENSE-FFmpeg.txt") -Force
            break
        }
    }

    # --- App -------------------------------------------------------------
    if (-not $SkipBuild) {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & cargo build --release --locked 2>&1 | ForEach-Object { Write-Host $_ }
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -ne 0) { throw "cargo build failed" }
    }
    $exe = Join-Path $root "target\release\runvideo.exe"
    if (-not (Test-Path $exe)) { throw "Missing $exe; run without -SkipBuild." }

    # --- Inno Setup ------------------------------------------------------
    if (-not $Iscc) {
        foreach ($candidate in @(
                (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
                "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
                "C:\Program Files\Inno Setup 6\ISCC.exe")) {
            if (Test-Path $candidate) { $Iscc = $candidate; break }
        }
    }
    if (-not $Iscc -or -not (Test-Path $Iscc)) {
        throw "ISCC.exe was not found. Install Inno Setup 6 or pass -Iscc."
    }

    $dist = Join-Path $root "dist"
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $Iscc "/DAppVersion=$Version" (Join-Path $PSScriptRoot "runvideo.iss") 2>&1 | ForEach-Object { Write-Host $_ }
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) { throw "Inno Setup compilation failed" }

    # --- Portable zip ----------------------------------------------------
    $portable = Join-Path $dist "portable"
    if (Test-Path $portable) { Remove-Item $portable -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Join-Path $portable "ffmpeg\bin") | Out-Null
    Copy-Item $exe $portable -Force
    Copy-Item (Join-Path $vendorBin "ffmpeg.exe") (Join-Path $portable "ffmpeg\bin") -Force
    Copy-Item (Join-Path $vendorBin "ffprobe.exe") (Join-Path $portable "ffmpeg\bin") -Force
    if (Test-Path (Join-Path $vendor "LICENSE-FFmpeg.txt")) {
        Copy-Item (Join-Path $vendor "LICENSE-FFmpeg.txt") (Join-Path $portable "ffmpeg") -Force
    }
    Copy-Item (Join-Path $root "README.md") $portable -Force
    Copy-Item (Join-Path $root "LICENSE") $portable -Force
    Copy-Item (Join-Path $root "THIRD-PARTY.md") $portable -Force
    $zip = Join-Path $dist "RunVideo-$Version-portable.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $portable "*") -DestinationPath $zip -Force

    Write-Host ""
    Get-ChildItem $dist -File | Where-Object { $_.Extension -in ".exe", ".zip" } |
        Select-Object Name, @{ Name = "SizeMB"; Expression = { [math]::Round($_.Length / 1MB, 1) } } |
        Format-Table -AutoSize
} finally {
    Pop-Location
}
