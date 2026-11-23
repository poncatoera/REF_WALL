//! Drawing tools: pen, pencil, brush, marker and eraser.
//!
//! Each brush has a profile (after the old Wall's sketch tools): how much
//! pressure thins it, how long its ends taper, and how see-through it is.
//! A mouse has no pressure, so drawing speed stands in for it: faster is
//! thinner, as with ink.

use crate::scene;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Brush {
    Pen,
    Pencil,
    Brush,
    Marker,
    Eraser,
}

pub const ALL: [Brush; 5] = [Brush::Pen, Brush::Pencil, Brush::Brush, Brush::Marker, Brush::Eraser];

/// The drawing colours on offer (the first is the default).
pub const COLORS: [&str; 8] = ["#ff6b3d", "#1b1b1d", "#f4f1ea", "#ffd23d", "#6fdc7a", "#3dc8ff", "#8d7bff", "#ff5fa8"];

/// How a brush behaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    /// How much low pressure thins the line (0 = not at all).
    pub thin: f32,
    /// Taper length at each end, in line widths.
    pub taper: f32,
    /// 1 = solid; below 1 the line is see-through (blended once per line).
    pub alpha: f32,
    /// Starting size in screen pixels.
    pub size_px: f32,
}

impl Brush {
    pub fn name(self) -> &'static str {
        match self {
            Brush::Pen => "pen",
            Brush::Pencil => "pencil",
            Brush::Brush => "brush",
            Brush::Marker => "marker",
            Brush::Eraser => "eraser",
        }
    }

    /// A stroke's stored brush name; unknown or missing is the pen.
    pub fn from_name(s: Option<&str>) -> Self {
        match s {
            Some("pencil") => Brush::Pencil,
            Some("brush") => Brush::Brush,
            Some("marker") => Brush::Marker,
            Some("eraser") => Brush::Eraser,
            _ => Brush::Pen,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Brush::Pen => "Pen",
            Brush::Pencil => "Pencil",
            Brush::Brush => "Brush",
            Brush::Marker => "Marker",
            Brush::Eraser => "Eraser",
        }
    }

    pub fn profile(self) -> Profile {
        match self {
            Brush::Pen => Profile { thin: 0.6, taper: 2.5, alpha: 1.0, size_px: 4.0 },
            Brush::Pencil => Profile { thin: 0.35, taper: 1.0, alpha: 0.85, size_px: 3.0 },
            Brush::Brush => Profile { thin: 0.9, taper: 4.0, alpha: 1.0, size_px: 18.0 },
            Brush::Marker => Profile { thin: 0.0, taper: 0.0, alpha: 0.5, size_px: 22.0 },
            Brush::Eraser => Profile { thin: 0.0, taper: 0.0, alpha: 1.0, size_px: 30.0 },
        }
    }

    fn index(self) -> usize {
        ALL.iter().position(|b| *b == self).unwrap_or(0)
    }
}

/// The drawing tool's settings, remembered between runs.
#[derive(Debug, Clone, PartialEq)]
pub struct PenState {
    pub brush: Brush,
    /// The brush used before switching to the eraser (E switches back).
    pub last: Brush,
    /// Size of each brush in screen pixels, in [`ALL`] order.
    pub sizes: [f32; 5],
    pub color: String,
}

impl Default for PenState {
    fn default() -> Self {
        PenState { brush: Brush::Pen, last: Brush::Pen, sizes: ALL.map(|b| b.profile().size_px), color: COLORS[0].into() }
    }
}

impl PenState {
    pub fn size(&self) -> f32 {
        self.sizes[self.brush.index()]
    }

    /// [ and ]: a fifth smaller or a quarter bigger, 1 to 160 px.
    pub fn grow(&mut self, bigger: bool) {
        let s = &mut self.sizes[self.brush.index()];
        *s = (*s * if bigger { 1.25 } else { 0.8 }).clamp(1.0, 160.0);
    }

    pub fn pick(&mut self, b: Brush) {
        if b == Brush::Eraser && self.brush != Brush::Eraser {
            self.last = self.brush;
        }
        self.brush = b;
    }

    /// E: the eraser, or back to the brush before it.
    pub fn toggle_eraser(&mut self) {
        if self.brush == Brush::Eraser {
            self.brush = self.last;
        } else {
            self.pick(Brush::Eraser);
        }
    }

    /// Picking a colour also leaves the eraser.
    pub fn set_color(&mut self, c: &str) {
        self.color = c.to_owned();
        if self.brush == Brush::Eraser {
            self.brush = self.last;
        }
    }

    /// "brush;color;s1,s2,s3,s4,s5" for settings.
    pub fn to_setting(&self) -> String {
        let sizes: Vec<String> = self.sizes.iter().map(|s| format!("{s:.1}")).collect();
        format!("{};{};{}", self.brush.name(), self.color, sizes.join(","))
    }

