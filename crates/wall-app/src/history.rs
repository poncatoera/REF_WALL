//! Version history (File > Version history…): earlier versions of the open
//! board, to look at and bring back, and boards that were deleted.
//!
//! With a library folder the app keeps and reads versions itself (see
//! `wall_format::history`). Without one (the web), it asks the shell with
//! [`ShellCommand::History`] and the shell answers with
//! [`App::history_answer`], the same way the share card works.
//!
//! Looking at a version never changes the board: the version is drawn in
//! place of the board (see [`App::render`]) while every edit is held off.
//! Bringing it back is an ordinary edit of the items that differ, so it
//! syncs like any change and Undo takes it back.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;
use wall_format::{Board, BoardItem, Frame, ItemKind, VERSION_EVERY_MS};
use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::menu::{ShellCommand, BAR};
use crate::panel::{self, PanelBox, TextRun};
use crate::{scene, theme, App, Button, Document, Gesture, Key};

/// What the app asks the shell, when it has no library folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAsk {
    /// When the versions of a board were made.
    Versions { board: Uuid },
    /// One version of a board.
    Version { board: Uuid, at_ms: u64 },
    /// Boards that were deleted.
    Deleted,
    /// Bring a deleted board (and the boards inside it) back.
    Restore { board: Uuid },
}

/// A deleted board, as the history card lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedBoard {
    pub id: Uuid,
    pub name: String,
    pub parent: Option<Uuid>,
    pub deleted_ms: u64,
    /// Things on it.
    pub items: usize,
    /// Boards inside it that were deleted with it (they come back with it).
    pub inside: usize,
}

/// The shell's answer to a [`HistoryAsk`].
#[derive(Debug, Clone)]
pub enum HistoryAnswer {
    Versions { board: Uuid, times: Vec<u64> },
    Version { board: Uuid, at_ms: u64, version: Board },
    Deleted(Vec<DeletedBoard>),
    /// The board is back; it arrives with the next sync.
    Restored(Uuid),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Versions,
    Deleted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Tab(Tab),
    Now,
    Version(u64),
    Restore,
    Copy,
    RestoreBoard(Uuid),
    Close,
}

/// The app's history state.
#[derive(Default)]
pub(crate) struct History {
    /// When a version of each board was last kept (this run).
    kept_at: HashMap<Uuid, u64>,
    card: Option<Card>,
    /// Minutes to add to UTC for local time (shells set it).
    pub utc_offset_min: i32,
    /// The shell answers [`HistoryAsk`]s (a web page with a cloud link).
    pub remote: bool,
    /// Bumped for every version shown, so the scene redraws it.
    shown: u64,
}

struct Card {
    tab: Tab,
    board: Uuid,
    /// None until known.
    versions: Option<Vec<u64>>,
    deleted: Option<Vec<DeletedBoard>>,
    /// The version looked at, and (once it's here) its board.
    selected: Option<u64>,
    preview: Option<Document>,
    /// Rows scrolled past at the top of the list.
    scroll: usize,
    note: Option<String>,
}

impl History {
    pub fn previewing(&self) -> bool {
        self.card.as_ref().is_some_and(|c| c.selected.is_some())
    }
}

const W: f32 = 320.0;
const PAD: f32 = 16.0;
const ROW: f32 = 30.0;
const HEAD: f32 = 26.0;
const DAY: i64 = 86_400_000;
/// Scene versions of shown versions start here, far from any real board's.
const PREVIEW_VERSION: u64 = 1 << 60;

/// The changes that turn `now` into `then`: items that differ come back as
/// they were, items added since are deleted. Ordinary edits, so they sync.
pub fn changes_to(now: &Board, then: &Board) -> Vec<BoardItem> {
    let mut out = Vec::new();
    for old in then.live_items() {
        match now.items.get(&old.id) {
            Some(cur) if !cur.deleted && cur.kind == old.kind && cur.frame == old.frame => {}
            _ => out.push(BoardItem { deleted: false, ..old.clone() }),
        }
    }
    for cur in now.live_items() {
        if then.items.get(&cur.id).is_none_or(|o| o.deleted) {
            out.push(BoardItem { deleted: true, ..cur.clone() });
        }
    }
    out
}

/// Local day number, hour and minute of a time.
fn local(ms: u64, offset_min: i32) -> (i64, u32, u32) {
    let t = ms as i64 + offset_min as i64 * 60_000;
    let day = t.div_euclid(DAY);
    let in_day = t.rem_euclid(DAY) / 60_000;
    (day, (in_day / 60) as u32, (in_day % 60) as u32)
}

/// "Today", "Yesterday", or "Sat 21 Nov" (with the year when it isn't this one).
fn day_name(day: i64, today: i64) -> String {
    if day == today {
        return "Today".into();
    }
    if day == today - 1 {
        return "Yesterday".into();
    }
    const WEEK: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTH: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let date = panel::format_date((day * DAY) as u64);
    let (y, m, d) = (&date[0..4], date[5..7].parse::<usize>().unwrap_or(1), date[8..10].parse::<u32>().unwrap_or(1));
    let this_year = &panel::format_date((today * DAY) as u64)[0..4];
    let mut s = format!("{} {d} {}", WEEK[day.rem_euclid(7) as usize], MONTH[(m - 1).min(11)]);
    if y != this_year {
        s.push(' ');
        s.push_str(y);
    }
    s
}

/// "Today 14:05", "Sat 21 Nov 09:30".
pub fn when(ms: u64, now_ms: u64, offset_min: i32) -> String {
    let (day, h, m) = local(ms, offset_min);
    let today = local(now_ms, offset_min).0;
    format!("{} {h:02}:{m:02}", day_name(day, today))
}

