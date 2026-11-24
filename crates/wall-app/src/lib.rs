//! The app logic shared by desktop and web: board document, camera, input,
//! selection, and turning the board into GPU instances.
//! Shells (apps/desktop, apps/web) only create the window or canvas, forward
//! input here and call [`App::render`].

pub mod arrange;
pub mod brush;
pub mod share;
pub mod history;
pub mod comments;
pub mod unread;
pub mod export;
pub mod presence;
pub mod doc;
mod drawing;
pub mod menu;
pub mod panel;
mod library_view;
mod picture_loader;
mod scene;
mod snap;
pub mod theme;
pub mod sidebar;
pub mod toolbar;
pub mod welcome;
mod zoom_pill;
mod grid;
pub mod video;

use std::collections::{BTreeSet, HashMap, HashSet};

use uuid::Uuid;
use wall_format::{ArrowEnd, Board, BoardItem, Frame, ItemKind, Rect, ItemSidecar, Library, MediaKind};
use wall_render::{ImageId, Renderer, ViewUniform};
use wall_text::Font;

pub use doc::Document;
mod snapshot;
pub mod sync;
mod batch;
pub mod collect;
pub use batch::grid_layout;
pub use snapshot::NewMedia;
pub use menu::ShellCommand;
pub use share::{CloudAnswer, CloudAsk, ShareLink};
pub use history::{DeletedBoard, HistoryAnswer, HistoryAsk};
pub use presence::Peer;
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
    /// Ctrl/Cmd+F: search the board.
    Find,
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
    /// `snap` is None while Ctrl/Cmd was held at the start (snapping off);
    /// `guides` are the alignment lines currently shown.
    Move { start: [f32; 2], originals: Vec<BoardItem>, delta: [f64; 2], snap: Option<snap::Snap>, guides: Vec<snap::Guide> },
    Marquee { start: [f32; 2], now: [f32; 2], base: BTreeSet<Uuid> },
    /// Alt-dragging out a new arrow.
    Connect { from: ArrowEnd, start: [f32; 2], now: [f32; 2] },
    /// Ctrl/Cmd-dragging a corner of a picture: crops instead of scaling.
    Crop { original: BoardItem, start: [f32; 2], left: bool, top: bool },
    /// Dragging an item's bottom-right handle.
    /// `left`/`top`: which corner is held; the opposite one stays put.
    Resize { original: BoardItem, start: [f32; 2], aspect: Option<f64>, attached: Vec<BoardItem>, left: bool, top: bool },
    /// Dragging along a video's scrub strip.
    Scrub { hash: ContentHash, x: f32, w: f32, duration: f64 },
    /// Dragging a tool out of the rail; released on the board, it makes
    /// that thing there (released on the rail, in the middle of the view).
    RailDrag { tool: toolbar::Tool },
    /// Moving the tool rail by its handle.
    RailMove,
    /// A board row in the left panel pressed: a click opens it, a drag
    /// reorders it.
    SideRow { id: Uuid, start: [f32; 2], moved: bool },
    /// Drawing: the line so far in board units, pressure at each point
    /// (from speed), the smoothed speed, when the last point came, and
    /// whether Shift makes it a straight line.
    Draw { points: Vec<[f32; 2]>, pressures: Vec<f32>, velocity: f32, last_ms: u64, straight: bool },
    /// Rubbing out; `depth` is the undo depth at the start, so the whole
    /// drag becomes one undo step.
    Erase { depth: usize, last: [f32; 2] },
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
    /// A to-do list being edited as text: the title on the first line, one
    /// entry per line after it. It turns back into a to-do card when done.
    todo: bool,
}

/// The editable text of an item, if it has one.
fn item_text(kind: &ItemKind) -> Option<&str> {
    match kind {
        ItemKind::Note { text, .. } | ItemKind::Label { text } => Some(text),
        ItemKind::Todo { title, .. } => Some(title),
        ItemKind::Group { name, .. } => Some(name),
        ItemKind::Link { url, .. } => Some(url),
        _ => None,
    }
}

