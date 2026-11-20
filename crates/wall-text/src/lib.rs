//! Text for Wall, drawn the Slug way: each glyph is rendered straight from
//! its Bézier outline in the fragment shader, so it is sharp at every zoom
//! level without bitmap atlases. Our own implementation of the published
//! method (Lengyel, JCGT 2017), not the commercial Slug library.
//!
//! This crate is CPU-only (no GL): font parsing, curve and band packing,
//! simple layout, and a CPU reference of the shader's coverage maths.

pub mod atlas;
pub mod coverage;
pub mod outline;

use std::collections::HashMap;

pub use atlas::{Atlas, GlyphEntry, TEXTURE_WIDTH};
use outline::OutlineCollector;

/// The UI font bundled with Wall (Inter, SIL Open Font License).
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.otf");

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("could not parse font: {0}")]
    Parse(#[from] ttf_parser::FaceParsingError),
}

/// A glyph placed on a line. Positions are in em units from the line's origin
/// (left end of the baseline), y up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub entry: GlyphEntry,
    pub origin: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaidOutLine {
    pub glyphs: Vec<PlacedGlyph>,
    /// Advance width of the whole line in em units.
    pub width: f32,
    pub ascender: f32,
    pub descender: f32,
}

pub struct Font {
    face: ttf_parser::Face<'static>,
    cache: HashMap<u16, Option<GlyphEntry>>,
    pub atlas: Atlas,
}

impl Font {
    pub fn new(data: &'static [u8]) -> Result<Self, TextError> {
        Ok(Self { face: ttf_parser::Face::parse(data, 0)?, cache: HashMap::new(), atlas: Atlas::default() })
    }

    fn upem(&self) -> f32 {
        self.face.units_per_em() as f32
    }

    /// The glyph's packed entry, adding it to the atlas the first time.
    pub fn glyph(&mut self, id: ttf_parser::GlyphId) -> Option<GlyphEntry> {
        if let Some(e) = self.cache.get(&id.0) {
            return *e;
        }
        let mut collector = OutlineCollector::new(self.face.units_per_em());
        let entry = self.face.outline_glyph(id, &mut collector).and_then(|_| self.atlas.add(&collector.curves));
        self.cache.insert(id.0, entry);
        entry
    }

    pub fn glyph_id(&self, c: char) -> Option<ttf_parser::GlyphId> {
        self.face.glyph_index(c)
    }

    /// Lays out one line with advances and pair kerning (no complex shaping
    /// yet; that comes with rustybuzz).
    pub fn layout_line(&mut self, text: &str) -> LaidOutLine {
        let upem = self.upem();
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        let mut prev: Option<ttf_parser::GlyphId> = None;
        for ch in text.chars() {
            let Some(id) = self.glyph_id(ch).or_else(|| self.glyph_id('?')) else { continue };
            if let Some(p) = prev {
                x += self.kerning(p, id) / upem;
            }
            if let Some(entry) = self.glyph(id) {
                glyphs.push(PlacedGlyph { entry, origin: [x, 0.0] });
            }
            x += self.face.glyph_hor_advance(id).unwrap_or(0) as f32 / upem;
            prev = Some(id);
        }
        LaidOutLine {
            glyphs,
            width: x,
            ascender: self.face.ascender() as f32 / upem,
            descender: self.face.descender() as f32 / upem,
        }
    }

