//! The library tab of the left panel (L): every picture and video in the
//! library as a grid of thumbnails. Click one to place it on the
//! board; the × on a tile moves it to the library's trash, and the Trash tab
//! puts things back.
//!
//! Like the side panel, layout is plain data in screen pixels (boxes, text,
//! image rects and click targets) so it can be tested without a GPU.

use std::collections::HashMap;

use wall_format::{ContentHash, ItemSidecar, MediaKind};
use wall_render::ImageId;
use wall_text::Font;

use crate::panel::{self, PanelBox, TextRun};

const PAD: f32 = 16.0;
const TILE_W: f32 = 148.0;
const IMAGE_H: f32 = 104.0;
const GAP: f32 = 12.0;
const HEADER_H: f32 = 40.0;
/// Thumbnails are decoded at most this wide or tall.
pub const THUMB_PX: u32 = 256;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    Trash,
}

/// Something a click in the drawer asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Put this picture or video on the board.
    Place(ContentHash),
    Trash(ContentHash),
    Restore(ContentHash),
    Tab(Tab),
    Close,
    /// A click on the drawer that does nothing, so it doesn't reach the board.
    None,
}

/// A decoded thumbnail, waiting for upload or on the GPU.
pub enum Thumb {
    Pending { rgba: Vec<u8>, width: u32, height: u32 },
    Ready(ImageId),
    /// A video preview frame asked of the shell, not back yet.
    Requested,
    /// Could not be read; shown as an empty tile.
    Failed,
}

/// The drawer's state while open.
pub struct LibraryView {
    pub tab: Tab,
    /// Library contents, newest first.
    pub items: Vec<ItemSidecar>,
    /// Trashed media, newest first (empty without a library).
    pub trashed: Vec<ItemSidecar>,
    /// First visible row.
    pub scroll_row: usize,
    /// Whether the Trash tab exists (only with a library on disk).
    pub has_trash: bool,
    pub thumbs: HashMap<ContentHash, Thumb>,
    /// The pointer, so a tile's × only shows while it's under it.
    pub hover: [f32; 2],
}

impl LibraryView {
    pub fn new(mut items: Vec<ItemSidecar>, trashed: Vec<ItemSidecar>, has_trash: bool) -> Self {
        items.sort_by(|a, b| b.added_ms.cmp(&a.added_ms).then_with(|| a.hash.to_string().cmp(&b.hash.to_string())));
        Self { tab: Tab::All, items, trashed, scroll_row: 0, has_trash, thumbs: HashMap::new(), hover: [-1.0, -1.0] }
    }

    pub fn shown(&self) -> &[ItemSidecar] {
        match self.tab {
            Tab::All => &self.items,
            Tab::Trash => &self.trashed,
        }
    }

    /// Scrolls by whole rows, staying in range.
    pub fn scroll(&mut self, rows: i32, layout: &Layout) {
        let r = self.scroll_row as i32 + rows;
        self.scroll_row = r.clamp(0, layout.max_scroll_row as i32) as usize;
    }
}

#[derive(Default)]
pub struct Layout {
    pub rect: [f32; 4],
    /// Drawn first: background, tiles.
    pub boxes: Vec<PanelBox>,
    /// Thumbnails: which picture, and where.
    pub images: Vec<(ContentHash, [f32; 4])>,
    /// Drawn over the thumbnails: trash buttons.
    pub top_boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Action)>,
    /// Hashes of tiles on screen, for loading their thumbnails.
    pub visible: Vec<ContentHash>,
    pub max_scroll_row: usize,
}

impl Layout {
    pub fn action_at(&self, p: [f32; 2]) -> Option<Action> {
        if !panel::inside(self.rect, p) {
            return None;
        }
        Some(self.hits.iter().rev().find(|(r, _)| panel::inside(*r, p)).map_or(Action::None, |(_, a)| a.clone()))
    }

    fn rect(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w: if stroke[3] > 0.0 { 1.0 } else { 0.0 } });
    }

    fn text(&mut self, text: impl Into<String>, at: [f32; 2], size: f32, color: [f32; 4]) {
        self.texts.push(TextRun { text: text.into(), at, size, color });
    }
}

