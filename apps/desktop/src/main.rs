//! Wall desktop shell (macOS, Windows). Opens a window with an OpenGL context,
//! hands input to `wall-app`, and draws with `wall-render`.
//!
//! Graphics: ANGLE (EGL + OpenGL ES 3.0 on Direct3D 11 / Metal) when
//! libEGL and libGLESv2 sit next to the executable, else desktop OpenGL 3.3
//! core. `WALL_GL=angle|gl` forces one. ANGLE runs the same GLSL ES 3.00 as
//! the web build.
//! Test hooks:
//! - `WALL_SCREENSHOT=path.png` renders a few frames, saves the window and exits.
//! - `WALL_CAMERA="x,y,zoom"` sets the start view.
//! - `WALL_BENCH=1` redraws continuously and prints fps (vsync off).
//! - `WALL_IMPORT="a.png;b.mp4"` imports files at start, as if dropped.
//! - `WALL_SCREENSHOT_FRAME=n` takes the screenshot at frame n (default 3).
//! - `WALL_EXPORT=board.png` also exports the whole board with the screenshot.
//! - `WALL_DEMO=n` opens a generated demo board with n notes instead of a library.
//! - `WALL_BOARD=<id>` opens that board of the library at start.
//! - `WALL_GPU_DECODE=1` decodes videos on the GPU (same as the
//!   `"gpu_video_decode": true` setting; off by default).
//!
//! Usage: `wall [path/to/Library.wall]`. Without a path it opens (or creates)
//! `Wall/Library.wall` in the home folder. Boards save automatically.

mod video;

use std::num::NonZeroU32;
use std::time::Instant;

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext, PossiblyCurrentContext, Version};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use wall_app::{App, Button, Key, Mods, Wheel};
use wall_render::{GlslTarget, Renderer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

struct Gfx {
    window: Window,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

struct Shell {
    app: App,
    gfx: Option<Gfx>,
    frames: u32,
    fps_clock: Instant,
    screenshot: Option<String>,
    bench: bool,
    mods: ModifiersState,
    cursor: [f32; 2],
    videos: video::Videos,
    /// Frame WALL_SCREENSHOT is taken at (WALL_SCREENSHOT_FRAME, default 3).
    screenshot_frame: u32,
    /// Video preview frames for the library drawer, decoded off the UI thread.
    posters: LibraryPosters,
    /// Window title: the library's name.
    title: String,
    /// An old-Wall import running in the background.
    old_import: Option<std::sync::mpsc::Receiver<Result<wall_import::old_wall::Report, String>>>,
    /// Keeps the library and its cloud copy the same, when it has one.
    cloud: Option<Cloud>,
    /// The library [`Self::cloud`] was last tried for.
    cloud_for: Option<std::path::PathBuf>,
    /// Answers from the share card's requests, running in the background.
    cloud_asks: Vec<std::sync::mpsc::Receiver<wall_app::CloudAnswer>>,
    /// The pictures of the drop in progress (one undo step).
    batch: Option<ImportBatch>,
    /// Light mode and grid as last saved to settings.
    saved_look: (bool, bool),
    /// Tool rail place and left panel, as last saved.
    saved_ui: String,
}

/// (hash, RGBA frame and its size, or None if ffmpeg couldn't read it).
type PosterResult = (wall_app::ContentHash, Option<(Vec<u8>, u32, u32)>);

/// Grabs one frame per video the library drawer asks for, on worker threads.
struct LibraryPosters {
    tx: std::sync::mpsc::Sender<PosterResult>,
    rx: std::sync::mpsc::Receiver<PosterResult>,
}

impl Default for LibraryPosters {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx }
    }
}

impl LibraryPosters {
    /// Starts decodes for frames the drawer wants and hands back finished ones.
    fn pump(&self, app: &mut App) {
        for (hash, path, w, h, at) in app.library_posters_wanted() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let frame = video::poster(&path, w, h, at).map(|px| (px, w, h));
                let _ = tx.send((hash, frame));
            });
        }
        while let Ok((hash, frame)) = self.rx.try_recv() {
            app.set_library_poster(hash, frame);
        }
    }
}

impl Shell {
    fn mods(&self) -> Mods {
        Mods { shift: self.mods.shift_key(), command: self.mods.control_key() || self.mods.super_key(), alt: self.mods.alt_key() }
    }
}

fn button(b: MouseButton) -> Option<Button> {
    match b {
        MouseButton::Left => Some(Button::Primary),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Right => Some(Button::Secondary),
        _ => None,
    }
}

