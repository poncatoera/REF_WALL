//! Wall web shell: runs `wall-app` in a `<canvas>` with WebGL2.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wall_app::{App, Button, Key, Mods, Wheel};
use wall_render::{GlslTarget, Renderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, KeyboardEvent, PointerEvent, WebGl2RenderingContext, WheelEvent};

struct State {
    /// Playing `<video>` elements and the textures their frames go to.
    videos: Vec<(web_sys::HtmlVideoElement, wall_app::ContentHash, wall_render::ImageId)>,
    app: App,
    renderer: Renderer,
    canvas: HtmlCanvasElement,
}

impl State {
    /// Keeps the drawing buffer at the canvas's size in device pixels.
    fn sync_size(&mut self) -> [u32; 2] {
        let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
        let w = (self.canvas.client_width() as f64 * dpr).round().max(1.0) as u32;
        let h = (self.canvas.client_height() as f64 * dpr).round().max(1.0) as u32;
        if self.canvas.width() != w || self.canvas.height() != h {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            self.app.resize([w as f32, h as f32]);
        }
        [w, h]
    }
}

fn dpr() -> f32 {
    web_sys::window().map(|w| w.device_pixel_ratio() as f32).unwrap_or(1.0)
}

/// Starts Wall in the canvas with the given element id.
#[wasm_bindgen]
pub fn start(canvas_id: &str) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let document = web_sys::window().ok_or("no window")?.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .get_element_by_id(canvas_id)
        .ok_or("canvas not found")?
        .dyn_into()?;
    let webgl: WebGl2RenderingContext = canvas
        .get_context("webgl2")?
        .ok_or("WebGL2 is not available in this browser")?
        .dyn_into()?;
    let gl = glow::Context::from_webgl2_context(webgl);
    let renderer = unsafe { Renderer::new(gl, GlslTarget::Es300) }.map_err(|e| JsValue::from_str(&e.to_string()))?;
    let app = App::demo(2000);
    web_sys::console::log_1(&format!("wall: WebGL2 ready, {} items", app.item_count()).into());

    let state = Rc::new(RefCell::new(State { videos: Vec::new(), app, renderer, canvas: canvas.clone() }));

    fn pos(e: &PointerEvent) -> [f32; 2] {
        [e.offset_x() as f32 * dpr(), e.offset_y() as f32 * dpr()]
    }
    fn button(b: i16) -> Option<Button> {
        match b {
            0 => Some(Button::Primary),
            1 => Some(Button::Middle),
            2 => Some(Button::Secondary),
            _ => None,
        }
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            let mut s = s.borrow_mut();
            s.app.set_time(js_sys::Date::now() as u64);
            let _ = s.canvas.set_pointer_capture(e.pointer_id());
            let mods = Mods { shift: e.shift_key(), command: e.ctrl_key() || e.meta_key(), alt: e.alt_key() };
            if let Some(b) = button(e.button()) {
                s.app.pointer_move(pos(&e));
                s.app.pointer_down(b, pos(&e), mods);
            }
        });
        canvas.add_event_listener_with_callback("pointerdown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            if let Some(b) = button(e.button()) {
                s.borrow_mut().app.pointer_up(b, pos(&e));
            }
        });
        canvas.add_event_listener_with_callback("pointerup", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| s.borrow_mut().app.pointer_move(pos(&e)));
        canvas.add_event_listener_with_callback("pointermove", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(WheelEvent)>::new(move |e: WheelEvent| {
            e.prevent_default();
            // Browser wheel: positive deltaY scrolls down = zoom out.
            let w = match e.delta_mode() {
                WheelEvent::DOM_DELTA_LINE => Wheel::Lines(-e.delta_y() as f32),
                _ => Wheel::Pixels(-e.delta_y() as f32),
            };
            s.borrow_mut().app.wheel(w);
        });
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        canvas.add_event_listener_with_callback_and_add_event_listener_options("wheel", cb.as_ref().unchecked_ref(), &opts)?;
        cb.forget();
    }
    for kind in ["keydown", "keyup"] {
        let s = state.clone();
        let down = kind == "keydown";
        let cb = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            let cmd = e.ctrl_key() || e.meta_key();
            let k = e.key();
            let mut s = s.borrow_mut();
            s.app.set_time(js_sys::Date::now() as u64);
            let editing = s.app.is_editing();
            let key = match k.as_str() {
                " " if !editing => Some(Key::Space { pressed: down }),
                _ if !down => None,
                "Delete" => Some(Key::Delete),
                "Backspace" => Some(Key::Backspace),
                "Enter" => Some(Key::Enter),
                "Escape" => Some(Key::Escape),
                "ArrowLeft" => Some(Key::Left),
                "ArrowRight" => Some(Key::Right),
                "Home" => Some(Key::Home),
                "End" => Some(Key::End),
                "z" | "Z" if cmd => Some(if e.shift_key() { Key::Redo } else { Key::Undo }),
                "y" | "Y" if cmd => Some(Key::Redo),
                "a" | "A" if cmd => Some(Key::SelectAll),
                _ => None,
            };
            if let Some(key) = key {
                e.prevent_default();
                s.app.key(key);
            } else if down && !cmd && k.chars().count() == 1 {
                // A printable key (the browser gives its character in `key`).
                if editing {
                    e.prevent_default();
                    s.app.text_input(&k);
                } else if let Some(c) = k.chars().next() {
                    s.app.key(Key::Char(c));
                }
            }
        });
        document.add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // Paste: image files are imported like drops; text is typed into the
    // note being edited, or becomes a new note at the cursor.
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |e: web_sys::ClipboardEvent| {
            let Some(data) = e.clipboard_data() else { return };
            e.prevent_default();
            let editing = s.borrow().app.is_editing();
            if let (false, Some(files)) = (editing, data.files()) {
                if files.length() > 0 {
                    for i in 0..files.length() {
                        if let Some(file) = files.get(i) {
                            import_any(s.clone(), file);
                        }
                    }
                    return;
                }
            }
            if let Ok(text) = data.get_data("text/plain") {
                s.borrow_mut().app.paste_text(&text);
            }
        });
        document.add_event_listener_with_callback("paste", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // Drag and drop image files onto the canvas.
    {
        let cb = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| e.prevent_default());
        canvas.add_event_listener_with_callback("dragover", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| {
            e.prevent_default();
            s.borrow_mut().app.pointer_move([e.offset_x() as f32 * dpr(), e.offset_y() as f32 * dpr()]);
            let Some(files) = e.data_transfer().and_then(|d| d.files()) else { return };
            for i in 0..files.length() {
                if let Some(file) = files.get(i) {
                    import_any(s.clone(), file);
                }
            }
        });
        canvas.add_event_listener_with_callback("drop", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // requestAnimationFrame loop; only redraws when something changed.
    let frame: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let frame2 = frame.clone();
    *frame2.borrow_mut() = Some(Closure::new(move || {
        {
            let mut s = state.borrow_mut();
            let size = s.sync_size();
            if !s.videos.is_empty() {
                let State { videos, renderer, app, .. } = &mut *s;
                use glow::HasContext;
                for (hash, cmd) in app.take_video_commands() {
                    let Some((v, _, _)) = videos.iter().find(|(_, h, _)| *h == hash) else { continue };
                    match cmd {
                        wall_app::VideoCommand::Play => drop(v.play()),
                        wall_app::VideoCommand::Pause => v.pause().unwrap_or(()),
                        wall_app::VideoCommand::Seek(t) => v.set_current_time(t),
                        wall_app::VideoCommand::Step(n) => {
                            v.pause().unwrap_or(());
                            // The browser doesn't expose the frame rate; assume 30.
                            v.set_current_time((v.current_time() + n as f64 / 30.0).max(0.0));
                        }
                        wall_app::VideoCommand::Speed(r) => v.set_playback_rate(r as f64),
                    }
                }
                for (v, hash, _) in videos.iter() {
                    let status = wall_app::VideoStatus { time: v.current_time(), duration: v.duration(), playing: !v.paused(), speed: v.playback_rate() as f32 };
                    app.set_video_status(hash.clone(), status);
                }
                for (v, _, tex) in videos.iter() {
                    if v.ready_state() >= 2 {
                        unsafe {
                            let gl = renderer.gl();
                            gl.bind_texture(glow::TEXTURE_2D, Some(renderer.texture(*tex)));
                            gl.tex_image_2d_with_html_video(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, glow::RGBA, glow::UNSIGNED_BYTE, v);
                            gl.bind_texture(glow::TEXTURE_2D, None);
                        }
                    }
                }
                app.needs_redraw = true;
            }
            if s.app.needs_redraw {
                let State { app, renderer, .. } = &mut *s;
                app.render(renderer, size);
            }
        }
        request_frame(frame.borrow().as_ref().unwrap());
    }));
    request_frame(frame2.borrow().as_ref().unwrap());
    Ok(())
}

fn import_any(s: Rc<RefCell<State>>, file: web_sys::File) {
    if file.type_().starts_with("video/") {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = import_video(s, &file).await {
                web_sys::console::warn_1(&format!("wall: could not import {}: {}", file.name(), e.as_string().unwrap_or_else(|| format!("{e:?}"))).into());
            }
        });
    } else {
        import_file(s, file);
    }
}

