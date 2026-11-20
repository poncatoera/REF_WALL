# Removes Wall installed by install.ps1: the app folder, the Start menu
# shortcut and the uninstall entry. Libraries (your boards, pictures and
# videos) and settings stay where they are.

param([switch]$Quiet)

Add-Type -AssemblyName System.Windows.Forms
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
# Only ever delete an installed Wall folder (not, say, this script's source).
if (-not ((Test-Path (Join-Path $dir "wall.exe")) -and (Test-Path (Join-Path $dir "README.txt")))) {
    Write-Host "Not an installed Wall folder: $dir"
    exit 1
}
if (-not $Quiet) {
    $r = [System.Windows.Forms.MessageBox]::Show("Remove Wall from this computer?`n`nYour boards, pictures and videos stay in your Wall library.", "Wall", "OKCancel", "Question")
    if ($r -ne "OK") { exit 1 }
}
Get-Process wall -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object {
    if (-not $Quiet) { [void][System.Windows.Forms.MessageBox]::Show("Wall is open; close it first.", "Wall") }
    exit 1
}
Remove-Item (Join-Path ([Environment]::GetFolderPath("Programs")) "Wall.lnk") -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Wall" -Recurse -ErrorAction SilentlyContinue
# This script is in the folder: delete it from a short-lived process.
Start-Process cmd.exe -ArgumentList "/c timeout /t 2 /nobreak >nul & rmdir /s /q `"$dir`"" -WindowStyle Hidden