/// Ctrl/Cmd+V: an image on the clipboard is imported like a dropped file;
/// text is typed into the note being edited, or becomes a new note.
fn paste(app: &mut App) {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("wall: clipboard unavailable: {e}");
            return;
        }
    };
    if !app.is_editing() {
        if let Ok(img) = clipboard.get_image() {
            let (w, h) = (img.width as u32, img.height as u32);
            let mut png = Vec::new();
            let encoded = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())
                .ok_or_else(|| "clipboard image has the wrong size".to_owned())
                .and_then(|rgba| {
                    rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())
                })
                .and_then(|()| app.import_image_bytes(&png, Some("Pasted image"), now_ms()).map_err(|e| e.to_string()));
            if let Err(e) = encoded {
                eprintln!("wall: could not paste image: {e}");
            }
            return;
        }
    }
    if let Ok(text) = clipboard.get_text() {
        app.paste_text(&text);
    }
}

/// Copies the selection to the clipboard; `cut` then deletes it.
fn copy(app: &mut App, cut: bool) {
    let Some(text) = app.copy_selection() else { return };
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
        Ok(()) if cut => app.key(Key::Delete),
        Ok(()) => {}
        Err(e) => eprintln!("wall: could not copy: {e}"),
    }
}

/// Imports a picture or video file, as if it was dropped on the window.
/// Returns the new picture's item (videos arrive later, once converted).
fn import_file(app: &mut App, videos: &mut video::Videos, path: std::path::PathBuf) -> Option<uuid::Uuid> {
    if video::is_video(&path) {
        videos.import(path);
        return None;
    }
    let name = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| app.import_image_bytes(&bytes, name.as_deref(), now_ms()).map_err(|e| e.to_string()));
    match result {
        Ok(id) => Some(id),
        Err(e) => {
            eprintln!("wall: could not import {}: {e}", path.display());
            None
        }
    }
}

/// Pictures imported together: one undo step, laid out in a grid.
struct ImportBatch {
    undo_from: usize,
    ids: Vec<uuid::Uuid>,
    at: Instant,
    /// Where the first picture landed; the grid stays centred there.
    centre: Option<(f64, f64)>,
}

/// Files dropped within this long of each other are one batch (the window
/// gets one event per dropped file).
const BATCH_MS: u128 = 1500;

/// Imports files as one batch with any batch still open: one undo step,
/// the pictures in rows from where the first one landed, all selected.
fn import_files(app: &mut App, videos: &mut video::Videos, batch: &mut Option<ImportBatch>, paths: Vec<std::path::PathBuf>) {
    let open = batch.take().filter(|b| b.at.elapsed().as_millis() < BATCH_MS && app.doc.undo_depth() >= b.undo_from);
    let mut b = open.unwrap_or_else(|| ImportBatch { undo_from: app.doc.undo_depth(), ids: Vec::new(), at: Instant::now(), centre: None });
    for path in paths {
        if let Some(id) = import_file(app, videos, path) {
            b.ids.push(id);
        }
    }
    b.centre = app.arrange_imported(&b.ids, b.undo_from, b.centre);
    b.at = Instant::now();
    *batch = Some(b);
}


/// File > Import: the system's file picker.
#[cfg(any(windows, target_os = "macos"))]
fn pick_files() -> Vec<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import into Wall")
        .add_filter("Pictures and videos", &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "mp4", "mov", "webm", "m4v", "mkv", "avi"])
        .pick_files()
        .unwrap_or_default()
}

/// No file picker on Linux yet: WALL_IMPORT or drag and drop.
#[cfg(not(any(windows, target_os = "macos")))]
fn pick_files() -> Vec<std::path::PathBuf> {
    eprintln!("wall: no file picker on this system; drop files on the window instead");
    Vec::new()
}

/// File > Open library / New library: the system's folder picker. A new
/// library is a folder whose name ends in `.wall`.
#[cfg(any(windows, target_os = "macos"))]
fn pick_library(new: bool) -> Option<std::path::PathBuf> {
    if new {
        let path = rfd::FileDialog::new().set_title("New Wall library").set_file_name("References.wall").save_file()?;
        Some(if path.extension().is_some_and(|e| e == "wall") { path } else { path.with_extension("wall") })
    } else {
        let path = rfd::FileDialog::new().set_title("Open a Wall library (a folder ending in .wall)").pick_folder()?;
        if path.join("wall.json").is_file() {
            Some(path)
        } else {
            show_error(&format!("{} is not a Wall library: it has no wall.json.", path.display()));
            None
        }
    }
}

