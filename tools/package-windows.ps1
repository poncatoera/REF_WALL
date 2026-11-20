# Builds a portable Windows package of the Wall desktop app:
#   dist/Wall/            wall.exe + ANGLE + ffmpeg, runs without a dev setup
#   dist/Wall-windows-x64.zip
#
# Usage (PowerShell 5+):
#   pwsh tools/package-windows.ps1 -AngleDir <dir> [-FfmpegDir <dir>] [-NoZip]
#
#   -AngleDir   folder with libEGL.dll and libGLESv2.dll (d3dcompiler_47.dll
#               too if present). Without them Wall falls back to desktop
#               OpenGL 3.3.
#   -FfmpegDir  folder with ffmpeg.exe and ffprobe.exe, for video import and
#               playback. Without it, videos show as blank cards.
#
# Licences: ANGLE is BSD-licensed. Check the ffmpeg build's licence before
# sharing the package: GPL builds (e.g. gyan.dev "full") are fine for your
# own use, but a release needs an LGPL build (see the architecture doc).

param(
    [Parameter(Mandatory = $true)][string]$AngleDir,
    [string]$FfmpegDir,
    [string]$Out,
    [switch]$NoZip
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = Join-Path $root "dist" }
$pkg = Join-Path $Out "Wall"

Write-Host "building wall-desktop (release)"
Push-Location $root
try {
    & cargo build --release -p wall-desktop
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }

if (Test-Path $pkg) { Remove-Item -Recurse -Force $pkg }
New-Item -ItemType Directory -Force -Path $pkg | Out-Null
Copy-Item (Join-Path $root "target/release/wall.exe") $pkg

# ANGLE: required pair, optional shader compiler.
foreach ($f in "libEGL.dll", "libGLESv2.dll") {
    $p = Join-Path $AngleDir $f
    if (-not (Test-Path $p)) { throw "missing $f in $AngleDir" }
    Copy-Item $p $pkg
}
$d3d = Join-Path $AngleDir "d3dcompiler_47.dll"
if (Test-Path $d3d) { Copy-Item $d3d $pkg }

# ffmpeg: wall.exe looks next to itself first.
$videoNote = "Videos: not included. Install ffmpeg on PATH or set WALL_FFMPEG to play videos."
if ($FfmpegDir) {
    foreach ($f in "ffmpeg.exe", "ffprobe.exe") {
        $p = Join-Path $FfmpegDir $f
        if (-not (Test-Path $p)) { throw "missing $f in $FfmpegDir" }
        Copy-Item $p $pkg
    }
    $lic = Get-ChildItem (Split-Path -Parent $FfmpegDir), $FfmpegDir -Filter "LICENSE*" -File -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($lic) { Copy-Item $lic.FullName (Join-Path $pkg "LICENSE.ffmpeg.txt") }
    $version = (& (Join-Path $FfmpegDir "ffmpeg.exe") -hide_banner -version 2>$null | Select-Object -First 1)
    $videoNote = "Videos: bundled $version"
}

$commit = (& git -C $root rev-parse --short HEAD).Trim()
$readme = @"
Wall for Windows (build $commit)

Run wall.exe. It opens (or creates) Wall\Library.wall in your user folder;
boards save automatically. To open another library: wall.exe D:\Refs\Moodboard.wall

Graphics: ANGLE (OpenGL ES 3.0 on Direct3D 11), the same shaders as the web
version. Set WALL_GL=gl to use desktop OpenGL instead.
$videoNote
GPU video decoding (less CPU, more memory with many videos playing): add
"gpu_video_decode": true to %APPDATA%\Wall\settings.json, or set WALL_GPU_DECODE=1.

Keys: L library and trash, P side panel, T to-do list, B new board,
Ctrl+F search, Alt-drag arrow, Ctrl-drag move without snapping, I / O video
in and out points, K play or pause, Esc back up. The menu bar lists the rest.
"@
Set-Content -Path (Join-Path $pkg "README.txt") -Value $readme -Encoding ascii

if (-not $NoZip) {
    $zip = Join-Path $Out "Wall-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path $pkg -DestinationPath $zip
    Write-Host "packaged $zip"
}
Get-ChildItem $pkg | ForEach-Object { "{0,-22} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
