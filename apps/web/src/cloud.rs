//! A library or board opened by a share link (`…/#share=<secret>`): fetched
//! from the server this page came from, and kept up to date while it's open.
//! With an edit link, changes go back up.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wall_app::App;
use wall_format::{Board, BoardItem, ContentHash, ItemKind, ItemSidecar};
use wall_sync::{Incoming, Meta, Reply, ReplyItem, State as Sync};
use wasm_bindgen::prelude::*;

use crate::State;

#[wasm_bindgen(module = "/js/cloud.js")]
extern "C" {
    #[wasm_bindgen(js_name = cloudCall, catch)]
    fn cloud_call(method: &str, path: &str, key: &str, body: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = cloudNote)]
    fn note(text: &str);
}

#[wasm_bindgen(module = "/js/store.js")]
extern "C" {
    #[wasm_bindgen(js_name = storeGet, catch)]
    fn store_get(key: &str) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = storePut, catch)]
    fn store_put(key: &str, value: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = storeDelete, catch)]
    fn store_delete(prefix: &str) -> Result<js_sys::Promise, JsValue>;
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

/// History questions from the app (versions of a board, deleted boards),
/// answered by the server.
pub fn history(s: Rc<RefCell<State>>, ask: wall_app::HistoryAsk) {
    let Some(key) = link() else { return };
    wasm_bindgen_futures::spawn_local(async move {
        let answer = history_call(&key, ask).await;
        s.borrow_mut().app.history_answer(answer);
    });
}

#[derive(Deserialize)]
struct Times {
    times: Vec<u64>,
}

#[derive(Deserialize)]
struct DeletedRow {
    id: Uuid,
    #[serde(default)]
    name: String,
    #[serde(default)]
    parent: Option<Uuid>,
    #[serde(default)]
    deleted_ms: u64,
    #[serde(default)]
    items: usize,
    #[serde(default)]
    inside: usize,
}

