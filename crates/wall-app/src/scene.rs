//! Turns the board into GPU instances. Text layouts are cached per item and
//! stored relative to the item, so dragging only offsets them.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;
use wall_format::{ContentHash, ItemKind};
use wall_render::{BoxInstance, GlyphInstance, ImageId, ImageInstance, Renderer, ViewUniform};
use wall_text::Font;

use crate::doc::Document;

const ACCENT: [f32; 4] = [0.36, 0.62, 1.0, 1.0];
const NOTE_FILL: [f32; 4] = [0.13, 0.135, 0.15, 1.0];
const NOTE_STROKE: [f32; 4] = [0.24, 0.25, 0.28, 1.0];
const TEXT: [f32; 4] = [0.92, 0.92, 0.94, 1.0];

struct CachedText {
    key: (String, u32),
    glyphs: Vec<GlyphInstance>,
}

#[derive(Default)]
pub struct Scene {
    version: Option<u64>,
    groups: Vec<BoxInstance>,
    images: Vec<(ContentHash, ImageInstance)>,
    cards: Vec<BoxInstance>,
    glyphs: Vec<GlyphInstance>,
    text_cache: HashMap<Uuid, CachedText>,
}

impl Scene {
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Rebuilds instances if the document changed since the last call.
    pub fn update(&mut self, doc: &Document, font: &mut Font) {
        if self.version == Some(doc.version) {
            return;
        }
        self.version = Some(doc.version);
        self.groups.clear();
        self.images.clear();
        self.cards.clear();
        self.glyphs.clear();

        let mut items: Vec<_> = doc.board.live_items().collect();
        items.sort_by(|a, b| a.frame.z.total_cmp(&b.frame.z));
        let mut seen = std::collections::HashSet::new();
        for item in items {
            seen.insert(item.id);
            let f = item.frame;
            let rect = [f.x as f32, f.y as f32, f.w as f32, f.h as f32];
            match &item.kind {
                ItemKind::Group { name, color } => {
                    let c = parse_hex(color.as_deref()).unwrap_or([0.6, 0.6, 0.65]);
                    self.groups.push(BoxInstance {
                        rect,
                        fill: [c[0] * 0.06, c[1] * 0.06, c[2] * 0.06, 0.6],
                        stroke: [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5, 0.9],
                        shape: [10.0, 1.5],
                    });
                    self.text(font, item.id, name, 20, [12.0, 30.0], 20.0, [c[0], c[1], c[2], 1.0], rect, None);
                }
                ItemKind::Media { hash, .. } => {
                    self.images.push((hash.clone(), ImageInstance { rect, params: [1.0, 6.0, 0.0, 0.0] }));
                }
                ItemKind::Note { text, color } => {
                    let fill = parse_hex(color.as_deref()).map(|c| [c[0] * 0.25, c[1] * 0.25, c[2] * 0.25, 1.0]).unwrap_or(NOTE_FILL);
                    self.cards.push(BoxInstance { rect, fill, stroke: NOTE_STROKE, shape: [8.0, 1.0] });
                    self.text(font, item.id, text, 16, [14.0, 30.0], 16.0, TEXT, rect, Some(f.w as f32 - 28.0));
                }
                ItemKind::Label { text } => {
                    self.text(font, item.id, text, 96, [0.0, 96.0], 96.0, [1.0; 4], rect, None);
                }
                ItemKind::Todo { title, .. } => {
                    self.cards.push(BoxInstance { rect, fill: NOTE_FILL, stroke: NOTE_STROKE, shape: [8.0, 1.0] });
                    self.text(font, item.id, title, 16, [14.0, 30.0], 16.0, TEXT, rect, Some(f.w as f32 - 28.0));
                }
                ItemKind::Board { .. } => {
                    self.cards.push(BoxInstance { rect, fill: NOTE_FILL, stroke: ACCENT, shape: [8.0, 1.5] });
                }
                ItemKind::Arrow { .. } => {} // arrows come with the vector pass
            }
        }
        self.text_cache.retain(|id, _| seen.contains(id));
    }

