//! Desktop video: probing and converting with ffmpeg (the 20 MB / 1080p
//! rules from `wall-media`), and playback by piping decoded RGBA frames
//! from an ffmpeg process. Without ffmpeg, videos show as blank cards.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, SyncSender};

use wall_app::VideoMeta;

pub use wall_media::ffmpeg::{command, ffmpeg, is_video, probe};

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
    let v = wall_media::ffmpeg::prepare_video(path)?;
    let meta = VideoMeta { width: v.width, height: v.height, duration_ms: v.duration_ms, fps: v.fps };
    Ok(Prepared { bytes: v.bytes, ext: v.ext, meta, path: v.path })
}

/// Size frames are decoded at: at most `max_w` wide, even.
pub fn playback_size_max(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    let scale = (max_w as f32 / w.max(1) as f32).min(1.0);
    let even = |v: f32| ((v.round() as u32) & !1).max(2);
    (even(w as f32 * scale), even(h as f32 * scale))
}

/// Decode width for a video shown `shown_px` wide: 320, 640 or 1280, so a
/// wall of small videos doesn't decode and upload full HD frames.
pub fn decode_width(shown_px: f32) -> u32 {
    match shown_px {
        p if p <= 320.0 => 320,
        p if p <= 640.0 => 640,
        _ => 1280,
    }
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

/// A picture Wall doesn't keep itself (GIF, BMP, TIFF, AVIF…), as a still
/// PNG. Not HEIC: ffmpeg may give only a tile of it.
pub fn to_png(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "gif" | "bmp" | "tif" | "tiff" | "avif" | "tga" | "psd") {
        return None;
    }
    let out = command(ffmpeg()?)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-an", "-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "pipe:1"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.stdout.starts_with(b"\x89PNG").then_some(out.stdout)
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
    /// Back and forth: one pass forwards records the loop's frames, then
    /// it plays them backwards from memory (ffmpeg only decodes forwards).
    pingpong: bool,
    cache: Vec<(f64, Vec<u8>)>,
    cache_bytes: usize,
    /// Recording frames since the loop restarted at its start.
    recording: bool,
    /// The cache holds a whole forward pass.
    cache_ready: bool,
    /// The loop is too big to keep in memory: plain looping instead.
    too_big: bool,
    /// Playing backwards: the time shown, and the cached frame shown last.
    back: Option<(f64, usize)>,
    /// Onion skin: blend the frames before and after into the one shown.
    onion: bool,
    /// The frame shown now, unblended, and the one shown before it.
    current: Option<Vec<u8>>,
    previous: Option<Vec<u8>>,
    /// The picture must be sent again (onion skin turned on or off).
    redraw: bool,
    /// The last picture sent had the frame after blended in.
    showed_after: bool,
}

/// Most memory one video's back-and-forth loop may hold.
const PINGPONG_CACHE_BYTES: usize = 256 << 20;

