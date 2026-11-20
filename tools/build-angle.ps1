# Builds ANGLE (libEGL.dll, libGLESv2.dll) for the Windows desktop app, from
# source, at a pinned version, with the Visual Studio C++ build tools
# (2019 or newer; Google's own GN build needs VS 2022).
#
# Uses vcpkg at a fixed commit, whose "angle" port pins ANGLE chromium/7258
# by source hash. Everything goes under -Tools (default: ../tools next to
# the repo); nothing is installed system-wide.
#
# Usage:   powershell -File tools/build-angle.ps1 [-Tools C:\webgpu_claude\tools]
# Result:  <Tools>\vcpkg\installed\x64-windows-release\bin\{libEGL,libGLESv2,libz}.dll
#          -> pass that folder to tools/package-windows.ps1 -AngleDir
#
# Tested 20 Nov 2026 on Windows 10 with VS 2019 Build Tools 16.11 and
# Windows SDK 10.0.22621: 7 minutes, ANGLE 2.1.7258, D3D11 backend.

param(
    [string]$Tools = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) "tools")
)

$ErrorActionPreference = "Stop"
$VcpkgCommit = "74f5ea45ca78988784b810ad9bbb148ea7ff76d8" # vcpkg master, 2026-11-20
$vcpkg = Join-Path $Tools "vcpkg"

if (-not (Test-Path $vcpkg)) {
    git clone https://github.com/microsoft/vcpkg $vcpkg
    if ($LASTEXITCODE -ne 0) { throw "git clone failed" }
}
git -C $vcpkg fetch --depth 1 origin $VcpkgCommit 2>$null
git -C $vcpkg checkout --quiet $VcpkgCommit
if ($LASTEXITCODE -ne 0) { throw "could not check out vcpkg $VcpkgCommit" }
if (-not (Test-Path (Join-Path $vcpkg "vcpkg.exe"))) {
    & (Join-Path $vcpkg "bootstrap-vcpkg.bat") -disableMetrics
    if ($LASTEXITCODE -ne 0) { throw "vcpkg bootstrap failed" }
}

$env:VCPKG_DISABLE_METRICS = "1"
& (Join-Path $vcpkg "vcpkg.exe") install "angle:x64-windows-release" `
    "--x-buildtrees-root=$(Join-Path $Tools 'vcpkg-bt')" --clean-after-build
if ($LASTEXITCODE -ne 0) { throw "ANGLE build failed" }

$bin = Join-Path $vcpkg "installed\x64-windows-release\bin"
foreach ($f in "libEGL.dll", "libGLESv2.dll") {
    if (-not (Test-Path (Join-Path $bin $f))) { throw "missing $f in $bin" }
}
"ANGLE " + (Get-Item (Join-Path $bin "libGLESv2.dll")).VersionInfo.ProductVersion + " in $bin"
