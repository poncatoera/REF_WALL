#!/usr/bin/env bash
# Builds a macOS app bundle of the Wall desktop app:
#   dist/Wall.app            universal (Apple silicon + Intel)
#   dist/Wall-macos.zip
#
# Usage (on a Mac with Xcode command line tools and rustup):
#   tools/package-mac.sh [--ffmpeg-dir <dir>] [--sign "<Developer ID Application: …>"] [--no-zip]
#
#   --ffmpeg-dir  folder with ffmpeg and ffprobe, for video import and
#                 playback. Without it Wall uses an ffmpeg on PATH, else
#                 videos show as blank cards.
#   --sign        codesign identity. Without it the app is signed ad hoc,
#                 which runs on this Mac; other Macs need a Developer ID
#                 signature and notarisation (Apple developer account).
#
# Graphics: Apple's OpenGL 3.3 (runs on Metal). ANGLE is not used on macOS
# yet. Licences: a release needs an LGPL ffmpeg build, as on Windows.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/dist"
ffmpeg_dir=""
sign="-"
zip=1
while [ $# -gt 0 ]; do
  case "$1" in
    --ffmpeg-dir) ffmpeg_dir="$2"; shift 2 ;;
    --sign) sign="$2"; shift 2 ;;
    --no-zip) zip=0; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

version="$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
commit="$(git -C "$root" rev-parse --short HEAD)"

echo "building wall-desktop (release, arm64 + x86_64)"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  (cd "$root" && cargo build --release -p wall-desktop --target "$t")
done

app="$out/Wall.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/wall" \
  "$root/target/aarch64-apple-darwin/release/wall" \
  "$root/target/x86_64-apple-darwin/release/wall"

# ffmpeg: wall looks next to its own executable first.
video_note="Videos: not included. Install ffmpeg (brew install ffmpeg) or set WALL_FFMPEG."
if [ -n "$ffmpeg_dir" ]; then
  for f in ffmpeg ffprobe; do
    [ -x "$ffmpeg_dir/$f" ] || { echo "missing $f in $ffmpeg_dir" >&2; exit 1; }
    cp "$ffmpeg_dir/$f" "$app/Contents/MacOS/"
  done
  video_note="Videos: bundled $("$ffmpeg_dir/ffmpeg" -hide_banner -version | head -1)"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Wall</string>
  <key>CFBundleDisplayName</key><string>Wall</string>
  <key>CFBundleIdentifier</key><string>studio.wall.desktop</string>
  <key>CFBundleExecutable</key><string>wall</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$commit</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
</dict>
</plist>
PLIST

cat > "$out/README-mac.txt" <<TXT
Wall for macOS (build $commit)

Open Wall.app. It opens (or creates) Wall/Library.wall in your home folder;
boards save automatically. To open another library from Terminal:
  Wall.app/Contents/MacOS/wall ~/Refs/Moodboard.wall

Graphics: OpenGL 3.3 on Metal.
$video_note
TXT

codesign --force --deep --sign "$sign" "$app"

if [ "$zip" = 1 ]; then
  rm -f "$out/Wall-macos.zip"
  ditto -c -k --keepParent "$app" "$out/Wall-macos.zip"
  echo "packaged $out/Wall-macos.zip"
fi
du -sh "$app"
