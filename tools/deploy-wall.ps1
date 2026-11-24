# Deploys server/ (built first with tools/build-server.ps1) to the Worker,
# keeping the live OWNER_HASH (wrangler.toml's is empty on purpose: a plain
# `wrangler deploy` would turn the owner off).
#
# Usage, from anywhere:
#   powershell -File tools\deploy-wall.ps1 -Account <Cloudflare account id> [-Expected <live version id>]
#
# -Account: the account the Worker is in (or set CLOUDFLARE_ACCOUNT_ID).
# -Expected: stop unless this is the version live now, so a deploy made
#   from somewhere else in between isn't overwritten without a look.
# It prints the version that was live before, to roll back to with
#   npx wrangler rollback <that id> --name wall
param(
    [string]$Account = $env:CLOUDFLARE_ACCOUNT_ID,
    [string]$Expected = ""
)
$ErrorActionPreference = "Stop"
if (-not $Account) { throw "give -Account <Cloudflare account id> (or set CLOUDFLARE_ACCOUNT_ID); nothing deployed" }
$env:CLOUDFLARE_ACCOUNT_ID = $Account
$env:WRANGLER_SEND_METRICS = "false"
$scoopNode = "$env:USERPROFILE\scoop\apps\nodejs-lts\current"
if (Test-Path $scoopNode) { $env:Path = "$scoopNode;$env:Path" }
$server = Join-Path (Split-Path -Parent $PSScriptRoot) "server"
$w = Join-Path $server "node_modules\.bin\wrangler.cmd"
if (-not (Test-Path $w)) { throw "wrangler isn't installed: run npm ci in $server first; nothing deployed" }
Set-Location $server
foreach ($f in "index.html", "pkg\wall_web_bg.wasm") {
    if (-not (Test-Path "public\$f")) { throw "server\public is not built ($f is missing): run tools\build-server.ps1 first; nothing deployed" }
}
$deployments = & $w deployments list --name wall --json | Out-String | ConvertFrom-Json
$live = ($deployments | Sort-Object { [datetime]$_.created_on } | Select-Object -Last 1).versions[0].version_id
if ($Expected -and $live -ne $Expected) { throw "live is $live, not $Expected; someone else deployed since. Nothing deployed" }
$version = & $w versions view $live --name wall --json | Out-String | ConvertFrom-Json
$hash = ($version.resources.bindings | Where-Object name -eq "OWNER_HASH").text
if ($hash -notmatch "^[0-9a-f]{64}$") { throw "could not read the live OWNER_HASH; nothing deployed" }
Write-Host "live version before (roll back to this): $live"
& $w deploy --var "OWNER_HASH:$hash" | ForEach-Object { $_ -replace "[0-9a-f]{64}", "<hash>" }
if ($LASTEXITCODE -ne 0) { throw "deploy failed" }
