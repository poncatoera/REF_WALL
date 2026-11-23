# Builds the web version into server/public, which the sync server serves.
# Nothing is uploaded. Usage: powershell -File tools/build-server.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    & cargo build --release --target wasm32-unknown-unknown -p wall-web
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    & wasm-bindgen --target web --out-dir apps/web/www/pkg target/wasm32-unknown-unknown/release/wall_web.wasm
    if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed" }
} finally { Pop-Location }
$out = Join-Path $root "server\public"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
Copy-Item -Recurse (Join-Path $root "apps\web\www") $out
Get-ChildItem $out -Recurse -Filter "*.d.ts" | Remove-Item -Force
# The page may load only its own files; nobody may put it in a frame; and a
# link's address isn't told to other sites. Inline scripts are this page's two.
Set-Content -Path (Join-Path $out "_headers") -Encoding ascii -Value @"
/*
  Content-Security-Policy: default-src 'none'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' blob: data:; media-src blob:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
  Referrer-Policy: no-referrer
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
"@
Get-ChildItem $out -Recurse -File | ForEach-Object { "{0,-60} {1,8:N0} KB" -f $_.FullName.Substring($out.Length + 1), ($_.Length / 1KB) }
