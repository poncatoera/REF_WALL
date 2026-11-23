//! A library or board opened by a share link (`…/#share=<secret>`): fetched
//! from the server this page came from, and kept up to date while it's open.
//! With an edit link, changes go back up.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use serde::Deserialize;
use uuid::Uuid;
use wall_app::App;
use wall_format::{Board, ContentHash, ItemSidecar};
use wall_sync::{Incoming, Reply, ReplyItem, State as Sync};
use wasm_bindgen::prelude::*;

use crate::State;

#[wasm_bindgen(module = "/js/cloud.js")]
extern "C" {
    #[wasm_bindgen(js_name = cloudCall, catch)]
    fn cloud_call(method: &str, path: &str, key: &str, body: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = cloudNote)]
    fn note(text: &str);
}

#[wasm_bindgen(module = "/js/live.js")]
extern "C" {
    #[wasm_bindgen(js_name = liveStart)]
    fn live_start(key: &str);
    #[wasm_bindgen(js_name = liveBoard)]
    fn live_board(board: &str);
    #[wasm_bindgen(js_name = liveCursor)]
    fn live_cursor(x: JsValue, y: JsValue);
    #[wasm_bindgen(js_name = liveTake)]
    fn live_take() -> JsValue;
    #[wasm_bindgen(js_name = liveWait)]
    fn live_wait(ms: i32, have: f64) -> js_sys::Promise;
    #[wasm_bindgen(js_name = liveMyName)]
    fn live_my_name() -> String;
    #[wasm_bindgen(js_name = liveSetName)]
    fn live_set_name(name: &str);
    #[wasm_bindgen(js_name = liveMyColor)]
    fn live_my_color() -> i32;
    #[wasm_bindgen(js_name = liveSetColor)]
    fn live_set_color(c: u8);
}

/// A colour picked on the board.
pub fn set_color(c: u8) {
    if LIVE.with(|l| l.borrow().is_some()) {
        live_set_color(c);
    }
}

/// A name typed on the board (the app's own badge).
pub fn set_name(name: &str) {
    if LIVE.with(|l| l.borrow().is_some()) {
        live_set_name(name);
    }
}

