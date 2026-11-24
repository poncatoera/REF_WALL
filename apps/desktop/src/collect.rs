//! The browser extension's door into the app: a small HTTP server on
//! 127.0.0.1 that takes pictures, videos and links from web pages.
//!
//! - `GET /status`: the app, its boards and the open one, recent sends.
//! - `POST /file?name=&board=&page=&title=`: the body is a picture or video
//!   file (the extension fetched it, or took a screenshot).
//! - `POST /link` (JSON `{url, page, title, referer, board, start, end,
//!   cookies}`): a video page or a direct link to a file; the app downloads
//!   it, with yt-dlp for video sites (fetched once into Wall's own folder).
//!
//! Only the extension may talk to it: requests must name 127.0.0.1 or
//! localhost as their host, and POSTs must come from a browser extension's
//! origin (GETs may have none, as Chrome sends extensions' GETs; a page
//! can't read those answers), so a web page can't send things to Wall or
//! read the board names.
//! `WALL_COLLECT_PORT` changes the port; `WALL_COLLECT_DEV=1` also allows
//! requests with no origin at all (curl, tests).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uuid::Uuid;
use wall_app::App;

pub const PORT: u16 = 47830;
/// Biggest file the extension may send: videos are shrunk after.
const MAX_BODY: u64 = 2 << 30;
/// How long a request waits for the window to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// What a sent thing becomes on the board.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// None: the open board.
    pub board: Option<Uuid>,
    pub page: Option<String>,
    pub title: Option<String>,
}

/// What the server thread asks the window for.
enum Ask {
    Status(SyncSender<Value>),
    /// A file on disk, ready to import.
    File { path: PathBuf, meta: Meta, reply: SyncSender<Value> },
    /// A link to download first.
    Link { link: Link, meta: Meta, reply: SyncSender<Value> },
}

#[derive(Debug, Clone)]
pub struct Link {
    pub url: String,
    pub referer: Option<String>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub cookies: Option<String>,
}

/// A download that finished (or didn't), back on the window's thread.
struct Downloaded {
    job: u64,
    result: Result<PathBuf, String>,
    meta: Meta,
}

/// One send, for the extension's "recent" list.
#[derive(Debug, Clone)]
struct Job {
    id: u64,
    label: String,
    board: String,
    state: &'static str,
    error: Option<String>,
    at: Instant,
}

pub struct Collector {
    asks: Receiver<Ask>,
    done_tx: Sender<Downloaded>,
    done: Receiver<Downloaded>,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    jobs: Vec<Job>,
    next_job: u64,
    /// Video imports still converting: tag -> (job, meta).
    videos: HashMap<u64, (u64, Meta)>,
    recent: Option<Recent>,
}