async fn history_call(key: &str, ask: wall_app::HistoryAsk) -> wall_app::HistoryAnswer {
    use wall_app::{HistoryAnswer as A, HistoryAsk as Q};
    let failed = |f: Fail| {
        A::Failed(match f {
            Fail::Offline => "Can't reach the cloud right now. Try again in a moment.".into(),
            Fail::Refused(s) | Fail::Other(s) => s,
            Fail::Full => "The library is full.".into(),
            Fail::Missing => "That isn't there any more.".into(),
        })
    };
    match ask {
        Q::Versions { board } => match json::<Times>(key, "GET", &format!("history?board={board}"), None).await {
            Ok(t) => A::Versions { board, times: t.times },
            Err(f) => failed(f),
        },
        Q::Version { board, at_ms } => match call(key, "GET", &format!("history?board={board}&at={at_ms}"), &JsValue::NULL).await {
            Ok(gz) => match wall_format::history::board_from_gz(&gz, std::path::Path::new("version")) {
                Ok(mut version) if version.id == board => {
                    // Written by other people: only what can be drawn.
                    version.items.retain(|_, i| wall_sync::sane(i));
                    A::Version { board, at_ms, version }
                }
                Ok(_) => A::Failed("The cloud sent the wrong board.".into()),
                Err(e) => A::Failed(e.to_string()),
            },
            Err(f) => failed(f),
        },
        Q::Deleted => match json::<Vec<DeletedRow>>(key, "GET", "deleted", None).await {
            Ok(rows) => A::Deleted(
                rows.into_iter()
                    .map(|r| wall_app::DeletedBoard { id: r.id, name: r.name.chars().filter(|c| !c.is_control()).take(200).collect(), parent: r.parent, deleted_ms: r.deleted_ms, items: r.items, inside: r.inside })
                    .collect(),
            ),
            Err(f) => failed(f),
        },
        Q::Restore { board } => {
            let body = serde_json::json!({ "board": board }).to_string();
            match call(key, "POST", "undelete", &JsValue::from_str(&body)).await {
                Ok(_) => A::Restored(board),
                Err(f) => failed(f),
            }
        }
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
    // Changes kept in the browser while they couldn't be sent (see `Unsent`).
    let mut kept = 0usize;
    let mut kept_media = HashSet::new();
    let mut restored = false;
    loop {
        let done = async {
            if !known {
                view = json::<Me>(&key, "GET", "me", None).await?.role == "view";
                s.borrow_mut().app.set_view_only(view);
                s.borrow_mut().app.welcome_guest(view);
                known = true;
            }
            let mut moved = round(&s, &key, &mut sync, view, &mut uploads, &mut sent).await?;
            if !opened {
                opened = s.borrow_mut().app.sync_open_top(blank);
            }
            // Changes made here last time, kept while offline: on the
            // boards again now that they're here, and up with the next round.
            if !restored {
                restored = true;
                kept = unstash(&s, &key, &sync, &mut uploads, &mut kept_media).await;
                if kept > 0 {
                    moved |= round(&s, &key, &mut sync, view, &mut uploads, &mut sent).await?;
                }
            }
            let files = files(&s, &key, &mut failed).await?;
            Ok::<bool, Fail>(moved || files)
        }
        .await;
        fails = if done.is_ok() { 0 } else { fails + 1 };
        if done.is_ok() && (kept > 0 || !kept_media.is_empty()) {
            kept = stash(&s, &key, &sync, view, &mut uploads, &mut kept_media).await.unwrap_or(kept);
        }
        match done {
            Ok(moved) => {
                quiet = if moved { 0 } else { quiet + 1 };
                note(match (opened, view) {
                    (false, _) => "There's nothing on this link.",
                    (true, true) => "View only: comments (C) and stamps (E) are kept; other changes aren't saved",
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
                kept = stash(&s, &key, &sync, view, &mut uploads, &mut kept_media).await.unwrap_or(kept);
                offline_note(kept);
                quiet += 4;
            }
            Err(Fail::Missing) => quiet += 1,
            Err(Fail::Other(e)) => {
                web_sys::console::warn_1(&format!("wall: cloud: {e}").into());
                kept = stash(&s, &key, &sync, view, &mut uploads, &mut kept_media).await.unwrap_or(kept);
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
        // Changes made meanwhile are kept in the browser; the connection
        // coming back cuts the wait short.
        if fails > 0 {
            let mut left = (5_000 << fails.min(4)).min(60_000);
            let mut was_online = online();
            while left > 0 {
                sleep(400).await;
                left -= 400;
                let now_online = online();
                if now_online && !was_online {
                    note("Back online: sending…");
                    break;
                }
                was_online = now_online;
                let changed = {
                    let st = s.borrow();
                    !st.app.busy_editing() && st.app.sync_stamp() != sent
                };
                if changed {
                    if let Some(n) = stash(&s, &key, &sync, view, &mut uploads, &mut kept_media).await {
                        kept = n;
                        offline_note(kept);
                    }
                    sent = s.borrow().app.sync_stamp();
                }
            }
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

fn offline_note(kept: usize) {
    match kept {
        0 => note("No connection; trying again…"),
        1 => note("No connection: 1 change is kept in this browser and goes up when it's back"),
        n => note(&format!("No connection: {n} changes are kept in this browser and go up when it's back")),
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
        let mut push = if view { sync.collect_marks(&boards) } else { sync.collect(&boards, &sidecars, &[]) };
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

// ---- changes kept in this browser while they can't be sent ----------------

/// What was changed here and hasn't reached the server: kept in the
/// browser (IndexedDB) while offline, so closing the tab loses nothing.
/// The next time this link opens here, it goes up.
#[derive(Default, Serialize, Deserialize)]
struct Unsent {
    #[serde(default)]
    boards: Vec<Meta>,
    #[serde(default)]
    items: Vec<(Uuid, BoardItem)>,
    #[serde(default)]
    sidecars: Vec<ItemSidecar>,
    /// Files added here; their bytes are kept under `<key>/media/<hash>`.
    #[serde(default)]
    media: Vec<ContentHash>,
}

impl Unsent {
    fn is_empty(&self) -> bool {
        self.boards.is_empty() && self.items.is_empty() && self.media.is_empty()
    }

    /// Everything the server hasn't had from here (unlike a push, not cut
    /// into parts). None during a drag or typing.
    fn of(app: &App, sync: &Sync, view: bool, uploads: &[(ContentHash, Vec<u8>)]) -> Option<Self> {
        let boards = app.sync_boards()?;
        let mut u = Unsent::default();
        for b in &boards {
            let base = sync.boards.get(&b.id);
            let meta = Meta::of(b);
            // As in a push: a link's empty starting board isn't news.
            let blank = base.is_none() && b.items.is_empty();
            if !view && !blank && base.map(|x| &x.meta) != Some(&meta) {
                u.boards.push(meta);
            }
            for i in b.items.values() {
                if i.rev == 0 || (view && (base.is_none() || !matches!(i.kind, ItemKind::Comment { .. } | ItemKind::Stamp { .. }))) {
                    continue;
                }
                let known = base.and_then(|x| x.items.get(&i.id));
                if known.is_none_or(|(rev, by)| (i.rev, i.updated_by.as_str()) > (*rev, by.as_str())) {
                    u.items.push((b.id, i.clone()));
                }
            }
        }
        if !view {
            u.media = uploads.iter().map(|(h, _)| h.clone()).collect();
            u.sidecars = app.sync_sidecars().into_iter().filter(|s| u.media.contains(&s.hash) || serde_json::to_value(s).ok().as_ref() != sync.sidecars.get(&s.hash)).collect();
        }
        Some(u)
    }
}

/// Where this link's unsent changes are kept (not the secret itself).
fn unsent_key(key: &str) -> String {
    format!("unsent/{}", &ContentHash::of(key.as_bytes()).to_string()[..24])
}

async fn store_call(p: Result<js_sys::Promise, JsValue>) -> Result<JsValue, JsValue> {
    wasm_bindgen_futures::JsFuture::from(p?).await
}

/// Keeps what hasn't gone up in the browser. Returns how many changes that is.
async fn stash(s: &Rc<RefCell<State>>, key: &str, sync: &Sync, view: bool, uploads: &mut Vec<(ContentHash, Vec<u8>)>, kept_media: &mut HashSet<ContentHash>) -> Option<usize> {
    if !view {
        uploads.extend(s.borrow_mut().app.take_new_media().into_iter().map(|m| (m.hash, m.bytes)));
    }
    let u = Unsent::of(&s.borrow().app, sync, view, uploads)?;
    let at = unsent_key(key);
    if u.is_empty() {
        if !kept_media.is_empty() || store_call(store_get(&at)).await.is_ok_and(|v| !v.is_null()) {
            let _ = store_call(store_delete(&at)).await;
            kept_media.clear();
        }
        return Some(0);
    }
    for (hash, bytes) in uploads.iter() {
        if kept_media.contains(hash) {
            continue;
        }
        if store_call(store_put(&format!("{at}/media/{hash}"), &js_sys::Uint8Array::from(&bytes[..]))).await.is_ok() {
            kept_media.insert(hash.clone());
        }
    }
    let json = serde_json::to_string(&u).ok()?;
    if let Err(e) = store_call(store_put(&at, &JsValue::from_str(&json))).await {
        web_sys::console::warn_1(&format!("wall: could not keep unsent changes: {}", crate::err_text(&e)).into());
        return None;
    }
    Some(u.items.len() + u.boards.len() + u.media.len())
}

/// Puts changes kept from an earlier visit back on the boards (newer ones
/// from elsewhere still win), so the next round sends them.
async fn unstash(s: &Rc<RefCell<State>>, key: &str, sync: &Sync, uploads: &mut Vec<(ContentHash, Vec<u8>)>, kept_media: &mut HashSet<ContentHash>) -> usize {
    let at = unsent_key(key);
    let Some(text) = store_call(store_get(&at)).await.ok().and_then(|v| v.as_string()) else { return 0 };
    let Ok(u) = serde_json::from_str::<Unsent>(&text) else { return 0 };
    s.borrow_mut().app.sync_know(&u.sidecars);
    for hash in &u.media {
        let Ok(v) = store_call(store_get(&format!("{at}/media/{hash}"))).await else { continue };
        if !v.is_object() {
            continue;
        }
        let bytes = js_sys::Uint8Array::new(&v).to_vec();
        let Some(sc) = u.sidecars.iter().find(|s| &s.hash == hash) else { continue };
        kept_media.insert(hash.clone());
        let video = s.borrow_mut().app.sync_media_bytes(sc, &bytes);
        if let Ok(true) = video {
            let _ = play(s, sc, &bytes).await;
        }
        uploads.push((hash.clone(), bytes));
    }
    let n = u.items.len() + u.boards.len();
    let inc = Incoming { boards: u.boards, items: u.items, ..Default::default() };
    loop {
        if s.borrow_mut().app.sync_apply(&inc, sync).is_some() {
            break;
        }
        sleep(500).await;
    }
    n + u.media.len()
}

/// Whether the browser thinks it's online.
fn online() -> bool {
    web_sys::window().is_none_or(|w| w.navigator().on_line())
}
