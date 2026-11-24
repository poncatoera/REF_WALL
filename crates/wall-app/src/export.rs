//! File > Export board as PDF: the whole board on the first page, with its
//! open comments as numbered pins, and the comments written out after it,
//! one numbered thread after another, on A4 pages.
//!
//! Every page is a picture drawn by Wall's own renderer (so any text and
//! any language looks as it does on the board), put in the PDF as a JPEG.
//! The file is written here by hand: a PDF of pictures is a few objects.

use wall_format::{CommentMessage, ItemKind};
use wall_render::{BoxInstance, GlyphInstance, Renderer, ViewUniform};
use wall_text::Font;

use crate::panel::{self, PanelBox, TextRun};
use crate::{history, scene, App, AppError};

/// A pin on the exported board, in picture pixels: at least this, and a
/// set share of the picture's longer side, so pins read on any board.
const PIN_PX: f32 = 30.0;
const PIN_SHARE: f32 = 0.03;
/// The pins' blue.
const PIN_FILL: [f32; 4] = [0.16, 0.42, 0.95, 1.0];
/// The comment pages: A4 at 150 dots an inch.
pub const PAGE_PX: [u32; 2] = [1240, 1754];
const PAGE_PT: [f32; 2] = [595.0, 842.0];
/// The board page's longer side, in points (A3's).
const BOARD_PT: f32 = 1190.0;
const MARGIN: f32 = 100.0;
const INK: [f32; 4] = [0.10, 0.10, 0.12, 1.0];
const DIM: [f32; 4] = [0.42, 0.42, 0.46, 1.0];
const RULE: [f32; 4] = [0.86, 0.86, 0.88, 1.0];
const PAPER: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const JPEG_QUALITY: u8 = 88;

/// Numbered pins drawn over the board, in board units (`zoom`: picture
/// pixels per board unit, `size` the picture's): a round head whose
/// corner points at the spot.
pub fn pin_marks(font: &mut Font, pins: &[([f32; 2], String)], zoom: f32, size: [u32; 2]) -> (Vec<BoxInstance>, Vec<GlyphInstance>) {
    let s = PIN_PX.max(size[0].max(size[1]) as f32 * PIN_SHARE) / zoom.max(1e-6);
    let mut boxes = Vec::new();
    let mut glyphs = Vec::new();
    for (p, label) in pins {
        let r = [p[0], p[1] - s, s, s];
        boxes.push(BoxInstance { rect: [p[0], p[1] - s * 0.4, s * 0.4, s * 0.4], fill: [1.0; 4], stroke: [0.0; 4], shape: [s * 0.05, 0.0] });
        boxes.push(BoxInstance { rect: r, fill: PIN_FILL, stroke: [1.0; 4], shape: [s / 2.0, s * 0.08] });
        let size = s * if label.chars().count() > 1 { 0.42 } else { 0.5 };
        let w = font.layout_line(label).width * size;
        scene::add_text(font, &mut glyphs, label, [r[0] + (s - w) / 2.0, r[1] + s / 2.0 + size * 0.36], size, [1.0; 4]);
    }
    (boxes, glyphs)
}

/// A comment as the PDF lists it.
pub struct Listed<'a> {
    pub number: usize,
    pub messages: &'a [CommentMessage],
}

/// What goes on one comment page, in page pixels.
#[derive(Default)]
pub struct Page {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
}