    pub fn from_setting(s: &str) -> Option<Self> {
        let mut it = s.split(';');
        let brush = Brush::from_name(it.next());
        let color = it.next().filter(|c| c.starts_with('#') && c.len() == 7)?.to_owned();
        let sizes: Vec<f32> = it.next()?.split(',').filter_map(|v| v.parse().ok()).collect();
        let sizes: [f32; 5] = sizes.try_into().ok()?;
        let brush = if brush == Brush::Eraser { Brush::Pen } else { brush };
        Some(PenState { brush, last: brush, sizes: sizes.map(|s| s.clamp(1.0, 160.0)), color })
    }
}

/// Pressure from pointer speed (pixels per millisecond, smoothed): slow is
/// 1, fast thins down to 0.2.
pub fn speed_pressure(velocity: f32) -> f32 {
    (1.0 - (velocity / 3.0).min(1.0) * 0.8).max(0.2)
}

/// Points about `step` apart along a line, following the same smooth curve
/// it is drawn as (quadratics through the midpoints), with a value carried
/// along for each. Pointer moves can be far apart on a fast or a straight
/// line; tapering and rubbing out need points in between.
pub fn resample(points: &[[f32; 2]], values: &[f32], step: f32) -> (Vec<[f32; 2]>, Vec<f32>) {
    let value = |i: usize| values.get(i).copied().unwrap_or(1.0);
    let all: Vec<[f32; 3]> = points.iter().enumerate().map(|(i, p)| [p[0], p[1], value(i)]).collect();
    let mid = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
    let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let n = all.len();
    if n < 2 || step <= 0.0 {
        return (points.to_vec(), (0..n).map(value).collect());
    }
    // Start, control and end of each piece.
    let mut pieces = Vec::with_capacity(n);
    if n == 2 {
        pieces.push((all[0], mid(all[0], all[1]), all[1]));
    } else {
        let (m0, m1) = (mid(all[0], all[1]), mid(all[n - 2], all[n - 1]));
        pieces.push((all[0], mid(all[0], m0), m0));
        for i in 1..n - 1 {
            pieces.push((mid(all[i - 1], all[i]), all[i], mid(all[i], all[i + 1])));
        }
        pieces.push((m1, mid(m1, all[n - 1]), all[n - 1]));
    }
    let mut out = vec![all[0]];
    // Distance left before the next point is due.
    let mut due = step;
    for (a, c, b) in pieces {
        let steps = (((dist(a, c) + dist(c, b)) / step * 4.0).ceil() as usize).clamp(1, 4096);
        let mut prev = a;
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let u = 1.0 - t;
            let q: [f32; 3] = std::array::from_fn(|j| u * u * a[j] + 2.0 * u * t * c[j] + t * t * b[j]);
            due -= dist(prev, q);
            prev = q;
            if due <= 0.0 {
                out.push(q);
                due = step;
            }
        }
    }
    // End exactly where the line ends.
    if out.len() > 1 && dist(out[out.len() - 1], all[n - 1]) < step * 0.5 {
        out.pop();
    }
    out.push(all[n - 1]);
    (out.iter().map(|p| [p[0], p[1]]).collect(), out.iter().map(|p| p[2]).collect())
}

/// How far apart a brush's points are kept, in board units.
pub fn point_step(width: f32, zoom: f32) -> f32 {
    (width * 0.3).max(1.5 / zoom.max(1e-6))
}

/// Width factor (0–1 of the full width) at each point: pressure thins it
/// by the brush's `thin`, and both ends taper over `taper` widths (at most
/// 30% of the line's length each).
pub fn width_factors(points: &[[f32; 2]], pressures: &[f32], width: f32, brush: Brush) -> Vec<f32> {
    let p = brush.profile();
    let n = points.len();
    let mut along = Vec::with_capacity(n);
    let mut total = 0.0;
    for (i, q) in points.iter().enumerate() {
        if i > 0 {
            let a = points[i - 1];
            total += ((q[0] - a[0]).powi(2) + (q[1] - a[1]).powi(2)).sqrt();
        }
        along.push(total);
    }
    let taper = (width * p.taper).min(total * 0.3);
    (0..n)
        .map(|i| {
            let pressure = pressures.get(i).copied().unwrap_or(1.0).clamp(0.05, 1.0);
            let mut f = 1.0 - p.thin + p.thin * pressure;
            if taper > 0.0 {
                let d = along[i].min(total - along[i]);
                if d < taper {
                    f *= 0.12 + 0.88 * (std::f32::consts::FRAC_PI_2 * d / taper).sin();
                }
            }
            f
        })
        .collect()
}

