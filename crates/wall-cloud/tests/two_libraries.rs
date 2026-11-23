//! Two (and more) libraries syncing through the real server code, run on this
//! computer by wrangler. Skipped when `server/node_modules` isn't installed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;
use wall_app::{App, Button, Camera, Key, Mods};
use wall_cloud::{Account, Accounts, Client, CloudError, Event, Status, Worker};
use wall_format::{ContentHash, ItemKind, Library};

const PORT: u16 = 8797;
const KEY: &str = "test-owner-key-that-only-exists-in-this-test";

struct Server(Child, tempfile::TempDir);

impl Server {
    fn start() -> Option<Self> {
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
        let c = client(KEY);
        for _ in 0..240 {
            if c.me().is_ok() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!("the local server didn't start");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/pid", &self.0.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = &self.1;
    }
}

fn server() -> String {
    format!("http://127.0.0.1:{PORT}")
}

fn client(key: &str) -> Client {
    Client::new(account(key)).unwrap()
}

fn account(key: &str) -> Account {
    Account { server: server(), key: key.into(), root: None, name: None, color: None }
}

/// One computer: an app, its library, and its sync worker.
struct Pc {
    root: PathBuf,
    settings: PathBuf,
    app: App,
    worker: Worker,
    key: String,
    stamp: u64,
    status: Option<Status>,
    now: u64,
}

impl Pc {
    fn new(dir: &Path, name: &str, key: &str) -> Self {
        Self::open(dir.join(format!("{name}.wall")), dir.join(format!("{name}-settings")), name, key)
    }

    fn open(root: PathBuf, settings: PathBuf, name: &str, key: &str) -> Self {
        let mut app = App::open_library(&root, name, 1).unwrap();
        app.resize([1000.0, 800.0]);
        app.set_time(1000);
        let worker = Worker::start(&root, account(key), &settings, app.sync_boards().unwrap()).unwrap();
        let stamp = app.sync_stamp();
        Pc { root, settings, app, worker, key: key.into(), stamp, status: None, now: 1000 }
    }

    /// What the window does each frame.
    fn pump(&mut self) {
        self.now += 20;
        self.app.set_time(self.now);
        while let Some(e) = self.worker.try_event() {
            match e {
                Event::Incoming(x) => {
                    let kept = self.app.sync_apply(&x.0, &x.1).expect("nobody is dragging");
                    self.app.save_if_dirty().unwrap();
                    self.stamp = self.app.sync_stamp();
                    self.worker.applied(kept, self.app.sync_boards().unwrap());
                }
                Event::Media(s) => self.app.sync_media_arrived(*s),
                Event::Status(s) => self.status = Some(s),
            }
        }
        if self.app.sync_stamp() != self.stamp {
            self.app.save_if_dirty().unwrap();
            self.stamp = self.app.sync_stamp();
            self.worker.boards(self.app.sync_boards().unwrap());
        }
    }

    fn note(&mut self, text: &str) -> Uuid {
        let id = self.app.new_note([0.0, 0.0], text.into());
        self.app.end_edit();
        id
    }

    fn texts(&self) -> Vec<String> {
        let mut t: Vec<String> = self.app.doc.board.live_items().filter_map(|i| match &i.kind { ItemKind::Note { text, .. } => Some(text.clone()), _ => None }).collect();
        t.sort();
        t
    }

    fn lib(&self) -> Library {
        Library::open(&self.root).unwrap()
    }

