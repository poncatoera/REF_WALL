//! Keeps a library and its cloud copy the same.
//!
//! [`State`] remembers what the server has seen from here (each item's rev,
//! each board's name and place). [`State::collect`] turns local boards into
//! what still has to go up; [`State::take`] checks a reply and turns it into
//! [`Incoming`] changes for the app to merge (with `Board::merge_item`, so
//! the order things arrive in doesn't matter).
//!
//! What comes down was written by other people: every value is parsed into
//! its type, and sizes, file kinds and ids are checked, before it's used.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use wall_format::{Board, BoardItem, ContentHash, ItemSidecar};

/// Rows in one push (the server takes 500) and roughly its size.
const MAX_ROWS: usize = 400;
const MAX_CHARS: usize = 2_500_000;
const MAX_NAME: usize = 200;
/// Coordinates beyond this are a mistake or a prank, not a board.
const MAX_COORD: f64 = 1e9;

/// What the server knows of one board from here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Base {
    pub meta: Meta,
    /// Item id → (rev, updated_by).
    #[serde(default)]
    pub items: BTreeMap<Uuid, (u64, String)>,
}

/// A board without its items and without what changes on every edit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Uuid>,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl Meta {
    pub fn of(b: &Board) -> Self {
        Meta { id: b.id, name: b.name.clone(), parent: b.parent, created_ms: b.created_ms, deleted: false }
    }
}

