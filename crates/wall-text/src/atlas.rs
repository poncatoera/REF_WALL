//! Packs glyph curves and band lists into two flat arrays that the GPU reads
//! as textures (Slug method: no bitmaps, every glyph drawn from its curves).
//!
//! `curves` (RGBA32F, 2 texels per curve):  [p0.x p0.y p1.x p1.y] [p2.x p2.y 0 0]
//! `bands`  (R32F, 1 value per texel), per glyph starting at `band_base`:
//!     H horizontal bands: (offset, count) pairs
//!     V vertical bands:   (offset, count) pairs
//!     then the curve-index lists the offsets point at (relative to band_base)
//! Curve indices are local to the glyph: texel = 2 * (curve_base + index).

use crate::outline::Quad;

/// Width of both data textures, in texels.
pub const TEXTURE_WIDTH: u32 = 4096;
/// Bands per axis for a glyph. Fewer curves per band = fewer loops per pixel.
pub const BANDS: usize = 8;

/// Where a glyph lives in the atlas and its bounding box in em units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphEntry {
    /// min x, min y, max x, max y (em units, y up). Empty glyphs have none.
    pub bounds: [f32; 4],
    pub curve_base: u32,
    pub band_base: u32,
    pub bands_h: u32,
    pub bands_v: u32,
}

/// Bands for one axis: for a horizontal ray (axis 0 sweeps along x) the bands
/// split the glyph by y, and each list is sorted by descending max x so the
/// shader can stop once curves are entirely left of the pixel.
pub fn build_bands(curves: &[Quad], bounds: [f32; 4], ray_axis: usize, n: usize) -> Vec<Vec<u32>> {
    let split = 1 - ray_axis; // bands are slices across the other axis
    let (lo, hi) = (bounds[split], bounds[split + 2]);
    let size = ((hi - lo) / n as f32).max(f32::EPSILON);
    let mut bands = vec![Vec::new(); n];
    for (i, c) in curves.iter().enumerate() {
        if c.is_flat(split) {
            continue;
        }
        let b0 = (((c.min(split) - lo) / size).floor() as isize).clamp(0, n as isize - 1) as usize;
        let b1 = (((c.max(split) - lo) / size).floor() as isize).clamp(0, n as isize - 1) as usize;
        for band in &mut bands[b0..=b1] {
            band.push(i as u32);
        }
    }
    for band in &mut bands {
        band.sort_by(|&a, &b| curves[b as usize].max(ray_axis).total_cmp(&curves[a as usize].max(ray_axis)));
    }
    bands
}

#[derive(Debug, Default, Clone)]
pub struct Atlas {
    pub curves: Vec<f32>,
    pub bands: Vec<f32>,
    /// Set when new glyphs were added since the GPU copy was made.
    pub dirty: bool,
}

impl Atlas {
    /// Adds a glyph and returns where it went. `curves` are in em units.
    pub fn add(&mut self, curves: &[Quad]) -> Option<GlyphEntry> {
        if curves.is_empty() {
            return None;
        }
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for c in curves {
            bounds[0] = bounds[0].min(c.min(0));
            bounds[1] = bounds[1].min(c.min(1));
            bounds[2] = bounds[2].max(c.max(0));
            bounds[3] = bounds[3].max(c.max(1));
        }

        let curve_base = (self.curves.len() / 8) as u32;
        for c in curves {
            self.curves.extend_from_slice(&[c.p0[0], c.p0[1], c.p1[0], c.p1[1], c.p2[0], c.p2[1], 0.0, 0.0]);
        }

        let h = build_bands(curves, bounds, 0, BANDS);
        let v = build_bands(curves, bounds, 1, BANDS);
        let band_base = self.bands.len() as u32;
        let mut offset = (2 * (h.len() + v.len())) as u32;
        let mut lists = Vec::new();
        for band in h.iter().chain(v.iter()) {
            self.bands.push(offset as f32);
            self.bands.push(band.len() as f32);
            offset += band.len() as u32;
            lists.extend(band.iter().map(|&i| i as f32));
        }
        self.bands.extend(lists);
        self.dirty = true;

        Some(GlyphEntry { bounds, curve_base, band_base, bands_h: h.len() as u32, bands_v: v.len() as u32 })
    }

    /// Texture heights needed for the current data (width = TEXTURE_WIDTH).
    pub fn texture_rows(&self) -> (u32, u32) {
        let curve_texels = (self.curves.len() / 4) as u32;
        let band_texels = self.bands.len() as u32;
        (curve_texels.div_ceil(TEXTURE_WIDTH).max(1), band_texels.div_ceil(TEXTURE_WIDTH).max(1))
    }

    /// Curve data padded to full texture rows (RGBA32F).
    pub fn curve_texture(&self) -> Vec<f32> {
        let rows = self.texture_rows().0 as usize;
        let mut v = self.curves.clone();
        v.resize(rows * TEXTURE_WIDTH as usize * 4, 0.0);
        v
    }

    /// Band data padded to full texture rows (R32F).
    pub fn band_texture(&self) -> Vec<f32> {
        let rows = self.texture_rows().1 as usize;
        let mut v = self.bands.clone();
        v.resize(rows * TEXTURE_WIDTH as usize, 0.0);
        v
    }
}
