//! Wall desktop shell (macOS, Windows). Opens a window with an OpenGL context,
//! hands input to `wall-app`, and draws with `wall-render`.
//!
//! Spike: uses desktop OpenGL 3.3 core. ANGLE (GLES on Metal / Direct3D) comes next.
//! Test hooks:
//! - `WALL_SCREENSHOT=path.png` renders a few frames, saves the window and exits.
//! - `WALL_CAMERA="x,y,zoom"` sets the start view.
//! - `WALL_BENCH=1` redraws continuously and prints fps (vsync off).
//! - `WALL_DEMO=n` opens a generated demo board with n notes instead of a library.
//!
//! Usage: `wall [path/to/Library.wall]`. Without a path it opens (or creates)
//! `Wall/Library.wall` in the home folder. Boards save automatically.

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
                println!("wall: OpenGL {version} on {gpu}; {} items", self.app.item_count());
                if self.bench {
                    let _ = gfx.surface.set_swap_interval(&gfx.context, SwapInterval::DontWait);
                }
                gfx.window.request_redraw();
                self.gfx = Some(gfx);
            }
            Err(e) => {
                eprintln!("wall: could not start graphics: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
                let key = match event.logical_key.as_ref() {
                    WKey::Named(NamedKey::Space) => Some(Key::Space { pressed }),
                    _ if !pressed => None,
                    WKey::Named(NamedKey::Delete | NamedKey::Backspace) => Some(Key::Delete),
                    WKey::Named(NamedKey::Escape) => Some(Key::Escape),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("z") => Some(if m.shift { Key::Redo } else { Key::Undo }),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("y") => Some(Key::Redo),
                    WKey::Character(c) if m.command && c.eq_ignore_ascii_case("a") => Some(Key::SelectAll),
                    _ => None,
                };
                if let Some(k) = key {
                    self.app.key(k);
                }
            }
            WindowEvent::DroppedFile(path) => {
                let name = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
                let result = std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| self.app.import_image_bytes(&bytes, name.as_deref(), now_ms()).map_err(|e| e.to_string()));
                if let Err(e) = result {
                    eprintln!("wall: could not import {}: {e}", path.display());
                }
            }
            WindowEvent::RedrawRequested => {
                self.app.render(&mut gfx.renderer, [size.width, size.height]);
                if let Some(path) = self.screenshot.as_deref().filter(|_| self.frames == 3) {
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
                if elapsed >= 2.0 {
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
        if self.app.needs_redraw {
            gfx.window.request_redraw();
        }
        if let Err(e) = self.app.save_if_dirty() {
            eprintln!("wall: could not save: {e}");
        }
    }
}

fn create_gfx(event_loop: &ActiveEventLoop) -> Result<Gfx, Box<dyn std::error::Error>> {
    let attrs = WindowAttributes::default()
        .with_title("Wall")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
    let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_transparency(false);
    let (window, config) = DisplayBuilder::new()
        .with_window_attributes(Some(attrs))
        .build(event_loop, template, pick_config)?;
    let window = window.ok_or("no window")?;
    let raw = window.window_handle()?.as_raw();
    let display = config.display();

    let ctx_attrs = ContextAttributesBuilder::new()
        .with_profile(GlProfile::Core)
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(Some(raw));
    let not_current = unsafe { display.create_context(&config, &ctx_attrs)? };

    let surface_attrs = window.build_surface_attributes(Default::default())?;
    let surface = unsafe { display.create_window_surface(&config, &surface_attrs)? };
    let context = not_current.make_current(&surface)?;
    let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));

    let gl = unsafe {
        glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s) as *const _)
    };
    let renderer = unsafe { Renderer::new(gl, GlslTarget::Core330)? };
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
    };
    event_loop.run_app(&mut shell)?;
    Ok(())
}