/// Saved between runs, outside the library (it's about this computer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The last change pulled.
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub boards: BTreeMap<Uuid, Base>,
    /// Picture and video details as last sent or received.
    #[serde(default)]
    pub sidecars: BTreeMap<ContentHash, Value>,
    /// Files the server has.
    #[serde(default)]
    pub uploaded: BTreeSet<ContentHash>,
    /// Files still to fetch, with their details.
    #[serde(default)]
    pub wanted: BTreeMap<ContentHash, ItemSidecar>,
    /// For a link to one board: the boards it reaches.
    #[serde(default)]
    pub scope: Option<BTreeSet<Uuid>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Push {
    pub since: u64,
    pub boards: Vec<Meta>,
    pub items: Vec<PushItem>,
    pub sidecars: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub known: Vec<Uuid>,
    /// There is more to send after this.
    #[serde(skip)]
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PushItem {
    pub board: Uuid,
    pub item: BoardItem,
}

impl Push {
    pub fn is_empty(&self) -> bool {
        self.boards.is_empty() && self.items.is_empty() && self.sidecars.is_empty()
    }
}

/// The server's answer to a push, as it comes.
#[derive(Debug, Default, Deserialize)]
pub struct Reply {
    pub seq: u64,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub boards: Vec<Value>,
    #[serde(default)]
    pub items: Vec<ReplyItem>,
    #[serde(default)]
    pub sidecars: Vec<Value>,
    #[serde(default)]
    pub refused: Vec<String>,
    #[serde(default)]
    pub scope: Option<BTreeSet<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct ReplyItem {
    pub board: Uuid,
    pub item: Value,
}

/// Checked changes to merge into the app's boards.
#[derive(Debug, Default, PartialEq)]
pub struct Incoming {
    pub boards: Vec<Meta>,
    pub items: Vec<(Uuid, BoardItem)>,
    pub sidecars: Vec<ItemSidecar>,
    /// Boards a link no longer reaches.
    pub out_of_reach: Vec<Uuid>,
    /// Boards a link has come to reach: fetch all their items.
    pub newly_reached: Vec<Uuid>,
    /// Things that couldn't be read (say, made by a newer Wall).
    pub skipped: usize,
}

impl Incoming {
    pub fn is_empty(&self) -> bool {
        *self == Incoming { skipped: self.skipped, ..Default::default() }
    }
}

/// A board's placeholder before anything is known of it.
pub fn new_board(m: &Meta) -> Board {
    let mut b = Board::new(m.name.clone(), m.created_ms);
    b.id = m.id;
    b.parent = m.parent;
    b
}

fn newer(a: (u64, &str), b: (u64, &str)) -> bool {
    a > b
}

impl State {
    /// What must go up from these boards and sidecars (pass at least the ones
    /// that changed), and boards deleted here.
    pub fn collect<'a>(&self, boards: impl IntoIterator<Item = &'a Board>, sidecars: &[ItemSidecar], deleted: &[Uuid]) -> Push {
        self.collect_where(boards, sidecars, deleted, false)
    }

    /// What a view-only link may send: new or changed comments and stamps,
    /// nothing else (no boards, no picture details).
    pub fn collect_marks<'a>(&self, boards: impl IntoIterator<Item = &'a Board>) -> Push {
        self.collect_where(boards, &[], &[], true)
    }

    fn collect_where<'a>(&self, boards: impl IntoIterator<Item = &'a Board>, sidecars: &[ItemSidecar], deleted: &[Uuid], marks_only: bool) -> Push {
        let mut p = Push { since: self.seq, known: self.scope.iter().flatten().copied().collect(), ..Default::default() };
        let mut room = (MAX_ROWS, MAX_CHARS);
        let mut fits = |size: usize, p: &mut Push| {
            if room.0 == 0 || (room.1 < size && room.1 < MAX_CHARS) {
                p.more = true;
                return false;
            }
            room = (room.0 - 1, room.1.saturating_sub(size));
            true
        };
        let boards: Vec<&Board> = boards.into_iter().collect();
        for b in boards.iter().filter(|_| !marks_only) {
            let base = self.boards.get(&b.id);
            let meta = Meta::of(b);
            // A new library's empty first board isn't worth a place in the
            // cloud until something is on it.
            let blank = base.is_none() && b.parent.is_none() && b.items.is_empty();
            if !blank && base.map(|x| &x.meta) != Some(&meta) && fits(200 + meta.name.len(), &mut p) {
                p.boards.push(meta);
            }
        }
        for id in deleted {
            if let Some(base) = self.boards.get(id).filter(|b| !b.meta.deleted) {
                if fits(200, &mut p) {
                    p.boards.push(Meta { deleted: true, ..base.meta.clone() });
                }
            }
        }
        for b in &boards {
            let base = self.boards.get(&b.id);
            for i in b.items.values() {
                if marks_only && !matches!(i.kind, wall_format::ItemKind::Comment { .. } | wall_format::ItemKind::Stamp { .. }) {
                    continue;
                }
                // A board this link can't see (one made here): nowhere to send to.
                if marks_only && base.is_none() {
                    continue;
                }
                let known = base.and_then(|x| x.items.get(&i.id));
                if known.is_some_and(|(rev, by)| !newer((i.rev, &i.updated_by), (*rev, by))) {
                    continue;
                }
                // Previews that were never committed have no rev yet.
                if i.rev == 0 {
                    continue;
                }
                let size = serde_json::to_string(i).map_or(0, |s| s.len());
                if fits(size, &mut p) {
                    p.items.push(PushItem { board: b.id, item: i.clone() });
                }
            }
        }
        for s in sidecars {
            let Ok(v) = serde_json::to_value(s) else { continue };
            if self.sidecars.get(&s.hash) != Some(&v) && fits(v.to_string().len(), &mut p) {
                p.sidecars.push(v);
            }
        }
        p
    }

    /// True if these boards are older than what went up from here before (a
    /// backup was put back): pull everything again rather than miss changes.
    pub fn looks_restored<'a>(&self, boards: impl IntoIterator<Item = &'a Board>) -> bool {
        boards.into_iter().any(|b| {
            self.boards.get(&b.id).is_some_and(|base| {
                base.items.iter().any(|(id, (rev, _))| b.items.get(id).is_none_or(|i| i.rev < *rev))
            })
        })
    }

    /// Records that `push` got through and checks what came back.
    pub fn take(&mut self, push: &Push, reply: Reply) -> Incoming {
        let refused: BTreeSet<&str> = reply.refused.iter().map(String::as_str).collect();
        for m in &push.boards {
            if !refused.contains(m.id.to_string().as_str()) {
                self.boards.entry(m.id).or_default().meta = m.clone();
            }
        }
        // Refused things are noted too: sending them again won't help.
        for e in &push.items {
            self.boards.entry(e.board).or_default().items.insert(e.item.id, (e.item.rev, e.item.updated_by.clone()));
        }
        for v in &push.sidecars {
            if let Ok(s) = serde_json::from_value::<ItemSidecar>(v.clone()) {
                self.sidecars.insert(s.hash, v.clone());
            }
        }
        let mut inc = Incoming::default();
        for v in reply.boards {
            match serde_json::from_value::<Meta>(v) {
                Ok(mut m) if m.parent != Some(m.id) => {
                    m.name = m.name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
                    inc.boards.push(m);
                }
                _ => inc.skipped += 1,
            }
        }
        self.take_items(reply.items, reply.sidecars, &mut inc);
        if let Some(now) = reply.scope {
            let was = self.scope.take().unwrap_or_default();
            inc.newly_reached = now.difference(&was).copied().collect();
            inc.out_of_reach = was.difference(&now).copied().collect();
            for b in &inc.out_of_reach {
                self.boards.remove(b);
            }
            self.scope = Some(now);
        }
        self.seq = self.seq.max(reply.seq);
        inc
    }

    /// Items and details from a reply (or from a board fetched whole).
    pub fn take_items(&mut self, items: Vec<ReplyItem>, sidecars: Vec<Value>, inc: &mut Incoming) {
        for e in items {
            match serde_json::from_value::<BoardItem>(e.item) {
                Ok(i) if sane(&i) => inc.items.push((e.board, i)),
                _ => inc.skipped += 1,
            }
        }
        for v in sidecars {
            match serde_json::from_value::<ItemSidecar>(v) {
                Ok(s) if known_kind(&s.ext) => inc.sidecars.push(s),
                _ => inc.skipped += 1,
            }
        }
    }

    /// Should a board's name and place come from the server? Not while a
    /// change made here is still on its way up.
    pub fn takes_meta(&self, remote: &Meta, local: Option<&Meta>) -> bool {
        match (local, self.boards.get(&remote.id)) {
            (Some(l), Some(base)) => *l == base.meta || l == remote,
            (Some(l), None) => l.name == remote.name && l.parent == remote.parent,
            (None, _) => true,
        }
    }

    /// Records that the app merged `inc`, so it isn't sent back.
    pub fn merged(&mut self, inc: &Incoming, kept_meta: &BTreeSet<Uuid>) {
        for m in &inc.boards {
            if !kept_meta.contains(&m.id) {
                self.boards.entry(m.id).or_default().meta = m.clone();
            }
        }
        for (b, i) in &inc.items {
            let items = &mut self.boards.entry(*b).or_default().items;
            let old = items.get(&i.id).map(|(r, by)| (*r, by.as_str()));
            if old.is_none_or(|o| newer((i.rev, &i.updated_by), o)) {
                items.insert(i.id, (i.rev, i.updated_by.clone()));
            }
        }
        for s in &inc.sidecars {
            if let Ok(v) = serde_json::to_value(s) {
                self.sidecars.insert(s.hash.clone(), v);
            }
        }
    }
}

