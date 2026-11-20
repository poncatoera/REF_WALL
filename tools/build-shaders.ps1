# Compiles shaders/*.slang into shaders/generated/*.glsl (GLSL ES 3.00).
#   slangc  -> SPIR-V          (https://github.com/shader-slang/slang/releases)
#   naga    -> GLSL ES 3.00    (cargo install naga-cli)
# Usage: pwsh tools/build-shaders.ps1   (slangc and naga must be on PATH)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root "shaders/generated"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "wall-shaders"
New-Item -ItemType Directory -Force -Path $out, $tmp | Out-Null

# name, entry point, slang stage, naga stage, output suffix
$entries = @(
    @("sdf_box", "vs_main", "vertex",   "vert", "vert"),
    @("sdf_box", "fs_main", "fragment", "frag", "frag"),
    @("glyph",   "vs_main", "vertex",   "vert", "vert"),
    @("glyph",   "fs_main", "fragment", "frag", "frag")
)

foreach ($e in $entries) {
    $name, $entry, $stage, $nagaStage, $suffix = $e
    $src = Join-Path $root "shaders/$name.slang"
    $spv = Join-Path $tmp "$name.$suffix.spv"
    $glsl = Join-Path $out "$name.$suffix.glsl"
    & slangc $src -target spirv -entry $entry -stage $stage -o $spv
    if ($LASTEXITCODE -ne 0) { throw "slangc failed for $name/$entry" }
    & naga $spv $glsl --profile es300 --shader-stage $nagaStage --entry-point $entry
    if ($LASTEXITCODE -ne 0) { throw "naga failed for $name/$entry" }
    Write-Host "built $glsl"
}