/// Picture rect inside a tile's image area, keeping its shape.
fn fit_rect(area: [f32; 4], w: u32, h: u32) -> [f32; 4] {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let s = (area[2] / w).min(area[3] / h);
    let (fw, fh) = (w * s, h * s);
    [area[0] + (area[2] - fw) / 2.0, area[1] + (area[3] - fh) / 2.0, fw, fh]
}

/// Lays the library out in `area`. `compact` is the narrow version for the
/// left side panel: two columns of smaller tiles, no frame, hint or close.
pub fn layout_in(font: &mut Font, view: &LibraryView, area: [f32; 4], compact: bool) -> Layout {
    let mut l = Layout::default();
    let [x0, y0, w, h] = area;
    l.rect = area;
    let (tile_w, image_h) = if compact {
        let tw = ((w - 2.0 * PAD - GAP) / 2.0).floor();
        (tw, (tw * 0.72).floor())
    } else {
        (TILE_W, IMAGE_H)
    };
    let tile_h = image_h + 24.0;
    if !compact {
        l.boxes.push(crate::theme::shadow(area, crate::theme::radius::PANEL));
        l.rect(area, BG(), LINE(), crate::theme::radius::PANEL);
    }

    // Header: tabs, hint, close.
    let mut x = x0 + PAD;
    let ty = y0 + 26.0;
    let mut tab = |l: &mut Layout, label: String, t: Tab, x: &mut f32| {
        let on = view.tab == t;
        let tw = panel::text_width(font, &label, 12.5);
        let r = [*x - 10.0, y0 + 9.0, tw + 20.0, 26.0];
        if on {
            l.rect(r, crate::theme::t().accent_soft, [0.0; 4], crate::theme::radius::INNER);
        }
        l.text(label, [*x, ty], 12.5, if on { crate::theme::t().accent_text } else { DIM() });
        l.hits.push((r, Action::Tab(t)));
        *x += tw + 26.0;
    };
    tab(&mut l, format!("Library  {}", view.items.len()), Tab::All, &mut x);
    if view.has_trash {
        tab(&mut l, format!("Trash  {}", view.trashed.len()), Tab::Trash, &mut x);
    }
    if !compact {
        let hint = match view.tab {
            Tab::All => "Click to place on the board · × moves to trash · L closes",
            Tab::Trash => "Click to put back in the library",
        };
        let close = [x0 + w - PAD - 18.0, y0 + 9.0, 22.0, 22.0];
        let hint_max = (close[0] - 16.0 - x).max(0.0);
        let hint = panel::fit(font, hint, 11.5, hint_max);
        let hint_w = panel::text_width(font, &hint, 11.5);
        l.text(hint, [close[0] - 16.0 - hint_w, ty], 11.5, DIM());
        l.text("×", [close[0] + 5.0, close[1] + 16.0], 18.0, DIM());
        l.hits.push((close, Action::Close));
    }

    // Grid.
    let gx = x0 + PAD;
    let gy = y0 + HEADER_H + 8.0;
    let cols = (((w - 2.0 * PAD + GAP) / (tile_w + GAP)).floor() as usize).max(1);
    let rows = (((h - HEADER_H - 8.0 - PAD + GAP) / (tile_h + GAP)).floor() as usize).max(1);
    let shown = view.shown();
    let total_rows = shown.len().div_ceil(cols);
    l.max_scroll_row = total_rows.saturating_sub(rows);
    let first = view.scroll_row.min(l.max_scroll_row) * cols;

    if shown.is_empty() {
        let msg = match view.tab {
            Tab::All if compact => "Nothing here yet.",
            Tab::All => "Nothing here yet. Drop pictures or videos on the window.",
            Tab::Trash => "The trash is empty.",
        };
        l.text(msg, [gx, gy + 40.0], 13.0, DIM());
    }
    for (i, sc) in shown.iter().enumerate().skip(first).take(cols * rows) {
        let k = i - first;
        let (c, r) = ((k % cols) as f32, (k / cols) as f32);
        let tile = [gx + c * (tile_w + GAP), gy + r * (tile_h + GAP), tile_w, tile_h];
        let area = [tile[0], tile[1], tile_w, image_h];
        l.rect(area, TILE(), LINE(), crate::theme::radius::CONTROL);
        l.visible.push(sc.hash.clone());
        let inner = [area[0] + 4.0, area[1] + 4.0, area[2] - 8.0, area[3] - 8.0];
        // Pictures, and videos' preview frames once the shell sends them.
        l.images.push((sc.hash.clone(), fit_rect(inner, sc.width, sc.height)));
        if sc.kind == MediaKind::Video {
            // A dark pill over the frame: play mark and length.
            let d = sc.duration_ms.map(|ms| panel::format_duration(ms as f64 / 1000.0)).unwrap_or_default();
            let dw = panel::text_width(font, &d, 11.0);
            let pill = [area[0] + 8.0, area[1] + area[3] - 26.0, dw + 30.0, 18.0];
            l.top_boxes.push(PanelBox { rect: pill, fill: [0.0, 0.0, 0.0, 0.6], stroke: [0.0; 4], radius: 9.0, stroke_w: 0.0 });
            l.text("▶", [pill[0] + 7.0, pill[1] + 13.5], 10.0, [1.0; 4]);
            l.text(d, [pill[0] + 22.0, pill[1] + 13.5], 11.0, [1.0; 4]);
        }
        let title = sc.title.clone().unwrap_or_else(|| format!("{} × {}", sc.width, sc.height));
        let title = panel::fit(font, &title, 12.0, tile_w - 4.0);
        l.text(title, [tile[0] + 2.0, tile[1] + image_h + 17.0], 12.0, DIM());

        match view.tab {
            Tab::All => {
                l.hits.push((tile, Action::Place(sc.hash.clone())));
                let x = [area[0] + area[2] - 26.0, area[1] + 6.0, 20.0, 20.0];
                // × only on the tile under the pointer (its hit is always there).
                if panel::inside(tile, view.hover) {
                    l.top_boxes.push(PanelBox { rect: x, fill: [0.0, 0.0, 0.0, 0.55], stroke: [0.0; 4], radius: 10.0, stroke_w: 0.0 });
                    l.text("×", [x[0] + 5.0, x[1] + 15.0], 15.0, [1.0; 4]);
                }
                l.hits.push((x, Action::Trash(sc.hash.clone())));
            }
            Tab::Trash => l.hits.push((tile, Action::Restore(sc.hash.clone()))),
        }
    }
    if l.max_scroll_row > 0 {
        let shown_rows = rows.min(total_rows);
        let msg = format!("{}{}–{} of {}{}", if compact { "" } else { "Rows " }, view.scroll_row.min(l.max_scroll_row) + 1, view.scroll_row.min(l.max_scroll_row) + shown_rows, total_rows, if compact { " rows" } else { " · scroll for more" });
        let mw = panel::text_width(font, &msg, 11.0);
        l.text(msg, [x0 + w - PAD - mw, y0 + h - 6.0], 11.0, DIM());
    }
    l
}

