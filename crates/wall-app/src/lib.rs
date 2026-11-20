//! The app logic shared by desktop and web: board document, camera, input,
//! selection, and turning the board into GPU instances.
//! Shells (apps/desktop, apps/web) only create the window or canvas, forward
//! input here and call [`App::render`].

pub mod doc;
mod scene;

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;
use wall_format::{Board, BoardItem, ContentHash, Frame, ItemKind, ItemSidecar, Library, MediaKind};
use wall_render::{ImageId, Renderer, ViewUniform};
use wall_text::Font;

pub use doc::Document;
use scene::Scene;

/// Pan and zoom over the infinite board.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Board point at the centre of the screen.
    pub center: [f32; 2],
    /// Screen pixels per board unit.
    pub zoom: f32,
}

pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 64.0;

impl Camera {
    pub fn screen_to_board(&self, p: [f32; 2], size: [f32; 2]) -> [f32; 2] {
        [
            self.center[0] + (p[0] - size[0] * 0.5) / self.zoom,
            self.center[1] + (p[1] - size[1] * 0.5) / self.zoom,
        ]
    }

    /// Moves the board with the pointer by a screen-pixel delta.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.center[0] -= dx / self.zoom;
        self.center[1] -= dy / self.zoom;
    }

    /// Zooms by `factor` keeping the board point under `cursor` fixed.
    pub fn zoom_at(&mut self, factor: f32, cursor: [f32; 2], size: [f32; 2]) {
        let before = self.screen_to_board(cursor, size);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.screen_to_board(cursor, size);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }
}

