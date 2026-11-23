//! Cloud sync on the desktop. A worker thread keeps a library and its cloud
//! copy the same; the app's thread only hands it boards in memory and merges
//! what arrives (see [`Worker`]).
//!
//! Where things are kept (never inside the library, which people share):
//! `cloud.json` holds each library's server and key, `cloud-<library>.json`
//! what has been synced.

pub mod live;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wall_format::{Board, ContentHash, ItemSidecar, Library};
use wall_sync::{Incoming, Reply, ReplyItem, State};

/// The server takes files up to this size.
const MAX_FILE: u64 = 25_000_000;
/// The longest answer read (a page of items).
const MAX_REPLY: u64 = 16_000_000;
/// How often to look for changes: just after something happened, and idle.
/// The free plan answers 100,000 requests a day.
const SOON: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_secs(30);
const LIVELY: Duration = Duration::from_secs(120);

/// Where a library's cloud copy is and the key to it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// `https://…`, without a trailing slash. Empty: not connected yet.
    pub server: String,
    pub key: String,
    /// The folder that syncs. A copy of the library (say, a backup) has the
    /// same id, and opening it must not bring deleted boards back.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// The name others on a shared board see for this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// Not derived: a key must not end up in a log.
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Account({}, key hidden)", self.server)
    }
}

/// `cloud.json` in the settings folder: library id → account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Accounts {
    #[serde(default)]
    pub libraries: HashMap<Uuid, Account>,
}

impl Accounts {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join("cloud.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// The account of the library in this folder, if it's connected.
    pub fn find(&self, library: &Library) -> Option<Account> {
        let a = self.libraries.get(&library.manifest().id)?;
        let same = a.root.as_deref().is_none_or(|r| same_folder(r, library.root()));
        (same && !a.server.is_empty()).then(|| a.clone())
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        write_atomic(&dir.join("cloud.json"), &serde_json::to_vec_pretty(self)?)
    }
}

pub fn same_folder(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    full(a) == full(b)
}

/// Wall's settings folder: next to `WALL_SETTINGS` if that's set (tests).
pub fn settings_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(p) = env("WALL_SETTINGS") {
        return p.parent().map(Path::to_owned);
    }
    if cfg!(windows) {
        Some(env("APPDATA")?.join("Wall"))
    } else if cfg!(target_os = "macos") {
        Some(env("HOME")?.join("Library/Application Support/Wall"))
    } else {
        Some(env("XDG_CONFIG_HOME").or_else(|| Some(env("HOME")?.join(".config")))?.join("wall"))
    }
}

/// A new owner key, from the system's random numbers (three v4 UUIDs: 366
/// random bits).
pub fn new_key() -> String {
    (0..3).map(|_| Uuid::new_v4().simple().to_string()).collect()
}