/// What this page last told the live connection.
struct Live {
    board: Option<Uuid>,
    cursor: Option<[f32; 2]>,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
struct LivePeer {
    id: String,
    name: String,
    x: Option<f32>,
    y: Option<f32>,
    #[serde(default)]
    color: Option<u8>,
}

/// Each frame, on a page opened by a link: tells the others which board
/// and where the pointer is, and shows who else is here.
pub fn tick(app: &mut App) {
    LIVE.with(|l| {
        let mut l = l.borrow_mut();
        let Some(l) = l.as_mut() else { return };
        // Our badge shows the name chosen in this browser ("Guest" until then).
        if !app.name_typing() {
            let n = live_my_name();
            let fixed = n.starts_with('=');
            app.set_name_fixed(fixed);
            let n = n.strip_prefix('=').map(str::to_owned).unwrap_or(n);
            let name = if n.trim().is_empty() { "Guest".to_owned() } else { n };
            if app.my_name() != Some(name.as_str()) {
                app.set_my_name(Some(name));
            }
        }
        let c = live_my_color();
        app.set_my_color((0..8).contains(&c).then_some(c as u8));
        let board = app.board_id();
        if l.board != Some(board) {
            l.board = Some(board);
            l.cursor = None;
            live_board(&board.to_string());
        }
        let at = app.pointer_on_board();
        // Small moves (less than a pixel on screen) aren't worth a message.
        let moved = match (at, l.cursor) {
            (Some(a), Some(b)) => (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) * app.camera.zoom >= 1.0,
            (a, b) => a.is_some() != b.is_some(),
        };
        if moved {
            l.cursor = at;
            match at {
                Some([x, y]) => live_cursor(x.into(), y.into()),
                None => live_cursor(JsValue::NULL, JsValue::NULL),
            }
        }
        if let Some(text) = live_take().as_string() {
            let peers: Vec<LivePeer> = serde_json::from_str(&text).unwrap_or_default();
            app.set_peers(
                peers
                    .into_iter()
                    .map(|p| wall_app::presence::Peer { at: p.x.zip(p.y).map(|(x, y)| [x, y]), id: p.id, name: p.name, color: p.color })
                    .collect(),
            );
        }
    });
}

/// The secret in this page's address, if it was opened by a link.
pub fn link() -> Option<String> {
    let hash = web_sys::window()?.location().hash().ok()?;
    let key = hash.strip_prefix("#share=")?;
    let fine = (20..=200).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    fine.then(|| key.to_owned())
}

/// Nothing yet; edits made here carry a name of their own.
pub fn empty_app() -> App {
    let me = Uuid::new_v4().simple().to_string();
    App::new(Board::new("Wall", 0), &format!("web-{}", &me[..8]))
}

enum Fail {
    /// Try again later.
    Offline,
    /// The link doesn't work (any more).
    Refused(String),
    Full,
    Missing,
    Other(String),
}

async fn call(key: &str, method: &str, path: &str, body: &JsValue) -> Result<Vec<u8>, Fail> {
    let p = cloud_call(method, path, key, body).map_err(|_| Fail::Offline)?;
    let r: js_sys::Array = wasm_bindgen_futures::JsFuture::from(p).await.map_err(|_| Fail::Offline)?.into();
    let status = r.get(0).as_f64().unwrap_or(0.0) as u16;
    let bytes = js_sys::Uint8Array::new(&r.get(1)).to_vec();
    if status == 200 {
        return Ok(bytes);
    }
    let said = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| Some(v.get("error")?.as_str()?.chars().take(200).collect::<String>()));
    let Some(said) = said else { return Err(Fail::Offline) };
    Err(match status {
        401 | 403 => Fail::Refused(said),
        404 => Fail::Missing,
        507 => Fail::Full,
        409 | 429 | 500..=599 => Fail::Offline,
        _ => Fail::Other(said),
    })
}

async fn json<T: for<'a> Deserialize<'a>>(key: &str, method: &str, path: &str, body: Option<String>) -> Result<T, Fail> {
    let b = body.map_or(JsValue::NULL, |b| JsValue::from_str(&b));
    serde_json::from_slice(&call(key, method, path, &b).await?).map_err(|e| Fail::Other(e.to_string()))
}

async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

#[derive(Deserialize)]
struct Me {
    role: String,
}

#[derive(Deserialize)]
struct Page {
    items: Vec<ReplyItem>,
    sidecars: Vec<serde_json::Value>,
    after: String,
    more: bool,
}

