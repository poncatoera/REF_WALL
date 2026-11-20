//! The app logic shared by desktop and web: board document, camera, input,
//! selection, and turning the board into GPU instances.
//! Shells (apps/desktop, apps/web) only create the window or canvas, forward
//! input here and call [`App::render`].

pub mod doc;
mod scene;
pub mod video;

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;
use wall_format::{ArrowEnd, Board, BoardItem, Frame, ItemKind, Rect, ItemSidecar, Library, MediaKind};
use wall_render::{ImageId, Renderer, ViewUniform};
use wall_text::Font;

pub use doc::Document;
pub use video::{VideoCommand, VideoStatus};
pub use wall_format::ContentHash;
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
    /// Delete key: removes the selection, or the character after the caret.
    Delete,
    /// Backspace: removes the selection, or the character before the caret.
    Backspace,
    /// Starts editing the selected text item; in a note, a line break.
    Enter,
    Left,
    Right,
    Home,
    End,
    /// Ctrl/Cmd+D: duplicate the selection.
    Duplicate,
    /// Ctrl/Cmd+]: bring the selection to the front.
    BringToFront,
    /// Ctrl/Cmd+[: send the selection to the back.
    SendToBack,
    /// A plain character key with no Ctrl/Cmd, while not typing. Video
    /// shortcuts: K play/pause, `,` `.` frame step, `[` `]` speed.
    Char(char),
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
    /// Alt-dragging out a new arrow.
    Connect { from: ArrowEnd, start: [f32; 2], now: [f32; 2] },
    /// Ctrl/Cmd-dragging a corner of a picture: crops instead of scaling.
    Crop { original: BoardItem, start: [f32; 2], left: bool, top: bool },
    /// Dragging an item's bottom-right handle.
    Resize { original: BoardItem, start: [f32; 2], aspect: Option<f64> },
    /// Dragging along a video's scrub strip.
    Scrub { hash: ContentHash, x: f32, w: f32, duration: f64 },
}

/// Text being typed into an item. The item shows the live text through
/// `Document::preview`; leaving the edit records one undo step.
#[derive(Debug, Clone)]
struct Editing {
    id: Uuid,
    /// Byte offset in the item's text.
    caret: usize,
    /// The item before editing, or None if this edit created it.
    original: Option<BoardItem>,
}

/// The editable text of an item, if it has one.
fn item_text(kind: &ItemKind) -> Option<&str> {
    match kind {
        ItemKind::Note { text, .. } | ItemKind::Label { text } => Some(text),
        ItemKind::Todo { title, .. } => Some(title),
        ItemKind::Group { name, .. } => Some(name),
        _ => None,
    }
}

fn item_text_mut(kind: &mut ItemKind) -> Option<&mut String> {
    match kind {
        ItemKind::Note { text, .. } | ItemKind::Label { text } => Some(text),
        ItemKind::Todo { title, .. } => Some(title),
        ItemKind::Group { name, .. } => Some(name),
        _ => None,
    }
}

const DOUBLE_CLICK_MS: u64 = 450;
/// Half the size of the resize handle, in screen pixels.
const HANDLE_PX: f32 = 8.0;
/// Prefix of Wall items on the system clipboard.
const CLIPBOARD_TAG: &str = "wall-items:";
/// Videos narrower than this on screen show a still frame instead of playing.
pub const MIN_PLAY_PX: f32 = 120.0;
/// Height of a group's name strip, in board units.
const GROUP_HEADER: f64 = 44.0;

/// Decoded pixels for a media hash, waiting for upload or already on the GPU.
enum Picture {
    Pending { rgba: Vec<u8>, width: u32, height: u32 },
    Ready(ImageId),
}

/// What a shell measured about a video file before importing it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMeta {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub fps: Option<f32>,
}