    /// Appends an item's text, from cache when the text and size are unchanged.
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, font: &mut Font, id: Uuid, text: &str, size_key: u32, at: [f32; 2], size: f32, color: [f32; 4], rect: [f32; 4], wrap: Option<f32>) {
        let key = (text.to_owned(), size_key);
        let entry = self.text_cache.entry(id).or_insert_with(|| CachedText { key: (String::new(), 0), glyphs: Vec::new() });
        if entry.key != key {
            entry.glyphs.clear();
            let max_lines = ((rect[3] - at[1]) / (size * 1.35)).floor().max(1.0) as usize;
            let lines = match wrap {
                Some(w) => wrap_text(font, text, w / size, max_lines),
                None => vec![text.to_owned()],
            };
            for (n, line) in lines.iter().enumerate() {
                crate::scene::add_text(font, &mut entry.glyphs, line, [at[0], at[1] + n as f32 * size * 1.35], size, color);
            }
            entry.key = key;
        }
        self.glyphs.extend(entry.glyphs.iter().map(|g| {
            let mut g = *g;
            g.rect[0] += rect[0];
            g.rect[1] += rect[1];
            g
        }));
    }

    pub fn draw(&self, renderer: &mut Renderer, view: &ViewUniform, textures: &HashMap<&ContentHash, ImageId>) {
        renderer.draw_boxes(view, &self.groups);
        let images: Vec<(ImageId, ImageInstance)> =
            self.images.iter().filter_map(|(h, inst)| textures.get(h).map(|id| (*id, *inst))).collect();
        renderer.draw_images(view, &images);
        renderer.draw_boxes(view, &self.cards);
        renderer.draw_glyphs(view, &self.glyphs);
    }
}

/// Selection outlines and the marquee, sized in screen pixels.
pub fn overlays(doc: &Document, selection: &BTreeSet<Uuid>, marquee: Option<[f32; 4]>, zoom: f32) -> Vec<BoxInstance> {
    let px = 1.0 / zoom;
    let mut out: Vec<BoxInstance> = selection
        .iter()
        .filter_map(|id| doc.item(id))
        .map(|i| {
            let f = i.frame;
            BoxInstance {
                rect: [f.x as f32 - 3.0 * px, f.y as f32 - 3.0 * px, f.w as f32 + 6.0 * px, f.h as f32 + 6.0 * px],
                fill: [0.0; 4],
                stroke: ACCENT,
                shape: [6.0 * px, 2.0 * px],
            }
        })
        .collect();
    if let Some(m) = marquee {
        out.push(BoxInstance {
            rect: [m[0], m[1], m[2] - m[0], m[3] - m[1]],
            fill: [ACCENT[0] * 0.12, ACCENT[1] * 0.12, ACCENT[2] * 0.12, 0.12],
            stroke: [ACCENT[0] * 0.8, ACCENT[1] * 0.8, ACCENT[2] * 0.8, 0.8],
            shape: [0.0, px],
        });
    }
    out
}

/// Greedy word wrap to `max_em` (width in em units).
pub fn wrap_text(font: &mut Font, text: &str, max_em: f32, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() { word.to_owned() } else { format!("{current} {word}") };
        if font.layout_line(&candidate).width <= max_em || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        }
        if lines.len() == max_lines {
            return lines;
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    lines
}

/// Lays out `text` with its baseline starting at `baseline` (board units,
/// y down) at `size` board units per em, and appends one quad per glyph.
pub fn add_text(font: &mut Font, out: &mut Vec<GlyphInstance>, text: &str, baseline: [f32; 2], size: f32, rgba: [f32; 4]) {
    let line = font.layout_line(text);
    for g in &line.glyphs {
        let [x0, y0, x1, y1] = g.entry.bounds;
        out.push(GlyphInstance {
            rect: [
                baseline[0] + (g.origin[0] + x0) * size,
                baseline[1] - (g.origin[1] + y1) * size,
                (x1 - x0) * size,
                (y1 - y0) * size,
            ],
            em: g.entry.bounds,
            color: rgba,
            data: [g.entry.curve_base as f32, g.entry.band_base as f32, g.entry.bands_h as f32, g.entry.bands_v as f32],
        });
    }
}

fn parse_hex(s: Option<&str>) -> Option<[f32; 3]> {
    let s = s?.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0])
}

pub fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_quads_sit_on_the_baseline() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let mut out = Vec::new();
        add_text(&mut font, &mut out, "Hx", [100.0, 200.0], 50.0, [1.0; 4]);
        assert_eq!(out.len(), 2);
        let h = out[0].rect;
        assert!((h[1] + h[3] - 200.0).abs() < 0.5, "{h:?}");
        assert!(out[1].rect[0] > h[0] + h[2]);
    }

    #[test]
    fn wrap_breaks_on_words_and_caps_lines() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let lines = wrap_text(&mut font, "Kinetic type on a grid with long tail easing", 8.0, 10);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(lines.iter().all(|l| font.layout_line(l).width <= 8.0 || !l.contains(' ')));
        assert_eq!(lines.join(" "), "Kinetic type on a grid with long tail easing");
        assert_eq!(wrap_text(&mut font, "a b c d e f g h", 0.5, 3).len(), 3);
    }

    #[test]
    fn hex_colours_parse() {
        assert_eq!(parse_hex(Some("#ff0000")), Some([1.0, 0.0, 0.0]));
        assert_eq!(parse_hex(Some("red")), None);
    }
}
