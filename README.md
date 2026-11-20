# Wall

Reference, moodboard and motion-study tool. One Rust codebase, built as a
desktop app (macOS, Windows) and a web app (WASM + WebGL2 on Cloudflare).

Architecture proposal: https://claude.ai/artifact/SCu78YrUEzKsEE8CEoSZ49

## Layout

| Path | What | Status |
| --- | --- | --- |
| `crates/wall-format` | The open `.wall` library format: manifest, media by BLAKE3 hash, sidecars, boards, per-item merge for sync | Working, 7 tests |
| `crates/wall-media` | Import rules: images max 2048 px wide, videos max 20 MB / 1080p, H.264 MP4 with a keyframe every second | Working, 13 tests |
| `shaders/` | Slang sources, compiled to SPIR-V and then GLSL ES 3.00 | First shader written, not compiled yet |
| `crates/wall-render`, `wall-text`, `wall-app` | Renderer, Slug-method text, canvas | Not started |
| `apps/desktop`, `apps/web`, `worker` | Shells and the Cloudflare Worker | Not started |

Both crates also build for `wasm32-unknown-unknown`, so the web app uses the
same rules and format code.

## Commands

```sh
cargo test                                   # all tests
cargo build --target wasm32-unknown-unknown -p wall-format -p wall-media
```

## Shader pipeline (planned)

`slangc shaders/x.slang -target spirv` → `naga x.spv x.glsl` (GLSL ES 3.00 for
WebGL2 and for desktop through ANGLE). A `build.rs` in `wall-render` will run
this and embed the results.
