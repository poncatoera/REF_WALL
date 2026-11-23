//! Wall web shell: runs `wall-app` in a `<canvas>` with WebGL2.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wall_app::{App, Button, Key, Mods, Wheel};
use wall_render::{GlslTarget, Renderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, KeyboardEvent, PointerEvent, WebGl2RenderingContext, WheelEvent};

mod cloud;
mod shrink;

struct State {
    /// Playing `<video>` elements and the textures their frames go to.
    videos: Vec<(web_sys::HtmlVideoElement, wall_app::ContentHash, wall_render::ImageId)>,
    /// Videos the person paused (others pause only while off screen).
    user_paused: std::collections::HashSet<wall_app::ContentHash>,
    /// Back-and-forth videos on their way back: paused, stepped backwards
    /// by hand each frame (browsers don't play backwards).
    reversing: std::collections::HashSet<wall_app::ContentHash>,
    app: App,
    renderer: Renderer,
    canvas: HtmlCanvasElement,
    /// Light mode and grid as last saved to localStorage.
    saved_look: (bool, bool),
    saved_ui: String,
    /// Boards are saved in the browser (off with `?demo`).
    store: bool,
    /// Saved boards have been looked for; saving may start.
    loaded: bool,
    /// `App::edit_stamp` of what was last saved, and when it was checked.
    saved_stamp: u64,
    checked_ms: f64,
    saving: bool,
}

#[wasm_bindgen(module = "/js/store.js")]
extern "C" {
    #[wasm_bindgen(js_name = storeGet, catch)]
    fn store_get(key: &str) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = storePut, catch)]
    fn store_put(key: &str, value: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = storeExport, catch)]
    fn store_export() -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = storeImport, catch)]
    fn store_import(text: &str) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = isBackup)]
    fn is_backup(text: &str) -> bool;
}

#[wasm_bindgen(module = "/js/drop.js")]
extern "C" {
    #[wasm_bindgen(js_name = notice)]
    fn notice(text: &str);
    #[wasm_bindgen(js_name = toPng, catch)]
    fn to_png(file: &web_sys::File) -> Result<js_sys::Promise, JsValue>;
}

/// Says why a file couldn't be added, where the person can see it.
fn import_failed(name: &str, why: &str) {
    web_sys::console::warn_1(&format!("wall: could not import {name}: {why}").into());
    notice(&format!("Couldn't add {name}: {why}"));
}

/// Adds a picture file: as it is, or through the browser if it's a kind
/// only the browser reads.
async fn add_picture(s: &Rc<RefCell<State>>, file: &web_sys::File) -> Option<uuid::Uuid> {
    let name = file.name();
    let buf = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
        Ok(b) => b,
        Err(_) => {
            // What a dropped folder looks like here.
            import_failed(&name, "a folder can't be dropped in the browser; drop the files in it");
            return None;
        }
    };
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    if bytes.is_empty() {
        import_failed(&name, "it's empty, or a folder (drop the files in it)");
        return None;
    }
    let now = js_sys::Date::now() as u64;
    let first = s.borrow_mut().app.import_image_bytes(&bytes, Some(&name), now);
    if let Ok(id) = first {
        return Some(id);
    }
    let png = match to_png(file) {
        Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok().filter(|v| v.is_object()).map(|v| js_sys::Uint8Array::new(&v).to_vec()),
        Err(_) => None,
    };
    let Some(png) = png else {
        let ext = name.rsplit('.').next().unwrap_or_default().to_lowercase();
        import_failed(&name, match ext.as_str() {
            "heic" | "heif" => "HEIC photos aren't supported; save it as JPG first",
            _ => "Wall can't read this kind of file",
        });
        return None;
    };
    let second = s.borrow_mut().app.import_image_bytes(&png, Some(&name), now);
    match second {
        Ok(id) => Some(id),
        Err(e) => {
            import_failed(&name, &e.to_string());
            None
        }
    }
}

async fn get(key: &str) -> Result<JsValue, JsValue> {
    wasm_bindgen_futures::JsFuture::from(store_get(key)?).await
}

async fn put(key: &str, value: &JsValue) -> Result<(), JsValue> {
    wasm_bindgen_futures::JsFuture::from(store_put(key, value)?).await.map(|_| ())
}

