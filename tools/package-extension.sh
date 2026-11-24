#!/bin/sh
# Zips the browser extension for the Edge Add-ons / Chrome Web Store upload:
# manifest.json at the top, without the notes. Usage: tools/package-extension.sh [out.zip]
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
ver=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$root/extension/manifest.json" | head -1)
out="${1:-$root/dist/wall-extension-$ver.zip}"
mkdir -p "$(dirname "$out")"
rm -f "$out"
cd "$root/extension"
zip -q -r -X "$out" . -x README.md -x STORE.md -x '.*'
echo "$out"
