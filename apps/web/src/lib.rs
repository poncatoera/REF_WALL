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

    let state = Rc::new(RefCell::new(State { app, renderer, canvas: canvas.clone() }));

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
            let key = match k.as_str() {
                " " => Some(Key::Space { pressed: down }),
                _ if !down => None,
                "Delete" | "Backspace" => Some(Key::Delete),
                "Escape" => Some(Key::Escape),
                "z" | "Z" if cmd => Some(if e.shift_key() { Key::Redo } else { Key::Undo }),
                "y" | "Y" if cmd => Some(Key::Redo),
                "a" | "A" if cmd => Some(Key::SelectAll),
                _ => None,
            };
            if let Some(key) = key {
                e.prevent_default();
                s.borrow_mut().app.key(key);
            }
        });
        document.add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // requestAnimationFrame loop; only redraws when something changed.
    let frame: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let frame2 = frame.clone();
    *frame2.borrow_mut() = Some(Closure::new(move || {
        {
            let mut s = state.borrow_mut();
            let size = s.sync_size();
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

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}