/// Wheel input in "lines" (desktop mouse) or pixels (trackpad / browser).
#[derive(Debug, Clone, Copy)]
pub enum Wheel {
    Lines(f32),
    Pixels(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Middle,
    Secondary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// Ctrl on Windows, Cmd on macOS.
    pub command: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Delete,
    Undo,
    Redo,
    SelectAll,
    Escape,
    /// Space held: drag pans the board.
    Space { pressed: bool },
}

#[derive(Debug, Clone)]
enum Gesture {
    Idle,
    Pan,
    Move { start: [f32; 2], originals: Vec<BoardItem>, delta: [f64; 2] },
    Marquee { start: [f32; 2], now: [f32; 2], base: BTreeSet<Uuid> },
}

/// Decoded pixels for a media hash, waiting for upload or already on the GPU.
enum Picture {
    Pending { rgba: Vec<u8>, width: u32, height: u32 },
    Ready(ImageId),
}

pub struct App {
    pub camera: Camera,
    pub doc: Document,
    pub selection: BTreeSet<Uuid>,
    font: Font,
    scene: Scene,
    pictures: HashMap<ContentHash, Picture>,
    gesture: Gesture,
    cursor: [f32; 2],
    viewport: [f32; 2],
    space_held: bool,
    /// Set when the next frame must be drawn.
    pub needs_redraw: bool,
    library: Option<Library>,
    saved_version: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Format(#[from] wall_format::Error),
    #[error(transparent)]
    Media(#[from] wall_media::MediaError),
    #[error("could not decode image: {0}")]
    Decode(#[from] image::ImageError),
}

const BACKGROUND: [f32; 4] = [0.075, 0.078, 0.086, 1.0];

impl App {
    pub fn new(board: Board, device: &str) -> Self {
        Self {
            camera: Camera { center: [0.0, 0.0], zoom: 1.0 },
            doc: Document::new(board, device),
            selection: BTreeSet::new(),
            font: Font::new(wall_text::INTER_REGULAR).expect("bundled font parses"),
            scene: Scene::default(),
            pictures: HashMap::new(),
            gesture: Gesture::Idle,
            cursor: [0.0, 0.0],
            viewport: [1.0, 1.0],
            space_held: false,
            needs_redraw: true,
            library: None,
            saved_version: 0,
        }
    }

    /// Opens (or creates) a `.wall` library and shows its first board,
    /// creating "Board 1" in an empty library.
    pub fn open_library(root: &std::path::Path, device: &str, now_ms: u64) -> Result<Self, AppError> {
        let library = if root.join("wall.json").is_file() {
            Library::open(root)?
        } else {
            Library::create(root, root.file_stem().and_then(|s| s.to_str()).unwrap_or("Wall"), now_ms)?
        };
        let mut boards = Vec::new();
        for id in library.board_ids()? {
            boards.push(library.read_board(id)?);
        }
        boards.sort_by_key(|b| b.created_ms);
        let board = match boards.into_iter().next() {
            Some(b) => b,
            None => {
                let b = Board::new("Board 1", now_ms);
                library.write_board(&b)?;
                b
            }
        };
        let mut app = Self::new(board, device);
        app.load_pictures(&library);
        app.library = Some(library);
        app.fit_view();
        Ok(app)
    }

    /// Decodes every picture the board uses that isn't loaded yet.
    fn load_pictures(&mut self, library: &Library) {
        let hashes: Vec<ContentHash> = self
            .doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Media { hash, .. } => Some(hash.clone()),
                _ => None,
            })
            .filter(|h| !self.pictures.contains_key(h))
            .collect();
        for hash in hashes {
            let loaded = library.read_sidecar(&hash).ok().and_then(|sc| {
                let bytes = std::fs::read(library.media_path(&hash, &sc.ext)).ok()?;
                image::load_from_memory(&bytes).ok()
            });
            match loaded {
                Some(img) => {
                    let img = img.to_rgba8();
                    let (w, h) = img.dimensions();
                    self.pictures.insert(hash, Picture::Pending { rgba: img.into_raw(), width: w, height: h });
                }
                None => eprintln!("wall: picture {hash} is missing from the library"),
            }
        }
    }

    /// Centres the view on everything on the board.
    pub fn fit_view(&mut self) {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for i in self.doc.board.live_items() {
            b = [b[0].min(i.frame.x), b[1].min(i.frame.y), b[2].max(i.frame.x + i.frame.w), b[3].max(i.frame.y + i.frame.h)];
        }
        if b[0] > b[2] {
            self.camera = Camera { center: [0.0, 0.0], zoom: 1.0 };
            return;
        }
        let (w, h) = ((b[2] - b[0]) as f32, (b[3] - b[1]) as f32);
        let zoom = (self.viewport[0] / (w + 80.0)).min(self.viewport[1] / (h + 80.0)).clamp(MIN_ZOOM, 1.0);
        self.camera = Camera { center: [((b[0] + b[2]) / 2.0) as f32, ((b[1] + b[3]) / 2.0) as f32], zoom };
        self.needs_redraw = true;
    }

    /// Imports an image file's bytes: applies the 2048 px rule, stores it in
    /// the library (if one is open), and drops it at the cursor.
    pub fn import_image_bytes(&mut self, bytes: &[u8], source: Option<&str>, now_ms: u64) -> Result<Uuid, AppError> {
        let prepared = wall_media::prepare_image(bytes, &wall_media::Limits::default())?;
        let rgba = image::load_from_memory(&prepared.bytes)?.to_rgba8();
        let hash = ContentHash::of(&prepared.bytes);
        if let Some(lib) = &self.library {
            lib.import_bytes(&prepared.bytes, prepared.ext)?;
            if !lib.has_media(&hash) {
                lib.write_sidecar(&ItemSidecar {
                    hash: hash.clone(),
                    kind: MediaKind::Image,
                    ext: prepared.ext.into(),
                    width: prepared.width,
                    height: prepared.height,
                    bytes: prepared.bytes.len() as u64,
                    duration_ms: None,
                    fps: None,
                    added_ms: now_ms,
                    title: source.map(|s| s.to_owned()),
                    source_url: None,
                    tags: Vec::new(),
                    auto_tags: Vec::new(),
                    colors: Vec::new(),
                    original: prepared.original.clone(),
                    extra: Default::default(),
                })?;
            }
        }
        let (w, h) = rgba.dimensions();
        self.pictures.entry(hash.clone()).or_insert(Picture::Pending { rgba: rgba.into_raw(), width: w, height: h });
        Ok(self.place_media(hash, w, h))
    }

    /// Writes the board to the library if it changed and no drag is in
    /// progress. Returns true when it saved.
    pub fn save_if_dirty(&mut self) -> Result<bool, AppError> {
        let Some(lib) = &self.library else { return Ok(false) };
        if self.saved_version == self.doc.version || !matches!(self.gesture, Gesture::Idle) {
            return Ok(false);
        }
        lib.write_board(&self.doc.board)?;
        self.saved_version = self.doc.version;
        Ok(true)
    }

    /// A demo board: a group of pictures, a group of `notes` notes, a title.
    pub fn demo(notes: u32) -> Self {
        let mut app = Self::new(Board::new("Demo", 0), "demo");
        let mut items: Vec<(Frame, ItemKind)> = Vec::new();
        let f = |x: f64, y: f64, w: f64, h: f64| Frame { x, y, w, h, ..Default::default() };

        items.push((f(0.0, -190.0, 900.0, 120.0), ItemKind::Label { text: "Wall".into() }));
        items.push((f(-40.0, -40.0, 6.0 * 340.0 + 60.0, 4.0 * 230.0 + 100.0), ItemKind::Group { name: "Motion refs".into(), color: Some("#e8a15d".into()) }));
        for i in 0..24u32 {
            let (w, h) = (320u32, 200u32);
            let rgba = demo_picture(i, w, h);
            let hash = app.add_picture(rgba, w, h);
            let (c, r) = (i % 6, i / 6);
            items.push((f(c as f64 * 340.0, 20.0 + r as f64 * 230.0, w as f64, h as f64), ItemKind::Media { hash, crop: None }));
        }

        let cols = (notes as f64).sqrt().ceil().max(1.0) as u32;
        let top = 4.0 * 230.0 + 140.0;
        items.push((
            f(-40.0, top, cols as f64 * 260.0 + 60.0, notes.div_ceil(cols) as f64 * 180.0 + 80.0),
            ItemKind::Group { name: format!("{notes} notes"), color: Some("#7aa2f7".into()) },
        ));
        let words = ["Title sequence", "Logo reveal", "Kinetic type on a grid", "Moodboard", "Style frame with grain", "Walk cycle", "Neon", "Ease out, long tail"];
        for i in 0..notes {
            let (c, r) = (i % cols, i / cols);
            let text = format!("{} #{}", words[(i as usize) % words.len()], i + 1);
            items.push((f(c as f64 * 260.0, top + 60.0 + r as f64 * 180.0, 230.0, 150.0), ItemKind::Note { text, color: None }));
        }
        app.doc.add(items);
        app.camera = Camera { center: [980.0, 420.0], zoom: 0.62 };
        app
    }

    /// Registers decoded pixels for a picture and returns its content hash.
    pub fn add_picture(&mut self, rgba: Vec<u8>, width: u32, height: u32) -> ContentHash {
        let hash = ContentHash::of(&rgba);
        self.pictures.entry(hash.clone()).or_insert(Picture::Pending { rgba, width, height });
        self.needs_redraw = true;
        hash
    }

    /// Drops an image on the board, centred at the cursor, selected.
    pub fn place_picture(&mut self, rgba: Vec<u8>, width: u32, height: u32) -> Uuid {
        let hash = self.add_picture(rgba, width, height);
        self.place_media(hash, width, height)
    }

    fn place_media(&mut self, hash: ContentHash, width: u32, height: u32) -> Uuid {
        let at = self.camera.screen_to_board(self.cursor, self.viewport);
        let (w, h) = (width as f64, height as f64);
        let frame = Frame { x: at[0] as f64 - w / 2.0, y: at[1] as f64 - h / 2.0, w, h, ..Default::default() };
        let id = self.doc.add(vec![(frame, ItemKind::Media { hash, crop: None })])[0];
        self.selection = [id].into();
        self.needs_redraw = true;
        id
    }

    pub fn item_count(&self) -> usize {
        self.doc.board.live_items().count()
    }

    pub fn glyph_count(&self) -> usize {
        self.scene.glyph_count()
    }

    // ---- input -------------------------------------------------------------

    pub fn resize(&mut self, size_px: [f32; 2]) {
        self.viewport = size_px;
        self.needs_redraw = true;
    }

    /// Topmost live item under a board point.
    pub fn hit(&self, p: [f32; 2]) -> Option<Uuid> {
        let (x, y) = (p[0] as f64, p[1] as f64);
        self.doc
            .board
            .live_items()
            .filter(|i| !matches!(i.kind, ItemKind::Arrow { .. }))
            .filter(|i| x >= i.frame.x && x <= i.frame.x + i.frame.w && y >= i.frame.y && y <= i.frame.y + i.frame.h)
            .max_by(|a, b| a.frame.z.total_cmp(&b.frame.z))
            .map(|i| i.id)
    }

    pub fn pointer_down(&mut self, button: Button, pos: [f32; 2], mods: Mods) {
        self.cursor = pos;
        let board = self.camera.screen_to_board(pos, self.viewport);
        self.needs_redraw = true;
        if button == Button::Middle || (button == Button::Primary && self.space_held) {
            self.gesture = Gesture::Pan;
            return;
        }
        if button != Button::Primary {
            return;
        }
        match self.hit(board) {
            Some(id) => {
                if mods.shift {
                    if !self.selection.remove(&id) {
                        self.selection.insert(id);
                    }
                } else if !self.selection.contains(&id) {
                    self.selection = [id].into();
                }
                let originals = self.drag_set();
                self.gesture = Gesture::Move { start: board, originals, delta: [0.0, 0.0] };
            }
            None => {
                let base = if mods.shift { self.selection.clone() } else { BTreeSet::new() };
                self.selection = base.clone();
                self.gesture = Gesture::Marquee { start: board, now: board, base };
            }
        }
    }

    /// Selected items plus everything inside selected groups (groups carry
    /// their contents, as in Milanote).
    fn drag_set(&self) -> Vec<BoardItem> {
        let mut ids = self.selection.clone();
        for id in &self.selection {
            if let Some(g) = self.doc.item(id).filter(|i| matches!(i.kind, ItemKind::Group { .. })) {
                let f = g.frame;
                ids.extend(self.items_in([f.x as f32, f.y as f32, (f.x + f.w) as f32, (f.y + f.h) as f32]));
            }
        }
        ids.iter().filter_map(|id| self.doc.item(id).cloned()).collect()
    }

    pub fn pointer_move(&mut self, pos: [f32; 2]) {
        let last = self.cursor;
        self.cursor = pos;
        let board = self.camera.screen_to_board(pos, self.viewport);
        match &mut self.gesture {
            Gesture::Idle => return,
            Gesture::Pan => self.camera.pan(pos[0] - last[0], pos[1] - last[1]),
            Gesture::Move { start, originals, delta } => {
                *delta = [(board[0] - start[0]) as f64, (board[1] - start[1]) as f64];
                let (originals, d) = (originals.clone(), *delta);
                self.doc.preview_offset(&originals, d[0], d[1]);
            }
            Gesture::Marquee { start, now, base } => {
                *now = board;
                let rect = norm_rect(*start, *now);
                let mut sel = base.clone();
                sel.extend(self.items_in(rect));
                self.selection = sel;
            }
        }
        self.needs_redraw = true;
    }

    pub fn pointer_up(&mut self, _button: Button, pos: [f32; 2]) {
        self.cursor = pos;
        if let Gesture::Move { originals, delta, .. } = std::mem::replace(&mut self.gesture, Gesture::Idle) {
            self.doc.commit_drag(&originals, delta[0], delta[1]);
        }
        self.gesture = Gesture::Idle;
        self.needs_redraw = true;
    }

    /// Items whose frame lies entirely inside a board rect (x0, y0, x1, y1).
    fn items_in(&self, r: [f32; 4]) -> Vec<Uuid> {
        let r = r.map(|v| v as f64);
        self.doc
            .board
            .live_items()
            .filter(|i| i.frame.x >= r[0] && i.frame.y >= r[1] && i.frame.x + i.frame.w <= r[2] && i.frame.y + i.frame.h <= r[3])
            .map(|i| i.id)
            .collect()
    }

    pub fn wheel(&mut self, w: Wheel) {
        let steps = match w {
            Wheel::Lines(l) => l,
            Wheel::Pixels(p) => p / 100.0,
        };
        self.camera.zoom_at(1.15f32.powf(steps), self.cursor, self.viewport);
        self.needs_redraw = true;
    }

    pub fn key(&mut self, key: Key) {
        match key {
            Key::Delete => {
                let sel = std::mem::take(&mut self.selection);
                self.doc.delete(&sel);
            }
            Key::Undo => {
                self.doc.undo();
                self.prune_selection();
            }
            Key::Redo => {
                self.doc.redo();
                self.prune_selection();
            }
            Key::SelectAll => self.selection = self.doc.board.live_items().map(|i| i.id).collect(),
            Key::Escape => self.selection.clear(),
            Key::Space { pressed } => self.space_held = pressed,
        }
        self.needs_redraw = true;
    }

    fn prune_selection(&mut self) {
        let live: BTreeSet<Uuid> = self.selection.iter().filter(|id| self.doc.item(id).is_some()).copied().collect();
        self.selection = live;
    }

    // ---- drawing -----------------------------------------------------------

    pub fn render(&mut self, renderer: &mut Renderer, size_px: [u32; 2]) {
        self.viewport = [size_px[0] as f32, size_px[1] as f32];
        for pic in self.pictures.values_mut() {
            if let Picture::Pending { rgba, width, height } = pic {
                match renderer.create_image(rgba, *width, *height) {
                    Ok(id) => *pic = Picture::Ready(id),
                    Err(e) => eprintln!("wall: image upload failed: {e}"),
                }
            }
        }
        let textures: HashMap<&ContentHash, ImageId> = self
            .pictures
            .iter()
            .filter_map(|(h, p)| match p {
                Picture::Ready(id) => Some((h, *id)),
                Picture::Pending { .. } => None,
            })
            .collect();

        self.scene.update(&self.doc, &mut self.font);
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        let marquee = match &self.gesture {
            Gesture::Marquee { start, now, .. } => Some(norm_rect(*start, *now)),
            _ => None,
        };
        let overlays = scene::overlays(&self.doc, &self.selection, marquee, self.camera.zoom);

        renderer.begin_frame(size_px, BACKGROUND);
        let view = ViewUniform::new(self.camera.center, self.camera.zoom, self.viewport);
        self.scene.draw(renderer, &view, &textures);
        renderer.draw_boxes(&view, &overlays);
        if self.doc.board.live_items().next().is_none() {
            // Empty board: a hint in the middle of the screen, constant size.
            let mut hint = Vec::new();
            let size = 18.0 / self.camera.zoom;
            let text = "Drop images here";
            let width = self.font.layout_line(text).width * size;
            let c = self.camera.center;
            scene::add_text(&mut self.font, &mut hint, text, [c[0] - width / 2.0, c[1]], size, [0.45, 0.46, 0.5, 1.0]);
            if self.font.atlas.dirty {
                renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
                self.font.atlas.dirty = false;
            }
            renderer.draw_glyphs(&view, &hint);
        }
        self.needs_redraw = false;
    }
}

fn norm_rect(a: [f32; 2], b: [f32; 2]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

/// A colourful placeholder picture, so the demo needs no image files.
fn demo_picture(seed: u32, w: u32, h: u32) -> Vec<u8> {
    let hue = (seed as f32 * 0.137) % 1.0;
    let a = scene::hsv(hue, 0.65, 0.95);
    let b = scene::hsv((hue + 0.35) % 1.0, 0.8, 0.35);
    let (cx, cy, r) = (w as f32 * (0.3 + 0.4 * ((seed * 7 % 10) as f32 / 10.0)), h as f32 * 0.5, h as f32 * 0.32);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = (x as f32 / w as f32 * 0.6 + y as f32 / h as f32 * 0.4).clamp(0.0, 1.0);
            let mut c = [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t];
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let ring = (1.0 - ((d - r).abs() / 3.0)).clamp(0.0, 1.0);
            let disc = (1.0 - (d - r * 0.55).max(0.0) / 1.5).clamp(0.0, 1.0) * 0.35;
            for v in &mut c {
                *v = (*v + ring * 0.8 + disc).min(1.0);
            }
            out.extend_from_slice(&[(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut a = App::new(Board::new("t", 0), "test");
        a.resize([1000.0, 800.0]);
        a.camera = Camera { center: [500.0, 400.0], zoom: 1.0 }; // screen == board
        a
    }

    fn add_note(a: &mut App, x: f64, y: f64) -> Uuid {
        let f = Frame { x, y, w: 100.0, h: 100.0, ..Default::default() };
        a.doc.add(vec![(f, ItemKind::Note { text: "n".into(), color: None })])[0]
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let size = [1000.0, 800.0];
        let mut cam = Camera { center: [50.0, -20.0], zoom: 1.3 };
        let cursor = [870.0, 120.0];
        let before = cam.screen_to_board(cursor, size);
        cam.zoom_at(2.5, cursor, size);
        let after = cam.screen_to_board(cursor, size);
        assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
        assert!((cam.zoom - 3.25).abs() < 1e-5);
        cam.zoom_at(1e9, cursor, size);
        assert_eq!(cam.zoom, MAX_ZOOM);
    }

    #[test]
    fn click_selects_topmost_and_drag_moves_it_in_one_undo() {
        let mut a = app();
        let below = add_note(&mut a, 100.0, 100.0);
        let top = add_note(&mut a, 150.0, 150.0);
        a.pointer_down(Button::Primary, [160.0, 160.0], Mods::default());
        assert_eq!(a.selection, [top].into());
        a.pointer_move([200.0, 170.0]);
        a.pointer_move([260.0, 210.0]);
        a.pointer_up(Button::Primary, [260.0, 210.0]);
        assert_eq!(a.doc.item(&top).unwrap().frame.x, 250.0);
        assert_eq!(a.doc.item(&below).unwrap().frame.x, 100.0);
        a.key(Key::Undo);
        assert_eq!(a.doc.item(&top).unwrap().frame.x, 150.0);
    }

    #[test]
    fn marquee_selects_contained_items_and_shift_adds() {
        let mut a = app();
        let n1 = add_note(&mut a, 100.0, 100.0);
        let n2 = add_note(&mut a, 300.0, 100.0);
        let n3 = add_note(&mut a, 600.0, 100.0);
        a.pointer_down(Button::Primary, [50.0, 50.0], Mods::default());
        a.pointer_move([450.0, 250.0]);
        a.pointer_up(Button::Primary, [450.0, 250.0]);
        assert_eq!(a.selection, [n1, n2].into());
        a.pointer_down(Button::Primary, [650.0, 150.0], Mods { shift: true, ..Default::default() });
        a.pointer_up(Button::Primary, [650.0, 150.0]);
        assert_eq!(a.selection, [n1, n2, n3].into());
        a.pointer_down(Button::Primary, [900.0, 700.0], Mods::default());
        a.pointer_up(Button::Primary, [900.0, 700.0]);
        assert!(a.selection.is_empty(), "click on empty board clears");
    }

    #[test]
    fn delete_then_undo_restores_and_space_drag_pans() {
        let mut a = app();
        let n = add_note(&mut a, 100.0, 100.0);
        a.key(Key::SelectAll);
        a.key(Key::Delete);
        assert_eq!(a.item_count(), 0);
        a.key(Key::Undo);
        assert!(a.doc.item(&n).is_some());

        a.key(Key::Space { pressed: true });
        let c = a.camera.center;
        a.pointer_down(Button::Primary, [150.0, 150.0], Mods::default());
        a.pointer_move([250.0, 150.0]);
        a.pointer_up(Button::Primary, [250.0, 150.0]);
        assert_eq!(a.camera.center, [c[0] - 100.0, c[1]]);
        assert_eq!(a.doc.item(&n).unwrap().frame.x, 100.0, "space-drag doesn't move items");
    }

    #[test]
    fn dragging_a_group_carries_its_contents() {
        let mut a = app();
        let g = Frame { x: 50.0, y: 50.0, w: 400.0, h: 300.0, ..Default::default() };
        let group = a.doc.add(vec![(g, ItemKind::Group { name: "g".into(), color: None })])[0];
        let inside = add_note(&mut a, 100.0, 120.0);
        let outside = add_note(&mut a, 600.0, 120.0);
        a.pointer_down(Button::Primary, [60.0, 60.0], Mods::default());
        assert_eq!(a.selection, [group].into());
        a.pointer_move([80.0, 60.0]);
        a.pointer_up(Button::Primary, [80.0, 60.0]);
        assert_eq!(a.doc.item(&inside).unwrap().frame.x, 120.0);
        assert_eq!(a.doc.item(&outside).unwrap().frame.x, 600.0);
    }

    #[test]
    fn dropped_picture_lands_at_cursor_selected() {
        let mut a = app();
        a.pointer_move([300.0, 200.0]);
        let id = a.place_picture(vec![255; 40 * 20 * 4], 40, 20);
        let f = a.doc.item(&id).unwrap().frame;
        assert_eq!((f.x, f.y, f.w, f.h), (280.0, 190.0, 40.0, 20.0));
        assert_eq!(a.selection, [id].into());
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 90, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn library_round_trip_import_save_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Refs.wall");
        let mut a = App::open_library(&root, "dev", 1).unwrap();
        a.resize([1000.0, 800.0]);
        assert_eq!(a.doc.board.name, "Board 1");
        let id = a.import_image_bytes(&png(3000, 1000), Some("big.png"), 5).unwrap();
        let f = a.doc.item(&id).unwrap().frame;
        assert_eq!((f.w, f.h), (2048.0, 683.0), "wide image scaled to 2048 on import");
        assert!(a.save_if_dirty().unwrap());
        assert!(!a.save_if_dirty().unwrap(), "nothing new to save");

        let b = App::open_library(&root, "dev", 9).unwrap();
        assert_eq!(b.doc.board.id, a.doc.board.id);
        let item = b.doc.item(&id).expect("item persisted");
        let ItemKind::Media { hash, .. } = &item.kind else { panic!("not media") };
        assert!(b.pictures.contains_key(hash), "picture reloaded from the library");
        let lib = Library::open(&root).unwrap();
        let sc = lib.read_sidecar(hash).unwrap();
        assert_eq!((sc.width, sc.original.as_ref().unwrap().width), (2048, 3000));
    }

    #[test]
    fn demo_board_has_pictures_notes_and_groups() {
        let a = App::demo(20);
        let items: Vec<_> = a.doc.board.live_items().collect();
        assert_eq!(items.iter().filter(|i| matches!(i.kind, ItemKind::Media { .. })).count(), 24);
        assert_eq!(items.iter().filter(|i| matches!(i.kind, ItemKind::Note { .. })).count(), 20);
        assert_eq!(items.iter().filter(|i| matches!(i.kind, ItemKind::Group { .. })).count(), 2);
    }
}