/// The comment pages: a heading, then each thread under its number. A
/// thread that doesn't fit at the foot of a page goes on to the next.
pub fn comment_pages(width: &mut dyn FnMut(&str, f32) -> f32, title: &str, subtitle: &str, list: &[Listed], offset_min: i32) -> Vec<Page> {
    let [pw, ph] = [PAGE_PX[0] as f32, PAGE_PX[1] as f32];
    let inner = pw - 2.0 * MARGIN;
    let text_x = MARGIN + 64.0;
    let text_w = pw - MARGIN - text_x;
    let bottom = ph - MARGIN;
    let mut pages = vec![Page::default()];
    let mut y = MARGIN + 40.0;
    {
        let p = &mut pages[0];
        p.texts.push(TextRun { text: panel::fit_with(width, title, 40.0, inner), at: [MARGIN, y], size: 40.0, color: INK });
        y += 40.0;
        p.texts.push(TextRun { text: subtitle.into(), at: [MARGIN, y], size: 22.0, color: DIM });
        y += 30.0;
    }
    for c in list {
        // Lay the thread out first, to see if it fits.
        let mut runs: Vec<(TextRun, f32)> = Vec::new();
        let mut h = 0.0;
        for (k, m) in c.messages.iter().enumerate() {
            if k > 0 {
                h += 14.0;
            }
            h += 30.0;
            let when = history::when(m.at_ms, 0, offset_min);
            let who = panel::fit_with(width, &m.by, 22.0, text_w * 0.6);
            let who_w = width(&who, 22.0);
            runs.push((TextRun { text: who, at: [text_x, 0.0], size: 22.0, color: INK }, h));
            runs.push((TextRun { text: format!("·  {when}"), at: [text_x + who_w + 10.0, 0.0], size: 20.0, color: DIM }, h));
            for line in m.text.split('\n').flat_map(|l| if l.trim().is_empty() { vec![String::new()] } else { panel::wrap_with(width, l, 24.0, text_w) }) {
                h += 34.0;
                runs.push((TextRun { text: line, at: [text_x, 0.0], size: 24.0, color: INK }, h));
            }
        }
        let block = 28.0 + h + 10.0;
        if y + block > bottom && !pages.last().unwrap().texts.is_empty() && y > MARGIN + 1.0 {
            pages.push(Page::default());
            y = MARGIN;
        }
        let p = pages.last_mut().unwrap();
        y += 28.0;
        p.boxes.push(PanelBox { rect: [MARGIN, y - 28.0 + 8.0, inner, 1.5], fill: RULE, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
        let num = c.number.to_string();
        let d = 44.0;
        let circle = [MARGIN, y + 6.0, d, d];
        p.boxes.push(PanelBox { rect: circle, fill: PIN_FILL, stroke: [0.0; 4], radius: d / 2.0, stroke_w: 0.0 });
        let size = if num.len() > 1 { 19.0 } else { 22.0 };
        let nw = width(&num, size);
        p.texts.push(TextRun { text: num, at: [circle[0] + (d - nw) / 2.0, circle[1] + d / 2.0 + size * 0.36], size, color: [1.0; 4] });
        for (mut run, ry) in runs {
            if y + ry > bottom + 20.0 {
                // A thread longer than a page: on to the next one.
                pages.push(Page::default());
                y = MARGIN - ry + 34.0;
            }
            run.at[1] = y + ry;
            pages.last_mut().unwrap().texts.push(run);
        }
        y += h + 10.0;
    }
    pages
}

/// One page of the PDF: a JPEG and the page's size in points.
pub struct PdfPage {
    pub jpeg: Vec<u8>,
    pub px: [u32; 2],
    pub pt: [f32; 2],
}

/// A PDF string: plain when it's printable ASCII, else UTF-16 in hex.
fn pdf_text(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for u in s.encode_utf16() {
            out.push_str(&format!("{u:04X}"));
        }
        out.push('>');
        out
    }
}

/// Writes the PDF: each page one picture filling it.
pub fn write_pdf(pages: &[PdfPage], title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    // Objects: 1 catalog, 2 page tree, 3 info, then per page: page, contents, picture.
    let page_obj = |k: usize| 4 + 3 * k;
    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..pages.len()).map(|k| format!("{} 0 R", page_obj(k))).collect();
    obj(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).as_bytes());
    obj(&mut out, &mut offsets, format!("<< /Title {} /Producer (Wall) >>", pdf_text(title)).as_bytes());
    for (k, p) in pages.iter().enumerate() {
        let [w, h] = p.pt;
        obj(
            &mut out,
            &mut offsets,
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w:.2} {h:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>", page_obj(k) + 2, page_obj(k) + 1).as_bytes(),
        );
        let draw = format!("q {w:.2} 0 0 {h:.2} 0 0 cm /Im0 Do Q");
        obj(&mut out, &mut offsets, format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).as_bytes());
        let mut body = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", p.px[0], p.px[1], p.jpeg.len()).into_bytes();
        body.extend_from_slice(&p.jpeg);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &body);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