/// File > Import old Wall library: the old library's folder (with library.json).
#[cfg(any(windows, target_os = "macos"))]
fn pick_old_wall() -> Option<std::path::PathBuf> {
    let path = rfd::FileDialog::new().set_title("Import an old Wall library (the folder with library.json)").pick_folder()?;
    if path.join("library.json").is_file() {
        Some(path)
    } else {
        show_error(&format!("{} has no library.json, so it isn't an old Wall library.", path.display()));
        None
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn pick_old_wall() -> Option<std::path::PathBuf> {
    std::env::var_os("WALL_PICK_OLD").map(Into::into)
}

#[cfg(any(windows, target_os = "macos"))]
fn show_info(text: &str) {
    println!("wall: {text}");
    rfd::MessageDialog::new().set_title("Wall").set_description(text).set_level(rfd::MessageLevel::Info).show();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_info(text: &str) {
    println!("wall: {text}");
}

/// No folder picker on Linux yet: WALL_PICK_LIBRARY names the folder (tests).
#[cfg(not(any(windows, target_os = "macos")))]
fn pick_library(_new: bool) -> Option<std::path::PathBuf> {
    std::env::var_os("WALL_PICK_LIBRARY").map(Into::into)
}

/// File > Export board as picture: where to save the PNG.
#[cfg(any(windows, target_os = "macos"))]
fn pick_export(name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_title("Export board as picture").set_file_name(name).add_filter("PNG picture", &["png"]).save_file()
}

/// No save dialog on Linux yet: WALL_EXPORT names the file (tests).
#[cfg(not(any(windows, target_os = "macos")))]
fn pick_export(_name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("WALL_EXPORT").map(Into::into)
}

/// Longest side of an exported board picture, in pixels.
const EXPORT_MAX: u32 = 8192;

fn export_png(app: &mut App, renderer: &mut Renderer, path: &std::path::Path) {
    let result = app.export_board(renderer, EXPORT_MAX).map_err(|e| e.to_string()).and_then(|(rgba, [w, h])| {
        let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("bad picture size")?;
        img.save(path).map_err(|e| e.to_string())?;
        Ok((w, h))
    });
    match result {
        Ok((w, h)) => println!("wall: exported {} ({w}×{h})", path.display()),
        Err(e) => show_error(&format!("Could not export the board: {e}")),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn show_error(text: &str) {
    eprintln!("wall: {text}");
    rfd::MessageDialog::new().set_title("Wall").set_description(text).set_level(rfd::MessageLevel::Warning).show();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_error(text: &str) {
    eprintln!("wall: {text}");
}

/// Opens (or creates) a library for the desktop and remembers it as the
/// one to open next time.
fn open_library(path: &std::path::Path) -> Result<App, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    println!("wall: library {}", path.display());
    let mut app = App::open_library(path, &device_name(), now_ms())?;
    app.desktop = true;
    settings::remember_library(path);
    Ok(app)
}

fn window_title(library: &std::path::Path) -> String {
    let name = library.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() { "Wall".into() } else { format!("{name} — Wall") }
}

/// Small per-user settings: the last library opened. Kept in
/// `%APPDATA%\Wall\settings.json`, `~/Library/Application Support/Wall`
/// or `~/.config/wall`; `WALL_SETTINGS` overrides the file (tests).
mod settings {
    use std::path::{Path, PathBuf};

    fn file() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("WALL_SETTINGS") {
            return Some(p.into());
        }
        let dir = if cfg!(windows) {
            PathBuf::from(std::env::var_os("APPDATA")?).join("Wall")
        } else if cfg!(target_os = "macos") {
            PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Wall")
        } else {
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?.join("wall")
        };
        Some(dir.join("settings.json"))
    }

    fn read() -> serde_json::Map<String, serde_json::Value> {
        file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b)).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    /// Decode videos on the GPU (`"gpu_video_decode": true`, or
    /// `WALL_GPU_DECODE=1`). Off by default: it frees most of the CPU while
    /// many videos play but ffmpeg then keeps GPU frame pools, about 4x the
    /// memory (16 1080p clips: 7.0 -> 2.6 cores, 0.7 -> 3.3 GB).
    pub fn gpu_video_decode() -> bool {
        match std::env::var("WALL_GPU_DECODE").ok().as_deref() {
            Some("1" | "true" | "on") => true,
            Some("0" | "false" | "off") => false,
            _ => read().get("gpu_video_decode").and_then(serde_json::Value::as_bool).unwrap_or(false),
        }
    }

    /// The library opened last, if it still exists.
    pub fn last_library() -> Option<PathBuf> {
        let p = PathBuf::from(read().get("last_library")?.as_str()?);
        p.join("wall.json").is_file().then_some(p)
    }

    pub fn get_bool(key: &str) -> Option<bool> {
        read().get(key).and_then(|v| v.as_bool())
    }

    pub fn get_str(key: &str) -> Option<String> {
        read().get(key).and_then(|v| v.as_str()).map(str::to_owned)
    }

    pub fn set_str(key: &str, val: &str) {
        save(key, val.into());
    }

    pub fn set_bool(key: &str, val: bool) {
        save(key, val.into());
    }

    pub fn remember_library(path: &Path) {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        // canonicalize gives \\?\C:\… on Windows; keep the plain form.
        let abs = abs.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
        save("last_library", abs.into());
    }

    /// Sets one key, keeping the others.
    fn save(key: &str, val: serde_json::Value) {
        let Some(f) = file() else { return };
        let mut s = read();
        s.insert(key.into(), val);
        let write = || -> std::io::Result<()> {
            if let Some(d) = f.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&f, serde_json::to_vec_pretty(&s).unwrap_or_default())
        };
        if let Err(e) = write() {
            eprintln!("wall: could not save settings {}: {e}", f.display());
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn default_library() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_else(|| ".".into());
    std::path::PathBuf::from(home).join("Wall").join("Library.wall")
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "desktop".into())
}

/// Does what the share card asked, with this library's owner key.
fn cloud_ask(root: Option<&std::path::Path>, ask: wall_app::CloudAsk) -> wall_app::CloudAnswer {
    use wall_app::{CloudAnswer, CloudAsk};
    let account = root
        .and_then(|r| wall_format::Library::open(r).ok())
        .and_then(|lib| Some(wall_cloud::Accounts::load(&wall_cloud::settings_dir()?).find(&lib)?))
        .filter(|a| !a.server.is_empty());
    let Some(account) = account else { return CloudAnswer::NotConnected };
    let client = match wall_cloud::Client::new(account) {
        Ok(c) => c,
        Err(e) => return CloudAnswer::Failed(e.to_string()),
    };
    let r = match ask {
        CloudAsk::Links => client.links().map(|links| {
            let me = client.me().ok();
            CloudAnswer::Links {
                links: links.into_iter().map(|k| wall_app::ShareLink { id: k.id, edit: k.role == "edit", board: k.board, label: k.label }).collect(),
                used: me.as_ref().and_then(|m| m.media_bytes),
                max: me.as_ref().and_then(|m| m.max_media_bytes),
            }
        }),
        CloudAsk::Make { board, edit, label } => client.make_link(board, edit, &label).and_then(|k| {
            let secret = k.secret.ok_or_else(|| wall_cloud::CloudError::Other("the server gave no link".into()))?;
            Ok(CloudAnswer::Made { url: client.link_address(&secret), edit })
        }),
        CloudAsk::Revoke(id) => client.revoke_link(id).map(|_| CloudAnswer::Revoked(id)),
    };
    r.unwrap_or_else(|e| CloudAnswer::Failed(e.to_string()))
}

/// Cloud sync of the open library (see `wall_cloud`).
struct Cloud {
    worker: wall_cloud::Worker,
    stamp: u64,
    root: std::path::PathBuf,
}

/// How often the window looks whether the cloud brought something.
const CLOUD_LOOK: std::time::Duration = std::time::Duration::from_millis(300);

impl Cloud {
    /// Starts syncing if this library is connected.
    fn start(app: &App) -> Option<Self> {
        let root = app.library_root()?.to_owned();
        let dir = wall_cloud::settings_dir()?;
        let lib = wall_format::Library::open(&root).ok()?;
        let account = wall_cloud::Accounts::load(&dir).find(&lib)?;
        match wall_cloud::Worker::start(&root, account, &dir, app.sync_boards()?) {
            Ok(worker) => Some(Cloud { worker, stamp: app.sync_stamp(), root }),
            Err(e) => {
                eprintln!("wall: cloud: {e}");
                None
            }
        }
    }

    fn pump(&mut self, app: &mut App, window_title: &str, window: &Window) {
        use wall_cloud::{Event, Status};
        if app.busy_editing() {
            return; // merged when the drag or typing is over
        }
        while let Some(e) = self.worker.try_event() {
            match e {
                Event::Incoming(x) => {
                    let Some(kept) = app.sync_apply(&x.0, &x.1) else { return };
                    if let Err(e) = app.save_if_dirty() {
                        eprintln!("wall: could not save: {e}");
                    }
                    self.stamp = app.sync_stamp();
                    self.worker.applied(kept, app.sync_boards().unwrap_or_default());
                }
                Event::Media(s) => app.sync_media_arrived(*s),
                Event::Status(s) => {
                    let note = match &s {
                        Status::UpToDate => "",
                        Status::Working => "  —  syncing…",
                        Status::Offline(_) => "  —  cloud: offline, trying again",
                        Status::Full => "  —  cloud: full, new files stay on this computer",
                        Status::Stopped(_) => "  —  cloud: stopped, the key or link doesn't work",
                    };
                    if let Status::Offline(e) | Status::Stopped(e) = &s {
                        eprintln!("wall: cloud: {e}");
                    }
                    window.set_title(&format!("{window_title}{note}"));
                }
            }
        }
        let stamp = app.sync_stamp();
        if stamp != self.stamp {
            if let Some(b) = app.sync_boards() {
                self.stamp = stamp;
                self.worker.boards(b);
            }
        }
    }
}

/// Saves the open board and opens another library in its place.
fn switch_library(app: &mut App, videos: &mut video::Videos, posters: &mut LibraryPosters, window: &Window, path: &std::path::Path) {
    match open_library(path) {
        Ok(new) => {
            if let Err(e) = app.save_if_dirty() {
                eprintln!("wall: could not save: {e}");
            }
            *app = new;
            *videos = video::Videos::default();
            *posters = LibraryPosters::default();
            let size = window.inner_size();
            app.resize([size.width as f32, size.height as f32]);
            window.set_title(&window_title(path));
            window.request_redraw();
        }
        Err(e) => show_error(&format!("Could not open {}: {e}", path.display())),
    }
}

/// Pictures and videos directly inside a folder, by name.
fn media_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    const EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "mp4", "mov", "webm", "m4v", "mkv", "avi"];
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p: &std::path::PathBuf| p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str())));
    files.sort();
    files
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match create_gfx(event_loop, &self.title) {
            Ok(gfx) => {
                let gl = gfx.renderer.gl();
                use glow::HasContext;
                let version = unsafe { gl.get_parameter_string(glow::VERSION) };
                let gpu = unsafe { gl.get_parameter_string(glow::RENDERER) };
                // Desktop GL reports "3.3.0 ...", GLES (ANGLE) "OpenGL ES 3.0.0 ...".
                let api = if version.starts_with("OpenGL") { "" } else { "OpenGL " };
                println!("wall: {api}{version} on {gpu}; {} items", self.app.item_count());
                if video::gpu_decode() {
                    println!("wall: videos decode on the GPU (gpu_video_decode)");
                }
                if self.bench {
                    let _ = gfx.surface.set_swap_interval(&gfx.context, SwapInterval::DontWait);
                }
                gfx.window.request_redraw();
                let size = gfx.window.inner_size();
                self.app.resize([size.width as f32, size.height as f32]);
                self.gfx = Some(gfx);
                // WALL_IMPORT="a.png;b.mp4" imports files at start, like a drop (for tests).
                let paths = std::env::var("WALL_IMPORT").unwrap_or_default().split(';').filter(|p| !p.is_empty()).map(std::path::PathBuf::from).collect();
                import_files(&mut self.app, &mut self.videos, &mut self.batch, paths);
            }
            Err(e) => {
                eprintln!("wall: could not start graphics: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.app.set_time(now_ms());
        let mods = self.mods();
        let Some(gfx) = self.gfx.as_mut() else { return };
        let size = gfx.window.inner_size();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let (Some(w), Some(h)) = (NonZeroU32::new(s.width), NonZeroU32::new(s.height)) {
                    gfx.surface.resize(&gfx.context, w, h);
                }
                self.app.resize([s.width as f32, s.height as f32]);
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::MouseInput { state, button: b, .. } => {
                if let Some(b) = button(b) {
                    let pos = self.cursor;
                    match state {
                        ElementState::Pressed => self.app.pointer_down(b, pos, mods),
                        ElementState::Released => self.app.pointer_up(b, pos),
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = [position.x as f32, position.y as f32];
                self.app.pointer_move(self.cursor);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let w = match delta {
                    MouseScrollDelta::LineDelta(_, y) => Wheel::Lines(y),
                    MouseScrollDelta::PixelDelta(p) => Wheel::Pixels(p.y as f32),
                };
                self.app.wheel(w);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let m = mods;
                let pressed = event.state == ElementState::Pressed;
                let editing = self.app.is_editing();
                let key = match event.logical_key.as_ref() {
                    WKey::Named(NamedKey::Space) if !editing => Some(Key::Space { pressed }),
                    _ if !pressed => None,
                    WKey::Named(NamedKey::Delete) => Some(Key::Delete),
                    WKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
                    WKey::Named(NamedKey::Enter) => Some(Key::Enter),
                    WKey::Named(NamedKey::Escape) => Some(Key::Escape),
                    WKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
                    WKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
                    WKey::Named(NamedKey::Home) => Some(Key::Home),
                    WKey::Named(NamedKey::End) => Some(Key::End),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("z") => Some(if m.shift { Key::Redo } else { Key::Undo }),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("y") => Some(Key::Redo),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("a") => Some(Key::SelectAll),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("d") => Some(Key::Duplicate),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("f") => Some(Key::Find),
                    WKey::Character("]") if m.command => Some(Key::BringToFront),
                    WKey::Character("[") if m.command => Some(Key::SendToBack),
                    WKey::Character(c) if m.command && !editing && (c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("x")) => {
                        copy(&mut self.app, c.eq_ignore_ascii_case("x"));
                        None
                    }
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("v") => {
                        paste(&mut self.app);
                        None
                    }
                    _ => None,
                };
                if let Some(k) = key {
                    self.app.key(k);
                } else if pressed && editing && !m.command {
                    if let Some(text) = event.text.as_deref() {
                        self.app.text_input(text);
                    }
                } else if pressed && !m.command {
                    let mut chars = event.text.as_deref().unwrap_or("").chars();
                    if let (Some(c), None) = (chars.next(), chars.next()) {
                        self.app.key(Key::Char(c));
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                if path.join("wall.json").is_file() {
                    // A library folder: open it.
                    switch_library(&mut self.app, &mut self.videos, &mut self.posters, &gfx.window, &path);
                } else if path.is_dir() {
                    // Any other folder: import the pictures and videos in it.
                    import_files(&mut self.app, &mut self.videos, &mut self.batch, media_in(&path));
                } else {
                    import_files(&mut self.app, &mut self.videos, &mut self.batch, vec![path]);
                }
            }
            WindowEvent::RedrawRequested => {
                for cmd in self.app.take_shell_commands() {
                    match cmd {
                        wall_app::ShellCommand::Copy { cut } => copy(&mut self.app, cut),
                        wall_app::ShellCommand::Paste => paste(&mut self.app),
                        wall_app::ShellCommand::OpenUrl(url) => open_url(&url),
                        wall_app::ShellCommand::CopyText(text) => {
                            if let Err(e) = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                                eprintln!("wall: could not copy: {e}");
                            }
                        }
                        wall_app::ShellCommand::Import => {
                            let paths = pick_files();
                            self.batch = None;
                            import_files(&mut self.app, &mut self.videos, &mut self.batch, paths);
                        }
                        wall_app::ShellCommand::OpenLibrary { new } => {
                            if let Some(path) = pick_library(new) {
                                switch_library(&mut self.app, &mut self.videos, &mut self.posters, &gfx.window, &path);
                            }
                        }
                        wall_app::ShellCommand::ImportOldWall => {
                            let root = self.app.library_root().map(|p| p.to_owned());
                            match (root, &self.old_import) {
                                (_, Some(_)) => show_error("An import is already running."),
                                (None, _) => show_error("Open a library first; the old boards are added to it."),
                                (Some(root), None) => {
                                    if let Some(old) = pick_old_wall() {
                                        println!("wall: importing the old Wall library {}", old.display());
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        std::thread::spawn(move || {
                                            let _ = tx.send(wall_import::old_wall::import_into(&old, &root, Some("Old Wall"), now_ms()));
                                        });
                                        self.old_import = Some(rx);
                                        gfx.window.set_title(&format!("{} (importing old Wall…)", self.title));
                                    }
                                }
                            }
                        }
                        wall_app::ShellCommand::ExportImage => {
                            let name = format!("{}.png", self.app.board_name());
                            if let Some(path) = pick_export(&name) {
                                export_png(&mut self.app, &mut gfx.renderer, &path);
                            }
                        }
                        wall_app::ShellCommand::Cloud(ask) => {
                            let root = self.app.library_root().map(|p| p.to_owned());
                            let (tx, rx) = std::sync::mpsc::channel();
                            std::thread::spawn(move || {
                                let _ = tx.send(cloud_ask(root.as_deref(), ask));
                            });
                            self.cloud_asks.push(rx);
                        }
                    }
                }
                let mut answers = Vec::new();
                self.cloud_asks.retain(|rx| match rx.try_recv() {
                    Ok(a) => {
                        answers.push(a);
                        false
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => true,
                    Err(_) => false,
                });
                for a in answers {
                    self.app.cloud_answer(a);
                }
                if let Some(result) = self.old_import.as_ref().and_then(|rx| rx.try_recv().ok()) {
                    self.old_import = None;
                    gfx.window.set_title(&self.title);
                    match result {
                        Ok(report) => {
                            if let Some(id) = report.container {
                                self.app.add_board_card(id, "Old Wall");
                            }
                            show_info(&format!("The old Wall library is in the \"Old Wall\" board.\n\n{}", report.text()));
                        }
                        Err(e) => show_error(&format!("Could not import the old Wall library: {e}")),
                    }
                }
                self.videos.pump(&mut self.app, &mut gfx.renderer);
                self.posters.pump(&mut self.app);
                self.app.render(&mut gfx.renderer, [size.width, size.height]);
                // Remember a changed theme or grid (View menu, G key).
                let look = (self.app.light_mode(), self.app.grid());
                if look != self.saved_look {
                    if look.0 != self.saved_look.0 {
                        settings::set_bool("light_mode", look.0);
                    }
                    if look.1 != self.saved_look.1 {
                        settings::set_bool("grid", look.1);
                    }
                    self.saved_look = look;
                }
                // Remember where the tool rail is and whether the left panel
                // is open (saved when a drag is over, not on every move).
                let ui = self.app.ui_setting();
                if ui != self.saved_ui && !self.app.busy_dragging() {
                    settings::set_str("ui", &ui);
                    self.saved_ui = ui;
                }
                let settled = self.videos.settled(1500) && !self.app.library_waiting() && !self.app.pictures_loading();
                if let Some(path) = self.screenshot.as_deref().filter(|_| self.frames >= self.screenshot_frame && settled) {
                    // WALL_EXPORT=board.png also exports the board (tests).
                    if let Some(export) = std::env::var_os("WALL_EXPORT") {
                        export_png(&mut self.app, &mut gfx.renderer, std::path::Path::new(&export));
                        self.app.render(&mut gfx.renderer, [size.width, size.height]);
                    }
                    match save_screenshot(&gfx.renderer, size.width, size.height, path) {
                        Ok(()) => println!("wall: saved {path}"),
                        Err(e) => eprintln!("wall: screenshot failed: {e}"),
                    }
                    event_loop.exit();
                }
                if let Err(e) = gfx.surface.swap_buffers(&gfx.context) {
                    eprintln!("wall: swap failed: {e}");
                }
                self.frames += 1;
                let elapsed = self.fps_clock.elapsed().as_secs_f32();
                if elapsed >= 2.0 && self.screenshot.is_none() {
                    println!("wall: {:.0} fps", self.frames as f32 / elapsed);
                    self.frames = 0;
                    self.fps_clock = Instant::now();
                }
                if self.screenshot.is_some() || self.bench {
                    gfx.window.request_redraw();
                }
            }
            _ => {}
        }
        if self.app.needs_redraw || self.videos.busy() || self.app.library_waiting() || self.old_import.is_some() || !self.cloud_asks.is_empty() {
            gfx.window.request_redraw();
        }
        if let Err(e) = self.app.save_if_dirty() {
            eprintln!("wall: could not save: {e}");
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = &self.gfx else { return };
        // Another library was opened: its own sync, or none.
        if self.cloud.as_ref().is_some_and(|c| Some(c.root.as_path()) != self.app.library_root()) || (self.cloud.is_none() && self.cloud_for.as_deref() != self.app.library_root()) {
            self.cloud = Cloud::start(&self.app);
            self.cloud_for = self.app.library_root().map(|p| p.to_owned());
        }
        let Some(cloud) = &mut self.cloud else {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            return;
        };
        self.app.set_time(now_ms());
        cloud.pump(&mut self.app, &self.title, &gfx.window);
        if self.app.needs_redraw {
            gfx.window.request_redraw();
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + CLOUD_LOOK));
    }
}

/// Use ANGLE (EGL + OpenGL ES 3.0 on Direct3D / Metal) when its libraries
/// sit next to the executable, else native desktop OpenGL 3.3.
/// `WALL_GL=angle` or `WALL_GL=gl` forces one.
fn want_angle() -> bool {
    match std::env::var("WALL_GL").ok().as_deref() {
        Some("angle") => true,
        Some("gl") => false,
        _ => {
            let (egl, gles) = if cfg!(windows) {
                ("libEGL.dll", "libGLESv2.dll")
            } else {
                return false;
            };
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|d| d.join(egl).is_file() && d.join(gles).is_file()))
                .unwrap_or(false)
        }
    }
}

fn create_gfx(event_loop: &ActiveEventLoop, title: &str) -> Result<Gfx, Box<dyn std::error::Error>> {
    let attrs = WindowAttributes::default()
        .with_title(title)
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
    let angle = want_angle();
    let api = if angle { glutin::config::Api::GLES3 } else { glutin::config::Api::OPENGL };
    let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_stencil_size(8).with_transparency(false).with_api(api);
    let preference = if angle { glutin_winit::ApiPreference::PreferEgl } else { glutin_winit::ApiPreference::FallbackEgl };
    let (window, config) = DisplayBuilder::new()
        .with_preference(preference)
        .with_window_attributes(Some(attrs))
        .build(event_loop, template, pick_config)?;
    let window = window.ok_or("no window")?;
    let raw = window.window_handle()?.as_raw();
    let display = config.display();
    // ANGLE is only in use if EGL actually loaded; otherwise glutin fell back.
    let gles = angle && is_egl(&display);
    if angle && !gles {
        eprintln!("wall: ANGLE requested but EGL did not load; using desktop OpenGL");
    }

    let ctx_attrs = if gles {
        ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(Some(Version::new(3, 0)))).build(Some(raw))
    } else {
        ContextAttributesBuilder::new()
            .with_profile(GlProfile::Core)
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .build(Some(raw))
    };
    let not_current = unsafe { display.create_context(&config, &ctx_attrs)? };

    let surface_attrs = window.build_surface_attributes(Default::default())?;
    let surface = unsafe { display.create_window_surface(&config, &surface_attrs)? };
    let context = not_current.make_current(&surface)?;
    let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));

    let gl = unsafe {
        glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s) as *const _)
    };
    let target = if gles { GlslTarget::Es300 } else { GlslTarget::Core330 };
    let mut renderer = unsafe { Renderer::new(gl, target)? };
    renderer.set_stencil(config.stencil_size() > 0);
    Ok(Gfx { window, surface, context, renderer })
}