fn err_text(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// Opens the boards saved in this browser, if any. True if there were some.
async fn load_saved(s: Rc<RefCell<State>>) -> Result<bool, JsValue> {
    let Some(json) = get("boards").await?.as_string() else { return Ok(false) };
    let mut media = std::collections::HashMap::new();
    for (hash, _, _) in App::snapshot_media(&json) {
        let v = get(&format!("media/{hash}")).await?;
        if v.is_object() {
            media.insert(hash, js_sys::Uint8Array::new(&v).to_vec());
        }
    }
    let videos = s.borrow_mut().app.restore(&json, &media).map_err(|e| JsValue::from_str(&e.to_string()))?;
    for (hash, ext, w, h) in videos {
        let Some(bytes) = media.get(&hash) else { continue };
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(&bytes[..]));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type(video_mime(&ext));
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)?;
        if let Err(e) = attach_player(s.clone(), &blob, hash, w, h).await {
            web_sys::console::warn_1(&format!("wall: saved video: {}", err_text(&e)).into());
        }
    }
    let missing = s.borrow().app.missing_media().len();
    if missing > 0 {
        web_sys::console::warn_1(&format!("wall: {missing} saved pictures or videos are missing").into());
    }
    Ok(true)
}

/// Saves the boards and any new media in the browser.
async fn save(s: Rc<RefCell<State>>) {
    let (json, media) = {
        let mut st = s.borrow_mut();
        st.saving = true;
        (st.app.snapshot(), st.app.take_new_media())
    };
    let result = async {
        for m in &media {
            put(&format!("media/{}", m.hash), &js_sys::Uint8Array::from(&m.bytes[..])).await?;
        }
        put("boards", &JsValue::from_str(&json)).await
    }
    .await;
    if let Err(e) = result {
        web_sys::console::warn_1(&format!("wall: could not save in the browser: {}", err_text(&e)).into());
    }
    s.borrow_mut().saving = false;
}

/// Saved boards first; the demo's clips only when there were none.
async fn startup(s: Rc<RefCell<State>>, demo_videos: Vec<String>) {
    let store = s.borrow().store;
    let saved = if store {
        load_saved(s.clone()).await.unwrap_or_else(|e| {
            web_sys::console::warn_1(&format!("wall: could not open saved boards: {}", err_text(&e)).into());
            false
        })
    } else {
        false
    };
    if !saved && !demo_videos.is_empty() {
        load_demo_videos(s.clone(), demo_videos).await;
    }
    let mut st = s.borrow_mut();
    // The untouched demo isn't saved; the first change saves everything.
    st.saved_stamp = st.app.edit_stamp();
    st.loaded = true;
    if saved {
        web_sys::console::log_1(&"wall: opened the boards saved in this browser".into());
    }
}

fn video_mime(ext: &str) -> &'static str {
    match ext {
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "video/mp4",
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Remembers the theme and grid, and tells the page (its background).
fn save_look(light: bool, grid: bool) {
    if let Some(st) = storage() {
        let _ = st.set_item("wall.light", if light { "1" } else { "0" });
        let _ = st.set_item("wall.grid", if grid { "1" } else { "0" });
    }
    if let Some(root) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", if light { "light" } else { "dark" });
    }
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
/// Starts Wall in the canvas. `demo_videos` are URLs of clips to add to
/// the demo board (the online preview passes a few; local builds none).
#[wasm_bindgen]
pub fn start_with_videos(canvas_id: &str, demo_videos: Vec<String>) -> Result<(), JsValue> {
    let state = start_state(canvas_id)?;
    match cloud::link() {
        Some(key) => wasm_bindgen_futures::spawn_local(cloud::run(state, key)),
        None => wasm_bindgen_futures::spawn_local(startup(state, demo_videos)),
    }
    Ok(())
}

/// Fetches clips and lays them out in a "Motion clips" group beside the
/// pictures.
async fn load_demo_videos(s: Rc<RefCell<State>>, urls: Vec<String>) {
    let n = urls.len();
    let (x0, y0, w, h, gap) = (2100.0, 20.0, 480.0, 270.0, 40.0);
    s.borrow_mut().app.add_group("Motion clips", [x0 - 40.0, y0 - 60.0, w + 80.0, n as f64 * (h + gap) + 60.0]);
    for (k, url) in urls.into_iter().enumerate() {
        let name = url.rsplit('/').next().unwrap_or("clip.mp4").to_owned();
        let fetched = async {
            let window = web_sys::window().ok_or("no window")?;
            let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url)).await?.dyn_into()?;
            if !resp.ok() {
                return Err(JsValue::from_str(&format!("HTTP {}", resp.status())));
            }
            let blob: web_sys::Blob = wasm_bindgen_futures::JsFuture::from(resp.blob()?).await?.dyn_into()?;
            let id = import_video_blob(s.clone(), &blob, &name, "video/mp4").await?;
            s.borrow_mut().app.set_item_frame(id, [x0, y0 + k as f64 * (h + gap), w, h]);
            Ok::<(), JsValue>(())
        };
        if let Err(e) = fetched.await {
            web_sys::console::warn_1(&format!("wall: demo video {url}: {e:?}").into());
        }
    }    // Show the pictures and the clips together.
    let mut st = s.borrow_mut();
    st.app.camera = wall_app::Camera { center: [1300.0, 470.0], zoom: 0.46 };
    st.app.needs_redraw = true;
}

