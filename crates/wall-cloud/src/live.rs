//! The live connection on the desktop: who else is on the open board and
//! where their pointers are, and a nudge when someone changed something so
//! the sync worker pulls at once. Runs on its own thread; the window asks
//! for news each frame ([`Live::try_event`]).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

/// Pointer updates go out at most this often.
const CURSOR_EVERY: Duration = Duration::from_millis(100);
/// Keep-alive: the server answers "ping" without waking up (not billed).
const PING_EVERY: Duration = Duration::from_secs(30);
/// How long a read waits before the thread looks at what to send.
const POLL: Duration = Duration::from_millis(25);

/// Someone else on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct LivePeer {
    pub id: String,
    pub name: String,
    pub at: Option<[f32; 2]>,
}

pub enum LiveEvent {
    /// Everyone on the board now (besides us).
    Peers(Vec<LivePeer>),
    /// Something changed in the library: pull now.
    Changed,
    /// The key or link doesn't work: the connection stopped for good.
    Stopped(String),
}

enum Out {
    Board(Uuid),
    Cursor(Option<[f32; 2]>),
    Name(String),
}

pub struct Live {
    tx: Sender<Out>,
    rx: Receiver<LiveEvent>,
}

impl Live {
    /// Connects to `server` (`https://…`) with `key`, showing `name` to others.
    pub fn start(server: &str, key: &str, name: &str) -> Self {
        let (tx, inbox) = channel();
        let (out, rx) = channel();
        let url = live_url(server);
        let (key, name) = (key.to_owned(), name.to_owned());
        // TLS needs a crypto provider chosen once per program; ring, as
        // ureq uses (an Err only means one is set already).
        let _ = rustls::crypto::ring::default_provider().install_default();
        if let Some(url) = url {
            let _ = std::thread::Builder::new().name("wall-live".into()).spawn(move || run(url, key, name, inbox, out));
        }
        Self { tx, rx }
    }

    /// The board the window shows.
    pub fn board(&self, id: Uuid) {
        let _ = self.tx.send(Out::Board(id));
    }

    /// Our pointer on the board, or None when it's off it.
    pub fn cursor(&self, at: Option<[f32; 2]>) {
        let _ = self.tx.send(Out::Cursor(at));
    }

    pub fn name(&self, name: &str) {
        let _ = self.tx.send(Out::Name(name.to_owned()));
    }

    pub fn try_event(&self) -> Option<LiveEvent> {
        self.rx.try_recv().ok()
    }
}

/// `https://host` → `wss://host/api/live`.
///
/// None for anything but https (and this computer, for tests): the key goes
/// over this connection.
pub fn live_url(server: &str) -> Option<String> {
    let s = server.trim_end_matches('/');
    if s.contains(['@', ' ', '"', '?', '#']) {
        return None;
    }
    let local = ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| s.starts_with(p));
    let ws = match s.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None if local => format!("ws://{}", s.strip_prefix("http://")?),
        None => return None,
    };
    Some(format!("{ws}/api/live"))
}

#[derive(Deserialize)]
struct In {
    t: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    board: Option<String>,
    #[serde(default)]
    peers: Vec<WirePeer>,
    #[serde(default)]
    peer: Option<WirePeer>,
    #[serde(default)]
    x: Option<f32>,
    #[serde(default)]
    y: Option<f32>,
}

#[derive(Deserialize, Clone)]
struct WirePeer {
    id: String,
    name: String,
    #[serde(default)]
    x: Option<f32>,
    #[serde(default)]
    y: Option<f32>,
}

impl From<WirePeer> for LivePeer {
    fn from(p: WirePeer) -> Self {
        LivePeer { id: p.id, name: p.name, at: p.x.zip(p.y).map(|(x, y)| [x, y]) }
    }
}

/// What the thread keeps between connections.
struct Here {
    board: Option<Uuid>,
    cursor: Option<[f32; 2]>,
    name: String,
    peers: BTreeMap<String, LivePeer>,
}