impl Collector {
    /// Starts listening. `wake` brings the window's event loop round so it
    /// answers; it's called from other threads. None if the port is taken
    /// (another Wall is open).
    pub fn start(wake: impl Fn() + Send + Sync + Clone + 'static) -> Option<Self> {
        let port = std::env::var("WALL_COLLECT_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT);
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("wall: browser extension port {port} unavailable: {e}");
                return None;
            }
        };
        let (tx, asks) = mpsc::channel();
        let w = wake.clone();
        std::thread::Builder::new()
            .name("collect".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, w) = (tx.clone(), w.clone());
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream, port, &tx, &w) {
                            eprintln!("wall: extension request failed: {e}");
                        }
                    });
                }
            })
            .ok()?;
        clean_downloads();
        let (done_tx, done) = mpsc::channel();
        println!("wall: browser extension can send to 127.0.0.1:{port}");
        Some(Self { asks, done_tx, done, wake: std::sync::Arc::new(wake), jobs: Vec::new(), next_job: 1, videos: HashMap::new(), recent: None })
    }

    /// Answers the extension and brings in what arrived. Call from the
    /// event loop. Returns true if the board changed.
    pub fn pump(&mut self, app: &mut App, videos: &mut crate::video::Videos, failed: &mut Vec<String>) -> bool {
        let mut changed = false;
        while let Ok(ask) = self.asks.try_recv() {
            match ask {
                Ask::Status(reply) => {
                    let _ = reply.send(self.status(app));
                }
                Ask::File { path, meta, reply } => {
                    let label = meta.title.clone().or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
                    let job = self.new_job(app, &meta, label);
                    let _ = reply.send(json!({ "ok": true, "board": self.board_name(app, &meta), "job": job }));
                    changed |= self.bring_in(app, videos, failed, job, path, meta);
                }
                Ask::Link { link, meta, reply } => {
                    let job = self.new_job(app, &meta, meta.title.clone().unwrap_or_else(|| link.url.clone()));
                    self.set_state(job, "downloading", None);
                    let _ = reply.send(json!({ "ok": true, "board": self.board_name(app, &meta), "job": job }));
                    let (tx, wake) = (self.done_tx.clone(), self.wake_fn());
                    std::thread::spawn(move || {
                        let result = download(&link);
                        let _ = tx.send(Downloaded { job, result, meta });
                        wake();
                    });
                }
            }
        }
        while let Ok(d) = self.done.try_recv() {
            match d.result {
                Ok(path) => changed |= self.bring_in(app, videos, failed, d.job, path, d.meta),
                Err(e) => {
                    let label = self.jobs.iter().find(|j| j.id == d.job).map(|j| j.label.clone()).unwrap_or_default();
                    eprintln!("wall: could not download {label}: {e}");
                    failed.push(format!("{label}: {e}"));
                    self.set_state(d.job, "failed", Some(e));
                }
            }
        }
        for (tag, item) in std::mem::take(&mut videos.imported) {
            let Some((job, meta)) = self.videos.remove(&tag) else { continue };
            match item {
                Some(id) => {
                    self.place(app, id, &meta);
                    self.set_state(job, "added", None);
                    changed = true;
                }
                None => self.set_state(job, "failed", Some("could not convert the video".into())),
            }
        }
        changed
    }

    /// Wakes the window from a download thread.
    fn wake_fn(&self) -> impl Fn() + Send + 'static {
        let w = self.wake.clone();
        move || w()
    }

    fn new_job(&mut self, app: &App, meta: &Meta, label: String) -> u64 {
        let id = self.next_job;
        self.next_job += 1;
        self.jobs.push(Job { id, label: short(&label, 80), board: self.board_name(app, meta), state: "adding", error: None, at: Instant::now() });
        // Keep the last few.
        let n = self.jobs.len();
        if n > 12 {
            self.jobs.drain(..n - 12);
        }
        id
    }

    fn set_state(&mut self, job: u64, state: &'static str, error: Option<String>) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
            j.state = state;
            j.error = error;
            j.at = Instant::now();
        }
    }

    fn board_name(&self, app: &App, meta: &Meta) -> String {
        let id = meta.board.filter(|b| app.has_board(*b)).unwrap_or_else(|| app.board_id());
        app.board_choices().into_iter().find(|b| b.id == id).map(|b| b.name).unwrap_or_else(|| "Board".into())
    }

    /// Imports a file onto the board; videos finish later in [`Self::pump`].
    fn bring_in(&mut self, app: &mut App, videos: &mut crate::video::Videos, failed: &mut Vec<String>, job: u64, path: PathBuf, meta: Meta) -> bool {
        if crate::video::is_video(&path) {
            self.set_state(job, "converting", None);
            self.videos.insert(job, (job, meta));
            videos.import_tagged(path, Some(job));
            return false;
        }
        let before = failed.len();
        match crate::import_file(app, videos, path, failed) {
            Some(id) => {
                self.place(app, id, &meta);
                self.set_state(job, "added", None);
                true
            }
            None => {
                let why = failed.get(before).cloned().unwrap_or_else(|| "could not add it".into());
                self.set_state(job, "failed", Some(why));
                false
            }
        }
    }

    fn status(&self, app: &App) -> Value {
        let boards: Vec<Value> = app.board_choices().into_iter().map(|b| json!({ "id": b.id, "name": b.name, "depth": b.depth })).collect();
        let jobs: Vec<Value> = self
            .jobs
            .iter()
            .rev()
            .map(|j| json!({ "id": j.id, "label": j.label, "board": j.board, "state": j.state, "error": j.error, "secondsAgo": j.at.elapsed().as_secs() }))
            .collect();
        json!({
            "app": "Wall",
            "version": env!("CARGO_PKG_VERSION"),
            "current": app.board_id(),
            "boards": boards,
            // yt-dlp is fetched when first needed, so video sites always work.
            "videoSites": true,
            "jobs": jobs,
        })
    }
}

