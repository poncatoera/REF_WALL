//! The side panel: details of the selected item at the right of the window.
//!
//! Layout is plain data (boxes, text runs and click targets in screen
//! pixels) so it can be tested without a GPU; `App` gathers what to show
//! into [`PanelInfo`], calls [`layout`], draws the result and routes clicks
//! inside the panel to [`Action`]s.

use wall_text::Font;

// Colours come from the theme (dark / light mode).
#[allow(non_snake_case)]
pub fn BG() -> [f32; 4] {
    crate::theme::t().panel_bg
}
#[allow(non_snake_case)]
fn LINE() -> [f32; 4] {
    crate::theme::t().line
}
#[allow(non_snake_case)]
fn FIELD() -> [f32; 4] {
    crate::theme::t().field
}
#[allow(non_snake_case)]
fn INK() -> [f32; 4] {
    crate::theme::t().ink
}
#[allow(non_snake_case)]
fn DIM() -> [f32; 4] {
    crate::theme::t().dim
}
#[allow(non_snake_case)]
fn ACCENT() -> [f32; 4] {
    crate::theme::t().accent
}
#[allow(non_snake_case)]
fn CHIP() -> [f32; 4] {
    crate::theme::t().chip
}

/// Panel width in screen pixels.
pub const WIDTH: f32 = 288.0;
/// Gap between the panel and the window edge.
const MARGIN: f32 = 12.0;
/// Top of the panel: below the menu bar.
const TOP: f32 = crate::menu::BAR + MARGIN;
const PAD: f32 = 16.0;

pub const PALETTE: [&str; 7] = ["#e5534b", "#e8a15d", "#e6c84f", "#57ab5a", "#7aa2f7", "#b083f0", "#8b8f98"];
const SPEEDS: [f32; 4] = [0.25, 0.5, 1.0, 2.0];


/// Which text field of the panel has the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// Title of a picture or video, or the name of a group or board.
    Title,
    /// The "add a tag" box.
    Tag,
    /// The name of a video's marker (its index).
    Marker(usize),
}

/// Typing into a panel field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldEdit {
    pub field: Field,
    pub text: String,
}

/// Something a click in the panel asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Close,
    Edit(Field),
    RemoveTag(String),
    /// None resets to no colour.
    Color(Option<String>),
    PlayPause,
    Speed(f32),
    ClearRange,
    OpenBoard,
    /// Show only items with this tag; clicking the active tag clears it.
    FilterTag(String),
    /// Turn a suggested tag into a real one.
    AcceptTag(String),
    /// Drop a suggested tag.
    DismissTag(String),
    /// Jump the video to marker k.
    SeekMarker(usize),
    RemoveMarker(usize),
    /// A click on the panel that does nothing, so it doesn't reach the board.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub duration_s: f64,
    pub fps: Option<f32>,
    pub playing: bool,
    pub speed: f32,
    /// In and out points in seconds.
    pub range: Option<(f64, f64)>,
    /// Markers: time in seconds and name.
    pub markers: Vec<(f64, String)>,
    /// The marker the video is on now.
    pub at_marker: Option<usize>,
}

/// What the panel shows, gathered from the selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelInfo {
    /// "Image", "Video", "Note", "Group", "Board", "Arrow", "3 items".
    pub kind: String,
    /// Editable title or name, when the kind has one.
    pub title: Option<String>,
    /// Placeholder shown when the title is empty.
    pub title_hint: String,
    /// Label / value rows, e.g. ("Size", "1920 × 1080 px").
    pub rows: Vec<(String, String)>,
    /// Some when the item takes tags.
    pub tags: Option<Vec<String>>,
    /// Tags Wall suggested that aren't real tags yet.
    pub suggested: Vec<String>,
    /// Main colours of a picture, `#rrggbb`.
    pub palette: Vec<String>,
    /// Some(current colour) when the selection can be coloured.
    pub color: Option<Option<String>>,
    pub video: Option<VideoInfo>,
    pub can_open: bool,
    /// Every tag on the board with how many items carry it (board panel).
    pub board_tags: Option<Vec<(String, usize)>>,
    /// The tag the board is filtered by.
    pub filter: Option<String>,
}

pub struct TextRun {
    pub text: String,
    /// Left end of the baseline.
    pub at: [f32; 2],
    pub size: f32,
    pub color: [f32; 4],
}

pub struct PanelBox {
    pub rect: [f32; 4],
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    pub radius: f32,
    pub stroke_w: f32,
}

#[derive(Default)]
pub struct Layout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Action)>,
    /// Where the text caret goes, when a field is being typed into.
    pub caret: Option<[f32; 4]>,
}

