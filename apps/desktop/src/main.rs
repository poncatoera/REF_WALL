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
//! - `WALL_DEMO=n` opens a generated demo board with n notes instead of a library.
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
fn import_file(app: &mut App, videos: &mut video::Videos, path: std::path::PathBuf) {
    if video::is_video(&path) {
        videos.import(path);
        return;
    }
    let name = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| app.import_image_bytes(&bytes, name.as_deref(), now_ms()).map_err(|e| e.to_string()));
    if let Err(e) = result {
        eprintln!("wall: could not import {}: {e}", path.display());
    }
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

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match create_gfx(event_loop) {
            Ok(gfx) => {
                let gl = gfx.renderer.gl();
                use glow::HasContext;
                let version = unsafe { gl.get_parameter_string(glow::VERSION) };
                let gpu = unsafe { gl.get_parameter_string(glow::RENDERER) };
                // Desktop GL reports "3.3.0 ...", GLES (ANGLE) "OpenGL ES 3.0.0 ...".
                let api = if version.starts_with("OpenGL") { "" } else { "OpenGL " };
                println!("wall: {api}{version} on {gpu}; {} items", self.app.item_count());
                if self.bench {
                    let _ = gfx.surface.set_swap_interval(&gfx.context, SwapInterval::DontWait);
                }
                gfx.window.request_redraw();
                let size = gfx.window.inner_size();
                self.app.resize([size.width as f32, size.height as f32]);
                self.gfx = Some(gfx);
                // WALL_IMPORT="a.png;b.mp4" imports files at start, like a drop (for tests).
                for path in std::env::var("WALL_IMPORT").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
                    let path = std::path::PathBuf::from(path);
                    if video::is_video(&path) {
                        self.videos.import(path);
                    } else if let Err(e) = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| self.app.import_image_bytes(&b, None, now_ms()).map_err(|e| e.to_string())) {
                        eprintln!("wall: could not import {}: {e}", path.display());
                    }
                }
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
            WindowEvent::DroppedFile(path) => import_file(&mut self.app, &mut self.videos, path),
            WindowEvent::RedrawRequested => {
                for cmd in self.app.take_shell_commands() {
                    match cmd {
                        wall_app::ShellCommand::Copy { cut } => copy(&mut self.app, cut),
                        wall_app::ShellCommand::Paste => paste(&mut self.app),
                        wall_app::ShellCommand::Import => {
                            for path in pick_files() {
                                import_file(&mut self.app, &mut self.videos, path);
                            }
                        }
                    }
                }
                self.videos.pump(&mut self.app, &mut gfx.renderer);
                self.posters.pump(&mut self.app);
                self.app.render(&mut gfx.renderer, [size.width, size.height]);
                let settled = self.videos.settled(1500) && !self.app.library_waiting();
                if let Some(path) = self.screenshot.as_deref().filter(|_| self.frames >= self.screenshot_frame && settled) {
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
        if self.app.needs_redraw || self.videos.busy() || self.app.library_waiting() {
            gfx.window.request_redraw();
        }
        if let Err(e) = self.app.save_if_dirty() {
            eprintln!("wall: could not save: {e}");
        }
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
            } else if cfg!(target_os = "macos") {
                ("libEGL.dylib", "libGLESv2.dylib")
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

fn create_gfx(event_loop: &ActiveEventLoop) -> Result<Gfx, Box<dyn std::error::Error>> {
    let attrs = WindowAttributes::default()
        .with_title("Wall")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
    let angle = want_angle();
    let api = if angle { glutin::config::Api::GLES3 } else { glutin::config::Api::OPENGL };
    let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_transparency(false).with_api(api);
    let preference = if angle { glutin_winit::ApiPreference::PreferEgl } else { glutin_winit::ApiPreference::FallbackEgl };
    let (window, config) = DisplayBuilder::new()
        .with_preference(preference)
        .with_window_attributes(Some(attrs))
        .build(event_loop, template, pick_config)?;
    let window = window.ok_or("no window")?;
    let raw = window.window_handle()?.as_raw();
    let display = config.display();
    // ANGLE is only in use if EGL actually loaded; otherwise glutin fell back.
    #[allow(irrefutable_let_patterns)]
    let gles = angle && matches!(display, glutin::display::Display::Egl(_));
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
    let renderer = unsafe { Renderer::new(gl, target)? };
    Ok(Gfx { window, surface, context, renderer })
}

fn pick_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|best, c| if c.num_samples() > best.num_samples() && c.num_samples() <= 4 { c } else { best })
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
    let mut app = match std::env::var("WALL_DEMO").ok() {
        Some(n) => App::demo(n.parse().unwrap_or(2000)),
        None => {
            let path = std::env::args_os().nth(1).map(std::path::PathBuf::from).unwrap_or_else(default_library);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            println!("wall: library {}", path.display());
            App::open_library(&path, &device_name(), now_ms())?
        }
    };
    // WALL_OPEN_LIBRARY=1 starts with the library drawer open (screenshots).
    if std::env::var_os("WALL_OPEN_LIBRARY").is_some() {
        app.toggle_library();
    }
    // WALL_CAMERA="x,y,zoom" sets the start view (used for screenshots).
    if let Some(v) = std::env::var("WALL_CAMERA").ok().map(|s| s.split(',').filter_map(|p| p.trim().parse::<f32>().ok()).collect::<Vec<_>>()) {
        if let [x, y, z] = v[..] {
            app.camera = wall_app::Camera { center: [x, y], zoom: z };
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
    };
    event_loop.run_app(&mut shell)?;
    Ok(())
}
