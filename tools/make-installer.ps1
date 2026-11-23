# Builds Wall-Setup.exe from a package made by tools/package-windows.ps1,
# with Inno Setup 6. What the installer does is in tools/installer/wall.iss:
# per user (no admin rights), %LOCALAPPDATA%\Programs\Wall, Start menu
# shortcut, optional desktop shortcut, "Open in Wall" on .wall folders,
# updates in place, uninstall from Settings > Apps. Libraries and settings
# are never in the app folder, so none of that touches them.
#
# Usage (PowerShell 5+):
#   powershell -File tools/make-installer.ps1 [-Package dist-next\Wall] [-Out dist-installer]
#
# Needs Inno Setup 6 once:  winget install --id JRSoftware.InnoSetup -e
# The .exe is not code-signed: Windows SmartScreen asks "Run anyway" the
# first time. Signing needs a certificate and comes with a release.

param(
    [string]$Package,
    [string]$Out
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Package) { $Package = Join-Path $root "dist-next\Wall" }
if (-not $Out) { $Out = Join-Path $root "dist-installer" }
$Package = (Resolve-Path $Package).Path
if (-not (Test-Path (Join-Path $Package "wall.exe"))) { throw "no wall.exe in $Package; run tools/package-windows.ps1 first" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path

$iscc = @(
    (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
    (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
    (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe")
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { $iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source }
if (-not $iscc) { throw "Inno Setup 6 not found. Install it once: winget install --id JRSoftware.InnoSetup -e" }

# Version: the workspace version, plus the commit the package was built from.
$cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
$numeric = if ($cargo -match '(?ms)^\[workspace\.package\].*?^version\s*=\s*"(\d+\.\d+\.\d+)"') { $Matches[1] } else { "0.0.0" }
$readme = Get-Content (Join-Path $Package "README.txt") -TotalCount 1 -ErrorAction SilentlyContinue
$build = if ($readme -match "build (\w+)") { $Matches[1] } else { "dev" }
$version = "$numeric ($build)"

$exe = Join-Path $Out "Wall-Setup.exe"
if (Test-Path $exe) { Remove-Item -Force $exe }
Write-Host "building $exe with $iscc"
& $iscc /Q "/DPackage=$Package" "/DAppVersion=$version" "/DNumericVersion=$numeric.0" "/DOutDir=$Out" (Join-Path $PSScriptRoot "installer\wall.iss")
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $exe)) { throw "Inno Setup failed (exit $LASTEXITCODE)" }
"{0}  {1:N1} MB  (Wall {2})" -f $exe, ((Get-Item $exe).Length / 1MB), $version