/// Carries out a menu command that needs the browser.
fn shell_command(s: Rc<RefCell<State>>, cmd: wall_app::ShellCommand) {
    let Some(window) = web_sys::window() else { return };
    match cmd {
        wall_app::ShellCommand::Import => {
            let Some(document) = window.document() else { return };
            let Ok(input) = document.create_element("input").and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().map_err(JsValue::from)) else { return };
            input.set_type("file");
            input.set_multiple(true);
            input.set_accept("image/*,video/*");
            let picked = input.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let Some(files) = picked.files() else { return };
                import_many(s.clone(), (0..files.length()).filter_map(|i| files.get(i)).collect());
            });
            input.set_onchange(Some(cb.as_ref().unchecked_ref()));
            cb.forget();
            input.click();
        }
        wall_app::ShellCommand::Copy { cut } => {
            let text = s.borrow().app.copy_selection();
            if let Some(text) = text {
                let _ = window.navigator().clipboard().write_text(&text);
                if cut {
                    s.borrow_mut().app.key(Key::Delete);
                }
            }
        }
        wall_app::ShellCommand::Paste => {
            let read = window.navigator().clipboard().read_text();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(t) = wasm_bindgen_futures::JsFuture::from(read).await {
                    if let Some(t) = t.as_string() {
                        s.borrow_mut().app.paste_text(&t);
                    }
                }
            });
        }
        // Libraries are folders on disk; the web menus don't offer them.
        wall_app::ShellCommand::OpenLibrary { .. } | wall_app::ShellCommand::ImportOldWall | wall_app::ShellCommand::Cloud(_) => {}
        wall_app::ShellCommand::SetName(name) => cloud::set_name(&name),
        wall_app::ShellCommand::SetColor(c) => cloud::set_color(c),
        wall_app::ShellCommand::CopyText(text) => {
            let _ = window.navigator().clipboard().write_text(&text);
        }
        wall_app::ShellCommand::OpenUrl(url) => {
            if let Some(w) = web_sys::window() {
                let _ = w.open_with_url_and_target_and_features(&url, "_blank", "noopener");
            }
        }
        wall_app::ShellCommand::ExportImage => {
            if let Err(e) = export_png(&s) {
                web_sys::console::error_1(&e);
            }
        }
    }
}

/// File > Export board as picture: draws the board off-screen (longest
/// side up to 4096 px, to stay inside browser memory) and downloads a PNG.
fn export_png(s: &Rc<RefCell<State>>) -> Result<(), JsValue> {
    let (png, name) = {
        let mut st = s.borrow_mut();
        let State { app, renderer, .. } = &mut *st;
        let (rgba, [w, h]) = app.export_board(renderer, 4096).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("bad picture size")?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| JsValue::from_str(&e.to_string()))?;
        (png, format!("{}.png", app.board_name()))
    };
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(&png[..]));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("image/png");
    download(&web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)?, &name)
}

/// Hands a blob to the browser as a download.
fn download(blob: &web_sys::Blob, name: &str) -> Result<(), JsValue> {
    let url = web_sys::Url::create_object_url_with_blob(blob)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    // The URL is not revoked right away: some browsers start the download
    // after this returns. One file per export is a small, bounded cost.
    Ok(())
}

/// Ctrl+S: saves now, then downloads everything saved in this browser as
/// one file. Dropping that file on the board brings it all back.
async fn backup(s: Rc<RefCell<State>>) {
    let result = async {
        if s.borrow().store {
            save(s.clone()).await;
        }
        let text = wasm_bindgen_futures::JsFuture::from(store_export()?).await?.as_string().ok_or("empty backup")?;
        let parts = js_sys::Array::of1(&JsValue::from_str(&text));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type("application/json");
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)?;
        let day: String = String::from(js_sys::Date::new_0().to_iso_string()).chars().take(10).collect();
        download(&blob, &format!("Wall backup {day}.json"))
    }
    .await;
    if let Err(e) = result {
        web_sys::console::warn_1(&format!("wall: backup failed: {}", err_text(&e)).into());
    }
}

