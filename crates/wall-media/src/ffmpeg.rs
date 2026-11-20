//! Running ffmpeg and ffprobe on desktop: finding them, probing a video,
//! and converting it to the import rules ([`crate::plan_video`]). Used by
//! the desktop app and `wall-cli`. Not built for the web.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::{plan_video, H264Encoder, Limits, VideoPlan, VideoProbe};

/// File extensions treated as video on drop.
pub fn is_video(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi")
}

/// `WALL_FFMPEG`, else an ffmpeg next to the Wall executable (the packaged
/// app ships one), else `ffmpeg` on PATH, if it runs.
pub fn ffmpeg() -> Option<&'static str> {
    static FOUND: OnceLock<Option<String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let bundled = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })))
                .filter(|p| p.is_file())
                .map(|p| p.to_string_lossy().into_owned());
            let exe = std::env::var("WALL_FFMPEG").ok().or(bundled).unwrap_or_else(|| "ffmpeg".into());
            let ok = command(&exe).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
            if !ok {
                eprintln!("wall: ffmpeg not found (set WALL_FFMPEG); videos will not play");
            }
            ok.then_some(exe)
        })
        .as_deref()
}

pub fn ffprobe() -> String {
    match ffmpeg() {
        Some(f) if f != "ffmpeg" => {
            let p = Path::new(f);
            let name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
            p.with_file_name(name).to_string_lossy().into_owned()
        }
        _ => "ffprobe".into(),
    }
}

/// A process without a console window flashing up on Windows.
pub fn command(exe: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub struct Probed {
    pub probe: VideoProbe,
    pub fps: Option<f32>,
}

pub fn probe(path: &Path) -> Result<Probed, String> {
    let out = command(&ffprobe())
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,avg_frame_rate:format=duration,format_name", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").ok_or("no video stream")?;
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let fps = video["avg_frame_rate"].as_str().and_then(|r| {
        let (n, d) = r.split_once('/')?;
        let (n, d): (f32, f32) = (n.parse().ok()?, d.parse().ok()?);
        (d > 0.0 && n > 0.0).then_some(n / d)
    });
    let duration: f64 = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let format = v["format"]["format_name"].as_str().unwrap_or("");
    // ffprobe reports mp4 and mov as "mov,mp4,m4a,3gp,3g2,mj2"; go by extension then.
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let container = if format.contains("mp4") { if ext == "mov" { "mov".into() } else { "mp4".into() } } else { format.split(',').next().unwrap_or("").to_owned() };
    Ok(Probed {
        probe: VideoProbe {
            bytes: std::fs::metadata(path).map_err(|e| e.to_string())?.len(),
            duration_ms: (duration * 1000.0) as u64,
            width: video["width"].as_u64().unwrap_or(0) as u32,
            height: video["height"].as_u64().unwrap_or(0) as u32,
            codec: video["codec_name"].as_str().unwrap_or("").to_owned(),
            container,
            has_audio,
            max_keyframe_gap_ms: None,
        },
        fps,
    })
}

/// H.264 encoders this ffmpeg has, best first: libx264 (GPL builds), then
/// the system's (VideoToolbox on macOS, Media Foundation on Windows), then
/// NVIDIA's. `WALL_H264_ENCODER=h264_mf` (etc.) puts one first.
pub fn h264_encoders() -> &'static [H264Encoder] {
    static FOUND: OnceLock<Vec<H264Encoder>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let listed = ffmpeg()
            .and_then(|f| command(f).args(["-hide_banner", "-encoders"]).stderr(Stdio::null()).output().ok())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let has = |e: &H264Encoder| listed.split_whitespace().any(|w| w == e.ffmpeg_name());
        let mut found: Vec<H264Encoder> = H264Encoder::ALL.into_iter().filter(has).collect();
        if let Ok(want) = std::env::var("WALL_H264_ENCODER") {
            if let Some(i) = found.iter().position(|e| e.ffmpeg_name() == want) {
                let e = found.remove(i);
                found.insert(0, e);
            }
        }
        found
    })
}

/// A video ready to import: bytes under the limit, as H.264 MP4 unless the
/// original already qualified.
pub struct PreparedVideo {
    pub bytes: Vec<u8>,
    pub ext: String,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub fps: Option<f32>,
    /// A file holding `bytes`: the original, or the converted temporary file.
    pub path: PathBuf,
}

/// Probes a video and converts it if the rules say so. Slow for big files:
/// run it off the UI thread.
pub fn prepare_video(path: &Path) -> Result<PreparedVideo, String> {
    let ffmpeg = ffmpeg().ok_or("ffmpeg is needed to import videos")?;
    let p = probe(path)?;
    let (path, p) = match plan_video(&p.probe, &Limits::default()).map_err(|e| e.to_string())? {
        VideoPlan::Keep => (path.to_owned(), p),
        VideoPlan::Reencode(settings) => {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let out = std::env::temp_dir().join(format!("wall-{}-{nanos}.mp4", std::process::id()));
            println!("wall: converting {} ({:?})", path.display(), settings.reasons);
            let mut converted = false;
            for enc in h264_encoders() {
                let args = settings.ffmpeg_args_with(&path.to_string_lossy(), &out.to_string_lossy(), p.fps.unwrap_or(30.0), *enc);
                let status = command(ffmpeg).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| e.to_string())?;
                if status.success() {
                    converted = true;
                    break;
                }
                // A GPU encoder can be listed but have no GPU behind it: try the next.
                eprintln!("wall: {} could not convert {}; trying the next encoder", enc.ffmpeg_name(), path.display());
            }
            if !converted {
                return Err(format!("ffmpeg could not convert {} (no working H.264 encoder)", path.display()));
            }
            let again = probe(&out)?;
            (out, again)
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    Ok(PreparedVideo { bytes, ext, width: p.probe.width, height: p.probe.height, duration_ms: p.probe.duration_ms, fps: p.fps, path })
}