impl Player {
    /// Plays at most `max_w` pixels wide (see [`decode_width`]).
    pub fn start(path: PathBuf, width: u32, height: u32, duration: f64, fps: f64, max_w: u32) -> Option<Self> {
        let ffmpeg = ffmpeg()?.to_owned();
        let (w, h) = playback_size_max(width, height, max_w);
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
            pingpong: false,
            cache: Vec::new(),
            cache_bytes: 0,
            recording: false,
            cache_ready: false,
            too_big: false,
            back: None,
            onion: false,
            current: None,
            previous: None,
            redraw: false,
            showed_after: false,
        })
    }

    pub fn command(&mut self, cmd: wall_app::VideoCommand) {
        use wall_app::VideoCommand as C;
        match cmd {
            C::Play => self.playing = true,
            C::Pause => self.playing = false,
            C::Speed(s) => self.speed = s,
            C::Seek(t) => {
                self.forget_loop();
                self.seek(t);
            }
            C::Onion(on) => {
                self.onion = on;
                self.redraw = true;
            }
            C::PingPong(on) => {
                self.forget_loop();
                self.pingpong = on;
                self.too_big = false;
            }
            C::Range(r) => {
                self.forget_loop();
                self.range = r;
                if let Some((a, b)) = r {
                    let t = self.position();
                    if t < a || t >= b {
                        self.seek(a);
                    }
                }
            }
            C::Step(n) => {
                self.playing = false;
                if let Some((t, _)) = self.back {
                    self.forget_loop();
                    self.seek(t + n as f64 / self.fps);
                    return;
                }
                if n < 0 {
                    self.seek(self.position() + n as f64 / self.fps);
                } else {
                    self.clock += n as f64 / self.fps;
                }
            }
        }
    }

    /// Drops the recorded loop (it changed, or playback jumped).
    fn forget_loop(&mut self) {
        self.cache.clear();
        self.cache_bytes = 0;
        self.recording = false;
        self.cache_ready = false;
        self.back = None;
    }

    fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.duration.max(0.0));
        self.gen += 1;
        self.clock = t;
        self.pending = None;
        // The frames around the old time say nothing about the new one.
        self.current = None;
        self.previous = None;
        let _ = self.seek_tx.send((self.gen, t));
    }

    /// Where playback is within the clip. `clock` itself keeps counting
    /// through loops, like the decoder's frame times, so a loop needs no
    /// restart of ffmpeg.
    fn position(&self) -> f64 {
        if self.duration > 0.0 { self.clock % self.duration } else { self.clock }
    }

    pub fn status(&self) -> wall_app::VideoStatus {
        let time = self.back.map_or_else(|| self.position(), |(t, _)| t);
        wall_app::VideoStatus { time, duration: self.duration, playing: self.playing, speed: self.speed }
    }

    /// Advances the clock and returns the picture to show now, if it changed
    /// (the frame, with onion skin blended in when that's on).
    pub fn tick(&mut self) -> Option<Vec<u8>> {
        let frame = self.next_frame();
        let after_ready = self.onion && self.back.is_none() && self.pending.as_ref().is_some_and(|p| p.0 == self.gen);
        match frame {
            Some(f) => self.previous = self.current.replace(f),
            // The frame after turned up late: show it too.
            None if self.redraw || (after_ready && !self.showed_after) => {}
            None => return None,
        }
        self.redraw = false;
        let cur = self.current.as_ref()?;
        if !self.onion {
            return Some(cur.clone());
        }
        let after = self.pending.as_ref().filter(|_| after_ready).map(|p| p.2.as_slice());
        self.showed_after = after.is_some();
        Some(onion_skin(cur, self.previous.as_deref(), after))
    }

    fn next_frame(&mut self) -> Option<Vec<u8>> {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if let Some((mut t, shown)) = self.back {
            // Backwards, from the recorded frames.
            let (a, _) = self.range.unwrap_or((0.0, self.duration));
            if self.playing {
                t -= dt * self.speed as f64;
            }
            if t <= a {
                self.back = None;
                self.seek(a);
                return self.cache.first().map(|f| f.1.clone());
            }
            let k = self.cache.partition_point(|f| f.0 <= t).saturating_sub(1);
            self.back = Some((t, k));
            return (k != shown).then(|| self.cache[k].1.clone());
        }
        if self.playing {
            self.clock += dt * self.speed as f64;
            let (a, b) = self.range.unwrap_or((0.0, self.duration));
            let end = match self.range {
                Some(_) => self.position() >= b || self.clock >= self.duration.max(b),
                None => self.pingpong && self.clock >= self.duration,
            };
            if end {
                if self.pingpong && (self.cache_ready || self.recording) && !self.cache.is_empty() {
                    self.cache_ready = true;
                    self.recording = false;
                    let last = self.cache.len() - 1;
                    self.back = Some((self.cache[last].0, usize::MAX));
                    return None;
                }
                self.seek(a);
                if self.pingpong && !self.too_big {
                    self.cache.clear();
                    self.cache_bytes = 0;
                    self.recording = true;
                }
            }
        }
        let half = 0.5 / self.fps;
        let mut show = None;
        while let Some(frame) = self.pending.take().or_else(|| self.frames.try_recv().ok()) {
            if frame.0 != self.gen {
                continue; // from before a seek
            }

            if frame.1 > self.clock + half {
                self.pending = Some(frame);
                break;
            }
            show = Some(frame.2);
        }
        if self.recording {
            if let Some(px) = &show {
                let t = self.position();
                if self.cache.last().is_none_or(|l| t > l.0) {
                    self.cache_bytes += px.len();
                    self.cache.push((t, px.clone()));
                }
                if self.cache_bytes > PINGPONG_CACHE_BYTES {
                    self.forget_loop();
                    self.too_big = true;
                }
            }
        }
        show
    }
}