/// A dropped backup file: after asking, replaces what's saved and reloads.
async fn restore_backup(file: web_sys::File) -> Result<(), JsValue> {
    let text = wasm_bindgen_futures::JsFuture::from(file.text()).await?.as_string().unwrap_or_default();
    if !is_backup(&text) {
        return Err(JsValue::from_str("not a Wall backup"));
    }
    let window = web_sys::window().ok_or("no window")?;
    let ok = window.confirm_with_message(&format!("Replace the boards saved in this browser with \"{}\"?", file.name()))?;
    if ok {
        wasm_bindgen_futures::JsFuture::from(store_import(&text)?).await?;
        window.location().reload()?;
    }
    Ok(())
}

#[wasm_bindgen]
pub fn start(canvas_id: &str) -> Result<(), JsValue> {
    start_with_videos(canvas_id, Vec::new())
}

fn start_state(canvas_id: &str) -> Result<Rc<RefCell<State>>, JsValue> {
    console_error_panic_hook::set_once();
    let document = web_sys::window().ok_or("no window")?.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .get_element_by_id(canvas_id)
        .ok_or("canvas not found")?
        .dyn_into()?;
    // A stencil buffer lets see-through pen strokes blend once.
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"stencil".into(), &true.into());
    let webgl: WebGl2RenderingContext = canvas
        .get_context_with_context_options("webgl2", &options)?
        .ok_or("WebGL2 is not available in this browser")?
        .dyn_into()?;
    let gl = glow::Context::from_webgl2_context(webgl);
    let renderer = unsafe { Renderer::new(gl, GlslTarget::Es300) }.map_err(|e| JsValue::from_str(&e.to_string()))?;
    let share = cloud::link();
    let mut app = match &share {
        Some(_) => cloud::empty_app(),
        None => App::demo(2000),
    };
    // Theme and grid as the person left them.
    let stored = |k: &str| storage().and_then(|s| s.get_item(k).ok().flatten());
    let light = stored("wall.light").as_deref() == Some("1");
    let grid = stored("wall.grid").as_deref() != Some("0");
    app.set_light_mode(light);
    app.set_grid(grid);
    let saved_ui = stored("wall.ui").unwrap_or_default();
    app.set_ui_setting(&saved_ui);
    web_sys::console::log_1(&format!("wall: WebGL2 ready, {} items", app.item_count()).into());

    let state = Rc::new(RefCell::new(State { videos: Vec::new(), user_paused: Default::default(), reversing: Default::default(), app, renderer, canvas: canvas.clone(),
        saved_look: (light, grid),
        saved_ui,
        // `?demo` shows the demo as it ships, without saving.
        // A shared library stays in the cloud, not among this browser's boards.
        store: share.is_none() && !web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default().contains("demo"),
        loaded: false,
        saved_stamp: 0,
        checked_ms: 0.0,
        saving: false,
    }));

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
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |_: PointerEvent| s.borrow_mut().app.pointer_left());
        canvas.add_event_listener_with_callback("pointerleave", cb.as_ref().unchecked_ref())?;
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
            if down && cmd && (k == "s" || k == "S") {
                // Instead of the browser's "save page": a backup file.
                e.prevent_default();
                wasm_bindgen_futures::spawn_local(backup(s.clone()));
                return;
            }
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
                "d" | "D" if cmd => Some(Key::Duplicate),
                "f" | "F" if cmd => Some(Key::Find),
                "]" if cmd => Some(Key::BringToFront),
                "[" if cmd => Some(Key::SendToBack),
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

    // Copy and cut: the selection goes to the clipboard as Wall items.
    for kind in ["copy", "cut"] {
        let s = state.clone();
        let cut = kind == "cut";
        let cb = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |e: web_sys::ClipboardEvent| {
            let mut s = s.borrow_mut();
            if s.app.is_editing() {
                return;
            }
            let (Some(text), Some(data)) = (s.app.copy_selection(), e.clipboard_data()) else { return };
            if data.set_data("text/plain", &text).is_ok() {
                e.prevent_default();
                if cut {
                    s.app.key(Key::Delete);
                }
            }
        });
        document.add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref())?;
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
            import_many(s.clone(), (0..files.length()).filter_map(|i| files.get(i)).collect());
        });
        canvas.add_event_listener_with_callback("drop", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // requestAnimationFrame loop; only redraws when something changed.
    let frame: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let frame2 = frame.clone();
    let ret = state.clone();
    *frame2.borrow_mut() = Some(Closure::new(move || {
        let cmds = state.borrow_mut().app.take_shell_commands();
        for c in cmds {
            shell_command(state.clone(), c);
        }
        // Autosave: at most once a second, after a change, not mid-drag.
        let due = {
            let mut s = state.borrow_mut();
            let now = js_sys::Date::now();
            if s.store && s.loaded && !s.saving && now - s.checked_ms > 1000.0 && !s.app.busy_editing() {
                s.checked_ms = now;
                let stamp = s.app.edit_stamp();
                let changed = stamp != s.saved_stamp;
                s.saved_stamp = stamp;
                changed
            } else {
                false
            }
        };
        if due {
            wasm_bindgen_futures::spawn_local(save(state.clone()));
        }
        {
            let mut s = state.borrow_mut();
            let size = s.sync_size();
            if !s.videos.is_empty() {
                let State { videos, renderer, app, user_paused, reversing, .. } = &mut *s;
                use glow::HasContext;
                for (hash, cmd) in app.take_video_commands() {
                    let Some((v, _, _)) = videos.iter().find(|(_, h, _)| *h == hash) else { continue };
                    match cmd {
                        wall_app::VideoCommand::Play => {
                            user_paused.remove(&hash);
                            drop(v.play());
                        }
                        wall_app::VideoCommand::Pause => {
                            user_paused.insert(hash.clone());
                            v.pause().unwrap_or(());
                        }
                        wall_app::VideoCommand::Seek(t) => v.set_current_time(t),
                        wall_app::VideoCommand::Step(n) => {
                            user_paused.insert(hash.clone());
                            v.pause().unwrap_or(());
                            // The browser doesn't expose the frame rate; assume 30.
                            v.set_current_time((v.current_time() + n as f64 / 30.0).max(0.0));
                        }
                        wall_app::VideoCommand::Speed(r) => v.set_playback_rate(r as f64),
                        wall_app::VideoCommand::Range(Some((a, _))) => v.set_current_time(a),
                        wall_app::VideoCommand::Range(None) => {}
                        wall_app::VideoCommand::PingPong(_) => {
                            reversing.remove(&hash);
                        }
                        // Not in the browser yet (it needs the frames around this one).
                        wall_app::VideoCommand::Onion(_) => {}
                    }
                }
                // Only videos on screen (and big enough) play; the rest keep their last frame.
                let playable = app.playable_videos();
                for (v, hash, _) in videos.iter() {
                    let (a, b) = app.video_range(hash).unwrap_or((0.0, v.duration()));
                    let t = v.current_time();
                    if reversing.contains(hash) {
                        if !user_paused.contains(hash) && playable.contains(hash) {
                            let back = t - v.playback_rate() / 60.0;
                            if back <= a {
                                reversing.remove(hash);
                                v.set_current_time(a);
                            } else {
                                v.set_current_time(back);
                            }
                        }
                    } else if app.video_pingpong(hash) && b.is_finite() && t >= b - 1.0 / 30.0 {
                        reversing.insert(hash.clone());
                        v.pause().unwrap_or(());
                    } else if app.video_range(hash).is_some() && (t >= b || t < a - 0.25) {
                        // Loop between in and out points.
                        v.set_current_time(a);
                    }
                    let want = playable.contains(hash) && !user_paused.contains(hash) && !reversing.contains(hash);
                    if want && v.paused() {
                        drop(v.play());
                    } else if !want && !v.paused() {
                        v.pause().unwrap_or(());
                    }
                    let status = wall_app::VideoStatus { time: v.current_time(), duration: v.duration(), playing: !user_paused.contains(hash), speed: v.playback_rate() as f32 };
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
            cloud::tick(&mut s.app);
            if s.app.needs_redraw {
                let State { app, renderer, .. } = &mut *s;
                app.render(renderer, size);
            }
            let look = (s.app.light_mode(), s.app.grid());
            if look != s.saved_look {
                s.saved_look = look;
                save_look(look.0, look.1);
            }
            let ui = s.app.ui_setting();
            if ui != s.saved_ui && !s.app.busy_dragging() {
                if let Some(st) = storage() {
                    let _ = st.set_item("wall.ui", &ui);
                }
                s.saved_ui = ui;
            }
        }
        request_frame(frame.borrow().as_ref().unwrap());
    }));
    request_frame(frame2.borrow().as_ref().unwrap());
    Ok(ret)
}

/// Files dropped or picked together. Pictures become one undo step, laid
/// out in rows where they landed; videos and a backup file go their own way.
fn import_many(s: Rc<RefCell<State>>, files: Vec<web_sys::File>) {
    let (pictures, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| !f.type_().starts_with("video/") && !f.name().to_lowercase().ends_with(".json"));
    for f in others {
        import_any(s.clone(), f);
    }
    match pictures.len() {
        0 => {}
        1 => import_file(s, pictures.into_iter().next().unwrap()),
        _ => wasm_bindgen_futures::spawn_local(async move {
            let from = s.borrow().app.doc.undo_depth();
            let mut ids = Vec::new();
            for file in pictures {
                ids.extend(add_picture(&s, &file).await);
            }
            s.borrow_mut().app.arrange_imported(&ids, from, None);
        }),
    }
}

fn import_any(s: Rc<RefCell<State>>, file: web_sys::File) {
    if file.name().to_lowercase().ends_with(".json") {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = restore_backup(file).await {
                import_failed("the backup", &err_text(&e));
            }
        });
    } else if file.type_().starts_with("video/") {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = import_video(s, &file).await {
                import_failed(&file.name(), &err_text(&e));
            }
        });
    } else {
        import_file(s, file);
    }
}