fn run(url: String, key: String, name: String, inbox: Receiver<Out>, out: Sender<LiveEvent>) {
    let mut here = Here { board: None, cursor: None, name, peers: BTreeMap::new() };
    let mut retry = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match session(&url, &key, &mut here, &inbox, &out) {
            End::Gone => return,
            End::Stopped(why) => {
                let _ = out.send(LiveEvent::Stopped(why));
                return;
            }
            End::Lost => {}
        }
        if !here.peers.is_empty() {
            here.peers.clear();
            if out.send(LiveEvent::Peers(Vec::new())).is_err() {
                return;
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            retry = Duration::from_secs(1);
        }
        // Wait, but keep up with what the window says meanwhile.
        let until = Instant::now() + retry;
        while Instant::now() < until {
            match inbox.recv_timeout(until - Instant::now()) {
                Ok(m) => here.note(m),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(_) => return,
            }
        }
        retry = (retry * 2).min(Duration::from_secs(60));
    }
}

enum End {
    /// The window went away.
    Gone,
    /// Try again later.
    Lost,
    Stopped(String),
}

impl Here {
    fn note(&mut self, m: Out) {
        match m {
            Out::Board(b) => self.board = Some(b),
            Out::Cursor(c) => self.cursor = c,
            Out::Name(n) => self.name = n,
        }
    }
}

fn set_timeout(ws: &WebSocket<MaybeTlsStream<TcpStream>>, t: Option<Duration>) {
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(t);
}

fn session(url: &str, key: &str, here: &mut Here, inbox: &Receiver<Out>, out: &Sender<LiveEvent>) -> End {
    // No redirects: the key would go to wherever they lead.
    let limits = tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(1 << 20)).max_frame_size(Some(1 << 20));
    let Ok((mut ws, _)) = tungstenite::client::connect_with_config(url, Some(limits), 0) else { return End::Lost };
    set_timeout(&ws, Some(POLL));
    let send = |ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, v: serde_json::Value| ws.send(Message::text(v.to_string())).is_ok();
    let hello = json!({ "t": "hello", "key": key, "board": here.board.map(|b| b.to_string()), "name": here.name });
    if !send(&mut ws, hello) {
        return End::Lost;
    }
    let mut sent_board = here.board;
    let mut sent_name = here.name.clone();
    let mut sent_cursor: Option<Option<[f32; 2]>> = None;
    let mut last_cursor = Instant::now() - CURSOR_EVERY;
    let mut last_ping = Instant::now();
    loop {
        // What the window said.
        loop {
            match inbox.try_recv() {
                Ok(m) => here.note(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    return End::Gone;
                }
            }
        }
        if here.board != sent_board {
            if let Some(b) = here.board {
                if !send(&mut ws, json!({ "t": "board", "board": b.to_string() })) {
                    return End::Lost;
                }
            }
            sent_board = here.board;
            sent_cursor = None;
            here.peers.clear();
            if out.send(LiveEvent::Peers(Vec::new())).is_err() {
                return End::Gone;
            }
        }
        if here.name != sent_name {
            if !send(&mut ws, json!({ "t": "name", "name": here.name })) {
                return End::Lost;
            }
            sent_name = here.name.clone();
        }
        if here.peers.is_empty() {
            sent_cursor = None; // nobody to see it: nothing to send
        } else if sent_cursor != Some(here.cursor) && last_cursor.elapsed() >= CURSOR_EVERY {
            let (x, y) = match here.cursor {
                Some([x, y]) => (json!(x), json!(y)),
                None => (json!(null), json!(null)),
            };
            if !send(&mut ws, json!({ "t": "cursor", "x": x, "y": y })) {
                return End::Lost;
            }
            sent_cursor = Some(here.cursor);
            last_cursor = Instant::now();
        }
        if last_ping.elapsed() >= PING_EVERY {
            if ws.send(Message::text("ping")).is_err() {
                return End::Lost;
            }
            last_ping = Instant::now();
        }

        // What the server said.
        match ws.read() {
            Ok(Message::Text(t)) => {
                if t.as_str() == "pong" {
                    continue;
                }
                let Ok(m) = serde_json::from_str::<In>(t.as_str()) else { continue };
                if !here.apply(m, out) {
                    return End::Gone;
                }
            }
            Ok(Message::Close(frame)) => {
                let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
                if code == 4001 || code == 4003 {
                    let why = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                    return End::Stopped(why);
                }
                return End::Lost;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return End::Lost,
        }
    }
}