fn known_kind(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif" | "mp4" | "webm" | "mov" | "m4v" | "mkv" | "avi")
}

/// A position and size that can be drawn.
pub fn sane(i: &BoardItem) -> bool {
    let f = &i.frame;
    [f.x, f.y, f.w, f.h, f.z].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) && f.w >= 0.0 && f.h >= 0.0 && i.rev > 0 && i.rev < (1 << 52) && i.updated_by.len() <= 200
}

#[cfg(test)]
mod tests {
    use super::*;
    use wall_format::{Frame, ItemKind};

    fn note(text: &str, rev: u64, by: &str) -> BoardItem {
        BoardItem {
            id: Uuid::new_v4(),
            frame: Frame { w: 10.0, h: 10.0, ..Default::default() },
            kind: ItemKind::Note { text: text.into(), color: None },
            rev,
            updated_ms: 1,
            updated_by: by.into(),
            deleted: false,
        }
    }

    fn board(name: &str, items: Vec<BoardItem>) -> Board {
        let mut b = Board::new(name, 1);
        b.items = items.into_iter().map(|i| (i.id, i)).collect();
        b
    }

    /// The server's rules, in a few lines: enough to check two copies agree.
    #[derive(Default)]
    struct Server {
        seq: u64,
        boards: BTreeMap<Uuid, (u64, Value)>,
        items: BTreeMap<(Uuid, Uuid), (u64, BoardItem)>,
    }

    impl Server {
        fn sync(&mut self, p: &Push) -> Reply {
            for m in &p.boards {
                self.seq += 1;
                self.boards.insert(m.id, (self.seq, serde_json::to_value(m).unwrap()));
            }
            for e in &p.items {
                let old = self.items.get(&(e.board, e.item.id));
                if old.is_none_or(|(_, o)| e.item.supersedes(o)) {
                    self.seq += 1;
                    self.items.insert((e.board, e.item.id), (self.seq, e.item.clone()));
                }
            }
            Reply {
                seq: self.seq,
                boards: self.boards.values().filter(|b| b.0 > p.since).map(|b| b.1.clone()).collect(),
                items: self.items.iter().filter(|(_, i)| i.0 > p.since).map(|((b, _), i)| ReplyItem { board: *b, item: serde_json::to_value(&i.1).unwrap() }).collect(),
                ..Default::default()
            }
        }
    }

    /// One computer: its boards and its sync state.
    #[derive(Default)]
    struct Copy {
        boards: BTreeMap<Uuid, Board>,
        state: State,
    }