pub async fn run(s: Rc<RefCell<State>>, key: String) {
    note("Opening…");
    live_start(&key);
    LIVE.with(|l| *l.borrow_mut() = Some(Live { board: None, cursor: None }));
    let mut view = true;
    let mut known = false;
    let mut sync = Sync::default();
    let blank = s.borrow().app.board_id();
    let mut opened = false;
    let mut uploads: Vec<(ContentHash, Vec<u8>)> = Vec::new();
    let mut failed = HashSet::new();
    let mut quiet = 0u32;
    let mut fails = 0u32;
    // `App::sync_stamp` as of what was last sent.
    let mut sent = 0u64;
    loop {
        let done = async {
            if !known {
                view = json::<Me>(&key, "GET", "me", None).await?.role == "view";
                known = true;
            }
            let moved = round(&s, &key, &mut sync, view, &mut uploads, &mut sent).await?;
            if !opened {
                opened = s.borrow_mut().app.sync_open_top(blank);
            }
            let files = files(&s, &key, &mut failed).await?;
            Ok::<bool, Fail>(moved || files)
        }
        .await;
        fails = if done.is_ok() { 0 } else { fails + 1 };
        match done {
            Ok(moved) => {
                quiet = if moved { 0 } else { quiet + 1 };
                note(match (opened, view) {
                    (false, _) => "There's nothing on this link.",
                    (true, true) => "View only: changes you make here aren't saved",
                    (true, false) => "",
                });
            }
            Err(Fail::Refused(e)) => {
                note(&format!("Not up to date: {e}"));
                return;
            }
            Err(Fail::Full) => {
                note("The cloud space is full: new files stay in this browser only until you leave");
                uploads.clear();
                quiet += 1;
            }
            Err(Fail::Offline) => {
                note("No connection; trying again…");
                quiet += 4;
            }
            Err(Fail::Missing) => quiet += 1,
            Err(Fail::Other(e)) => {
                web_sys::console::warn_1(&format!("wall: cloud: {e}").into());
                quiet += 4;
            }
        }
        // Often while things change, then every half minute; less in a tab
        // nobody looks at (the free plan's requests are counted).
        let hidden = web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden());
        let wait = match (hidden, quiet < 24) {
            (true, _) => 120_000,
            (false, true) => 5_000,
            (false, false) => 30_000,
        };
        // After a failure only time helps: not a round per announced change,
        // which a server over its daily limit would turn into a storm.
        if fails > 0 {
            sleep((5_000 << fails.min(4)).min(60_000)).await;
            continue;
        }
        // Someone else's change cuts the wait short; so does one made here,
        // once it's finished and nothing has followed for a moment.
        let mut left = wait;
        while left > 0 {
            let step = left.min(400);
            left -= step;
            if wasm_bindgen_futures::JsFuture::from(live_wait(step, sync.seq as f64)).await.ok().and_then(|v| v.as_bool()) == Some(true) {
                quiet = 0;
                break;
            }
            if view {
                continue;
            }
            let changed = {
                let st = s.borrow();
                !st.app.busy_editing() && st.app.sync_stamp() != sent
            };
            if changed {
                sleep(250).await;
                quiet = 0;
                break;
            }
        }
        // A breath between rounds, whatever wakes them.
        sleep(100).await;
    }
}

