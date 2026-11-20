//! CPU twin of the glyph fragment shader (`shaders/glyph.slang`). It reads the
//! same packed atlas arrays, so tests here check both the packing and the math.
//! Follows Lengyel, "GPU-Centered Font Rendering Directly from Glyph
//! Outlines" (JCGT 2017).

use crate::atlas::{Atlas, GlyphEntry};

fn saturate(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Which roots of a curve (shifted so the pixel is at the origin) cross the
/// ray, from the signs of the three control-point coordinates. Bit 0: root 1,
/// bit 1: root 2.
fn root_code(a: f32, b: f32, c: f32) -> u32 {
    let shift = (if a > 0.0 { 2 } else { 0 }) + (if b > 0.0 { 4 } else { 0 }) + (if c > 0.0 { 8 } else { 0 });
    (0x2E74u32 >> shift) & 3
}

/// Solves y(t) = 0 and returns x at both roots. Points are (x, y) with the
/// ray along +x.
fn solve(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]) -> [f32; 2] {
    let a = [p0[0] - 2.0 * p1[0] + p2[0], p0[1] - 2.0 * p1[1] + p2[1]];
    let b = [p0[0] - p1[0], p0[1] - p1[1]];
    let (t1, t2) = if a[1].abs() < 1.0 / 65536.0 {
        let t = p0[1] * 0.5 / b[1];
        (t, t)
    } else {
        let d = (b[1] * b[1] - a[1] * p0[1]).max(0.0).sqrt();
        ((b[1] - d) / a[1], (b[1] + d) / a[1])
    };
    [(a[0] * t1 - b[0] * 2.0) * t1 + p0[0], (a[0] * t2 - b[0] * 2.0) * t2 + p0[0]]
}

fn curve(atlas: &Atlas, g: &GlyphEntry, i: u32) -> [[f32; 2]; 3] {
    let o = ((g.curve_base + i) * 8) as usize;
    let c = &atlas.curves[o..o + 6];
    [[c[0], c[1]], [c[2], c[3]], [c[4], c[5]]]
}

fn band<'a>(atlas: &'a Atlas, g: &GlyphEntry, index: u32) -> impl Iterator<Item = u32> + 'a {
    let base = g.band_base as usize;
    let off = atlas.bands[base + 2 * index as usize] as usize;
    let count = atlas.bands[base + 2 * index as usize + 1] as usize;
    atlas.bands[base + off..base + off + count].iter().map(|&v| v as u32)
}

fn band_index(v: f32, lo: f32, hi: f32, n: u32) -> u32 {
    (((v - lo) / (hi - lo) * n as f32).floor() as i32).clamp(0, n as i32 - 1) as u32
}

/// Coverage (0..1) of the pixel centred at `p` (em units), where one pixel is
/// `1 / ppe` em wide and tall.
pub fn coverage(atlas: &Atlas, g: &GlyphEntry, p: [f32; 2], ppe: [f32; 2]) -> f32 {
    let [x0, y0, x1, y1] = g.bounds;

    // Horizontal ray (+x).
    let (mut xcov, mut xwgt) = (0.0f32, 0.0f32);
    for i in band(atlas, g, band_index(p[1], y0, y1, g.bands_h)) {
        let [a, b, c] = curve(atlas, g, i);
        let (a, b, c) = ([a[0] - p[0], a[1] - p[1]], [b[0] - p[0], b[1] - p[1]], [c[0] - p[0], c[1] - p[1]]);
        if a[0].max(b[0]).max(c[0]) * ppe[0] < -0.5 {
            break; // sorted by max x: every later curve is left of the pixel
        }
        let code = root_code(a[1], b[1], c[1]);
        if code != 0 {
            let r = solve(a, b, c);
            let r = [r[0] * ppe[0], r[1] * ppe[0]];
            if code & 1 != 0 {
                xcov += saturate(r[0] + 0.5);
                xwgt = xwgt.max(saturate(1.0 - r[0].abs() * 2.0));
            }
            if code > 1 {
                xcov -= saturate(r[1] + 0.5);
                xwgt = xwgt.max(saturate(1.0 - r[1].abs() * 2.0));
            }
        }
    }

    // Vertical ray (+y): same with axes swapped.
    let (mut ycov, mut ywgt) = (0.0f32, 0.0f32);
    for i in band(atlas, g, g.bands_h + band_index(p[0], x0, x1, g.bands_v)) {
        let [a, b, c] = curve(atlas, g, i);
        let (a, b, c) = ([a[1] - p[1], a[0] - p[0]], [b[1] - p[1], b[0] - p[0]], [c[1] - p[1], c[0] - p[0]]);
        if a[0].max(b[0]).max(c[0]) * ppe[1] < -0.5 {
            break;
        }
        let code = root_code(a[1], b[1], c[1]);
        if code != 0 {
            let r = solve(a, b, c);
            let r = [r[0] * ppe[1], r[1] * ppe[1]];
            if code & 1 != 0 {
                ycov -= saturate(r[0] + 0.5);
                ywgt = ywgt.max(saturate(1.0 - r[0].abs() * 2.0));
            }
            if code > 1 {
                ycov += saturate(r[1] + 0.5);
                ywgt = ywgt.max(saturate(1.0 - r[1].abs() * 2.0));
            }
        }
    }

    let blended = (xcov * xwgt + ycov * ywgt).abs() / (xwgt + ywgt).max(1.0 / 65536.0);
    saturate(blended.max(xcov.abs().min(ycov.abs())))
}
