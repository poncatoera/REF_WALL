use crate::{Limits, MediaError};

/// What the importer learned about a video (from ffprobe on desktop, from the
/// MP4 demuxer + WebCodecs on the web).
#[derive(Debug, Clone, PartialEq)]
pub struct VideoProbe {
    pub bytes: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// Lowercase codec name, e.g. `h264`, `hevc`, `vp9`, `av1`, `gif`.
    pub codec: String,
    /// Lowercase container, e.g. `mp4`, `mov`, `webm`, `gif`.
    pub container: String,
    pub has_audio: bool,
    /// Largest distance between keyframes, if known.
    pub max_keyframe_gap_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoPlan {
    /// Store the file as it is.
    Keep,
    Reencode(EncodeSettings),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncodeSettings {
    pub width: u32,
    pub height: u32,
    /// Target video bitrate in kbit/s, sized so the file lands under the cap.
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub keyframe_interval_ms: u64,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    TooLarge,
    TooTall,
    NotH264Mp4,
    SparseKeyframes,
}

const AUDIO_KBPS: u32 = 128;
const KEYFRAME_MS: u64 = 1_000;
/// Leave room for container overhead and encoder overshoot.
const SIZE_HEADROOM: f64 = 0.92;
/// Never ask the encoder for less than this; very long clips get scaled down instead.
const MIN_VIDEO_KBPS: u32 = 300;
const MAX_VIDEO_KBPS: u32 = 12_000;

/// Decides whether a video must be re-encoded, and with what settings.
pub fn plan_video(p: &VideoProbe, limits: &Limits) -> Result<VideoPlan, MediaError> {
    let mut reasons = Vec::new();
    if p.bytes > limits.max_video_bytes {
        reasons.push(Reason::TooLarge);
    }
    if p.height.min(p.width) > limits.max_video_height {
        reasons.push(Reason::TooTall);
    }
    if !(p.codec == "h264" && p.container == "mp4") {
        reasons.push(Reason::NotH264Mp4);
    }
    if p.max_keyframe_gap_ms.is_some_and(|g| g > limits.max_keyframe_gap_ms) {
        reasons.push(Reason::SparseKeyframes);
    }
    if reasons.is_empty() {
        return Ok(VideoPlan::Keep);
    }
    if p.duration_ms == 0 {
        return Err(MediaError::NoDuration);
    }

    let audio_kbps = if p.has_audio { AUDIO_KBPS } else { 0 };
    let seconds = p.duration_ms as f64 / 1000.0;
    let budget_kbps = (limits.max_video_bytes as f64 * 8.0 * SIZE_HEADROOM / 1000.0 / seconds) as u32;
    let mut video_kbps = budget_kbps.saturating_sub(audio_kbps).min(MAX_VIDEO_KBPS);

    // Short side capped at max_video_height (1080p for landscape and portrait).
    let mut scale = (limits.max_video_height as f64 / p.width.min(p.height) as f64).min(1.0);
    // If the budget is very low, drop resolution rather than starve the encoder.
    if video_kbps < MIN_VIDEO_KBPS * 2 {
        scale = scale.min(0.5);
    }
    video_kbps = video_kbps.max(MIN_VIDEO_KBPS);

    Ok(VideoPlan::Reencode(EncodeSettings {
        width: even((p.width as f64 * scale).round() as u32),
        height: even((p.height as f64 * scale).round() as u32),
        video_kbps,
        audio_kbps,
        keyframe_interval_ms: KEYFRAME_MS,
        reasons,
    }))
}

/// H.264 needs even dimensions.
fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

impl EncodeSettings {
    /// Arguments for the desktop's bundled ffmpeg. `fps` sets the keyframe interval.
    pub fn ffmpeg_args(&self, input: &str, output: &str, fps: f32) -> Vec<String> {
        let gop = ((fps as f64) * self.keyframe_interval_ms as f64 / 1000.0).round().max(1.0) as u32;
        let mut a: Vec<String> = [
            "-hide_banner", "-y", "-i", input,
            "-map", "0:v:0", "-map", "0:a:0?",
            "-c:v", "libx264", "-preset", "medium", "-pix_fmt", "yuv420p",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        a.extend([
            "-vf".into(), format!("scale={}:{}", self.width, self.height),
            "-b:v".into(), format!("{}k", self.video_kbps),
            "-maxrate".into(), format!("{}k", self.video_kbps * 3 / 2),
            "-bufsize".into(), format!("{}k", self.video_kbps * 2),
            "-g".into(), gop.to_string(),
            "-keyint_min".into(), gop.to_string(),
        ]);
        if self.audio_kbps > 0 {
            a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", self.audio_kbps)]);
        } else {
            a.push("-an".into());
        }
        a.extend(["-movflags".into(), "+faststart".into(), output.into()]);
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> VideoProbe {
        VideoProbe {
            bytes: 5_000_000,
            duration_ms: 10_000,
            width: 1920,
            height: 1080,
            codec: "h264".into(),
            container: "mp4".into(),
            has_audio: true,
            max_keyframe_gap_ms: Some(1_000),
        }
    }

    fn settings(p: &VideoProbe) -> EncodeSettings {
        match plan_video(p, &Limits::default()).unwrap() {
            VideoPlan::Reencode(s) => s,
            VideoPlan::Keep => panic!("expected re-encode"),
        }
    }

    #[test]
    fn small_h264_mp4_is_kept() {
        assert_eq!(plan_video(&probe(), &Limits::default()).unwrap(), VideoPlan::Keep);
    }

    #[test]
    fn large_file_gets_bitrate_that_fits_20mb() {
        let p = VideoProbe { bytes: 80_000_000, duration_ms: 60_000, ..probe() };
        let s = settings(&p);
        assert_eq!(s.reasons, vec![Reason::TooLarge]);
        let est_bytes = (s.video_kbps + s.audio_kbps) as f64 * 1000.0 / 8.0 * 60.0;
        assert!(est_bytes < 20_000_000.0, "estimated {est_bytes}");
        assert!(est_bytes > 15_000_000.0, "should use most of the budget, got {est_bytes}");
        assert_eq!((s.width, s.height), (1920, 1080));
    }

    #[test]
    fn uhd_is_scaled_to_1080p_and_portrait_too() {
        let s = settings(&VideoProbe { width: 3840, height: 2160, ..probe() });
        assert_eq!((s.width, s.height), (1920, 1080));
        assert!(s.reasons.contains(&Reason::TooTall));
        let s = settings(&VideoProbe { width: 2160, height: 3840, ..probe() });
        assert_eq!((s.width, s.height), (1080, 1920));
    }

    #[test]
    fn other_codecs_and_sparse_keyframes_are_reencoded() {
        assert_eq!(settings(&VideoProbe { codec: "vp9".into(), container: "webm".into(), ..probe() }).reasons, vec![Reason::NotH264Mp4]);
        assert_eq!(settings(&VideoProbe { codec: "gif".into(), container: "gif".into(), has_audio: false, ..probe() }).audio_kbps, 0);
        assert_eq!(settings(&VideoProbe { max_keyframe_gap_ms: Some(10_000), ..probe() }).reasons, vec![Reason::SparseKeyframes]);
    }

    #[test]
    fn very_long_clip_drops_resolution_instead_of_starving() {
        let s = settings(&VideoProbe { bytes: 900_000_000, duration_ms: 3_600_000, ..probe() });
        assert_eq!(s.video_kbps, MIN_VIDEO_KBPS);
        assert_eq!((s.width, s.height), (960, 540));
    }

    #[test]
    fn odd_sizes_become_even() {
        let s = settings(&VideoProbe { width: 1281, height: 721, codec: "hevc".into(), ..probe() });
        assert_eq!((s.width % 2, s.height % 2), (0, 0));
    }

    #[test]
    fn ffmpeg_args_have_keyframes_every_second() {
        let s = settings(&VideoProbe { codec: "hevc".into(), ..probe() });
        let a = s.ffmpeg_args("in.mov", "out.mp4", 30.0);
        let g = a.iter().position(|x| x == "-g").unwrap();
        assert_eq!(a[g + 1], "30");
        assert_eq!(a.last().unwrap(), "out.mp4");
        assert!(a.contains(&"aac".to_string()));
    }

    #[test]
    fn zero_duration_is_an_error_when_encoding_needed() {
        let p = VideoProbe { duration_ms: 0, codec: "vp9".into(), ..probe() };
        assert!(matches!(plan_video(&p, &Limits::default()), Err(MediaError::NoDuration)));
    }
}
