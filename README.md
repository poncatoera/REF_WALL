# Wall

Reference, moodboard and motion-study tool. One Rust codebase, built as a
desktop app (macOS, Windows) and a web app (WASM + WebGL2 on Cloudflare).

Architecture proposal: https://claude.ai/artifact/SCu78YrUEzKsEE8CEoSZ49

## Layout

| Path | What | Status |
| --- | --- | --- |
| `crates/wall-format` | The open `.wall` library format: manifest, media by BLAKE3 hash, sidecars, boards, per-item merge for sync | Working, 7 tests |
| `crates/wall-media` | Import rules: images max 2048 px wide, videos max 20 MB / 1080p, H.264 MP4 with a keyframe every second | Working, 13 tests |
| `crates/wall-text` | Slug-method text: outlines to quadratic curves, band packing, layout, CPU twin of the shader | Working, 5 tests |
| `crates/wall-render` | GLES3 / WebGL2 renderer: SDF boxes and glyphs | Working on Linux (Mesa) and headless Chrome |
| `crates/wall-app` | Camera, pan and zoom, demo board | Working, 4 tests |
| `apps/desktop` | winit + glutin, desktop GL 3.3 for now (ANGLE next) | Runs on Linux; Windows/Mac not yet confirmed |
| `apps/web` | wasm-bindgen + WebGL2 | Runs in Chromium |
| `shaders/` | Slang sources; `generated/` holds GLSL ES 3.00 | Generated files are hand-ported until `tools/build-shaders.ps1` runs |
| `worker` | Cloudflare Worker | Not started |

Both crates also build for `wasm32-unknown-unknown`, so the web app uses the
same rules and format code.

Fonts: `assets/fonts/Inter-Regular.otf` (SIL Open Font License, see `Inter-OFL.txt`).

## Commands

```sh
cargo test                                   # all tests
cargo run -p wall-desktop                    # desktop app
# web: build, bind, then serve apps/web/www
cargo build -p wall-web --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir apps/web/www/pkg target/wasm32-unknown-unknown/release/wall_web.wasm
```

Backups: File > Back up to folder… keeps a plain copy of the library in
another folder, updated every 30 minutes while Wall is open and when it
closes (only what changed is copied). File > Export library as .zip… writes
it all as one file. Same from the command line: `wall-cli backup`,
`wall-cli export-library`, `wall-cli restore`. On the web, Ctrl+S downloads
a .zip of the boards kept in that browser; dropping it back restores them.

Big-board checks: `wall-cli stress <library> --pictures 1500 --videos 300`
makes a test board; `WALL_BENCH=1` prints fps and where each frame's time
goes. `node tools/test-web-offline.mjs` checks that edits made on a share
link while offline survive and go up later (needs the web build in
`server/public`).

Desktop test hooks: `WALL_SCREENSHOT=out.png` saves a frame and exits;
`WALL_CAMERA="x,y,zoom"` sets the start view.

## Shader pipeline (planned)

`slangc shaders/x.slang -target spirv` → `naga x.spv x.glsl` (GLSL ES 3.00 for
WebGL2 and for desktop through ANGLE). A `build.rs` in `wall-render` will run
this and embed the results.