/// Things sent within this long of each other on the open board are laid
/// out side by side, like files dropped together.
const ROW_SECS: u64 = 120;

/// Sent things that landed on the open board lately.
struct Recent {
    board: Uuid,
    ids: Vec<Uuid>,
    centre: Option<(f64, f64)>,
    at: Instant,
}

impl Collector {
    /// Notes where it came from and moves it to the chosen board, or lays it
    /// out beside the last things sent to the open board.
    fn place(&mut self, app: &mut App, id: Uuid, meta: &Meta) {
        app.set_item_source(id, meta.page.as_deref(), meta.title.as_deref());
        if let Some(b) = meta.board.filter(|b| *b != app.board_id() && app.has_board(*b)) {
            app.select_only(id);
            app.move_selection_to_board(b);
            return;
        }
        let board = app.board_id();
        let mut r = self
            .recent
            .take()
            .filter(|r| r.board == board && r.at.elapsed().as_secs() < ROW_SECS)
            .unwrap_or(Recent { board, ids: Vec::new(), centre: None, at: Instant::now() });
        r.ids.retain(|i| app.media_hash(i).is_some());
        r.ids.push(id);
        r.centre = app.arrange_imported(&r.ids, app.doc.undo_depth(), r.centre);
        r.at = Instant::now();
        self.recent = Some(r);
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(n - 1).collect();
    t.push('…');
    t
}

// ---- HTTP ----------------------------------------------------------------

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
}