/// What the server keeps instead of a key.
pub fn key_hash(key: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(key.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[derive(Debug, Clone, PartialEq)]
pub enum CloudError {
    /// No connection, or the server is out for the moment (also: the free
    /// plan's daily limit is used up). Try again later.
    Offline(String),
    /// The key or link doesn't work (any more).
    Refused(String),
    /// The space Wall may use is full.
    Full,
    Missing,
    Other(String),
}

impl std::fmt::Display for CloudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloudError::Offline(e) => write!(f, "the cloud can't be reached: {e}"),
            CloudError::Refused(e) => write!(f, "{e}"),
            CloudError::Full => write!(f, "the cloud space Wall may use is full"),
            CloudError::Missing => write!(f, "not found"),
            CloudError::Other(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub role: String,
    #[serde(default)]
    pub media_bytes: Option<u64>,
    #[serde(default)]
    pub max_media_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Link {
    pub id: Uuid,
    pub role: String,
    #[serde(default)]
    pub board: Option<Uuid>,
    #[serde(default)]
    pub label: String,
    /// Only when just made.
    #[serde(default)]
    pub secret: Option<String>,
}

/// Requests to the server.
pub struct Client {
    account: Account,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(account: Account) -> Result<Self, CloudError> {
        let s = &account.server;
        // Plain http only to this computer (tests): the key is in each request.
        let local = ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| s.starts_with(p));
        if !(s.starts_with("https://") || local) || s.contains(['@', ' ', '"', '?', '#']) {
            return Err(CloudError::Other(format!("not a server address: {s}")));
        }
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(180)))
            .http_status_as_error(false)
            // A redirect would carry the key to another address.
            .max_redirects(0)
            .max_redirects_will_error(true)
            .build()
            .new_agent();
        let server = account.server.trim_end_matches('/').to_owned();
        Ok(Self { account: Account { server, ..account }, agent })
    }

    fn request(&self, method: &str, path: &str, body: Option<&[u8]>, limit: u64) -> Result<Vec<u8>, CloudError> {
        let url = format!("{}/api/{path}", self.account.server);
        let key = format!("Bearer {}", self.account.key);
        let sent = match (method, body) {
            ("POST", b) => self.agent.post(&url).header("authorization", &key).send(b.unwrap_or_default()),
            ("PUT", b) => self.agent.put(&url).header("authorization", &key).send(b.unwrap_or_default()),
            ("DELETE", _) => self.agent.delete(&url).header("authorization", &key).call(),
            _ => self.agent.get(&url).header("authorization", &key).call(),
        };
        let mut r = sent.map_err(|e| CloudError::Offline(e.to_string()))?;
        let status = r.status().as_u16();
        let bytes = r.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| CloudError::Offline(e.to_string()))?;
        if status == 200 {
            return Ok(bytes);
        }
        let said = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| Some(v.get("error")?.as_str()?.chars().take(200).collect::<String>()));
        // Only Wall's own server answers with an "error": a 403 or 429 page
        // from Cloudflare itself means try later, not that the key is bad.
        let Some(said) = said else { return Err(CloudError::Offline(format!("answer {status}"))) };
        Err(match status {
            401 | 403 => CloudError::Refused(said),
            404 => CloudError::Missing,
            507 => CloudError::Full,
            409 | 429 | 500..=599 => CloudError::Offline(said),
            _ => CloudError::Other(said),
        })
    }

    fn json<T: for<'a> Deserialize<'a>>(&self, method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<T, CloudError> {
        let bytes = body.map(|b| b.to_string().into_bytes());
        let r = self.request(method, path, bytes.as_deref(), MAX_REPLY)?;
        serde_json::from_slice(&r).map_err(|e| CloudError::Other(format!("unexpected answer: {e}")))
    }

    pub fn me(&self) -> Result<Me, CloudError> {
        self.json("GET", "me", None)
    }

    pub fn sync(&self, push: &wall_sync::Push) -> Result<Reply, CloudError> {
        let v = serde_json::to_value(push).map_err(|e| CloudError::Other(e.to_string()))?;
        self.json("POST", "sync", Some(&v))
    }

    /// All items of one board.
    pub fn board_items(&self, board: Uuid) -> Result<(Vec<ReplyItem>, Vec<serde_json::Value>), CloudError> {
        #[derive(Deserialize)]
        struct Page {
            items: Vec<ReplyItem>,
            sidecars: Vec<serde_json::Value>,
            after: String,
            more: bool,
        }
        let (mut items, mut sidecars, mut after) = (Vec::new(), Vec::new(), String::new());
        loop {
            let p: Page = self.json("GET", &format!("items?board={board}&after={after}"), None)?;
            items.extend(p.items);
            sidecars.extend(p.sidecars);
            if !p.more || p.after.is_empty() || p.after.len() > 40 || items.len() > 1_000_000 {
                return Ok((items, sidecars));
            }
            after = p.after;
        }
    }

    pub fn links(&self) -> Result<Vec<Link>, CloudError> {
        #[derive(Deserialize)]
        struct All {
            links: Vec<Link>,
        }
        Ok(self.json::<All>("GET", "links", None)?.links)
    }

    /// A new link: the whole library or one board, to view or to edit. Its
    /// secret is in the answer, this once.
    pub fn make_link(&self, board: Option<Uuid>, edit: bool, label: &str) -> Result<Link, CloudError> {
        let v = serde_json::json!({ "role": if edit { "edit" } else { "view" }, "board": board, "label": label });
        self.json("POST", "links", Some(&v))
    }

    pub fn revoke_link(&self, id: Uuid) -> Result<(), CloudError> {
        self.request("DELETE", &format!("links/{id}"), None, MAX_REPLY).map(|_| ())
    }

    /// The address to give someone. The secret is after `#`, which browsers
    /// keep to themselves.
    pub fn link_address(&self, secret: &str) -> String {
        format!("{}/#share={secret}", self.account.server)
    }

    pub fn upload(&self, hash: &ContentHash, bytes: &[u8]) -> Result<(), CloudError> {
        self.request("PUT", &format!("media/{hash}"), Some(bytes), MAX_REPLY).map(|_| ())
    }

    /// A file, checked to be the one asked for.
    pub fn download(&self, hash: &ContentHash) -> Result<Vec<u8>, CloudError> {
        let bytes = self.request("GET", &format!("media/{hash}"), None, MAX_FILE + 1)?;
        if ContentHash::of(&bytes) != *hash {
            return Err(CloudError::Other("a file arrived damaged".into()));
        }
        Ok(bytes)
    }
}