impl Here {
    /// Takes in a message from the server. False when the window went away.
    fn apply(&mut self, m: In, out: &Sender<LiveEvent>) -> bool {
        let before = self.peers.clone();
        match m.t.as_str() {
            "welcome" | "poke" => return out.send(LiveEvent::Changed).is_ok(),
            "peers" => {
                if m.board.as_deref() != self.board.map(|b| b.to_string()).as_deref() {
                    return true;
                }
                self.peers = m.peers.into_iter().map(|p| (p.id.clone(), p.into())).collect();
            }
            "peer" => {
                if let Some(p) = m.peer {
                    self.peers.insert(p.id.clone(), p.into());
                }
            }
            "cursor" => {
                if let Some(p) = m.id.and_then(|id| self.peers.get_mut(&id)) {
                    p.at = m.x.zip(m.y).map(|(x, y)| [x, y]);
                }
            }
            "leave" => {
                if let Some(id) = m.id {
                    self.peers.remove(&id);
                }
            }
            _ => {}
        }
        if self.peers != before {
            return out.send(LiveEvent::Peers(self.peers.values().cloned().collect())).is_ok();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_websocket_addresses() {
        assert_eq!(live_url("https://wall.example.dev/").as_deref(), Some("wss://wall.example.dev/api/live"));
        assert_eq!(live_url("http://127.0.0.1:8787").as_deref(), Some("ws://127.0.0.1:8787/api/live"));
        for s in ["http://wall.example.dev", "http://127.0.0.1.evil.dev:1", "ws://x", "https://a@b.dev", ""] {
            assert_eq!(live_url(s), None, "{s}");
        }
    }

    #[test]
    fn messages_keep_the_list_of_people() {
        let (tx, rx) = channel();
        let board = Uuid::new_v4();
        let mut h = Here { board: Some(board), cursor: None, name: "Paul".into(), peers: BTreeMap::new() };
        let m = |s: &str| serde_json::from_str::<In>(s).unwrap();
        let peers = format!(r#"{{"t":"peers","board":"{board}","peers":[{{"id":"a","name":"Mia","x":1,"y":2}}]}}"#);
        assert!(h.apply(m(&peers), &tx));
        assert!(matches!(rx.try_recv(), Ok(LiveEvent::Peers(p)) if p == vec![LivePeer { id: "a".into(), name: "Mia".into(), at: Some([1.0, 2.0]) }]));
        // For another board (an answer that crossed a board change): ignored.
        let other = r#"{"t":"peers","board":"00000000-0000-0000-0000-000000000000","peers":[]}"#;
        assert!(h.apply(m(other), &tx));
        assert!(rx.try_recv().is_err());
        assert!(h.apply(m(r#"{"t":"cursor","id":"a","x":null,"y":null}"#), &tx));
        assert!(matches!(rx.try_recv(), Ok(LiveEvent::Peers(p)) if p[0].at.is_none()));
        assert!(h.apply(m(r#"{"t":"peer","peer":{"id":"b","name":"Ben"}}"#), &tx));
        assert!(matches!(rx.try_recv(), Ok(LiveEvent::Peers(p)) if p.len() == 2));
        assert!(h.apply(m(r#"{"t":"leave","id":"a"}"#), &tx));
        assert!(matches!(rx.try_recv(), Ok(LiveEvent::Peers(p)) if p.len() == 1 && p[0].name == "Ben"));
        assert!(h.apply(m(r#"{"t":"poke","seq":9}"#), &tx));
        assert!(matches!(rx.try_recv(), Ok(LiveEvent::Changed)));
    }
}