fn serve(stream: TcpStream, port: u16, tx: &Sender<Ask>, wake: &(impl Fn() + ?Sized)) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let Some(req) = read_head(&mut reader)? else { return Ok(()) };
    let origin = req.headers.get("origin").cloned();
    let respond = |out: &mut TcpStream, code: u16, body: &Value| -> std::io::Result<()> {
        let text = body.to_string();
        let reason = match code {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            413 => "Payload Too Large",
            _ => "Error",
        };
        let mut head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", text.len());
        if let Some(o) = origin.as_deref().filter(|o| allowed_origin(Some(o))) {
            head.push_str(&format!("Access-Control-Allow-Origin: {o}\r\nAccess-Control-Allow-Methods: GET, POST\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\nVary: Origin\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(text.as_bytes())?;
        out.flush()
    };
    let host_ok = req.headers.get("host").is_some_and(|h| h == &format!("127.0.0.1:{port}") || h == &format!("localhost:{port}"));
    // Chrome leaves the origin off an extension's GETs. A web page's GET
    // without one can't read the answer (no CORS header goes back), so only
    // the requests that change things must name an extension.
    let origin_ok = allowed_origin(origin.as_deref()) || (origin.is_none() && req.method == "GET");
    if !host_ok || !origin_ok {
        return respond(&mut out, 403, &json!({ "error": "only the Wall browser extension may send to Wall" }));
    }
    if req.method == "OPTIONS" {
        return respond(&mut out, 204, &json!({}));
    }
    let length: u64 = req.headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return respond(&mut out, 413, &json!({ "error": "that file is too big (over 2 GB)" }));
    }
    let ask = |make: &dyn Fn(SyncSender<Value>) -> Ask| -> Value {
        let (reply, answer) = mpsc::sync_channel(1);
        if tx.send(make(reply)).is_err() {
            return json!({ "error": "Wall is closing" });
        }
        wake();
        answer.recv_timeout(ANSWER_WAIT).unwrap_or_else(|_| json!({ "error": "Wall is busy; try again" }))
    };
    let meta = |board: Option<&str>, page: Option<&str>, title: Option<&str>| Meta {
        board: board.and_then(|b| b.parse().ok()),
        page: page.filter(|p| p.starts_with("http")).map(|p| short(p, 2000)),
        title: title.map(str::trim).filter(|t| !t.is_empty()).map(|t| short(t, 200)),
    };
    let answer = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/status") => ask(&|r| Ask::Status(r)),
        ("POST", "/file") => {
            let q = &req.query;
            let name = safe_name(q.get("name").map(String::as_str).unwrap_or("picture.png"));
            // A folder of its own keeps the file's own name for its title.
            let dir = downloads_dir().join(Uuid::new_v4().simple().to_string());
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(name);
            let mut file = std::fs::File::create(&path)?;
            let copied = std::io::copy(&mut (&mut reader).take(length), &mut file)?;
            drop(file);
            if copied != length || length == 0 {
                let _ = std::fs::remove_file(&path);
                return respond(&mut out, 400, &json!({ "error": "the file didn't arrive whole" }));
            }
            let m = meta(q.get("board").map(String::as_str), q.get("page").map(String::as_str), q.get("title").map(String::as_str));
            ask(&|r| Ask::File { path: path.clone(), meta: m.clone(), reply: r })
        }
        ("POST", "/link") => {
            let mut body = Vec::new();
            (&mut reader).take(length.min(4 << 20)).read_to_end(&mut body)?;
            let Ok(v) = serde_json::from_slice::<Value>(&body) else {
                return respond(&mut out, 400, &json!({ "error": "bad request" }));
            };
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
            let Some(url) = s("url").filter(|u| u.starts_with("http://") || u.starts_with("https://")) else {
                return respond(&mut out, 400, &json!({ "error": "that isn't a web link Wall can download" }));
            };
            let link = Link {
                url,
                referer: s("referer").filter(|r| r.starts_with("http")),
                start: v.get("start").and_then(Value::as_f64).filter(|t| *t >= 0.0),
                end: v.get("end").and_then(Value::as_f64).filter(|t| *t > 0.0),
                cookies: s("cookies").filter(|c| c.len() < 1 << 20),
            };
            let m = meta(s("board").as_deref(), s("page").as_deref(), s("title").as_deref());
            ask(&|r| Ask::Link { link: link.clone(), meta: m.clone(), reply: r })
        }
        _ => return respond(&mut out, 404, &json!({ "error": "not found" })),
    };
    let code = if answer.get("error").is_some() { 400 } else { 200 };
    respond(&mut out, code, &answer)
}

/// Chrome and Edge extensions are `chrome-extension://`, Firefox's
/// `moz-extension://`, Safari's `safari-web-extension://`.
fn allowed_origin(origin: Option<&str>) -> bool {
    match origin {
        Some(o) => ["chrome-extension://", "moz-extension://", "safari-web-extension://"].iter().any(|p| o.starts_with(p)),
        None => std::env::var_os("WALL_COLLECT_DEV").is_some(),
    }
}

