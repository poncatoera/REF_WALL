//! Big videos dropped on the web page are made smaller in the browser,
//! with the same rules as the desktop app (`wall_media::plan_video`):
//! H.264 MP4, at most 1080p, sized to land under 20 MB. Video only, as
//! Wall plays videos muted.

use wall_media::mp4::{self, Mp4Writer, VideoTrack};
use wall_media::{plan_video, Limits, VideoPlan, VideoProbe};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/shrink.js")]
extern "C" {
    #[wasm_bindgen(js_name = shrinkSupport, catch)]
    fn shrink_support(codec: &str, description: &[u8], out: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = shrinkVideo, catch)]
    fn shrink_video(file: &web_sys::Blob, codec: &str, description: &[u8], samples: &[f64], out: &JsValue, on_progress: &js_sys::Function) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = shrinkProgress)]
    pub fn shrink_progress(text: &str);
}

/// What to do with a dropped video.
pub enum Verdict {
    /// Store it as it is.
    Keep,
    /// Store this smaller copy instead.
    Shrunk(Vec<u8>),
}

async fn slice(file: &web_sys::Blob, start: u64, end: u64) -> Result<Vec<u8>, JsValue> {
    let part = file.slice_with_f64_and_f64(start as f64, end as f64)?;
    let buf = wasm_bindgen_futures::JsFuture::from(part.array_buffer()).await?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Reads the video track's sample table, without reading the frames.
async fn read_track(file: &web_sys::Blob) -> Result<VideoTrack, String> {
    let len = file.size() as u64;
    let mut at = 0u64;
    while at + 8 <= len {
        let head = slice(file, at, (at + 16).min(len)).await.map_err(|_| "the file can't be read".to_string())?;
        let h = mp4::box_header(&head).map_err(|e| e.to_string())?;
        let size = h.size.unwrap_or(len - at);
        if &h.kind == b"moov" {
            if size > 200_000_000 {
                return Err("its index is too big".into());
            }
            let moov = slice(file, at, at + size).await.map_err(|_| "the file can't be read".to_string())?;
            return mp4::parse_moov(&moov).map_err(|e| e.to_string());
        }
        at += size;
    }
    Err("only MP4 and MOV videos can be made smaller here".into())
}

fn set(o: &js_sys::Object, k: &str, v: impl Into<JsValue>) {
    let _ = js_sys::Reflect::set(o, &k.into(), &v.into());
}

/// Decides whether `file` must be converted, and converts it if so.
/// `name` is for the progress line.
pub async fn prepare(file: &web_sys::Blob, name: &str) -> Result<Verdict, String> {
    let limits = Limits::default();
    let bytes = file.size() as u64;
    let big = bytes > limits.max_video_bytes;
    let track = match read_track(file).await {
        Ok(t) => t,
        // A small file Wall can't read the index of: the browser may still play it.
        Err(_) if !big => return Ok(Verdict::Keep),
        Err(e) => return Err(format!("{:.0} MB is over the 20 MB limit, and {e}; the desktop app can make it smaller", bytes as f64 / 1e6)),
    };
    let (w, h) = track.upright_size();
    let probe = VideoProbe {
        bytes,
        duration_ms: track.duration_ms(),
        width: w,
        height: h,
        codec: match track.fourcc.as_str() {
            "avc1" | "avc3" => "h264".into(),
            "hvc1" | "hev1" => "hevc".into(),
            "av01" => "av1".into(),
            "vp09" => "vp9".into(),
            other => other.into(),
        },
        container: "mp4".into(),
        has_audio: false,
        max_keyframe_gap_ms: Some(track.max_keyframe_gap_ms()),
    };
    let settings = match plan_video(&probe, &limits).map_err(|e| e.to_string())? {
        VideoPlan::Keep => return Ok(Verdict::Keep),
        VideoPlan::Reencode(s) => s,
    };
    match convert(file, name, &track, &settings, &limits).await {
        Ok(v) => Ok(Verdict::Shrunk(v)),
        // Small files that only wanted tidying (keyframes, codec) go in as they are.
        Err(_) if !big => Ok(Verdict::Keep),
        Err(e) => Err(e),
    }
}

async fn convert(file: &web_sys::Blob, name: &str, track: &VideoTrack, s: &wall_media::video_rules::EncodeSettings, limits: &Limits) -> Result<Vec<u8>, String> {
    let fps = if track.duration_ms() > 0 { track.samples.len() as f64 * 1000.0 / track.duration_ms() as f64 } else { 30.0 };
    let fps = fps.clamp(1.0, 120.0);
    let ts = track.timescale as f64;
    let start = track.samples.iter().map(|x| x.cts).min().unwrap_or(0);
    let mut flat = Vec::with_capacity(track.samples.len() * 4);
    for x in &track.samples {
        flat.extend_from_slice(&[x.offset as f64, x.size as f64, ((x.cts - start) as f64 * 1e6 / ts).round(), if x.key { 1.0 } else { 0.0 }]);
    }

    let mut kbps = s.video_kbps;
    for attempt in 0..3 {
        let out = js_sys::Object::new();
        set(&out, "width", s.width);
        set(&out, "height", s.height);
        set(&out, "rotation", track.rotation);
        set(&out, "kbps", kbps);
        set(&out, "keyframeUs", (s.keyframe_interval_ms * 1000) as f64);
        set(&out, "fps", fps);
        let support = wasm_bindgen_futures::JsFuture::from(shrink_support(&track.codec, &track.description, &out).map_err(|e| crate::err_text(&e))?)
            .await
            .map_err(|e| crate::err_text(&e))?;
        let get = |k: &str| js_sys::Reflect::get(&support, &k.into()).ok().and_then(|v| v.as_string());
        if let Some(why) = get("error") {
            return Err(why);
        }
        set(&out, "codec", get("codec").unwrap_or_default());

        let label = name.to_owned();
        let progress = Closure::<dyn FnMut(f64)>::new(move |f: f64| {
            shrink_progress(&format!("Making {label} smaller… {:.0}%", f * 100.0));
        });
        shrink_progress(&format!("Making {name} smaller…"));
        let result = wasm_bindgen_futures::JsFuture::from(
            shrink_video(file, &track.codec, &track.description, &flat, &out, progress.as_ref().unchecked_ref()).map_err(|e| crate::err_text(&e))?,
        )
        .await;
        shrink_progress("");
        drop(progress);
        let result = result.map_err(|e| format!("converting failed ({})", js_error(&e)))?;

        let description = js_sys::Reflect::get(&result, &"description".into()).map_err(|e| crate::err_text(&e))?;
        let mut w = Mp4Writer::new(js_sys::Uint8Array::new(&description).to_vec(), s.width, s.height);
        let frames: js_sys::Array = js_sys::Reflect::get(&result, &"frames".into()).map_err(|e| crate::err_text(&e))?.into();
        let mut last = 0.0;
        for f in frames.iter() {
            let data = js_sys::Uint8Array::new(&js_sys::Reflect::get(&f, &"data".into()).unwrap_or_default()).to_vec();
            let time = js_sys::Reflect::get(&f, &"time".into()).ok().and_then(|v| v.as_f64()).unwrap_or(last);
            let key = js_sys::Reflect::get(&f, &"key".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false);
            w.push(&data, time as i64, key);
            last = time;
        }
        let file = w.finish((1e6 / fps) as i64);
        if (file.len() as u64) <= limits.max_video_bytes {
            return Ok(file);
        }
        // The encoder overshot: try again lower.
        let ratio = limits.max_video_bytes as f64 * 0.85 / file.len() as f64;
        kbps = ((kbps as f64 * ratio) as u32).max(200);
        web_sys::console::warn_1(&format!("wall: converted {name} is {} bytes; retry {attempt} at {kbps} kbit/s", file.len()).into());
    }
    Err("it stays over 20 MB even when converted; the desktop app can make it smaller".into())
}

fn js_error(e: &JsValue) -> String {
    js_sys::Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_else(|| crate::err_text(e))
}