/// glutin has no EGL on macOS, so there ANGLE can't load through it yet and
/// Wall uses Apple's OpenGL 3.3 (which runs on Metal underneath).
#[cfg(not(target_os = "macos"))]
fn is_egl(display: &glutin::display::Display) -> bool {
    #[allow(irrefutable_let_patterns)]
    let egl = matches!(display, glutin::display::Display::Egl(_));
    egl
}

#[cfg(target_os = "macos")]
fn is_egl(_display: &glutin::display::Display) -> bool {
    false
}

fn pick_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        // A stencil buffer first (see-through pen strokes), then up to 4x MSAA.
        .reduce(|best, c| {
            let score = |c: &Config| (c.stencil_size() > 0, if c.num_samples() <= 4 { c.num_samples() } else { 0 });
            if score(&c) > score(&best) { c } else { best }
        })
        .expect("no OpenGL config available")
}

fn save_screenshot(renderer: &Renderer, w: u32, h: u32, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use glow::HasContext;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    unsafe {
        renderer.gl().read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
    }
    // GL rows start at the bottom.
    let row = (w * 4) as usize;
    let flipped: Vec<u8> = pixels.chunks_exact(row).rev().flatten().copied().collect();
    image::save_buffer(path, &flipped, w, h, image::ColorType::Rgba8)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let mut title = "Wall".to_owned();
    let mut app = match std::env::var("WALL_DEMO").ok() {
        Some(n) => App::demo(n.parse().unwrap_or(2000)),
        None => {
            // Older macOS passes `-psn_…` when Finder opens the app; that isn't a path.
            let path = std::env::args_os()
                .skip(1)
                .find(|a| !a.to_string_lossy().starts_with("-psn"))
                .map(std::path::PathBuf::from)
                .or_else(settings::last_library)
                .unwrap_or_else(default_library);
            title = window_title(&path);
            open_library(&path)?
        }
    };
    app.desktop = true;
    // Theme and grid from settings; WALL_THEME=light|dark overrides (tests).
    let light = match std::env::var("WALL_THEME").ok().as_deref() {
        Some("light") => true,
        Some("dark") => false,
        _ => settings::get_bool("light_mode").unwrap_or(false),
    };
    app.set_light_mode(light);
    let grid = settings::get_bool("grid").unwrap_or(true);
    app.set_grid(grid);
    // Tool rail and left panel as left.
    if let Some(ui) = settings::get_str("ui") {
        app.set_ui_setting(&ui);
    }
    // WALL_BOARD=<board id> opens that board of the library at start (tests).
    if let Some(id) = std::env::var("WALL_BOARD").ok().and_then(|s| s.trim().parse::<uuid::Uuid>().ok()) {
        app.open_board(id, true);
    }
    // WALL_OPEN_LIBRARY=1 starts with the library tab open (screenshots).
    if std::env::var_os("WALL_OPEN_LIBRARY").is_some() && !app.library_is_open() {
        app.toggle_sidebar(wall_app::sidebar::Tab::Library);
    }
    // WALL_CAMERA="x,y,zoom" sets the start view (used for screenshots).
    if let Some(v) = std::env::var("WALL_CAMERA").ok().map(|s| s.split(',').filter_map(|p| p.trim().parse::<f32>().ok()).collect::<Vec<_>>()) {
        if let [x, y, z] = v[..] {
            app.set_camera(wall_app::Camera { center: [x, y], zoom: z });
        }
    }
    let mut shell = Shell {
        app,
        gfx: None,
        frames: 0,
        fps_clock: Instant::now(),
        screenshot: std::env::var("WALL_SCREENSHOT").ok(),
        bench: std::env::var_os("WALL_BENCH").is_some(),
        mods: ModifiersState::empty(),
        cursor: [0.0, 0.0],
        videos: video::Videos::default(),
        screenshot_frame: std::env::var("WALL_SCREENSHOT_FRAME").ok().and_then(|v| v.parse().ok()).unwrap_or(3),
        posters: LibraryPosters::default(),
        title,
        old_import: None,
        batch: None,
        cloud: None,
        cloud_for: None,
        cloud_asks: Vec::new(),
        saved_look: (light, grid),
        saved_ui: settings::get_str("ui").unwrap_or_default(),
    };
    event_loop.run_app(&mut shell)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_last_library() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WALL_SETTINGS", dir.path().join("cfg").join("settings.json"));
        assert_eq!(settings::last_library(), None);
        let lib = dir.path().join("Moods.wall");
        let app = open_library(&lib).unwrap();
        assert!(app.desktop);
        let last = settings::last_library().unwrap();
        assert_eq!(std::fs::canonicalize(last).unwrap(), std::fs::canonicalize(&lib).unwrap());
        assert_eq!(window_title(&lib), "Moods — Wall");
        // A library that was moved away isn't offered again.
        std::fs::rename(&lib, dir.path().join("gone.wall")).unwrap();
        assert_eq!(settings::last_library(), None);
    }
}

/// Opens a web address in the default browser. The app only sends
/// http(s) addresses without spaces or quotes, and they go to the opener
/// as one argument (no shell).
fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        show_error(&format!("Could not open {url}: {e}"));
    }
}