    fn kerning(&self, left: ttf_parser::GlyphId, right: ttf_parser::GlyphId) -> f32 {
        let Some(kern) = self.face.tables().kern else { return 0.0 };
        kern.subtables
            .into_iter()
            .filter(|s| s.horizontal && !s.variable)
            .find_map(|s| s.glyphs_kerning(left, right))
            .unwrap_or(0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coverage::coverage;

    fn font() -> Font {
        Font::new(INTER_REGULAR).unwrap()
    }

    fn cov(f: &mut Font, ch: char, p: [f32; 2], px_per_em: f32) -> f32 {
        let id = f.glyph_id(ch).unwrap();
        let g = f.glyph(id).unwrap();
        coverage(&f.atlas, &g, p, [px_per_em, px_per_em])
    }

    /// Renders a glyph to ASCII art with the CPU coverage function.
    fn ascii(f: &mut Font, ch: char, cols: usize, rows: usize) -> String {
        let id = f.glyph_id(ch).unwrap();
        let g = f.glyph(id).unwrap();
        let [x0, y0, x1, y1] = g.bounds;
        let ppe = [cols as f32 / (x1 - x0), rows as f32 / (y1 - y0)];
        let mut s = String::new();
        for r in 0..rows {
            for c in 0..cols {
                let p = [x0 + (c as f32 + 0.5) / ppe[0], y1 - (r as f32 + 0.5) / ppe[1]];
                let v = coverage(&f.atlas, &g, p, ppe);
                s.push(if v > 0.75 { '#' } else if v > 0.25 { '+' } else { '.' });
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn letter_o_has_a_hole_and_a_ring() {
        let mut f = font();
        let id = f.glyph_id('o').unwrap();
        let g = f.glyph(id).unwrap();
        let [x0, y0, x1, y1] = g.bounds;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert_eq!(cov(&mut f, 'o', [cx, cy], 64.0), 0.0, "centre of o is empty");
        let ring_x = x0 + 0.02; // just inside the left side of the bowl
        assert!(cov(&mut f, 'o', [ring_x, cy], 256.0) > 0.9, "left stroke of o is filled");
        assert_eq!(cov(&mut f, 'o', [x1 + 0.2, cy], 64.0), 0.0, "outside is empty");
    }

    #[test]
    fn ascii_render_of_capital_h_looks_like_h() {
        let mut f = font();
        let art = ascii(&mut f, 'H', 12, 12);
        let rows: Vec<&str> = art.lines().collect();
        // Both stems filled on every row.
        for r in &rows {
            assert!(r.starts_with('#') && r.ends_with('#'), "stems missing:\n{art}");
        }
        // Middle of the top row is empty; the crossbar row is full.
        assert_eq!(&rows[0][5..7], "..", "\n{art}");
        assert!(rows.iter().any(|r| r.chars().all(|c| c == '#')), "no crossbar:\n{art}");
    }

    #[test]
    fn edges_are_antialiased() {
        let mut f = font();
        let id = f.glyph_id('l').unwrap();
        let g = f.glyph(id).unwrap();
        let [x0, _, x1, _] = g.bounds;
        let y = 0.3;
        let ppe = 100.0;
        // Pixel centred exactly on the left edge is about half covered.
        let half = cov(&mut f, 'l', [x0, y], ppe);
        assert!((0.3..0.7).contains(&half), "edge coverage {half}");
        assert!(cov(&mut f, 'l', [(x0 + x1) / 2.0, y], ppe) > 0.99);
    }

    #[test]
    fn layout_advances_and_caches_glyphs() {
        let mut f = font();
        let line = f.layout_line("Wall wall");
        assert_eq!(line.glyphs.len(), 8, "space has no outline");
        assert!(line.width > 3.0 && line.width < 6.0, "width {}", line.width);
        assert!(line.glyphs.windows(2).all(|w| w[1].origin[0] > w[0].origin[0]));
        let curves_before = f.atlas.curves.len();
        f.layout_line("wall");
        assert_eq!(f.atlas.curves.len(), curves_before, "glyphs are reused, not re-added");
        assert!(line.ascender > 0.8 && line.descender < 0.0);
    }

    #[test]
    fn atlas_textures_are_padded_rows() {
        let mut f = font();
        f.layout_line("The quick brown fox jumps over the lazy dog 0123456789");
        let (cr, br) = f.atlas.texture_rows();
        assert_eq!(f.atlas.curve_texture().len(), (cr * TEXTURE_WIDTH * 4) as usize);
        assert_eq!(f.atlas.band_texture().len(), (br * TEXTURE_WIDTH) as usize);
        assert!(f.atlas.dirty);
    }
}
