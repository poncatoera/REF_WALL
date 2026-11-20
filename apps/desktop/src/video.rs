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

/// One still frame at `at` seconds, scaled to `w`×`h`, as RGBA. Used as a
/// video's poster until (or unless) it plays.
pub fn poster(path: &Path, w: u32, h: u32, at: f64) -> Option<Vec<u8>> {
    let out = command(ffmpeg()?)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{at:.3}"), "-i"])
        .arg(path)
        .args(["-an", "-frames:v", "1", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.stdout.len() == (w * h * 4) as usize).then_some(out.stdout)
}

/// A decoded frame: generation (bumped on every seek), time in seconds, RGBA.
type Frame = (u64, f64, Vec<u8>);

/// Plays a file on a loop. A decoder thread runs ffmpeg as fast as the
/// channel lets it; this side paces frames by the clock, which gives pause,
/// speed and frame steps without restarting ffmpeg. Seeks restart it at the
/// new time.
pub struct Player {
    pub width: u32,
    pub height: u32,
    frames: Receiver<Frame>,
    seek_tx: mpsc::Sender<(u64, f64)>,
    gen: u64,
    /// Media time shown, in seconds.
    clock: f64,
    duration: f64,
    fps: f64,
    playing: bool,
    speed: f32,
    pending: Option<Frame>,
    last_tick: std::time::Instant,
    /// In and out points: playback loops between them.
    range: Option<(f64, f64)>,
}

impl Player {
    pub fn start(path: PathBuf, width: u32, height: u32, duration: f64, fps: f64) -> Option<Self> {
        let ffmpeg = ffmpeg()?.to_owned();
        let (w, h) = playback_size(width, height);
        let (tx, rx) = mpsc::sync_channel(3);
        let (seek_tx, seek_rx) = mpsc::channel();
        std::thread::spawn(move || decode(&ffmpeg, &path, w, h, fps, tx, seek_rx));
        Some(Self {
            width: w,
            height: h,
            frames: rx,
            seek_tx,
            gen: 0,
            clock: 0.0,
            duration,
            fps,
            playing: true,
            speed: 1.0,
            pending: None,
            last_tick: std::time::Instant::now(),
            range: None,
        })
    }

    pub fn command(&mut self, cmd: wall_app::VideoCommand) {
        use wall_app::VideoCommand as C;
        match cmd {
            C::Play => self.playing = true,
            C::Pause => self.playing = false,
            C::Speed(s) => self.speed = s,
            C::Seek(t) => self.seek(t),
            C::Range(r) => {
                self.range = r;
                if let Some((a, b)) = r {
                    if self.clock < a || self.clock >= b {
                        self.seek(a);
                    }
                }
            }
            C::Step(n) => {
                self.playing = false;
                let t = self.clock + n as f64 / self.fps;
                if n < 0 { self.seek(t) } else { self.clock = t.min(self.duration) }
            }
        }
    }

    fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.duration.max(0.0));
        self.gen += 1;
        self.clock = t;
        self.pending = None;
        let _ = self.seek_tx.send((self.gen, t));
    }

    pub fn status(&self) -> wall_app::VideoStatus {
        wall_app::VideoStatus { time: self.clock, duration: self.duration, playing: self.playing, speed: self.speed }
    }

    /// Advances the clock and returns the frame to show now, if it changed.
    pub fn tick(&mut self) -> Option<Vec<u8>> {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if self.playing {
            self.clock += dt * self.speed as f64;
            if let Some((a, b)) = self.range {
                if self.clock >= b {
                    self.seek(a);
                }
            } else if self.duration > 0.0 && self.clock >= self.duration {
                self.clock %= self.duration;
            }
        }
        let half = 0.5 / self.fps;
        let mut show = None;
        while let Some(frame) = self.pending.take().or_else(|| self.frames.try_recv().ok()) {
            if frame.0 != self.gen {
                continue; // from before a seek
            }
            if frame.1 > self.clock + 0.5 {
                continue; // from before the loop wrapped
            }
            if frame.1 > self.clock + half {
                self.pending = Some(frame);
                break;
            }
            show = Some(frame.2);
        }
        show
    }
}