/// Plays a dropped video in a hidden `<video>` element (the browser
/// decodes), stores its bytes, and copies each frame into a texture.
async fn import_video(s: Rc<RefCell<State>>, file: &web_sys::File) -> Result<(), JsValue> {
    const MAX_BYTES: f64 = 20_000_000.0;
    if file.size() > MAX_BYTES {
        // Converting in the browser (WebCodecs) comes with the cloud library.
        return Err(format!("{:.1} MB is over the 20 MB limit; convert it first", file.size() / 1e6).into());
    }
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let video: web_sys::HtmlVideoElement = document.create_element("video")?.dyn_into()?;
    video.set_muted(true);
    video.set_loop(true);
    video.set_autoplay(true);
    video.set_attribute("playsinline", "")?;
    let loaded = js_sys::Promise::new(&mut |resolve, reject| {
        let _ = video.add_event_listener_with_callback("loadeddata", &resolve);
        let _ = video.add_event_listener_with_callback("error", &reject);
    });
    video.set_src(&web_sys::Url::create_object_url_with_blob(file)?);
    wasm_bindgen_futures::JsFuture::from(loaded).await.map_err(|_| JsValue::from_str("the browser cannot play this video"))?;
    let buf = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    let ext = match file.type_().as_str() {
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "mp4",
    };
    let meta = wall_app::VideoMeta {
        width: video.video_width(),
        height: video.video_height(),
        duration_ms: (video.duration() * 1000.0) as u64,
        fps: None,
    };
    let mut st = s.borrow_mut();
    let st = &mut *st;
    let id = st.app.import_video(&bytes, ext, meta, Some(&file.name()), js_sys::Date::now() as u64).map_err(|e| e.to_string())?;
    let hash = st.app.media_hash(&id).ok_or("video item missing")?;
    let tex = st.renderer.create_video_texture(meta.width, meta.height).map_err(|e| e.to_string())?;
    st.app.attach_texture(hash.clone(), tex);
    let _ = video.play();
    st.videos.push((video, hash, tex));
    Ok(())
}

/// Reads a dropped or pasted file and imports it as a picture.
fn import_file(s: Rc<RefCell<State>>, file: web_sys::File) {
    wasm_bindgen_futures::spawn_local(async move {
        let Ok(buf) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else { return };
        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
        let now = js_sys::Date::now() as u64;
        if let Err(err) = s.borrow_mut().app.import_image_bytes(&bytes, Some(&file.name()), now) {
            web_sys::console::warn_1(&format!("wall: could not import {}: {err}", file.name()).into());
        }
    });
}

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}