    impl Copy {
        fn go(&mut self, s: &mut Server) -> usize {
            let push = self.state.collect(self.boards.values(), &[], &[]);
            let sent = push.boards.len() + push.items.len();
            let reply = s.sync(&push);
            let inc = self.state.take(&push, reply);
            let mut kept = BTreeSet::new();
            for m in &inc.boards {
                let local = self.boards.get(&m.id).map(Meta::of);
                if !self.state.takes_meta(m, local.as_ref()) {
                    kept.insert(m.id);
                    continue;
                }
                let b = self.boards.entry(m.id).or_insert_with(|| new_board(m));
                b.name = m.name.clone();
                b.parent = m.parent;
            }
            for (b, i) in &inc.items {
                if let Some(b) = self.boards.get_mut(b) {
                    b.merge_item(i.clone());
                }
            }
            self.state.merged(&inc, &kept);
            sent
        }

        fn same(&self, other: &Copy) -> bool {
            let key = |c: &Copy| c.boards.values().filter(|b| !b.items.is_empty() || b.parent.is_some()).map(|b| (b.id, b.name.clone(), b.parent, b.items.clone())).collect::<Vec<_>>();
            key(self) == key(other)
        }
    }

    #[test]
    fn two_copies_end_up_the_same_and_then_go_quiet() {
        let (mut s, mut a, mut b) = (Server::default(), Copy::default(), Copy::default());
        let home = board("Home", vec![note("one", 1, "a"), note("two", 1, "a")]);
        let id = home.id;
        a.boards.insert(id, home);
        // The other computer starts with an empty first board, which stays there.
        let blank = board("Board 1", vec![]);
        b.boards.insert(blank.id, blank);
        assert_eq!(a.go(&mut s), 3);
        assert_eq!(b.go(&mut s), 0);
        assert!(a.same(&b));
        // Both edit the same note, and one adds and one deletes another.
        let ids: Vec<Uuid> = a.boards[&id].items.keys().copied().collect();
        let edit = |c: &mut Copy, k: usize, text: &str, by: &str| {
            let i = c.boards.get_mut(&id).unwrap().items.get_mut(&ids[k]).unwrap();
            i.kind = ItemKind::Note { text: text.into(), color: None };
            i.rev += 1;
            i.updated_by = by.into();
        };
        edit(&mut a, 0, "from a", "a");
        edit(&mut b, 0, "from b", "b");
        b.boards.get_mut(&id).unwrap().items.get_mut(&ids[1]).unwrap().deleted = true;
        edit(&mut b, 1, "gone", "b");
        let extra = note("new", 1, "a");
        a.boards.get_mut(&id).unwrap().items.insert(extra.id, extra);
        for _ in 0..2 {
            a.go(&mut s);
            b.go(&mut s);
        }
        assert!(a.same(&b));
        assert_eq!(a.boards[&id].live_items().count(), 2);
        assert!(matches!(&a.boards[&id].items[&ids[0]].kind, ItemKind::Note { text, .. } if text == "from b"), "a tie goes the same way everywhere");
        assert_eq!((a.go(&mut s), b.go(&mut s)), (0, 0), "nothing to say any more");
    }

    #[test]
    fn a_view_link_sends_only_its_comments_and_stamps() {
        let (mut s, mut a, mut b) = (Server::default(), Copy::default(), Copy::default());
        let home = board("Home", vec![note("x", 1, "a")]);
        let id = home.id;
        a.boards.insert(id, home);
        a.go(&mut s);
        b.go(&mut s);
        let bb = b.boards.get_mut(&id).unwrap();
        bb.name = "renamed".into();
        bb.items.values_mut().next().unwrap().rev = 9;
        let mut c = note("c", 1, "b");
        c.kind = wall_format::ItemKind::Comment { messages: vec![wall_format::CommentMessage { by: "Mia".into(), text: "hi".into(), at_ms: 1 }], resolved: false };
        bb.items.insert(c.id, c.clone());
        let mut elsewhere = board("Mine", vec![]);
        let mut st = note("s", 1, "b");
        st.kind = wall_format::ItemKind::Stamp { mark: "★".into(), by: String::new() };
        elsewhere.items.insert(st.id, st);
        b.boards.insert(elsewhere.id, elsewhere);
        let p = b.state.collect_marks(b.boards.values());
        assert!(p.boards.is_empty() && p.sidecars.is_empty());
        assert_eq!(p.items.iter().map(|i| i.item.id).collect::<Vec<_>>(), vec![c.id], "the comment, not the note, not a board the server lacks");
    }

