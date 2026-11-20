//! Wall web shell: runs `wall-app` in a `<canvas>` with WebGL2.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wall_app::{App, Wheel};
use wall_render::{GlslTarget, Renderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, PointerEvent, WebGl2RenderingContext, WheelEvent};

struct State {
    app: App,
    renderer: Renderer,
    canvas: HtmlCanvasElement,
    dirty: bool,
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
            self.dirty = true;
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
    let app = App::demo(60, 40);
    web_sys::console::log_1(&format!("wall: WebGL2 ready, {} boxes", app.box_count()).into());

    let state = Rc::new(RefCell::new(State { app, renderer, canvas: canvas.clone(), dirty: true }));

    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            let mut s = s.borrow_mut();
            let _ = s.canvas.set_pointer_capture(e.pointer_id());
            s.app.pointer_move([e.offset_x() as f32 * dpr(), e.offset_y() as f32 * dpr()]);
            s.app.pointer_down();
        });
        canvas.add_event_listener_with_callback("pointerdown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |_e: PointerEvent| s.borrow_mut().app.pointer_up());
        canvas.add_event_listener_with_callback("pointerup", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            let mut s = s.borrow_mut();
            s.app.pointer_move([e.offset_x() as f32 * dpr(), e.offset_y() as f32 * dpr()]);
            s.dirty = true;
        });
        canvas.add_event_listener_with_callback("pointermove", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(WheelEvent)>::new(move |e: WheelEvent| {
            e.prevent_default();
            let mut s = s.borrow_mut();
            let size = [s.canvas.width() as f32, s.canvas.height() as f32];
            // Browser wheel: positive deltaY scrolls down = zoom out.
            let w = match e.delta_mode() {
                WheelEvent::DOM_DELTA_LINE => Wheel::Lines(-e.delta_y() as f32),
                _ => Wheel::Pixels(-e.delta_y() as f32),
            };
            s.app.wheel(w, size);
            s.dirty = true;
        });
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        canvas.add_event_listener_with_callback_and_add_event_listener_options("wheel", cb.as_ref().unchecked_ref(), &opts)?;
        cb.forget();
    }

    // requestAnimationFrame loop; only redraws when something changed.
    let frame: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let frame2 = frame.clone();
    *frame2.borrow_mut() = Some(Closure::new(move || {
        {
            let mut s = state.borrow_mut();
            let size = s.sync_size();
            if s.dirty {
                let State { app, renderer, .. } = &mut *s;
                app.render(renderer, size);
                s.dirty = false;
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
