//! Importing one picture or video file into a library, with the import
//! rules, colours and suggested tags.

use std::path::Path;

use wall_format::{ContentHash, ItemSidecar, Library, MediaKind};

use crate::Res;

/// Stores a video, converted to the import rules with ffmpeg when needed,
/// and its sidecar. Colours come from a frame one second in.
fn import_video(lib: &Library, path: &Path, name: &str, now: u64) -> Res<(ContentHash, u32, u32)> {
    use wall_media::ffmpeg;
    let before = ffmpeg::probe(path).ok();
    let v = ffmpeg::prepare_video(path)?;
    let converted = v.path != path;
    let hash = lib.import_bytes(&v.bytes, &v.ext).map_err(|e| e.to_string())?.hash().clone();
    if converted {
        let _ = std::fs::remove_file(&v.path);
    }
    if lib.read_sidecar(&hash).is_err() {
        let colors = video_frame(path, &v)
            .map(|(rgba, w, h)| wall_media::autotag::dominant_colors(&rgba, w, h, 4))
            .unwrap_or_default();
        let auto_tags = wall_media::autotag::local_tags(&colors, v.width, v.height, Some(v.duration_ms));
        let original = converted.then(|| before.map(|b| wall_format::OriginalInfo { width: b.probe.width, height: b.probe.height, bytes: b.probe.bytes, kept_as: None })).flatten();
        let sc = ItemSidecar {
            hash: hash.clone(),
            kind: MediaKind::Video,
            ext: v.ext.clone(),
            width: v.width,
            height: v.height,
            bytes: v.bytes.len() as u64,
            duration_ms: Some(v.duration_ms),
            fps: v.fps,
            added_ms: now,
            title: Some(name.to_owned()),
            source_url: None,
            tags: Vec::new(),
            auto_tags,
            colors,
            original,
            range: None,
            extra: Default::default(),
        };
        lib.write_sidecar(&sc).map_err(|e| e.to_string())?;
    }
    Ok((hash, v.width, v.height))
}

/// One small RGBA frame of a video, for its colours.
fn video_frame(path: &Path, v: &wall_media::ffmpeg::PreparedVideo) -> Option<(Vec<u8>, u32, u32)> {
    let w = 96u32;
    let h = ((w as u64 * v.height as u64 / v.width.max(1) as u64) as u32).max(2) & !1;
    let at = format!("{:.2}", (v.duration_ms as f64 / 1000.0).min(1.0) * 0.999);
    let out = wall_media::ffmpeg::command(wall_media::ffmpeg::ffmpeg()?)
        .args(["-v", "error", "-ss", &at, "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .ok()?;
    (out.status.success() && out.stdout.len() == (w * h * 4) as usize).then_some((out.stdout, w, h))
}

/// Stores one file and its sidecar. Returns its hash and pixel size.
pub fn import_one(lib: &Library, path: &Path, name: &str, now: u64) -> Res<(ContentHash, u32, u32)> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if matches!(ext.as_str(), "mp4" | "mov" | "webm" | "m4v" | "mkv" | "avi") {
        return import_video(lib, path, name, now);
    }
    let p = wall_media::prepare_image(&bytes, &wall_media::Limits::default()).map_err(|e| e.to_string())?;
    let hash = lib.import_bytes(&p.bytes, p.ext).map_err(|e| e.to_string())?.hash().clone();
    if lib.read_sidecar(&hash).is_err() {
        let rgba = image::load_from_memory(&p.bytes).map_err(|e| e.to_string())?.to_rgba8();
        let colors = wall_media::autotag::dominant_colors(rgba.as_raw(), p.width, p.height, 4);
        let auto_tags = wall_media::autotag::local_tags(&colors, p.width, p.height, None);
        let sc = ItemSidecar {
            hash: hash.clone(),
            kind: MediaKind::Image,
            ext: p.ext.into(),
            width: p.width,
            height: p.height,
            bytes: p.bytes.len() as u64,
            duration_ms: None,
            fps: None,
            added_ms: now,
            title: Some(name.to_owned()),
            source_url: None,
            tags: Vec::new(),
            auto_tags,
            colors,
            original: p.original.clone(),
            range: None,
            extra: Default::default(),
        };
        lib.write_sidecar(&sc).map_err(|e| e.to_string())?;
    }
    Ok((hash, p.width, p.height))
}

