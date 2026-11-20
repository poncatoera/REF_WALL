//! Wall desktop shell (macOS, Windows). Opens a window with an OpenGL context,
//! hands input to `wall-app`, and draws with `wall-render`.
//!
//! Spike: uses desktop OpenGL 3.3 core. ANGLE (GLES on Metal / Direct3D) comes next.
//! `WALL_SCREENSHOT=path.png` renders a few frames, saves the window and exits;
//! used for automated checks.

use std::num::NonZeroU32;
use std::time::Instant;

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext, PossiblyCurrentContext, Version};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use wall_app::{App, Wheel};
use wall_render::{GlslTarget, Renderer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
                println!("wall: OpenGL {version} on {gpu}; {} boxes, {} glyphs", self.app.box_count(), self.app.glyph_count());
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
        let Some(gfx) = self.gfx.as_mut() else { return };
        let size = gfx.window.inner_size();
        let fsize = [size.width as f32, size.height as f32];
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let (Some(w), Some(h)) = (NonZeroU32::new(s.width), NonZeroU32::new(s.height)) {
                    gfx.surface.resize(&gfx.context, w, h);
                }
                gfx.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left | MouseButton::Middle, .. } => {
                match state {
                    ElementState::Pressed => self.app.pointer_down(),
                    ElementState::Released => self.app.pointer_up(),
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.app.pointer_move([position.x as f32, position.y as f32]);
                gfx.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let w = match delta {
                    MouseScrollDelta::LineDelta(_, y) => Wheel::Lines(y),
                    MouseScrollDelta::PixelDelta(p) => Wheel::Pixels(p.y as f32),
                };
                self.app.wheel(w, fsize);
                gfx.window.request_redraw();
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
                if self.screenshot.is_some() {
                    gfx.window.request_redraw();
                }
            }
            _ => {}
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
    let mut app = App::demo(60, 40);
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
    };
    event_loop.run_app(&mut shell)?;
    Ok(())
}