/// Plays a dropped video in a hidden `<video>` element (the browser
/// decodes), stores its bytes, and copies each frame into a texture.
/// Big videos are made smaller first (see `shrink`).
async fn import_video(s: Rc<RefCell<State>>, file: &web_sys::File) -> Result<(), JsValue> {
    let name = file.name();
    match shrink::prepare(file, &name).await.map_err(|e| JsValue::from_str(&e))? {
        shrink::Verdict::Keep => import_video_blob(s, file, &name, &file.type_()).await.map(|_| ()),
        shrink::Verdict::Shrunk(bytes) => {
            let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(&bytes[..]));
            let bag = web_sys::BlobPropertyBag::new();
            bag.set_type("video/mp4");
            let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)?;
            import_video_blob(s, &blob, &name, "video/mp4").await.map(|_| ())
        }
    }
}

async fn import_video_blob(s: Rc<RefCell<State>>, file: &web_sys::Blob, name: &str, mime: &str) -> Result<uuid::Uuid, JsValue> {
    const MAX_BYTES: f64 = 20_000_000.0;
    if file.size() > MAX_BYTES {
        // Dropped videos are made smaller before they get here.
        return Err(format!("{:.1} MB is over the 20 MB limit here; the desktop app makes it smaller", file.size() / 1e6).into());
    }
    let video = video_element(file).await?;
    let buf = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    let ext = match mime {
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
    let id = st.app.import_video(&bytes, ext, meta, Some(name), js_sys::Date::now() as u64).map_err(|e| e.to_string())?;
    let hash = st.app.media_hash(&id).ok_or("video item missing")?;
    let tex = st.renderer.create_video_texture(meta.width, meta.height).map_err(|e| e.to_string())?;
    st.app.attach_texture(hash.clone(), tex);
    let _ = video.play();
    st.videos.push((video, hash, tex));
    Ok(id)
}

/// A muted, looping `<video>` for a blob, once it can show frames.
async fn video_element(blob: &web_sys::Blob) -> Result<web_sys::HtmlVideoElement, JsValue> {
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
    video.set_src(&web_sys::Url::create_object_url_with_blob(blob)?);
    wasm_bindgen_futures::JsFuture::from(loaded).await.map_err(|_| JsValue::from_str("the browser cannot play this video"))?;
    Ok(video)
}

/// Gives a saved video on the board its player again.
async fn attach_player(s: Rc<RefCell<State>>, blob: &web_sys::Blob, hash: wall_app::ContentHash, w: u32, h: u32) -> Result<(), JsValue> {
    let video = video_element(blob).await?;
    let mut st = s.borrow_mut();
    let st = &mut *st;
    let tex = st.renderer.create_video_texture(w, h).map_err(|e| e.to_string())?;
    st.app.attach_texture(hash.clone(), tex);
    let _ = video.play();
    st.videos.push((video, hash, tex));
    Ok(())
}

/// Reads a dropped or pasted file and imports it as a picture.
fn import_file(s: Rc<RefCell<State>>, file: web_sys::File) {
    wasm_bindgen_futures::spawn_local(async move {
        add_picture(&s, &file).await;
    });
}

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}
