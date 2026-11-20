//! Desktop video: probing and converting with ffmpeg (the 20 MB / 1080p
//! rules from `wall-media`), and playback by piping decoded RGBA frames
//! from an ffmpeg process. Without ffmpeg, videos show as blank cards.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::OnceLock;

use wall_app::VideoMeta;
use wall_media::{plan_video, Limits, VideoPlan, VideoProbe};

/// File extensions treated as video on drop.
pub fn is_video(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi")
}

/// `WALL_FFMPEG` or `ffmpeg` on PATH, if it runs.
pub fn ffmpeg() -> Option<&'static str> {
    static FOUND: OnceLock<Option<String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let exe = std::env::var("WALL_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
            let ok = command(&exe).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
            if !ok {
                eprintln!("wall: ffmpeg not found (set WALL_FFMPEG); videos will not play");
            }
            ok.then_some(exe)
        })
        .as_deref()
}

fn ffprobe() -> String {
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
fn command(exe: &str) -> Command {
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

/// A video ready to import: bytes under the limit, as H.264 MP4 unless the
/// original already qualified.
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub ext: String,
    pub meta: VideoMeta,
    /// A file holding `bytes`, for playback when no library is open.
    pub path: PathBuf,
}

/// Probes a video and converts it if the rules say so. Slow for big files:
/// run it off the UI thread.
pub fn prepare(path: &Path) -> Result<Prepared, String> {
    let ffmpeg = ffmpeg().ok_or("ffmpeg is needed to import videos")?;
    let p = probe(path)?;
    let (path, p) = match plan_video(&p.probe, &Limits::default()).map_err(|e| e.to_string())? {
        VideoPlan::Keep => (path.to_owned(), p),
        VideoPlan::Reencode(settings) => {
            let out = std::env::temp_dir().join(format!("wall-{}-{}.mp4", std::process::id(), crate::now_ms()));
            println!("wall: converting {} ({:?})", path.display(), settings.reasons);
            let args = settings.ffmpeg_args(&path.to_string_lossy(), &out.to_string_lossy(), p.fps.unwrap_or(30.0));
            let status = command(ffmpeg).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("ffmpeg could not convert {}", path.display()));
            }
            let again = probe(&out)?;
            (out, again)
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let meta = VideoMeta { width: p.probe.width, height: p.probe.height, duration_ms: p.probe.duration_ms, fps: p.fps };
    Ok(Prepared { bytes, ext, meta, path })
}

/// Size frames are decoded at: at most 1280 wide, even.
pub fn playback_size(w: u32, h: u32) -> (u32, u32) {
    let scale = (1280.0 / w.max(1) as f32).min(1.0);
    let even = |v: f32| ((v.round() as u32) & !1).max(2);
    (even(w as f32 * scale), even(h as f32 * scale))
}

/// Plays a file on a loop at its own frame rate, sending RGBA frames.
pub struct Player {
    pub width: u32,
    pub height: u32,
    frames: Receiver<Vec<u8>>,
}

impl Player {
    pub fn start(path: PathBuf, width: u32, height: u32) -> Option<Self> {
        let ffmpeg = ffmpeg()?.to_owned();
        let (w, h) = playback_size(width, height);
        let (tx, rx) = mpsc::sync_channel(2);
        std::thread::spawn(move || play(&ffmpeg, &path, w, h, tx));
        Some(Self { width: w, height: h, frames: rx })
    }

    /// The newest decoded frame, if one arrived since the last call.
    pub fn latest(&self) -> Option<Vec<u8>> {
        let mut last = None;
        while let Ok(f) = self.frames.try_recv() {
            last = Some(f);
        }
        last
    }
}

fn play(ffmpeg: &str, path: &Path, w: u32, h: u32, tx: SyncSender<Vec<u8>>) {
    let child = command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-re", "-stream_loop", "-1", "-i"])
        .arg(path)
        .args(["-an", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("wall: could not play {}: {e}", path.display());
            return;
        }
    };
    let Some(mut out) = child.stdout.take() else { return };
    let size = (w * h * 4) as usize;
    loop {
        let mut frame = vec![0u8; size];
        if out.read_exact(&mut frame).is_err() {
            break;
        }
        match tx.try_send(frame) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_size_caps_width_and_keeps_even() {
        assert_eq!(playback_size(1920, 1080), (1280, 720));
        assert_eq!(playback_size(641, 361), (640, 360));
    }

    #[test]
    fn video_extensions() {
        assert!(is_video(Path::new("a/B.MOV")));
        assert!(!is_video(Path::new("a/b.png")));
    }
}

/// Imports running in the background and players for videos on the board.
#[derive(Default)]
pub struct Videos {
    jobs: Vec<Receiver<(String, Result<Prepared, String>)>>,
    players: Vec<(Player, wall_render::ImageId)>,
    started: std::collections::HashSet<wall_app::ContentHash>,
    /// Files for videos imported without a library open.
    paths: std::collections::HashMap<wall_app::ContentHash, PathBuf>,
    /// When the last import finished.
    settled_at: Option<std::time::Instant>,
}

impl Videos {
    /// Starts probing / converting a dropped file off the UI thread.
    pub fn import(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("video").to_owned();
        std::thread::spawn(move || {
            let _ = tx.send((name, prepare(&path)));
        });
        self.jobs.push(rx);
    }

    /// No imports running and, if any finished, at least `ms` since then
    /// (gives players time to show frames; used for test screenshots).
    pub fn settled(&self, ms: u64) -> bool {
        self.jobs.is_empty() && self.settled_at.is_none_or(|t| t.elapsed().as_millis() as u64 >= ms)
    }

    /// True while anything needs frames drawn: imports or playing videos.
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || !self.players.is_empty()
    }

    /// Finishes imports, starts players, uploads new frames. Call once per frame.
    pub fn pump(&mut self, app: &mut wall_app::App, renderer: &mut wall_render::Renderer) {
        let mut still = Vec::new();
        for rx in self.jobs.drain(..) {
            match rx.try_recv() {
                Ok((name, Ok(p))) => match app.import_video(&p.bytes, &p.ext, p.meta, Some(&name), crate::now_ms()) {
                    Ok(id) => {
                        if let Some(hash) = app.media_hash(&id) {
                            self.paths.insert(hash, p.path);
                        }
                    }
                    Err(e) => eprintln!("wall: could not import {name}: {e}"),
                },
                Ok((name, Err(e))) => eprintln!("wall: could not import {name}: {e}"),
                Err(mpsc::TryRecvError::Empty) => still.push(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if self.jobs.len() != still.len() && still.is_empty() {
            self.settled_at = Some(std::time::Instant::now());
        }
        self.jobs = still;

        for src in app.video_sources() {
            if !self.started.insert(src.hash.clone()) {
                continue;
            }
            let Some(path) = src.path.clone().filter(|p| p.is_file()).or_else(|| self.paths.get(&src.hash).cloned()) else { continue };
            let Some(player) = Player::start(path, src.width, src.height) else { continue };
            match renderer.create_video_texture(player.width, player.height) {
                Ok(tex) => {
                    app.attach_texture(src.hash, tex);
                    self.players.push((player, tex));
                }
                Err(e) => eprintln!("wall: video texture failed: {e}"),
            }
        }

        for (player, tex) in &self.players {
            if let Some(frame) = player.latest() {
                renderer.update_image(*tex, &frame, player.width, player.height);
                app.needs_redraw = true;
            }
        }
    }
}