/// RGBA8 pixels as a JPEG (the alpha is dropped: pages are opaque).
pub fn jpeg(rgba: &[u8], size: [u32; 2]) -> Result<Vec<u8>, AppError> {
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(&rgb, size[0], size[1], image::ExtendedColorType::Rgb8)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(out)
}

/// The board page's size in points: its longer side A3's, the other to scale.
fn board_points(px: [u32; 2]) -> [f32; 2] {
    let s = BOARD_PT / px[0].max(px[1]).max(1) as f32;
    [(px[0] as f32 * s).max(72.0), (px[1] as f32 * s).max(72.0)]
}

impl App {
    /// The open comments in reading order (top to bottom, then left to
    /// right), numbered from 1, with where their pins are.
    fn comments_for_export(&self) -> Vec<([f32; 2], Vec<CommentMessage>)> {
        let mut list: Vec<([f32; 2], Vec<CommentMessage>)> = self
            .doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Comment { messages, resolved: false } if !messages.is_empty() => Some(([i.frame.x as f32, i.frame.y as f32], messages.clone())),
                _ => None,
            })
            .collect();
        list.sort_by(|a, b| a.0[1].total_cmp(&b.0[1]).then(a.0[0].total_cmp(&b.0[0])));
        list
    }

    /// File > Export board as PDF: the board with its comments numbered,
    /// then the comments. `max_side`: the board picture's longer side.
    pub fn export_pdf(&mut self, renderer: &mut Renderer, max_side: u32) -> Result<Vec<u8>, AppError> {
        let comments = self.comments_for_export();
        let pins: Vec<([f32; 2], String)> = comments.iter().enumerate().map(|(k, (at, _))| (*at, (k + 1).to_string())).collect();
        let (rgba, px) = self.export_board_marked(renderer, max_side, &pins)?;
        let mut pages = vec![PdfPage { jpeg: jpeg(&rgba, px)?, px, pt: board_points(px) }];
        drop(rgba);
        if !comments.is_empty() {
            let list: Vec<Listed> = comments.iter().enumerate().map(|(k, (_, m))| Listed { number: k + 1, messages: m }).collect();
            let n = list.len();
            let date = history::when(self.now_ms, 0, self.history.utc_offset_min);
            let date = date.rsplit_once(' ').map_or(date.clone(), |(d, _)| d.to_owned());
            let subtitle = format!("{n} open comment{}  ·  exported {date}", if n == 1 { "" } else { "s" });
            let title = self.doc.board.name.clone();
            let font = &mut self.font;
            let laid = comment_pages(&mut |s, size| panel::text_width(font, s, size), &title, &subtitle, &list, self.history.utc_offset_min);
            for page in laid {
                let rgba = self.draw_page(renderer, &page)?;
                pages.push(PdfPage { jpeg: jpeg(&rgba, PAGE_PX)?, px: PAGE_PX, pt: PAGE_PT });
            }
        }
        self.needs_redraw = true;
        Ok(write_pdf(&pages, &self.doc.board.name))
    }

    fn draw_page(&mut self, renderer: &mut Renderer, page: &Page) -> Result<Vec<u8>, AppError> {
        let size = PAGE_PX;
        let view = ViewUniform::new([size[0] as f32 / 2.0, size[1] as f32 / 2.0], 1.0, [size[0] as f32, size[1] as f32]);
        let boxes: Vec<BoxInstance> = page.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        let mut glyphs = Vec::new();
        for t in &page.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer
            .render_to_pixels(size, |r| {
                r.begin_frame(size, PAPER);
                r.draw_boxes(&view, &boxes);
                r.draw_glyphs(&view, &glyphs);
            })
            .map_err(|e| AppError::Other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font {
        Font::new(wall_text::INTER_REGULAR).unwrap()
    }

    #[test]
    fn a_pdf_of_pictures_has_a_correct_table_of_contents() {
        let img = jpeg(&[200u8; 4 * 6 * 4], [6, 4]).unwrap();
        assert_eq!(&img[..2], &[0xFF, 0xD8], "a JPEG");
        let pages = [PdfPage { jpeg: img.clone(), px: [6, 4], pt: board_points([6, 4]) }, PdfPage { jpeg: img, px: [6, 4], pt: PAGE_PT }];
        let pdf = write_pdf(&pages, "Nike (moodboard) ★");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf).to_string();
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/Title <FEFF004E0069006B0065"), "non-ASCII titles go as UTF-16");
        assert!(text.contains("/MediaBox [0 0 1190.00 793.33]"));
        // startxref points at the xref table, and every entry at its object.
        let start: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(pdf[start..].starts_with(b"xref\n0 10\n"));
        let table = String::from_utf8_lossy(&pdf[start..]).to_string();
        for (k, line) in table.lines().skip(3).take(9).enumerate() {
            let at: usize = line[..10].parse().unwrap();
            assert!(pdf[at..].starts_with(format!("{} 0 obj", k + 1).as_bytes()), "object {}", k + 1);
        }
        assert_eq!(pdf_text("a (b) \\"), "(a \\(b\\) \\\\)");
    }

    #[test]
    fn comment_pages_number_threads_and_go_on_to_more_pages() {
        let mut f = font();
        let mut width = |s: &str, size: f32| panel::text_width(&mut f, s, size);
        let msg = |by: &str, text: &str| CommentMessage { by: by.into(), text: text.into(), at_ms: 1_795_000_000_000 };
        let short = vec![msg("Mia", "Love the lighting here"), msg("Paul", "Agreed, we'll push it")];
        let pages = comment_pages(&mut width, "Moodboard", "2 open comments", &[Listed { number: 1, messages: &short }, Listed { number: 2, messages: &short }], 0);
        assert_eq!(pages.len(), 1);
        let said: Vec<&str> = pages[0].texts.iter().map(|t| t.text.as_str()).collect();
        assert!(said.contains(&"Moodboard") && said.contains(&"1") && said.contains(&"2") && said.contains(&"Mia"));
        assert!(said.iter().any(|t| t.contains("Nov 2026")), "dates in full: {said:?}");
        // Many threads: more pages, nothing below the bottom margin.
        let long = vec![msg("Client", &"a long note about the grade and the framing ".repeat(8))];
        let many: Vec<Listed> = (1..=30).map(|n| Listed { number: n, messages: &long }).collect();
        let pages = comment_pages(&mut width, "Moodboard", "30 open comments", &many, 0);
        assert!(pages.len() > 2, "{} pages", pages.len());
        for p in &pages {
            assert!(p.texts.iter().all(|t| t.at[1] <= PAGE_PX[1] as f32 - MARGIN + 25.0 && t.at[1] >= MARGIN - 40.0));
        }
        let numbers: Vec<&str> = pages.iter().flat_map(|p| &p.texts).filter(|t| t.color == [1.0; 4]).map(|t| t.text.as_str()).collect();
        assert_eq!(numbers.len(), 30);
        // A thread longer than a page goes on over the page break.
        let huge = vec![msg("Client", &"word ".repeat(3000))];
        let pages = comment_pages(&mut width, "B", "", &[Listed { number: 1, messages: &huge }], 0);
        assert!(pages.len() >= 3);
        for p in &pages {
            assert!(p.texts.iter().all(|t| t.at[1] <= PAGE_PX[1] as f32 - MARGIN + 25.0), "nothing off the page");
        }
    }
}