impl Layout {
    /// The action for a click at a screen point inside the panel.
    pub fn action_at(&self, p: [f32; 2]) -> Option<Action> {
        if !inside(self.rect, p) {
            return None;
        }
        Some(self.hits.iter().rev().find(|(r, _)| inside(*r, p)).map_or(Action::None, |(_, a)| a.clone()))
    }

    fn rect(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w: if stroke[3] > 0.0 { 1.0 } else { 0.0 } });
    }

    fn text(&mut self, text: impl Into<String>, at: [f32; 2], size: f32, color: [f32; 4]) {
        self.texts.push(TextRun { text: text.into(), at, size, color });
    }
}

pub fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

/// Width of a line of text in pixels.
pub fn text_width(font: &mut Font, text: &str, size: f32) -> f32 {
    font.layout_line(text).width * size
}

/// `text`, shortened with an ellipsis to fit `max` pixels.
pub fn fit(font: &mut Font, text: &str, size: f32, max: f32) -> String {
    if text_width(font, text, size) <= max {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() {
        s.pop();
        let t = format!("{}…", s.trim_end());
        if text_width(font, &t, size) <= max {
            return t;
        }
    }
    "…".into()
}

/// As [`fit`], measuring with `width(text, size)`.
pub fn fit_with(width: &mut dyn FnMut(&str, f32) -> f32, text: &str, size: f32, max: f32) -> String {
    if width(text, size) <= max {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() {
        s.pop();
        let t = format!("{}…", s.trim_end());
        if width(&t, size) <= max {
            return t;
        }
    }
    "…".into()
}

/// `text` broken into lines of at most `max` pixels, at spaces.
pub fn wrap_with(width: &mut dyn FnMut(&str, f32) -> f32, text: &str, size: f32, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let tried = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
        if width(&tried, size) <= max || line.is_empty() {
            line = tried;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn parse_hex(c: &str) -> Option<[f32; 4]> {
    let c = c.strip_prefix('#')?;
    if c.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(c, 16).ok()?;
    Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0])
}

/// Lays out the panel at the right edge of a `viewport`-sized window.
pub fn layout(font: &mut Font, info: &PanelInfo, edit: Option<&FieldEdit>, viewport: [f32; 2]) -> Layout {
    let mut l = Layout::default();
    let x0 = (viewport[0] - WIDTH - MARGIN).max(0.0);
    let inner = WIDTH - 2.0 * PAD;
    let left = x0 + PAD;
    let mut y = TOP + PAD;

    // Header: kind and a close button.
    l.text(info.kind.to_uppercase(), [left, y + 11.0], 11.5, DIM());
    let close = [x0 + WIDTH - PAD - 18.0, y - 4.0, 22.0, 22.0];
    l.text("×", [close[0] + 5.0, close[1] + 16.0], 18.0, DIM());
    l.hits.push((close, Action::Close));
    y += 26.0;

    // Title / name field.
    if let Some(title) = &info.title {
        let r = [left, y, inner, 34.0];
        let typing = edit.filter(|e| e.field == Field::Title);
        l.rect(r, FIELD(), if typing.is_some() { ACCENT() } else { LINE() }, 6.0);
        let (shown, colour) = match typing {
            Some(e) => (e.text.clone(), INK()),
            None if title.is_empty() => (info.title_hint.clone(), DIM()),
            None => (title.clone(), INK()),
        };
        let size = 15.0;
        let shown = if typing.is_some() { shown } else { fit(font, &shown, size, inner - 20.0) };
        l.text(shown.clone(), [r[0] + 10.0, r[1] + 22.0], size, colour);
        if typing.is_some() {
            let w = text_width(font, &shown, size);
            l.caret = Some([r[0] + 10.0 + w + 1.0, r[1] + 8.0, 1.5, 18.0]);
        }
        l.hits.push((r, Action::Edit(Field::Title)));
        y += 34.0 + 14.0;
    }

    // Detail rows.
    for (label, value) in &info.rows {
        l.text(label.clone(), [left, y + 12.0], 12.5, DIM());
        let vx = left + 84.0;
        let value = fit(font, value, 12.5, inner - 84.0);
        l.text(value, [vx, y + 12.0], 12.5, INK());
        y += 20.0;
    }
    if !info.palette.is_empty() {
        l.text("Colours", [left, y + 12.0], 12.5, DIM());
        let mut x = left + 84.0;
        for c in &info.palette {
            l.rect([x, y + 1.0, 14.0, 14.0], parse_hex(c).unwrap_or(DIM()), LINE(), 7.0);
            x += 20.0;
        }
        y += 20.0;
    }
    if !info.rows.is_empty() {
        y += 8.0;
    }

    // Video controls: play/pause and speed.
    if let Some(v) = &info.video {
        divider(&mut l, x0, &mut y);
        l.text("PLAYBACK", [left, y + 11.0], 11.0, DIM());
        y += 20.0;
        let play = [left, y, 64.0, 28.0];
        l.rect(play, FIELD(), LINE(), 6.0);
        l.text(if v.playing { "Pause" } else { "Play" }, [play[0] + 12.0, play[1] + 19.0], 13.0, INK());
        l.hits.push((play, Action::PlayPause));
        let mut x = left + 72.0;
        let bw = (inner - 72.0 - 4.0 * 4.0) / SPEEDS.len() as f32;
        for s in SPEEDS {
            let r = [x, y, bw, 28.0];
            let on = (s - v.speed).abs() < 1e-3;
            l.rect(r, if on { ACCENT() } else { FIELD() }, if on { ACCENT() } else { LINE() }, 6.0);
            let t = format!("{}×", trim_float(s));
            let tw = text_width(font, &t, 12.0);
            l.text(t, [r[0] + (bw - tw) / 2.0, r[1] + 18.5], 12.0, if on { [1.0; 4] } else { INK() });
            l.hits.push((r, Action::Speed(s)));
            x += bw + 4.0;
        }
        y += 28.0 + 12.0;
        match v.range {
            Some((a, b)) => {
                let t = format!("In {}   Out {}", crate::video::format_time(a), crate::video::format_time(b));
                l.text(t, [left, y + 17.0], 12.5, INK());
                let r = [left + inner - 60.0, y, 60.0, 24.0];
                l.rect(r, FIELD(), LINE(), 6.0);
                l.text("Clear", [r[0] + 14.0, r[1] + 16.5], 12.5, INK());
                l.hits.push((r, Action::ClearRange));
            }
            None => l.text("I and O set in and out points", [left, y + 17.0], 12.5, DIM()),
        }
        y += 24.0 + 14.0;
        if v.markers.is_empty() {
            l.text("M marks a moment", [left, y + 12.0], 12.5, DIM());
            y += 20.0 + 8.0;
        }
        for (k, (t, label)) in v.markers.iter().enumerate() {
            // Time (jumps there), name (click to type), remove.
            let on = v.at_marker == Some(k);
            let tr = [left, y, 58.0, 24.0];
            l.rect(tr, if on { ACCENT() } else { FIELD() }, if on { ACCENT() } else { LINE() }, 6.0);
            l.text(crate::video::format_time(*t), [tr[0] + 8.0, tr[1] + 16.5], 12.0, if on { [1.0; 4] } else { INK() });
            l.hits.push((tr, Action::SeekMarker(k)));
            let nr = [left + 64.0, y, inner - 64.0 - 28.0, 24.0];
            let typing = edit.filter(|e| e.field == Field::Marker(k));
            l.rect(nr, FIELD(), if typing.is_some() { ACCENT() } else { LINE() }, 6.0);
            let (shown, colour) = match typing {
                Some(e) => (e.text.clone(), INK()),
                None if label.is_empty() => ("Name this moment".to_owned(), DIM()),
                None => (label.clone(), INK()),
            };
            let shown = if typing.is_some() { shown } else { fit(font, &shown, 12.5, nr[2] - 16.0) };
            l.text(shown.clone(), [nr[0] + 8.0, nr[1] + 16.5], 12.5, colour);
            if typing.is_some() {
                let w = text_width(font, &shown, 12.5);
                l.caret = Some([nr[0] + 8.0 + w + 1.0, nr[1] + 5.0, 1.5, 14.0]);
            }
            l.hits.push((nr, Action::Edit(Field::Marker(k))));
            let xr = [left + inner - 24.0, y, 24.0, 24.0];
            l.text("×", [xr[0] + 7.0, xr[1] + 17.0], 15.0, DIM());
            l.hits.push((xr, Action::RemoveMarker(k)));
            y += 28.0;
        }
        if !v.markers.is_empty() {
            y += 8.0;
        }
    }

    // Colour swatches.
    if let Some(current) = &info.color {
        divider(&mut l, x0, &mut y);
        l.text("COLOUR", [left, y + 11.0], 11.0, DIM());
        y += 20.0;
        let n = PALETTE.len() + 1;
        let s = 26.0;
        let gap = (inner - s * n as f32) / (n as f32 - 1.0);
        for k in 0..n {
            let r = [left + k as f32 * (s + gap), y, s, s];
            let colour = (k > 0).then(|| PALETTE[k - 1].to_owned());
            let on = *current == colour;
            match &colour {
                Some(c) => l.rect(r, parse_hex(c).unwrap_or(INK()), if on { INK() } else { [0.0; 4] }, s / 2.0),
                None => {
                    l.rect(r, FIELD(), if on { INK() } else { LINE() }, s / 2.0);
                    l.text("/", [r[0] + 10.0, r[1] + 18.0], 14.0, DIM());
                }
            }
            if on {
                l.boxes.last_mut().unwrap().stroke_w = 2.0;
            }
            l.hits.push((r, Action::Color(colour)));
        }
        y += s + 16.0;
    }

    // Tags: chips with a remove button, then an "add tag" box.
    if let Some(tags) = &info.tags {
        divider(&mut l, x0, &mut y);
        l.text("TAGS", [left, y + 11.0], 11.0, DIM());
        y += 20.0;
        let (h, size) = (24.0, 12.5);
        let mut x = left;
        for tag in tags {
            let label = fit(font, tag, size, inner - 40.0);
            let w = text_width(font, &label, size) + 34.0;
            if x + w > left + inner && x > left {
                x = left;
                y += h + 6.0;
            }
            let r = [x, y, w, h];
            let on = info.filter.as_ref() == Some(tag);
            l.rect(r, if on { ACCENT() } else { CHIP() }, [0.0; 4], h / 2.0);
            l.text(label, [x + 10.0, y + 16.5], size, if on { [1.0; 4] } else { INK() });
            l.hits.push((r, Action::FilterTag(tag.clone())));
            let xr = [x + w - 22.0, y + 2.0, 20.0, 20.0];
            l.text("×", [xr[0] + 4.0, xr[1] + 15.0], 14.0, DIM());
            l.hits.push((xr, Action::RemoveTag(tag.clone())));
            x += w + 6.0;
        }
        if !tags.is_empty() {
            y += h + 8.0;
        }
        if !info.suggested.is_empty() {
            l.text("Suggested", [left, y + 12.0], 11.5, DIM());
            y += 18.0;
            let mut x = left;
            for tag in &info.suggested {
                let label = fit(font, tag, size, inner - 60.0);
                let w = text_width(font, &label, size) + 54.0;
                if x + w > left + inner && x > left {
                    x = left;
                    y += h + 6.0;
                }
                let r = [x, y, w, h];
                l.rect(r, FIELD(), LINE(), h / 2.0);
                l.text(label, [x + 10.0, y + 16.5], size, DIM());
                l.hits.push((r, Action::FilterTag(tag.clone())));
                let add = [x + w - 44.0, y + 2.0, 20.0, 20.0];
                l.text("+", [add[0] + 4.0, add[1] + 15.0], 15.0, ACCENT());
                l.hits.push((add, Action::AcceptTag(tag.clone())));
                let xr = [x + w - 22.0, y + 2.0, 20.0, 20.0];
                l.text("×", [xr[0] + 4.0, xr[1] + 15.0], 14.0, DIM());
                l.hits.push((xr, Action::DismissTag(tag.clone())));
                x += w + 6.0;
            }
            y += h + 8.0;
        }
        let r = [left, y, inner, 30.0];
        let typing = edit.filter(|e| e.field == Field::Tag);
        l.rect(r, FIELD(), if typing.is_some() { ACCENT() } else { LINE() }, 6.0);
        match typing {
            Some(e) => {
                l.text(e.text.clone(), [r[0] + 10.0, r[1] + 20.0], 13.0, INK());
                let w = text_width(font, &e.text, 13.0);
                l.caret = Some([r[0] + 10.0 + w + 1.0, r[1] + 7.0, 1.5, 16.0]);
            }
            None => l.text("Add a tag", [r[0] + 10.0, r[1] + 20.0], 13.0, DIM()),
        }
        l.hits.push((r, Action::Edit(Field::Tag)));
        y += 30.0 + 16.0;
    }

    // Board panel: every tag with its count; a click filters the board.
    if let Some(tags) = &info.board_tags {
        divider(&mut l, x0, &mut y);
        l.text("TAGS ON THIS BOARD", [left, y + 11.0], 11.0, DIM());
        y += 20.0;
        if tags.is_empty() {
            l.text("No tags yet. Select a picture to add some.", [left, y + 13.0], 12.5, DIM());
            y += 22.0;
        }
        let (h, size) = (24.0, 12.5);
        let mut x = left;
        for (tag, n) in tags {
            let label = fit(font, tag, size, inner - 50.0);
            let count = n.to_string();
            let cw = text_width(font, &count, 11.5);
            let w = text_width(font, &label, size) + cw + 28.0;
            if x + w > left + inner && x > left {
                x = left;
                y += h + 6.0;
            }
            let r = [x, y, w, h];
            let on = info.filter.as_ref() == Some(tag);
            l.rect(r, if on { ACCENT() } else { CHIP() }, [0.0; 4], h / 2.0);
            l.text(label, [x + 10.0, y + 16.5], size, if on { [1.0; 4] } else { INK() });
            l.text(count, [x + w - 10.0 - cw, y + 16.5], 11.5, if on { [1.0; 4] } else { DIM() });
            l.hits.push((r, Action::FilterTag(tag.clone())));
            x += w + 6.0;
        }
        if !tags.is_empty() {
            y += h + 16.0;
        }
    }

    if info.can_open {
        divider(&mut l, x0, &mut y);
        let r = [left, y, inner, 32.0];
        l.rect(r, FIELD(), LINE(), 6.0);
        let t = if info.kind == "Link" { "Open link" } else { "Open board" };
        let tw = text_width(font, t, 13.5);
        l.text(t, [r[0] + (inner - tw) / 2.0, r[1] + 21.0], 13.5, INK());
        l.hits.push((r, Action::OpenBoard));
        y += 32.0 + 16.0;
    }

    let h = (y - TOP).max(60.0).min(viewport[1] - TOP - MARGIN);
    l.rect = [x0, TOP, WIDTH, h];
    l.boxes.insert(0, PanelBox { rect: l.rect, fill: BG(), stroke: LINE(), radius: 10.0, stroke_w: 1.0 });
    l
}

fn divider(l: &mut Layout, x0: f32, y: &mut f32) {
    l.rect([x0 + 1.0, *y, WIDTH - 2.0, 1.0], LINE(), [0.0; 4], 0.0);
    *y += 14.0;
}

fn trim_float(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// "1.4 MB", "820 KB", "512 B".
pub fn format_bytes(b: u64) -> String {
    match b {
        0..1_000 => format!("{b} B"),
        1_000..1_000_000 => format!("{:.0} KB", b as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        _ => format!("{:.2} GB", b as f64 / 1e9),
    }
}

/// "0:07", "2:05", "1:02:03".
pub fn format_duration(s: f64) -> String {
    let t = s.max(0.0).round() as u64;
    let (h, m, sec) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// A Unix time in milliseconds as "2026-11-20" (UTC).
pub fn format_date(ms: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font {
        Font::new(wall_text::INTER_REGULAR).unwrap()
    }

    #[test]
    fn formats() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(820_400), "820 KB");
        assert_eq!(format_bytes(11_400_000), "11.4 MB");
        assert_eq!(format_duration(7.4), "0:07");
        assert_eq!(format_duration(3723.0), "1:02:03");
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_795_132_800_000), "2026-11-20");
        assert_eq!(trim_float(0.25), "0.25");
        assert_eq!(trim_float(2.0), "2");
    }

    #[test]
    fn clicks_map_to_actions() {
        let mut f = font();
        let info = PanelInfo {
            kind: "Image".into(),
            title: Some("cat.jpg".into()),
            tags: Some(vec!["warm".into()]),
            color: None,
            rows: vec![("Size".into(), "10 × 10 px".into())],
            ..Default::default()
        };
        let l = layout(&mut f, &info, None, [1200.0, 800.0]);
        assert!(l.rect[0] > 1200.0 - WIDTH - 20.0);
        let centre = |r: [f32; 4]| [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        let title = l.hits.iter().find(|(_, a)| *a == Action::Edit(Field::Title)).unwrap().0;
        assert_eq!(l.action_at(centre(title)), Some(Action::Edit(Field::Title)));
        let remove = l.hits.iter().find(|(_, a)| *a == Action::RemoveTag("warm".into())).unwrap().0;
        assert_eq!(l.action_at(centre(remove)), Some(Action::RemoveTag("warm".into())));
        // Empty panel space swallows the click; outside the panel doesn't.
        assert_eq!(l.action_at([l.rect[0] + 2.0, l.rect[1] + l.rect[3] - 2.0]), Some(Action::None));
        assert_eq!(l.action_at([10.0, 10.0]), None);
    }

    #[test]
    fn long_titles_are_shortened() {
        let mut f = font();
        let long = "a very long file name that will never fit in the side panel.jpg";
        let s = fit(&mut f, long, 15.0, 200.0);
        assert!(s.ends_with('…') && text_width(&mut f, &s, 15.0) <= 200.0);
    }
}