/// A video on the board that has no texture yet; the shell starts a player
/// for it and hands back a texture with [`App::attach_texture`].
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSource {
    pub hash: ContentHash,
    /// The file in the open library, if there is one.
    pub path: Option<std::path::PathBuf>,
    pub width: u32,
    pub height: u32,
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
    editing: Option<Editing>,
    last_click: Option<(u64, [f32; 2])>,
    now_ms: u64,
    /// Video hashes and their file extension and size.
    videos: HashMap<ContentHash, (String, u32, u32)>,
    video_status: HashMap<ContentHash, VideoStatus>,
    video_commands: Vec<(ContentHash, VideoCommand)>,
    /// Fit the view to the board once the real window size is known.
    fit_on_resize: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Format(#[from] wall_format::Error),
    #[error(transparent)]
    Media(#[from] wall_media::MediaError),
    #[error("could not decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("video is {0} bytes; convert it under the 20 MB limit first")]
    VideoTooLarge(u64),
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
            editing: None,
            last_click: None,
            now_ms: 0,
            videos: HashMap::new(),
            video_status: HashMap::new(),
            video_commands: Vec::new(),
            fit_on_resize: false,
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
        app.fit_on_resize = true;
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
            if let Ok(sc) = library.read_sidecar(&hash) {
                if sc.kind == MediaKind::Video {
                    self.videos.insert(hash, (sc.ext, sc.width, sc.height));
                    continue;
                }
            }
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

    /// Imports a video file's bytes (already under the size limit; shells
    /// convert larger files first) and drops it at the cursor. Its frames
    /// come from a player the shell runs; see [`Self::video_sources`].
    pub fn import_video(&mut self, bytes: &[u8], ext: &str, meta: VideoMeta, source: Option<&str>, now_ms: u64) -> Result<Uuid, AppError> {
        let limits = wall_media::Limits::default();
        if bytes.len() as u64 > limits.max_video_bytes {
            return Err(AppError::VideoTooLarge(bytes.len() as u64));
        }
        let hash = ContentHash::of(bytes);
        if let Some(lib) = &self.library {
            let known = lib.has_media(&hash);
            lib.import_bytes(bytes, ext)?;
            if !known {
                lib.write_sidecar(&ItemSidecar {
                    hash: hash.clone(),
                    kind: MediaKind::Video,
                    ext: ext.into(),
                    width: meta.width,
                    height: meta.height,
                    bytes: bytes.len() as u64,
                    duration_ms: Some(meta.duration_ms),
                    fps: meta.fps,
                    added_ms: now_ms,
                    title: source.map(|s| s.to_owned()),
                    source_url: None,
                    tags: Vec::new(),
                    auto_tags: Vec::new(),
                    colors: Vec::new(),
                    original: None,
                    extra: Default::default(),
                })?;
            }
        }
        self.videos.insert(hash.clone(), (ext.to_owned(), meta.width, meta.height));
        Ok(self.place_media(hash, meta.width, meta.height))
    }

    /// Videos on the board that still need a player and texture.
    pub fn video_sources(&self) -> Vec<VideoSource> {
        let on_board: BTreeSet<&ContentHash> = self
            .doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Media { hash, .. } => Some(hash),
                _ => None,
            })
            .collect();
        self.videos
            .iter()
            .filter(|(h, _)| on_board.contains(h) && !self.pictures.contains_key(*h))
            .map(|(h, (ext, w, h2))| VideoSource {
                hash: h.clone(),
                path: self.library.as_ref().map(|l| l.media_path(h, ext)),
                width: *w,
                height: *h2,
            })
            .collect()
    }

    /// The media hash of a picture or video item.
    pub fn media_hash(&self, id: &Uuid) -> Option<ContentHash> {
        match &self.doc.item(id)?.kind {
            ItemKind::Media { hash, .. } => Some(hash.clone()),
            _ => None,
        }
    }

    // ---- video controls ----------------------------------------------------

    /// The video hash of an item, if it is a video.
    fn video_of(&self, id: &Uuid) -> Option<ContentHash> {
        self.media_hash(id).filter(|h| self.videos.contains_key(h))
    }

    /// Selected videos.
    fn selected_videos(&self) -> Vec<ContentHash> {
        self.selection.iter().filter_map(|id| self.video_of(id)).collect()
    }

    /// If a board point is on the scrub strip of a selected video: its hash,
    /// frame x, frame width and duration.
    fn scrub_target(&self, id: Uuid, board: [f32; 2]) -> Option<(ContentHash, f32, f32, f64)> {
        if !self.selection.contains(&id) {
            return None;
        }
        let hash = self.video_of(&id)?;
        let status = self.video_status.get(&hash)?;
        let f = self.doc.item(&id)?.frame;
        let strip = video::SCRUB_PX / self.camera.zoom;
        let bottom = (f.y + f.h) as f32;
        (board[1] >= bottom - strip && status.duration > 0.0).then_some((hash, f.x as f32, f.w as f32, status.duration))
    }

    fn toggle_play(&mut self, hash: ContentHash) {
        let playing = self.video_status.get(&hash).is_none_or(|s| s.playing);
        self.video_commands.push((hash, if playing { VideoCommand::Pause } else { VideoCommand::Play }));
        self.needs_redraw = true;
    }

    fn video_key(&mut self, c: char) {
        for hash in self.selected_videos() {
            let speed = self.video_status.get(&hash).map_or(1.0, |s| s.speed);
            let cmd = match c.to_ascii_lowercase() {
                'k' => {
                    self.toggle_play(hash);
                    continue;
                }
                ',' => VideoCommand::Step(-1),
                '.' => VideoCommand::Step(1),
                '[' => VideoCommand::Speed(video::next_speed(speed, false)),
                ']' => VideoCommand::Speed(video::next_speed(speed, true)),
                _ => continue,
            };
            self.video_commands.push((hash, cmd));
        }
    }

    /// Videos worth decoding now: on screen and at least `MIN_PLAY_PX` wide.
    /// Shells pause or stop the others and keep their last frame.
    pub fn playable_videos(&self) -> BTreeSet<ContentHash> {
        let tl = self.camera.screen_to_board([0.0, 0.0], self.viewport);
        let br = self.camera.screen_to_board(self.viewport, self.viewport);
        self.doc
            .board
            .live_items()
            .filter(|i| {
                let f = i.frame;
                (f.w as f32) * self.camera.zoom >= MIN_PLAY_PX
                    && (f.x as f32) < br[0]
                    && ((f.x + f.w) as f32) > tl[0]
                    && (f.y as f32) < br[1]
                    && ((f.y + f.h) as f32) > tl[1]
            })
            .filter_map(|i| match &i.kind {
                ItemKind::Media { hash, .. } if self.videos.contains_key(hash) => Some(hash.clone()),
                _ => None,
            })
            .collect()
    }

    /// Commands for the shell's players, oldest first.
    pub fn take_video_commands(&mut self) -> Vec<(ContentHash, VideoCommand)> {
        std::mem::take(&mut self.video_commands)
    }

    /// A player's state, reported by the shell each frame.
    pub fn set_video_status(&mut self, hash: ContentHash, status: VideoStatus) {
        if self.video_status.get(&hash) != Some(&status) {
            self.video_status.insert(hash, status);
            self.needs_redraw = true;
        }
    }

    /// Scrub bars for selected videos: track, played part and playhead.
    fn scrub_bars(&self) -> Vec<wall_render::BoxInstance> {
        let px = 1.0 / self.camera.zoom;
        let mut out = Vec::new();
        for id in &self.selection {
            let (Some(hash), Some(item)) = (self.video_of(id), self.doc.item(id)) else { continue };
            let Some(st) = self.video_status.get(&hash) else { continue };
            let f = item.frame;
            let (x, w) = (f.x as f32 + 8.0 * px, f.w as f32 - 16.0 * px);
            let y = (f.y + f.h) as f32 - 12.0 * px;
            let t = if st.duration > 0.0 { (st.time / st.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
            let h = 4.0 * px;
            out.push(wall_render::BoxInstance { rect: [x, y, w, h], fill: [0.0, 0.0, 0.0, 0.55], stroke: [0.0; 4], shape: [2.0 * px, 0.0] });
            out.push(wall_render::BoxInstance { rect: [x, y, w * t, h], fill: [0.36, 0.62, 1.0, 1.0], stroke: [0.0; 4], shape: [2.0 * px, 0.0] });
            out.push(wall_render::BoxInstance { rect: [x + w * t - 5.0 * px, y - 3.0 * px, 10.0 * px, 10.0 * px], fill: [1.0; 4], stroke: [0.0; 4], shape: [5.0 * px, 0.0] });
            if !st.playing {
                // Pause sign at the top-left corner.
                for dx in [0.0, 9.0] {
                    out.push(wall_render::BoxInstance {
                        rect: [f.x as f32 + (12.0 + dx) * px, f.y as f32 + 12.0 * px, 5.0 * px, 18.0 * px],
                        fill: [1.0, 1.0, 1.0, 0.9],
                        stroke: [0.0, 0.0, 0.0, 0.5],
                        shape: [1.5 * px, px],
                    });
                }
            }
        }
        out
    }

    /// Gives a picture or video a texture the shell created and fills.
    pub fn attach_texture(&mut self, hash: ContentHash, id: ImageId) {
        self.pictures.insert(hash, Picture::Ready(id));
        self.needs_redraw = true;
    }

    /// Writes the board to the library if it changed and no drag is in
    /// progress. Returns true when it saved.
    pub fn save_if_dirty(&mut self) -> Result<bool, AppError> {
        let Some(lib) = &self.library else { return Ok(false) };
        if self.saved_version == self.doc.version || !matches!(self.gesture, Gesture::Idle) || self.editing.is_some() {
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
        // At the cursor, or the middle of the view if the pointer was never
        // over the window (e.g. files imported at start-up).
        let c = self.cursor;
        let inside = c != [0.0, 0.0] && c[0] >= 0.0 && c[1] >= 0.0 && c[0] <= self.viewport[0] && c[1] <= self.viewport[1];
        let at = if inside { self.camera.screen_to_board(c, self.viewport) } else { self.camera.center };
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
        if std::mem::take(&mut self.fit_on_resize) {
            self.fit_view();
        }
    }

    /// Topmost live item under a board point.
    pub fn hit(&self, p: [f32; 2]) -> Option<Uuid> {
        // Arrows are thin: anything within a few pixels of one picks it first.
        let tol = 6.0 / self.camera.zoom;
        let arrow = self
            .doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Arrow { from, to } => scene::arrow_points(&self.doc, from, to)
                    .filter(|pts| scene::segment_distance(p, pts[0], pts[1]) <= tol)
                    .map(|_| i),
                _ => None,
            })
            .max_by(|a, b| a.frame.z.total_cmp(&b.frame.z));
        if let Some(a) = arrow {
            return Some(a.id);
        }
        self.hit_item(p)
    }

    /// Topmost live item (not arrow) under a board point.
    fn hit_item(&self, p: [f32; 2]) -> Option<Uuid> {
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
        let hit = self.hit(board);
        let double = self
            .last_click
            .is_some_and(|(t, p)| self.now_ms.saturating_sub(t) <= DOUBLE_CLICK_MS && (p[0] - pos[0]).abs() < 6.0 && (p[1] - pos[1]).abs() < 6.0);
        self.last_click = Some((self.now_ms, pos));
        if let Some(e) = &self.editing {
            if hit == Some(e.id) {
                self.place_caret(board);
                return;
            }
            self.end_edit();
        }
        if mods.alt {
            // Alt-drag draws an arrow, from an item or from empty space.
            let from = match self.hit_item(board) {
                Some(id) => ArrowEnd::Item { id },
                None => ArrowEnd::Point { x: board[0] as f64, y: board[1] as f64 },
            };
            self.gesture = Gesture::Connect { from, start: board, now: board };
            return;
        }
        if mods.command {
            if let Some((item, left, top)) = self.crop_corner_at(board) {
                self.gesture = Gesture::Crop { original: item, start: board, left, top };
                return;
            }
        }
        if let Some(item) = self.resize_handle_at(board) {
            // Pictures and videos keep their shape unless Shift is held.
            let aspect = matches!(item.kind, ItemKind::Media { .. }).then(|| item.frame.w / item.frame.h.max(1e-6)).filter(|_| !mods.shift);
            self.gesture = Gesture::Resize { original: item, start: board, aspect };
            return;
        }
        if let Some((hash, x, w, duration)) = hit.and_then(|id| self.scrub_target(id, board)) {
            self.selection = [hit.unwrap()].into();
            self.video_commands.push((hash.clone(), VideoCommand::Seek(video::time_at(board[0], x, w, duration))));
            self.gesture = Gesture::Scrub { hash, x, w, duration };
            return;
        }
        if double && !mods.shift {
            if let Some(hash) = hit.and_then(|id| self.video_of(&id)) {
                self.toggle_play(hash);
                self.last_click = None;
                return;
            }
            // Inside a group, only the header edits its name; the rest of
            // the group is space for new notes.
            let in_group_body = hit
                .and_then(|id| self.doc.item(&id))
                .is_some_and(|i| matches!(i.kind, ItemKind::Group { .. }) && (board[1] as f64) > i.frame.y + GROUP_HEADER);
            match hit {
                Some(id) if !in_group_body && self.doc.item(&id).is_some_and(|i| item_text(&i.kind).is_some()) => self.begin_edit(id),
                Some(_) if !in_group_body => {}
                _ => {
                    self.new_note(board, String::new());
                }
            }
            self.last_click = None;
            return;
        }
        match hit {
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
            Gesture::Connect { now, .. } => *now = board,
            Gesture::Crop { original, start, left, top } => {
                let item = crop_item(original, [(board[0] - start[0]) as f64, (board[1] - start[1]) as f64], *left, *top);
                self.doc.preview(item);
            }
            Gesture::Resize { original, start, aspect } => {
                let mut item = original.clone();
                let min = 20.0;
                item.frame.w = (original.frame.w + (board[0] - start[0]) as f64).max(min);
                item.frame.h = (original.frame.h + (board[1] - start[1]) as f64).max(min);
                if let Some(a) = *aspect {
                    // Follow whichever side moved more, keeping the shape.
                    if item.frame.w / a > item.frame.h {
                        item.frame.h = item.frame.w / a;
                    } else {
                        item.frame.w = item.frame.h * a;
                    }
                }
                self.doc.preview(item);
            }
            Gesture::Scrub { hash, x, w, duration } => {
                let cmd = VideoCommand::Seek(video::time_at(board[0], *x, *w, *duration));
                // Only the newest seek per frame matters.
                self.video_commands.retain(|(h, c)| !(h == hash && matches!(c, VideoCommand::Seek(_))));
                self.video_commands.push((hash.clone(), cmd));
            }
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
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Move { originals, delta, .. } => {
                self.doc.commit_drag(&originals, delta[0], delta[1]);
            }
            Gesture::Connect { from, start, now } => {
                let board = self.camera.screen_to_board(pos, self.viewport);
                let _ = now;
                let to = match self.hit_item(board) {
                    Some(id) if from != (ArrowEnd::Item { id }) => Some(ArrowEnd::Item { id }),
                    Some(_) => None, // released on the item it started from
                    None => {
                        let far = ((board[0] - start[0]).powi(2) + (board[1] - start[1]).powi(2)).sqrt() * self.camera.zoom > 12.0;
                        far.then_some(ArrowEnd::Point { x: board[0] as f64, y: board[1] as f64 })
                    }
                };
                if let Some(to) = to {
                    let frame = Frame { x: start[0].min(board[0]) as f64, y: start[1].min(board[1]) as f64, w: (board[0] - start[0]).abs() as f64, h: (board[1] - start[1]).abs() as f64, ..Default::default() };
                    let id = self.doc.add(vec![(frame, ItemKind::Arrow { from, to })])[0];
                    self.selection = [id].into();
                }
            }
            Gesture::Crop { original, .. } | Gesture::Resize { original, .. } => {
                if let Some(after) = self.doc.board.items.get(&original.id).cloned() {
                    self.doc.commit_edit(Some(&original), after);
                }
            }
            _ => {}
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
        self.needs_redraw = true;
        if self.editing.is_some() {
            match key {
                Key::Escape => self.end_edit(),
                Key::Backspace => self.edit(|t, c| {
                    if let Some(prev) = t[..*c].chars().next_back() {
                        *c -= prev.len_utf8();
                        t.remove(*c);
                    }
                }),
                Key::Delete => self.edit(|t, c| {
                    if *c < t.len() {
                        t.remove(*c);
                    }
                }),
                Key::Enter if self.editing_multiline() => {
                    self.text_input("\n");
                }
                Key::Enter => self.end_edit(),
                Key::Left => self.move_caret(|t, c| t[..c].chars().next_back().map_or(c, |ch| c - ch.len_utf8())),
                Key::Right => self.move_caret(|t, c| t[c..].chars().next().map_or(c, |ch| c + ch.len_utf8())),
                Key::Home => self.move_caret(|t, c| t[..c].rfind('\n').map_or(0, |i| i + 1)),
                Key::End => self.move_caret(|t, c| t[c..].find('\n').map_or(t.len(), |i| c + i)),
                Key::Undo | Key::Redo => {
                    self.end_edit();
                    self.key(key);
                }
                Key::SelectAll | Key::Space { .. } | Key::Duplicate | Key::BringToFront | Key::SendToBack => {}
                Key::Char(c) => {
                    self.text_input(&c.to_string());
                }
            }
            return;
        }
        match key {
            Key::Delete | Key::Backspace => {
                let mut sel = std::mem::take(&mut self.selection);
                // Arrows attached to deleted items go with them.
                let attached: Vec<Uuid> = self
                    .doc
                    .board
                    .live_items()
                    .filter(|i| match &i.kind {
                        ItemKind::Arrow { from, to } => [from, to].iter().any(|e| matches!(e, ArrowEnd::Item { id } if sel.contains(id))),
                        _ => false,
                    })
                    .map(|i| i.id)
                    .collect();
                sel.extend(attached);
                self.doc.delete(&sel);
            }
            Key::Enter => {
                if let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] {
                    if self.doc.item(&id).is_some_and(|i| item_text(&i.kind).is_some()) {
                        self.begin_edit(id);
                    }
                }
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
            Key::Left | Key::Right | Key::Home | Key::End => {}
            Key::Char(c @ '0'..='7') => self.set_color(c),
            Key::Char(c) => self.video_key(c),
            Key::Duplicate => self.duplicate(),
            Key::BringToFront => self.restack(true),
            Key::SendToBack => self.restack(false),
        }
    }

    /// Selected arrows highlighted, and the arrow being drawn.
    fn overlay_curves(&self) -> Vec<wall_render::CurveInstance> {
        let accent = [0.36, 0.62, 1.0, 1.0];
        let mut out: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| match &self.doc.item(id)?.kind {
                ItemKind::Arrow { from, to } => scene::arrow_points(&self.doc, from, to),
                _ => None,
            })
            .map(|pts| scene::arrow_curve(pts, accent, scene::ARROW_WIDTH + 1.0))
            .collect();
        if let Gesture::Connect { from, now, .. } = &self.gesture {
            let to = ArrowEnd::Point { x: now[0] as f64, y: now[1] as f64 };
            if let Some(pts) = scene::arrow_points(&self.doc, from, &to) {
                out.push(scene::arrow_curve(pts, accent, scene::ARROW_WIDTH));
            }
        }
        out
    }

    // ---- arranging ---------------------------------------------------------

    /// The single selected item, if its resize handle is under a board point.
    fn resize_handle_at(&self, board: [f32; 2]) -> Option<BoardItem> {
        let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] else { return None };
        let item = self.doc.item(&id)?;
        let r = HANDLE_PX / self.camera.zoom;
        let (hx, hy) = ((item.frame.x + item.frame.w) as f32, (item.frame.y + item.frame.h) as f32);
        ((board[0] - hx).abs() <= r && (board[1] - hy).abs() <= r).then(|| item.clone())
    }

    /// A corner of the single selected picture under a board point:
    /// (item, is left corner, is top corner).
    fn crop_corner_at(&self, board: [f32; 2]) -> Option<(BoardItem, bool, bool)> {
        let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] else { return None };
        let item = self.doc.item(&id).filter(|i| matches!(i.kind, ItemKind::Media { .. }))?;
        let r = (HANDLE_PX * 1.5 / self.camera.zoom) as f64;
        let f = item.frame;
        let (x, y) = (board[0] as f64, board[1] as f64);
        let left = (x - f.x).abs() <= r;
        let right = (x - (f.x + f.w)).abs() <= r;
        let top = (y - f.y).abs() <= r;
        let bottom = (y - (f.y + f.h)).abs() <= r;
        ((left || right) && (top || bottom)).then(|| (item.clone(), left, top))
    }

    /// Number keys colour the selected notes and groups: 0 resets, 1-7 pick
    /// red, orange, yellow, green, blue, purple, grey. One undo step.
    fn set_color(&mut self, key: char) {
        const PALETTE: [&str; 7] = ["#e5534b", "#e8a15d", "#e6c84f", "#57ab5a", "#7aa2f7", "#b083f0", "#8b8f98"];
        let color = key.to_digit(10).filter(|d| *d > 0).map(|d| PALETTE[d as usize - 1].to_owned());
        let afters: Vec<BoardItem> = self
            .selected_items()
            .into_iter()
            .filter_map(|mut i| {
                match &mut i.kind {
                    ItemKind::Note { color: c, .. } | ItemKind::Group { color: c, .. } => *c = color.clone(),
                    _ => return None,
                }
                Some(i)
            })
            .collect();
        self.doc.replace(afters);
    }

    /// Selected items, ordered back to front.
    fn selected_items(&self) -> Vec<BoardItem> {
        let mut items: Vec<BoardItem> = self.selection.iter().filter_map(|id| self.doc.item(id).cloned()).collect();
        items.sort_by(|a, b| a.frame.z.total_cmp(&b.frame.z));
        items
    }

    /// Moves the selection above (or below) everything else, keeping its
    /// own stacking order. One undo step.
    fn restack(&mut self, front: bool) {
        let items = self.selected_items();
        let n = items.len() as f64;
        let base = if front {
            self.doc.top_z() + 1.0
        } else {
            self.doc.board.live_items().map(|i| i.frame.z).fold(f64::MAX, f64::min).min(0.0) - n
        };
        let afters: Vec<BoardItem> = items
            .into_iter()
            .enumerate()
            .map(|(k, mut i)| {
                i.frame.z = base + k as f64;
                i
            })
            .collect();
        self.doc.replace(afters);
    }

    /// Copies of the selection (with group contents), offset a little, selected.
    fn duplicate(&mut self) {
        let items = self.drag_set();
        let ids = self.paste_items(items, [24.0, 24.0]);
        self.selection = ids.into_iter().collect();
    }

    /// Adds copies of `items` with new ids, moved by `offset`, on top.
    fn paste_items(&mut self, mut items: Vec<BoardItem>, offset: [f64; 2]) -> Vec<Uuid> {
        items.sort_by(|a, b| a.frame.z.total_cmp(&b.frame.z));
        let kinds = items
            .into_iter()
            .map(|i| {
                let mut f = i.frame;
                f.x += offset[0];
                f.y += offset[1];
                (f, i.kind)
            })
            .collect();
        self.needs_redraw = true;
        self.doc.add(kinds)
    }

    /// The selection as text for the system clipboard. Other apps see JSON;
    /// pasting it back into Wall recreates the items.
    pub fn copy_selection(&self) -> Option<String> {
        let items = self.drag_set();
        if items.is_empty() {
            return None;
        }
        let json = serde_json::to_string(&items).ok()?;
        Some(format!("{CLIPBOARD_TAG}{json}"))
    }

    // ---- text editing ------------------------------------------------------

    /// Wall-clock time for double clicks and `updated_ms`; shells call this
    /// before forwarding each event.
    pub fn set_time(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.doc.set_clock(now_ms);
    }

    /// True while the keyboard types into an item; shells then send typed
    /// characters to [`Self::text_input`] instead of shortcuts like Space.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Starts typing into an item's text, caret at the end.
    pub fn begin_edit(&mut self, id: Uuid) {
        self.end_edit();
        let Some(item) = self.doc.item(&id) else { return };
        let Some(text) = item_text(&item.kind) else { return };
        self.editing = Some(Editing { id, caret: text.len(), original: Some(item.clone()) });
        self.selection = [id].into();
        self.needs_redraw = true;
    }

    /// Finishes typing: records the edit as one undo step. A new note left
    /// empty is dropped.
    pub fn end_edit(&mut self) {
        let Some(e) = self.editing.take() else { return };
        self.needs_redraw = true;
        let Some(item) = self.doc.board.items.get(&e.id).cloned() else { return };
        let empty = item_text(&item.kind).is_some_and(|t| t.trim().is_empty());
        if e.original.is_none() && empty {
            self.doc.discard_preview(&e.id);
            self.selection.remove(&e.id);
            return;
        }
        self.doc.commit_edit(e.original.as_ref(), item);
    }

    /// Creates a note at a board point and starts typing into it.
    pub fn new_note(&mut self, at: [f32; 2], text: String) -> Uuid {
        self.end_edit();
        let frame = Frame { x: at[0] as f64 - 20.0, y: at[1] as f64 - 20.0, w: 240.0, h: 72.0, z: self.doc.top_z() + 1.0, ..Default::default() };
        let id = Uuid::new_v4();
        let caret = text.len();
        let item = BoardItem { id, frame, kind: ItemKind::Note { text, color: None }, rev: 0, updated_ms: 0, updated_by: String::new(), deleted: false };
        self.doc.preview(item);
        self.editing = Some(Editing { id, caret, original: None });
        self.selection = [id].into();
        self.edit(|_, _| {}); // grows the frame to fit pasted text
        self.needs_redraw = true;
        id
    }

    /// Types text at the caret. Returns false when nothing is being edited.
    pub fn text_input(&mut self, s: &str) -> bool {
        if self.editing.is_none() {
            return false;
        }
        let multiline = self.editing_multiline();
        let clean: String = s
            .chars()
            .map(|c| if c == '\r' || (c == '\n' && !multiline) || c == '\t' { ' ' } else { c })
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        if !clean.is_empty() {
            self.edit(|t, c| {
                t.insert_str(*c, &clean);
                *c += clean.len();
            });
        }
        true
    }

    /// Pasted text: typed at the caret, or a new note at the cursor.
    pub fn paste_text(&mut self, s: &str) {
        if let Some(json) = s.strip_prefix(CLIPBOARD_TAG).filter(|_| self.editing.is_none()) {
            if let Ok(items) = serde_json::from_str::<Vec<BoardItem>>(json) {
                // Centre the pasted items on the cursor.
                let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for i in &items {
                    (x0, y0) = (x0.min(i.frame.x), y0.min(i.frame.y));
                    (x1, y1) = (x1.max(i.frame.x + i.frame.w), y1.max(i.frame.y + i.frame.h));
                }
                let at = self.camera.screen_to_board(self.cursor, self.viewport);
                let offset = [at[0] as f64 - (x0 + x1) / 2.0, at[1] as f64 - (y0 + y1) / 2.0];
                let ids = self.paste_items(items, offset);
                self.selection = ids.into_iter().collect();
                return;
            }
        }
        if self.text_input(s) {
            return;
        }
        let text = s.replace("\r\n", "\n");
        if text.trim().is_empty() {
            return;
        }
        let at = self.camera.screen_to_board(self.cursor, self.viewport);
        let id = self.new_note(at, text);
        let _ = id;
        self.end_edit();
    }

    fn editing_multiline(&self) -> bool {
        self.editing
            .as_ref()
            .and_then(|e| self.doc.item(&e.id))
            .and_then(|i| scene::TextStyle::of(&i.kind, i.frame.w as f32))
            .is_some_and(|s| s.multiline)
    }

    /// Applies a change to the edited text and caret, growing the frame so
    /// all of the text shows.
    fn edit(&mut self, f: impl FnOnce(&mut String, &mut usize)) {
        let Some(e) = self.editing.as_mut() else { return };
        let Some(mut item) = self.doc.board.items.get(&e.id).cloned() else { return };
        let Some(text) = item_text_mut(&mut item.kind) else { return };
        e.caret = e.caret.min(text.len());
        f(text, &mut e.caret);
        let text = text.clone();
        if let Some(style) = scene::TextStyle::of(&item.kind, item.frame.w as f32) {
            match item.kind {
                ItemKind::Note { .. } | ItemKind::Todo { .. } => {
                    let need = (style.height_for(&mut self.font, &text) + 16.0) as f64;
                    let min = e.original.as_ref().map_or(72.0, |o| o.frame.h);
                    item.frame.h = need.max(min);
                }
                ItemKind::Label { .. } => {
                    let w = (self.font.layout_line(&text).width * style.size + style.at[0]) as f64;
                    item.frame.w = item.frame.w.max(w);
                }
                _ => {}
            }
        }
        self.doc.preview(item);
        self.needs_redraw = true;
    }

    fn move_caret(&mut self, f: impl FnOnce(&str, usize) -> usize) {
        let Some(e) = self.editing.as_mut() else { return };
        let Some(text) = self.doc.item(&e.id).and_then(|i| item_text(&i.kind)) else { return };
        e.caret = f(text, e.caret.min(text.len()));
    }

    /// Moves the caret to the character nearest a board point.
    fn place_caret(&mut self, board: [f32; 2]) {
        let Some(e) = self.editing.as_mut() else { return };
        let Some(item) = self.doc.item(&e.id) else { return };
        let (Some(text), Some(style)) = (item_text(&item.kind), scene::TextStyle::of(&item.kind, item.frame.w as f32)) else { return };
        let local = [board[0] - item.frame.x as f32, board[1] - item.frame.y as f32];
        e.caret = style.hit(&mut self.font, text, style.max_lines(item.frame.h as f32), local);
    }

    /// The caret as a board rect, while editing.
    fn caret_rect(&mut self) -> Option<[f32; 4]> {
        let e = self.editing.as_ref()?;
        let item = self.doc.item(&e.id)?;
        let text = item_text(&item.kind)?;
        let style = scene::TextStyle::of(&item.kind, item.frame.w as f32)?;
        let r = style.caret(&mut self.font, text, style.max_lines(item.frame.h as f32), e.caret.min(text.len()));
        Some([item.frame.x as f32 + r[0], item.frame.y as f32 + r[1], 1.5 / self.camera.zoom, r[3]])
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
        let caret = self.caret_rect();
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
        let mut overlays = scene::overlays(&self.doc, &self.selection, marquee, self.camera.zoom);
        overlays.extend(self.scrub_bars());
        if let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] {
            if let (Some(item), None) = (self.doc.item(&id), &self.editing) {
                let px = 1.0 / self.camera.zoom;
                let (hx, hy) = ((item.frame.x + item.frame.w) as f32, (item.frame.y + item.frame.h) as f32);
                overlays.push(wall_render::BoxInstance {
                    rect: [hx - 5.0 * px, hy - 5.0 * px, 10.0 * px, 10.0 * px],
                    fill: [1.0; 4],
                    stroke: [0.36, 0.62, 1.0, 1.0],
                    shape: [2.0 * px, 1.5 * px],
                });
            }
        }
        if let Some(r) = caret {
            overlays.push(wall_render::BoxInstance { rect: r, fill: [0.36, 0.62, 1.0, 1.0], stroke: [0.0; 4], shape: [0.0, 0.0] });
        }

        renderer.begin_frame(size_px, BACKGROUND);
        let view = ViewUniform::new(self.camera.center, self.camera.zoom, self.viewport);
        self.scene.draw(renderer, &view, &textures);
        renderer.draw_boxes(&view, &overlays);
        renderer.draw_curves(&view, &self.overlay_curves());
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