/// The `gpu_video_decode` setting, read once.
/// Blends the frame before (warm) and after (cool) faintly into `cur`, so
/// movement shows as coloured edges. All three are RGBA of the same size.
pub fn onion_skin(cur: &[u8], before: Option<&[u8]>, after: Option<&[u8]>) -> Vec<u8> {
    const GHOST: u32 = 72; // of 256, for each neighbour
    const WARM: [u32; 3] = [256, 150, 110];
    const COOL: [u32; 3] = [110, 190, 256];
    let before = before.filter(|b| b.len() == cur.len());
    let after = after.filter(|a| a.len() == cur.len());
    let mut out = cur.to_vec();
    for (i, px) in out.chunks_exact_mut(4).enumerate() {
        let o = i * 4;
        for c in 0..3 {
            let mut keep = 256u32;
            let mut sum = 0u32;
            if let Some(b) = before {
                keep -= GHOST;
                sum += GHOST * ((b[o + c] as u32 * WARM[c]) >> 8);
            }
            if let Some(a) = after {
                keep -= GHOST;
                sum += GHOST * ((a[o + c] as u32 * COOL[c]) >> 8);
            }
            px[c] = ((keep * cur[o + c] as u32 + sum) >> 8).min(255) as u8;
        }
    }
    out
}

pub fn gpu_decode() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(crate::settings::gpu_video_decode)
}

