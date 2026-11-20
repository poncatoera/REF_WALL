//! Video player controls shared by desktop and web. The app decides what the
//! person asked for (play, pause, seek, step, speed) and queues commands;
//! the shell's player carries them out and reports back its status, which
//! the app draws as a scrub bar.

/// Something the shell's player should do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoCommand {
    Play,
    Pause,
    /// Jump to a time in seconds.
    Seek(f64),
    /// Move by whole frames while paused (negative = back).
    Step(i32),
    /// Playback rate, 1.0 = normal.
    Speed(f32),
}

/// What a player reports each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoStatus {
    pub time: f64,
    pub duration: f64,
    pub playing: bool,
    pub speed: f32,
}

/// Speeds `[` and `]` step through.
pub const SPEEDS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 4.0];

/// The next speed up or down the ladder from `current`.
pub fn next_speed(current: f32, up: bool) -> f32 {
    let i = SPEEDS.iter().position(|s| (s - current).abs() < 1e-3).unwrap_or(3);
    let j = if up { (i + 1).min(SPEEDS.len() - 1) } else { i.saturating_sub(1) };
    SPEEDS[j]
}

/// Height of the scrub strip at the bottom of a video, in screen pixels.
pub const SCRUB_PX: f32 = 22.0;

/// Time for a board x inside a video's frame (clamped to the clip).
pub fn time_at(x: f32, frame_x: f32, frame_w: f32, duration: f64) -> f64 {
    (((x - frame_x) / frame_w.max(1e-3)) as f64).clamp(0.0, 1.0) * duration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_ladder_clamps() {
        assert_eq!(next_speed(1.0, true), 1.5);
        assert_eq!(next_speed(4.0, true), 4.0);
        assert_eq!(next_speed(0.1, false), 0.1);
        assert_eq!(next_speed(0.7, false), 0.5); // unknown speed starts from 1x
    }

    #[test]
    fn scrub_maps_x_to_time() {
        assert_eq!(time_at(150.0, 100.0, 200.0, 8.0), 2.0);
        assert_eq!(time_at(0.0, 100.0, 200.0, 8.0), 0.0);
        assert_eq!(time_at(900.0, 100.0, 200.0, 8.0), 8.0);
    }
}