    fn boards(&self) -> Vec<Uuid> {
        let mut b = self.lib().board_ids().unwrap();
        b.sort();
        b
    }
}

/// Runs all the computers until `done`, or fails.
fn until(what: &str, pcs: &mut [&mut Pc], mut done: impl FnMut(&[&mut Pc]) -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        pcs.iter_mut().for_each(|p| p.pump());
        if done(pcs) {
            return;
        }
        if Instant::now() > end {
            for p in pcs.iter() {
                eprintln!("{:?}: {:?} boards {:?} notes {:?}", p.root.file_name(), p.status, p.boards().len(), p.texts());
            }
            panic!("took too long: {what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn png(seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(60, 40, |x, y| image::Rgba([x as u8 * 4, y as u8 * 6, seed, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[test]
fn libraries_stay_the_same_through_the_cloud() {
    let Some(_server) = Server::start() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // A fills a library: notes, a picture, a board inside, a private board.
    let mut a = Pc::new(dir, "a", KEY);
    let home = a.app.board_id();
    a.note("one");
    a.note("two");
    let pic = a.app.import_image_bytes(&png(7), Some("pic.png"), 5).unwrap();
    let hash = a.app.media_hash(&pic).unwrap();
    let shared = a.app.new_board();
    let shared = a.lib().board_ids().unwrap().into_iter().find(|b| *b != home).unwrap_or(shared);
    a.app.open_board(shared, true);
    a.note("for the client");
    let pic2 = a.app.import_image_bytes(&png(9), Some("shared.png"), 6).unwrap();
    let hash2 = a.app.media_hash(&pic2).unwrap();
    a.app.go_up();
    a.app.save_if_dirty().unwrap();
    let me = client(KEY);
    until("a is up", &mut [&mut a], |p| p[0].status == Some(Status::UpToDate) && me.download(&hash).is_ok() && me.download(&hash2).is_ok());

    // A second computer, with a new empty library and the same key.
    let mut b = Pc::new(dir, "b", KEY);
    let blank = b.app.board_id();
    until("b has it all", &mut [&mut a, &mut b], |p| p[1].boards().contains(&home) && p[1].boards().contains(&shared) && p[1].lib().has_media(&hash) && p[1].lib().has_media(&hash2));
    assert!(!a.boards().contains(&blank), "b's empty first board stays there");
    b.app.open_board(home, false);
    assert_eq!(b.texts(), ["one", "two"]);
    let (la, lb) = (a.lib(), b.lib());
    let path = |l: &Library| l.media_path(&hash, &l.read_sidecar(&hash).unwrap().ext);
    assert_eq!(std::fs::read(path(&la)).unwrap(), std::fs::read(path(&lb)).unwrap());
    assert_eq!(la.read_sidecar(&hash).unwrap(), lb.read_sidecar(&hash).unwrap());

    // Both work at once.
    a.note("from a");
    b.note("from b");
    let all = ["from a", "from b", "one", "two"];
    until("both notes everywhere", &mut [&mut a, &mut b], |p| p.iter().all(|p| p.texts() == all));

    // Deleting a note with undo, across computers.
    a.app.key(Key::SelectAll);
    a.app.key(Key::Delete);
    until("b is empty too", &mut [&mut a, &mut b], |p| p[1].texts().is_empty());
    a.app.key(Key::Undo);
    until("undo reaches b", &mut [&mut a, &mut b], |p| p[1].texts() == all);

    // Nothing is merged into a board while something on it is dragged.
    b.app.set_camera(Camera { center: [500.0, 400.0], zoom: 1.0 });
    b.app.key(Key::Escape);
    let on = b.app.doc.board.live_items().find(|i| matches!(i.kind, ItemKind::Note { .. })).unwrap().frame;
    let at = [on.x as f32 + 5.0, on.y as f32 + 5.0];
    b.app.set_camera(Camera { center: [at[0], at[1]], zoom: 1.0 });
    b.app.pointer_down(Button::Primary, [500.0, 400.0], Mods::default());
    b.app.pointer_move([540.0, 430.0]);
    assert!(b.app.sync_boards().is_none());
    b.app.pointer_up(Button::Primary, [540.0, 430.0]);
    let stamp = |p: &Pc| p.app.doc.board.items.values().map(|i| (i.id, i.rev, i.updated_by.clone())).collect::<Vec<_>>();
    until("the drag reaches a", &mut [&mut a, &mut b], |p| stamp(p[0]) == stamp(p[1]));

    // A renamed board, and one deleted: to the trash on the other computer.
    a.app.rename_board(shared, "Client refs".into());
    until("renamed on b", &mut [&mut a, &mut b], |p| p[1].lib().read_board(shared).unwrap().name == "Client refs");
    a.app.new_board();
    a.app.save_if_dirty().unwrap();
    until("third board on b", &mut [&mut a, &mut b], |p| p[0].boards().len() == 3 && p[0].boards().iter().all(|x| p[1].boards().contains(x)));
    let third = a.boards().into_iter().find(|x| *x != home && *x != shared).unwrap();
    assert!(b.app.delete_board(third));
    until("deleted on a", &mut [&mut a, &mut b], |p| !p[0].boards().contains(&third));
    assert!(std::fs::read_dir(a.root.join("trash")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(&third.to_string())));
    // Brought back from the trash on b: back on a too, card and all.
    assert!(b.app.restore_deleted_board(third));
    b.app.save_if_dirty().unwrap();
    until("restored on a", &mut [&mut a, &mut b], |p| p[0].boards().contains(&third));
    assert!(b.app.delete_board(third));
    until("deleted again on a", &mut [&mut a, &mut b], |p| !p[0].boards().contains(&third));

    // A link to one board, to look at: that board and its files only.
    let view = me.make_link(Some(shared), false, "client", "").unwrap();
    let vkey = view.secret.clone().unwrap();
    let mut c = Pc::new(dir, "c", &vkey);
    until("the viewer has the board", &mut [&mut a, &mut b, &mut c], |p| p[2].boards().contains(&shared) && p[2].lib().has_media(&hash2));
    assert!(!c.boards().contains(&home));
    assert!(!c.lib().has_media(&hash), "a file used elsewhere stays there");
    c.app.open_board(shared, false);
    assert_eq!(c.texts(), ["for the client"]);
    c.note("not allowed");
    a.app.open_board(shared, true);
    a.note("news");
    until("the viewer sees news", &mut [&mut a, &mut b, &mut c], |p| p[2].texts().contains(&"news".into()));
    assert!(!a.texts().contains(&"not allowed".into()), "looking only: nothing goes up");

    // A link to edit it.
    let edit = me.make_link(Some(shared), true, "colleague", "Mia").unwrap();
    let mut d = Pc::new(dir, "d", edit.secret.as_ref().unwrap());
    until("the editor has the board", &mut [&mut a, &mut d], |p| p[1].boards().contains(&shared) && p[1].lib().has_media(&hash2));
    d.app.open_board(shared, false);
    d.note("from the colleague");
    d.app.import_image_bytes(&png(50), Some("theirs.png"), 7).unwrap();
    until("the colleague's things reach a", &mut [&mut a, &mut d], |p| p[0].texts().contains(&"from the colleague".into()) && p[0].lib().sidecars().unwrap().len() == 3 && p[0].app.sync_boards().is_some());
    let theirs = a.lib().sidecars().unwrap().into_iter().flatten().find(|s| s.hash != hash && s.hash != hash2).unwrap();
    assert!(a.lib().media_path(&theirs.hash, &theirs.ext).is_file());

    // Revoked: it stops, and says why.
    me.revoke_link(view.id).unwrap();
    until("the viewer is told", &mut [&mut a, &mut c], |p| matches!(p[1].status, Some(Status::Stopped(_))));
    assert_eq!(me.links().unwrap().len(), 1);
    assert!(me.links().unwrap().iter().all(|l| l.secret.is_none()));
    drop(c);

    // An old backup of a library put back: it catches up, and takes nothing back.
    a.app.open_board(home, false);
    a.app.save_if_dirty().unwrap();
    until("quiet", &mut [&mut a, &mut b, &mut d], |p| p.iter().all(|p| p.status == Some(Status::UpToDate)));
    let backup: Vec<_> = b.boards().into_iter().map(|id| (id, std::fs::read(b.root.join(format!("boards/{id}.json"))).unwrap())).collect();
    a.note("after the backup");
    until("b has it", &mut [&mut a, &mut b], |p| p[1].texts().contains(&"after the backup".into()));
    let (root, settings) = (b.root.clone(), b.settings.clone());
    b.app.save_if_dirty().unwrap();
    drop(b);
    std::thread::sleep(Duration::from_secs(2)); // its worker ends
    for (id, bytes) in backup {
        std::fs::write(root.join(format!("boards/{id}.json")), bytes).unwrap();
    }
    std::fs::remove_file(root.join(format!("boards/{shared}.json"))).unwrap();
    let mut b = Pc::open(root, settings, "b", KEY);
    b.app.open_board(home, false);
    assert!(!b.texts().contains(&"after the backup".into()));
    until("the restored library catches up", &mut [&mut a, &mut b], |p| p[1].texts().contains(&"after the backup".into()) && p[1].boards().contains(&shared));
    assert!(a.texts().contains(&"after the backup".into()) && a.boards().contains(&shared), "nothing was lost elsewhere");
    until("shared board whole again", &mut [&mut a, &mut b], |p| {
        let live = |l: Library| l.read_board(shared).unwrap().live_items().count();
        live(p[0].lib()) == live(p[1].lib())
    });

    // A file that isn't what its name says is never kept.
    let fake: ContentHash = "ab".repeat(32).parse().unwrap();
    me.upload(&fake, b"something else").unwrap();
    assert!(matches!(me.download(&fake), Err(CloudError::Other(_))));
    let _ = (&a.key, &a.settings);
}

#[test]
fn a_key_never_goes_to_an_odd_address_and_isnt_printed() {
    for s in ["http://example.com", "ftp://x", "https://a@b.com", "https://x.dev/?a", "http://127.0.0.1.evil.com:80", "javascript:1"] {
        assert!(Client::new(Account { server: s.into(), key: "k".into(), root: None, name: None, color: None }).is_err(), "{s}");
    }
    let a = Account { server: "https://x.dev".into(), key: "sesame".into(), root: None, name: None, color: None };
    assert!(!format!("{a:?}").contains("sesame"));
    let dir = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    Accounts { libraries: [(id, a.clone())].into() }.save(dir.path()).unwrap();
    assert_eq!(Accounts::load(dir.path()).libraries[&id], a);
    // A copy of a synced library (a backup) doesn't sync.
    let lib = Library::create(dir.path().join("L.wall"), "L", 1).unwrap();
    let mut acc = Accounts { libraries: [(lib.manifest().id, Account { root: Some(lib.root().to_owned()), ..a.clone() })].into() };
    assert!(acc.find(&lib).is_some());
    std::fs::create_dir(dir.path().join("Copy.wall")).unwrap();
    std::fs::copy(lib.root().join("wall.json"), dir.path().join("Copy.wall/wall.json")).unwrap();
    assert!(acc.find(&Library::open(dir.path().join("Copy.wall")).unwrap()).is_none());
    acc.libraries.get_mut(&lib.manifest().id).unwrap().server.clear();
    assert!(acc.find(&lib).is_none(), "a key without a server isn't connected");
    let (k1, k2) = (wall_cloud::new_key(), wall_cloud::new_key());
    assert!(k1 != k2 && k1.len() == 96 && wall_cloud::key_hash("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    // No server there: offline, not a lost key.
    let c = Client::new(Account { server: "http://127.0.0.1:9".into(), key: "k".into(), root: None, name: None, color: None }).unwrap();
    assert!(matches!(c.me(), Err(CloudError::Offline(_))));
}