/// Decodes a picture file into a thumbnail at most [`THUMB_PX`] on a side.
pub fn decode_thumb(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let t = img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
    let (w, h) = t.dimensions();
    Some((t.into_raw(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(n: u8, kind: MediaKind, added: u64) -> ItemSidecar {
        ItemSidecar {
            hash: ContentHash::of(&[n]),
            kind,
            ext: if kind == MediaKind::Video { "mp4".into() } else { "png".into() },
            width: 400,
            height: 200,
            bytes: 1,
            duration_ms: (kind == MediaKind::Video).then_some(4000),
            fps: None,
            added_ms: added,
            title: Some(format!("Item {n}")),
            source_url: None,
            tags: vec![],
            auto_tags: vec![],
            colors: vec![],
            original: None,
            range: None,
            markers: Vec::new(),
            pingpong: false,
            extra: Default::default(),
        }
    }

    fn font() -> Font {
        Font::new(wall_text::INTER_REGULAR).unwrap()
    }

    #[test]
    fn newest_first_and_clicks_map_to_actions() {
        let items = vec![sidecar(1, MediaKind::Image, 10), sidecar(2, MediaKind::Video, 20)];
        let v = LibraryView::new(items, vec![], true);
        assert_eq!(v.items[0].title.as_deref(), Some("Item 2"));
        let l = layout_in(&mut font(), &v, [0.0, 80.0, 279.0, 720.0], true);
        assert_eq!(l.visible.len(), 2);
        // Both get an image slot (the video's preview frame comes from the
        // shell); the video also shows its length.
        assert_eq!(l.images.len(), 2);
        assert!(l.texts.iter().any(|t| t.text == "0:04"));
        // A click in the middle of the first tile places it; its × trashes it.
        let tile = l.hits.iter().find(|(_, a)| matches!(a, Action::Place(_))).unwrap().0;
        let mid = [tile[0] + 20.0, tile[1] + 60.0];
        assert_eq!(l.action_at(mid), Some(Action::Place(v.items[0].hash.clone())));
        let x = l.hits.iter().find(|(_, a)| matches!(a, Action::Trash(_))).unwrap().0;
        assert_eq!(l.action_at([x[0] + 10.0, x[1] + 10.0]), Some(Action::Trash(v.items[0].hash.clone())));
        // Clicks above the drawer reach the board; inside on nothing are eaten.
        assert_eq!(l.action_at([600.0, 10.0]), None);
        assert_eq!(l.action_at([l.rect[0] + l.rect[2] - 40.0, l.rect[1] + l.rect[3] - 20.0]), Some(Action::None));
    }

    #[test]
    fn trash_tab_restores_and_hidden_without_library() {
        let mut v = LibraryView::new(vec![], vec![sidecar(3, MediaKind::Image, 1)], true);
        v.tab = Tab::Trash;
        let l = layout_in(&mut font(), &v, [0.0, 80.0, 279.0, 720.0], true);
        assert!(l.hits.iter().any(|(_, a)| *a == Action::Restore(v.trashed[0].hash.clone())));
        let web = LibraryView::new(vec![], vec![], false);
        let l = layout_in(&mut font(), &web, [0.0, 80.0, 279.0, 720.0], true);
        assert!(!l.hits.iter().any(|(_, a)| *a == Action::Tab(Tab::Trash)));
    }

    #[test]
    fn scrolls_by_rows_within_range() {
        let items: Vec<_> = (0..60).map(|n| sidecar(n, MediaKind::Image, n as u64)).collect();
        let mut v = LibraryView::new(items, vec![], true);
        let l = layout_in(&mut font(), &v, [0.0, 80.0, 279.0, 720.0], true);
        assert!(l.max_scroll_row > 0);
        v.scroll(100, &l);
        assert_eq!(v.scroll_row, l.max_scroll_row);
        v.scroll(-100, &l);
        assert_eq!(v.scroll_row, 0);
        let first = l.visible[0].clone();
        v.scroll(1, &l);
        let l2 = layout_in(&mut font(), &v, [0.0, 80.0, 279.0, 720.0], true);
        assert_ne!(l2.visible[0], first);
    }

    #[test]
    fn app_places_trashes_and_restores_from_a_real_library() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = App::open_library(&dir.path().join("L.wall"), "test", 1).unwrap();
        a.resize([1000.0, 800.0]);
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 4, image::Rgba([200, 30, 90, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let first = a.import_image_bytes(&png, Some("Pink"), 5).unwrap();
        a.selection = [first].into();
        a.key(crate::Key::Delete);
        assert_eq!(a.item_count(), 0);

        a.key(crate::Key::Char('l'));
        assert!(a.library_is_open());
        let l = a.library_layout().unwrap();
        let tile = l.hits.iter().find(|(_, x)| matches!(x, Action::Place(_))).unwrap().0;
        a.pointer_down(crate::Button::Primary, [tile[0] + 20.0, tile[1] + 60.0], crate::Mods::default());
        assert_eq!(a.item_count(), 1, "clicking a tile places it on the board");
        // Placed beside the panel, not under it.
        let f = a.doc.board.live_items().next().unwrap().frame;
        assert!((f.x + f.w / 2.0) > a.camera.screen_to_board([crate::sidebar::WIDTH, 0.0], a.viewport)[0] as f64);

        let l = a.library_layout().unwrap();
        let x = l.hits.iter().find(|(_, x)| matches!(x, Action::Trash(_))).unwrap().0;
        a.set_time(10_000);
        a.pointer_down(crate::Button::Primary, [x[0] + 10.0, x[1] + 10.0], crate::Mods::default());
        let lib = a.library.as_ref().unwrap();
        assert!(lib.sidecars().unwrap().is_empty());
        assert_eq!(lib.trashed_media().unwrap().len(), 1);

        a.library_action(Action::Tab(Tab::Trash));
        let l = a.library_layout().unwrap();
        let r = l.hits.iter().find(|(_, x)| matches!(x, Action::Restore(_))).unwrap().0;
        a.set_time(20_000);
        a.pointer_down(crate::Button::Primary, [r[0] + 20.0, r[1] + 60.0], crate::Mods::default());
        assert_eq!(a.library.as_ref().unwrap().sidecars().unwrap().len(), 1);
        a.key(crate::Key::Escape);
        assert!(!a.library_is_open());
    }

    #[test]
    fn videos_off_the_board_ask_the_shell_for_a_preview_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = App::open_library(&dir.path().join("V.wall"), "test", 1).unwrap();
        a.resize([1000.0, 800.0]);
        let meta = crate::VideoMeta { width: 1920, height: 1080, duration_ms: 8000, fps: Some(25.0) };
        let id = a.import_video(b"fake mp4", "mp4", meta, Some("clip.mp4"), 5).unwrap();
        a.selection = [id].into();
        a.key(crate::Key::Delete);
        a.toggle_sidebar(crate::sidebar::Tab::Library);

        let wanted = a.library_posters_wanted();
        assert_eq!(wanted.len(), 1);
        let (hash, path, w, h, at) = wanted[0].clone();
        assert!(path.is_file(), "points at the library's copy");
        assert_eq!((w, h), (256, 144), "fits 256 px, even sizes");
        assert_eq!(at, 0.8, "a little way in: 10% of 8 s");
        assert!(a.library_waiting());
        assert!(a.library_posters_wanted().is_empty(), "asked once");

        a.set_library_poster(hash, Some((vec![0; 256 * 144 * 4], 256, 144)));
        assert!(!a.library_waiting());
    }

    #[test]
    fn fit_keeps_shape_and_centres() {
        assert_eq!(fit_rect([0.0, 0.0, 100.0, 100.0], 200, 100), [0.0, 25.0, 100.0, 50.0]);
    }
}

// ---- App side: open/close, clicks, drawing --------------------------------

use crate::{App, Picture};

// Colours come from the theme (dark / light mode).
#[allow(non_snake_case)]
fn BG() -> [f32; 4] {
    crate::theme::t().panel_bg
}
#[allow(non_snake_case)]
fn LINE() -> [f32; 4] {
    crate::theme::t().line
}
#[allow(non_snake_case)]
fn TILE() -> [f32; 4] {
    crate::theme::t().tile
}
#[allow(non_snake_case)]
fn DIM() -> [f32; 4] {
    crate::theme::t().dim
}

impl App {
    /// Reads the library's contents for the Library tab (once per opening
    /// of the tab, so new imports show up).
    pub(crate) fn ensure_library_view(&mut self) {
        self.needs_redraw = true;
        let (items, trashed, has_trash) = match &self.library {
            Some(lib) => {
                let items = lib.sidecars().map(|v| v.into_iter().filter_map(Result::ok).collect()).unwrap_or_default();
                (items, lib.trashed_media().unwrap_or_default(), true)
            }
            // No library on disk (web, demo): what this session has seen.
            None => (self.sidecars.values().cloned().collect(), Vec::new(), false),
        };
        self.library_view = Some(LibraryView::new(items, trashed, has_trash));
    }

    /// True while the left panel shows the Library tab.
    pub fn library_is_open(&self) -> bool {
        self.library_view.is_some() && self.sidebar_tab() == Some(crate::sidebar::Tab::Library)
    }

    pub(crate) fn library_layout(&mut self) -> Option<Layout> {
        if !self.library_is_open() {
            return None;
        }
        let area = self.sidebar_layout()?.content;
        let cursor = self.cursor;
        let view = self.library_view.as_mut()?;
        view.hover = cursor;
        Some(layout_in(&mut self.font, view, area, true))
    }

    /// Wheel over the drawer scrolls it. True if it was over the drawer.
    pub(crate) fn library_wheel(&mut self, steps: f32) -> bool {
        let Some(l) = self.library_layout() else { return false };
        if !panel::inside(l.rect, self.cursor) {
            return false;
        }
        let rows = if steps > 0.0 { -1 } else if steps < 0.0 { 1 } else { 0 };
        if let Some(v) = &mut self.library_view {
            v.scroll(rows, &l);
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn library_action(&mut self, action: Action) {
        self.needs_redraw = true;
        let now = self.now_ms;
        match action {
            Action::Close => self.close_sidebar(),
            Action::None => {}
            Action::Tab(t) => {
                if let Some(v) = &mut self.library_view {
                    v.tab = t;
                    v.scroll_row = 0;
                }
            }
            Action::Place(hash) => self.library_place(hash),
            Action::Trash(hash) => {
                let Some(lib) = &self.library else { return };
                match lib.trash_media(&hash, now) {
                    Ok(_) => {
                        if let Some(v) = &mut self.library_view {
                            if let Some(i) = v.items.iter().position(|s| s.hash == hash) {
                                let sc = v.items.remove(i);
                                v.trashed.insert(0, sc);
                            }
                        }
                    }
                    Err(e) => eprintln!("wall: could not move to trash: {e}"),
                }
            }
            Action::Restore(hash) => {
                let Some(lib) = &self.library else { return };
                match lib.restore_media(&hash) {
                    Ok(()) => {
                        if let Some(v) = &mut self.library_view {
                            if let Some(i) = v.trashed.iter().position(|s| s.hash == hash) {
                                let sc = v.trashed.remove(i);
                                v.items.insert(0, sc);
                            }
                        }
                    }
                    Err(e) => eprintln!("wall: could not restore: {e}"),
                }
            }
        }
    }

    /// Puts a library item on the board, in the middle of the view above
    /// the drawer.
    fn library_place(&mut self, hash: ContentHash) {
        let Some(sc) = self.library_view.as_ref().and_then(|v| v.items.iter().find(|s| s.hash == hash)).cloned() else { return };
        let saved = self.cursor;
        let left = self.left_inset();
        self.cursor = [left + (self.viewport[0] - left) / 2.0, self.viewport[1] / 2.0];
        self.place_media(hash, sc.width, sc.height);
        self.cursor = saved;
        // Decode the full picture (or register the video) like on open.
        if let Some(lib) = self.library.take() {
            self.load_pictures(&lib);
            self.library = Some(lib);
        } else {
            self.sidecars.entry(sc.hash.clone()).or_insert(sc);
        }
    }

    /// Videos on screen in the drawer that need a preview frame, as (hash,
    /// file, width, height, time in seconds). Each is asked for once; the
    /// shell decodes it (off the UI thread) and hands it to
    /// [`App::set_library_poster`].
    pub fn library_posters_wanted(&mut self) -> Vec<(ContentHash, std::path::PathBuf, u32, u32, f64)> {
        // The boards panel's video thumbnails come the same way.
        let mut out = self.sidebar_posters_wanted();
        let Some(l) = self.library_layout() else { return out };
        let (Some(v), Some(lib)) = (self.library_view.as_mut(), self.library.as_ref()) else { return out };
        for hash in &l.visible {
            if v.thumbs.contains_key(hash) || matches!(self.pictures.get(hash), Some(Picture::Ready(_) | Picture::Preview(_))) {
                continue;
            }
            let Some(sc) = v.items.iter().find(|s| &s.hash == hash && s.kind == MediaKind::Video) else { continue };
            // At most THUMB_PX on a side, even sizes for ffmpeg's scaler.
            let s = (THUMB_PX as f32 / sc.width.max(sc.height).max(1) as f32).min(1.0);
            let even = |x: f32| ((x.round() as u32) & !1).max(2);
            let (w, h) = (even(sc.width as f32 * s), even(sc.height as f32 * s));
            // A frame a little way in: first frames are often black.
            let at = sc.duration_ms.map_or(0.0, |ms| (ms as f64 / 1000.0 * 0.1).min(1.0));
            out.push((hash.clone(), lib.media_path(hash, &sc.ext), w, h, at));
            v.thumbs.insert(hash.clone(), Thumb::Requested);
        }
        out
    }

    /// True while preview frames asked of the shell are still out, so the
    /// shell keeps drawing until they arrive.
    pub fn library_waiting(&self) -> bool {
        self.library_view.as_ref().is_some_and(|v| v.thumbs.values().any(|t| matches!(t, Thumb::Requested)))
            || self.board_thumbs.values().any(|t| matches!(t, Thumb::Requested))
    }

    /// A video preview frame from the shell (None if it couldn't decode).
    pub fn set_library_poster(&mut self, hash: ContentHash, frame: Option<(Vec<u8>, u32, u32)>) {
        if matches!(self.board_thumbs.get(&hash), Some(Thumb::Requested)) {
            self.board_thumbs.insert(
                hash.clone(),
                match &frame {
                    Some((rgba, width, height)) => Thumb::Pending { rgba: rgba.clone(), width: *width, height: *height },
                    None => Thumb::Failed,
                },
            );
            self.needs_redraw = true;
        }
        if let Some(v) = &mut self.library_view {
            v.thumbs.insert(hash, match frame {
                Some((rgba, width, height)) => Thumb::Pending { rgba, width, height },
                None => Thumb::Failed,
            });
            self.needs_redraw = true;
        }
    }

    /// Draws the drawer over everything else, decoding a few missing
    /// thumbnails per frame.
    pub(crate) fn draw_library(&mut self, renderer: &mut wall_render::Renderer) {
        let Some(l) = self.library_layout() else { return };
        let Some(v) = self.library_view.as_mut() else { return };

        // Thumbnails: reuse the board's full picture when loaded, else decode
        // a small one from the library file (a few per frame).
        let mut budget = 3;
        for hash in &l.visible {
            if v.thumbs.contains_key(hash) || matches!(self.pictures.get(hash), Some(Picture::Ready(_) | Picture::Preview(_))) {
                continue;
            }
            let Some(sc) = v.items.iter().chain(&v.trashed).find(|s| &s.hash == hash) else { continue };
            if sc.kind == MediaKind::Video {
                continue;
            }
            if budget == 0 {
                self.needs_redraw = true;
                break;
            }
            budget -= 1;
            let thumb = match &self.pictures.get(hash) {
                Some(Picture::Pending { rgba, width, height }) => {
                    image::RgbaImage::from_raw(*width, *height, rgba.clone()).map(|img| {
                        let t = image::DynamicImage::ImageRgba8(img).thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
                        let (w, h) = t.dimensions();
                        (t.into_raw(), w, h)
                    })
                }
                _ => self.library.as_ref().and_then(|lib| {
                    // Trashed files live in the trash folder; restore first to see them.
                    std::fs::read(lib.media_path(hash, &sc.ext)).ok().and_then(|b| decode_thumb(&b))
                }),
            };
            v.thumbs.insert(hash.clone(), match thumb {
                Some((rgba, width, height)) => Thumb::Pending { rgba, width, height },
                None => Thumb::Failed,
            });
        }
        for t in v.thumbs.values_mut() {
            if let Thumb::Pending { rgba, width, height } = t {
                *t = match renderer.create_image(rgba, *width, *height) {
                    Ok(id) => Thumb::Ready(id),
                    Err(_) => Thumb::Failed,
                };
            }
        }

        let view = wall_render::ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let to_box = |b: &PanelBox| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] };
        renderer.draw_boxes(&view, &l.boxes.iter().map(to_box).collect::<Vec<_>>());
        let images: Vec<_> = l
            .images
            .iter()
            .filter_map(|(hash, rect)| {
                let id = match (v.thumbs.get(hash), self.pictures.get(hash)) {
                    (Some(Thumb::Ready(id)), _) | (_, Some(Picture::Ready(id) | Picture::Preview(id))) => *id,
                    _ => return None,
                };
                Some((id, wall_render::ImageInstance { rect: *rect, params: [1.0, 4.0, 0.0, 0.0], uv_rect: [0.0, 0.0, 1.0, 1.0] }))
            })
            .collect();
        renderer.draw_images(&view, &images);
        renderer.draw_boxes(&view, &l.top_boxes.iter().map(to_box).collect::<Vec<_>>());
        let mut glyphs = Vec::new();
        for t in &l.texts {
            crate::scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}
