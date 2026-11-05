<#
.SYNOPSIS
  Downloads a Windows FFmpeg build (ffmpeg.exe + ffprobe.exe) into
  src-tauri/binaries so game-capture can run and be bundled.

.DESCRIPTION
  Uses the GPL builds from https://github.com/BtbN/FFmpeg-Builds, which
  include ddagrab, scale_d3d11, NVENC, AMF, QuickSync (libvpl), x264, x265
  and SVT-AV1. FFmpeg 8.x is required for the GPU-side scale_d3d11 filter;
  older builds still work but downscaling then happens on the CPU.

.PARAMETER Version
  FFmpeg release branch to fetch (default 8.1). Falls back to the latest
  master build if that branch is not published.
#>
param(
    [string]$Version = "8.1",
    [string]$Destination = (Join-Path $PSScriptRoot "..\src-tauri\binaries")
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$base = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest"
$candidates = @(
    "ffmpeg-n$Version-latest-win64-gpl-$Version.zip",
    "ffmpeg-master-latest-win64-gpl.zip"
)

New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("gc-ffmpeg-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp "ffmpeg.zip"
    $ok = $false
    foreach ($name in $candidates) {
        try {
            Write-Host "Downloading $name"
            Invoke-WebRequest -Uri "$base/$name" -OutFile $zip -UseBasicParsing
            $ok = $true
            break
        } catch {
            Write-Warning "  not available: $($_.Exception.Message)"
        }
    }
    if (-not $ok) { throw "No FFmpeg build could be downloaded." }

    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    foreach ($exe in @("ffmpeg.exe", "ffprobe.exe")) {
        $found = Get-ChildItem -Path $tmp -Recurse -Filter $exe | Select-Object -First 1
        if (-not $found) { throw "$exe missing from archive" }
        Copy-Item $found.FullName (Join-Path $Destination $exe) -Force
    }
    $license = Get-ChildItem -Path $tmp -Recurse -Filter "LICENSE.txt" | Select-Object -First 1
    if (-not $license) { throw "LICENSE.txt missing from archive" }
    Copy-Item $license.FullName (Join-Path $Destination "FFMPEG-LICENSE.txt") -Force
    & (Join-Path $Destination "ffmpeg.exe") -hide_banner -version | Select-Object -First 1
    Write-Host "FFmpeg installed to $Destination"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
