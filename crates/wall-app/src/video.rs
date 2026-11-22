//! Video player controls shared by desktop and web. The app decides what the
//! person asked for (play, pause, seek, step, speed) and queues commands;
//! the shell's player carries them out and reports back its status, which
//! the app draws as a scrub bar.

pub use wall_format::Marker;

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
    /// Loop between in and out points (seconds); None plays the whole clip.
    Range(Option<(f64, f64)>),
    /// Play the loop forwards then backwards, again and again.
    PingPong(bool),
}

/// "0:01.2": minutes, seconds and tenths, for in and out points.
pub fn format_time(s: f64) -> String {
    let t = (s.max(0.0) * 10.0).round() as u64;
    format!("{}:{:02}.{}", t / 600, (t / 10) % 60, t % 10)
}

/// New in/out points after pressing I (`set_in`) or O at `t` seconds.
/// Returns None when the range covers the whole clip.
pub fn mark(range: Option<(f64, f64)>, t: f64, duration: f64, set_in: bool) -> Option<(f64, f64)> {
    let (mut a, mut b) = range.unwrap_or((0.0, duration));
    if set_in {
        a = t;
        if b <= a {
            b = duration;
        }
    } else {
        b = t;
        if b <= a {
            a = 0.0;
        }
    }
    (a > 0.01 || b < duration - 0.01).then_some((a, b))
}

/// Markers after pressing M at `t` seconds: removes a marker within a
/// tenth of a second of `t`, otherwise adds one there. Kept sorted.
pub fn toggle_marker(markers: &[Marker], t: f64) -> Vec<Marker> {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    let mut out: Vec<Marker> = markers.iter().filter(|m| m.ms.abs_diff(ms) > 100).cloned().collect();
    if out.len() == markers.len() {
        out.push(Marker::at(ms));
        out.sort_by_key(|m| m.ms);
    }
    out
}

/// The marker shown at `t` seconds (within half a frame at 25 fps).
pub fn marker_at(markers: &[Marker], t: f64) -> Option<usize> {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    markers.iter().position(|m| m.ms.abs_diff(ms) <= 20)
}

/// The marker to jump to from `t` seconds: the next one after it
/// (`forward`) or the one before it, in seconds.
pub fn next_marker(markers: &[Marker], t: f64, forward: bool) -> Option<f64> {
    let ms = (t.max(0.0) * 1000.0).round() as i64;
    let mut it = markers.iter().map(|m| m.ms as i64);
    let found = if forward { it.find(|m| *m > ms + 30) } else { it.rev().find(|m| *m < ms - 30) };
    found.map(|m| m as f64 / 1000.0)
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
    fn in_and_out_points() {
        assert_eq!(mark(None, 2.0, 10.0, true), Some((2.0, 10.0)));
        assert_eq!(mark(Some((2.0, 10.0)), 6.0, 10.0, false), Some((2.0, 6.0)));
        // An in point after the out point drops the out point.
        assert_eq!(mark(Some((2.0, 6.0)), 8.0, 10.0, true), Some((8.0, 10.0)));
        // An out point before the in point drops the in point.
        assert_eq!(mark(Some((8.0, 10.0)), 3.0, 10.0, false), Some((0.0, 3.0)));
        assert_eq!(mark(Some((0.0, 3.0)), 10.0, 10.0, false), None, "whole clip");
        assert_eq!(format_time(61.25), "1:01.3");
    }

    #[test]
    fn markers_toggle_and_jump() {
        let m = toggle_marker(&[], 2.0);
        let m = toggle_marker(&m, 0.5);
        assert_eq!(m, vec![Marker::at(500), Marker::at(2000)]);
        assert_eq!(toggle_marker(&m, 2.05), vec![Marker::at(500)], "pressing M near one removes it");
        assert_eq!(marker_at(&m, 2.01), Some(1));
        assert_eq!(marker_at(&m, 1.0), None);
        assert_eq!(next_marker(&m, 1.0, true), Some(2.0));
        assert_eq!(next_marker(&m, 2.0, true), None, "already on the last one");
        assert_eq!(next_marker(&m, 2.0, false), Some(0.5));
        assert_eq!(next_marker(&m, 0.2, false), None);
    }

    #[test]
    fn scrub_maps_x_to_time() {
        assert_eq!(time_at(150.0, 100.0, 200.0, 8.0), 2.0);
        assert_eq!(time_at(0.0, 100.0, 200.0, 8.0), 0.0);
        assert_eq!(time_at(900.0, 100.0, 200.0, 8.0), 8.0);
    }
}
