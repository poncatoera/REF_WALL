//! The desktop's live connection against the real server code, run on this
//! computer by wrangler. Skipped when `server/node_modules` isn't installed.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;
use wall_cloud::live::{Live, LiveEvent, LivePeer};
use wall_cloud::{Account, Client};

const PORT: u16 = 8798;
const KEY: &str = "test-owner-key-for-the-live-test-only";

struct Server(Child, tempfile::TempDir);

impl Drop for Server {
    fn drop(&mut self) {
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/pid", &self.0.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = &self.1; // the server's data, removed after it stops
    }
}

fn server() -> Option<Server> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../server");
    let wrangler = dir.join("node_modules/wrangler/bin/wrangler.js");
    if !wrangler.is_file() {
        eprintln!("skipped: run npm install in server/");
        return None;
    }
    let data = tempfile::tempdir().unwrap();
    let child = Command::new("node")
        .arg(&wrangler)
        .args(["dev", "--local", "--ip", "127.0.0.1", "--port", &PORT.to_string(), "--show-interactive-dev-session=false", "--persist-to"])
        .arg(data.path())
        .args(["--var", &format!("OWNER_HASH:{}", wall_cloud::key_hash(KEY))])
        .env("WRANGLER_SEND_METRICS", "false")
        .env("CI", "1")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let s = Server(child, data);
    let c = Client::new(Account { server: url(), key: KEY.into(), root: None, name: None, color: None }).unwrap();
    for _ in 0..240 {
        if c.me().is_ok() {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("the local server didn't start");
}

fn url() -> String {
    format!("http://127.0.0.1:{PORT}")
}

/// Waits for the people list that `want` accepts.
#[track_caller]
fn peers(l: &Live, want: impl Fn(&[LivePeer]) -> bool) -> Vec<LivePeer> {
    let until = Instant::now() + Duration::from_secs(10);
    let mut last = Vec::new();
    while Instant::now() < until {
        while let Some(e) = l.try_event() {
            if let LiveEvent::Peers(p) = e {
                last = p;
            }
        }
        if want(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("never saw the people wanted; last: {last:?}");
}

#[test]
fn two_desktops_on_a_board_see_each_other_and_their_pointers() {
    let Some(_server) = server() else { return };
    let board = Uuid::new_v4();
    let a = Live::start(&url(), KEY, "Paul", Some(2));
    a.board(board);
    let b = Live::start(&url(), KEY, "Studio PC", None);
    b.board(board);
    let seen = peers(&a, |p| p.len() == 1);
    assert_eq!(seen[0].name, "Studio PC");
    let paul = peers(&b, |p| p.len() == 1);
    assert_eq!((paul[0].name.as_str(), paul[0].color), ("Paul", Some(2)));
    a.color(Some(5));
    peers(&b, |p| p.first().is_some_and(|x| x.color == Some(5)));

    b.cursor(Some([120.0, -40.5]));
    peers(&a, |p| p.first().is_some_and(|x| x.at == Some([120.0, -40.5])));
    b.cursor(None);
    peers(&a, |p| p.first().is_some_and(|x| x.at.is_none()));

    // B opens another board: A is alone again (told so, not just nothing heard).
    b.board(Uuid::new_v4());
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(LiveEvent::Peers(p)) = a.try_event() {
            if p.is_empty() {
                break;
            }
        }
        assert!(Instant::now() < until, "A should hear that B left");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Back together: B follows A, sees A's view, and is told when A opens another board.
    b.board(board);
    let a_id = peers(&b, |p| p.len() == 1)[0].id.clone();
    peers(&a, |p| p.len() == 1);
    b.follow(Some(a_id.clone()));
    peers(&a, |p| p.first().is_some_and(|x| x.follow.as_deref() == Some(a_id.as_str())));
    a.view(Some([1.0, 2.0, 300.0, 200.0]));
    peers(&b, |p| p.first().is_some_and(|x| x.view == Some([1.0, 2.0, 300.0, 200.0])));
    let next = Uuid::new_v4();
    a.board(next);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(LiveEvent::Goto { id, board }) = b.try_event() {
            assert_eq!((id, board), (a_id.clone(), next));
            break;
        }
        assert!(Instant::now() < until, "B should be told where A went");
        std::thread::sleep(Duration::from_millis(20));
    }
    // A wrong key stops for good.
    let c = Live::start(&url(), "a-key-nobody-has-0000000000", "x", None);
    c.board(board);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(LiveEvent::Stopped(_)) = c.try_event() {
            break;
        }
        assert!(Instant::now() < until, "a wrong key should stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}