/// Sends what changed here and merges what changed there.
async fn round(s: &Rc<RefCell<State>>, key: &str, sync: &mut Sync, view: bool, uploads: &mut Vec<(ContentHash, Vec<u8>)>, sent: &mut u64) -> Result<bool, Fail> {
    let mut moved = false;
    loop {
        // Not under a drag or while typing.
        let (boards, sidecars) = loop {
            let got = {
                let st = s.borrow();
                let b = st.app.sync_boards();
                if b.is_some() {
                    *sent = st.app.sync_stamp();
                }
                b.map(|b| (b, st.app.sync_sidecars()))
            };
            match got {
                Some(x) => break x,
                None => sleep(500).await,
            }
        };
        uploads.extend(s.borrow_mut().app.take_new_media().into_iter().map(|m| (m.hash, m.bytes)));
        if view {
            uploads.clear();
        }
        // Files before the pictures that show them: a board link may only
        // put a file on its board once it has sent it (or the same bytes).
        while let Some((hash, bytes)) = uploads.first() {
            let body = js_sys::Uint8Array::from(&bytes[..]);
            match call(key, "PUT", &format!("media/{hash}"), &body).await {
                Ok(_) => moved = true,
                Err(Fail::Other(e)) => web_sys::console::warn_1(&format!("wall: {hash}: {e}").into()),
                Err(e) => return Err(e),
            }
            uploads.remove(0);
        }
        let mut push = if view { sync.collect([], &[], &[]) } else { sync.collect(&boards, &sidecars, &[]) };
        let more_to_send = push.more;
        moved |= !push.is_empty();
        let before = sync.clone();
        let mut inc = Incoming::default();
        loop {
            let body = serde_json::to_string(&push).map_err(|e| Fail::Other(e.to_string()))?;
            let reply: Reply = match json(key, "POST", "sync", Some(body)).await {
                Ok(r) => r,
                Err(e) => {
                    sync.seq = before.seq;
                    sync.scope = before.scope;
                    return Err(e);
                }
            };
            let (more, seq) = (reply.more, reply.seq);
            let part = sync.take(&push, reply);
            inc.boards.extend(part.boards);
            inc.items.extend(part.items);
            inc.sidecars.extend(part.sidecars);
            inc.out_of_reach.extend(part.out_of_reach);
            inc.newly_reached.extend(part.newly_reached);
            if !more || seq <= push.since {
                break;
            }
            push = wall_sync::Push { since: sync.seq, known: sync.scope.iter().flatten().copied().collect(), ..Default::default() };
        }
        for b in std::mem::take(&mut inc.newly_reached) {
            let mut after = String::new();
            loop {
                let p: Page = match json(key, "GET", &format!("items?board={b}&after={after}"), None).await {
                    Ok(p) => p,
                    Err(Fail::Missing) => break,
                    Err(e) => {
                        *sync = before;
                        return Err(e);
                    }
                };
                sync.take_items(p.items, p.sidecars, &mut inc);
                if !p.more || p.after.is_empty() || p.after.len() > 40 {
                    break;
                }
                after = p.after;
            }
        }
        inc.boards.retain(|m| sync.boards.get(&m.id).map(|b| &b.meta) != Some(m));
        inc.items.retain(|(b, i)| sync.boards.get(b).and_then(|b| b.items.get(&i.id)).is_none_or(|(rev, by)| (i.rev, i.updated_by.as_str()) > (*rev, by.as_str())));
        if !inc.is_empty() {
            moved = true;
            let kept = loop {
                let k = {
                    let mut st = s.borrow_mut();
                    // What's merged now isn't news to send; what was changed
                    // here meanwhile still is.
                    let same = st.app.sync_stamp() == *sent;
                    let k = st.app.sync_apply(&inc, sync);
                    if k.is_some() && same {
                        *sent = st.app.sync_stamp();
                    }
                    k
                };
                match k {
                    Some(k) => break k,
                    None => sleep(500).await,
                }
            };
            s.borrow_mut().app.sync_know(&inc.sidecars);
            sync.merged(&inc, &kept);
        }
        if !more_to_send {
            break;
        }
    }
    Ok(moved)
}

/// Fetches files the open board shows, checked to be the ones asked for.
async fn files(s: &Rc<RefCell<State>>, key: &str, failed: &mut HashSet<ContentHash>) -> Result<bool, Fail> {
    let mut moved = false;
    loop {
        let next: Option<ItemSidecar> = s.borrow().app.sync_missing_here().into_iter().find(|m| !failed.contains(&m.hash));
        let Some(sc) = next else { return Ok(moved) };
        let bytes = match call(key, "GET", &format!("media/{}", sc.hash), &JsValue::NULL).await {
            Ok(b) => b,
            // Not uploaded yet from where it was added: look again later.
            Err(Fail::Missing) => return Ok(moved),
            Err(e) => return Err(e),
        };
        moved = true;
        if ContentHash::of(&bytes) != sc.hash {
            web_sys::console::warn_1(&format!("wall: {} arrived damaged", sc.hash).into());
            failed.insert(sc.hash.clone());
            continue;
        }
        let video = s.borrow_mut().app.sync_media_bytes(&sc, &bytes);
        match video {
            Ok(false) => {}
            Ok(true) => {
                if let Err(e) = play(s, &sc, &bytes).await {
                    web_sys::console::warn_1(&format!("wall: video: {}", crate::err_text(&e)).into());
                    failed.insert(sc.hash.clone());
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("wall: {}: {e}", sc.hash).into());
                failed.insert(sc.hash.clone());
            }
        }
    }
}

async fn play(s: &Rc<RefCell<State>>, sc: &ItemSidecar, bytes: &[u8]) -> Result<(), JsValue> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(crate::video_mime(&sc.ext));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)?;
    crate::attach_player(s.clone(), &blob, sc.hash.clone(), sc.width, sc.height).await
}
