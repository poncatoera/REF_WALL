# Installs Wall for the current user: no admin rights needed.
#   %LOCALAPPDATA%\Programs\Wall        the app
#   Start menu > Wall                   shortcut
#   Settings > Apps > Wall              uninstall entry (HKCU)
# Runs from the folder the installer unpacked into. Libraries and settings
# are never touched (they live in the user's Wall folder and %APPDATA%\Wall).
#
# For tests: -Dir <folder> installs there; -NoShortcut / -NoRegister skip
# the Start menu and the uninstall entry; -Quiet shows no message boxes.

param([string]$Dir, [switch]$NoShortcut, [switch]$NoRegister, [switch]$Quiet)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
function Say([string]$text, [string]$icon = "Information") {
    if ($Quiet) { Write-Host $text } else { [void][System.Windows.Forms.MessageBox]::Show($text, "Wall", "OK", $icon) }
}

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA "Programs\Wall" }
$exe = Join-Path $Dir "wall.exe"

try {
    # A running Wall locks its files; ask instead of half-updating.
    while (Get-Process wall -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) {
        if ($Quiet) { throw "Wall is running from $Dir; close it first." }
        $r = [System.Windows.Forms.MessageBox]::Show("Wall is open. Close it, then press OK to continue.", "Wall", "OKCancel", "Warning")
        if ($r -ne "OK") { exit 1 }
    }
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    # Files from an older version that this one no longer ships go away;
    # everything here came from an installer.
    $files = Get-Content (Join-Path $here "files.txt")
    Get-ChildItem $Dir -File | Where-Object { $files -notcontains $_.Name -and $_.Name -ne "uninstall.ps1" } | Remove-Item -Force
    foreach ($f in $files) { Copy-Item (Join-Path $here $f) $Dir -Force }
    Copy-Item (Join-Path $here "uninstall.ps1") $Dir -Force

    $version = (Get-Content (Join-Path $here "version.txt") -Raw).Trim()
    if (-not $NoShortcut) {
        $lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Wall.lnk"
        $sh = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
        $sh.TargetPath = $exe
        $sh.WorkingDirectory = $Dir
        $sh.Description = "Wall: boards of pictures, videos and notes"
        $sh.Save()
    }
    if (-not $NoRegister) {
        $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Wall"
        New-Item -Path $key -Force | Out-Null
        $size = [int]((Get-ChildItem $Dir -File | Measure-Object Length -Sum).Sum / 1KB)
        $values = @{
            DisplayName = "Wall"; DisplayVersion = $version; Publisher = "Wall"
            InstallLocation = $Dir; DisplayIcon = $exe; EstimatedSize = $size; NoModify = 1; NoRepair = 1
            UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Dir\uninstall.ps1`""
        }
        foreach ($k in $values.Keys) {
            $type = if ($values[$k] -is [int]) { "DWord" } else { "String" }
            New-ItemProperty -Path $key -Name $k -Value $values[$k] -PropertyType $type -Force | Out-Null
        }
    }
    Say "Wall $version is installed. Open it from the Start menu."
    if (-not $Quiet) { Start-Process $exe -WorkingDirectory $Dir }
} catch {
    Say "Could not install Wall: $($_.Exception.Message)" "Error"
    exit 1
}
