//! Import rules shared by Wall desktop and Wall web.
//!
//! * Images wider than [`Limits::max_image_width`] (2048 px) are scaled down.
//!   Smaller images are stored byte-for-byte, never re-encoded.
//! * Videos over [`Limits::max_video_bytes`] (20 MB) are re-encoded to H.264 MP4.
//!   This crate only *plans* the encode; desktop runs it with ffmpeg and the web
//!   build runs it with WebCodecs.

pub mod autotag;
#[cfg(not(target_arch = "wasm32"))]
pub mod ffmpeg;
pub mod image_rules;
pub mod video_rules;

pub use image_rules::{prepare_image, PreparedImage};
pub use video_rules::{plan_video, H264Encoder, VideoPlan, VideoProbe};

/// Limits applied on import. `Default` gives the values agreed for v1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_image_width: u32,
    /// Decimal megabytes: 20 MB = 20,000,000 bytes.
    pub max_video_bytes: u64,
    /// Longest side cap for re-encoded video (1080p by default).
    pub max_video_height: u32,
    /// Videos whose keyframes are further apart than this are re-encoded even
    /// when small, so frame stepping and scrubbing stay fast.
    pub max_keyframe_gap_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_image_width: 2048, max_video_bytes: 20_000_000, max_video_height: 1080, max_keyframe_gap_ms: 2_000 }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("could not read image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("unsupported image type: {0}")]
    Unsupported(String),
    #[error("video has no duration; can't plan an encode")]
    NoDuration,
}