/// A picture with one corner moved by `d` board units, cropping: the image
/// keeps its scale on the board and only the visible part changes.
fn crop_item(original: &BoardItem, d: [f64; 2], left: bool, top: bool) -> BoardItem {
    let mut item = original.clone();
    let ItemKind::Media { crop, .. } = &mut item.kind else { return item };
    let c = crop.unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
    let f = original.frame;
    // Board units per texture unit, fixed while cropping.
    let (sx, sy) = (f.w / c.w, f.h / c.h);
    let min = 10.0;
    let (mut x0, mut y0) = (f.x, f.y);
    let (mut x1, mut y1) = (f.x + f.w, f.y + f.h);
    // Limits: the full image's edges on the board.
    let (full_x0, full_y0) = (f.x - c.x * sx, f.y - c.y * sy);
    let (full_x1, full_y1) = (full_x0 + sx, full_y0 + sy);
    if left {
        x0 = (x0 + d[0]).clamp(full_x0, x1 - min);
    } else {
        x1 = (x1 + d[0]).clamp(x0 + min, full_x1);
    }
    if top {
        y0 = (y0 + d[1]).clamp(full_y0, y1 - min);
    } else {
        y1 = (y1 + d[1]).clamp(y0 + min, full_y1);
    }
    item.frame.x = x0;
    item.frame.y = y0;
    item.frame.w = x1 - x0;
    item.frame.h = y1 - y0;
    let new = Rect { x: (x0 - full_x0) / sx, y: (y0 - full_y0) / sy, w: (x1 - x0) / sx, h: (y1 - y0) / sy };
    let whole = new.x.abs() < 1e-9 && new.y.abs() < 1e-9 && (new.w - 1.0).abs() < 1e-9 && (new.h - 1.0).abs() < 1e-9;
    *crop = (!whole).then_some(new);
    item
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

    fn note_text(a: &App, id: &Uuid) -> String {
        item_text(&a.doc.item(id).unwrap().kind).unwrap().to_owned()
    }

    fn click(a: &mut App, t: u64, p: [f32; 2]) {
        a.set_time(t);
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
    }

    #[test]
    fn double_click_empty_space_makes_a_note_you_type_into() {
        let mut a = app();
        click(&mut a, 1000, [300.0, 300.0]);
        click(&mut a, 1200, [301.0, 300.0]);
        assert!(a.is_editing());
        let id = *a.selection.iter().next().unwrap();
        a.text_input("Hello");
        a.key(Key::Enter);
        a.text_input("wörld");
        a.key(Key::Backspace);
        a.key(Key::Left);
        a.key(Key::Backspace);
        assert_eq!(note_text(&a, &id), "Hello\nwöl");
        // Not saved or recorded while typing; one undo step after.
        assert!(!a.doc.can_undo());
        a.key(Key::Escape);
        assert!(!a.is_editing());
        assert_eq!(note_text(&a, &id), "Hello\nwöl");
        a.key(Key::Undo);
        assert!(a.doc.item(&id).is_none());
        a.key(Key::Redo);
        assert_eq!(note_text(&a, &id), "Hello\nwöl");
    }

    #[test]
    fn double_click_in_a_group_body_adds_a_note_and_on_its_header_renames() {
        let mut a = app();
        let f = Frame { x: 100.0, y: 100.0, w: 600.0, h: 400.0, ..Default::default() };
        let g = a.doc.add(vec![(f, ItemKind::Group { name: "Refs".into(), color: None })])[0];
        click(&mut a, 1000, [400.0, 300.0]);
        click(&mut a, 1100, [400.0, 300.0]);
        let id = *a.selection.iter().next().unwrap();
        assert_ne!(id, g);
        a.text_input("x");
        a.key(Key::Escape);
        click(&mut a, 3000, [150.0, 120.0]);
        click(&mut a, 3100, [150.0, 120.0]);
        a.text_input("!");
        a.key(Key::Enter);
        assert!(!a.is_editing());
        assert_eq!(note_text(&a, &g), "Refs!");
    }

    #[test]
    fn videos_import_into_the_library_and_ask_for_a_player() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("V.wall");
        let mut a = App::open_library(&root, "test", 1).unwrap();
        a.resize([1000.0, 800.0]);
        let bytes = b"not really an mp4, but the app only stores it".to_vec();
        let meta = VideoMeta { width: 640, height: 360, duration_ms: 4000, fps: Some(25.0) };
        let id = a.import_video(&bytes, "mp4", meta, Some("clip.mp4"), 5).unwrap();
        assert_eq!(a.doc.item(&id).unwrap().frame.w, 640.0);
        let src = a.video_sources();
        assert_eq!(src.len(), 1);
        let path = src[0].path.clone().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        a.save_if_dirty().unwrap();
        // Reopened, the video still asks for a player and is not decoded as an image.
        let b = App::open_library(&root, "test", 2).unwrap();
        assert_eq!(b.video_sources(), src);
        let big = vec![0u8; 20_000_001];
        assert!(matches!(a.import_video(&big, "mp4", meta, None, 6), Err(AppError::VideoTooLarge(_))));
    }

    #[test]
    fn video_shortcuts_and_scrubbing_queue_commands() {
        let mut a = app();
        let meta = VideoMeta { width: 400, height: 200, duration_ms: 8000, fps: Some(25.0) };
        a.pointer_move([500.0, 400.0]);
        let id = a.import_video(b"clip", "mp4", meta, None, 1).unwrap();
        let hash = a.media_hash(&id).unwrap();
        a.set_video_status(hash.clone(), VideoStatus { time: 1.0, duration: 8.0, playing: true, speed: 1.0 });
        a.key(Key::Char('k'));
        a.key(Key::Char(']'));
        a.key(Key::Char('.'));
        let cmds: Vec<VideoCommand> = a.take_video_commands().into_iter().map(|(_, c)| c).collect();
        assert_eq!(cmds, vec![VideoCommand::Pause, VideoCommand::Speed(1.5), VideoCommand::Step(1)]);
        // The video spans x 300..700, y 300..500. Drag along its bottom strip.
        a.set_time(10_000);
        a.pointer_down(Button::Primary, [400.0, 495.0], Mods::default());
        a.pointer_move([450.0, 495.0]);
        a.pointer_move([500.0, 495.0]);
        a.pointer_up(Button::Primary, [500.0, 495.0]);
        let cmds: Vec<VideoCommand> = a.take_video_commands().into_iter().map(|(_, c)| c).collect();
        // Seeks not yet taken by the player collapse into the newest.
        assert_eq!(cmds, vec![VideoCommand::Seek(4.0)]);
        assert_eq!(a.doc.item(&id).unwrap().frame.x, 300.0, "scrubbing must not move the video");
        // Double-click in the middle toggles playback.
        a.set_video_status(hash, VideoStatus { time: 4.0, duration: 8.0, playing: false, speed: 1.5 });
        click(&mut a, 20_000, [500.0, 350.0]);
        click(&mut a, 20_100, [500.0, 350.0]);
        assert_eq!(a.take_video_commands().last().map(|(_, c)| *c), Some(VideoCommand::Play));
    }

    #[test]
    fn only_videos_on_screen_and_big_enough_play() {
        let mut a = app();
        let meta = VideoMeta { width: 400, height: 200, duration_ms: 1000, fps: None };
        a.pointer_move([500.0, 400.0]);
        let on = a.import_video(b"a", "mp4", meta, None, 1).unwrap();
        let off = a.import_video(b"b", "mp4", meta, None, 1).unwrap();
        a.doc.translate(&[off].into(), 5000.0, 0.0);
        let (on, off) = (a.media_hash(&on).unwrap(), a.media_hash(&off).unwrap());
        assert_eq!(a.playable_videos(), [on.clone()].into());
        assert!(!a.playable_videos().contains(&off));
        a.camera.zoom = 0.2; // 400 units -> 80 px: too small to play
        assert!(a.playable_videos().is_empty());
    }

    #[test]
    fn resize_handle_keeps_picture_shape_and_undoes() {
        let mut a = app();
        let f = Frame { x: 100.0, y: 100.0, w: 200.0, h: 100.0, ..Default::default() };
        let hash = a.add_picture(vec![0; 4], 1, 1);
        let id = a.doc.add(vec![(f, ItemKind::Media { hash, crop: None })])[0];
        a.selection = [id].into();
        a.pointer_down(Button::Primary, [300.0, 200.0], Mods::default());
        a.pointer_move([500.0, 210.0]);
        a.pointer_up(Button::Primary, [500.0, 210.0]);
        let fr = a.doc.item(&id).unwrap().frame;
        assert_eq!((fr.x, fr.w, fr.h), (100.0, 400.0, 200.0));
        a.key(Key::Undo);
        assert_eq!(a.doc.item(&id).unwrap().frame.w, 200.0);
        // Notes resize freely.
        let n = add_note(&mut a, 600.0, 100.0);
        a.selection = [n].into();
        a.pointer_down(Button::Primary, [700.0, 200.0], Mods::default());
        a.pointer_move([760.0, 400.0]);
        a.pointer_up(Button::Primary, [760.0, 400.0]);
        let fr = a.doc.item(&n).unwrap().frame;
        assert_eq!((fr.w, fr.h), (160.0, 300.0));
    }

    #[test]
    fn restack_duplicate_and_copy_paste() {
        let mut a = app();
        let n1 = add_note(&mut a, 100.0, 100.0);
        let n2 = add_note(&mut a, 120.0, 120.0);
        a.selection = [n1].into();
        a.key(Key::BringToFront);
        assert!(a.doc.item(&n1).unwrap().frame.z > a.doc.item(&n2).unwrap().frame.z);
        a.key(Key::SendToBack);
        assert!(a.doc.item(&n1).unwrap().frame.z < a.doc.item(&n2).unwrap().frame.z);
        a.key(Key::Duplicate);
        assert_eq!(a.item_count(), 3);
        let dup = *a.selection.iter().next().unwrap();
        assert_ne!(dup, n1);
        assert_eq!(a.doc.item(&dup).unwrap().frame.x, 124.0);
        let clip = a.copy_selection().unwrap();
        assert!(clip.starts_with("wall-items:"));
        a.pointer_move([700.0, 600.0]);
        a.paste_text(&clip);
        assert_eq!(a.item_count(), 4);
        let pasted = a.doc.item(a.selection.iter().next().unwrap()).unwrap().frame;
        assert_eq!((pasted.x + pasted.w / 2.0, pasted.y + pasted.h / 2.0), (700.0, 600.0));
        a.key(Key::Undo);
        assert_eq!(a.item_count(), 3);
    }

    #[test]
    fn draw_order_follows_z_with_few_batches() {
        let mut a = App::demo(400);
        a.scene.update(&a.doc, &mut a.font);
        assert_eq!(a.scene.run_count(), 1, "nothing overlaps in the demo grid: one batch");
        // A note dragged over a picture, then the picture brought forward:
        // the picture must draw after (above) the note.
        let mut b = app();
        let hash = b.add_picture(vec![0; 4], 1, 1);
        let f = Frame { x: 100.0, y: 100.0, w: 200.0, h: 100.0, ..Default::default() };
        let pic = b.doc.add(vec![(f, ItemKind::Media { hash, crop: None })])[0];
        add_note(&mut b, 150.0, 120.0);
        b.selection = [pic].into();
        b.key(Key::BringToFront);
        b.scene.update(&b.doc, &mut b.font);
        assert_eq!(b.scene.run_count(), 2);
    }

    #[test]
    fn dragged_items_come_to_the_front() {
        let mut a = app();
        let low = add_note(&mut a, 100.0, 100.0);
        let high = add_note(&mut a, 400.0, 100.0);
        a.pointer_down(Button::Primary, [150.0, 150.0], Mods::default());
        a.pointer_move([420.0, 150.0]);
        a.pointer_up(Button::Primary, [420.0, 150.0]);
        assert!(a.doc.item(&low).unwrap().frame.z > a.doc.item(&high).unwrap().frame.z);
        a.key(Key::Undo);
        assert!(a.doc.item(&low).unwrap().frame.z < a.doc.item(&high).unwrap().frame.z);
    }

    #[test]
    fn alt_drag_connects_items_and_arrows_follow_and_delete_with_them() {
        let mut a = app();
        let n1 = add_note(&mut a, 100.0, 100.0);
        let n2 = add_note(&mut a, 500.0, 100.0);
        let alt = Mods { alt: true, ..Default::default() };
        a.pointer_down(Button::Primary, [150.0, 150.0], alt);
        a.pointer_move([300.0, 150.0]);
        a.pointer_move([550.0, 150.0]);
        a.pointer_up(Button::Primary, [550.0, 150.0]);
        let arrow = *a.selection.iter().next().unwrap();
        let kind = a.doc.item(&arrow).unwrap().kind.clone();
        assert_eq!(kind, ItemKind::Arrow { from: ArrowEnd::Item { id: n1 }, to: ArrowEnd::Item { id: n2 } });
        // Clicking near the line picks the arrow; the notes are unchanged.
        assert_eq!(a.hit([350.0, 152.0]), Some(arrow));
        assert_eq!(a.hit([150.0, 120.0]), Some(n1));
        // Drawing it: the scene ends the arrow at the note edges.
        let pts = scene::arrow_points(&a.doc, &ArrowEnd::Item { id: n1 }, &ArrowEnd::Item { id: n2 }).unwrap();
        assert!(pts[0][0] > 200.0 && pts[1][0] < 500.0, "{pts:?}");
        // Deleting a note takes the arrow with it.
        a.selection = [n2].into();
        a.key(Key::Delete);
        assert!(a.doc.item(&arrow).is_none());
        a.key(Key::Undo);
        assert!(a.doc.item(&arrow).is_some());
        // Alt-drag from empty space to empty space makes a free arrow that moves.
        a.pointer_down(Button::Primary, [100.0, 600.0], alt);
        a.pointer_up(Button::Primary, [300.0, 650.0]);
        let free = *a.selection.iter().next().unwrap();
        a.pointer_down(Button::Primary, [200.0, 625.0], Mods::default());
        a.pointer_move([250.0, 625.0]);
        a.pointer_up(Button::Primary, [250.0, 625.0]);
        assert_eq!(a.doc.item(&free).unwrap().kind, ItemKind::Arrow { from: ArrowEnd::Point { x: 150.0, y: 600.0 }, to: ArrowEnd::Point { x: 350.0, y: 650.0 } });
    }

    #[test]
    fn ctrl_drag_corner_crops_without_rescaling() {
        let mut a = app();
        let f = Frame { x: 100.0, y: 100.0, w: 400.0, h: 200.0, ..Default::default() };
        let hash = a.add_picture(vec![0; 4], 1, 1);
        let id = a.doc.add(vec![(f, ItemKind::Media { hash, crop: None })])[0];
        a.selection = [id].into();
        let cmd = Mods { command: true, ..Default::default() };
        // Top-left corner in by (100, 50): a quarter of the width, a quarter of the height.
        a.pointer_down(Button::Primary, [100.0, 100.0], cmd);
        a.pointer_move([200.0, 150.0]);
        a.pointer_up(Button::Primary, [200.0, 150.0]);
        let item = a.doc.item(&id).unwrap().clone();
        assert_eq!((item.frame.x, item.frame.y, item.frame.w, item.frame.h), (200.0, 150.0, 300.0, 150.0));
        let ItemKind::Media { crop: Some(c), .. } = item.kind else { panic!("no crop") };
        assert_eq!((c.x, c.y, c.w, c.h), (0.25, 0.25, 0.75, 0.75));
        // Pulling the corner back past the image edge stops at the edge and clears the crop.
        a.pointer_down(Button::Primary, [200.0, 150.0], cmd);
        a.pointer_move([0.0, 0.0]);
        a.pointer_up(Button::Primary, [0.0, 0.0]);
        let item = a.doc.item(&id).unwrap();
        assert_eq!((item.frame.x, item.frame.w), (100.0, 400.0));
        assert!(matches!(item.kind, ItemKind::Media { crop: None, .. }));
        a.key(Key::Undo);
        a.key(Key::Undo);
        let back = a.doc.item(&id).unwrap().frame;
        assert_eq!((back.x, back.y, back.w, back.h), (f.x, f.y, f.w, f.h));
    }

    #[test]
    fn number_keys_colour_notes() {
        let mut a = app();
        let n = add_note(&mut a, 100.0, 100.0);
        a.selection = [n].into();
        a.key(Key::Char('5'));
        assert!(matches!(&a.doc.item(&n).unwrap().kind, ItemKind::Note { color: Some(c), .. } if c == "#7aa2f7"));
        a.key(Key::Char('0'));
        assert!(matches!(&a.doc.item(&n).unwrap().kind, ItemKind::Note { color: None, .. }));
        a.key(Key::Undo);
        assert!(matches!(&a.doc.item(&n).unwrap().kind, ItemKind::Note { color: Some(_), .. }));
    }

    #[test]
    fn empty_new_note_disappears_and_slow_clicks_do_not_edit() {
        let mut a = app();
        click(&mut a, 1000, [300.0, 300.0]);
        click(&mut a, 2000, [300.0, 300.0]);
        assert!(!a.is_editing());
        click(&mut a, 2100, [300.0, 300.0]);
        assert!(a.is_editing());
        a.key(Key::Escape);
        assert_eq!(a.item_count(), 0);
        assert!(!a.doc.can_undo());
    }

    #[test]
    fn editing_an_existing_note_grows_it_and_undoes_in_one_step() {
        let mut a = app();
        let id = add_note(&mut a, 100.0, 100.0);
        a.selection = [id].into();
        a.key(Key::Enter);
        assert!(a.is_editing());
        for _ in 0..12 {
            a.text_input(" more words to wrap");
        }
        assert!(a.doc.item(&id).unwrap().frame.h > 100.0);
        // Clicking elsewhere ends the edit.
        click(&mut a, 5000, [900.0, 700.0]);
        assert!(!a.is_editing());
        a.key(Key::Undo);
        assert_eq!(note_text(&a, &id), "n");
        assert_eq!(a.doc.item(&id).unwrap().frame.h, 100.0);
    }

    #[test]
    fn pasted_text_becomes_a_note_at_the_cursor() {
        let mut a = app();
        a.pointer_move([400.0, 300.0]);
        a.paste_text("from the clipboard\r\nline two");
        assert_eq!(a.item_count(), 1);
        let id = *a.selection.iter().next().unwrap();
        assert_eq!(note_text(&a, &id), "from the clipboard\nline two");
        assert!(!a.is_editing());
        a.key(Key::Undo);
        assert_eq!(a.item_count(), 0);
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