/// How sync is doing, for the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Working,
    UpToDate,
    /// Not connected; it keeps trying.
    Offline(String),
    /// Stopped: the key or link doesn't work (any more).
    Stopped(String),
    /// Boards sync, but new files don't fit any more.
    Full,
}

/// From the worker to the app.
pub enum Event {
    /// Merge this (the app's `sync_apply`), save, then call
    /// [`Worker::applied`]. Nothing more happens until then.
    Incoming(Arc<(Incoming, State)>),
    /// A file is in the library now.
    Media(Box<ItemSidecar>),
    Status(Status),
}

enum Msg {
    Boards(Vec<Board>),
    Applied(BTreeSet<Uuid>, Vec<Board>),
    /// Someone changed something: look now.
    Poke,
}

pub struct Worker {
    tx: Sender<Msg>,
    rx: Receiver<Event>,
}

impl Worker {
    /// `boards` are those in memory (the open one and those visited); the
    /// rest are read from the library.
    pub fn start(library: &Path, account: Account, settings: &Path, boards: Vec<Board>) -> Result<Self, CloudError> {
        let lib = Library::open(library).map_err(|e| CloudError::Other(e.to_string()))?;
        let client = Client::new(account)?;
        let state_file = settings.join(format!("cloud-{}.json", lib.manifest().id));
        let state = std::fs::read(&state_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (tx, inbox) = channel();
        let (out, rx) = channel();
        let mut e = Engine { lib, client, state, state_file, memory: boards, files: HashMap::new(), sidecars: HashMap::new(), out, inbox, full: false, status: None, view: false, poked: false };
        std::thread::Builder::new().name("wall-cloud".into()).spawn(move || e.run()).map_err(|e| CloudError::Other(e.to_string()))?;
        Ok(Self { tx, rx })
    }

    /// Boards in memory changed.
    pub fn boards(&self, boards: Vec<Board>) {
        let _ = self.tx.send(Msg::Boards(boards));
    }

    /// The last [`Event::Incoming`] was merged; `kept` is what `sync_apply`
    /// returned and `boards` the ones in memory now.
    pub fn applied(&self, kept: BTreeSet<Uuid>, boards: Vec<Board>) {
        let _ = self.tx.send(Msg::Applied(kept, boards));
    }

    pub fn try_event(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    /// Someone else changed something (see [`live`]): pull now.
    pub fn poke(&self) {
        let _ = self.tx.send(Msg::Poke);
    }
}

struct Engine {
    lib: Library,
    client: Client,
    state: State,
    state_file: PathBuf,
    memory: Vec<Board>,
    /// Boards and sidecars on disk, by file time, so each is read once.
    files: HashMap<Uuid, (SystemTime, Board)>,
    sidecars: HashMap<PathBuf, (SystemTime, ItemSidecar)>,
    out: Sender<Event>,
    inbox: Receiver<Msg>,
    full: bool,
    status: Option<Status>,
    /// The link can only look: nothing goes up.
    view: bool,
    /// A change elsewhere was announced during a round: another one at once.
    poked: bool,
}

/// The app went away.
struct Gone;

impl Engine {
    fn run(&mut self) {
        let mut last_change = Instant::now();
        let mut wait = Duration::ZERO;
        let mut fails = 0u32;
        loop {
            match self.inbox.recv_timeout(wait) {
                Ok(Msg::Boards(b)) => {
                    self.memory = b;
                    // Let a burst of edits settle into one push.
                    std::thread::sleep(Duration::from_millis(700));
                    while let Ok(m) = self.inbox.try_recv() {
                        if let Msg::Boards(b) = m {
                            self.memory = b;
                        }
                    }
                    last_change = Instant::now();
                }
                Ok(Msg::Poke) => last_change = Instant::now(),
                Ok(Msg::Applied(..)) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            match self.round() {
                Ok(Ok(changed)) => {
                    fails = 0;
                    if changed {
                        last_change = Instant::now();
                    }
                    wait = if std::mem::take(&mut self.poked) {
                        Duration::ZERO
                    } else if last_change.elapsed() < LIVELY {
                        SOON
                    } else {
                        IDLE
                    };
                    let s = if self.full { Status::Full } else { Status::UpToDate };
                    if self.say(s).is_err() {
                        return;
                    }
                }
                Ok(Err(CloudError::Refused(e))) => {
                    let _ = self.say(Status::Stopped(e));
                    return;
                }
                Ok(Err(e)) => {
                    fails += 1;
                    wait = Duration::from_secs((5u64 << fails.min(6)).min(300));
                    if self.say(Status::Offline(e.to_string())).is_err() {
                        return;
                    }
                }
                Err(Gone) => return,
            }
        }
    }

    fn say(&mut self, s: Status) -> Result<(), Gone> {
        if self.status.as_ref() != Some(&s) {
            self.status = Some(s.clone());
            self.out.send(Event::Status(s)).map_err(|_| Gone)?;
        }
        Ok(())
    }

    fn save_state(&self) {
        let r = serde_json::to_vec(&self.state).map_err(std::io::Error::other).and_then(|b| write_atomic(&self.state_file, &b));
        if let Err(e) = r {
            eprintln!("wall: could not save what's synced: {e}");
        }
    }

    /// Every board: from memory, else from disk (read again when changed).
    fn local(&mut self) -> Vec<Board> {
        let in_memory: BTreeSet<Uuid> = self.memory.iter().map(|b| b.id).collect();
        let mut out = self.memory.clone();
        let ids = self.lib.board_ids().unwrap_or_default();
        self.files.retain(|id, _| ids.contains(id));
        for id in ids.into_iter().filter(|id| !in_memory.contains(id)) {
            let path = self.lib.root().join("boards").join(format!("{id}.json"));
            let Ok(time) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
            if self.files.get(&id).is_none_or(|(t, _)| *t != time) {
                let Ok(b) = self.lib.read_board(id) else { continue };
                self.files.insert(id, (time, b));
            }
            out.extend(self.files.get(&id).map(|(_, b)| b.clone()));
        }
        out
    }

    fn local_sidecars(&mut self) -> Vec<ItemSidecar> {
        let Ok(dir) = std::fs::read_dir(self.lib.root().join("items")) else { return Vec::new() };
        let mut here = BTreeSet::new();
        for e in dir.flatten() {
            let path = e.path();
            let Some(hash) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<ContentHash>().ok()) else { continue };
            let Ok(time) = e.metadata().and_then(|m| m.modified()) else { continue };
            here.insert(path.clone());
            if self.sidecars.get(&path).is_none_or(|(t, _)| *t != time) {
                if let Ok(s) = self.lib.read_sidecar(&hash) {
                    self.sidecars.insert(path, (time, s));
                }
            }
        }
        self.sidecars.retain(|p, _| here.contains(p));
        let mut all: Vec<_> = self.sidecars.values().map(|(_, s)| s.clone()).collect();
        all.sort_by(|a, b| a.hash.cmp(&b.hash));
        all
    }

    /// Boards that were synced and are now in this library's trash. A board
    /// merely missing from disk is fetched again instead.
    fn deleted(&self, local: &[Board]) -> Vec<Uuid> {
        let here: BTreeSet<Uuid> = local.iter().map(|b| b.id).collect();
        let Ok(dir) = std::fs::read_dir(self.lib.root().join("trash")) else { return Vec::new() };
        dir.flatten()
            .filter_map(|e| e.file_name().to_str()?.split_once("-board-")?.1.strip_suffix(".json")?.parse().ok())
            .filter(|id| !here.contains(id))
            .collect()
    }

    /// One round: send, fetch, hand over, then files. True if anything moved.
    fn round(&mut self) -> Result<Result<bool, CloudError>, Gone> {
        let mut changed = false;
        loop {
            let local = self.local();
            let sidecars = self.local_sidecars();
            let deleted = self.deleted(&local);
            if self.state.looks_restored(local.iter().filter(|b| !deleted.contains(&b.id))) || self.state.boards.iter().any(|(id, b)| !b.meta.deleted && !b.items.is_empty() && !deleted.contains(id) && !local.iter().any(|l| l.id == *id)) {
                // A backup was put back, or a crash lost the last save.
                self.state.seq = 0;
                self.state.boards.retain(|id, _| local.iter().any(|l| l.id == *id) || deleted.contains(id));
                for b in self.state.boards.values_mut() {
                    b.items.clear();
                }
            }
            let mut push = if self.view { self.state.collect([], &[], &[]) } else { self.state.collect(&local, &sidecars, &deleted) };
            let sending = !push.is_empty();
            if sending {
                self.say(Status::Working)?;
            }
            let more_to_send = push.more;
            // Everything new, over as many pages as it takes, then one merge:
            // a board's name can come on a later page than its items.
            let before = self.state.clone();
            let mut inc = Incoming::default();
            loop {
                let reply = match self.client.sync(&push) {
                    Ok(r) => r,
                    Err(CloudError::Refused(e)) if !push.is_empty() && e.contains("only look") => {
                        self.view = true;
                        self.state = before;
                        return Ok(Ok(true));
                    }
                    Err(e) => {
                        // What was sent so far got there; what was fetched
                        // wasn't merged, so fetch it again next time.
                        self.state.seq = before.seq;
                        self.state.scope = before.scope;
                        return Ok(Err(e));
                    }
                };
                let (more, seq) = (reply.more, reply.seq);
                if !reply.refused.is_empty() {
                    eprintln!("wall: the cloud didn't take {} changes outside the shared board", reply.refused.len());
                }
                let part = self.state.take(&push, reply);
                inc.boards.extend(part.boards);
                inc.items.extend(part.items);
                inc.sidecars.extend(part.sidecars);
                inc.out_of_reach.extend(part.out_of_reach);
                inc.newly_reached.extend(part.newly_reached);
                inc.skipped += part.skipped;
                if !more || seq <= push.since {
                    break;
                }
                push = wall_sync::Push { since: self.state.seq, known: self.state.scope.iter().flatten().copied().collect(), ..Default::default() };
            }
            for b in std::mem::take(&mut inc.newly_reached) {
                match self.client.board_items(b) {
                    Ok((items, sidecars)) => self.state.take_items(items, sidecars, &mut inc),
                    Err(CloudError::Missing) => {}
                    Err(e) => {
                        self.state = before;
                        return Ok(Err(e));
                    }
                }
            }
            // Not what went up from here a moment ago.
            let st = &self.state;
            inc.boards.retain(|m| st.boards.get(&m.id).map(|b| &b.meta) != Some(m));
            inc.items.retain(|(b, i)| st.boards.get(b).and_then(|b| b.items.get(&i.id)).is_none_or(|(rev, by)| (i.rev, i.updated_by.as_str()) > (*rev, by.as_str())));
            inc.sidecars.retain(|s| serde_json::to_value(s).ok().as_ref() != st.sidecars.get(&s.hash));
            if inc.skipped > 0 {
                eprintln!("wall: {} things from the cloud couldn't be read", inc.skipped);
            }
            changed |= sending;
            if !inc.is_empty() {
                changed = true;
                for s in &inc.sidecars {
                    if !self.lib.media_path(&s.hash, &s.ext).is_file() {
                        self.state.wanted.insert(s.hash.clone(), s.clone());
                    }
                }
                let inc = Arc::new((inc, self.state.clone()));
                self.out.send(Event::Incoming(inc.clone())).map_err(|_| Gone)?;
                let kept = loop {
                    match self.inbox.recv() {
                        Ok(Msg::Applied(kept, boards)) => {
                            self.memory = boards;
                            break kept;
                        }
                        Ok(Msg::Boards(b)) => self.memory = b,
                        Ok(Msg::Poke) => self.poked = true,
                        Err(_) => return Err(Gone),
                    }
                };
                self.state.merged(&inc.0, &kept);
            }
            self.save_state();
            if !more_to_send {
                break;
            }
        }
        match self.files_round()? {
            Ok(moved) => Ok(Ok(changed || moved)),
            Err(e) => Ok(Err(e)),
        }
    }

    /// Sends files the server lacks and fetches those missing here.
    fn files_round(&mut self) -> Result<Result<bool, CloudError>, Gone> {
        let mut moved = false;
        let wanted: Vec<ItemSidecar> = self.state.wanted.values().cloned().collect();
        for s in wanted {
            self.say(Status::Working)?;
            match self.client.download(&s.hash) {
                Ok(bytes) => {
                    if let Err(e) = self.lib.import_bytes(&bytes, &s.ext) {
                        return Ok(Err(CloudError::Other(e.to_string())));
                    }
                    moved = true;
                    self.state.wanted.remove(&s.hash);
                    self.state.uploaded.insert(s.hash.clone());
                    if let Ok(v) = serde_json::to_value(&s) {
                        self.state.sidecars.insert(s.hash.clone(), v);
                    }
                    self.save_state();
                    self.out.send(Event::Media(Box::new(s))).map_err(|_| Gone)?;
                }
                // Not up yet from where it was added: look again later.
                Err(CloudError::Missing) => {}
                Err(CloudError::Other(e)) => eprintln!("wall: {}: {e}", s.hash),
                Err(e) => return Ok(Err(e)),
            }
        }
        if self.view || self.full {
            return Ok(Ok(moved));
        }
        for s in self.local_sidecars() {
            if self.state.uploaded.contains(&s.hash) || self.state.wanted.contains_key(&s.hash) {
                continue;
            }
            let path = self.lib.media_path(&s.hash, &s.ext);
            let Ok(size) = std::fs::metadata(&path).map(|m| m.len()) else { continue };
            if size == 0 || size > MAX_FILE {
                eprintln!("wall: {} is too big for the cloud ({size} bytes)", s.hash);
                self.state.uploaded.insert(s.hash.clone());
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            self.say(Status::Working)?;
            match self.client.upload(&s.hash, &bytes) {
                Ok(()) => {
                    moved = true;
                    self.state.uploaded.insert(s.hash.clone());
                    self.save_state();
                }
                Err(CloudError::Full) => {
                    self.full = true;
                    return Ok(Ok(moved));
                }
                Err(CloudError::Other(e)) => eprintln!("wall: {}: {e}", s.hash),
                Err(e) => return Ok(Err(e)),
            }
        }
        Ok(Ok(moved))
    }
}