fn item_text_mut(kind: &mut ItemKind) -> Option<&mut String> {
    match kind {
        ItemKind::Note { text, .. } | ItemKind::Label { text } => Some(text),
        ItemKind::Todo { title, .. } => Some(title),
        ItemKind::Group { name, .. } => Some(name),
        ItemKind::Link { url, .. } => Some(url),
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
/// Videos decoding at once, at most (the old Wall used 16 too). Each is an
/// ffmpeg process on desktop and a `<video>` element on the web.
pub const MAX_PLAYING: usize = 16;
/// Height of a group's name strip, in board units.
const GROUP_HEADER: f64 = 44.0;

/// A media hash's pixels: not loaded yet, decoded and waiting for upload, a
/// small preview on the GPU, or full size on the GPU (also video textures).
/// See `picture_loader` for when each loads.
enum Picture {
    /// In the library; loads when it comes on screen.
    Unloaded,
    Pending { rgba: Vec<u8>, width: u32, height: u32 },
    /// A preview (at most `picture_loader::PREVIEW_PX`); full size loads
    /// when the picture is shown larger.
    Preview(ImageId),
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
    /// Videos showing onion skin (not saved: a study aid for now).
    onion: HashSet<ContentHash>,
    /// Fit the view to the board once the real window size is known.
    fit_on_resize: bool,
    /// Boards opened before (with their undo history and view), by id.
    parked: HashMap<Uuid, (Document, Camera)>,
    /// Items moved to other boards: undo on the board they left also takes
    /// the copies off the other board (and redo puts them back).
    moved_out: Vec<sidebar::MovedOut>,
    /// Names of every known board, for board cards and the breadcrumb.
    board_names: HashMap<Uuid, String>,
    /// The boards above the open one, outermost first.
    nav: Vec<Uuid>,
    /// The search box, while open.
    search: Option<Search>,
    /// What Wall knows about each picture and video: title, tags, size.
    sidecars: HashMap<ContentHash, ItemSidecar>,
    /// The side panel shows the selection's details (I toggles it).
    panel_open: bool,
    /// Typing into a side panel field.
    panel_edit: Option<panel::FieldEdit>,
    /// Show only items with this tag (others are dimmed).
    tag_filter: Option<String>,
    /// The menu dropped down from the menu bar, if any.
    menu_open: Option<usize>,
    /// Menu commands waiting for the shell (file picker, clipboard).
    shell_commands: Vec<menu::ShellCommand>,
    /// The share card, while open.
    share: Option<share::ShareCard>,
    /// Other people on this board (see `presence`).
    peers: Vec<presence::Peer>,
    /// The pointer left the window since it last moved.
    cursor_out: bool,
    /// Our name on shared boards, when there's a live connection.
    my_name: Option<String>,
    /// Which comments this person has seen (see `unread`).
    seen: unread::Seen,
    /// Typing a new name for ourselves.
    name_edit: Option<String>,
    my_color: Option<u8>,
    comment_card: Option<comments::CommentCard>,
    /// Which mark the Stamp tool puts down.
    stamp_mark: usize,
    /// A view-only link: only comments and stamps are kept.
    view_only: bool,
    /// Our name comes with the link we opened.
    name_fixed: bool,
    /// Set by the desktop shell: shows the menu entries only it can carry out.
    pub desktop: bool,
    /// The library drawer, while open (L).
    library_view: Option<library_view::LibraryView>,
    /// Dots on the board (G or View > Show grid).
    show_grid: bool,
    /// The rail's current tool (Select, or Arrow while drawing arrows).
    tool: toolbar::Tool,
    /// Where the tool rail floats (shells remember it).
    rail_place: toolbar::Placement,
    /// Drawing tool settings (brush, colour, sizes).
    pen: brush::PenState,
    /// The boards and library panel on the left, while open.
    sidebar: Option<sidebar::Sidebar>,
    /// Every board's parent, dates and thumbnail picture.
    board_meta: HashMap<Uuid, sidebar::BoardMeta>,
    /// Saved order of boards among their sisters.
    board_order: Vec<Uuid>,
    /// Thumbnails of boards' pictures that aren't loaded on the board.
    board_thumbs: HashMap<ContentHash, library_view::Thumb>,
    /// Loads pictures as they come on screen (previews, then full size).
    loader: picture_loader::Loader,
    /// Media imported with no library open, for the shell to store.
    new_media: Vec<snapshot::NewMedia>,
    /// Versions kept, and the history card (see `history`).
    history: history::History,
    /// The first-time walkthrough, while it shows (see `welcome`).
    welcome: Option<welcome::Welcome>,
    /// Which walkthroughs were seen (`welcome::SEEN_*` bits, in the settings).
    welcome_seen: u8,
}

/// Find on the board: matches update as you type, Enter steps through them.
#[derive(Debug, Clone, Default)]
struct Search {
    query: String,
    matches: Vec<Uuid>,
    /// Index into `matches` of the one shown, if any.
    current: Option<usize>,
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
    #[error("{0}")]
    Other(String),
}

/// The board colour of the current theme.
#[allow(non_snake_case)]
fn BACKGROUND() -> [f32; 4] {
    theme::t().background
}

impl App {
    pub fn new(board: Board, device: &str) -> Self {
        let (board_id, board_name) = (board.id, board.name.clone());
        let root_meta = sidebar::BoardMeta::of(&board);
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
            onion: HashSet::new(),
            fit_on_resize: false,
            parked: HashMap::new(),
            moved_out: Vec::new(),
            board_names: [(board_id, board_name)].into(),
            nav: Vec::new(),
            search: None,
            sidecars: HashMap::new(),
            panel_open: true,
            panel_edit: None,
            tag_filter: None,
            menu_open: None,
            shell_commands: Vec::new(),
            share: None,
            peers: Vec::new(),
            cursor_out: false,
            my_name: None,
            seen: unread::Seen::default(),
            name_edit: None,
            my_color: None,
            comment_card: None,
            stamp_mark: 0,
            view_only: false,
            name_fixed: false,
            desktop: false,
            library_view: None,
            show_grid: true,
            tool: toolbar::Tool::Select,
            rail_place: toolbar::Placement::default(),
            pen: brush::PenState::default(),
            sidebar: None,
            board_meta: [(board_id, root_meta)].into(),
            board_order: Vec::new(),
            board_thumbs: HashMap::new(),
            loader: Default::default(),
            new_media: Vec::new(),
            history: Default::default(),
            welcome: None,
            welcome_seen: 0,
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
        let names: HashMap<Uuid, String> = boards.iter().map(|b| (b.id, b.name.clone())).collect();
        let meta: HashMap<Uuid, sidebar::BoardMeta> = boards.iter().map(|b| (b.id, sidebar::BoardMeta::of(b))).collect();
        let board = match boards.into_iter().find(|b| b.parent.is_none()) {
            Some(b) => b,
            None => {
                let b = Board::new("Board 1", now_ms);
                library.write_board(&b)?;
                b
            }
        };
        let mut app = Self::new(board, device);
        app.board_names.extend(names);
        app.board_meta.extend(meta);
        app.board_order = library.manifest().board_order.clone();
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
                self.sidecars.insert(hash.clone(), sc.clone());
                if sc.kind == MediaKind::Video {
                    self.videos.insert(hash, (sc.ext, sc.width, sc.height));
                    continue;
                }
            }
            // Decoded later, when on screen, at the size it's shown
            // (picture_loader); opening a big board stays instant.
            if self.sidecars.contains_key(&hash) {
                self.pictures.insert(hash, Picture::Unloaded);
            } else {
                eprintln!("wall: picture {hash} is missing from the library");
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
        // Fit in the part of the window the left panel leaves free.
        let inset = self.left_inset();
        let zoom = ((self.viewport[0] - inset) / (w + 80.0)).min(self.viewport[1] / (h + 80.0)).clamp(MIN_ZOOM, 1.0);
        self.camera = Camera { center: [((b[0] + b[2]) / 2.0) as f32 - inset / 2.0 / zoom, ((b[1] + b[3]) / 2.0) as f32], zoom };
        self.needs_redraw = true;
    }

    /// Imports an image file's bytes: applies the 2048 px rule, stores it in
    /// the library (if one is open), and drops it at the cursor.
    pub fn import_image_bytes(&mut self, bytes: &[u8], source: Option<&str>, now_ms: u64) -> Result<Uuid, AppError> {
        let prepared = wall_media::prepare_image(bytes, &wall_media::Limits::default())?;
        let rgba = image::load_from_memory(&prepared.bytes)?.to_rgba8();
        let hash = ContentHash::of(&prepared.bytes);
        let mut sidecar = ItemSidecar {
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
            range: None,
            markers: Vec::new(),
            pingpong: false,
            extra: Default::default(),
        };
        auto_tag(&mut sidecar, Some(rgba.as_raw()));
        if let Some(lib) = &self.library {
            let known = lib.has_media(&hash);
            lib.import_bytes(&prepared.bytes, prepared.ext)?;
            match known.then(|| lib.read_sidecar(&hash).ok()).flatten() {
                Some(existing) => sidecar = existing,
                None => lib.write_sidecar(&sidecar)?,
            }
        }
        if self.library.is_none() {
            self.new_media.push(snapshot::NewMedia { hash: hash.clone(), ext: prepared.ext.into(), bytes: prepared.bytes.clone() });
        }
        self.sidecars.entry(hash.clone()).or_insert(sidecar);
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
        let mut sidecar = ItemSidecar {
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
            range: None,
            markers: Vec::new(),
            pingpong: false,
            extra: Default::default(),
        };
        auto_tag(&mut sidecar, None);
        if let Some(lib) = &self.library {
            let known = lib.has_media(&hash);
            lib.import_bytes(bytes, ext)?;
            match known.then(|| lib.read_sidecar(&hash).ok()).flatten() {
                Some(existing) => sidecar = existing,
                None => lib.write_sidecar(&sidecar)?,
            }
        }
        if self.library.is_none() {
            self.new_media.push(snapshot::NewMedia { hash: hash.clone(), ext: ext.into(), bytes: bytes.to_vec() });
        }
        self.videos.insert(hash.clone(), (ext.to_owned(), meta.width, meta.height));
        self.sidecars.entry(hash.clone()).or_insert(sidecar);
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
                c @ ('i' | 'o') => {
                    let Some(st) = self.video_status.get(&hash).copied() else { continue };
                    let range = video::mark(self.video_range(&hash), st.time, st.duration, c == 'i');
                    self.set_video_range(&hash, range);
                    continue;
                }
                'j' => {
                    let on = !self.video_pingpong(&hash);
                    self.update_sidecar(&hash, |sc| sc.pingpong = on);
                    VideoCommand::PingPong(on)
                }
                'u' => {
                    let on = !self.onion.remove(&hash);
                    if on {
                        self.onion.insert(hash.clone());
                    }
                    VideoCommand::Onion(on)
                }
                'm' => {
                    let Some(st) = self.video_status.get(&hash).copied() else { continue };
                    let markers = video::toggle_marker(&self.video_markers(&hash), st.time);
                    self.update_sidecar(&hash, |sc| sc.markers = markers);
                    continue;
                }
                c @ (';' | '\'') => {
                    let Some(st) = self.video_status.get(&hash).copied() else { continue };
                    match video::next_marker(&self.video_markers(&hash), st.time, c == '\'') {
                        Some(t) => VideoCommand::Seek(t),
                        None => continue,
                    }
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

    /// Videos worth decoding now: on screen and at least `MIN_PLAY_PX` wide,
    /// at most [`MAX_PLAYING`] of them (selected ones first, then the
    /// largest on screen). Shells pause or stop the others and keep their
    /// last frame.
    pub fn playable_videos(&self) -> BTreeSet<ContentHash> {
        let tl = self.camera.screen_to_board([0.0, 0.0], self.viewport);
        let br = self.camera.screen_to_board(self.viewport, self.viewport);
        let mut on_screen: Vec<(bool, f64, &ContentHash)> = self
            .doc
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
                ItemKind::Media { hash, .. } if self.videos.contains_key(hash) => Some((self.selection.contains(&i.id), i.frame.w * i.frame.h, hash)),
                _ => None,
            })
            .collect();
        on_screen.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)));
        let mut out = BTreeSet::new();
        for (_, _, hash) in on_screen {
            if out.len() == MAX_PLAYING {
                break;
            }
            out.insert(hash.clone());
        }
        out
    }

    /// How many screen pixels wide a video is shown (its largest copy on the
    /// board), so shells can decode it at a matching size.
    pub fn video_shown_px(&self, hash: &ContentHash) -> f32 {
        self.doc
            .board
            .live_items()
            .filter(|i| matches!(&i.kind, ItemKind::Media { hash: h, .. } if h == hash))
            .map(|i| i.frame.w as f32 * self.camera.zoom)
            .fold(0.0, f32::max)
    }

    /// A frame of a video (its poster or first played frame): picks the
    /// video's main colours and colour tags if it has none yet.
    pub fn video_frame_seen(&mut self, hash: &ContentHash, rgba: &[u8], width: u32, height: u32) {
        if !self.sidecars.get(hash).is_some_and(|s| s.colors.is_empty()) {
            return;
        }
        let colors = wall_media::autotag::dominant_colors(rgba, width, height, 4);
        if colors.is_empty() {
            return;
        }
        self.update_sidecar(hash, |sc| {
            sc.colors = colors;
            auto_tag(sc, None);
        });
    }

    /// A video's in and out points in seconds, if it has them.
    pub fn video_range(&self, hash: &ContentHash) -> Option<(f64, f64)> {
        self.sidecars.get(hash)?.range.map(|r| (r.in_ms as f64 / 1000.0, r.out_ms as f64 / 1000.0))
    }

    /// True when a video shows onion skin.
    pub fn video_onion(&self, hash: &ContentHash) -> bool {
        self.onion.contains(hash)
    }

    /// True when a video loops back and forth.
    pub fn video_pingpong(&self, hash: &ContentHash) -> bool {
        self.sidecars.get(hash).is_some_and(|s| s.pingpong)
    }

    /// A video's markers, in milliseconds.
    pub fn video_markers(&self, hash: &ContentHash) -> Vec<video::Marker> {
        self.sidecars.get(hash).map(|s| s.markers.clone()).unwrap_or_default()
    }

    /// Sets (or clears) a video's in and out points, saves them and tells
    /// the player.
    pub fn set_video_range(&mut self, hash: &ContentHash, range: Option<(f64, f64)>) {
        let r = range.map(|(a, b)| wall_format::ClipRange { in_ms: (a * 1000.0).round() as u64, out_ms: (b * 1000.0).round() as u64 });
        self.update_sidecar(hash, |sc| sc.range = r);
        self.video_commands.push((hash.clone(), VideoCommand::Range(range)));
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
            // A dark strip behind the controls so they read on any frame.
            out.push(wall_render::BoxInstance { rect: [x - 6.0 * px, y - 10.0 * px, w + 12.0 * px, 24.0 * px], fill: [0.0, 0.0, 0.0, 0.5], stroke: [0.0; 4], shape: [theme::radius::INNER * px, 0.0] });
            out.push(wall_render::BoxInstance { rect: [x, y, w, h], fill: [0.22, 0.22, 0.22, 0.22], stroke: [0.0; 4], shape: [2.0 * px, 0.0] });
            if let Some((a, b)) = self.video_range(&hash).filter(|_| st.duration > 0.0) {
                // The in-out range: a band above the track with end ticks.
                let (xa, xb) = (x + w * (a / st.duration).clamp(0.0, 1.0) as f32, x + w * (b / st.duration).clamp(0.0, 1.0) as f32);
                let band = [0.98, 0.78, 0.3, 1.0];
                out.push(wall_render::BoxInstance { rect: [xa, y - 5.0 * px, xb - xa, 3.0 * px], fill: band, stroke: [0.0; 4], shape: [px, 0.0] });
                for xe in [xa, xb] {
                    out.push(wall_render::BoxInstance { rect: [xe - px, y - 7.0 * px, 2.0 * px, 13.0 * px], fill: band, stroke: [0.0; 4], shape: [0.0, 0.0] });
                }
            }
            if st.duration > 0.0 {
                // Markers: small pink ticks through the track.
                for m in self.video_markers(&hash) {
                    let xm = x + w * ((m.ms as f64 / 1000.0) / st.duration).clamp(0.0, 1.0) as f32;
                    out.push(wall_render::BoxInstance { rect: [xm - px, y - 6.0 * px, 2.0 * px, 16.0 * px], fill: [1.0, 0.36, 0.62, 1.0], stroke: [0.0; 4], shape: [px, 0.0] });
                }
            }
            out.push(wall_render::BoxInstance { rect: [x, y, w * t, h], fill: theme::t().accent, stroke: [0.0; 4], shape: [2.0 * px, 0.0] });
            // Playhead: a white dot with a dark rim.
            out.push(wall_render::BoxInstance { rect: [x + w * t - 6.0 * px, y - 4.0 * px, 12.0 * px, 12.0 * px], fill: [1.0; 4], stroke: [0.0, 0.0, 0.0, 0.35], shape: [6.0 * px, px] });
            if !st.playing {
                // Paused: two bars on a dark chip at the top-left corner.
                out.push(wall_render::BoxInstance { rect: [f.x as f32 + 8.0 * px, f.y as f32 + 8.0 * px, 26.0 * px, 26.0 * px], fill: [0.0, 0.0, 0.0, 0.5], stroke: [0.0; 4], shape: [theme::radius::INNER * px, 0.0] });
                for dx in [0.0, 7.0] {
                    out.push(wall_render::BoxInstance {
                        rect: [f.x as f32 + (15.0 + dx) * px, f.y as f32 + 14.0 * px, 4.0 * px, 14.0 * px],
                        fill: [1.0, 1.0, 1.0, 0.95],
                        stroke: [0.0; 4],
                        shape: [px, 0.0],
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
        if self.library.is_none() || self.saved_version == self.doc.version || !matches!(self.gesture, Gesture::Idle) || self.editing.is_some() {
            return Ok(false);
        }
        self.keep_history(self.doc.board.id);
        let Some(lib) = &self.library else { return Ok(false) };
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
            let moods = ["warm", "cool", "dusk", "soft", "bold", "hazy"];
            app.sidecars.insert(
                hash.clone(),
                ItemSidecar {
                    hash: hash.clone(),
                    kind: MediaKind::Image,
                    ext: "png".into(),
                    width: w,
                    height: h,
                    bytes: (w * h * 4) as u64,
                    duration_ms: None,
                    fps: None,
                    added_ms: 1_795_132_800_000,
                    title: Some(format!("demo-frame-{:02}.png", i + 1)),
                    source_url: None,
                    tags: vec![moods[i as usize % moods.len()].into(), "demo".into()],
                    auto_tags: Vec::new(),
                    colors: Vec::new(),
                    original: None,
                    range: None,
                    markers: Vec::new(),
                    pingpong: false,
                    extra: Default::default(),
                },
            );
            if let Some(sc) = app.sidecars.get_mut(&hash) {
                auto_tag(sc, Some(&demo_picture(i, w, h)));
            }
            let (c, r) = (i % 6, i / 6);
            items.push((f(c as f64 * 340.0, 20.0 + r as f64 * 230.0, w as f64, h as f64), ItemKind::Media { hash, crop: None, look: Default::default() }));
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
        app.camera = Camera { center: [980.0, 380.0], zoom: 0.62 };
        app
    }

    /// Adds a named group frame (x, y, w, h in board units) behind everything.
    pub fn add_group(&mut self, name: &str, r: [f64; 4]) -> Uuid {
        let z = self.doc.board.live_items().map(|i| i.frame.z).fold(0.0, f64::min) - 1.0;
        let frame = Frame { x: r[0], y: r[1], w: r[2], h: r[3], z, ..Default::default() };
        self.doc.add(vec![(frame, ItemKind::Group { name: name.into(), color: None })])[0]
    }

    /// Moves and sizes an item (x, y, w, h in board units).
    pub fn set_item_frame(&mut self, id: Uuid, r: [f64; 4]) {
        let Some(mut item) = self.doc.item(&id).cloned() else { return };
        item.frame.x = r[0];
        item.frame.y = r[1];
        item.frame.w = r[2];
        item.frame.h = r[3];
        self.doc.replace(vec![item]);
        self.needs_redraw = true;
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
        let id = self.doc.add(vec![(frame, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
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
    /// True unless `item` is a pen line that `p` is not close to (a drawing
    /// is picked by its line, not by the empty box around it).
    fn near_stroke(&self, item: &BoardItem, p: [f32; 2]) -> bool {
        let ItemKind::Stroke { points, width, .. } = &item.kind else { return true };
        let f = item.frame;
        let pts = scene::stroke_points(points, [f.x as f32, f.y as f32, f.w as f32, f.h as f32]);
        let tol = width / 2.0 + 6.0 / self.camera.zoom;
        match pts.len() {
            0 => false,
            1 => scene::segment_distance(p, pts[0], pts[0]) <= tol,
            _ => pts.windows(2).any(|w| scene::segment_distance(p, w[0], w[1]) <= tol),
        }
    }

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
            .filter(|i| self.near_stroke(i, p))
            .max_by(|a, b| a.frame.z.total_cmp(&b.frame.z))
            .map(|i| i.id)
    }

    pub fn pointer_down(&mut self, button: Button, pos: [f32; 2], mods: Mods) {
        self.cursor = pos;
        let board = self.camera.screen_to_board(pos, self.viewport);
        self.needs_redraw = true;
        if self.welcome_modal() && button != Button::Primary {
            return;
        }
        if button == Button::Primary && self.welcome_press(pos) {
            return;
        }
        if button == Button::Middle || (button == Button::Primary && self.space_held) {
            self.gesture = Gesture::Pan;
            return;
        }
        if self.history_press(button, pos) {
            return;
        }
        if button != Button::Primary {
            return;
        }
        if self.presence_press(pos) {
            return;
        }
        if self.menu_click(pos) {
            return;
        }
        let rail = self.rail_layout();
        if rail.contains(pos) && panel::inside(rail.side, pos) {
            self.toggle_sidebar(sidebar::Tab::Boards);
            return;
        }
        if rail.contains(pos) {
            if panel::inside(rail.grip, pos) {
                self.gesture = Gesture::RailMove;
            } else if panel::inside(rail.fold, pos) {
                self.rail_place.collapsed = !self.rail_place.collapsed;
            } else if let Some(tool) = rail.tool_at(pos) {
                self.rail_click(tool);
            }
            return;
        }
        if self.share_press(pos) || self.zoom_pill_press(pos) {
            return;
        }
        if self.comment_card_press(pos) {
            return;
        }
        if self.pen_bar_press(pos) || self.stamp_bar_press(pos) || self.unread_pill_press(pos) {
            return;
        }
        if self.sidebar_press(pos) {
            return;
        }
        if let Some(action) = self.library_layout().and_then(|l| l.action_at(pos)) {
            self.library_action(action);
            return;
        }
        if let Some(action) = self.panel_layout().and_then(|l| l.action_at(pos)) {
            self.panel_action(action);
            return;
        }
        if self.panel_edit.is_some() {
            self.commit_panel_edit();
        }
        if self.comment_board_press(pos, board) {
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
        if self.tool == toolbar::Tool::Pen {
            self.start_draw(board, mods.shift);
            return;
        }
        if mods.alt || self.tool == toolbar::Tool::Arrow {
            // Alt-drag (or any drag with the arrow tool) draws an arrow, from an item or from empty space.
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
        if let Some((item, left, top)) = self.resize_handle_at(board) {
            // Pictures and videos keep their shape unless Shift is held.
            let aspect = matches!(item.kind, ItemKind::Media { .. }).then(|| item.frame.w / item.frame.h.max(1e-6)).filter(|_| !mods.shift);
            let attached = self.lines_on(&[item.id].into());
            self.gesture = Gesture::Resize { original: item, start: board, aspect, attached, left, top };
            return;
        }
        if let Some((id, k)) = hit.and_then(|id| self.todo_box_at(id, board).map(|k| (id, k))) {
            self.selection = [id].into();
            self.toggle_todo(id, k);
            self.last_click = None;
            return;
        }
        if let Some(hex) = hit.filter(|id| self.selection.contains(id) && !mods.shift).and_then(|id| self.swatch_at(id, board)) {
            // A click on a colour of a selected palette copies its hex code.
            self.shell_commands.push(menu::ShellCommand::CopyText(hex));
            self.last_click = None;
            return;
        }
        if let Some((hash, x, w, duration)) = hit.and_then(|id| self.scrub_target(id, board)) {
            self.selection = [hit.unwrap()].into();
            self.video_commands.push((hash.clone(), VideoCommand::Seek(video::time_at(board[0], x, w, duration))));
            self.gesture = Gesture::Scrub { hash, x, w, duration };
            return;
        }
        if double && !mods.shift {
            if let Some(board_id) = hit.and_then(|id| match self.doc.item(&id)?.kind {
                ItemKind::Board { board_id } => Some(board_id),
                _ => None,
            }) {
                self.last_click = None;
                self.open_board(board_id, true);
                return;
            }
            if let Some(url) = hit.and_then(|id| match &self.doc.item(&id)?.kind {
                ItemKind::Link { url, .. } if !url.is_empty() => Some(url.clone()),
                _ => None,
            }) {
                self.open_url(url);
                self.last_click = None;
                return;
            }
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
                // A line of a drawing picks the whole drawing.
                let mates = self.drawing_mates(&id);
                if mods.shift {
                    if self.selection.remove(&id) {
                        for m in &mates {
                            self.selection.remove(m);
                        }
                    } else {
                        self.selection.insert(id);
                        self.selection.extend(mates);
                    }
                } else if !self.selection.contains(&id) {
                    self.selection = [id].into();
                    self.selection.extend(mates);
                }
                let originals = self.drag_set();
                let snap = (!mods.command).then(|| self.snap_targets(&originals)).flatten();
                self.gesture = Gesture::Move { start: board, originals, delta: [0.0, 0.0], snap, guides: Vec::new() };
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
    /// What a drag of `moving` can snap to: other items (not arrows) on or
    /// near the screen. None if nothing is being moved.
    fn snap_targets(&self, moving: &[BoardItem]) -> Option<snap::Snap> {
        let frames: Vec<_> = moving.iter().filter(|i| !matches!(i.kind, ItemKind::Arrow { .. })).map(|i| i.frame).collect();
        let first = frames.first()?;
        let mut b = [first.x, first.y, first.x + first.w, first.y + first.h];
        for f in &frames {
            b = [b[0].min(f.x), b[1].min(f.y), b[2].max(f.x + f.w), b[3].max(f.y + f.h)];
        }
        // The visible board area, grown by a screen in every direction.
        let tl = self.camera.screen_to_board([0.0, 0.0], self.viewport);
        let br = self.camera.screen_to_board(self.viewport, self.viewport);
        let (w, h) = ((br[0] - tl[0]) as f64, (br[1] - tl[1]) as f64);
        let area = [tl[0] as f64 - w, tl[1] as f64 - h, br[0] as f64 + w, br[1] as f64 + h];
        let ids: BTreeSet<Uuid> = moving.iter().map(|i| i.id).collect();
        let targets = self
            .doc
            .board
            .live_items()
            .filter(|i| !ids.contains(&i.id) && !matches!(i.kind, ItemKind::Arrow { .. }))
            .map(|i| [i.frame.x, i.frame.y, i.frame.w, i.frame.h])
            .filter(|t| t[0] < area[2] && t[0] + t[2] > area[0] && t[1] < area[3] && t[1] + t[3] > area[1])
            .collect();
        let grid = self.show_grid.then(|| theme::grid_spacing(self.camera.zoom) as f64);
        Some(snap::Snap { bounds: [b[0], b[1], b[2] - b[0], b[3] - b[1]], targets, grid })
    }

    fn drag_set(&self) -> Vec<BoardItem> {
        let mut ids = self.selection.clone();
        for id in &self.selection {
            if let Some(g) = self.doc.item(id).filter(|i| matches!(i.kind, ItemKind::Group { .. })) {
                let f = g.frame;
                ids.extend(self.items_in([f.x as f32, f.y as f32, (f.x + f.w) as f32, (f.y + f.h) as f32]));
            }
        }
        // Lines drawn on a picture go with it.
        let on: Vec<Uuid> = self.lines_on(&ids).iter().map(|i| i.id).collect();
        ids.extend(on);
        ids.iter().filter_map(|id| self.doc.item(id).cloned()).collect()
    }

    pub fn pointer_move(&mut self, pos: [f32; 2]) {
        let last = self.cursor;
        self.cursor = pos;
        self.cursor_out = false;
        if self.menu_open.is_some() {
            self.needs_redraw = true;
        }
        let board = self.camera.screen_to_board(pos, self.viewport);
        match &mut self.gesture {
            Gesture::Idle => {
                // Tooltips on the rail and the size ring follow the pointer.
                if self.tool == toolbar::Tool::Pen || self.rail_layout().contains(pos) || self.rail_layout().contains(last) {
                    self.needs_redraw = true;
                }
                // Hover looks on the menu bar, the side panel and the zoom pill.
                // (Cheap checks: this runs on every pointer move.)
                let side = self.sidebar_tab().is_some();
                let pill = self.zoom_pill_rect();
                let over = |p: [f32; 2]| p[1] < menu::BAR || (side && p[0] < sidebar::WIDTH) || panel::inside(pill, p);
                if over(pos) || over(last) {
                    self.needs_redraw = true;
                }
                return;
            }
            Gesture::RailDrag { .. } => {}
            Gesture::RailMove => {
                self.rail_place = self.rail_place.dragged(pos, self.viewport);
                self.needs_redraw = true;
                return;
            }
            Gesture::SideRow { start, moved, .. } => {
                if (pos[0] - start[0]).abs() + (pos[1] - start[1]).abs() > 5.0 {
                    *moved = true;
                }
                self.needs_redraw = true;
                return;
            }
            Gesture::Draw { .. } | Gesture::Erase { .. } => {
                self.draw_move(board);
                return;
            }
            Gesture::Connect { now, .. } => *now = board,
            Gesture::Crop { original, start, left, top } => {
                let item = crop_item(original, [(board[0] - start[0]) as f64, (board[1] - start[1]) as f64], *left, *top);
                self.doc.preview(item);
            }
            Gesture::Resize { original, start, aspect, attached, left, top } => {
                let mut item = original.clone();
                let min = 20.0;
                let (dx, dy) = ((board[0] - start[0]) as f64, (board[1] - start[1]) as f64);
                let o = original.frame;
                item.frame.w = (if *left { o.w - dx } else { o.w + dx }).max(min);
                item.frame.h = (if *top { o.h - dy } else { o.h + dy }).max(min);
                if let Some(a) = *aspect {
                    // Follow whichever side moved more, keeping the shape.
                    if item.frame.w / a > item.frame.h {
                        item.frame.h = item.frame.w / a;
                    } else {
                        item.frame.w = item.frame.h * a;
                    }
                }
                // The corner opposite the one held stays where it was.
                if *left {
                    item.frame.x = o.x + o.w - item.frame.w;
                }
                if *top {
                    item.frame.y = o.y + o.h - item.frame.h;
                }
                let lines: Vec<BoardItem> = attached.iter().map(|l| drawing::follow_frame(l, original.frame, item.frame)).collect();
                self.doc.preview(item);
                for l in lines {
                    self.doc.preview(l);
                }
            }
            Gesture::Scrub { hash, x, w, duration } => {
                let cmd = VideoCommand::Seek(video::time_at(board[0], *x, *w, *duration));
                // Only the newest seek per frame matters.
                self.video_commands.retain(|(h, c)| !(h == hash && matches!(c, VideoCommand::Seek(_))));
                self.video_commands.push((hash.clone(), cmd));
            }
            Gesture::Pan => self.camera.pan(pos[0] - last[0], pos[1] - last[1]),
            Gesture::Move { start, originals, delta, snap, guides } => {
                *delta = [(board[0] - start[0]) as f64, (board[1] - start[1]) as f64];
                if let Some(s) = snap {
                    // 6 screen pixels, whatever the zoom.
                    let (d, g) = s.apply(*delta, 6.0 / self.camera.zoom as f64);
                    *delta = d;
                    *guides = g;
                }
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
                // Dropped on a board in the left panel: moved to that board.
                let onto = if self.sidebar.is_some() { self.side_rows_at(pos).filter(|(t, _)| *t != self.doc.board.id) } else { None };
                match onto {
                    Some((target, _)) => {
                        self.doc.preview_offset(&originals, 0.0, 0.0);
                        self.move_selection_to_board(target);
                    }
                    None => {
                        self.doc.commit_drag(&originals, delta[0], delta[1]);
                    }
                }
            }
            g @ (Gesture::Draw { .. } | Gesture::Erase { .. }) => {
                let board = self.camera.screen_to_board(pos, self.viewport);
                self.finish_draw(board, g);
            }
            Gesture::SideRow { id, moved, .. } => self.sidebar_release(id, moved, pos),
            Gesture::RailMove => {
                // Keep where it shows (inside the window), not where the
                // pointer went.
                let r = toolbar::rect(&self.rail_place, self.viewport, 0.0);
                self.rail_place.at = [r[0], r[1]];
            }
            Gesture::RailDrag { tool } => {
                let at = if self.rail_layout().contains(pos) { self.camera.center } else { self.camera.screen_to_board(pos, self.viewport) };
                self.make_with_tool(tool, at);
            }
            Gesture::Connect { from, start, now } => {
                // One arrow per pick of the arrow tool, as in Milanote.
                self.tool = toolbar::Tool::Select;
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
            Gesture::Crop { original, .. } => {
                if let Some(after) = self.doc.board.items.get(&original.id).cloned() {
                    self.doc.commit_edit(Some(&original), after);
                }
            }
            Gesture::Resize { original, attached, .. } => {
                // The picture and the lines on it: one undo step.
                let depth = self.doc.undo_depth();
                for o in std::iter::once(&original).chain(&attached) {
                    if let Some(after) = self.doc.board.items.get(&o.id).cloned() {
                        self.doc.commit_edit(Some(o), after);
                    }
                }
                self.doc.join_undo_since(depth);
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
        if self.welcome_modal() {
            return;
        }
        if self.panel_layout().is_some_and(|l| panel::inside(l.rect, self.cursor)) {
            return;
        }
        let steps = match w {
            Wheel::Lines(l) => l,
            Wheel::Pixels(p) => p / 100.0,
        };
        if self.history_wheel(steps) || self.sidebar_wheel(steps) || self.library_wheel(steps) {
            return;
        }
        self.camera.zoom_at(1.15f32.powf(steps), self.cursor, self.viewport);
        self.needs_redraw = true;
    }

    pub fn key(&mut self, key: Key) {
        self.needs_redraw = true;
        if self.menu_open.take().is_some() && key == Key::Escape {
            return;
        }
        if self.welcome_key(key) {
            return;
        }
        if self.history_key(key) || self.name_key(key) || self.share_key(key) || self.comment_key(key) || self.sidebar_key(key) {
            return;
        }
        if let (toolbar::Tool::Pen, Key::Char(c)) = (self.tool, key) {
            if !self.is_editing() && self.pen_key(c) {
                return;
            }
        }
        if let Some(e) = &mut self.panel_edit {
            match key {
                Key::Escape => self.panel_edit = None,
                Key::Enter => self.commit_panel_edit(),
                Key::Backspace => {
                    e.text.pop();
                }
                Key::Char(c) => {
                    e.text.push(c);
                }
                _ => {}
            }
            return;
        }
        if let Some(search) = &mut self.search {
            match key {
                Key::Escape | Key::Find => self.search = None,
                Key::Backspace => {
                    search.query.pop();
                    self.run_search();
                }
                Key::Enter | Key::Right => self.step_search(1),
                Key::Left => self.step_search(-1),
                Key::Char(c) => {
                    self.text_input(&c.to_string());
                }
                _ => {}
            }
            return;
        }
        if key == Key::Find {
            self.end_edit();
            self.search = Some(Search::default());
            return;
        }
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
                Key::SelectAll | Key::Space { .. } | Key::Duplicate | Key::BringToFront | Key::SendToBack | Key::Find => {}
                Key::Char(c) => {
                    self.text_input(&c.to_string());
                }
            }
            return;
        }
        match key {
            Key::Delete | Key::Backspace => {
                let mut sel = std::mem::take(&mut self.selection);
                // Lines drawn on deleted pictures go with them.
                let on: Vec<Uuid> = self.lines_on(&sel).iter().map(|i| i.id).collect();
                sel.extend(on);
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
                self.sync_moved_out();
                self.prune_selection();
            }
            Key::Redo => {
                self.doc.redo();
                self.sync_moved_out();
                self.prune_selection();
            }
            Key::SelectAll => self.selection = self.doc.board.live_items().map(|i| i.id).collect(),
            Key::Escape if self.share.is_some() => self.share = None,
            Key::Escape if self.tool != toolbar::Tool::Select => self.tool = toolbar::Tool::Select,
            Key::Escape if self.library_is_open() => self.close_sidebar(),
            Key::Escape if self.selection.is_empty() && self.tag_filter.is_some() => self.filter_by_tag(None),
            Key::Escape if self.selection.is_empty() => self.go_up(),
            Key::Escape => self.selection.clear(),
            Key::Space { pressed } => self.space_held = pressed,
            Key::Left | Key::Right | Key::Home | Key::End => {}
            Key::Char(c) if self.stamp_key(c) => {}
            Key::Char(c @ '0'..='7') => self.set_color(c),
            Key::Char('c' | 'C') => self.rail_click(toolbar::Tool::Comment),
            Key::Char('e' | 'E') => self.rail_click(toolbar::Tool::Stamp),
            Key::Char('b' | 'B') => {
                self.new_board();
            }
            Key::Char('p' | 'P') => self.panel_open = !self.panel_open,
            Key::Char('t' | 'T') => {
                let at = self.camera.screen_to_board(self.cursor, self.viewport);
                self.new_todo(at);
            }
            Key::Char('f' | 'F') => self.run_command(menu::Command::FitBoard),
            Key::Char('+' | '=') => self.run_command(menu::Command::ZoomIn),
            Key::Char('-' | '_') => self.run_command(menu::Command::ZoomOut),
            Key::Char('z' | 'Z') => self.run_command(menu::Command::ActualSize),
            Key::Char('l' | 'L') => self.toggle_sidebar(sidebar::Tab::Library),
            Key::Char('s' | 'S') => self.toggle_sidebar(sidebar::Tab::Boards),
            Key::Char('d' | 'D') => self.toggle_drawing(),
            Key::Char('g' | 'G') => self.set_grid(!self.show_grid),
            Key::Char('a') => self.run_command(menu::Command::TidyRows),
            Key::Char('A') => self.run_command(menu::Command::TidySameHeight),
            Key::Char('r' | 'R') => self.run_command(menu::Command::RotateRight),
            Key::Char('h' | 'H') => self.run_command(menu::Command::Mirror),
            Key::Char('v' | 'V') => self.run_command(menu::Command::FlipVertical),
            Key::Char(c) => self.video_key(c),
            Key::Duplicate => self.duplicate(),
            Key::Find => {}
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
        out.extend(self.drawing_preview().0);
        if let Gesture::Connect { from, now, .. } = &self.gesture {
            let to = ArrowEnd::Point { x: now[0] as f64, y: now[1] as f64 };
            if let Some(pts) = scene::arrow_points(&self.doc, from, &to) {
                out.push(scene::arrow_curve(pts, accent, scene::ARROW_WIDTH));
            }
        }
        out
    }

    // ---- search ------------------------------------------------------------

    /// Everything searchable about an item, lowercase.
    fn item_search_text(&self, item: &BoardItem) -> String {
        let text = match &item.kind {
            ItemKind::Media { hash, .. } => self
                .sidecars
                .get(hash)
                .map(|sc| sc.title.iter().chain(&sc.tags).chain(sc.auto_tags.iter().map(|t| &t.tag)).cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
            ItemKind::Board { board_id } => self.board_names.get(board_id).cloned().unwrap_or_default(),
            ItemKind::Todo { title, entries } => todo_to_text(title, entries),
            ItemKind::Link { url, title } => format!("{title} {url}"),
            ItemKind::Palette { colors, name } => format!("{name} palette {}", colors.join(" ")),
            kind => item_text(kind).unwrap_or_default().to_owned(),
        };
        text.to_lowercase()
    }

    /// Finds items containing every word of the query, top-left first, and
    /// shows the first.
    fn run_search(&mut self) {
        let Some(search) = &self.search else { return };
        let words: Vec<String> = search.query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let mut matches: Vec<&BoardItem> = if words.is_empty() {
            Vec::new()
        } else {
            self.doc
                .board
                .live_items()
                .filter(|i| {
                    let t = self.item_search_text(i);
                    words.iter().all(|w| t.contains(w.as_str()))
                })
                .collect()
        };
        matches.sort_by(|a, b| (a.frame.y, a.frame.x).partial_cmp(&(b.frame.y, b.frame.x)).unwrap_or(std::cmp::Ordering::Equal));
        let ids: Vec<Uuid> = matches.iter().map(|i| i.id).collect();
        if let Some(search) = &mut self.search {
            search.matches = ids;
            search.current = None;
        }
        self.step_search(1);
    }

    /// Shows the next (or previous) match: selects it and centres the view.
    fn step_search(&mut self, dir: i32) {
        let Some(search) = &mut self.search else { return };
        let n = search.matches.len();
        if n == 0 {
            search.current = None;
            self.needs_redraw = true;
            return;
        }
        let next = match search.current {
            None => 0,
            Some(c) => (c as i64 + dir as i64).rem_euclid(n as i64) as usize,
        };
        search.current = Some(next);
        let id = search.matches[next];
        if let Some(item) = self.doc.item(&id) {
            let f = item.frame;
            self.camera.center = [(f.x + f.w / 2.0) as f32, (f.y + f.h / 2.0) as f32];
            // Zoom in if the match would be tiny, out if it wouldn't fit.
            let fit = (self.viewport[0] / (f.w as f32 + 80.0)).min(self.viewport[1] / (f.h as f32 + 80.0));
            if self.camera.zoom * (f.w as f32) < 60.0 || self.camera.zoom > fit {
                self.camera.zoom = fit.clamp(MIN_ZOOM, 2.0);
            }
            self.selection = [id].into();
        }
        self.needs_redraw = true;
    }

    /// The search box text and the "n of m" count, while searching.
    pub fn search_status(&self) -> Option<(String, usize, usize)> {
        self.search.as_ref().map(|s| (s.query.clone(), s.current.map_or(0, |c| c + 1), s.matches.len()))
    }

    // ---- boards inside boards ---------------------------------------------

    /// Id of the open board.
    pub fn board_id(&self) -> Uuid {
        self.doc.board.id
    }

    /// Names from the outermost board down to the open one.
    /// The open board's name, safe as a file name.
    pub fn board_name(&self) -> String {
        let n: String = self.doc.board.name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
        let n = n.trim();
        if n.is_empty() { "Board".into() } else { n.to_owned() }
    }

    pub fn breadcrumb(&self) -> Vec<String> {
        self.nav
            .iter()
            .chain(std::iter::once(&self.doc.board.id))
            .map(|id| self.board_names.get(id).cloned().unwrap_or_else(|| "Board".into()))
            .collect()
    }

    /// Adds a new, empty board inside this one as a card at the cursor.
    pub fn new_board(&mut self) -> Uuid {
        self.end_edit();
        let mut board = Board::new(format!("Board {}", self.board_names.len() + 1), self.now_ms);
        board.parent = Some(self.doc.board.id);
        let id = board.id;
        self.board_names.insert(id, board.name.clone());
        self.board_meta.insert(id, sidebar::BoardMeta::of(&board));
        match &self.library {
            Some(lib) => {
                if let Err(e) = lib.write_board(&board) {
                    eprintln!("wall: could not create board: {e}");
                }
            }
            None => {
                let doc = Document::new(board, self.doc.device.clone());
                self.parked.insert(id, (doc, Camera { center: [0.0, 0.0], zoom: 1.0 }));
            }
        }
        let at = self.camera.screen_to_board(self.cursor, self.viewport);
        let s = self.card_scale();
        let frame = Frame { x: at[0] as f64 - 110.0 * s, y: at[1] as f64 - 70.0 * s, w: 220.0 * s, h: 140.0 * s, text_scale: s, ..Default::default() };
        let item = self.doc.add(vec![(frame, ItemKind::Board { board_id: id })])[0];
        self.selection = [item].into();
        self.needs_redraw = true;
        id
    }

    /// Opens another board. `down` records the current one as its parent in
    /// the breadcrumb (false when going back up).
    pub fn open_board(&mut self, id: Uuid, down: bool) {
        if id == self.doc.board.id {
            return;
        }
        self.end_edit();
        if let Err(e) = self.save_if_dirty() {
            eprintln!("wall: could not save: {e}");
        }
        let next = match self.parked.remove(&id) {
            Some(p) => Some(p),
            None => self.library.as_ref().and_then(|l| l.read_board(id).ok()).map(|b| (Document::new(b, self.doc.device.clone()), Camera { center: [0.0, 0.0], zoom: 1.0 })),
        };
        let Some((mut doc, camera)) = next else {
            eprintln!("wall: board {id} not found");
            return;
        };
        doc.set_clock(self.now_ms);
        let fresh = !doc.can_undo() && doc.version == 0;
        let prev_doc = std::mem::replace(&mut self.doc, doc);
        let prev_cam = std::mem::replace(&mut self.camera, camera);
        let prev_id = prev_doc.board.id;
        self.board_meta.insert(prev_id, sidebar::BoardMeta { seen_ms: self.now_ms, ..sidebar::BoardMeta::of(&prev_doc.board) });
        self.board_meta.insert(id, sidebar::BoardMeta { seen_ms: self.now_ms, ..sidebar::BoardMeta::of(&self.doc.board) });
        self.parked.insert(prev_id, (prev_doc, prev_cam));
        if down {
            self.nav.push(prev_id);
        }
        self.saved_version = self.doc.version;
        self.selection.clear();
        self.gesture = Gesture::Idle;
        self.scene.invalidate();
        if let Some(lib) = self.library.take() {
            self.load_pictures(&lib);
            self.library = Some(lib);
        }
        if fresh {
            self.fit_view();
        }
        self.needs_redraw = true;
    }

    /// Back to the board this one sits in.
    pub fn go_up(&mut self) {
        if let Some(parent) = self.nav.pop() {
            self.open_board(parent, false);
        }
    }

    // ---- arranging ---------------------------------------------------------

    /// The single selected item, if one of its four resize handles is under
    /// a board point: (item, is left corner, is top corner). Not for arrows
    /// and lines, which have no handles.
    fn resize_handle_at(&self, board: [f32; 2]) -> Option<(BoardItem, bool, bool)> {
        let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] else { return None };
        let item = self.doc.item(&id).filter(|i| !matches!(i.kind, ItemKind::Arrow { .. }))?;
        let r = HANDLE_PX / self.camera.zoom;
        let f = item.frame;
        for (left, top) in [(false, false), (true, false), (false, true), (true, true)] {
            let hx = if left { f.x } else { f.x + f.w } as f32;
            let hy = if top { f.y } else { f.y + f.h } as f32;
            if (board[0] - hx).abs() <= r && (board[1] - hy).abs() <= r {
                return Some((item.clone(), left, top));
            }
        }
        None
    }

    /// A corner of the single selected picture under a board point:
    /// (item, is left corner, is top corner).
    fn crop_corner_at(&self, board: [f32; 2]) -> Option<(BoardItem, bool, bool)> {
        let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] else { return None };
        // Cropping works in the source's own coordinates, so only on pictures
        // that aren't turned or mirrored.
        let item = self.doc.item(&id).filter(|i| matches!(&i.kind, ItemKind::Media { look, .. } if look.turns % 4 == 0 && !look.mirror && !look.flip_v))?;
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
        let color = key.to_digit(10).filter(|d| *d > 0).map(|d| panel::PALETTE[d as usize - 1].to_owned());
        self.set_color_value(color);
    }

    fn set_color_value(&mut self, color: Option<String>) {
        let afters: Vec<BoardItem> = self
            .selected_items()
            .into_iter()
            .filter_map(|mut i| {
                match &mut i.kind {
                    ItemKind::Note { color: c, .. } | ItemKind::Group { color: c, .. } | ItemKind::Stroke { color: c, .. } => *c = color.clone(),
                    _ => return None,
                }
                Some(i)
            })
            .collect();
        self.doc.replace(afters);
    }

    /// Turns, mirrors or greys the selected pictures and videos. A quarter
    /// turn swaps the item's width and height around its centre. One undo step.
    fn change_look(&mut self, change: impl Fn(&mut wall_format::Look)) {
        let afters: Vec<BoardItem> = self
            .selected_items()
            .into_iter()
            .filter_map(|mut i| {
                let ItemKind::Media { look, .. } = &mut i.kind else { return None };
                let was = look.sideways();
                change(look);
                if look.sideways() != was {
                    let f = &mut i.frame;
                    let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
                    (f.w, f.h) = (f.h, f.w);
                    (f.x, f.y) = (cx - f.w / 2.0, cy - f.h / 2.0);
                }
                Some(i)
            })
            .collect();
        self.doc.replace(afters);
        self.needs_redraw = true;
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
        for i in &mut items {
            i.frame.x += offset[0];
            i.frame.y += offset[1];
        }
        self.needs_redraw = true;
        self.doc.add_copies(&items)
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
        self.editing.is_some() || self.search.is_some() || self.panel_edit.is_some() || self.name_edit.is_some() || self.share_typing() || self.comment_typing() || self.sidebar_typing()
    }

    /// Starts typing into an item's text, caret at the end.
    pub fn begin_edit(&mut self, id: Uuid) {
        self.end_edit();
        let Some(item) = self.doc.item(&id).cloned() else { return };
        if let ItemKind::Todo { title, entries } = &item.kind {
            let text = todo_to_text(title, entries);
            let mut as_text = item.clone();
            as_text.kind = ItemKind::Note { text: text.clone(), color: None };
            self.doc.preview(as_text);
            self.editing = Some(Editing { id, caret: text.len(), original: Some(item), todo: true });
            self.selection = [id].into();
            self.edit(|_, _| {});
            return;
        }
        let Some(text) = item_text(&item.kind) else { return };
        self.editing = Some(Editing { id, caret: text.len(), original: Some(item.clone()), todo: false });
        self.selection = [id].into();
        self.needs_redraw = true;
    }

    /// Creates a to-do list at a board point and starts typing into it.
    /// The open library's folder, if one is open.
    pub fn library_root(&self) -> Option<&std::path::Path> {
        self.library.as_ref().map(|l| l.root())
    }

    /// Adds a card for a board that was just written to the library (by an
    /// import) to the open board, in the middle of the view, and selects it.
    pub fn add_board_card(&mut self, board_id: Uuid, name: &str) {
        self.board_names.insert(board_id, name.to_owned());
        let s = self.card_scale();
        let at = self.camera.center;
        let frame = Frame { x: at[0] as f64 - 110.0 * s, y: at[1] as f64 - 70.0 * s, w: 220.0 * s, h: 140.0 * s, z: self.doc.top_z() + 1.0, text_scale: s, ..Default::default() };
        let item = self.doc.add(vec![(frame, ItemKind::Board { board_id })])[0];
        self.selection = [item].into();
        self.needs_redraw = true;
    }

    /// Text scale for a card made now: cards made while zoomed far out are
    /// drawn larger so they can be read at that zoom (as in the old Wall).
    fn card_scale(&self) -> f64 {
        let z = self.camera.zoom as f64;
        if z < 0.5 { (1.0 / z).min(8.0) } else { 1.0 }
    }

    pub fn new_todo(&mut self, at: [f32; 2]) -> Uuid {
        let id = self.new_note(at, "To-do\n".into());
        if let Some(e) = &mut self.editing {
            e.todo = true;
        }
        id
    }

    /// Ticks or unticks entry `k` of a to-do card. One undo step.
    pub fn toggle_todo(&mut self, id: Uuid, k: usize) {
        let Some(mut item) = self.doc.item(&id).cloned() else { return };
        let ItemKind::Todo { entries, .. } = &mut item.kind else { return };
        let Some(e) = entries.get_mut(k) else { return };
        e.done = !e.done;
        self.doc.replace(vec![item]);
        self.needs_redraw = true;
    }

    /// The to-do entry whose tick box is under a board point.
    fn todo_box_at(&self, id: Uuid, p: [f32; 2]) -> Option<usize> {
        let item = self.doc.item(&id)?;
        let ItemKind::Todo { entries, .. } = &item.kind else { return None };
        let f = item.frame;
        // Boxes are laid out at the card's unscaled size (see scene).
        let s = if f.text_scale > 0.0 { f.text_scale as f32 } else { 1.0 };
        let rect = [f.x as f32, f.y as f32, f.w as f32 / s, f.h as f32 / s];
        let p = [rect[0] + (p[0] - rect[0]) / s, rect[1] + (p[1] - rect[1]) / s];
        (0..entries.len()).find(|k| {
            let b = scene::todo_box(rect, *k);
            let pad = 4.0;
            p[0] >= b[0] - pad && p[0] <= b[0] + b[2] + pad && p[1] >= b[1] - pad && p[1] <= b[1] + b[3] + pad && b[1] + b[3] <= rect[1] + rect[3]
        })
    }

    /// Finishes typing: records the edit as one undo step. A new note left
    /// empty is dropped.
    pub fn end_edit(&mut self) {
        let Some(e) = self.editing.take() else { return };
        self.needs_redraw = true;
        let Some(mut item) = self.doc.board.items.get(&e.id).cloned() else { return };
        if e.todo {
            // Back from text to a to-do card; ticks follow their entries.
            let text = item_text(&item.kind).unwrap_or_default().to_owned();
            let old = match e.original.as_ref().map(|o| &o.kind) {
                Some(ItemKind::Todo { entries, .. }) => entries.clone(),
                _ => Vec::new(),
            };
            let (title, entries) = text_to_todo(&text, &old);
            if e.original.is_none() && entries.is_empty() && (title.is_empty() || title == "To-do") {
                self.doc.discard_preview(&e.id);
                self.selection.remove(&e.id);
                return;
            }
            item.frame.h = scene::todo_height(entries.len()) as f64 * item.frame.text_scale.max(0.05);
            item.kind = ItemKind::Todo { title, entries };
            self.doc.commit_edit(e.original.as_ref(), item);
            return;
        }
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
        let s = self.card_scale();
        let frame = Frame { x: at[0] as f64 - 20.0 * s, y: at[1] as f64 - 20.0 * s, w: 240.0 * s, h: 72.0 * s, z: self.doc.top_z() + 1.0, text_scale: s, ..Default::default() };
        let id = Uuid::new_v4();
        let caret = text.len();
        let item = BoardItem { id, frame, kind: ItemKind::Note { text, color: None }, rev: 0, updated_ms: 0, updated_by: String::new(), deleted: false };
        self.doc.preview(item);
        self.editing = Some(Editing { id, caret, original: None, todo: false });
        self.selection = [id].into();
        self.edit(|_, _| {}); // grows the frame to fit pasted text
        self.needs_redraw = true;
        id
    }

    /// Types text at the caret. Returns false when nothing is being edited.
    pub fn text_input(&mut self, s: &str) -> bool {
        if self.welcome_modal() {
            return true;
        }
        if self.history_previewing() || self.name_text(s) || self.share_text(s) || self.comment_text(s) || self.sidebar_text(s) {
            return true;
        }
        if let Some(e) = &mut self.panel_edit {
            e.text.push_str(&s.replace(['\n', '\r', '\t'], " "));
            self.needs_redraw = true;
            return true;
        }
        if let Some(search) = &mut self.search {
            search.query.push_str(&s.replace(['\n', '\r', '\t'], " "));
            self.run_search();
            return true;
        }
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
        if let Some(url) = as_url(&text) {
            self.new_link(at, url);
        } else if let Some(colors) = as_hex_colors(&text) {
            self.new_palette(at, colors);
        } else {
            self.new_note(at, text);
        }
        self.end_edit();
    }

    fn editing_multiline(&self) -> bool {
        self.editing
            .as_ref()
            .and_then(|e| self.doc.item(&e.id))
            .and_then(scene::TextStyle::for_item)
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
        if let Some(style) = scene::TextStyle::for_item(&item) {
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
        let (Some(text), Some(style)) = (item_text(&item.kind), scene::TextStyle::for_item(item)) else { return };
        let local = [board[0] - item.frame.x as f32, board[1] - item.frame.y as f32];
        e.caret = style.hit(&mut self.font, text, style.max_lines(item.frame.h as f32), local);
    }

    /// The caret as a board rect, while editing.
    fn caret_rect(&mut self) -> Option<[f32; 4]> {
        let e = self.editing.as_ref()?;
        let item = self.doc.item(&e.id)?;
        let text = item_text(&item.kind)?;
        let style = scene::TextStyle::for_item(item)?;
        let r = style.caret(&mut self.font, text, style.max_lines(item.frame.h as f32), e.caret.min(text.len()));
        Some([item.frame.x as f32 + r[0], item.frame.y as f32 + r[1], 1.5 / self.camera.zoom, r[3]])
    }

    fn prune_selection(&mut self) {
        let live: BTreeSet<Uuid> = self.selection.iter().filter(|id| self.doc.item(id).is_some()).copied().collect();
        self.selection = live;
    }

    // ---- drawing -----------------------------------------------------------

    /// The board's area for an export: every live item plus a margin, or
    /// None for an empty board.
    pub fn board_bounds(&self) -> Option<[f32; 4]> {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for i in self.doc.board.live_items() {
            b = [b[0].min(i.frame.x), b[1].min(i.frame.y), b[2].max(i.frame.x + i.frame.w), b[3].max(i.frame.y + i.frame.h)];
        }
        (b[0] <= b[2]).then(|| {
            let m = 48.0;
            [(b[0] - m) as f32, (b[1] - m) as f32, (b[2] - b[0] + 2.0 * m) as f32, (b[3] - b[1] + 2.0 * m) as f32]
        })
    }

    /// Pixel size and zoom for exporting the whole board: up to 2× board
    /// units, with the longer side at most `max_side` pixels.
    pub fn export_size(&self, max_side: u32) -> Option<([u32; 2], f32)> {
        let b = self.board_bounds()?;
        let zoom = (max_side as f32 / b[2].max(b[3])).min(2.0);
        let size = [((b[2] * zoom).ceil() as u32).max(1), ((b[3] * zoom).ceil() as u32).max(1)];
        Some((size, zoom))
    }

    /// Draws the whole board, without selection, panels or menus, into an
    /// off-screen picture. Returns straight RGBA8 pixels and their size.
    pub fn export_board(&mut self, renderer: &mut Renderer, max_side: u32) -> Result<(Vec<u8>, [u32; 2]), AppError> {
        self.export_board_marked(renderer, max_side, &[])
    }

    /// [`Self::export_board`] with numbered pins at board points (the PDF's
    /// comments), each `pin_px` pixels across in the picture.
    pub(crate) fn export_board_marked(&mut self, renderer: &mut Renderer, max_side: u32, pins: &[([f32; 2], String)]) -> Result<(Vec<u8>, [u32; 2]), AppError> {
        let max_side = max_side.min(renderer.max_picture_size());
        let (size, zoom) = self.export_size(max_side).ok_or_else(|| AppError::Other("the board is empty".into()))?;
        let b = self.board_bounds().unwrap_or_default();
        self.pump_pictures(renderer);
        // Every picture at the size it has in the export: the preview when
        // that's enough (most pictures on a big board), full size otherwise.
        // Ones not loaded at that size are decoded now (previews from the
        // cache) and dropped after the export, one at a time, so a big
        // board doesn't need all its full-size pictures in memory at once.
        let mut temporary = Vec::new();
        let mut ids: HashMap<ContentHash, ImageId> = HashMap::new();
        let mut shown: HashMap<ContentHash, f32> = HashMap::new();
        for i in self.doc.board.live_items() {
            if let ItemKind::Media { hash, crop, .. } = &i.kind {
                let px = i.frame.w as f32 * zoom / crop.map_or(1.0, |c| c.w).max(1e-3) as f32;
                let e = shown.entry(hash.clone()).or_insert(0.0);
                *e = e.max(px);
            }
        }
        for (hash, px) in shown {
            let Some(sc) = self.sidecars.get(&hash) else { continue };
            let need_full = px > picture_loader::PREVIEW_PX as f32 * 1.1 && sc.width.max(sc.height) > picture_loader::PREVIEW_PX;
            let loaded = match self.pictures.get(&hash) {
                Some(Picture::Ready(id)) => Some(*id),
                Some(Picture::Preview(id)) if !need_full => Some(*id),
                Some(Picture::Preview(_) | Picture::Unloaded) => {
                    let tier = if need_full { picture_loader::Tier::Full } else { picture_loader::Tier::Preview };
                    let decoded = self.library.as_ref().and_then(|lib| {
                        let job = picture_loader::Job {
                            hash: hash.clone(),
                            media: lib.media_path(&hash, &sc.ext),
                            cache_dir: lib.root().join("cache").join("previews"),
                            tier,
                            want_tiny: false,
                        };
                        let done = picture_loader::run_job(&job);
                        done.full.or(done.preview)
                    });
                    match decoded.and_then(|(rgba, w, h)| renderer.create_image(&rgba, w, h).ok()) {
                        Some(id) => {
                            temporary.push(id);
                            Some(id)
                        }
                        None => match self.pictures.get(&hash) {
                            Some(Picture::Preview(id)) => Some(*id),
                            _ => None,
                        },
                    }
                }
                _ => None,
            };
            if let Some(id) = loaded {
                ids.insert(hash, id);
            }
        }
        let textures: HashMap<&ContentHash, ImageId> = ids.iter().map(|(h, id)| (h, *id)).collect();
        self.scene.update(&self.doc, &mut self.font, &self.board_names);
        let (pin_boxes, pin_glyphs) = export::pin_marks(&mut self.font, pins, zoom, size);
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        let view = ViewUniform::new([b[0] + b[2] / 2.0, b[1] + b[3] / 2.0], zoom, [size[0] as f32, size[1] as f32]);
        let scene = &self.scene;
        let pixels = renderer
            .render_to_pixels(size, |r| {
                r.begin_frame(size, BACKGROUND());
                // Always the full-size picture, whatever its size in the export.
                scene.draw(r, &view, &|h, _| textures.get(h).map(|id| (*id, [0.0, 0.0, 1.0, 1.0])));
                r.draw_boxes(&view, &pin_boxes);
                r.draw_glyphs(&view, &pin_glyphs);
            })
            .map_err(|e| AppError::Other(e.to_string()));
        for id in temporary {
            renderer.delete_image(id);
        }
        self.needs_redraw = true;
        Ok((pixels?, size))
    }

    pub fn render(&mut self, renderer: &mut Renderer, size_px: [u32; 2]) {
        // A version from the history card is drawn in place of the board.
        let swapped = self.history_swap();
        self.viewport = [size_px[0] as f32, size_px[1] as f32];
        self.pump_pictures(renderer);
        let caret = self.caret_rect();
        self.scene.update(&self.doc, &mut self.font, &self.board_names);
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
        if let Gesture::Move { guides, .. } = &self.gesture {
            // Snap guides: 1 screen pixel wide, pink so they read over any card.
            let px = 1.0 / self.camera.zoom;
            overlays.extend(guides.iter().map(|g| wall_render::BoxInstance {
                rect: [g[0] as f32 - px / 2.0, g[1] as f32 - px / 2.0, (g[2] - g[0]) as f32 + px, (g[3] - g[1]) as f32 + px],
                fill: [1.0, 0.33, 0.6, 1.0],
                stroke: [0.0; 4],
                shape: [0.0, 0.0],
            }));
        }
        if let [id] = self.selection.iter().copied().collect::<Vec<_>>()[..] {
            if let (Some(item), None) = (self.doc.item(&id), &self.editing) {
                let px = 1.0 / self.camera.zoom;
                if !matches!(item.kind, ItemKind::Arrow { .. }) {
                    let f = item.frame;
                    for (hx, hy) in [(f.x, f.y), (f.x + f.w, f.y), (f.x, f.y + f.h), (f.x + f.w, f.y + f.h)] {
                        overlays.push(wall_render::BoxInstance {
                            rect: [hx as f32 - 4.0 * px, hy as f32 - 4.0 * px, 8.0 * px, 8.0 * px],
                            fill: [1.0; 4],
                            stroke: theme::t().accent,
                            shape: [2.0 * px, 1.5 * px],
                        });
                    }
                }
            }
        }
        if let Some(r) = caret {
            overlays.push(wall_render::BoxInstance { rect: r, fill: theme::t().accent, stroke: [0.0; 4], shape: [0.0, 0.0] });
        }

        renderer.begin_frame(size_px, BACKGROUND());
        if self.show_grid {
            self.draw_grid(renderer);
        }
        let view = ViewUniform::new(self.camera.center, self.camera.zoom, self.viewport);
        self.scene.draw(renderer, &view, &|h, px| self.picture_texture(h, px));
        let dim: Vec<wall_render::BoxInstance> = self
            .filtered_out()
            .iter()
            .map(|i| wall_render::BoxInstance {
                rect: [i.frame.x as f32 - 1.0, i.frame.y as f32 - 1.0, i.frame.w as f32 + 2.0, i.frame.h as f32 + 2.0],
                // Fade toward the board colour (works in both themes).
                fill: {
                    let b = BACKGROUND();
                    [b[0], b[1], b[2], 0.85]
                },
                stroke: [0.0; 4],
                shape: [0.0, 0.0],
            })
            .collect();
        if !dim.is_empty() {
            renderer.draw_boxes(&view, &dim);
        }
        renderer.draw_boxes(&view, &overlays);
        renderer.draw_curves(&view, &self.overlay_curves());
        renderer.draw_curves_once(&view, &self.drawing_preview().1);
        if let Some((query, at, total)) = self.search_status() {
            // Search box: top centre, constant size on screen.
            let px = 1.0 / self.camera.zoom;
            let (w, h) = (360.0, 40.0);
            let tl = self.camera.screen_to_board([self.viewport[0] / 2.0 - w / 2.0, menu::BAR + 12.0], self.viewport);
            let bx = wall_render::BoxInstance { rect: [tl[0], tl[1], w * px, h * px], fill: theme::t().search_bg, stroke: theme::t().accent, shape: [theme::radius::CONTROL * px, 1.5 * px] };
            renderer.draw_boxes(&view, &[bx]);
            let shown = if query.is_empty() { "Search this board".to_owned() } else { query.clone() };
            let colour = if query.is_empty() { theme::t().dim } else { theme::t().ink };
            let count = if query.is_empty() { String::new() } else { format!("{at} of {total}") };
            let mut glyphs = Vec::new();
            scene::add_text(&mut self.font, &mut glyphs, &shown, [tl[0] + 14.0 * px, tl[1] + 26.0 * px], 15.0 * px, colour);
            let cw = self.font.layout_line(&count).width * 13.0 * px;
            scene::add_text(&mut self.font, &mut glyphs, &count, [tl[0] + (w - 14.0) * px - cw, tl[1] + 26.0 * px], 13.0 * px, theme::t().dim);
            if self.font.atlas.dirty {
                renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
                self.font.atlas.dirty = false;
            }
            renderer.draw_glyphs(&view, &glyphs);
        }
        if !self.nav.is_empty() {
            // Breadcrumb at the top-left, constant size on screen.
            let text = format!("{}   (Esc to go up)", self.breadcrumb().join("  ›  "));
            let size = 14.0 / self.camera.zoom;
            let at = self.camera.screen_to_board([16.0, menu::BAR + 24.0], self.viewport);
            let mut crumb = Vec::new();
            scene::add_text(&mut self.font, &mut crumb, &text, at, size, theme::t().dim);
            if self.font.atlas.dirty {
                renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
                self.font.atlas.dirty = false;
            }
            renderer.draw_glyphs(&view, &crumb);
        }
        if self.doc.board.live_items().next().is_none() {
            // Empty board: a quiet drop area in the middle of the screen.
            let t = theme::t();
            let screen = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
            let (w, h) = (440.0f32.min(self.viewport[0] - 32.0), 150.0);
            let r = [(self.viewport[0] - w) / 2.0, (self.viewport[1] - h) / 2.0, w, h];
            renderer.draw_boxes(&screen, &[wall_render::BoxInstance { rect: r, fill: t.hover, stroke: t.line, shape: [theme::radius::PANEL, 1.0] }]);
            let mut hint = Vec::new();
            let lines: [(&str, f32, [f32; 4]); 2] = [
                ("Drop pictures or videos here", 17.0, t.ink),
                ("Double-click for a note  ·  B new board  ·  S your boards", 12.5, t.dim),
            ];
            for (k, (text, size, ink)) in lines.into_iter().enumerate() {
                let tw = self.font.layout_line(text).width * size;
                let y = r[1] + if k == 0 { 66.0 } else { 96.0 };
                scene::add_text(&mut self.font, &mut hint, text, [r[0] + (w - tw) / 2.0, y], size, ink);
            }
            if self.font.atlas.dirty {
                renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
                self.font.atlas.dirty = false;
            }
            renderer.draw_glyphs(&screen, &hint);
        }
        self.draw_panel(renderer);
        self.draw_sidebar(renderer);
        let redraw_library = self.library_layout().is_some();
        self.needs_redraw = false;
        if redraw_library {
            // Sets needs_redraw again while thumbnails are still loading.
            self.draw_library(renderer);
        }
        self.draw_comments(renderer);
        self.draw_zoom_pill(renderer);
        self.draw_rail(renderer);
        self.draw_pen_ui(renderer);
        self.draw_share(renderer);
        if swapped {
            self.history_swap();
        }
        self.draw_history(renderer);
        // The menu bar and its drop-downs go over everything, but for the
        // badges of who's here, which sit in the bar.
        self.draw_menu(renderer);
        self.draw_presence(renderer);
        // The walkthrough goes over everything.
        self.draw_welcome(renderer);
    }
}

impl App {
    // ---- side panel --------------------------------------------------------

    /// What the side panel shows for the selection, if it is open.
    pub fn panel_info(&self) -> Option<panel::PanelInfo> {
        use panel::{format_bytes, format_date, format_duration, PanelInfo, VideoInfo};
        if !self.panel_open {
            return None;
        }
        if self.selection.is_empty() {
            return Some(PanelInfo {
                kind: "Board".into(),
                title: Some(self.doc.board.name.clone()),
                title_hint: "Name this board".into(),
                rows: vec![("Items".into(), self.doc.board.live_items().filter(|i| !matches!(i.kind, ItemKind::Arrow { .. })).count().to_string())],
                board_tags: Some(self.board_tags()),
                filter: self.tag_filter.clone(),
                ..Default::default()
            });
        }
        let items = self.selected_items();
        let colorable = |k: &ItemKind| matches!(k, ItemKind::Note { .. } | ItemKind::Group { .. } | ItemKind::Stroke { .. });
        let color_of = |k: &ItemKind| match k {
            ItemKind::Note { color, .. } | ItemKind::Group { color, .. } | ItemKind::Stroke { color, .. } => color.clone(),
            _ => None,
        };
        if items.len() > 1 {
            let colour = items.iter().any(|i| colorable(&i.kind)).then(|| {
                let first = items.iter().find(|i| colorable(&i.kind)).and_then(|i| color_of(&i.kind));
                items.iter().filter(|i| colorable(&i.kind)).all(|i| color_of(&i.kind) == first).then_some(first).flatten()
            });
            return Some(PanelInfo { kind: format!("{} items", items.len()), color: colour, ..Default::default() });
        }
        let item = items.into_iter().next()?;
        let mut info = PanelInfo::default();
        let size_row = |w: f64, h: f64| ("On board".to_owned(), format!("{:.0} × {:.0}", w, h));
        match &item.kind {
            ItemKind::Media { hash, crop, look } => {
                let sc = self.sidecars.get(hash);
                if !look.is_plain() {
                    info.rows.push(("Shown".into(), describe_look(look)));
                }
                if sc.is_some_and(|s| s.pingpong) {
                    info.rows.push(("Loop".into(), "Back and forth".into()));
                }
                if self.onion.contains(hash) {
                    info.rows.push(("Onion skin".into(), "On  ·  U".into()));
                }
                let video = self.videos.contains_key(hash);
                info.kind = if video { "Video" } else { "Image" }.into();
                info.title = Some(sc.and_then(|s| s.title.clone()).unwrap_or_default());
                info.title_hint = "Add a title".into();
                if let Some(sc) = sc {
                    info.rows.push(("Size".into(), format!("{} × {} px", sc.width, sc.height)));
                    info.rows.push(("File".into(), format!("{} · {}", sc.ext.to_uppercase(), format_bytes(sc.bytes))));
                    if let Some(o) = &sc.original {
                        info.rows.push(("Original".into(), format!("{} × {} · {}", o.width, o.height, format_bytes(o.bytes))));
                    }
                    if let Some(d) = sc.duration_ms {
                        let fps = sc.fps.map(|f| format!(" · {f:.0} fps")).unwrap_or_default();
                        info.rows.push(("Length".into(), format!("{}{fps}", format_duration(d as f64 / 1000.0))));
                    }
                    info.rows.push(("Added".into(), format_date(sc.added_ms)));
                    info.rows.push(("Source".into(), sc.source_url.clone().unwrap_or_else(|| "—".into())));
                    info.tags = Some(sc.tags.clone());
                    info.suggested = sc.auto_tags.iter().map(|t| t.tag.clone()).filter(|t| !sc.tags.contains(t)).collect();
                    info.palette = sc.colors.clone();
                } else {
                    info.tags = None;
                }
                if crop.is_some() {
                    info.rows.push(("Crop".into(), "Cropped (Ctrl-drag a corner)".into()));
                }
                if video {
                    let st = self.video_status.get(hash);
                    info.video = Some(VideoInfo {
                        range: self.video_range(hash),
                        markers: sc.map(|s| s.markers.iter().map(|m| (m.ms as f64 / 1000.0, m.label.clone())).collect()).unwrap_or_default(),
                        at_marker: st.and_then(|s| video::marker_at(&sc.map(|s| s.markers.clone()).unwrap_or_default(), s.time)),
                        duration_s: st.map_or(0.0, |s| s.duration),
                        fps: sc.and_then(|s| s.fps),
                        playing: st.is_none_or(|s| s.playing),
                        speed: st.map_or(1.0, |s| s.speed),
                    });
                }
            }
            ItemKind::Note { text, color } => {
                info.kind = "Note".into();
                info.rows.push(("Words".into(), text.split_whitespace().count().to_string()));
                info.rows.push(size_row(item.frame.w, item.frame.h));
                info.color = Some(color.clone());
            }
            ItemKind::Label { text } => {
                info.kind = "Label".into();
                info.rows.push(("Words".into(), text.split_whitespace().count().to_string()));
            }
            ItemKind::Todo { entries, .. } => {
                info.kind = "To-do".into();
                info.rows.push(("Done".into(), format!("{} of {}", entries.iter().filter(|e| e.done).count(), entries.len())));
            }
            ItemKind::Group { name, color } => {
                info.kind = "Group".into();
                info.title = Some(name.clone());
                info.title_hint = "Name this group".into();
                let f = item.frame;
                let inside = self.items_in([f.x as f32, f.y as f32, (f.x + f.w) as f32, (f.y + f.h) as f32]).len().saturating_sub(1);
                info.rows.push(("Contains".into(), format!("{inside} items")));
                info.rows.push(size_row(f.w, f.h));
                info.color = Some(color.clone());
            }
            ItemKind::Palette { colors, name } => {
                info.kind = "Palette".into();
                info.title = Some(name.clone());
                info.title_hint = "Name this palette".into();
                info.palette = colors.clone();
                info.rows.push(("Copy".into(), "Click a colour on the card".into()));
            }
            ItemKind::Stroke { points, color, .. } => {
                info.kind = "Drawing".into();
                info.color = Some(color.clone());
                info.rows.push(("Points".into(), points.len().to_string()));
                info.rows.push(size_row(item.frame.w, item.frame.h));
            }
            ItemKind::Link { url, title } => {
                info.kind = "Link".into();
                info.title = Some(title.clone());
                info.title_hint = "Add a title".into();
                info.rows.push(("Site".into(), scene::link_site(url)));
                info.rows.push(("Address".into(), url.clone()));
                info.can_open = !url.is_empty();
            }
            ItemKind::Board { board_id } => {
                info.kind = "Board".into();
                info.title = Some(self.board_names.get(board_id).cloned().unwrap_or_default());
                info.title_hint = "Name this board".into();
                if let Some(n) = self.parked.get(board_id).map(|(d, _)| d.board.live_items().count()) {
                    info.rows.push(("Contains".into(), format!("{n} items")));
                }
                info.can_open = true;
            }
            ItemKind::Arrow { from, to } => {
                info.kind = "Arrow".into();
                let end = |e: &ArrowEnd| match e {
                    ArrowEnd::Item { id } => self.doc.item(id).map_or("—".into(), |i| self.short_name(i)),
                    ArrowEnd::Point { .. } => "Free end".into(),
                };
                info.rows.push(("From".into(), end(from)));
                info.rows.push(("To".into(), end(to)));
            }
            ItemKind::Comment { messages, resolved } => {
                info.kind = "Comment".into();
                info.rows.push(("Messages".into(), messages.len().to_string()));
                if let Some(m) = messages.first() {
                    info.rows.push(("Started by".into(), m.by.clone()));
                }
                info.rows.push(("State".into(), if *resolved { "Resolved" } else { "Open" }.into()));
            }
            ItemKind::Stamp { mark, by } => {
                info.kind = "Stamp".into();
                info.rows.push(("Mark".into(), mark.clone()));
                info.rows.push(("By".into(), if by.is_empty() { "—".into() } else { by.clone() }));
            }
        }
        info.filter = self.tag_filter.clone();
        Some(info)
    }

    /// Tags of the pictures and videos on this board with their counts,
    /// most used first.
    pub fn board_tags(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for i in self.doc.board.live_items() {
            if let ItemKind::Media { hash, .. } = &i.kind {
                if let Some(sc) = self.sidecars.get(hash) {
                    for t in sidecar_tags(sc) {
                        *counts.entry(t).or_default() += 1;
                    }
                }
            }
        }
        let mut v: Vec<(String, usize)> = counts.into_iter().map(|(t, n)| (t.to_owned(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Items hidden by the tag filter: pictures and videos without the tag,
    /// and notes, labels and boards. Groups and arrows stay visible.
    fn filtered_out(&self) -> Vec<&BoardItem> {
        let Some(tag) = &self.tag_filter else { return Vec::new() };
        self.doc
            .board
            .live_items()
            .filter(|i| match &i.kind {
                ItemKind::Media { hash, .. } => !self.sidecars.get(hash).is_some_and(|s| sidecar_tags(s).any(|t| t == tag)),
                ItemKind::Group { .. } | ItemKind::Arrow { .. } => false,
                _ => true,
            })
            .collect()
    }

    /// Sets or clears the tag filter.
    pub fn filter_by_tag(&mut self, tag: Option<String>) {
        self.tag_filter = tag;
        self.needs_redraw = true;
    }

    /// A short human name for an item, for the panel.
    fn short_name(&self, item: &BoardItem) -> String {
        match &item.kind {
            ItemKind::Media { hash, .. } => self.sidecars.get(hash).and_then(|s| s.title.clone()).unwrap_or_else(|| "Picture".into()),
            ItemKind::Board { board_id } => self.board_names.get(board_id).cloned().unwrap_or_else(|| "Board".into()),
            kind => item_text(kind).map(|t| t.lines().next().unwrap_or("").to_owned()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Note".into()),
        }
    }

    fn panel_layout(&mut self) -> Option<panel::Layout> {
        let info = self.panel_info()?;
        Some(panel::layout(&mut self.font, &info, self.panel_edit.as_ref(), self.viewport))
    }

    fn panel_action(&mut self, action: panel::Action) {
        use panel::{Action, Field, FieldEdit};
        self.needs_redraw = true;
        if !matches!(action, Action::None | Action::Edit(_)) && self.panel_edit.is_some() {
            self.commit_panel_edit();
        }
        let id = self.selection.iter().next().copied();
        match action {
            Action::None => {}
            Action::Close => {
                self.panel_edit = None;
                self.panel_open = false;
            }
            Action::Edit(field) => {
                if self.panel_edit.as_ref().is_some_and(|e| e.field == field) {
                    return;
                }
                self.commit_panel_edit();
                self.end_edit();
                let text = match field {
                    Field::Title => self.panel_info().and_then(|i| i.title).unwrap_or_default(),
                    Field::Tag => String::new(),
                    Field::Marker(k) => id.and_then(|id| self.media_hash(&id)).and_then(|h| self.video_markers(&h).get(k).map(|m| m.label.clone())).unwrap_or_default(),
                };
                self.panel_edit = Some(FieldEdit { field, text });
            }
            Action::RemoveTag(tag) => {
                if let Some(hash) = id.and_then(|id| self.media_hash(&id)) {
                    self.update_sidecar(&hash, |sc| sc.tags.retain(|t| *t != tag));
                }
            }
            Action::Color(c) => self.set_color_value(c),
            Action::SeekMarker(k) => {
                if let Some(hash) = id.and_then(|id| self.media_hash(&id)) {
                    if let Some(m) = self.video_markers(&hash).get(k) {
                        self.video_commands.push((hash.clone(), VideoCommand::Pause));
                        self.video_commands.push((hash, VideoCommand::Seek(m.ms as f64 / 1000.0)));
                    }
                }
            }
            Action::RemoveMarker(k) => {
                if let Some(hash) = id.and_then(|id| self.media_hash(&id)) {
                    self.update_sidecar(&hash, |sc| {
                        if k < sc.markers.len() {
                            sc.markers.remove(k);
                        }
                    });
                }
            }
            Action::AcceptTag(t) => {
                if let Some(hash) = id.and_then(|id| self.media_hash(&id)) {
                    self.update_sidecar(&hash, |sc| {
                        if !sc.tags.contains(&t) {
                            sc.tags.push(t.clone());
                        }
                        sc.auto_tags.retain(|a| a.tag != t);
                    });
                }
            }
            Action::DismissTag(t) => {
                if let Some(hash) = id.and_then(|id| self.media_hash(&id)) {
                    self.update_sidecar(&hash, |sc| sc.auto_tags.retain(|a| a.tag != t));
                }
            }
            Action::FilterTag(t) => {
                let same = self.tag_filter.as_ref() == Some(&t);
                self.filter_by_tag((!same).then_some(t));
            }
            Action::PlayPause => {
                if let Some(hash) = id.and_then(|id| self.video_of(&id)) {
                    self.toggle_play(hash);
                }
            }
            Action::ClearRange => {
                if let Some(hash) = id.and_then(|id| self.video_of(&id)) {
                    self.set_video_range(&hash, None);
                }
            }
            Action::Speed(s) => {
                if let Some(hash) = id.and_then(|id| self.video_of(&id)) {
                    self.video_commands.push((hash, VideoCommand::Speed(s)));
                }
            }
            Action::OpenBoard => {
                if let Some(url) = id.and_then(|id| match &self.doc.item(&id)?.kind {
                    ItemKind::Link { url, .. } => Some(url.clone()),
                    _ => None,
                }) {
                    self.open_url(url);
                }
                if let Some(board_id) = id.and_then(|id| match self.doc.item(&id)?.kind {
                    ItemKind::Board { board_id } => Some(board_id),
                    _ => None,
                }) {
                    self.open_board(board_id, true);
                }
            }
        }
    }

    /// Applies what was typed into the panel field.
    fn commit_panel_edit(&mut self) {
        use panel::Field;
        let Some(e) = self.panel_edit.take() else { return };
        self.needs_redraw = true;
        let text = e.text.trim().to_owned();
        if self.selection.is_empty() {
            if e.field == Field::Title && !text.is_empty() {
                let id = self.doc.board.id;
                self.doc.board.name = text.clone();
                self.doc.version += 1;
                self.board_names.insert(id, text);
            }
            return;
        }
        let Some(item) = self.selection.iter().next().and_then(|id| self.doc.item(id)).cloned() else { return };
        if let (Field::Marker(k), ItemKind::Media { hash, .. }) = (&e.field, &item.kind) {
            let (k, hash) = (*k, hash.clone());
            self.update_sidecar(&hash, |sc| {
                if let Some(m) = sc.markers.get_mut(k) {
                    m.label = text;
                }
            });
            return;
        }
        match (e.field, &item.kind) {
            (Field::Tag, ItemKind::Media { hash, .. }) => {
                let hash = hash.clone();
                // Commas add several tags at once.
                let new: Vec<String> = text.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect();
                self.update_sidecar(&hash, |sc| {
                    for t in new {
                        if !sc.tags.contains(&t) {
                            sc.tags.push(t);
                        }
                    }
                });
            }
            (Field::Title, ItemKind::Media { hash, .. }) => {
                let hash = hash.clone();
                self.update_sidecar(&hash, |sc| sc.title = (!text.is_empty()).then_some(text));
            }
            (Field::Title, ItemKind::Group { .. }) => {
                let mut after = item.clone();
                if let ItemKind::Group { name, .. } = &mut after.kind {
                    *name = text;
                }
                self.doc.replace(vec![after]);
            }
            (Field::Title, ItemKind::Palette { .. }) => {
                let mut after = item.clone();
                if let ItemKind::Palette { name, .. } = &mut after.kind {
                    *name = text;
                }
                self.doc.replace(vec![after]);
            }
            (Field::Title, ItemKind::Link { .. }) => {
                let mut after = item.clone();
                if let ItemKind::Link { title, .. } = &mut after.kind {
                    *title = text;
                }
                self.doc.replace(vec![after]);
            }
            (Field::Title, ItemKind::Board { board_id }) if !text.is_empty() => self.rename_board(*board_id, text),
            _ => {}
        }
    }

    /// Changes a picture's or video's details and saves them to the library.
    fn update_sidecar(&mut self, hash: &ContentHash, f: impl FnOnce(&mut ItemSidecar)) {
        let Some(sc) = self.sidecars.get_mut(hash) else { return };
        f(sc);
        if let Some(lib) = &self.library {
            if let Err(e) = lib.write_sidecar(sc) {
                eprintln!("wall: could not save details: {e}");
            }
        }
        if self.search.is_some() {
            self.run_search();
        }
        self.scene.invalidate();
        self.needs_redraw = true;
    }

    /// Renames a board inside this one.
    pub fn rename_board(&mut self, id: Uuid, name: String) {
        self.board_names.insert(id, name.clone());
        if let Some((doc, _)) = self.parked.get_mut(&id) {
            doc.board.name = name.clone();
        }
        if let Some(lib) = &self.library {
            let board = match self.parked.get(&id) {
                Some((doc, _)) => Ok(doc.board.clone()),
                None => lib.read_board(id),
            };
            match board {
                Ok(mut b) => {
                    b.name = name;
                    if let Err(e) = lib.write_board(&b) {
                        eprintln!("wall: could not rename board: {e}");
                    }
                }
                Err(e) => eprintln!("wall: could not rename board: {e}"),
            }
        }
        self.scene.invalidate();
        self.needs_redraw = true;
    }

    fn draw_panel(&mut self, renderer: &mut Renderer) {
        let Some(l) = self.panel_layout() else { return };
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let mut boxes: Vec<wall_render::BoxInstance> = l
            .boxes
            .iter()
            .map(|b| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] })
            .collect();
        if let Some(c) = l.caret {
            boxes.push(wall_render::BoxInstance { rect: c, fill: [0.36, 0.62, 1.0, 1.0], stroke: [0.0; 4], shape: [0.0, 0.0] });
        }
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}

impl App {
    // ---- tool rail ---------------------------------------------------------

    fn rail_layout(&mut self) -> toolbar::Layout {
        let dragging = match &self.gesture {
            Gesture::RailDrag { tool } => Some(*tool),
            _ => None,
        };
        let inset = self.left_inset();
        let side_open = self.sidebar.is_some();
        let font = &mut self.font;
        toolbar::layout(self.tool, side_open, self.cursor, dragging, &self.rail_place, self.viewport, inset, &mut |s| panel::text_width(font, s, 12.0))
    }

    /// Where the tool rail floats, as "x,y,horizontal,folded" for settings.
    pub fn rail_setting(&self) -> String {
        self.rail_place.to_setting()
    }

    /// True while a drag of any kind is under way.
    pub fn busy_dragging(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle)
    }

    /// The tool rail and the left panel as one settings string:
    /// "x,y,horizontal,folded;boards|library|" .
    pub fn ui_setting(&self) -> String {
        let side = match self.sidebar_tab() {
            Some(sidebar::Tab::Boards) => "boards",
            Some(sidebar::Tab::Library) => "library",
            None => "",
        };
        format!("{};{side}|{}|w{}", self.rail_place.to_setting(), self.pen.to_setting(), self.welcome_seen)
    }

    /// Restores [`Self::ui_setting`].
    pub fn set_ui_setting(&mut self, s: &str) {
        // "|wN" at the end: which walkthroughs were seen (none if missing).
        let s = match s.rsplit_once("|w").map(|(rest, bits)| (rest, bits.parse::<u8>())) {
            Some((rest, Ok(bits))) => {
                self.welcome_seen = bits;
                rest
            }
            _ => s,
        };
        let (s, pen) = s.split_once('|').unwrap_or((s, ""));
        if let Some(p) = brush::PenState::from_setting(pen) {
            self.pen = p;
        }
        let (rail, side) = s.split_once(';').unwrap_or((s, ""));
        self.set_rail_setting(rail);
        match side {
            "boards" if self.sidebar.is_none() => self.toggle_sidebar(sidebar::Tab::Boards),
            "library" if self.sidebar.is_none() => self.toggle_sidebar(sidebar::Tab::Library),
            _ => {}
        }
    }

    /// Puts the tool rail back where a setting says (ignored if garbled).
    pub fn set_rail_setting(&mut self, s: &str) {
        if let Some(p) = toolbar::Placement::from_setting(s) {
            self.rail_place = p;
            self.needs_redraw = true;
        }
    }

    /// The tool under the pointer was pressed: modes switch, things to make
    /// start a drag (a click without moving makes it mid-view).
    fn rail_click(&mut self, tool: toolbar::Tool) {
        use toolbar::Tool;
        self.end_edit();
        match tool {
            Tool::Select => self.tool = Tool::Select,
            Tool::Arrow | Tool::Pen | Tool::Comment | Tool::Stamp => self.tool = if self.tool == tool { Tool::Select } else { tool },
            Tool::Import => self.run_command(menu::Command::Import),
            t => self.gesture = Gesture::RailDrag { tool: t },
        }
        self.needs_redraw = true;
    }

    /// Makes what `tool` makes, centred near board point `at`, and selects it
    /// (notes, text and to-dos start in typing mode).
    pub fn make_with_tool(&mut self, tool: toolbar::Tool, at: [f32; 2]) {
        use toolbar::Tool;
        self.tool = Tool::Select;
        match tool {
            Tool::Note => {
                self.new_note(at, String::new());
            }
            Tool::Todo => {
                self.new_todo(at);
            }
            Tool::Text => {
                self.new_text(at);
            }
            Tool::Link => {
                self.new_link(at, String::new());
            }
            Tool::Palette => {
                let colors = self.selection_colors();
                self.new_palette(at, colors);
            }
            Tool::Group => {
                let s = self.card_scale();
                let (w, h) = (480.0 * s, 320.0 * s);
                let id = self.add_group("Group", [at[0] as f64 - 20.0 * s, at[1] as f64 - 20.0 * s, w, h]);
                self.selection = [id].into();
            }
            Tool::Board => {
                // new_board places the card under the cursor.
                let z = self.camera.zoom;
                let screen = [(at[0] - self.camera.center[0]) * z + self.viewport[0] / 2.0, (at[1] - self.camera.center[1]) * z + self.viewport[1] / 2.0];
                let cursor = std::mem::replace(&mut self.cursor, screen);
                self.new_board();
                self.cursor = cursor;
            }
            Tool::Select | Tool::Arrow | Tool::Pen | Tool::Comment | Tool::Stamp | Tool::Import => {}
        }
        self.needs_redraw = true;
    }

    /// Turns a pen line (board units) into a drawing item. One undo step.
    pub fn add_stroke(&mut self, line: &[[f32; 2]]) -> Option<Uuid> {
        let width = PEN_PX / self.camera.zoom;
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in line {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        if line.is_empty() {
            return None;
        }
        // Pad so a straight line still has a box with some size.
        let pad = width;
        let (x, y, w, h) = (b[0] - pad, b[1] - pad, b[2] - b[0] + 2.0 * pad, b[3] - b[1] + 2.0 * pad);
        let points = line.iter().map(|p| [(p[0] - x) / w, (p[1] - y) / h]).collect();
        let frame = Frame { x: x as f64, y: y as f64, w: w as f64, h: h as f64, z: self.doc.top_z() + 1.0, ..Default::default() };
        let id = self.doc.add(vec![(frame, ItemKind::Stroke { points, width, color: None, brush: None, widths: Vec::new(), on: None, drawing: None })])[0];
        self.needs_redraw = true;
        Some(id)
    }

    /// Colours of the selected pictures and videos (their extracted
    /// palettes, first ones first, no repeats, at most 8), or a starter set.
    fn selection_colors(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for i in self.selected_items() {
            if let ItemKind::Media { hash, .. } = &i.kind {
                for c in self.sidecars.get(hash).map(|s| s.colors.clone()).unwrap_or_default() {
                    if !out.contains(&c) && out.len() < 8 {
                        out.push(c);
                    }
                }
            }
        }
        if out.is_empty() {
            out = ["#1d1f24", "#3d5a80", "#98c1d9", "#e0fbfc", "#ee6c4d"].map(String::from).to_vec();
        }
        out
    }

    /// A palette card at a board point, selected. One undo step.
    pub fn new_palette(&mut self, at: [f32; 2], colors: Vec<String>) -> Uuid {
        self.end_edit();
        let s = self.card_scale();
        let w = (20.0 + 62.0 * colors.len().max(2) as f64) * s;
        let frame = Frame { x: at[0] as f64 - 20.0 * s, y: at[1] as f64 - 20.0 * s, w, h: 110.0 * s, z: self.doc.top_z() + 1.0, text_scale: s, ..Default::default() };
        let id = self.doc.add(vec![(frame, ItemKind::Palette { colors, name: String::new() })])[0];
        self.selection = [id].into();
        self.needs_redraw = true;
        id
    }

    /// The colour of the palette swatch under a board point.
    fn swatch_at(&self, id: Uuid, p: [f32; 2]) -> Option<String> {
        let item = self.doc.item(&id)?;
        let ItemKind::Palette { colors, name } = &item.kind else { return None };
        let f = item.frame;
        // Laid out at the card's unscaled size, like to-dos.
        let s = if f.text_scale > 0.0 { f.text_scale as f32 } else { 1.0 };
        let rect = [f.x as f32, f.y as f32, f.w as f32 / s, f.h as f32 / s];
        let p = [rect[0] + (p[0] - rect[0]) / s, rect[1] + (p[1] - rect[1]) / s];
        let k = scene::palette_swatches(rect, colors.len(), !name.is_empty()).iter().position(|r| panel::inside(*r, p))?;
        colors.get(k).cloned()
    }

    /// A link card at a board point. Empty `url`: ready for typing one.
    pub fn new_link(&mut self, at: [f32; 2], url: String) -> Uuid {
        self.end_edit();
        let s = self.card_scale();
        let frame = Frame { x: at[0] as f64 - 20.0 * s, y: at[1] as f64 - 20.0 * s, w: 280.0 * s, h: 68.0 * s, z: self.doc.top_z() + 1.0, text_scale: s, ..Default::default() };
        let id = Uuid::new_v4();
        let caret = url.len();
        let item = BoardItem { id, frame, kind: ItemKind::Link { url, title: String::new() }, rev: 0, updated_ms: 0, updated_by: String::new(), deleted: false };
        self.doc.preview(item);
        self.editing = Some(Editing { id, caret, original: None, todo: false });
        self.selection = [id].into();
        self.needs_redraw = true;
        id
    }

    /// Asks the shell to open a web address (only http and https).
    pub fn open_url(&mut self, url: String) {
        let url = if url.contains("://") { url } else { format!("https://{url}") };
        let plain = !url.contains(|c: char| c.is_whitespace() || c.is_control() || c == '"' || c == '\'');
        if plain && (url.starts_with("https://") || url.starts_with("http://")) {
            self.shell_commands.push(menu::ShellCommand::OpenUrl(url));
        }
    }

    /// A line of text straight on the board (no card), ready for typing.
    pub fn new_text(&mut self, at: [f32; 2]) -> Uuid {
        self.end_edit();
        let s = self.card_scale();
        let frame = Frame { x: at[0] as f64 - 10.0 * s, y: at[1] as f64 - 24.0 * s, w: 160.0 * s, h: 48.0 * s, z: self.doc.top_z() + 1.0, text_scale: s, ..Default::default() };
        let id = Uuid::new_v4();
        let item = BoardItem { id, frame, kind: ItemKind::Label { text: String::new() }, rev: 0, updated_ms: 0, updated_by: String::new(), deleted: false };
        self.doc.preview(item);
        self.editing = Some(Editing { id, caret: 0, original: None, todo: false });
        self.selection = [id].into();
        self.needs_redraw = true;
        id
    }

    fn draw_rail(&mut self, renderer: &mut Renderer) {
        let l = self.rail_layout();
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<wall_render::BoxInstance> = l
            .boxes
            .iter()
            .map(|b| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] })
            .collect();
        renderer.draw_boxes(&view, &boxes);
        renderer.draw_curves(&view, &l.curves);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }

    // ---- menu bar ----------------------------------------------------------

    fn menu_layout(&mut self) -> menu::Layout {
        let title = self.breadcrumb().join(" › ");
        menu::layout(&mut self.font, self.menu_open, self.cursor, &title, self.viewport, self.desktop)
    }

    /// Handles a click on the menu bar or an open menu. Returns true when
    /// the click was used up (including a click that only closes a menu).
    fn menu_click(&mut self, pos: [f32; 2]) -> bool {
        let hit = self.menu_layout().hit_at(pos);
        let was_open = self.menu_open.take();
        match hit {
            Some(menu::Hit::Title(i)) => {
                self.menu_open = (was_open != Some(i)).then_some(i);
                true
            }
            Some(menu::Hit::Entry(c)) => {
                self.run_command(c);
                true
            }
            Some(menu::Hit::None) => true,
            None => was_open.is_some(),
        }
    }

    /// Commands for the shell, oldest first.
    pub fn take_shell_commands(&mut self) -> Vec<menu::ShellCommand> {
        std::mem::take(&mut self.shell_commands)
    }

    /// Runs a menu command.
    pub fn run_command(&mut self, c: menu::Command) {
        use menu::{Command as C, ShellCommand as S};
        self.needs_redraw = true;
        self.menu_open = None;
        let centre = [self.viewport[0] / 2.0, self.viewport[1] / 2.0];
        match c {
            C::Import => self.shell_commands.push(S::Import),
            C::OpenLibrary => self.shell_commands.push(S::OpenLibrary { new: false }),
            C::NewLibrary => self.shell_commands.push(S::OpenLibrary { new: true }),
            C::ExportImage => self.shell_commands.push(S::ExportImage),
            C::ExportPdf => self.shell_commands.push(S::ExportPdf),
            C::Share => self.open_share(),
            C::WelcomeTour => self.start_welcome(welcome::Kind::Full),
            C::History => self.open_history(history::Tab::Versions),
            C::DeletedBoards => self.open_history(history::Tab::Deleted),
            C::ImportOldWall => self.shell_commands.push(S::ImportOldWall),
            C::Cut => self.shell_commands.push(S::Copy { cut: true }),
            C::Copy => self.shell_commands.push(S::Copy { cut: false }),
            C::Paste => self.shell_commands.push(S::Paste),
            C::NewNote => {
                let at = self.camera.center;
                self.new_note(at, String::new());
            }
            C::NewTodo => {
                let at = self.camera.center;
                self.new_todo(at);
            }
            C::NewBoard => {
                let cursor = std::mem::replace(&mut self.cursor, centre);
                self.new_board();
                self.cursor = cursor;
            }
            C::Undo => self.key(Key::Undo),
            C::Redo => self.key(Key::Redo),
            C::Duplicate => self.key(Key::Duplicate),
            C::Delete => self.key(Key::Delete),
            C::SelectAll => self.key(Key::SelectAll),
            C::Find => self.key(Key::Find),
            C::BringToFront => self.key(Key::BringToFront),
            C::SendToBack => self.key(Key::SendToBack),
            C::FitBoard => self.fit_view(),
            C::ZoomIn => self.camera.zoom_at(1.25, centre, self.viewport),
            C::ZoomOut => self.camera.zoom_at(0.8, centre, self.viewport),
            C::ActualSize => {
                let z = self.camera.zoom;
                self.camera.zoom_at(1.0 / z, centre, self.viewport);
            }
            C::TogglePanel => self.panel_open = !self.panel_open,
            C::ToggleLibrary => self.toggle_sidebar(sidebar::Tab::Library),
            C::ToggleBoards => self.toggle_sidebar(sidebar::Tab::Boards),
            C::ToggleDrawing => self.toggle_drawing(),
            C::ToggleTheme => self.set_light_mode(!theme::is_light()),
            C::ToggleGrid => self.set_grid(!self.show_grid),
            C::RotateRight => self.change_look(|l| l.turns = (l.turns + 1) % 4),
            C::RotateLeft => self.change_look(|l| l.turns = (l.turns + 3) % 4),
            C::Mirror => self.change_look(|l| l.mirror = !l.mirror),
            C::FlipVertical => self.change_look(|l| l.flip_v = !l.flip_v),
            C::BlackAndWhite => self.change_look(|l| l.grayscale = !l.grayscale),
            C::GoUp => self.go_up(),
            C::TidyRows => _ = self.arrange(arrange::Arrange::Rows),
            C::TidySameHeight => _ = self.arrange(arrange::Arrange::SameHeight),
            C::TidyPack => _ = self.arrange(arrange::Arrange::Pack),
            C::TidyColour => _ = self.arrange(arrange::Arrange::Colour),
        }
    }

    fn draw_menu(&mut self, renderer: &mut Renderer) {
        let l = self.menu_layout();
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<wall_render::BoxInstance> = l
            .boxes
            .iter()
            .map(|b| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] })
            .collect();
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}

/// A to-do list as editable text: title, then one entry per line.
fn todo_to_text(title: &str, entries: &[wall_format::TodoEntry]) -> String {
    let mut s = title.to_owned();
    for e in entries {
        s.push('\n');
        s.push_str(&e.text);
    }
    s
}

/// Text back to a to-do list. An entry keeps its tick when an entry with
/// the same text was ticked.
fn text_to_todo(text: &str, old: &[wall_format::TodoEntry]) -> (String, Vec<wall_format::TodoEntry>) {
    let mut lines = text.lines();
    let title = lines.next().unwrap_or("").trim().to_owned();
    let mut used = vec![false; old.len()];
    let texts: Vec<String> = lines.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect();
    let mut entries: Vec<wall_format::TodoEntry> = texts.iter().map(|t| wall_format::TodoEntry { text: t.clone(), done: false }).collect();
    for e in entries.iter_mut() {
        if let Some(j) = old.iter().enumerate().position(|(j, o)| !used[j] && o.text == e.text) {
            used[j] = true;
            e.done = old[j].done;
        }
    }
    (title, entries)
}

/// Fills in main colours and local auto-tags (colour, shape, length).
/// `rgba` is the decoded picture, when there is one.
fn auto_tag(sc: &mut ItemSidecar, rgba: Option<&[u8]>) {
    use wall_media::autotag;
    if let Some(px) = rgba {
        if sc.colors.is_empty() {
            sc.colors = autotag::dominant_colors(px, sc.width, sc.height, 4);
        }
    }
    let fresh = autotag::local_tags(&sc.colors, sc.width, sc.height, sc.duration_ms);
    for t in fresh {
        if !sc.auto_tags.iter().any(|a| a.tag == t.tag) {
            sc.auto_tags.push(t);
        }
    }
}

/// A picture's own tags and suggested tags, without repeats.
fn sidecar_tags(sc: &ItemSidecar) -> impl Iterator<Item = &str> {
    sc.tags.iter().map(String::as_str).chain(sc.auto_tags.iter().map(|a| a.tag.as_str()).filter(|t| !sc.tags.iter().any(|u| u == t)))
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

/// "turned 90°, mirrored, black and white".
fn describe_look(look: &wall_format::Look) -> String {
    let mut parts = Vec::new();
    if !look.turns.is_multiple_of(4) {
        parts.push(format!("turned {}°", (look.turns % 4) as u32 * 90));
    }
    if look.mirror {
        parts.push("mirrored".into());
    }
    if look.flip_v {
        parts.push("upside down".into());
    }
    if look.grayscale {
        parts.push("black and white".into());
    }
    let s = parts.join(", ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Pasted text that is just one web address.
fn as_url(text: &str) -> Option<String> {
    let t = text.trim();
    let ok = (t.starts_with("https://") || t.starts_with("http://")) && t.len() > 10 && !t.contains(char::is_whitespace);
    ok.then(|| t.to_owned())
}

/// Pen line width in screen pixels when drawn.
const PEN_PX: f32 = 3.0;

/// Pasted text that is only hex colours ("#aabbcc", "aabbcc", "#abc"; two or
/// more, split by spaces, commas or lines), as "#rrggbb".
fn as_hex_colors(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for w in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|w| !w.is_empty()) {
        let h = w.trim_start_matches('#');
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let full = match h.len() {
            6 => h.to_lowercase(),
            3 => h.chars().flat_map(|c| [c, c]).collect::<String>().to_lowercase(),
            _ => return None,
        };
        out.push(format!("#{full}"));
    }
    (out.len() >= 2 && out.len() <= 32).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_made_zoomed_out_are_scaled_up() {
        let mut a = app();
        let id = a.new_note([0.0, 0.0], "near".into());
        assert_eq!(a.doc.item(&id).unwrap().frame.text_scale, 1.0);
        a.end_edit();
        a.camera.zoom = 0.25;
        let id = a.new_note([0.0, 0.0], "far".into());
        a.end_edit();
        let f = a.doc.item(&id).unwrap().frame;
        assert_eq!(f.text_scale, 4.0);
        assert_eq!(f.w, 960.0);
        let style = scene::TextStyle::for_item(a.doc.item(&id).unwrap()).unwrap();
        assert_eq!(style.size, 64.0);
        assert!(f.h >= style.at[1] as f64, "the card grew to fit its scaled text: {}", f.h);
    }

    #[test]
    fn export_size_fits_the_board() {
        let mut a = app();
        assert_eq!(a.export_size(4096), None, "empty board");
        a.new_note([0.0, 0.0], "a".into());
        let (size, zoom) = a.export_size(4096).unwrap();
        assert_eq!(zoom, 2.0, "small boards export at 2x");
        assert!(size[0] < 4096 && size[1] < 4096);
        a.new_note([20000.0, 3000.0], "far".into());
        let (size, zoom) = a.export_size(4096).unwrap();
        assert!(zoom < 1.0 && size[0] <= 4096 && size[1] < size[0], "{size:?} {zoom}");
        assert_eq!(a.board_name(), "t");
    }

    fn app() -> App {
        let mut a = App::new(Board::new("t", 0), "test");
        a.resize([1000.0, 800.0]);
        a.camera = Camera { center: [500.0, 400.0], zoom: 1.0 }; // screen == board
        a.panel_open = false; // keep the right of the test window for the board
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
    fn video_markers_are_kept_and_jumped_between() {
        let mut a = app();
        let meta = VideoMeta { width: 400, height: 200, duration_ms: 8000, fps: Some(25.0) };
        a.pointer_move([500.0, 400.0]);
        let id = a.import_video(b"clip", "mp4", meta, None, 1).unwrap();
        let hash = a.media_hash(&id).unwrap();
        for t in [1.0, 4.0] {
            a.set_video_status(hash.clone(), VideoStatus { time: t, duration: 8.0, playing: false, speed: 1.0 });
            a.key(Key::Char('m'));
        }
        assert_eq!(a.video_markers(&hash).iter().map(|m| m.ms).collect::<Vec<_>>(), vec![1000, 4000]);
        a.take_video_commands();
        a.set_video_status(hash.clone(), VideoStatus { time: 2.0, duration: 8.0, playing: false, speed: 1.0 });
        a.key(Key::Char('\''));
        a.key(Key::Char(';'));
        assert_eq!(a.take_video_commands(), vec![(hash.clone(), VideoCommand::Seek(4.0)), (hash.clone(), VideoCommand::Seek(1.0))]);
        a.panel_open = true;
        assert_eq!(a.panel_info().unwrap().video.unwrap().markers, vec![(1.0, String::new()), (4.0, String::new())]);
        // Naming the second one in the side panel.
        a.panel_action(panel::Action::Edit(panel::Field::Marker(1)));
        a.text_input("Contact");
        a.key(Key::Enter);
        assert_eq!(a.video_markers(&hash)[1].label, "Contact");
        a.take_video_commands();
        a.panel_action(panel::Action::SeekMarker(1));
        assert_eq!(a.take_video_commands(), vec![(hash.clone(), VideoCommand::Pause), (hash.clone(), VideoCommand::Seek(4.0))]);
        a.panel_action(panel::Action::RemoveMarker(0));
        assert_eq!(a.video_markers(&hash).len(), 1);
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
        let id = a.doc.add(vec![(f, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
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
        // The top-left handle: the bottom-right corner stays put.
        let (x1, y1) = (fr.x + fr.w, fr.y + fr.h);
        a.pointer_down(Button::Primary, [fr.x as f32, fr.y as f32], Mods::default());
        a.pointer_move([fr.x as f32 - 40.0, fr.y as f32 + 50.0]);
        a.pointer_up(Button::Primary, [fr.x as f32 - 40.0, fr.y as f32 + 50.0]);
        let g = a.doc.item(&n).unwrap().frame;
        assert_eq!((g.w, g.h, g.x + g.w, g.y + g.h), (200.0, 250.0, x1, y1));
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
        a.scene.update(&a.doc, &mut a.font, &a.board_names);
        assert_eq!(a.scene.run_count(), 1, "nothing overlaps in the demo grid: one batch");
        // A note dragged over a picture, then the picture brought forward:
        // the picture must draw after (above) the note.
        let mut b = app();
        let hash = b.add_picture(vec![0; 4], 1, 1);
        let f = Frame { x: 100.0, y: 100.0, w: 200.0, h: 100.0, ..Default::default() };
        let pic = b.doc.add(vec![(f, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
        add_note(&mut b, 150.0, 120.0);
        b.selection = [pic].into();
        b.key(Key::BringToFront);
        b.scene.update(&b.doc, &mut b.font, &b.board_names);
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
    fn at_most_sixteen_videos_play_selected_and_largest_first() {
        let mut a = app();
        a.camera = Camera { center: [1000.0, 800.0], zoom: 0.5 };
        let meta = VideoMeta { width: 320, height: 180, duration_ms: 1000, fps: Some(25.0) };
        let mut ids = Vec::new();
        for n in 0..20u32 {
            let id = a.import_video(&n.to_le_bytes(), "mp4", meta, None, 1).unwrap();
            // Grid of 5 x 4, the last one smallest (still playable size).
            let w = if n == 19 { 260.0 } else { 300.0 + n as f64 };
            let mut item = a.doc.item(&id).unwrap().clone();
            item.frame = Frame { x: (n % 5) as f64 * 400.0, y: (n / 5) as f64 * 400.0, w, h: w * 0.5625, ..Default::default() };
            a.doc.preview(item.clone());
            a.doc.commit_edit(None, item);
            ids.push(id);
        }
        a.selection = [ids[19]].into();
        let playing = a.playable_videos();
        assert_eq!(playing.len(), MAX_PLAYING);
        assert!(playing.contains(&a.media_hash(&ids[19]).unwrap()), "the selected video plays even though it is smallest");
        assert!(!playing.contains(&a.media_hash(&ids[0]).unwrap()), "the smallest unselected ones wait");
    }

    #[test]
    fn moving_snaps_to_nearby_edges_unless_ctrl_is_held() {
        let mut a = app();
        let moved = add_note(&mut a, 100.0, 100.0);
        add_note(&mut a, 400.0, 300.0);
        // Left edge ends 3 px short of the other note's left edge: snaps.
        a.pointer_down(Button::Primary, [150.0, 150.0], Mods::default());
        a.pointer_move([447.0, 200.0]);
        assert!(matches!(&a.gesture, Gesture::Move { guides, .. } if guides.len() == 1));
        a.pointer_up(Button::Primary, [447.0, 200.0]);
        assert_eq!(a.doc.item(&moved).unwrap().frame.x, 400.0);
        // The same drag with Ctrl/Cmd held lands exactly where the pointer says.
        a.key(Key::Undo);
        a.set_time(10_000); // not a double-click
        a.pointer_down(Button::Primary, [150.0, 150.0], Mods { command: true, ..Mods::default() });
        a.pointer_move([447.0, 200.0]);
        a.pointer_up(Button::Primary, [447.0, 200.0]);
        assert_eq!(a.doc.item(&moved).unwrap().frame.x, 397.0);
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
    fn pictures_turn_mirror_and_go_grey_in_one_undo_step_each() {
        let mut a = app();
        let f = Frame { x: 100.0, y: 100.0, w: 400.0, h: 200.0, ..Default::default() };
        let hash = a.add_picture(vec![0; 4], 1, 1);
        let id = a.doc.add(vec![(f, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
        let look = |a: &App| match &a.doc.item(&id).unwrap().kind {
            ItemKind::Media { look, .. } => *look,
            _ => unreachable!(),
        };
        a.selection = [id].into();
        a.key(Key::Char('r'));
        // A quarter turn swaps width and height around the centre (300, 200).
        let g = a.doc.item(&id).unwrap().frame;
        assert_eq!((g.x, g.y, g.w, g.h), (200.0, 0.0, 200.0, 400.0));
        assert_eq!(look(&a).turns, 1);
        assert!(a.crop_corner_at([200.0, 0.0]).is_none(), "no cropping while turned");
        a.key(Key::Char('h'));
        a.run_command(menu::Command::BlackAndWhite);
        assert_eq!(look(&a), wall_format::Look { turns: 1, mirror: true, flip_v: false, grayscale: true });
        a.panel_open = true;
        assert_eq!(a.panel_info().unwrap().rows[0], ("Shown".into(), "Turned 90°, mirrored, black and white".into()));
        a.key(Key::Undo);
        a.key(Key::Undo);
        a.key(Key::Undo);
        assert!(look(&a).is_plain());
        assert_eq!(a.doc.item(&id).unwrap().frame.w, 400.0);
        // Notes are left alone.
        let n = add_note(&mut a, 600.0, 100.0);
        let before = a.doc.item(&n).cloned();
        a.selection = [n].into();
        a.key(Key::Char('r'));
        assert_eq!(a.doc.item(&n).cloned(), before);
    }

    #[test]
    fn ctrl_drag_corner_crops_without_rescaling() {
        let mut a = app();
        let f = Frame { x: 100.0, y: 100.0, w: 400.0, h: 200.0, ..Default::default() };
        let hash = a.add_picture(vec![0; 4], 1, 1);
        let id = a.doc.add(vec![(f, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
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

    fn panel_click(a: &mut App, action: panel::Action) {
        let l = a.panel_layout().expect("panel open");
        let r = l.hits.iter().find(|(_, x)| *x == action).unwrap_or_else(|| panic!("no {action:?}")).0;
        click(a, 50_000, [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]);
    }

    #[test]
    fn side_panel_shows_and_edits_picture_details() {
        let mut a = App::demo(4);
        a.resize([1400.0, 900.0]);
        assert_eq!(a.panel_info().unwrap().kind, "Board", "nothing selected: the board panel");
        let pic = a
            .doc
            .board
            .live_items()
            .find(|i| matches!(&i.kind, ItemKind::Media { hash, .. } if a.sidecars[hash].title.as_deref() == Some("demo-frame-01.png")))
            .unwrap()
            .id;
        a.selection = [pic].into();
        let info = a.panel_info().unwrap();
        assert_eq!(info.kind, "Image");
        assert_eq!(info.title.as_deref(), Some("demo-frame-01.png"));
        assert!(info.rows.iter().any(|(k, v)| k == "Size" && v == "320 × 200 px"));
        assert_eq!(info.tags.as_ref().unwrap(), &vec!["warm".to_owned(), "demo".to_owned()]);

        // Add two tags by typing, remove one with its ×.
        panel_click(&mut a, panel::Action::Edit(panel::Field::Tag));
        assert!(a.is_editing());
        a.text_input("Hero shot, Night");
        a.key(Key::Enter);
        assert!(!a.is_editing());
        assert_eq!(a.panel_info().unwrap().tags.unwrap(), vec!["warm", "demo", "hero shot", "night"]);
        panel_click(&mut a, panel::Action::RemoveTag("demo".into()));
        assert_eq!(a.panel_info().unwrap().tags.unwrap(), vec!["warm", "hero shot", "night"]);
        assert_eq!(a.selection.len(), 1, "panel clicks don't change the selection");

        // Rename; search finds the new title and the new tags.
        panel_click(&mut a, panel::Action::Edit(panel::Field::Title));
        for _ in 0.."demo-frame-01.png".len() {
            a.key(Key::Backspace);
        }
        a.text_input("Opening shot");
        a.key(Key::Enter);
        assert_eq!(a.panel_info().unwrap().title.as_deref(), Some("Opening shot"));
        a.key(Key::Find);
        a.text_input("night");
        assert_eq!(a.search_status().unwrap().2, 1);
        a.key(Key::Escape);

        // P hides the panel and brings it back; × closes it.
        a.key(Key::Char('p'));
        assert!(a.panel_info().is_none());
        a.key(Key::Char('p'));
        panel_click(&mut a, panel::Action::Close);
        assert!(a.panel_info().is_none());
    }

    #[test]
    fn side_panel_colours_and_renames() {
        let mut a = app();
        a.panel_open = true;
        let n = add_note(&mut a, 0.0, 0.0);
        let m = add_note(&mut a, 120.0, 0.0);
        a.selection = [n, m].into();
        assert_eq!(a.panel_info().unwrap().kind, "2 items");
        panel_click(&mut a, panel::Action::Color(Some(panel::PALETTE[3].into())));
        for id in [n, m] {
            assert!(matches!(&a.doc.item(&id).unwrap().kind, ItemKind::Note { color: Some(c), .. } if c == panel::PALETTE[3]));
        }
        a.key(Key::Undo);
        assert!(matches!(&a.doc.item(&n).unwrap().kind, ItemKind::Note { color: None, .. }), "one undo step");

        // Rename a board card; the name shows on the card and in the breadcrumb.
        a.selection.clear();
        let inner = a.new_board();
        panel_click(&mut a, panel::Action::Edit(panel::Field::Title));
        while a.panel_edit.as_ref().is_some_and(|e| !e.text.is_empty()) {
            a.key(Key::Backspace);
        }
        a.text_input("Client picks");
        a.key(Key::Enter);
        assert_eq!(a.board_names[&inner], "Client picks");
        panel_click(&mut a, panel::Action::OpenBoard);
        assert_eq!(a.breadcrumb(), vec!["t", "Client picks"]);
    }

    #[test]
    fn tag_filter_dims_other_items() {
        let mut a = App::demo(4);
        a.resize([1400.0, 900.0]);
        let info = a.panel_info().unwrap();
        assert_eq!(info.kind, "Board");
        let tags = info.board_tags.unwrap();
        assert_eq!(tags[0], ("demo".to_owned(), 24));
        assert!(tags.contains(&("warm".to_owned(), 4)));
        panel_click(&mut a, panel::Action::FilterTag("warm".into()));
        assert_eq!(a.tag_filter.as_deref(), Some("warm"));
        // 20 pictures without "warm", 4 notes and the label are dimmed.
        assert_eq!(a.filtered_out().len(), 20 + 4 + 1);
        panel_click(&mut a, panel::Action::FilterTag("warm".into()));
        assert!(a.tag_filter.is_none(), "clicking the active tag clears it");
        a.filter_by_tag(Some("cool".into()));
        a.key(Key::Escape);
        assert!(a.tag_filter.is_none(), "Esc clears the filter");
        // Rename the board from the board panel.
        panel_click(&mut a, panel::Action::Edit(panel::Field::Title));
        a.text_input(" board");
        a.key(Key::Enter);
        assert_eq!(a.doc.board.name, "Demo board");
        assert_eq!(a.breadcrumb(), vec!["Demo board"]);
    }

    #[test]
    fn imported_pictures_get_suggested_tags() {
        let mut a = app();
        a.panel_open = true;
        // A tall, mostly blue picture.
        let img = image::RgbaImage::from_fn(60, 120, |_, y| if y < 100 { image::Rgba([30, 80, 220, 255]) } else { image::Rgba([240, 140, 40, 255]) });
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let id = a.import_image_bytes(&png, Some("sky.png"), 0).unwrap();
        a.selection = [id].into();
        let info = a.panel_info().unwrap();
        assert_eq!(info.suggested, vec!["blue", "orange", "portrait"]);
        assert_eq!(info.palette.len(), 2);
        panel_click(&mut a, panel::Action::AcceptTag("blue".into()));
        panel_click(&mut a, panel::Action::DismissTag("orange".into()));
        let info = a.panel_info().unwrap();
        assert_eq!(info.tags.unwrap(), vec!["blue"]);
        assert_eq!(info.suggested, vec!["portrait"]);
        // Suggested tags still count for search and the tag filter.
        a.selection.clear();
        assert!(a.board_tags().contains(&("portrait".to_owned(), 1)));
        a.filter_by_tag(Some("portrait".into()));
        assert!(a.filtered_out().is_empty());
    }

    #[test]
    fn menu_bar_opens_and_runs_commands() {
        let mut a = app();
        let n = add_note(&mut a, 300.0, 300.0);
        let title = |a: &mut App, i: usize| {
            let l = a.menu_layout();
            let r = l.hits.iter().find(|(_, h)| *h == menu::Hit::Title(i)).unwrap().0;
            [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
        };
        let entry = |a: &mut App, c: menu::Command| {
            let l = a.menu_layout();
            let r = l.hits.iter().find(|(_, h)| *h == menu::Hit::Entry(c)).unwrap().0;
            [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
        };
        // Edit > Select all, then Edit > Delete, then Edit > Undo.
        let p = title(&mut a, 1);
        click(&mut a, 1_000, p);
        assert_eq!(a.menu_open, Some(1));
        let p = entry(&mut a, menu::Command::SelectAll);
        click(&mut a, 2_000, p);
        assert_eq!(a.menu_open, None);
        assert_eq!(a.selection, [n].into());
        let p = title(&mut a, 1);
        click(&mut a, 3_000, p);
        let p = entry(&mut a, menu::Command::Delete);
        click(&mut a, 4_000, p);
        assert!(a.doc.item(&n).is_none());
        a.run_command(menu::Command::Undo);
        assert!(a.doc.item(&n).is_some());
        // A click outside an open menu only closes it.
        let p = title(&mut a, 3);
        click(&mut a, 5_000, p);
        click(&mut a, 6_000, [500.0, 600.0]);
        assert_eq!(a.menu_open, None);
        assert!(a.doc.board.live_items().count() == 1, "no note made by that click");
        // Copy goes to the shell; zoom works in place.
        a.run_command(menu::Command::Copy);
        assert_eq!(a.take_shell_commands(), vec![menu::ShellCommand::Copy { cut: false }]);
        a.run_command(menu::Command::ZoomIn);
        assert!((a.camera.zoom - 1.25).abs() < 1e-4);
        a.run_command(menu::Command::ActualSize);
        assert!((a.camera.zoom - 1.0).abs() < 1e-4);
    }

    #[test]
    fn in_and_out_points_loop_a_video() {
        let mut a = app();
        let id = a.import_video(&[1, 2, 3], "mp4", VideoMeta { width: 320, height: 180, duration_ms: 10_000, fps: Some(25.0) }, Some("clip.mp4"), 0).unwrap();
        let hash = a.media_hash(&id).unwrap();
        a.selection = [id].into();
        let at = |a: &mut App, t: f64| a.set_video_status(hash.clone(), VideoStatus { time: t, duration: 10.0, playing: true, speed: 1.0 });
        at(&mut a, 2.0);
        a.key(Key::Char('i'));
        at(&mut a, 6.5);
        a.key(Key::Char('o'));
        assert_eq!(a.video_range(&hash), Some((2.0, 6.5)));
        assert!(a.take_video_commands().contains(&(hash.clone(), VideoCommand::Range(Some((2.0, 6.5))))));
        assert_eq!(a.panel_info(), None, "panel closed in tests");
        a.panel_open = true;
        assert_eq!(a.panel_info().unwrap().video.unwrap().range, Some((2.0, 6.5)));
        // The range shows on the scrub bar (track, band, two ticks, ...).
        assert!(a.scrub_bars().len() >= 6);
        panel_click(&mut a, panel::Action::ClearRange);
        assert_eq!(a.video_range(&hash), None);
    }

    #[test]
    fn todo_cards_type_tick_and_keep_ticks() {
        let mut a = app();
        a.cursor = [100.0, 100.0];
        a.key(Key::Char('t'));
        assert!(a.is_editing());
        // Typing starts on the first entry, under the "To-do" title.
        a.text_input("Reference pass");
        a.key(Key::Enter);
        a.text_input("Grade");
        a.key(Key::Enter);
        a.text_input("Export");
        a.key(Key::Escape);
        let id = *a.selection.iter().next().unwrap();
        let ItemKind::Todo { title, entries } = a.doc.item(&id).unwrap().kind.clone() else { panic!("not a to-do") };
        assert_eq!(title, "To-do");
        assert_eq!(entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["Reference pass", "Grade", "Export"]);

        // Click the second tick box.
        let f = a.doc.item(&id).unwrap().frame;
        let b = scene::todo_box([f.x as f32, f.y as f32, f.w as f32, f.h as f32], 1);
        click(&mut a, 90_000, [b[0] + 8.0, b[1] + 8.0]);
        assert!(matches!(&a.doc.item(&id).unwrap().kind, ItemKind::Todo { entries, .. } if entries[1].done && !entries[0].done));

        // Edit as text: drop the first entry; "Grade" keeps its tick.
        a.begin_edit(id);
        a.edit(|t, c| {
            *t = "Shot list\nGrade\nExport\nUpload".into();
            *c = t.len();
        });
        a.end_edit();
        let ItemKind::Todo { title, entries } = a.doc.item(&id).unwrap().kind.clone() else { panic!() };
        assert_eq!(title, "Shot list");
        assert_eq!(entries.iter().map(|e| (e.text.as_str(), e.done)).collect::<Vec<_>>(), [("Grade", true), ("Export", false), ("Upload", false)]);
        a.key(Key::Undo);
        assert!(matches!(&a.doc.item(&id).unwrap().kind, ItemKind::Todo { entries, .. } if entries.len() == 3 && entries[0].text == "Reference pass"));

        // An untouched new list is dropped.
        a.key(Key::Char('t'));
        a.key(Key::Escape);
        assert_eq!(a.doc.board.live_items().count(), 1);
    }

    #[test]
    fn clicking_the_board_ends_panel_typing() {
        let mut a = app();
        a.panel_open = true;
        let n = add_note(&mut a, 0.0, 0.0);
        a.selection = [n].into();
        let g = a.doc.add(vec![(Frame { x: 0.0, y: 300.0, w: 300.0, h: 200.0, ..Default::default() }, ItemKind::Group { name: "Old".into(), color: None })])[0];
        a.selection = [g].into();
        panel_click(&mut a, panel::Action::Edit(panel::Field::Title));
        a.text_input(" refs");
        click(&mut a, 60_000, [100.0, 100.0]);
        assert!(!a.is_editing());
        assert!(matches!(&a.doc.item(&g).unwrap().kind, ItemKind::Group { name, .. } if name == "Old refs"));
        assert_eq!(a.selection, [n].into());
    }

    #[test]
    fn boards_inside_boards_open_edit_and_come_back() {
        let mut a = app();
        a.pointer_move([500.0, 400.0]);
        let child = a.new_board();
        let card = *a.selection.iter().next().unwrap();
        let root = a.board_id();
        // Double-click the card to open it.
        click(&mut a, 1000, [500.0, 400.0]);
        click(&mut a, 1100, [500.0, 400.0]);
        assert_eq!(a.board_id(), child);
        assert_eq!(a.breadcrumb(), vec!["t".to_owned(), "Board 2".to_owned()]);
        let n = add_note(&mut a, 0.0, 0.0);
        a.key(Key::Escape); // nothing selected: go up
        assert_eq!(a.board_id(), root);
        assert!(a.doc.item(&card).is_some());
        assert!(a.doc.item(&n).is_none(), "the note lives in the child board");
        a.open_board(child, true);
        assert!(a.doc.item(&n).is_some());
        a.key(Key::Undo); // undo history survived the trip
        assert!(a.doc.item(&n).is_none());
    }

    #[test]
    fn child_boards_save_to_the_library_and_the_root_opens_first() {
        let dir = tempfile::tempdir().unwrap();
        let root_path = dir.path().join("B.wall");
        let mut a = App::open_library(&root_path, "test", 1).unwrap();
        a.resize([1000.0, 800.0]);
        let root = a.board_id();
        let child = a.new_board();
        a.open_board(child, true);
        add_note(&mut a, 0.0, 0.0);
        a.go_up();
        a.save_if_dirty().unwrap();
        let b = App::open_library(&root_path, "test", 2).unwrap();
        assert_eq!(b.board_id(), root);
        let mut b = b;
        b.open_board(child, true);
        assert_eq!(b.item_count(), 1);
    }

    #[test]
    fn find_matches_words_steps_and_types_into_the_box() {
        let mut a = App::demo(40);
        a.resize([1000.0, 800.0]);
        a.key(Key::Find);
        assert!(a.is_editing(), "typing goes to the search box");
        a.text_input("neon");
        let (_, at, total) = a.search_status().unwrap();
        assert_eq!((at, total), (1, 5)); // notes 7, 15, 23, 31, 39 are "Neon #n"
        let first = *a.selection.iter().next().unwrap();
        a.key(Key::Enter);
        assert_ne!(*a.selection.iter().next().unwrap(), first);
        a.key(Key::Left);
        assert_eq!(*a.selection.iter().next().unwrap(), first);
        a.text_input(" #15");
        assert_eq!(a.search_status().unwrap().2, 1);
        a.key(Key::Backspace);
        a.key(Key::Backspace);
        a.key(Key::Backspace);
        assert_eq!(a.search_status().unwrap().0, "neon ");
        a.key(Key::Escape);
        assert!(a.search_status().is_none() && !a.is_editing());
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
    fn tool_rail_makes_things_by_click_or_drag_and_arms_the_arrow() {
        use toolbar::Tool;
        let mut a = app();
        let button = |a: &mut App, t: Tool| {
            let r = a.rail_layout().hits.iter().find(|(_, x)| *x == t).unwrap().0;
            [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
        };
        // A click on Note makes one in the middle of the view, ready for typing.
        let p = button(&mut a, Tool::Note);
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
        let id = a.editing.as_ref().unwrap().id;
        assert!(matches!(a.doc.item(&id).unwrap().kind, ItemKind::Note { .. }));
        a.text_input("hi");
        a.end_edit();
        // Dragging Group onto the board puts it where it was dropped.
        let p = button(&mut a, Tool::Group);
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_move([600.0, 400.0]);
        a.pointer_up(Button::Primary, [600.0, 400.0]);
        let g = a.doc.item(a.selection.iter().next().unwrap()).unwrap();
        assert!(matches!(g.kind, ItemKind::Group { .. }));
        let at = a.camera.screen_to_board([600.0, 400.0], a.viewport);
        assert_eq!((g.frame.x, g.frame.y), (at[0] as f64 - 20.0, at[1] as f64 - 20.0));
        // The arrow tool makes a plain drag draw one arrow, then goes back to Select.
        let p = button(&mut a, Tool::Arrow);
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
        assert_eq!(a.tool, Tool::Arrow);
        let before = a.item_count();
        a.pointer_down(Button::Primary, [200.0, 900.0], Mods::default());
        a.pointer_move([400.0, 950.0]);
        a.pointer_up(Button::Primary, [400.0, 950.0]);
        assert_eq!(a.item_count(), before + 1);
        assert_eq!(a.tool, Tool::Select);
    }

    #[test]
    fn pasted_address_makes_a_link_card_that_opens_on_double_click() {
        let mut a = app();
        a.cursor = [400.0, 300.0];
        a.paste_text("  https://www.example.com/refs?q=1 ");
        let id = *a.selection.iter().next().unwrap();
        let item = a.doc.item(&id).unwrap().clone();
        assert_eq!(item.kind, ItemKind::Link { url: "https://www.example.com/refs?q=1".into(), title: String::new() });
        assert_eq!(scene::link_site("https://www.example.com/refs?q=1"), "example.com");
        // A double-click asks the shell to open it.
        let p = [item.frame.x as f32 + 30.0, item.frame.y as f32 + 30.0];
        let screen = [(p[0] - a.camera.center[0]) * a.camera.zoom + a.viewport[0] / 2.0, (p[1] - a.camera.center[1]) * a.camera.zoom + a.viewport[1] / 2.0];
        a.take_shell_commands();
        for _ in 0..2 {
            a.pointer_down(Button::Primary, screen, Mods::default());
            a.pointer_up(Button::Primary, screen);
        }
        assert_eq!(a.take_shell_commands(), vec![menu::ShellCommand::OpenUrl("https://www.example.com/refs?q=1".into())]);
        // Ordinary text still becomes a note; odd schemes are never opened.
        a.paste_text("see https://example.com later");
        assert!(matches!(a.doc.item(a.selection.iter().next().unwrap()).unwrap().kind, ItemKind::Note { .. }));
        a.open_url("file:///C:/Windows/system32/calc.exe".into());
        a.open_url("https://a.com/\" x".into());
        assert!(a.take_shell_commands().is_empty());
        // The rail's link tool makes an empty card to type an address into.
        a.make_with_tool(toolbar::Tool::Link, [0.0, 0.0]);
        a.text_input("example.org");
        let id = a.editing.as_ref().unwrap().id;
        a.end_edit();
        assert!(matches!(&a.doc.item(&id).unwrap().kind, ItemKind::Link { url, .. } if url == "example.org"));
    }

    #[test]
    fn pen_draws_lines_that_are_picked_by_the_line_not_their_box() {
        let mut a = app();
        a.rail_click(toolbar::Tool::Pen);
        // An L shape: down the left, then along the bottom.
        let to_screen = |a: &App, p: [f32; 2]| [(p[0] - a.camera.center[0]) * a.camera.zoom + a.viewport[0] / 2.0, (p[1] - a.camera.center[1]) * a.camera.zoom + a.viewport[1] / 2.0];
        let path = [[100.0, 100.0], [100.0, 200.0], [100.0, 300.0], [200.0, 300.0], [300.0, 300.0]];
        a.pointer_down(Button::Primary, to_screen(&a, path[0]), Mods::default());
        for p in &path[1..] {
            a.pointer_move(to_screen(&a, *p));
        }
        a.pointer_up(Button::Primary, to_screen(&a, path[4]));
        let stroke = a.doc.board.live_items().find(|i| matches!(i.kind, ItemKind::Stroke { .. })).unwrap().clone();
        let ItemKind::Stroke { points, .. } = &stroke.kind else { unreachable!() };
        assert!(points.len() >= 5 && points.iter().all(|p| (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])));
        // Still drawing: a second line.
        assert_eq!(a.tool, toolbar::Tool::Pen);
        a.key(Key::Escape);
        assert_eq!(a.tool, toolbar::Tool::Select);
        assert_eq!(a.hit([101.0, 250.0]), Some(stroke.id), "on the line");
        assert_eq!(a.hit([250.0, 150.0]), None, "inside the box but far from the line");
        a.selection = [stroke.id].into();
        a.key(Key::Char('5'));
        assert!(matches!(&a.doc.item(&stroke.id).unwrap().kind, ItemKind::Stroke { color: Some(_), .. }), "number keys colour drawings");
        a.key(Key::Undo);
        // Moving it keeps the shape; undo takes it away.
        a.key(Key::Undo);
        assert!(a.doc.item(&stroke.id).is_none());
    }

    #[test]
    fn palettes_come_from_pasted_hex_or_selected_pictures_and_copy_on_click() {
        let mut a = app();
        assert_eq!(as_hex_colors("#FFF, 102030\n#aabbcc"), Some(vec!["#ffffff".into(), "#102030".into(), "#aabbcc".into()]));
        assert_eq!(as_hex_colors("#fff and #000"), None);
        assert_eq!(as_hex_colors("#fff"), None, "one colour is just text");
        a.cursor = [400.0, 300.0];
        a.paste_text("#ff0000 #00ff00 #0000ff");
        let id = *a.selection.iter().next().unwrap();
        let item = a.doc.item(&id).unwrap().clone();
        assert!(matches!(&item.kind, ItemKind::Palette { colors, .. } if colors.len() == 3));
        // Clicking the second swatch of the selected palette copies it.
        let f = item.frame;
        let r = scene::palette_swatches([f.x as f32, f.y as f32, f.w as f32, f.h as f32], 3, false)[1];
        let p = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        let screen = [(p[0] - a.camera.center[0]) * a.camera.zoom + a.viewport[0] / 2.0, (p[1] - a.camera.center[1]) * a.camera.zoom + a.viewport[1] / 2.0];
        a.take_shell_commands();
        a.pointer_down(Button::Primary, screen, Mods::default());
        a.pointer_up(Button::Primary, screen);
        assert_eq!(a.take_shell_commands(), vec![menu::ShellCommand::CopyText("#00ff00".into())]);
        // From the rail with a picture selected: the picture's colours.
        let hash = a.add_picture(vec![0; 4], 1, 1);
        a.sidecars.insert(
            hash.clone(),
            ItemSidecar {
                hash: hash.clone(),
                kind: MediaKind::Image,
                ext: "png".into(),
                width: 1,
                height: 1,
                bytes: 4,
                duration_ms: None,
                fps: None,
                added_ms: 0,
                title: None,
                source_url: None,
                tags: Vec::new(),
                auto_tags: Vec::new(),
                colors: vec!["#112233".into(), "#445566".into()],
                original: None,
                range: None,
                markers: Vec::new(),
                pingpong: false,
                extra: Default::default(),
            },
        );
        let pic = a.doc.add(vec![(Frame { x: 0.0, y: 0.0, w: 100.0, h: 100.0, ..Default::default() }, ItemKind::Media { hash, crop: None, look: Default::default() })])[0];
        a.selection = [pic].into();
        a.make_with_tool(toolbar::Tool::Palette, [500.0, 500.0]);
        let made = a.doc.item(a.selection.iter().next().unwrap()).unwrap();
        assert_eq!(made.kind, ItemKind::Palette { colors: vec!["#112233".into(), "#445566".into()], name: String::new() });
    }

    #[test]
    fn marquee_selects_contained_items_and_shift_adds() {
        let mut a = app();
        let n1 = add_note(&mut a, 100.0, 100.0);
        let n2 = add_note(&mut a, 300.0, 100.0);
        let n3 = add_note(&mut a, 600.0, 100.0);
        a.pointer_down(Button::Primary, [70.0, 50.0], Mods::default()); // just right of the tool rail
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
        a.set_grid(false); // no snapping to dots, so the move is exact
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

    /// Scene rebuild cost while dragging on a 5,000-note board. Run with
    /// `cargo test -p wall-app --release -- --ignored --nocapture scene_rebuild`.
    #[test]
    #[ignore]
    fn scene_rebuild_cost_5000() {
        let mut a = App::demo(5000);
        a.scene.update(&a.doc, &mut a.font, &a.board_names);
        let id = a.doc.board.live_items().find(|i| matches!(i.kind, ItemKind::Note { .. })).unwrap().id;
        let ids: BTreeSet<Uuid> = [id].into();
        let n = 30;
        let (mut edit, mut rebuild) = (0.0, 0.0);
        for _ in 0..n {
            let t = std::time::Instant::now();
            a.doc.translate(&ids, 1.0, 0.0);
            edit += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            a.scene.update(&a.doc, &mut a.font, &a.board_names);
            rebuild += t.elapsed().as_secs_f64();
        }
        let per = |s: f64| s * 1000.0 / n as f64;
        println!("{} items: edit {:.2} ms + scene rebuild {:.2} ms per drag frame", a.item_count(), per(edit), per(rebuild));
    }
}
