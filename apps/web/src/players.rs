//! The browser's `<video>` elements: which play, which are loaded at all,
//! and getting their pictures into textures without needless work.

use std::collections::HashSet;

use wall_app::{App, ContentHash, VideoCommand, VideoStatus};
use wall_render::{ImageId, Renderer};
use wasm_bindgen::prelude::*;

/// Videos kept ready to play, however many there are on the board: the ones
/// playing plus the ones most recently on screen.
const KEEP_LOADED: usize = 24;
/// A video this long off screen may be unloaded.
const IDLE_MS: f64 = 4000.0;
/// The last picture of an unloaded video is kept at this width.
const IDLE_WIDTH: u32 = 640;
/// Without word of new pictures from the browser, look this often anyway.
const LOOK_MS: f64 = 250.0;

#[wasm_bindgen(module = "/js/video.js")]
extern "C" {
    #[wasm_bindgen(js_name = watchFrames)]
    fn watch_frames(video: &web_sys::HtmlVideoElement);
    #[wasm_bindgen(js_name = framesShown)]
    fn frames_shown(video: &web_sys::HtmlVideoElement) -> f64;
    #[wasm_bindgen(js_name = unloadVideo)]
    fn unload_video(video: &web_sys::HtmlVideoElement);
    #[wasm_bindgen(js_name = reloadVideo)]
    fn reload_video(video: &web_sys::HtmlVideoElement, url: &str, time: f64, rate: f64);
}

pub struct Player {
    video: web_sys::HtmlVideoElement,
    hash: ContentHash,
    tex: ImageId,
    url: String,
    loaded: bool,
    /// Where it was, for when it's loaded again.
    time: f64,
    duration: f64,
    rate: f64,
    /// When it was last worth playing.
    wanted_ms: f64,
    /// `frames_shown` and time of the picture in the texture, and when that
    /// was put there.
    count: f64,
    shown_time: f64,
    shown_ms: f64,
}

#[derive(Default)]
pub struct Players {
    list: Vec<Player>,
    /// Videos the person paused (others pause only while off screen).
    user_paused: HashSet<ContentHash>,
    /// Back-and-forth videos on their way back: paused, stepped backwards
    /// by hand each frame (browsers don't play backwards).
    reversing: HashSet<ContentHash>,
}

impl Player {
    fn new(video: web_sys::HtmlVideoElement, hash: ContentHash, tex: ImageId, now: f64) -> Self {
        watch_frames(&video);
        Player {
            url: video.src(),
            duration: video.duration(),
            time: video.current_time(),
            rate: 1.0,
            loaded: true,
            wanted_ms: now,
            count: -2.0,
            shown_time: -1.0,
            shown_ms: 0.0,
            video,
            hash,
            tex,
        }
    }

    fn load(&mut self) {
        if !self.loaded {
            self.loaded = true;
            reload_video(&self.video, &self.url, self.time, self.rate);
            self.shown_time = -1.0;
        }
    }

    fn unload(&mut self, renderer: &Renderer) {
        self.time = self.video.current_time();
        renderer.shrink_image(self.tex, self.video.video_width(), self.video.video_height(), IDLE_WIDTH);
        unload_video(&self.video);
        self.loaded = false;
    }

    /// Whether the video has a picture the texture doesn't.
    fn new_picture(&mut self, now: f64) -> bool {
        if self.video.ready_state() < 2 {
            return false;
        }
        let (count, time) = (frames_shown(&self.video), self.video.current_time());
        let moved = time != self.shown_time;
        // The browser's word first; but a seek, or a browser that never
        // says, shows up in the time.
        let fresh = if count >= 0.0 { count != self.count || (moved && now - self.shown_ms > LOOK_MS) } else { moved };
        if fresh {
            (self.count, self.shown_time, self.shown_ms) = (count, time, now);
        }
        fresh
    }
}

impl Players {
    /// A video whose first picture has loaded.
    pub fn add(&mut self, video: web_sys::HtmlVideoElement, hash: ContentHash, tex: ImageId) {
        self.list.push(Player::new(video, hash, tex, js_sys::Date::now()));
    }

    /// Once per frame, before drawing.
    pub fn tick(&mut self, app: &mut App, renderer: &Renderer) {
        if self.list.is_empty() {
            return;
        }
        let now = js_sys::Date::now();
        let Players { list, user_paused, reversing } = self;
        for (hash, cmd) in app.take_video_commands() {
            let Some(p) = list.iter_mut().find(|p| p.hash == hash) else { continue };
            p.load();
            p.wanted_ms = now;
            let v = &p.video;
            match cmd {
                VideoCommand::Play => {
                    user_paused.remove(&hash);
                    drop(v.play());
                }
                VideoCommand::Pause => {
                    user_paused.insert(hash.clone());
                    v.pause().unwrap_or(());
                }
                VideoCommand::Seek(t) => v.set_current_time(t),
                VideoCommand::Step(n) => {
                    user_paused.insert(hash.clone());
                    v.pause().unwrap_or(());
                    // The browser doesn't expose the frame rate; assume 30.
                    v.set_current_time((v.current_time() + n as f64 / 30.0).max(0.0));
                }
                VideoCommand::Speed(r) => {
                    p.rate = r as f64;
                    v.set_playback_rate(p.rate);
                }
                VideoCommand::Range(Some((a, _))) => v.set_current_time(a),
                VideoCommand::Range(None) => {}
                VideoCommand::PingPong(_) => {
                    reversing.remove(&hash);
                }
                // Not in the browser yet (it needs the frames around this one).
                VideoCommand::Onion(_) => {}
            }
        }
        // Only videos on screen (and big enough) play; the rest keep their last frame.
        let playable = app.playable_videos();
        for p in list.iter_mut() {
            let on_screen = playable.contains(&p.hash);
            if on_screen {
                p.wanted_ms = now;
                p.load();
            }
            if !p.loaded {
                continue;
            }
            let (v, hash) = (&p.video, &p.hash);
            let d = v.duration();
            if d.is_finite() {
                p.duration = d;
            }
            let (a, b) = app.video_range(hash).unwrap_or((0.0, d));
            let t = v.current_time();
            if reversing.contains(hash) {
                if !user_paused.contains(hash) && on_screen {
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
            let want = on_screen && !user_paused.contains(hash) && !reversing.contains(hash);
            if want && v.paused() {
                drop(v.play());
            } else if !want && !v.paused() {
                v.pause().unwrap_or(());
            }
            let fresh = p.new_picture(now);
            if fresh {
                use glow::HasContext;
                unsafe {
                    let gl = renderer.gl();
                    gl.bind_texture(glow::TEXTURE_2D, Some(renderer.texture(p.tex)));
                    gl.tex_image_2d_with_html_video(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, glow::RGBA, glow::UNSIGNED_BYTE, &p.video);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                }
                app.needs_redraw = true;
            }
            // The time moves on 60 times a second; that alone is no reason to
            // draw a board whose picture is the same.
            if fresh || !want {
                let status = VideoStatus { time: p.video.current_time(), duration: p.duration, playing: !user_paused.contains(&p.hash), speed: p.rate as f32 };
                app.set_video_status(p.hash.clone(), status);
            }
        }
        // At most one a frame: giving up a decoder takes a moment.
        if list.iter().filter(|p| p.loaded).count() > KEEP_LOADED {
            let oldest = list.iter_mut().filter(|p| p.loaded && p.count > -2.0 && now - p.wanted_ms > IDLE_MS).min_by(|a, b| a.wanted_ms.total_cmp(&b.wanted_ms));
            if let Some(p) = oldest {
                p.unload(renderer);
            }
        }
    }
}