fn decode(ffmpeg: &str, path: &Path, w: u32, h: u32, fps: f64, tx: SyncSender<Frame>, seeks: Receiver<(u64, f64)>) {
    let (mut gen, mut start) = (0u64, 0.0f64);
    'outer: loop {
        let child = command(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i"])
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
        let mut n = 0u64;
        let mut restart = None;
        loop {
            if let Some(s) = seeks.try_iter().last() {
                restart = Some(s);
                break;
            }
            let mut frame = vec![0u8; size];
            if out.read_exact(&mut frame).is_err() {
                break; // end of clip: loop from the start
            }
            let t = start + n as f64 / fps;
            n += 1;
            // Blocks while the player is paused or ahead; wake up for seeks.
            let mut item = (gen, t, frame);
            loop {
                match tx.try_send(item) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Full(back)) => {
                        if let Some(s) = seeks.try_iter().last() {
                            restart = Some(s);
                            break;
                        }
                        item = back;
                        std::thread::sleep(std::time::Duration::from_millis(4));
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break 'outer;
                    }
                }
            }
            if restart.is_some() {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        (gen, start) = restart.unwrap_or((gen, 0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_size_caps_width_and_keeps_even() {
        assert_eq!(playback_size(1920, 1080), (1280, 720));
        assert_eq!(playback_size(641, 361), (640, 360));
    }

    /// Needs ffmpeg; skipped without it.
    #[test]
    fn poster_grabs_one_frame() {
        if ffmpeg().is_none() {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("p.mp4");
        let ok = command(ffmpeg().unwrap())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:size=64x36:rate=25", "-t", "2", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(ok.success());
        let frame = poster(&clip, 64, 36, 0.5).expect("a frame");
        assert_eq!(frame.len(), 64 * 36 * 4);
        let colors = wall_media::autotag::dominant_colors(&frame, 64, 36, 2);
        assert_eq!(wall_media::autotag::color_name(&colors[0]), Some("blue"));
    }

    #[test]
    fn player_paces_pauses_steps_and_seeks() {
        if ffmpeg().is_none() {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("c.mp4");
        let ok = command(ffmpeg().unwrap())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25", "-t", "4", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(ok.success());
        let p = probe(&clip).unwrap();
        assert_eq!((p.probe.width, p.probe.height, p.probe.duration_ms), (160, 90, 4000));
        let mut player = Player::start(clip, 160, 90, 4.0, 25.0).unwrap();
        let wait = |player: &mut Player, ms: u64| {
            let mut frames = 0;
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                if player.tick().is_some() {
                    frames += 1;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            frames
        };
        let shown = wait(&mut player, 600);
        assert!(shown > 5, "frames shown while playing: {shown}");
        let t = player.status().time;
        assert!((0.4..1.0).contains(&t), "clock {t}");
        player.command(wall_app::VideoCommand::Pause);
        let paused_at = player.status().time;
        wait(&mut player, 200);
        assert_eq!(player.status().time, paused_at);
        player.command(wall_app::VideoCommand::Step(1));
        assert!((player.status().time - paused_at - 0.04).abs() < 1e-6);
        player.command(wall_app::VideoCommand::Seek(3.0));
        let after_seek = wait(&mut player, 800);
        assert!(after_seek >= 1, "a frame after seeking");
        assert_eq!(player.status().time, 3.0);
        player.command(wall_app::VideoCommand::Speed(2.0));
        player.command(wall_app::VideoCommand::Play);
        wait(&mut player, 600);
        // 3.0 + ~1.2 s at 2x wraps past 4.0 back near 0.2.
        let t = player.status().time;
        assert!(t < 1.0, "wrapped clock {t}");
    }

    #[test]
    fn video_extensions() {
        assert!(is_video(Path::new("a/B.MOV")));
        assert!(!is_video(Path::new("a/b.png")));
    }
}

/// One video on the board: its file, its texture once a frame arrived, and
/// a player while it is on screen. Off screen the player is dropped (its
/// ffmpeg stops) and the texture keeps the last frame.
struct Slot {
    hash: wall_app::ContentHash,
    path: PathBuf,
    width: u32,
    height: u32,
    duration: f64,
    fps: f64,
    tex: Option<wall_render::ImageId>,
    player: Option<Player>,
    /// Where to resume when it comes back on screen.
    saved: wall_app::VideoStatus,
    /// A still frame being grabbed for videos that haven't played yet.
    poster: Option<Receiver<Option<Vec<u8>>>>,
}

/// Imports running in the background and the videos on the board.
#[derive(Default)]
pub struct Videos {
    jobs: Vec<Receiver<(String, Result<Prepared, String>)>>,
    slots: Vec<Slot>,
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
        !self.jobs.is_empty() || self.slots.iter().any(|s| s.player.is_some())
    }

    /// Finishes imports, starts and stops players as videos come on and go
    /// off screen, applies controls, uploads new frames. Call once per frame.
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
            if self.slots.iter().any(|s| s.hash == src.hash) {
                continue;
            }
            let Some(path) = src.path.clone().filter(|p| p.is_file()).or_else(|| self.paths.get(&src.hash).cloned()) else { continue };
            let (duration, fps) = probe(&path).map_or((0.0, 30.0), |p| (p.probe.duration_ms as f64 / 1000.0, p.fps.unwrap_or(30.0) as f64));
            let saved = wall_app::VideoStatus { time: 0.0, duration, playing: true, speed: 1.0 };
            let (pw, ph) = playback_size(src.width, src.height);
            let (tx, rx) = mpsc::sync_channel(1);
            let (p2, at) = (path.clone(), (duration / 3.0).min(1.0));
            std::thread::spawn(move || {
                let _ = tx.send(poster(&p2, pw, ph, at));
            });
            self.slots.push(Slot { hash: src.hash, path, width: src.width, height: src.height, duration, fps, tex: None, player: None, saved, poster: Some(rx) });
        }

        let playable = app.playable_videos();
        for slot in &mut self.slots {
            let want = playable.contains(&slot.hash);
            match (&slot.player, want) {
                (None, true) => {
                    if let Some(mut p) = Player::start(slot.path.clone(), slot.width, slot.height, slot.duration, slot.fps) {
                        if slot.saved.time > 0.0 {
                            p.command(wall_app::VideoCommand::Seek(slot.saved.time));
                        }
                        p.command(wall_app::VideoCommand::Speed(slot.saved.speed));
                        p.command(wall_app::VideoCommand::Range(app.video_range(&slot.hash)));
                        if !slot.saved.playing {
                            p.command(wall_app::VideoCommand::Pause);
                        }
                        slot.player = Some(p);
                    }
                }
                (Some(p), false) => {
                    slot.saved = p.status();
                    slot.player = None;
                }
                _ => {}
            }
        }

        for (hash, cmd) in app.take_video_commands() {
            let Some(slot) = self.slots.iter_mut().find(|s| s.hash == hash) else { continue };
            match &mut slot.player {
                Some(p) => p.command(cmd),
                None => match cmd {
                    wall_app::VideoCommand::Play => slot.saved.playing = true,
                    wall_app::VideoCommand::Pause => slot.saved.playing = false,
                    wall_app::VideoCommand::Speed(s) => slot.saved.speed = s,
                    wall_app::VideoCommand::Seek(t) => slot.saved.time = t,
                    wall_app::VideoCommand::Step(_) | wall_app::VideoCommand::Range(_) => {}
                },
            }
        }

        for slot in &mut self.slots {
            if let Some(rx) = &slot.poster {
                match rx.try_recv() {
                    Ok(frame) => {
                        slot.poster = None;
                        if let Some(frame) = frame {
                            let (pw, ph) = playback_size(slot.width, slot.height);
                            app.video_frame_seen(&slot.hash, &frame, pw, ph);
                            if slot.tex.is_none() {
                                match renderer.create_video_texture(pw, ph) {
                                    Ok(tex) => {
                                        renderer.update_image(tex, &frame, pw, ph);
                                        app.attach_texture(slot.hash.clone(), tex);
                                        slot.tex = Some(tex);
                                    }
                                    Err(e) => eprintln!("wall: video texture failed: {e}"),
                                }
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => slot.poster = None,
                }
            }
            let Some(player) = &mut slot.player else {
                app.set_video_status(slot.hash.clone(), slot.saved);
                continue;
            };
            if let Some(frame) = player.tick() {
                if slot.tex.is_none() {
                    match renderer.create_video_texture(player.width, player.height) {
                        Ok(tex) => {
                            app.attach_texture(slot.hash.clone(), tex);
                            slot.tex = Some(tex);
                        }
                        Err(e) => eprintln!("wall: video texture failed: {e}"),
                    }
                }
                if let Some(tex) = slot.tex {
                    renderer.update_image(tex, &frame, player.width, player.height);
                    app.needs_redraw = true;
                }
            }
            app.set_video_status(slot.hash.clone(), player.status());
        }
    }
}
