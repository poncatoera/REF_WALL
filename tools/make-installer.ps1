# Builds Wall-Setup.exe from a package made by tools/package-windows.ps1.
# The installer is per user (no admin rights): it copies Wall into
# %LOCALAPPDATA%\Programs\Wall, adds a Start menu shortcut and an entry in
# Settings > Apps to uninstall it. See tools/installer/install.ps1.
#
# Usage (PowerShell 5+):
#   powershell -File tools/make-installer.ps1 [-Package dist-next\Wall] [-Out dist-installer]
#
# It uses IExpress, which comes with Windows, so nothing extra is needed.
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

# IExpress takes a flat list of files from one folder.
$stage = Join-Path $Out "stage"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
$files = Get-ChildItem $Package -File
if (Get-ChildItem $Package -Directory) { throw "$Package has sub-folders; the installer copies files only" }
$files | Copy-Item -Destination $stage
Copy-Item (Join-Path $PSScriptRoot "installer\install.ps1"), (Join-Path $PSScriptRoot "installer\uninstall.ps1") $stage
Set-Content -Path (Join-Path $stage "files.txt") -Value ($files | ForEach-Object Name) -Encoding ascii
$readme = Get-Content (Join-Path $Package "README.txt") -TotalCount 1
$version = if ($readme -match "build (\w+)") { $Matches[1] } else { "dev" }
Set-Content -Path (Join-Path $stage "version.txt") -Value $version -Encoding ascii

$exe = Join-Path $Out "Wall-Setup.exe"
$all = Get-ChildItem $stage -File | ForEach-Object Name
$strings = for ($i = 0; $i -lt $all.Count; $i++) { "FILE$i=`"$($all[$i])`"" }
$list = for ($i = 0; $i -lt $all.Count; $i++) { "%FILE$i%=" }
$sed = @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgramWindow=0
HideExtractAnimation=1
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=%InstallPrompt%
DisplayLicense=%DisplayLicense%
FinishMessage=%FinishMessage%
TargetName=%TargetName%
FriendlyName=%FriendlyName%
AppLaunched=%AppLaunched%
PostInstallCmd=%PostInstallCmd%
AdminQuietInstCmd=%AdminQuietInstCmd%
UserQuietInstCmd=%UserQuietInstCmd%
SourceFiles=SourceFiles
[Strings]
InstallPrompt=
DisplayLicense=
FinishMessage=
TargetName=$exe
FriendlyName=Wall
AppLaunched=powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File install.ps1
PostInstallCmd=<None>
AdminQuietInstCmd=
UserQuietInstCmd=
$($strings -join "`r`n")
[SourceFiles]
SourceFiles0=$stage\
[SourceFiles0]
$($list -join "`r`n")
"@
$sedPath = Join-Path $Out "wall.sed"
Set-Content -Path $sedPath -Value $sed -Encoding ascii
if (Test-Path $exe) { Remove-Item -Force $exe }
Write-Host "building $exe"
# Piped so PowerShell waits for it (IExpress is a windowed program).
& "$env:WINDIR\System32\iexpress.exe" /N /Q $sedPath | Out-Null
if (-not (Test-Path $exe)) { throw "IExpress did not make $exe (exit $LASTEXITCODE)" }
"{0}  {1:N1} MB  (build {2})" -f $exe, ((Get-Item $exe).Length / 1MB), $version