fn decode(ffmpeg: &str, path: &Path, w: u32, h: u32, fps: f64, tx: SyncSender<Frame>, seeks: Receiver<(u64, f64)>) {
    let (mut gen, mut start) = (0u64, 0.0f64);
    'outer: loop {
        let child = command(ffmpeg)
            // Two decode threads each: with many videos on screen, ffmpeg's
            // default (one per core, plus frame buffers each) oversubscribes
            // the CPU and costs ~150 MB per video.
            .args(["-hide_banner", "-loglevel", "error"])
            // GPU decoding when the setting is on; ffmpeg falls back to the
            // CPU by itself when the GPU can't decode a file.
            .args(if gpu_decode() { &["-hwaccel", "auto"][..] } else { &[][..] })
            .args(["-threads", "2", "-stream_loop", "-1", "-ss", &format!("{start:.3}"), "-i"])
            .arg(path)
            .args(["-an", "-vf", &format!("scale={w}:{h}"), "-threads", "2", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
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
                break; // ffmpeg stopped (it loops by itself): start it again
            }
            // Keeps counting through loops (-stream_loop), like Player::clock.
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
        assert_eq!(playback_size_max(1920, 1080, 1280), (1280, 720));
        assert_eq!(playback_size_max(641, 361, 1280), (640, 360));
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
    fn pingpong_plays_the_loop_backwards_after_one_pass() {
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
        let mut player = Player::start(clip, 160, 90, 4.0, 25.0, 1280).unwrap();
        player.command(wall_app::VideoCommand::Range(Some((1.0, 1.6))));
        player.command(wall_app::VideoCommand::PingPong(true));
        player.command(wall_app::VideoCommand::Speed(2.0));
        // Sample the clock: after a first plain pass and a recorded one, it
        // must go down (backwards) and stay inside the loop.
        let mut times = Vec::new();
        let end = std::time::Instant::now() + std::time::Duration::from_millis(2500);
        while std::time::Instant::now() < end {
            player.tick();
            times.push(player.status().time);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let fell = times.windows(2).filter(|w| w[1] < w[0] - 1e-9 && w[0] - w[1] < 0.1).count();
        assert!(fell > 10, "clock ran backwards for a while: {fell} small steps down");
        assert!(times.iter().skip(5).all(|t| (0.95..=1.65).contains(t)), "stays in the loop: {:?}", times.iter().fold((9.0f64, 0.0f64), |m, t| (m.0.min(*t), m.1.max(*t))));
        assert!(player.cache_ready && !player.cache.is_empty());
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
        let mut player = Player::start(clip, 160, 90, 4.0, 25.0, 1280).unwrap();
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
        // Playback carries on through the loop (ffmpeg loops by itself).
        let shown = wait(&mut player, 1500);
        assert!(shown > 20, "frames shown after the loop: {shown}");
        let t2 = player.status().time;
        assert!(t2 < 4.0 && t2 > t, "{t} then {t2}");
    }

    #[test]
    fn onion_skin_shows_once_paused_with_both_neighbours() {
        if ffmpeg().is_none() {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("c.mp4");
        let ok = command(ffmpeg().unwrap())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25", "-t", "2", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(ok.success());
        let mut player = Player::start(clip, 160, 90, 2.0, 25.0, 1280).unwrap();
        let wait = |p: &mut Player, ms: u64| {
            let mut last = None;
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                if let Some(f) = p.tick() {
                    last = Some(f);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            last
        };
        wait(&mut player, 400);
        player.command(wall_app::VideoCommand::Pause);
        player.command(wall_app::VideoCommand::Step(1));
        wait(&mut player, 200);
        player.command(wall_app::VideoCommand::Onion(true));
        let shown = wait(&mut player, 200).expect("turning onion skin on redraws");
        assert!(player.previous.is_some() && player.showed_after, "blends the frame before and after");
        assert_ne!(&shown, player.current.as_ref().unwrap());
        player.command(wall_app::VideoCommand::Onion(false));
        assert_eq!(&wait(&mut player, 50).unwrap(), player.current.as_ref().unwrap(), "off again: the plain frame");
    }

    #[test]
    fn onion_skin_tints_the_neighbours_into_the_frame() {
        let cur = [100u8, 100, 100, 255];
        assert_eq!(onion_skin(&cur, None, None), cur.to_vec());
        let white = [255u8; 4];
        let out = onion_skin(&cur, Some(&white), None);
        assert!(out[0] > out[2], "the frame before shows warm: {out:?}");
        let out = onion_skin(&cur, None, Some(&white));
        assert!(out[2] > out[0], "the frame after shows cool: {out:?}");
        assert_eq!(out[3], 255);
        assert_eq!(onion_skin(&cur, Some(&[0u8; 8]), None), cur.to_vec(), "a frame of another size is ignored");
    }

    #[test]
    fn video_extensions() {
        assert!(is_video(Path::new("a/B.MOV")));
        assert!(!is_video(Path::new("a/b.png")));
    }
}

/// Still frames and file checks running at once. Each is an ffmpeg
/// process; a board of hundreds of videos would otherwise start hundreds.
const MAX_JOBS: usize = 3;
/// Still frames are at most this wide (a video off screen or not yet
/// playing needs no more).
const POSTER_MAX_W: u32 = 640;

/// What a background job found: length and frame rate (if it had to look),
/// and a still frame with its size.
type JobDone = (Option<(f64, f64)>, Option<(Vec<u8>, u32, u32)>);

/// One video on the board: its file, its texture once a frame arrived, and
/// a player while it is on screen. Off screen the player is dropped (its
/// ffmpeg stops) and the texture keeps the last frame, made smaller.
struct Slot {
    hash: wall_app::ContentHash,
    path: PathBuf,
    width: u32,
    height: u32,
    duration: f64,
    fps: f64,
    /// Length and frame rate are known (from its details or a probe).
    probed: bool,
    tex: Option<wall_render::ImageId>,
    /// Size of `tex`, to replace it when the player's size changes.
    tex_size: (u32, u32),
    player: Option<Player>,
    /// Where to resume when it comes back on screen.
    saved: wall_app::VideoStatus,
    /// A probe and/or still frame being made off the UI thread.
    job: Option<Receiver<JobDone>>,
    /// A still frame was tried already.
    posted: bool,
    /// Folder for still frames (`cache/posters`), if in a library.
    cache: Option<PathBuf>,
}

/// A still frame from the cache, else from ffmpeg (then kept in the cache).
fn cached_poster(cache: Option<&Path>, hash: &wall_app::ContentHash, path: &Path, w: u32, h: u32, at: f64) -> Option<Vec<u8>> {
    let file = cache.map(|c| c.join(format!("{hash}-{w}x{h}.jpg")));
    if let Some(img) = file.as_ref().and_then(|f| image::open(f).ok()) {
        let rgba = img.to_rgba8();
        if rgba.dimensions() == (w, h) {
            return Some(rgba.into_raw());
        }
    }
    let px = poster(path, w, h, at)?;
    if let (Some(file), Some(img)) = (file, image::RgbaImage::from_raw(w, h, px.clone())) {
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")));
        let tmp = file.with_extension("tmp");
        if rgb.save_with_format(&tmp, image::ImageFormat::Jpeg).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
    Some(px)
}

/// Imports running in the background and the videos on the board.
#[derive(Default)]
pub struct Videos {
    jobs: Vec<(Option<u64>, Receiver<(String, Result<Prepared, String>)>)>,
    /// Tagged imports that finished: (tag, the new item if it worked).
    pub imported: Vec<(u64, Option<uuid::Uuid>)>,
    slots: Vec<Slot>,
    /// Files for videos imported without a library open.
    paths: std::collections::HashMap<wall_app::ContentHash, PathBuf>,
    /// Videos let go of when they left the open board: length, frame rate
    /// and where they were, for when they come back.
    known: std::collections::HashMap<wall_app::ContentHash, (f64, f64, wall_app::VideoStatus)>,
    /// When the last import finished.
    settled_at: Option<std::time::Instant>,
    /// Files that couldn't be added, and why; the window takes them.
    pub failed: Vec<String>,
}

impl Videos {
    /// Starts probing / converting a dropped file off the UI thread.
    pub fn import(&mut self, path: PathBuf) {
        self.import_tagged(path, None);
    }

    /// Like [`Self::import`]; the item lands in [`Self::imported`] with `tag`.
    pub fn import_tagged(&mut self, path: PathBuf, tag: Option<u64>) {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("video").to_owned();
        std::thread::spawn(move || {
            let _ = tx.send((name, prepare(&path)));
        });
        self.jobs.push((tag, rx));
    }

    /// No imports running and, if any finished, at least `ms` since then
    /// (gives players time to show frames; used for test screenshots).
    pub fn settled(&self, ms: u64) -> bool {
        self.jobs.is_empty() && self.slots.iter().all(|s| s.job.is_none()) && self.settled_at.is_none_or(|t| t.elapsed().as_millis() as u64 >= ms)
    }

    /// True while anything needs frames drawn: imports, still frames on
    /// their way, or playing videos.
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || self.slots.iter().any(|s| s.player.is_some() || s.job.is_some())
    }

    /// Finishes imports, starts and stops players as videos come on and go
    /// off screen, applies controls, uploads new frames. Call once per frame.
    pub fn pump(&mut self, app: &mut wall_app::App, renderer: &mut wall_render::Renderer) {
        let mut still = Vec::new();
        for (tag, rx) in self.jobs.drain(..) {
            let done = |imported: &mut Vec<(u64, Option<uuid::Uuid>)>, id| {
                if let Some(t) = tag {
                    imported.push((t, id));
                }
            };
            match rx.try_recv() {
                Ok((name, Ok(p))) => match app.import_video(&p.bytes, &p.ext, p.meta, Some(&name), crate::now_ms()) {
                    Ok(id) => {
                        done(&mut self.imported, Some(id));
                        if let Some(hash) = app.media_hash(&id) {
                            self.paths.insert(hash, p.path);
                        }
                    }
                    Err(e) => {
                        done(&mut self.imported, None);
                        eprintln!("wall: could not import {name}: {e}");
                        self.failed.push(format!("{name}: {e}"));
                    }
                },
                Ok((name, Err(e))) => {
                    done(&mut self.imported, None);
                    eprintln!("wall: could not import {name}: {e}");
                    self.failed.push(format!("{name}: {e}"));
                }
                Err(mpsc::TryRecvError::Empty) => still.push((tag, rx)),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if self.jobs.len() != still.len() && still.is_empty() {
            self.settled_at = Some(std::time::Instant::now());
        }
        self.jobs = still;

        // Videos no longer on the open board (another board was opened):
        // their players and textures go, so visiting many boards of videos
        // doesn't pile up memory.
        let shown = app.videos_shown();
        let known = &mut self.known;
        self.slots.retain(|s| {
            if shown.contains_key(&s.hash) {
                return true;
            }
            if let Some(t) = s.tex {
                renderer.delete_image(t);
                app.forget_video_texture(&s.hash);
            }
            if s.probed {
                let saved = s.player.as_ref().map_or(s.saved, |p| p.status());
                known.insert(s.hash.clone(), (s.duration, s.fps, saved));
            }
            false
        });

        // New videos: nothing slow here (no ffmpeg on this thread).
        for src in app.video_sources() {
            if self.slots.iter().any(|s| s.hash == src.hash) {
                continue;
            }
            let Some(path) = src.path.clone().filter(|p| p.is_file()).or_else(|| self.paths.get(&src.hash).cloned()) else { continue };
            let from_details = src.duration_ms.map(|d| (d as f64 / 1000.0, src.fps.map_or(30.0, f64::from)));
            let (duration, fps, saved, probed) = match (self.known.get(&src.hash), from_details) {
                (Some(&(d, f, saved)), _) => (d, f, saved, true),
                (None, Some((d, f))) => (d, f, wall_app::VideoStatus { time: 0.0, duration: d, playing: true, speed: 1.0 }, true),
                (None, None) => (0.0, 30.0, wall_app::VideoStatus { time: 0.0, duration: 0.0, playing: true, speed: 1.0 }, false),
            };
            self.slots.push(Slot { hash: src.hash, path, width: src.width, height: src.height, duration, fps, probed, tex: None, tex_size: (0, 0), player: None, saved, job: None, posted: false, cache: src.poster_cache });
        }

        // Probes and still frames, a few at a time: what plays first, then
        // what's on screen, then the rest.
        let playable = app.playable_videos();
        let mut running = self.slots.iter().filter(|s| s.job.is_some()).count();
        if running < MAX_JOBS {
            let mut todo: Vec<(u8, usize)> = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.job.is_none() && (!s.probed || (!s.posted && s.tex.is_none())))
                .map(|(i, s)| {
                    let on = shown.get(&s.hash).is_some_and(|x| x.1);
                    (if playable.contains(&s.hash) { 0 } else if on { 1 } else { 2 }, i)
                })
                .collect();
            todo.sort();
            for (_, i) in todo {
                if running >= MAX_JOBS {
                    break;
                }
                let s = &mut self.slots[i];
                let probe_it = !s.probed;
                let shown_px = shown.get(&s.hash).map_or(0.0, |x| x.0);
                let poster_it = !s.posted && s.tex.is_none();
                let (pw, ph) = playback_size_max(s.width, s.height, decode_width(shown_px).min(POSTER_MAX_W));
                let (path, hash, cache, known_d) = (s.path.clone(), s.hash.clone(), s.cache.clone(), s.duration);
                let (tx, rx) = mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let probed = probe_it.then(|| probe(&path).map_or((0.0, 30.0), |p| (p.probe.duration_ms as f64 / 1000.0, p.fps.unwrap_or(30.0) as f64)));
                    let d = probed.map_or(known_d, |p| p.0);
                    let frame = if poster_it { cached_poster(cache.as_deref(), &hash, &path, pw, ph, (d / 3.0).min(1.0)).map(|px| (px, pw, ph)) } else { None };
                    let _ = tx.send((probed, frame));
                });
                s.posted |= poster_it;
                s.job = Some(rx);
                running += 1;
            }
        }
        for slot in &mut self.slots {
            let Some(rx) = &slot.job else { continue };
            let (probed, frame) = match rx.try_recv() {
                Ok(r) => r,
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => (None, None),
            };
            slot.job = None;
            if let Some((d, f)) = probed {
                (slot.duration, slot.fps, slot.probed) = (d, f, true);
                slot.saved.duration = d;
            }
            slot.probed = true;
            if let Some((frame, pw, ph)) = frame {
                app.video_frame_seen(&slot.hash, &frame, pw, ph);
                if slot.tex.is_none() {
                    match renderer.create_video_texture(pw, ph) {
                        Ok(tex) => {
                            renderer.update_image(tex, &frame, pw, ph);
                            app.attach_texture(slot.hash.clone(), tex);
                            slot.tex = Some(tex);
                            slot.tex_size = (pw, ph);
                        }
                        Err(e) => eprintln!("wall: video texture failed: {e}"),
                    }
                }
            }
            app.needs_redraw = true;
        }

        for slot in &mut self.slots {
            let want = playable.contains(&slot.hash) && slot.probed;
            let max_w = decode_width(shown.get(&slot.hash).map_or(0.0, |x| x.0));
            // Zoomed in past what the player decodes: restart it larger, at
            // the same time. (Zooming out keeps the sharper frames.)
            if let Some(p) = &slot.player {
                if want && p.width < playback_size_max(slot.width, slot.height, max_w).0 {
                    slot.saved = p.status();
                    slot.player = None;
                }
            }
            match (&slot.player, want) {
                (None, true) => {
                    if let Some(mut p) = Player::start(slot.path.clone(), slot.width, slot.height, slot.duration, slot.fps, max_w) {
                        if slot.saved.time > 0.0 {
                            p.command(wall_app::VideoCommand::Seek(slot.saved.time));
                        }
                        p.command(wall_app::VideoCommand::Speed(slot.saved.speed));
                        p.command(wall_app::VideoCommand::Range(app.video_range(&slot.hash)));
                        p.command(wall_app::VideoCommand::PingPong(app.video_pingpong(&slot.hash)));
                        p.command(wall_app::VideoCommand::Onion(app.video_onion(&slot.hash)));
                        if !slot.saved.playing {
                            p.command(wall_app::VideoCommand::Pause);
                        }
                        slot.player = Some(p);
                    }
                }
                (Some(p), false) => {
                    slot.saved = p.status();
                    slot.player = None;
                    // Off screen it needs no more than a still frame.
                    if let Some(tex) = slot.tex.filter(|_| slot.tex_size.0 > POSTER_MAX_W) {
                        let (w, h) = slot.tex_size;
                        renderer.shrink_image(tex, w, h, POSTER_MAX_W);
                        slot.tex_size = (POSTER_MAX_W, ((h as u64 * POSTER_MAX_W as u64 / w as u64) as u32).max(1));
                    }
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
                    wall_app::VideoCommand::Step(_) | wall_app::VideoCommand::Range(_) | wall_app::VideoCommand::PingPong(_) | wall_app::VideoCommand::Onion(_) => {}
                },
            }
        }

        for slot in &mut self.slots {
            let Some(player) = &mut slot.player else {
                app.set_video_status(slot.hash.clone(), slot.saved);
                continue;
            };
            if let Some(frame) = player.tick() {
                // A new texture when the player's size differs (first frame,
                // or the player restarted at another size).
                if slot.tex.is_some_and(|_| slot.tex_size != (player.width, player.height)) {
                    if let Some(old) = slot.tex.take() {
                        renderer.delete_image(old);
                    }
                }
                if slot.tex.is_none() {
                    match renderer.create_video_texture(player.width, player.height) {
                        Ok(tex) => {
                            app.attach_texture(slot.hash.clone(), tex);
                            slot.tex = Some(tex);
                            slot.tex_size = (player.width, player.height);
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