/// The parts of a line left after rubbing out everything within `r` of
/// `at`: runs of two or more points, each with its width factors. None if
/// nothing was within reach.
pub fn erase(points: &[[f32; 2]], factors: &[f32], at: [f32; 2], r: f32) -> Option<Vec<(Vec<[f32; 2]>, Vec<f32>)>> {
    let near = |i: usize| {
        let q = points[i];
        ((q[0] - at[0]).powi(2) + (q[1] - at[1]).powi(2)).sqrt() <= r
    };
    // A long segment can pass through the eraser between two far points.
    let seg_near = |i: usize| i + 1 < points.len() && scene::segment_distance(at, points[i], points[i + 1]) <= r;
    if !(0..points.len()).any(|i| near(i) || seg_near(i)) {
        return None;
    }
    let mut out = Vec::new();
    let (mut run, mut fs) = (Vec::new(), Vec::new());
    for i in 0..points.len() {
        if near(i) {
            if run.len() >= 2 {
                out.push((std::mem::take(&mut run), std::mem::take(&mut fs)));
            }
            run.clear();
            fs.clear();
            continue;
        }
        run.push(points[i]);
        fs.push(factors.get(i).copied().unwrap_or(1.0));
        if seg_near(i) && !near(i + 1) {
            // Cut between this point and the next.
            if run.len() >= 2 {
                out.push((std::mem::take(&mut run), std::mem::take(&mut fs)));
            }
            run.clear();
            fs.clear();
        }
    }
    if run.len() >= 2 {
        out.push((run, fs));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_taper_and_speed_thins_the_brush_but_not_the_marker() {
        let line: Vec<[f32; 2]> = (0..50).map(|i| [i as f32 * 4.0, 0.0]).collect();
        let slow = vec![1.0; 50];
        let f = width_factors(&line, &slow, 10.0, Brush::Pen);
        assert!(f[0] < 0.2 && f[49] < 0.2, "tapered ends: {} {}", f[0], f[49]);
        assert!((f[25] - 1.0).abs() < 1e-5, "full width in the middle");
        let fast = vec![speed_pressure(5.0); 50];
        assert!(width_factors(&line, &fast, 10.0, Brush::Brush)[25] < 0.35);
        assert_eq!(width_factors(&line, &fast, 10.0, Brush::Marker)[25], 1.0);
    }

    #[test]
    fn rubbing_out_the_middle_leaves_two_lines() {
        let line: Vec<[f32; 2]> = (0..11).map(|i| [i as f32 * 10.0, 0.0]).collect();
        let f = vec![1.0; 11];
        let parts = erase(&line, &f, [50.0, 0.0], 12.0).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0.last(), Some(&[30.0, 0.0]));
        assert_eq!(parts[1].0[0], [70.0, 0.0]);
        assert!(erase(&line, &f, [50.0, 40.0], 12.0).is_none());
        // Everything rubbed out: nothing left.
        assert_eq!(erase(&line, &f, [50.0, 0.0], 200.0).unwrap().len(), 0);
    }

    #[test]
    fn resampling_fills_in_sparse_lines_and_keeps_their_ends() {
        // A straight line of two points can taper and still be full in the middle.
        let (p, v) = resample(&[[0.0, 0.0], [200.0, 0.0]], &[1.0, 1.0], 3.0);
        assert!(p.len() > 50 && p.len() == v.len(), "{}", p.len());
        assert_eq!((p[0], *p.last().unwrap()), ([0.0, 0.0], [200.0, 0.0]));
        assert!(p.windows(2).all(|w| w[1][0] > w[0][0] && w[1][0] - w[0][0] < 6.0 && w[1][1] == 0.0));
        let f = width_factors(&p, &v, 10.0, Brush::Pen);
        assert!(f[0] < 0.2 && (f[p.len() / 2] - 1.0).abs() < 1e-5);
        // Values are carried along; a dot stays a dot.
        let (_, v) = resample(&[[0.0, 0.0], [100.0, 0.0]], &[1.0, 0.0], 1.0);
        assert!((v[v.len() / 2] - 0.5).abs() < 0.05);
        assert_eq!(resample(&[[3.0, 4.0]], &[0.7], 1.0), (vec![[3.0, 4.0]], vec![0.7]));
    }

    #[test]
    fn settings_round_trip_and_eraser_switches_back() {
        let mut p = PenState::default();
        p.pick(Brush::Marker);
        p.grow(true);
        p.toggle_eraser();
        assert_eq!(p.brush, Brush::Eraser);
        p.toggle_eraser();
        assert_eq!(p.brush, Brush::Marker);
        let back = PenState::from_setting(&p.to_setting()).unwrap();
        assert_eq!((back.brush, back.size()), (Brush::Marker, 27.5));
        assert!(PenState::from_setting("nonsense").is_none());
    }
}