    #[test]
    fn a_rename_here_isnt_undone_by_an_older_name_from_there() {
        let (mut s, mut a, mut b) = (Server::default(), Copy::default(), Copy::default());
        let home = board("Home", vec![note("x", 1, "a")]);
        let id = home.id;
        a.boards.insert(id, home);
        a.go(&mut s);
        b.go(&mut s);
        a.boards.get_mut(&id).unwrap().name = "Refs".into();
        // b's item comes down with nothing about the name changed.
        let n = note("y", 1, "b");
        b.boards.get_mut(&id).unwrap().items.insert(n.id, n);
        b.go(&mut s);
        a.go(&mut s);
        b.go(&mut s);
        assert_eq!((a.boards[&id].name.as_str(), b.boards[&id].name.as_str()), ("Refs", "Refs"));
    }

    #[test]
    fn a_big_board_goes_up_in_parts_and_a_restored_backup_is_noticed() {
        let mut s = Server::default();
        let mut a = Copy::default();
        let home = board("Home", (0..1000).map(|k| note(&format!("{k}"), 2, "a")).collect());
        let id = home.id;
        a.boards.insert(id, home);
        let mut rounds = 0;
        loop {
            let more = a.state.collect(a.boards.values(), &[], &[]).more;
            a.go(&mut s);
            rounds += 1;
            if !more {
                break;
            }
        }
        assert_eq!((rounds, s.items.len()), (3, 1000));
        assert!(!a.state.looks_restored(a.boards.values()));
        a.boards.get_mut(&id).unwrap().items.values_mut().next().unwrap().rev = 1;
        assert!(a.state.looks_restored(a.boards.values()));
    }

    #[test]
    fn what_others_wrote_is_checked_before_its_used() {
        let mut st = State::default();
        let b = Uuid::new_v4();
        let good = serde_json::to_value(note("fine", 1, "x")).unwrap();
        let mut bad = Vec::new();
        for (path, v) in [("/frame/x", serde_json::json!(1e300)), ("/frame/w", serde_json::json!(-5)), ("/rev", serde_json::json!(0)), ("/rev", serde_json::json!(u64::MAX)), ("/id", serde_json::json!("../x")), ("/updated_by", serde_json::json!("x".repeat(5000)))] {
            let mut i = good.clone();
            *i.pointer_mut(path).unwrap() = v;
            bad.push(i);
        }
        let mut media = good.clone();
        media["type"] = "media".into();
        media["hash"] = "../../etc".into();
        bad.push(media);
        let n = bad.len();
        let side = |ext: &str| serde_json::json!({"hash": "a".repeat(64), "kind": "image", "ext": ext, "width": 1, "height": 1, "bytes": 1, "added_ms": 1, "tags": []});
        let reply = Reply {
            seq: 9,
            boards: vec![serde_json::json!({"id": b, "name": format!("A\u{0}\n{}", "n".repeat(900))}), serde_json::json!({"id": b, "name": "self", "parent": b}), serde_json::json!({"id": "x", "name": "y"})],
            items: bad.into_iter().chain([good]).map(|item| ReplyItem { board: b, item }).collect(),
            sidecars: vec![side("png"), side("exe"), side("../x"), serde_json::json!({"hash": "zz"})],
            ..Default::default()
        };
        let inc = st.take(&Push::default(), reply);
        assert_eq!((inc.boards.len(), inc.items.len(), inc.sidecars.len()), (1, 1, 1));
        assert_eq!(inc.skipped, n + 2 + 3);
        assert!(inc.boards[0].name.len() <= MAX_NAME && inc.boards[0].name.starts_with("An"));
    }

    #[test]
    fn deletions_are_sent_once_and_only_for_boards_the_server_has() {
        let mut st = State::default();
        let b = board("x", vec![note("n", 1, "a")]);
        assert!(st.collect([], &[], &[b.id]).is_empty());
        let p = st.collect([&b], &[], &[]);
        st.take(&p, Reply::default());
        let p = st.collect([], &[], &[b.id]);
        assert!(p.boards.len() == 1 && p.boards[0].deleted);
        st.take(&p, Reply::default());
        assert!(st.collect([], &[], &[b.id]).is_empty());
    }

    #[test]
    fn a_link_learns_which_boards_come_and_go() {
        let mut st = State::default();
        let (x, y) = (Uuid::new_v4(), Uuid::new_v4());
        let inc = st.take(&Push::default(), Reply { scope: Some([x].into()), ..Default::default() });
        assert_eq!((inc.newly_reached, inc.out_of_reach), (vec![x], vec![]));
        let p = st.collect([], &[], &[]);
        assert_eq!(p.known, vec![x]);
        let inc = st.take(&p, Reply { scope: Some([y].into()), ..Default::default() });
        assert_eq!((inc.newly_reached, inc.out_of_reach), (vec![y], vec![x]));
    }
}