fn read_head(reader: &mut impl BufRead) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else { return Ok(None) };
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let query = qs
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect();
    let mut headers = HashMap::new();
    let mut size = 0;
    let mut h = String::new();
    loop {
        h.clear();
        let n = reader.read_line(&mut h)?;
        size += n;
        if n == 0 || h == "\r\n" || h == "\n" || size > 64 << 10 {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    Ok(Some(Request { method: method.to_owned(), path: path.to_owned(), query, headers }))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A file name without folders or characters Windows refuses.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.');
    if clean.is_empty() { "file".into() } else { short(clean, 100) }
}

// ---- downloads -------------------------------------------------------------

/// Where sent files wait to be imported. Cleared of old ones at start.
fn downloads_dir() -> PathBuf {
    let d = std::env::temp_dir().join("wall-collect");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn clean_downloads() {
    let Ok(list) = std::fs::read_dir(downloads_dir()) else { return };
    let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
    for e in list.flatten() {
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < old) {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
}

const FILE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif", "mp4", "m4v", "mov", "webm", "mkv"];

/// A link straight to a picture or video file, by its extension.
fn direct_ext(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.rsplit('/').next()?;
    let ext = last.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_EXTS.contains(&ext.as_str()).then_some(ext)
}

/// Downloads a link: files directly, anything else (video pages) with yt-dlp.
fn download(link: &Link) -> Result<PathBuf, String> {
    let clip = link.start.is_some() || link.end.is_some();
    // A clip of a file is cut by yt-dlp too.
    if let Some(ext) = direct_ext(&link.url).filter(|_| !clip) {
        match download_file(link, &ext) {
            Ok(p) => return Ok(p),
            Err(e) => eprintln!("wall: direct download failed ({e}); trying yt-dlp"),
        }
    }
    ytdlp(link)
}

fn download_file(link: &Link, ext: &str) -> Result<PathBuf, String> {
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).build().new_agent();
    let mut req = agent.get(&link.url).header("User-Agent", "Mozilla/5.0 (Wall)");
    if let Some(r) = &link.referer {
        req = req.header("Referer", r);
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let path = downloads_dir().join(format!("{}.{ext}", Uuid::new_v4().simple()));
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut body = resp.into_body();
    let n = std::io::copy(&mut body.with_config().limit(MAX_BODY).reader(), &mut file).map_err(|e| e.to_string())?;
    if n == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("the link gave an empty file".into());
    }
    Ok(path)
}

/// yt-dlp: next to Wall, in Wall's own folder (fetched there the first time),
/// or on PATH. `WALL_YTDLP` points at another.
fn ytdlp_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let candidates = [
        std::env::var_os("WALL_YTDLP").map(PathBuf::from),
        std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(name))),
        Some(tools_dir().join(name)),
    ];
    if let Some(p) = candidates.into_iter().flatten().find(|p| p.is_file()) {
        return Some(p);
    }
    wall_media::ffmpeg::command(name).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from(name))
}

fn tools_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"));
    base.unwrap_or_else(std::env::temp_dir).join("Wall").join("tools")
}

/// Fetches the official yt-dlp release into Wall's tools folder.
fn fetch_ytdlp() -> Result<PathBuf, String> {
    let (asset, name) = if cfg!(windows) {
        ("yt-dlp.exe", "yt-dlp.exe")
    } else if cfg!(target_os = "macos") {
        ("yt-dlp_macos", "yt-dlp")
    } else {
        ("yt-dlp", "yt-dlp")
    };
    let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}");
    println!("wall: fetching yt-dlp for video sites from {url}");
    let dir = tools_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{name}.part"));
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().new_agent();
    let resp = agent.get(&url).call().map_err(|e| format!("could not fetch yt-dlp: {e}"))?;
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    std::io::copy(&mut resp.into_body().with_config().limit(100 << 20).reader(), &mut file).map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    let path = dir.join(name);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

static YTDLP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ytdlp(link: &Link) -> Result<PathBuf, String> {
    let exe = {
        let _one_at_a_time = YTDLP_LOCK.lock();
        match ytdlp_path() {
            Some(p) => p,
            None => fetch_ytdlp()?,
        }
    };
    let dir = downloads_dir();
    let stem = Uuid::new_v4().simple().to_string();
    let mut cmd = wall_media::ffmpeg::command(&exe.to_string_lossy());
    cmd.args(["--no-playlist", "--no-progress", "--quiet", "--no-warnings", "--no-mtime"])
        // A good picture without paying for 4K: Wall shrinks to 20 MB anyway.
        .args(["-f", "bv*[height<=1080][vcodec^=avc1]+ba[ext=m4a]/bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/b"])
        .args(["--merge-output-format", "mp4"])
        .arg("-o")
        .arg(dir.join(format!("{stem}.%(ext)s")))
        .args(["--print", "after_move:filepath"]);
    if let Some(ff) = wall_media::ffmpeg::ffmpeg() {
        cmd.arg("--ffmpeg-location").arg(ff);
    }
    if let Some(r) = &link.referer {
        cmd.args(["--referer", r]);
    }
    if link.start.is_some() || link.end.is_some() {
        let start = link.start.unwrap_or(0.0);
        let end = link.end.map_or("inf".to_owned(), |e| format!("{e:.2}"));
        // Cut at the nearest keyframes: an exact cut makes yt-dlp re-encode
        // the clip, and Wall would then encode it a second time.
        cmd.args(["--download-sections", &format!("*{start:.2}-{end}")]);
    }
    // The site's sign-in, for posts only signed-in people can see. The file
    // is deleted as soon as yt-dlp is done.
    let cookies = link.cookies.as_ref().map(|c| {
        let p = dir.join(format!("{stem}.cookies.txt"));
        let _ = std::fs::write(&p, c);
        p
    });
    if let Some(c) = &cookies {
        cmd.arg("--cookies").arg(c);
    }
    cmd.arg("--").arg(&link.url);
    let out = cmd.output().map_err(|e| format!("could not run yt-dlp: {e}"));
    if let Some(c) = &cookies {
        let _ = std::fs::remove_file(c);
    }
    let out = out?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or_else(|| err.lines().last().unwrap_or("download failed"));
        return Err(short(line.trim_start_matches("ERROR:").trim(), 200));
    }
    let printed = String::from_utf8_lossy(&out.stdout);
    let path = printed.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(PathBuf::from);
    path.filter(|p| p.is_file()).or_else(|| first_with_stem(&dir, &stem)).ok_or_else(|| "yt-dlp finished but left no video".into())
}