#[derive(Default)]
pub struct Layout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Hit)>,
    /// The list's box, for the wheel.
    pub list: [f32; 4],
    /// Rows that fit in the list.
    pub fits: usize,
    /// A strip over the board while a version is shown.
    pub banner: Option<([f32; 4], String)>,
}

struct Inputs<'a> {
    card: &'a Card,
    board_name: &'a str,
    items_now: usize,
    now_ms: u64,
    offset: i32,
    can_restore_boards: bool,
}

fn layout(i: &Inputs, hover: [f32; 2], viewport: [f32; 2], tw: &mut dyn FnMut(&str, f32) -> f32) -> Layout {
    let t = theme::t();
    let c = i.card;
    let mut l = Layout::default();
    let x0 = (viewport[0] - W - 12.0).max(8.0);
    let y0 = BAR + 12.0;
    let inner = W - 2.0 * PAD;
    let bottom = viewport[1] - 12.0;
    let mut texts = Vec::new();
    let mut boxes = Vec::new();
    let mut hits = Vec::new();
    let mut y = y0 + PAD + 14.0;
    texts.push(TextRun { text: "History".into(), at: [x0 + PAD, y], size: 16.0, color: t.ink });
    let close = [x0 + W - PAD - 22.0, y0 + 10.0, 22.0, 22.0];
    texts.push(TextRun { text: "×".into(), at: [close[0] + 5.0, close[1] + 16.0], size: 18.0, color: t.dim });
    hits.push((close, Hit::Close));
    y += 14.0;
    // Two tabs.
    let tabs = [(Tab::Versions, "This board"), (Tab::Deleted, "Deleted boards")];
    let tab_w = inner / 2.0;
    for (k, (tab, label)) in tabs.iter().enumerate() {
        let r = [x0 + PAD + k as f32 * tab_w, y, tab_w, 28.0];
        let on = c.tab == *tab;
        boxes.push(PanelBox { rect: [r[0], r[1] + r[3] - 2.0, r[2], 2.0], fill: if on { t.accent } else { t.line }, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
        let w = tw(label, 12.5);
        texts.push(TextRun { text: (*label).into(), at: [r[0] + (r[2] - w) / 2.0, r[1] + 18.0], size: 12.5, color: if on { t.ink } else { t.dim } });
        hits.push((r, Hit::Tab(*tab)));
    }
    y += 28.0 + 8.0;
    let wrap = |tw: &mut dyn FnMut(&str, f32) -> f32, texts: &mut Vec<TextRun>, y: &mut f32, s: &str, color: [f32; 4]| {
        for line in panel::wrap_with(tw, s, 12.0, inner) {
            texts.push(TextRun { text: line, at: [x0 + PAD, *y + 12.0], size: 12.0, color });
            *y += 17.0;
        }
    };
    // What goes under the list: a note, and the buttons for a shown version.
    let footer = |selected: bool, note: bool| -> f32 { (if selected { 52.0 + 40.0 } else { 0.0 }) + if note { 40.0 } else { 0.0 } };
    match c.tab {
        Tab::Versions => {
            let Some(versions) = &c.versions else {
                wrap(tw, &mut texts, &mut y, "Looking…", t.dim);
                return finish(l, x0, y0, y, boxes, texts, hits, hover, tw);
            };
            if versions.is_empty() {
                wrap(tw, &mut texts, &mut y, &format!("No earlier versions of “{}” yet. While a board changes, Wall keeps a version every {} minutes.", i.board_name, VERSION_EVERY_MS / 60_000), t.dim);
            }
            // Rows: Now, then versions under a heading per day.
            enum Row {
                Head(String),
                Now,
                At(u64),
            }
            let today = local(i.now_ms, i.offset).0;
            let mut rows = vec![Row::Now];
            let mut last_day = None;
            for at in versions {
                let (day, _, _) = local(*at, i.offset);
                if last_day != Some(day) {
                    rows.push(Row::Head(day_name(day, today).to_uppercase()));
                    last_day = Some(day);
                }
                rows.push(Row::At(*at));
            }
            let room = (bottom - PAD - footer(c.selected.is_some(), c.note.is_some()) - y).max(ROW * 3.0);
            let fits = (room / ROW).floor().max(1.0) as usize;
            l.fits = fits;
            l.list = [x0, y, W, fits as f32 * ROW];
            let scroll = c.scroll.min(rows.len().saturating_sub(fits));
            for row in rows.iter().skip(scroll).take(fits) {
                match row {
                    Row::Head(s) => {
                        texts.push(TextRun { text: s.clone(), at: [x0 + PAD, y + HEAD - 8.0], size: 10.5, color: t.dim });
                        y += HEAD;
                    }
                    Row::Now | Row::At(_) => {
                        let r = [x0 + 6.0, y, W - 12.0, ROW];
                        let on = match row {
                            Row::Now => c.selected.is_none(),
                            Row::At(at) => c.selected == Some(*at),
                            Row::Head(_) => false,
                        };
                        if on {
                            boxes.push(PanelBox { rect: r, fill: t.field, stroke: t.accent, radius: 6.0, stroke_w: 1.0 });
                        } else if panel::inside(r, hover) {
                            boxes.push(PanelBox { rect: r, fill: t.field, stroke: [0.0; 4], radius: 6.0, stroke_w: 0.0 });
                        }
                        let (label, sub, hit) = match row {
                            Row::Now => ("Now".to_owned(), format!("{} things", i.items_now), Hit::Now),
                            Row::At(at) => {
                                let (_, h, m) = local(*at, i.offset);
                                (format!("{h:02}:{m:02}"), String::new(), Hit::Version(*at))
                            }
                            Row::Head(_) => unreachable!(),
                        };
                        texts.push(TextRun { text: label.clone(), at: [r[0] + 10.0, y + 19.5], size: 13.0, color: t.ink });
                        if !sub.is_empty() {
                            let x = r[0] + 10.0 + tw(&label, 13.0) + 8.0;
                            texts.push(TextRun { text: sub, at: [x, y + 19.5], size: 12.0, color: t.dim });
                        }
                        hits.push((r, hit));
                        y += ROW;
                    }
                }
            }
            if rows.len() > scroll + fits {
                texts.push(TextRun { text: format!("{} more, scroll for older", rows.len() - scroll - fits), at: [x0 + PAD, y + 14.0], size: 11.0, color: t.dim });
                y += 20.0;
            }
            if let Some(at) = c.selected {
                y += 8.0;
                let shown = lower_first(&when(at, i.now_ms, i.offset));
                if c.preview.is_none() {
                    wrap(tw, &mut texts, &mut y, &format!("Fetching {shown}…"), t.dim);
                } else {
                    wrap(tw, &mut texts, &mut y, &format!("Showing {shown}. Nothing changes until you restore it. Undo takes a restore back."), t.dim);
                    y += 6.0;
                    let bw = (inner - 10.0) / 2.0;
                    l.hits.push(([x0 + PAD, y, bw, 32.0], Hit::Restore));
                    l.hits.push(([x0 + PAD + bw + 10.0, y, bw, 32.0], Hit::Copy));
                    y += 32.0;
                }
                l.banner = Some(([0.0; 4], format!("Looking at “{}” as it was {shown}  ·  Esc goes back to now  ·  ← → older and newer", i.board_name)));
            }
        }
        Tab::Deleted => {
            let Some(deleted) = &c.deleted else {
                wrap(tw, &mut texts, &mut y, "Looking…", t.dim);
                return finish(l, x0, y0, y, boxes, texts, hits, hover, tw);
            };
            if deleted.is_empty() {
                wrap(tw, &mut texts, &mut y, "No deleted boards. A board you delete waits here, with the boards inside it, until you bring it back.", t.dim);
            }
            let row_h = 46.0;
            let room = (bottom - PAD - footer(false, c.note.is_some()) - y).max(row_h * 2.0);
            let fits = (room / row_h).floor().max(1.0) as usize;
            l.fits = fits;
            l.list = [x0, y, W, fits as f32 * row_h];
            let scroll = c.scroll.min(deleted.len().saturating_sub(fits));
            for d in deleted.iter().skip(scroll).take(fits) {
                let r = [x0 + 6.0, y, W - 12.0, row_h];
                if panel::inside(r, hover) {
                    boxes.push(PanelBox { rect: r, fill: t.field, stroke: [0.0; 4], radius: 6.0, stroke_w: 0.0 });
                }
                let name = panel::fit_with(tw, &d.name, 13.0, inner - 90.0);
                texts.push(TextRun { text: name, at: [r[0] + 10.0, y + 19.0], size: 13.0, color: t.ink });
                let things = match d.items {
                    0 => "empty".to_owned(),
                    1 => "1 thing".to_owned(),
                    n => format!("{n} things"),
                };
                let deleted = if d.deleted_ms == 0 { "Deleted earlier".to_owned() } else { format!("Deleted {}", lower_first(&when(d.deleted_ms, i.now_ms, i.offset))) };
                let mut sub = format!("{deleted}  ·  {things}");
                if d.inside > 0 {
                    sub.push_str(&format!("  ·  {} inside", d.inside));
                }
                texts.push(TextRun { text: panel::fit_with(tw, &sub, 11.0, inner - 90.0), at: [r[0] + 10.0, y + 36.0], size: 11.0, color: t.dim });
                if i.can_restore_boards {
                    l.hits.push(([x0 + W - PAD - 76.0, y + 9.0, 76.0, 28.0], Hit::RestoreBoard(d.id)));
                }
                y += row_h;
            }
            if deleted.len() > scroll + fits {
                texts.push(TextRun { text: format!("{} more, scroll down", deleted.len() - scroll - fits), at: [x0 + PAD, y + 14.0], size: 11.0, color: t.dim });
                y += 20.0;
            }
        }
    }
    if let Some(n) = &c.note {
        y += 6.0;
        wrap(tw, &mut texts, &mut y, n, t.ink);
    }
    finish(l, x0, y0, y, boxes, texts, hits, hover, tw)
}

fn lower_first(s: &str) -> String {
    // "Today 14:05" reads "today 14:05" mid-sentence; "Sat 21 Nov" stays.
    match s.split_once(' ') {
        Some(("Today" | "Yesterday", rest)) => format!("{} {rest}", s.split(' ').next().unwrap_or_default().to_lowercase()),
        _ => s.to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(mut l: Layout, x0: f32, y0: f32, y: f32, boxes: Vec<PanelBox>, texts: Vec<TextRun>, hits: Vec<([f32; 4], Hit)>, hover: [f32; 2], tw: &mut dyn FnMut(&str, f32) -> f32) -> Layout {
    let t = theme::t();
    l.rect = [x0, y0, W, y + PAD - y0];
    l.boxes.push(PanelBox { rect: l.rect, fill: t.panel_bg, stroke: t.line, radius: 12.0, stroke_w: 1.0 });
    l.boxes.extend(boxes);
    l.texts = texts;
    // Buttons were put in `l.hits` while laying out; draw them now.
    let buttons: Vec<([f32; 4], Hit)> = std::mem::take(&mut l.hits);
    l.hits = hits;
    for (r, hit) in buttons {
        let (label, primary) = match &hit {
            Hit::Restore => ("Restore this version", true),
            Hit::Copy => ("Copy as new board", false),
            Hit::RestoreBoard(_) => ("Restore", false),
            _ => ("", false),
        };
        let over = panel::inside(r, hover);
        let (fill, stroke, ink) = if primary { (t.accent, [0.0; 4], t.panel_bg) } else { (if over { t.field } else { [0.0; 4] }, t.line, t.ink) };
        l.boxes.push(PanelBox { rect: r, fill, stroke, radius: 7.0, stroke_w: if primary { 0.0 } else { 1.0 } });
        let w = tw(label, 12.5);
        l.texts.push(TextRun { text: label.into(), at: [r[0] + (r[2] - w) / 2.0, r[1] + r[3] / 2.0 + 4.5], size: 12.5, color: ink });
        l.hits.push((r, hit));
    }
    l
}

impl App {
    // ---- keeping versions --------------------------------------------------

    /// Before a board's file is overwritten: keep it as it is on disk as a
    /// version, if the last one kept is [`VERSION_EVERY_MS`] old.
    pub(crate) fn keep_history(&mut self, board: Uuid) {
        let Some(lib) = &self.library else { return };
        let now = self.now_ms;
        if self.history.kept_at.get(&board).is_some_and(|t| now.saturating_sub(*t) < VERSION_EVERY_MS) {
            return;
        }
        match lib.keep_saved_version(board, now) {
            Ok(true) => {
                self.history.kept_at.insert(board, now);
            }
            // Nothing worth keeping yet (a new, empty board): look again
            // at the next save.
            Ok(false) => {}
            Err(e) => eprintln!("wall: could not keep a version of board {board}: {e}"),
        }
    }

    /// Minutes to add to UTC for the person's local time, for the dates
    /// the history card shows.
    pub fn set_utc_offset(&mut self, minutes: i32) {
        self.history.utc_offset_min = minutes;
    }

    /// The shell can answer [`HistoryAsk`]s (a page without a library).
    pub fn set_history_remote(&mut self, remote: bool) {
        self.history.remote = remote;
    }

    /// A version is shown in place of the board: edits are held off.
    pub fn history_previewing(&self) -> bool {
        self.history.previewing()
    }

    pub fn history_open(&self) -> bool {
        self.history.card.is_some()
    }

    // ---- the card ----------------------------------------------------------

    /// Opens the history card on a tab and asks for what it lists.
    pub fn open_history(&mut self, tab: Tab) {
        self.end_edit();
        self.share = None;
        let board = self.doc.board.id;
        self.history.card = Some(Card { tab, board, versions: None, deleted: None, selected: None, preview: None, scroll: 0, note: None });
        self.needs_redraw = true;
        self.history_ask(match tab {
            Tab::Versions => HistoryAsk::Versions { board },
            Tab::Deleted => HistoryAsk::Deleted,
        });
    }

    pub fn close_history(&mut self) {
        if self.history.card.take().is_some_and(|c| c.preview.is_some()) {
            self.scene.invalidate();
        }
        self.needs_redraw = true;
    }

    /// Answers a question here when there's a library, else asks the shell.
    fn history_ask(&mut self, ask: HistoryAsk) {
        if self.library.is_none() {
            if self.history.remote {
                self.shell_commands.push(ShellCommand::History(ask));
            } else {
                self.history_answer(HistoryAnswer::Failed("History isn't kept for boards saved only in this browser.".into()));
            }
            return;
        }
        let answer = self.history_local(&ask);
        self.history_answer(answer);
    }

    fn history_local(&mut self, ask: &HistoryAsk) -> HistoryAnswer {
        let Some(lib) = &self.library else { return HistoryAnswer::Failed("no library".into()) };
        let r = match *ask {
            HistoryAsk::Versions { board } => lib.versions(board).map(|times| HistoryAnswer::Versions { board, times }),
            HistoryAsk::Version { board, at_ms } => lib.read_version(board, at_ms).map(|version| HistoryAnswer::Version { board, at_ms, version }),
            HistoryAsk::Deleted => lib.trashed_boards().map(|t| HistoryAnswer::Deleted(deleted_list(&t))),
            HistoryAsk::Restore { board } => return self.restore_board_here(board),
        };
        r.unwrap_or_else(|e| HistoryAnswer::Failed(e.to_string()))
    }

    /// The shell's answer to a [`HistoryAsk`] (or the app's own).
    pub fn history_answer(&mut self, answer: HistoryAnswer) {
        self.needs_redraw = true;
        let Some(c) = &mut self.history.card else { return };
        match answer {
            HistoryAnswer::Versions { board, times } if board == c.board => {
                c.versions = Some(times);
            }
            HistoryAnswer::Version { board, at_ms, version } if board == c.board && c.selected == Some(at_ms) => {
                self.history.shown += 1;
                let mut doc = Document::new(version, self.doc.device.clone());
                doc.version = PREVIEW_VERSION + self.history.shown;
                c.preview = Some(doc);
                self.scene.invalidate();
            }
            HistoryAnswer::Deleted(list) => c.deleted = Some(list),
            HistoryAnswer::Restored(id) => {
                let name = c.deleted.iter().flatten().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_else(|| "The board".into());
                c.note = Some(format!("“{name}” is back."));
                if let Some(list) = &mut c.deleted {
                    list.retain(|d| d.id != id);
                }
            }
            HistoryAnswer::Failed(e) => {
                c.note = Some(e);
                if c.versions.is_none() && c.tab == Tab::Versions {
                    c.versions = Some(Vec::new());
                }
                if c.deleted.is_none() && c.tab == Tab::Deleted {
                    c.deleted = Some(Vec::new());
                }
            }
            _ => {}
        }
    }

    /// Shows a version in place of the board (None: back to now).
    fn history_select(&mut self, at: Option<u64>) {
        let Some(c) = &mut self.history.card else { return };
        if c.selected == at {
            return;
        }
        c.selected = at;
        c.preview = None;
        c.note = None;
        let board = c.board;
        self.selection.clear();
        self.scene.invalidate();
        self.needs_redraw = true;
        if let Some(at_ms) = at {
            self.end_edit();
            self.gesture = Gesture::Idle;
            self.history_ask(HistoryAsk::Version { board, at_ms });
        }
    }

    /// Shows the k-th newest version (screenshots and tests).
    pub fn history_show(&mut self, k: usize) {
        let at = self.history.card.as_ref().and_then(|c| c.versions.as_ref()?.get(k).copied());
        if at.is_some() {
            self.history_select(at);
        }
    }

    /// Steps to an older (`dir` = 1) or newer (-1) version.
    fn history_step(&mut self, dir: i32) {
        let Some(c) = &self.history.card else { return };
        let Some(versions) = &c.versions else { return };
        let k = match c.selected {
            None => -1,
            Some(at) => versions.iter().position(|v| *v == at).map_or(-1, |k| k as i64),
        };
        let next = k + dir as i64;
        let at = if next < 0 { None } else { versions.get(next as usize).copied() };
        if next < versions.len() as i64 {
            self.history_select(at);
        }
    }

    /// Brings the shown version back on the board, as one undo step.
    fn history_restore(&mut self) {
        let Some(c) = &mut self.history.card else { return };
        let (Some(at), Some(preview)) = (c.selected, c.preview.take()) else { return };
        let then = preview.board;
        if c.board != self.doc.board.id {
            return;
        }
        // What's there now is kept too, so the restore can be looked back on.
        if let Some(lib) = &self.library {
            if let Err(e) = lib.keep_version(&self.doc.board, self.now_ms, self.now_ms) {
                eprintln!("wall: could not keep a version before restoring: {e}");
            }
        }
        let changes = changes_to(&self.doc.board, &then);
        let n = changes.len();
        self.doc.set_clock(self.now_ms);
        self.doc.replace(changes);
        self.scene.invalidate();
        let shown = when(at, self.now_ms, self.history.utc_offset_min);
        if let Some(c) = &mut self.history.card {
            c.selected = None;
            c.note = Some(if n == 0 { format!("{shown} was the same as now.") } else { format!("Restored {}. Undo (Ctrl Z) takes it back.", lower_first(&shown)) });
            if let Some(v) = &mut c.versions {
                if !v.contains(&self.now_ms) && self.library.is_some() {
                    v.insert(0, self.now_ms);
                }
            }
        }
        self.needs_redraw = true;
    }

    /// Makes a new board beside this one from the shown version, and opens it.
    fn history_copy(&mut self) -> Option<Uuid> {
        let c = self.history.card.as_ref()?;
        let at = c.selected?;
        let then = c.preview.as_ref()?.board.clone();
        let now = self.now_ms;
        let mut b = Board::new(format!("{} ({})", self.doc.board.name, when(at, now, self.history.utc_offset_min)), now);
        let parent = self.doc.board.parent.unwrap_or(self.doc.board.id);
        b.parent = Some(parent);
        for i in then.live_items() {
            b.items.insert(i.id, BoardItem { rev: 1, updated_ms: now, updated_by: self.doc.device.clone(), ..i.clone() });
        }
        let id = b.id;
        self.board_names.insert(id, b.name.clone());
        self.board_meta.insert(id, crate::sidebar::BoardMeta::of(&b));
        match &self.library {
            Some(lib) => {
                if let Err(e) = lib.write_board(&b) {
                    if let Some(c) = &mut self.history.card {
                        c.note = Some(format!("Couldn't make the copy: {e}"));
                    }
                    return None;
                }
            }
            None => {
                let doc = Document::new(b, self.doc.device.clone());
                self.parked.insert(id, (doc, crate::Camera { center: [0.0, 0.0], zoom: 1.0 }));
            }
        }
        self.add_card_on(parent, id);
        self.close_history();
        self.go_to_board(id);
        self.fit_view();
        Some(id)
    }

    /// Puts a card for board `id` on board `on`: its old card back if it was
    /// deleted, else a new one to the right of everything there.
    fn add_card_on(&mut self, on: Uuid, id: Uuid) {
        self.edit_board(on, |doc| {
            let old: Vec<Uuid> = doc.board.items.values().filter(|i| i.deleted && matches!(i.kind, ItemKind::Board { board_id } if board_id == id)).map(|i| i.id).collect();
            if doc.board.live_items().any(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == id)) {
                return;
            }
            if let Some(last) = old.last() {
                doc.restore(&[*last]);
                return;
            }
            let right = doc.board.live_items().map(|i| i.frame.x + i.frame.w).fold(f64::MIN, f64::max);
            let top = doc.board.live_items().map(|i| i.frame.y).fold(f64::MAX, f64::min);
            let mut f = Frame { w: 220.0, h: 140.0, ..Default::default() };
            if right > f64::MIN {
                f.x = right + 80.0;
                f.y = top;
            }
            doc.add(vec![(f, ItemKind::Board { board_id: id })]);
        });
    }

    /// Brings a deleted board (and the boards deleted inside it) back from
    /// the library's trash. True if it's back.
    pub fn restore_deleted_board(&mut self, id: Uuid) -> bool {
        matches!(self.restore_board_here(id), HistoryAnswer::Restored(_))
    }

    /// Brings a deleted board, and the boards deleted inside it, back from
    /// the library's trash, with its card where it was.
    fn restore_board_here(&mut self, id: Uuid) -> HistoryAnswer {
        let Some(lib) = self.library.clone() else { return HistoryAnswer::Failed("no library".into()) };
        let trashed = match lib.trashed_boards() {
            Ok(t) => t,
            Err(e) => return HistoryAnswer::Failed(e.to_string()),
        };
        let Some(top) = trashed.iter().find(|t| t.board.id == id) else { return HistoryAnswer::Failed("It's no longer in the trash.".into()) };
        let mut ids = vec![id];
        loop {
            let more: Vec<Uuid> = trashed.iter().filter(|t| t.deleted_ms == top.deleted_ms && !ids.contains(&t.board.id) && t.board.parent.is_some_and(|p| ids.contains(&p))).map(|t| t.board.id).collect();
            if more.is_empty() {
                break;
            }
            ids.extend(more);
        }
        // Its old parent, or the library's top board if that's gone too.
        let root = self.board_meta.iter().filter(|(_, m)| m.parent.is_none()).min_by_key(|(_, m)| m.created_ms).map(|(b, _)| *b);
        let parent = top.board.parent.filter(|p| self.board_meta.contains_key(p)).or(root);
        let now = self.now_ms;
        for b in &ids {
            let mut board = match lib.restore_board(*b) {
                Ok(board) => board,
                Err(e) => return HistoryAnswer::Failed(e.to_string()),
            };
            // Newer revisions, so every copy of the library takes them again.
            for i in board.items.values_mut() {
                i.rev += 1;
                i.updated_ms = now;
                i.updated_by = self.doc.device.clone();
            }
            if *b == id {
                board.parent = parent;
            }
            board.updated_ms = now;
            if let Err(e) = lib.write_board(&board) {
                return HistoryAnswer::Failed(e.to_string());
            }
            self.board_names.insert(*b, board.name.clone());
            self.board_meta.insert(*b, crate::sidebar::BoardMeta::of(&board));
        }
        if let Some(p) = parent {
            self.add_card_on(p, id);
        }
        self.needs_redraw = true;
        HistoryAnswer::Restored(id)
    }

    fn history_layout(&mut self) -> Option<Layout> {
        let c = self.history.card.as_ref()?;
        let can_restore_boards = self.library.is_some() || self.history.remote;
        let items_now = self.doc.board.live_items().count();
        let font = &mut self.font;
        let inputs = Inputs { card: c, board_name: &self.doc.board.name, items_now, now_ms: self.now_ms, offset: self.history.utc_offset_min, can_restore_boards };
        Some(layout(&inputs, self.cursor, self.viewport, &mut |s, size| panel::text_width(font, s, size)))
    }

    /// A press while the card is open. True when the card took it. While a
    /// version is shown every press is taken: outside the card it pans.
    pub(crate) fn history_press(&mut self, button: Button, pos: [f32; 2]) -> bool {
        let Some(l) = self.history_layout() else { return false };
        self.needs_redraw = true;
        if !panel::inside(l.rect, pos) {
            if self.history.previewing() {
                if button == Button::Primary {
                    self.gesture = Gesture::Pan;
                }
                return true;
            }
            return false;
        }
        if button != Button::Primary {
            return true;
        }
        let Some(hit) = l.hits.iter().rev().find(|(r, _)| panel::inside(*r, pos)).map(|(_, h)| h.clone()) else { return true };
        match hit {
            Hit::Close => self.close_history(),
            Hit::Tab(tab) => {
                if self.history.card.as_ref().is_some_and(|c| c.tab != tab) {
                    self.history_select(None);
                    self.open_history(tab);
                }
            }
            Hit::Now => self.history_select(None),
            Hit::Version(at) => self.history_select(Some(at)),
            Hit::Restore => self.history_restore(),
            Hit::Copy => {
                self.history_copy();
            }
            Hit::RestoreBoard(id) => self.history_ask(HistoryAsk::Restore { board: id }),
        }
        true
    }

    /// Keys while the card is open. While a version is shown, Esc goes back
    /// to now, ← → step through versions, and nothing else reaches the board.
    pub(crate) fn history_key(&mut self, key: Key) -> bool {
        if self.history.card.is_none() {
            return false;
        }
        let previewing = self.history.previewing();
        self.needs_redraw = true;
        match key {
            Key::Escape if previewing => self.history_select(None),
            Key::Escape => self.close_history(),
            Key::Left if self.history.card.as_ref().is_some_and(|c| c.tab == Tab::Versions) => self.history_step(1),
            Key::Right if self.history.card.as_ref().is_some_and(|c| c.tab == Tab::Versions) => self.history_step(-1),
            Key::Space { pressed } => self.space_held = pressed,
            _ => return previewing,
        }
        true
    }

    /// The wheel over the card scrolls its list.
    pub(crate) fn history_wheel(&mut self, steps: f32) -> bool {
        let Some(l) = self.history_layout() else { return false };
        if !panel::inside(l.rect, self.cursor) {
            return false;
        }
        if let Some(c) = &mut self.history.card {
            let by = steps.abs().ceil().max(1.0) as usize;
            c.scroll = if steps > 0.0 { c.scroll.saturating_sub(by) } else { c.scroll + by };
            let total = match c.tab {
                Tab::Versions => c.versions.as_ref().map_or(0, |v| v.len() * 2 + 1),
                Tab::Deleted => c.deleted.as_ref().map_or(0, Vec::len),
            };
            c.scroll = c.scroll.min(total.saturating_sub(l.fits.max(1)));
        }
        self.needs_redraw = true;
        true
    }

    /// For [`App::render`]: swaps the shown version in place of the board
    /// (and back again after drawing).
    pub(crate) fn history_swap(&mut self) -> bool {
        match self.history.card.as_mut().and_then(|c| c.preview.as_mut()) {
            Some(doc) => {
                std::mem::swap(&mut self.doc, doc);
                true
            }
            None => false,
        }
    }

    pub(crate) fn draw_history(&mut self, renderer: &mut Renderer) {
        let Some(mut l) = self.history_layout() else { return };
        let t = theme::t();
        if let Some((_, text)) = l.banner.take() {
            let w = panel::text_width(&mut self.font, &text, 13.0) + 32.0;
            let r = [((self.viewport[0] - W - 24.0 - w) / 2.0).max(8.0), BAR + 12.0, w, 34.0];
            l.boxes.push(PanelBox { rect: r, fill: t.accent, stroke: [0.0; 4], radius: 17.0, stroke_w: 0.0 });
            l.texts.push(TextRun { text, at: [r[0] + 16.0, r[1] + 22.0], size: 13.0, color: t.panel_bg });
        }
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<BoxInstance> = l.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}

/// Deleted boards as the card lists them: boards deleted inside another
/// one (at the same moment) are counted with it, not listed.
pub fn deleted_list(trashed: &[wall_format::TrashedBoard]) -> Vec<DeletedBoard> {
    let mut out = Vec::new();
    for t in trashed {
        let with_parent = t.board.parent.is_some_and(|p| trashed.iter().any(|o| o.board.id == p && o.deleted_ms == t.deleted_ms));
        if with_parent {
            continue;
        }
        let mut inside: BTreeSet<Uuid> = BTreeSet::new();
        let mut grew = true;
        while grew {
            grew = false;
            for o in trashed.iter().filter(|o| o.deleted_ms == t.deleted_ms) {
                if o.board.parent.is_some_and(|p| p == t.board.id || inside.contains(&p)) && inside.insert(o.board.id) {
                    grew = true;
                }
            }
        }
        out.push(DeletedBoard { id: t.board.id, name: t.board.name.clone(), parent: t.board.parent, deleted_ms: t.deleted_ms, items: t.board.live_items().filter(|i| !matches!(i.kind, ItemKind::Board { .. })).count(), inside: inside.len() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Mods};
    use wall_format::Library;

    const MIN: u64 = 60_000;

    fn open(dir: &std::path::Path, now: u64) -> App {
        let mut a = App::open_library(&dir.join("L.wall"), "here", now).unwrap();
        a.resize([1400.0, 900.0]);
        a.set_time(now);
        a
    }

    fn note(a: &mut App, text: &str) -> Uuid {
        a.doc.set_clock(a.now_ms);
        a.doc.add(vec![(Frame { x: 0.0, y: 0.0, w: 100.0, h: 50.0, ..Default::default() }, ItemKind::Note { text: text.into(), color: None })])[0]
    }

    fn texts(a: &App) -> Vec<String> {
        let mut v: Vec<String> = a.doc.board.live_items().filter_map(|i| match &i.kind {
            ItemKind::Note { text, .. } => Some(text.clone()),
            _ => None,
        }).collect();
        v.sort();
        v
    }

    fn press(a: &mut App, hit: &Hit) {
        let l = a.history_layout().unwrap();
        let r = l.hits.iter().find(|(_, h)| h == hit).unwrap_or_else(|| panic!("no {hit:?} in {:?}", l.hits.iter().map(|h| &h.1).collect::<Vec<_>>())).0;
        let p = [r[0] + 4.0, r[1] + 4.0];
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
    }

    #[test]
    fn saving_keeps_versions_that_can_be_looked_at_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = 1_800_000_000_000;
        let mut a = open(dir.path(), t0);
        let board = a.board_id();
        note(&mut a, "morning");
        a.save_if_dirty().unwrap();
        // Ten minutes of more work: the board as it was is kept.
        a.set_time(t0 + 11 * MIN);
        note(&mut a, "afternoon");
        a.save_if_dirty().unwrap();
        // Saves in between keep nothing new.
        a.set_time(t0 + 12 * MIN);
        note(&mut a, "more afternoon");
        a.save_if_dirty().unwrap();
        a.set_time(t0 + 25 * MIN);
        let late = note(&mut a, "evening");
        a.save_if_dirty().unwrap();
        let lib = Library::open(dir.path().join("L.wall")).unwrap();
        let versions = lib.versions(board).unwrap();
        // The empty board wasn't kept: only the two states before the
        // stretches of work after them.
        assert_eq!(versions, vec![t0 + 12 * MIN, t0], "{versions:?}");

        a.run_command(crate::menu::Command::History);
        assert!(a.history_open());
        let older = *versions.last().unwrap();
        let newer = versions[0];
        assert_eq!(lib.read_version(board, newer).unwrap().live_items().count(), 3);
        press(&mut a, &Hit::Version(newer));
        assert!(a.history_previewing());
        // Shown in place of the board, which itself is untouched.
        assert!(a.history_swap());
        assert_eq!(texts(&a), vec!["afternoon", "more afternoon", "morning"]);
        a.history_swap();
        assert_eq!(texts(&a), vec!["afternoon", "evening", "more afternoon", "morning"]);
        // Nothing edits the board meanwhile, and sync waits.
        a.text_input("x");
        a.key(Key::Char('t'));
        a.key(Key::Delete);
        assert_eq!(a.item_count(), 4);
        assert!(a.sync_boards().is_none());
        // ← goes older.
        a.key(Key::Left);
        assert_eq!(a.history.card.as_ref().unwrap().selected, Some(older));
        a.key(Key::Right);
        assert_eq!(a.history.card.as_ref().unwrap().selected, Some(newer));

        a.key(Key::Left);
        press(&mut a, &Hit::Restore);
        assert!(!a.history_previewing());
        assert_eq!(texts(&a), vec!["morning"]);
        assert!(a.doc.board.items[&late].deleted && a.doc.board.items[&late].rev > 1, "a tombstone that syncs");
        assert!(a.sync_boards().is_some());
        // What was there before the restore is a version too, and Undo
        // takes the restore back.
        assert!(lib.versions(board).unwrap().len() >= 3);
        a.key(Key::Undo);
        assert_eq!(texts(&a), vec!["afternoon", "evening", "more afternoon", "morning"]);
        a.key(Key::Escape);
        assert!(!a.history_open());
    }

    #[test]
    fn a_version_can_become_a_new_board_beside_this_one() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = 1_800_000_000_000;
        let mut a = open(dir.path(), t0);
        let home = a.board_id();
        note(&mut a, "first");
        a.save_if_dirty().unwrap();
        a.set_time(t0 + 20 * MIN);
        note(&mut a, "second");
        a.save_if_dirty().unwrap();
        assert_eq!(Library::open(dir.path().join("L.wall")).unwrap().versions(home).unwrap().len(), 1);
        a.run_command(crate::menu::Command::History);
        let at = a.history.card.as_ref().unwrap().versions.as_ref().unwrap()[0];
        press(&mut a, &Hit::Version(at));
        press(&mut a, &Hit::Copy);
        assert!(!a.history_open());
        assert_ne!(a.board_id(), home);
        assert_eq!(texts(&a), vec!["first"]);
        assert!(a.doc.board.name.starts_with("Board 1 (Today "), "{}", a.doc.board.name);
        // Its card is on the board it came from, which is unchanged.
        a.open_board(home, false);
        assert_eq!(texts(&a), vec!["first", "second"]);
        assert_eq!(a.doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Board { .. })).count(), 1);
    }

    #[test]
    fn deleted_boards_come_back_with_the_boards_inside_and_their_card() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = 1_800_000_000_000;
        let mut a = open(dir.path(), t0);
        let home = a.board_id();
        let outer = a.new_board();
        a.open_board(outer, true);
        note(&mut a, "on outer");
        let inner = a.new_board();
        a.go_up();
        a.save_if_dirty().unwrap();
        let card = a.doc.board.live_items().find(|i| matches!(i.kind, ItemKind::Board { .. })).unwrap().id;
        assert!(a.delete_board(outer));
        assert!(!a.board_names.contains_key(&inner));

        a.run_command(crate::menu::Command::DeletedBoards);
        let list = a.history.card.as_ref().unwrap().deleted.clone().unwrap();
        assert_eq!(list.len(), 1, "the inner board goes with the outer one");
        assert_eq!((list[0].id, list[0].items, list[0].inside), (outer, 1, 1));
        press(&mut a, &Hit::RestoreBoard(outer));
        let c = a.history.card.as_ref().unwrap();
        assert!(c.deleted.as_ref().unwrap().is_empty());
        assert!(c.note.as_ref().unwrap().contains("is back"));
        assert!(a.board_names.contains_key(&outer) && a.board_names.contains_key(&inner));
        // The same card is back (not a second one), as a newer revision.
        let back = &a.doc.board.items[&card];
        assert!(!back.deleted);
        let lib = Library::open(dir.path().join("L.wall")).unwrap();
        assert!(lib.board_ids().unwrap().contains(&inner));
        let o = lib.read_board(outer).unwrap();
        assert_eq!(o.parent, Some(home));
        assert!(o.live_items().all(|i| i.rev >= 2 && i.updated_by == "here"), "newer revisions, so other copies take them");
        a.close_history();
        a.open_board(outer, true);
        assert_eq!(texts(&a), vec!["on outer"]);
    }

    #[test]
    fn without_a_library_the_shell_is_asked() {
        let mut a = App::new(Board::new("Shared", 0), "web");
        a.resize([1400.0, 900.0]);
        a.run_command(crate::menu::Command::History);
        assert!(a.take_shell_commands().is_empty());
        assert!(a.history.card.as_ref().unwrap().note.as_ref().unwrap().contains("isn't kept"));
        a.close_history();
        a.set_history_remote(true);
        let board = a.board_id();
        a.run_command(crate::menu::Command::History);
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::History(HistoryAsk::Versions { board })]);
        a.history_answer(HistoryAnswer::Versions { board, times: vec![5_000, 1_000] });
        press(&mut a, &Hit::Version(1_000));
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::History(HistoryAsk::Version { board, at_ms: 1_000 })]);
        let mut then = Board::new("Shared", 0);
        then.id = board;
        let id = Uuid::new_v4();
        then.items.insert(id, BoardItem { id, frame: Frame { w: 10.0, h: 10.0, ..Default::default() }, kind: ItemKind::Note { text: "then".into(), color: None }, rev: 3, updated_ms: 1, updated_by: "x".into(), deleted: false });
        a.history_answer(HistoryAnswer::Version { board, at_ms: 1_000, version: then });
        press(&mut a, &Hit::Restore);
        assert_eq!(texts(&a), vec!["then"]);
        assert_eq!(a.doc.board.items[&id].rev, 1, "new to this copy: its first revision here");
        a.run_command(crate::menu::Command::DeletedBoards);
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::History(HistoryAsk::Deleted)]);
        let gone = Uuid::new_v4();
        a.history_answer(HistoryAnswer::Deleted(vec![DeletedBoard { id: gone, name: "Old".into(), parent: None, deleted_ms: 1, items: 2, inside: 0 }]));
        press(&mut a, &Hit::RestoreBoard(gone));
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::History(HistoryAsk::Restore { board: gone })]);
        a.history_answer(HistoryAnswer::Restored(gone));
        assert!(a.history.card.as_ref().unwrap().note.as_ref().unwrap().contains("“Old” is back"));
    }

    #[test]
    fn dates_read_in_local_time() {
        let now = 1_795_132_800_000 + 10 * 3_600_000; // 2026-11-20 10:00 UTC
        assert_eq!(when(now - 3_600_000, now, 0), "Today 09:00");
        assert_eq!(when(now - 86_400_000, now, 0), "Yesterday 10:00");
        assert_eq!(when(now - 86_400_000, now, 8 * 60), "Yesterday 18:00");
        assert_eq!(when(now - 5 * 86_400_000, now, 0), "Sun 15 Nov 10:00");
        assert_eq!(when(now - 400 * 86_400_000, now, 0), "Thu 16 Oct 2025 10:00");
    }
}