fn first_with_stem(dir: &Path, stem: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem) && p.extension().is_some_and(|e| e != "txt" && e != "part"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_extensions_on_localhost_get_in() {
        assert!(allowed_origin(Some("chrome-extension://abcdef")));
        assert!(allowed_origin(Some("moz-extension://x")));
        assert!(!allowed_origin(Some("https://evil.example")));
        assert!(!allowed_origin(Some("null")));
    }

    #[test]
    fn names_links_and_queries_are_tidied() {
        assert_eq!(safe_name("../../a:b?.png"), "a_b_.png");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(percent_decode("a%20b+c%2Fd%"), "a b c/d%");
        assert_eq!(direct_ext("https://x.com/a/b.JPG?w=2"), Some("jpg".into()));
        assert_eq!(direct_ext("https://youtube.com/watch?v=1"), None);
        assert_eq!(direct_ext("https://x.com/clip.mp4#t=3"), Some("mp4".into()));
    }

    #[test]
    fn a_request_head_is_read() {
        let raw = b"POST /file?name=a%20b.png&board=x HTTP/1.1\r\nHost: 127.0.0.1:47830\r\nOrigin: chrome-extension://id\r\nContent-Length: 3\r\n\r\nabc";
        let mut r = BufReader::new(&raw[..]);
        let req = read_head(&mut r).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/file");
        assert_eq!(req.query["name"], "a b.png");
        assert_eq!(req.headers["content-length"], "3");
        let mut rest = String::new();
        r.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "abc");
    }
}

// ---- installing the extension ---------------------------------------------

/// The extension's folder: next to wall.exe in an installed Wall, else the
/// repository's (running from a build).
pub fn extension_dir() -> Option<PathBuf> {
    let installed = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("extension")));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension");
    [installed, Some(repo)].into_iter().flatten().find(|d| d.join("manifest.json").is_file()).map(|d| tidy_path(&d))
}

/// Resolves `..` without Windows' `\\?\` prefix, which Explorer and the
/// browsers' folder pickers don't take.
fn tidy_path(p: &Path) -> PathBuf {
    let c = p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let s = c.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => c,
    }
}

/// The steps shown with the folder. Browsers won't open their extensions
/// page for another app, so that's step one.
pub fn install_steps(dir: &Path) -> String {
    format!(
        "Add Wall to your browser, once:\n\n\
         1. In Edge, type edge://extensions in the address bar (Chrome: chrome://extensions).\n\
         2. Turn on Developer mode.\n\
         3. Click Load unpacked and choose this folder (its address is on the clipboard, and it's open in a window too):\n\n\
         {}\n\n\
         Then right-click any picture, video or page and choose Add to Wall. Keep Wall open while you collect.",
        dir.display()
    )
}
