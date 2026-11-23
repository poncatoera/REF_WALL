//! The side panel on the left (S): every board in the library as a tree,
//! with recent boards and a search box, and the library's pictures as a
//! second tab (L). It pushes the board over while open.
//!
//! Click a board to open it, double-click to rename, drag it up or down
//! among its sisters to reorder or onto the middle of another board to put
//! it inside that one, drop pictures from the board onto it to move them
//! there, and delete it with the × that shows on hover (asks first; the
//! board goes to the library's trash folder).
//!
//! As with the other panels, layout is plain data in screen pixels so it
//! can be tested without a GPU.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;
use wall_format::{Board, ContentHash, ItemKind, MediaKind};
use wall_text::Font;

use crate::library_view::Thumb;
use crate::menu::BAR;
use crate::panel::{self, PanelBox, TextRun};
use crate::{theme, App, Picture};

/// Width of the panel.
pub const WIDTH: f32 = 280.0;
const PAD: f32 = 14.0;
const HEADER_H: f32 = 40.0;
const ROW_H: f32 = 30.0;
const INDENT: f32 = 14.0;
const THUMB: [f32; 2] = [30.0, 22.0];
/// How many recent boards are listed.
const RECENT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Boards,
    Library,
}

/// What can be typed into in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Search,
    Rename(Uuid),
}

/// The panel's state while open.
#[derive(Debug, Clone)]
pub struct Sidebar {
    pub tab: Tab,
    /// Boards whose sub-boards are hidden.
    pub folded: HashSet<Uuid>,
    pub search: String,
    /// What is being typed into, and the text so far.
    pub typing: Option<(Field, String)>,
    /// First tree row shown.
    pub scroll: usize,
    /// A board whose row asks "Delete?".
    pub confirm: Option<Uuid>,
}

impl Sidebar {
    pub fn new(tab: Tab) -> Self {
        Sidebar { tab, folded: HashSet::new(), search: String::new(), typing: None, scroll: 0, confirm: None }
    }
}

/// Items moved from one board to another (dropped on a board's row).
#[derive(Debug, Clone)]
pub struct MovedOut {
    pub from: Uuid,
    pub originals: Vec<Uuid>,
    pub target: Uuid,
    pub copies: Vec<Uuid>,
}

/// What the app knows about a board without opening it.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardMeta {
    pub parent: Option<Uuid>,
    pub created_ms: u64,
    /// Last opened or changed, for "Recent".
    pub seen_ms: u64,
    /// The biggest picture on it, for the row's thumbnail.
    pub thumb: Option<ContentHash>,
}

impl BoardMeta {
    pub fn of(board: &Board) -> Self {
        BoardMeta { parent: board.parent, created_ms: board.created_ms, seen_ms: board.updated_ms, thumb: thumb_of(board) }
    }
}

/// The biggest picture or video on a board.
pub fn thumb_of(board: &Board) -> Option<ContentHash> {
    board
        .live_items()
        .filter_map(|i| match &i.kind {
            ItemKind::Media { hash, .. } => Some((i.frame.w * i.frame.h, hash)),
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, h)| h.clone())
}

/// One line in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: Uuid,
    pub name: String,
    pub depth: usize,
    pub has_children: bool,
    pub folded: bool,
    pub current: bool,
    pub thumb: Option<ContentHash>,
    /// Small grey text after the name (the parent board, in search results).
    pub note: Option<String>,
}

/// Boards in tree order: each board followed by its sub-boards. Sisters are
/// in `order` first, the rest oldest first. Boards whose parent is unknown
/// count as top level.
pub fn tree_order(meta: &HashMap<Uuid, BoardMeta>, order: &[Uuid]) -> Vec<(Uuid, usize)> {
    let rank: HashMap<Uuid, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
    for (id, m) in meta {
        let parent = m.parent.filter(|p| meta.contains_key(p) && p != id);
        children.entry(parent).or_default().push(*id);
    }
    for list in children.values_mut() {
        list.sort_by_key(|id| (rank.get(id).copied().unwrap_or(usize::MAX), meta[id].created_ms, *id));
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk(id: Uuid, depth: usize, children: &HashMap<Option<Uuid>, Vec<Uuid>>, out: &mut Vec<(Uuid, usize)>, seen: &mut HashSet<Uuid>) {
        if !seen.insert(id) {
            return; // a loop in the parent links
        }
        out.push((id, depth));
        for c in children.get(&Some(id)).into_iter().flatten() {
            walk(*c, depth + 1, children, out, seen);
        }
    }
    for root in children.get(&None).cloned().unwrap_or_default() {
        walk(root, 0, &children, &mut out, &mut seen);
    }
    // Boards only reachable through a loop still get listed.
    let mut rest: Vec<Uuid> = meta.keys().filter(|id| !seen.contains(id)).copied().collect();
    rest.sort();
    for id in rest {
        walk(id, 0, &children, &mut out, &mut seen);
    }
    out
}

/// Something a click in the panel asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Tab(Tab),
    Close,
    /// A press on a board's row: opens it on release, renames on a
    /// double-click, or reorders when dragged.
    Row(Uuid),
    Fold(Uuid),
    New,
    Search,
    /// The × on a row: asks whether to delete the board.
    Delete(Uuid),
    ConfirmDelete(Uuid),
    CancelDelete,
    /// A click on the panel that does nothing, so it doesn't reach the board.
    None,
}

#[derive(Default)]
pub struct Layout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub images: Vec<(ContentHash, [f32; 4])>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Action)>,
    /// Tree rows on screen (for dropping onto and reordering).
    pub rows: Vec<([f32; 4], Uuid)>,
    /// Where the library tab draws its grid.
    pub content: [f32; 4],
    pub caret: Option<[f32; 4]>,
    pub max_scroll: usize,
}

impl Layout {
    pub fn action_at(&self, p: [f32; 2]) -> Option<Action> {
        if !panel::inside(self.rect, p) {
            return None;
        }
        Some(self.hits.iter().rev().find(|(r, _)| panel::inside(*r, p)).map_or(Action::None, |(_, a)| a.clone()))
    }

    /// The board row under `p`: Some(false) in its top quarter (before
    /// it), Some(true) in its bottom quarter (after it), None in the middle
    /// (into it).
    pub fn row_at(&self, p: [f32; 2]) -> Option<(Uuid, Option<bool>)> {
        self.rows.iter().find(|(r, _)| panel::inside(*r, p)).map(|(r, id)| {
            let t = (p[1] - r[1]) / r[3];
            (*id, if t < 0.25 { Some(false) } else if t > 0.75 { Some(true) } else { None })
        })
    }

    fn rect(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w: if stroke[3] > 0.0 { 1.0 } else { 0.0 } });
    }

    fn text(&mut self, text: impl Into<String>, at: [f32; 2], size: f32, color: [f32; 4]) {
        self.texts.push(TextRun { text: text.into(), at, size, color });
    }
}

/// What to draw in the panel.
pub struct View<'a> {
    pub state: &'a Sidebar,
    pub recent: &'a [Row],
    pub tree: &'a [Row],
    /// The row being dragged, and the row it would land next to.
    pub dragging: Option<Uuid>,
    /// A board row highlighted as the place pictures will be moved to, or
    /// the row a dragged board will go before (false) or after (true).
    pub drop: Option<(Uuid, Option<bool>)>,
    pub hover: [f32; 2],
}

pub fn layout(font: &mut Font, v: &View, viewport: [f32; 2]) -> Layout {
    let t = theme::t();
    let mut l = Layout::default();
    let (x0, y0) = (0.0, BAR);
    let h = (viewport[1] - BAR).max(0.0);
    l.rect = [x0, y0, WIDTH, h];
    l.rect(l.rect, t.panel_bg, [0.0; 4], 0.0);
    l.rect([WIDTH - 1.0, y0, 1.0, h], t.line, [0.0; 4], 0.0);

    // Tabs (a segmented control) and close.
    let seg = [x0 + PAD - 4.0, y0 + 7.0, WIDTH - 2.0 * PAD - 26.0, 28.0];
    l.rect(seg, if theme::is_light() { t.chip } else { t.field }, [0.0; 4], theme::radius::CONTROL);
    let half = (seg[2] - 6.0) / 2.0;
    for (k, (label, tab)) in [("Boards", Tab::Boards), ("Library", Tab::Library)].into_iter().enumerate() {
        let on = v.state.tab == tab;
        let r = [seg[0] + 3.0 + k as f32 * half, seg[1] + 3.0, half, seg[3] - 6.0];
        if on {
            l.rect(r, if theme::is_light() { t.panel_bg } else { t.chip }, [0.0; 4], theme::radius::INNER);
        }
        let tw = panel::text_width(font, label, 12.5);
        l.text(label, [r[0] + (r[2] - tw) / 2.0, r[1] + 15.5], 12.5, if on { t.ink } else { t.dim });
        l.hits.push((r, Action::Tab(tab)));
    }
    let close = [x0 + WIDTH - PAD - 20.0, y0 + 9.0, 22.0, 22.0];
    l.text("×", [close[0] + 5.0, close[1] + 16.0], 18.0, t.dim);
    l.hits.push((close, Action::Close));
    l.content = [x0, y0 + HEADER_H - 40.0 + 40.0, WIDTH - 1.0, h - HEADER_H];
    if v.state.tab == Tab::Library {
        // The library grid draws itself in `content`, under its own tabs.
        l.content = [x0, y0 + HEADER_H, WIDTH - 1.0, h - HEADER_H];
        return l;
    }

    // Search box and "+".
    let mut y = y0 + HEADER_H + 4.0;
    let inner = WIDTH - 2.0 * PAD;
    let sr = [x0 + PAD, y, inner - 34.0, 28.0];
    let typing_search = matches!(v.state.typing, Some((Field::Search, _)));
    l.rect(sr, t.field, if typing_search { t.accent } else { t.line }, theme::radius::CONTROL);
    let (shown, colour) = match &v.state.typing {
        Some((Field::Search, s)) => (s.clone(), t.ink),
        _ if v.state.search.is_empty() => ("Search boards".to_owned(), t.dim),
        _ => (v.state.search.clone(), t.ink),
    };
    let shown_fit = if typing_search { shown.clone() } else { panel::fit(font, &shown, 12.5, sr[2] - 16.0) };
    l.text(shown_fit.clone(), [sr[0] + 8.0, sr[1] + 18.5], 12.5, colour);
    if typing_search {
        let w = panel::text_width(font, &shown_fit, 12.5);
        l.caret = Some([sr[0] + 8.0 + w + 1.0, sr[1] + 7.0, 1.5, 14.0]);
    }
    l.hits.push((sr, Action::Search));
    let plus = [x0 + PAD + inner - 28.0, y, 28.0, 28.0];
    let over = panel::inside(plus, v.hover);
    l.rect(plus, if over { t.hover } else { [0.0; 4] }, t.line, theme::radius::CONTROL);
    l.text("+", [plus[0] + 8.5, plus[1] + 20.0], 17.0, t.ink);
    l.hits.push((plus, Action::New));
    y += 28.0 + 14.0;

    let searching = !v.state.search.is_empty() || typing_search;
    if !searching && !v.recent.is_empty() {
        l.text("Recent", [x0 + PAD, y + 10.0], 11.0, t.dim);
        y += 18.0;
        for row in v.recent {
            let r = [x0 + 6.0, y, WIDTH - 12.0, ROW_H];
            draw_row(&mut l, font, v, row, r, false);
            y += ROW_H;
        }
        y += 10.0;
    }
    l.text(if searching { "Found" } else { "All boards" }, [x0 + PAD, y + 10.0], 11.0, t.dim);
    y += 18.0;
    let bottom = y0 + h - 30.0;
    let fits = (((bottom - y) / ROW_H).floor() as usize).max(1);
    l.max_scroll = v.tree.len().saturating_sub(fits);
    let first = v.state.scroll.min(l.max_scroll);
    if v.tree.is_empty() {
        l.text(if searching { "No board by that name." } else { "No boards yet." }, [x0 + PAD, y + 18.0], 12.5, t.dim);
    }
    for row in v.tree.iter().skip(first).take(fits) {
        let r = [x0 + 6.0, y, WIDTH - 12.0, ROW_H];
        draw_row(&mut l, font, v, row, r, true);
        l.rows.push((r, row.id));
        y += ROW_H;
    }
    let hint = if l.max_scroll > 0 {
        format!("{}–{} of {}  ·  scroll for more", first + 1, (first + fits).min(v.tree.len()), v.tree.len())
    } else {
        "Double-click to rename  ·  drag to move".to_owned()
    };
    let hint = panel::fit(font, &hint, 11.0, inner);
    l.text(hint, [x0 + PAD, y0 + h - 12.0], 11.0, t.dim);
    l
}

fn draw_row(l: &mut Layout, font: &mut Font, v: &View, row: &Row, r: [f32; 4], tree: bool) {
    let t = theme::t();
    let over = panel::inside(r, v.hover);
    let drop = v.drop.filter(|(id, _)| *id == row.id && tree);
    match drop {
        Some((_, None)) => l.rect(r, t.chip, t.accent, 6.0),
        Some((_, Some(after))) => l.rect([r[0] + 4.0, if after { r[1] + r[3] - 1.0 } else { r[1] - 1.0 }, r[2] - 8.0, 2.0], t.accent, [0.0; 4], 1.0),
        None if row.current => l.rect(r, t.accent_soft, [0.0; 4], theme::radius::INNER),
        None if over && v.dragging.is_none() => l.rect(r, t.hover, [0.0; 4], theme::radius::INNER),
        None => {}
    }
    let indent = if tree { row.depth as f32 * INDENT } else { 0.0 };
    let mut x = r[0] + 6.0 + indent;
    if tree {
        // Fold arrow for boards with sub-boards.
        if row.has_children {
            let fr = [x - 2.0, r[1], 16.0, r[3]];
            l.text(if row.folded { "›" } else { "⌄" }, [x + 1.0, r[1] + 19.0], 13.0, t.dim);
            l.hits.push((fr, Action::Fold(row.id)));
        }
        x += 14.0;
    }
    // Tiny thumbnail (or an empty card).
    let th = [x, r[1] + (r[3] - THUMB[1]) / 2.0, THUMB[0], THUMB[1]];
    l.rect(th, t.tile, [0.0; 4], 4.0);
    if let Some(h) = &row.thumb {
        l.images.push((h.clone(), [th[0] + 1.0, th[1] + 1.0, th[2] - 2.0, th[3] - 2.0]));
    }
    x += THUMB[0] + 8.0;
    let mut right = r[0] + r[2] - 8.0;
    if tree && v.state.confirm == Some(row.id) {
        // "Delete?  Delete  Cancel" in place of the name.
        l.text("Delete?", [x, r[1] + 19.5], 12.5, t.ink);
        let cancel_w = panel::text_width(font, "Cancel", 12.0) + 14.0;
        let del_w = panel::text_width(font, "Delete", 12.0) + 14.0;
        let cancel = [right - cancel_w, r[1] + 4.0, cancel_w, r[3] - 8.0];
        let del = [cancel[0] - 6.0 - del_w, cancel[1], del_w, cancel[3]];
        l.rect(del, t.accent, [0.0; 4], crate::theme::radius::CONTROL);
        l.text("Delete", [del[0] + 7.0, r[1] + 19.0], 12.0, t.accent_ink);
        l.rect(cancel, [0.0; 4], t.line, crate::theme::radius::CONTROL);
        l.text("Cancel", [cancel[0] + 7.0, r[1] + 19.0], 12.0, t.ink);
        l.hits.insert(0, (r, Action::None));
        l.hits.push((del, Action::ConfirmDelete(row.id)));
        l.hits.push((cancel, Action::CancelDelete));
        return;
    }
    // × to delete, on hover (not for the library's top board).
    if tree && over && v.dragging.is_none() && row.depth > 0 && v.state.typing.is_none() {
        let xr = [right - 18.0, r[1] + (r[3] - 20.0) / 2.0, 20.0, 20.0];
        l.text("×", [xr[0] + 4.5, xr[1] + 15.5], 16.0, t.dim);
        l.hits.push((xr, Action::Delete(row.id)));
        right = xr[0] - 4.0;
    }
    let renaming = matches!(&v.state.typing, Some((Field::Rename(id), _)) if *id == row.id) && tree;
    let ink = if row.current { t.accent_text } else { t.ink };
    if renaming {
        let Some((_, text)) = &v.state.typing else { unreachable!() };
        let fr = [x - 4.0, r[1] + 3.0, right - x + 4.0, r[3] - 6.0];
        l.rect(fr, t.field, t.accent, crate::theme::radius::CONTROL);
        l.text(text.clone(), [x, r[1] + 19.5], 12.5, t.ink);
        let w = panel::text_width(font, text, 12.5);
        l.caret = Some([x + w + 1.0, r[1] + 8.0, 1.5, 14.0]);
    } else {
        let name = panel::fit(font, &row.name, 12.5, right - x);
        let nw = panel::text_width(font, &name, 12.5);
        l.text(name, [x, r[1] + 19.5], 12.5, ink);
        if let Some(note) = &row.note {
            let room = right - x - nw - 8.0;
            if room > 30.0 {
                l.text(panel::fit(font, note, 11.0, room), [x + nw + 8.0, r[1] + 19.5], 11.0, t.dim);
            }
        }
    }
    // The row itself is pressed after the fold arrow is checked.
    l.hits.insert(0, (r, Action::Row(row.id)));
}

impl App {
    // ---- side panel on the left --------------------------------------------

    /// Opens the panel on `tab`, or closes it if it already shows that tab.
    pub fn toggle_sidebar(&mut self, tab: Tab) {
        self.needs_redraw = true;
        match &mut self.sidebar {
            Some(s) if s.tab == tab => self.close_sidebar(),
            Some(s) => {
                s.tab = tab;
                if tab == Tab::Library {
                    self.ensure_library_view();
                }
            }
            None => {
                self.sidebar = Some(Sidebar::new(tab));
                if tab == Tab::Library {
                    self.ensure_library_view();
                }
                // Push the board over rather than cover it.
                self.camera.center[0] -= WIDTH / self.camera.zoom;
            }
        }
    }

    pub fn close_sidebar(&mut self) {
        if self.sidebar.take().is_some() {
            self.camera.center[0] += WIDTH / self.camera.zoom;
            self.needs_redraw = true;
        }
    }

    /// Which tab of the left panel shows, if it is open.
    pub fn sidebar_tab(&self) -> Option<Tab> {
        self.sidebar.as_ref().map(|s| s.tab)
    }

    /// Pixels on the left covered by the panel.
    pub(crate) fn left_inset(&self) -> f32 {
        if self.sidebar.is_some() { WIDTH } else { 0.0 }
    }

    /// Recent boards, newest first.
    fn recent_rows(&self) -> Vec<Row> {
        let mut by_seen: Vec<(&Uuid, &BoardMeta)> = self.board_meta.iter().collect();
        by_seen.sort_by(|a, b| b.1.seen_ms.cmp(&a.1.seen_ms).then(a.0.cmp(b.0)));
        by_seen.into_iter().take(RECENT).map(|(id, _)| self.row(*id, 0, false, None)).collect()
    }

    /// A board's thumbnail: its biggest picture or video, else (a board of
    /// boards) the first of its boards that has one.
    fn board_thumb(&self, id: Uuid, depth: usize) -> Option<ContentHash> {
        let own = if id == self.doc.board.id { thumb_of(&self.doc.board) } else { self.board_meta.get(&id).and_then(|m| m.thumb.clone()) };
        if own.is_some() || depth > 4 {
            return own;
        }
        let mut children: Vec<(&Uuid, &BoardMeta)> = self.board_meta.iter().filter(|(_, m)| m.parent == Some(id)).collect();
        children.sort_by_key(|(c, m)| (self.board_order.iter().position(|o| o == *c).unwrap_or(usize::MAX), m.created_ms));
        children.into_iter().find_map(|(c, _)| self.board_thumb(*c, depth + 1))
    }

    /// Videos among the panel's thumbnails that need a frame from the shell,
    /// as for [`App::library_posters_wanted`]. Each is asked for once.
    pub(crate) fn sidebar_posters_wanted(&mut self) -> Vec<(ContentHash, std::path::PathBuf, u32, u32, f64)> {
        let Some(l) = self.sidebar_layout() else { return Vec::new() };
        let Some(lib) = self.library.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        for (hash, _) in &l.images {
            if self.board_thumbs.contains_key(hash) || matches!(self.pictures.get(hash), Some(Picture::Ready(_) | Picture::Preview(_))) {
                continue;
            }
            let Some(sc) = self.sidecars.get(hash).cloned().or_else(|| lib.read_sidecar(hash).ok()) else { continue };
            if sc.kind != MediaKind::Video {
                continue;
            }
            let s = (128.0 / sc.width.max(sc.height).max(1) as f32).min(1.0);
            let even = |x: f32| ((x.round() as u32) & !1).max(2);
            let at = sc.duration_ms.map_or(0.0, |ms| (ms as f64 / 1000.0 * 0.1).min(1.0));
            out.push((hash.clone(), lib.media_path(hash, &sc.ext), even(sc.width as f32 * s), even(sc.height as f32 * s), at));
            self.board_thumbs.insert(hash.clone(), Thumb::Requested);
        }
        out
    }

    fn row(&self, id: Uuid, depth: usize, has_children: bool, note: Option<String>) -> Row {
        let current = id == self.doc.board.id;
        let thumb = self.board_thumb(id, 0);
        Row {
            id,
            name: self.board_names.get(&id).cloned().unwrap_or_else(|| "Board".into()),
            depth,
            has_children,
            folded: self.sidebar.as_ref().is_some_and(|s| s.folded.contains(&id)),
            current,
            thumb,
            note,
        }
    }

    /// The tree as rows (folded boards hide theirs), or the boards whose
    /// name holds the search text.
    fn tree_rows(&self) -> Vec<Row> {
        let order = tree_order(&self.board_meta, &self.board_order);
        let query = self
            .sidebar
            .as_ref()
            .map(|s| match &s.typing {
                Some((Field::Search, q)) => q.clone(),
                _ => s.search.clone(),
            })
            .unwrap_or_default()
            .to_lowercase();
        if !query.trim().is_empty() {
            return order
                .iter()
                .filter(|(id, _)| self.board_names.get(id).is_some_and(|n| n.to_lowercase().contains(query.trim())))
                .map(|(id, _)| {
                    let parent = self.board_meta.get(id).and_then(|m| m.parent).and_then(|p| self.board_names.get(&p)).map(|n| format!("in {n}"));
                    self.row(*id, 0, false, parent)
                })
                .collect();
        }
        let folded = self.sidebar.as_ref().map(|s| s.folded.clone()).unwrap_or_default();
        let mut rows = Vec::new();
        let mut hide_below: Option<usize> = None;
        for (k, (id, depth)) in order.iter().enumerate() {
            if let Some(d) = hide_below {
                if *depth > d {
                    continue;
                }
                hide_below = None;
            }
            let has_children = order.get(k + 1).is_some_and(|(_, d)| d > depth);
            rows.push(self.row(*id, *depth, has_children, None));
            if has_children && folded.contains(id) {
                hide_below = Some(*depth);
            }
        }
        rows
    }

    pub(crate) fn sidebar_layout(&mut self) -> Option<Layout> {
        let state = self.sidebar.clone()?;
        let (recent, tree) = if state.tab == Tab::Boards { (self.recent_rows(), self.tree_rows()) } else { (Vec::new(), Vec::new()) };
        let dragged_row = match &self.gesture {
            crate::Gesture::SideRow { id, moved: true, .. } => Some(*id),
            _ => None,
        };
        let (dragging, drop) = match &self.gesture {
            _ if dragged_row.is_some() => {
                let id = dragged_row.unwrap();
                let target = self.side_rows_at(self.cursor).filter(|(t, _)| *t != id);
                (Some(id), target.map(|(t, place)| (t, place)))
            }
            crate::Gesture::Move { .. } => (None, self.side_rows_at(self.cursor).filter(|(t, _)| *t != self.doc.board.id).map(|(t, _)| (t, None))),
            _ => (None, None),
        };
        let v = View { state: &state, recent: &recent, tree: &tree, dragging, drop, hover: self.cursor };
        Some(layout(&mut self.font, &v, self.viewport))
    }

    /// The tree row under a screen point (no drop highlight involved).
    pub(crate) fn side_rows_at(&mut self, p: [f32; 2]) -> Option<(Uuid, Option<bool>)> {
        let state = self.sidebar.clone()?;
        if state.tab != Tab::Boards || p[0] > WIDTH {
            return None;
        }
        let tree = self.tree_rows();
        let recent = self.recent_rows();
        let v = View { state: &state, recent: &recent, tree: &tree, dragging: None, drop: None, hover: p };
        layout(&mut self.font, &v, self.viewport).row_at(p)
    }

    /// A press in the panel. Returns true when it was inside it.
    pub(crate) fn sidebar_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(l) = self.sidebar_layout() else { return false };
        let Some(action) = l.action_at(pos) else { return false };
        if self.sidebar.as_ref().is_some_and(|s| s.tab == Tab::Library) && panel::inside(l.content, pos) {
            return false; // the library grid handles it
        }
        // Leaving a text box commits it (a search stays as typed).
        if !matches!(action, Action::Search) {
            self.commit_sidebar_typing();
        }
        self.needs_redraw = true;
        match action {
            Action::Tab(t) => {
                if let Some(s) = &mut self.sidebar {
                    s.tab = t;
                }
                if t == Tab::Library {
                    self.ensure_library_view();
                }
            }
            Action::Close => self.close_sidebar(),
            Action::Fold(id) => {
                if let Some(s) = &mut self.sidebar {
                    if !s.folded.remove(&id) {
                        s.folded.insert(id);
                    }
                }
            }
            Action::New => {
                // Inside the open board, as a card in the middle of the view.
                let saved = self.cursor;
                self.cursor = [WIDTH + (self.viewport[0] - WIDTH) / 2.0, self.viewport[1] / 2.0];
                let id = self.new_board();
                self.cursor = saved;
                if let Some(s) = &mut self.sidebar {
                    s.search.clear();
                    s.typing = Some((Field::Rename(id), String::new()));
                }
            }
            Action::Search => {
                if let Some(s) = &mut self.sidebar {
                    if s.typing.is_none() {
                        s.typing = Some((Field::Search, s.search.clone()));
                    }
                }
            }
            Action::Row(id) => {
                let double = self
                    .last_click
                    .is_some_and(|(t, p)| self.now_ms.saturating_sub(t) <= crate::DOUBLE_CLICK_MS && (p[0] - pos[0]).abs() < 6.0 && (p[1] - pos[1]).abs() < 6.0);
                self.last_click = Some((self.now_ms, pos));
                if double {
                    let name = self.board_names.get(&id).cloned().unwrap_or_default();
                    if let Some(s) = &mut self.sidebar {
                        s.typing = Some((Field::Rename(id), name));
                    }
                } else {
                    self.gesture = crate::Gesture::SideRow { id, start: pos, moved: false };
                }
            }
            Action::Delete(id) => {
                if let Some(s) = &mut self.sidebar {
                    s.confirm = Some(id);
                }
            }
            Action::ConfirmDelete(id) => {
                if let Some(s) = &mut self.sidebar {
                    s.confirm = None;
                }
                self.delete_board(id);
            }
            Action::CancelDelete => {
                if let Some(s) = &mut self.sidebar {
                    s.confirm = None;
                }
            }
            Action::None => {}
        }
        true
    }

    /// After undo or redo on the open board: items moved away from it that
    /// are back here are taken off the other board, and the other way round.
    pub(crate) fn sync_moved_out(&mut self) {
        let here = self.doc.board.id;
        let moves: Vec<MovedOut> = self.moved_out.iter().filter(|m| m.from == here).cloned().collect();
        for m in moves {
            let back = m.originals.iter().all(|id| self.doc.item(id).is_some_and(|i| !i.deleted));
            let away = m.originals.iter().all(|id| self.doc.item(id).is_none_or(|i| i.deleted));
            if !back && !away {
                continue;
            }
            self.edit_board(m.target, |doc| {
                if back {
                    let live: std::collections::BTreeSet<Uuid> = m.copies.iter().filter(|id| doc.item(id).is_some_and(|i| !i.deleted)).copied().collect();
                    if !live.is_empty() {
                        doc.delete(&live);
                    }
                } else {
                    doc.restore(&m.copies);
                }
            });
        }
    }

    /// A board and every board inside it, the board first.
    pub fn board_and_descendants(&self, id: Uuid) -> Vec<Uuid> {
        let mut out = vec![id];
        let mut k = 0;
        while k < out.len() {
            let at = out[k];
            let mut kids: Vec<Uuid> = self.board_meta.iter().filter(|(c, m)| m.parent == Some(at) && !out.contains(c)).map(|(c, _)| *c).collect();
            kids.sort();
            out.extend(kids);
            k += 1;
        }
        out
    }

    /// Runs `f` on any board's document (open, kept in memory, or read from
    /// the library) and saves it. False if the board can't be found or saved.
    fn edit_board(&mut self, id: Uuid, f: impl FnOnce(&mut crate::doc::Document)) -> bool {
        if id == self.doc.board.id {
            f(&mut self.doc);
            self.scene.invalidate();
            if let Some(lib) = &self.library {
                return lib.write_board(&self.doc.board).map_err(|e| eprintln!("wall: could not save: {e}")).is_ok();
            }
            return true;
        }
        let found = match self.parked.remove(&id) {
            Some(p) => Some(p),
            None => self.library.as_ref().and_then(|l| l.read_board(id).ok()).map(|b| (crate::doc::Document::new(b, self.doc.device.clone()), crate::Camera { center: [0.0, 0.0], zoom: 1.0 })),
        };
        let Some((mut doc, camera)) = found else { return false };
        doc.set_clock(self.now_ms);
        f(&mut doc);
        let saved = match &self.library {
            Some(lib) => lib.write_board(&doc.board).map_err(|e| eprintln!("wall: could not save board {id}: {e}")).is_ok(),
            None => true,
        };
        if let Some(m) = self.board_meta.get_mut(&id) {
            m.parent = doc.board.parent;
        }
        self.parked.insert(id, (doc, camera));
        saved
    }

    /// Puts a board inside another one: its card leaves the old parent and
    /// a new card goes to the right of the new parent's things. Not into
    /// itself or a board inside it, and not the library's top board.
    pub fn move_board(&mut self, id: Uuid, into: Uuid) -> bool {
        let Some(old) = self.board_meta.get(&id).and_then(|m| m.parent) else { return false };
        if old == into || self.board_and_descendants(id).contains(&into) || !self.board_meta.contains_key(&into) {
            return false;
        }
        self.end_edit();
        // The card on the old parent goes, as one undo step there.
        let card = |doc: &crate::doc::Document| -> std::collections::BTreeSet<Uuid> {
            doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == id)).map(|i| i.id).collect()
        };
        let old_frame = |doc: &crate::doc::Document| doc.board.live_items().find(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == id)).map(|i| i.frame);
        let mut frame = None;
        if !self.edit_board(old, |doc| {
            frame = old_frame(doc);
            let ids = card(doc);
            doc.delete(&ids);
        }) {
            return false;
        }
        let mut f = frame.unwrap_or(wall_format::Frame { w: 220.0, h: 140.0, ..Default::default() });
        self.edit_board(into, |doc| {
            let right = doc.board.live_items().map(|i| i.frame.x + i.frame.w).fold(f64::MIN, f64::max);
            let top = doc.board.live_items().map(|i| i.frame.y).fold(f64::MAX, f64::min);
            if right > f64::MIN {
                f.x = right + 80.0;
                f.y = top;
            }
            doc.add(vec![(f, ItemKind::Board { board_id: id })]);
        });
        self.edit_board(id, |doc| doc.board.parent = Some(into));
        if let Some(m) = self.board_meta.get_mut(&id) {
            m.parent = Some(into);
        }
        // The breadcrumb now leads through the new parent.
        if self.board_and_descendants(id).contains(&self.doc.board.id) {
            let at = self.doc.board.id;
            self.nav.clear();
            let mut p = self.board_meta.get(&at).and_then(|m| m.parent);
            while let Some(b) = p {
                if self.nav.contains(&b) {
                    break;
                }
                self.nav.insert(0, b);
                p = self.board_meta.get(&b).and_then(|m| m.parent);
            }
        }
        self.needs_redraw = true;
        true
    }

    /// Deletes a board and the boards inside it: their files go to the
    /// library's trash folder and the card leaves the parent board. If one
    /// of them is open, the parent opens first. Not the library's top board.
    pub fn delete_board(&mut self, id: Uuid) -> bool {
        let Some(parent) = self.board_meta.get(&id).and_then(|m| m.parent) else { return false };
        let gone = self.board_and_descendants(id);
        self.end_edit();
        if gone.contains(&self.doc.board.id) {
            self.go_to_board(parent);
            if gone.contains(&self.doc.board.id) {
                return false;
            }
        }
        self.edit_board(parent, |doc| {
            let ids: std::collections::BTreeSet<Uuid> = doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == id)).map(|i| i.id).collect();
            doc.delete(&ids);
        });
        for b in &gone {
            self.parked.remove(b);
            if let Some(lib) = &self.library {
                if let Err(e) = lib.trash_board(*b, self.now_ms) {
                    eprintln!("wall: could not move board {b} to the trash: {e}");
                }
            }
            self.board_meta.remove(b);
            self.board_names.remove(b);
        }
        self.nav.retain(|b| !gone.contains(b));
        self.board_order.retain(|b| !gone.contains(b));
        self.needs_redraw = true;
        true
    }

    /// The pointer let go after pressing a board row.
    pub(crate) fn sidebar_release(&mut self, id: Uuid, moved: bool, pos: [f32; 2]) {
        self.needs_redraw = true;
        if !moved {
            self.go_to_board(id);
            return;
        }
        match self.side_rows_at(pos).filter(|(t, _)| *t != id) {
            Some((target, Some(after))) => {
                // Next to a board: into that board's parent if it differs.
                let parent = self.board_meta.get(&target).and_then(|m| m.parent);
                if let Some(p) = parent.filter(|p| self.board_meta.get(&id).and_then(|m| m.parent) != Some(*p)) {
                    self.move_board(id, p);
                }
                self.reorder_board(id, target, after);
            }
            Some((target, None)) => {
                self.move_board(id, target);
            }
            None => {}
        }
    }

    /// Opens any board, with the breadcrumb leading to it.
    pub fn go_to_board(&mut self, id: Uuid) {
        if id == self.doc.board.id {
            return;
        }
        let mut chain = Vec::new();
        let mut at = self.board_meta.get(&id).and_then(|m| m.parent);
        while let Some(p) = at {
            if chain.contains(&p) || p == id {
                break;
            }
            chain.push(p);
            at = self.board_meta.get(&p).and_then(|m| m.parent);
        }
        chain.reverse();
        let before = self.doc.board.id;
        self.open_board(id, false);
        if self.doc.board.id != before {
            self.nav = chain;
        }
    }

    /// Puts `id` just before or after `target` when both have the same
    /// parent (boards keep their parent; only the order changes).
    pub fn reorder_board(&mut self, id: Uuid, target: Uuid, after: bool) {
        let parent = |a: &Self, b: &Uuid| a.board_meta.get(b).map(|m| m.parent);
        if parent(self, &id) != parent(self, &target) || parent(self, &id).is_none() && !self.board_meta.contains_key(&id) {
            return;
        }
        let mut order: Vec<Uuid> = tree_order(&self.board_meta, &self.board_order).into_iter().map(|(b, _)| b).collect();
        // Move the board on its own: its sub-boards follow it through the
        // parent links anyway.
        order.retain(|b| *b != id);
        let Some(k) = order.iter().position(|b| *b == target) else { return };
        order.insert(if after { k + 1 } else { k }, id);
        self.board_order = order.clone();
        if let Some(lib) = &mut self.library {
            if let Err(e) = lib.set_board_order(order) {
                eprintln!("wall: could not save the board order: {e}");
            }
        }
    }

    /// Moves the selected items (with the arrows between them) to another
    /// board, to the right of what is already there. Arrows to items that
    /// stay behind are left out.
    pub fn move_selection_to_board(&mut self, target: Uuid) -> usize {
        if target == self.doc.board.id || self.selection.is_empty() {
            return 0;
        }
        self.end_edit();
        // Lines drawn on the pictures go along.
        let on: Vec<Uuid> = self.lines_on(&self.selection).iter().map(|i| i.id).collect();
        self.selection.extend(on);
        let picked: Vec<wall_format::BoardItem> = self.selection.iter().filter_map(|id| self.doc.item(id).cloned()).filter(|i| !matches!(i.kind, ItemKind::Arrow { .. })).collect();
        if picked.is_empty() {
            return 0;
        }
        let ids: std::collections::BTreeSet<Uuid> = picked.iter().map(|i| i.id).collect();
        let arrows: Vec<wall_format::BoardItem> = self
            .doc
            .board
            .live_items()
            .filter(|i| match &i.kind {
                ItemKind::Arrow { from, to, .. } => {
                    let ok = |e: &wall_format::ArrowEnd| match e {
                        wall_format::ArrowEnd::Item { id } => ids.contains(id),
                        wall_format::ArrowEnd::Point { .. } => true,
                    };
                    ok(from) && ok(to) && (matches!(from, wall_format::ArrowEnd::Item { .. }) || matches!(to, wall_format::ArrowEnd::Item { .. }))
                }
                _ => false,
            })
            .cloned()
            .collect();
        // The other board, from memory or from the library.
        let target_doc = match self.parked.remove(&target) {
            Some(p) => Some(p),
            None => self
                .library
                .as_ref()
                .and_then(|l| l.read_board(target).ok())
                .map(|b| (crate::doc::Document::new(b, self.doc.device.clone()), crate::Camera { center: [0.0, 0.0], zoom: 1.0 })),
        };
        let Some((mut doc, camera)) = target_doc else { return 0 };
        doc.set_clock(self.now_ms);
        // Line them up to the right of the other board's things.
        let bounds = |items: &mut dyn Iterator<Item = &wall_format::BoardItem>| {
            items.fold(None, |b: Option<[f64; 4]>, i| {
                let f = i.frame;
                Some(match b {
                    None => [f.x, f.y, f.x + f.w, f.y + f.h],
                    Some(b) => [b[0].min(f.x), b[1].min(f.y), b[2].max(f.x + f.w), b[3].max(f.y + f.h)],
                })
            })
        };
        let ours = bounds(&mut picked.iter()).unwrap_or([0.0; 4]);
        let (dx, dy) = match bounds(&mut doc.board.live_items()) {
            Some(theirs) => (theirs[2] + 120.0 - ours[0], theirs[1] - ours[1]),
            None => (0.0, 0.0),
        };
        let shifted = |i: &wall_format::BoardItem| {
            let mut i = i.clone();
            crate::doc::shift(&mut i, dx, dy);
            i
        };
        let new_ids = doc.add_copies(&picked.iter().map(shifted).collect::<Vec<_>>());
        let map: HashMap<Uuid, Uuid> = picked.iter().map(|i| i.id).zip(new_ids.iter().copied()).collect();
        let remap = |e: &wall_format::ArrowEnd| match e {
            wall_format::ArrowEnd::Item { id } => wall_format::ArrowEnd::Item { id: map[id] },
            p => p.clone(),
        };
        let arrow_kinds: Vec<_> = arrows
            .iter()
            .map(|a| {
                let mut s = shifted(a);
                if let ItemKind::Arrow { from, to, .. } = &mut s.kind {
                    *from = remap(from);
                    *to = remap(to);
                }
                (s.frame, s.kind)
            })
            .collect();
        let mut copies: Vec<Uuid> = new_ids.clone();
        if !arrow_kinds.is_empty() {
            copies.extend(doc.add(arrow_kinds));
        }
        let saved = match &self.library {
            Some(lib) => lib.write_board(&doc.board).map_err(|e| eprintln!("wall: could not save the other board: {e}")).is_ok(),
            None => true,
        };
        if let Some(m) = self.board_meta.get_mut(&target) {
            m.thumb = thumb_of(&doc.board);
            m.seen_ms = self.now_ms;
        }
        if self.library.is_none() {
            self.parked.insert(target, (doc, camera));
        }
        if !saved {
            return 0;
        }
        let mut gone = ids;
        gone.extend(arrows.iter().map(|a| a.id));
        self.doc.delete(&gone);
        self.moved_out.push(MovedOut { from: self.doc.board.id, originals: gone.into_iter().collect(), target, copies });
        self.selection.clear();
        self.needs_redraw = true;
        map.len()
    }

    /// Types into the panel's search or rename box. False if not typing there.
    pub(crate) fn sidebar_text(&mut self, s: &str) -> bool {
        let Some((_, text)) = self.sidebar.as_mut().and_then(|b| b.typing.as_mut()) else { return false };
        text.push_str(&s.replace(['\n', '\r', '\t'], " "));
        if let Some(b) = &mut self.sidebar {
            b.scroll = 0;
        }
        self.needs_redraw = true;
        true
    }

    /// Keys while typing in the panel. False if not typing there.
    pub(crate) fn sidebar_key(&mut self, key: crate::Key) -> bool {
        use crate::Key;
        let Some(b) = self.sidebar.as_mut() else { return false };
        let Some((field, text)) = b.typing.as_mut() else { return false };
        match key {
            Key::Escape => {
                if *field == Field::Search {
                    b.search.clear();
                }
                b.typing = None;
            }
            Key::Enter => self.commit_sidebar_typing(),
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) => text.push(c),
            _ => {}
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn sidebar_typing(&self) -> bool {
        self.sidebar.as_ref().is_some_and(|s| s.typing.is_some())
    }

    fn commit_sidebar_typing(&mut self) {
        let Some((field, text)) = self.sidebar.as_mut().and_then(|s| s.typing.take()) else { return };
        match field {
            Field::Search => {
                if let Some(s) = &mut self.sidebar {
                    s.search = text.trim().to_owned();
                }
            }
            Field::Rename(id) => {
                let name = text.trim().to_owned();
                if name.is_empty() {
                } else if id == self.doc.board.id {
                    self.doc.board.name = name.clone();
                    self.doc.version += 1;
                    self.board_names.insert(id, name);
                } else {
                    self.rename_board(id, name);
                }
            }
        }
        self.needs_redraw = true;
    }

    /// Wheel over the panel scrolls it. True if it was over the panel.
    pub(crate) fn sidebar_wheel(&mut self, steps: f32) -> bool {
        if self.sidebar.is_none() || self.cursor[0] > WIDTH || self.cursor[1] < BAR {
            return false;
        }
        if self.sidebar_tab() == Some(Tab::Library) {
            self.library_wheel(steps);
            return true;
        }
        let Some(l) = self.sidebar_layout() else { return true };
        if let Some(s) = &mut self.sidebar {
            let rows = if steps > 0.0 { -2 } else if steps < 0.0 { 2 } else { 0 };
            s.scroll = (s.scroll as i32 + rows).clamp(0, l.max_scroll as i32) as usize;
        }
        self.needs_redraw = true;
        true
    }

    /// Draws the panel, decoding a few missing thumbnails per frame.
    pub(crate) fn draw_sidebar(&mut self, renderer: &mut wall_render::Renderer) {
        let Some(l) = self.sidebar_layout() else { return };
        let mut budget = 3;
        for (hash, _) in &l.images {
            if self.board_thumbs.contains_key(hash) || matches!(self.pictures.get(hash), Some(Picture::Ready(_) | Picture::Preview(_))) {
                continue;
            }
            if budget == 0 {
                self.needs_redraw = true;
                break;
            }
            let Some(lib) = self.library.as_ref() else { break };
            let Some(sc) = self.sidecars.get(hash).cloned().or_else(|| lib.read_sidecar(hash).ok()) else {
                self.board_thumbs.insert(hash.clone(), Thumb::Failed);
                continue;
            };
            if sc.kind == MediaKind::Video {
                // A frame comes from the shell (sidebar_posters_wanted).
                continue;
            }
            budget -= 1;
            // The tiny cached image if there is one, else the picture itself.
            let tiny = lib.root().join("cache").join("previews").join(format!("{hash}.t.jpg"));
            let thumb = std::fs::read(&tiny)
                .ok()
                .and_then(|b| crate::library_view::decode_thumb(&b))
                .or_else(|| std::fs::read(lib.media_path(hash, &sc.ext)).ok().and_then(|b| crate::library_view::decode_thumb(&b)));
            self.board_thumbs.insert(hash.clone(), match thumb {
                Some((rgba, width, height)) => Thumb::Pending { rgba, width, height },
                None => Thumb::Failed,
            });
        }
        for t in self.board_thumbs.values_mut() {
            if let Thumb::Pending { rgba, width, height } = t {
                *t = match renderer.create_image(rgba, *width, *height) {
                    Ok(id) => Thumb::Ready(id),
                    Err(_) => Thumb::Failed,
                };
            }
        }
        let view = wall_render::ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let to_box = |b: &PanelBox| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] };
        renderer.draw_boxes(&view, &l.boxes.iter().map(to_box).collect::<Vec<_>>());
        let images: Vec<_> = l
            .images
            .iter()
            .filter_map(|(hash, rect)| {
                let id = match (self.board_thumbs.get(hash), self.pictures.get(hash)) {
                    (_, Some(Picture::Ready(id) | Picture::Preview(id))) | (Some(Thumb::Ready(id)), _) => *id,
                    _ => return None,
                };
                // Fill the little frame, cropping the picture to it.
                Some((id, wall_render::ImageInstance { rect: *rect, params: [1.0, 4.0, 0.0, 0.0], uv_rect: [0.0, 0.0, 1.0, 1.0] }))
            })
            .collect();
        renderer.draw_images(&view, &images);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            crate::scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
        if let Some(c) = l.caret {
            renderer.draw_boxes(&view, &[wall_render::BoxInstance { rect: c, fill: theme::t().ink, stroke: [0.0; 4], shape: [0.0, 0.0] }]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(parent: Option<Uuid>, created: u64) -> BoardMeta {
        BoardMeta { parent, created_ms: created, seen_ms: created, thumb: None }
    }

    use crate::{Button, Key, Mods};
    use wall_format::Frame;

    fn app() -> App {
        let mut a = App::new(Board::new("Home", 0), "test");
        a.resize([1200.0, 800.0]);
        a.camera = crate::Camera { center: [600.0, 400.0], zoom: 1.0 };
        a.panel_open = false;
        a
    }

    fn click(a: &mut App, p: [f32; 2]) {
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
    }

    fn row_centre(a: &mut App, id: Uuid) -> [f32; 2] {
        let l = a.sidebar_layout().unwrap();
        let r = l.rows.iter().find(|(_, b)| *b == id).unwrap().0;
        [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
    }

    #[test]
    fn panel_pushes_the_board_lists_boards_and_makes_renames_and_opens_them() {
        let mut a = app();
        let home = a.board_id();
        let before = a.camera.center[0];
        a.key(Key::Char('s'));
        assert_eq!(a.sidebar_tab(), Some(Tab::Boards));
        assert_eq!(before - a.camera.center[0], WIDTH, "the board moved right by the panel's width");
        // "+" makes a board inside this one and starts naming it.
        let plus = a.sidebar_layout().unwrap().hits.iter().find(|(_, x)| *x == Action::New).unwrap().0;
        a.set_time(1_000);
        click(&mut a, [plus[0] + 5.0, plus[1] + 5.0]);
        a.text_input("Refs");
        a.key(Key::Enter);
        let refs = *a.board_meta.keys().find(|id| **id != home).unwrap();
        assert_eq!(a.board_names[&refs], "Refs");
        assert_eq!(a.board_meta[&refs].parent, Some(home));
        let tree = a.tree_rows();
        assert_eq!(tree.iter().map(|r| (r.name.as_str(), r.depth)).collect::<Vec<_>>(), vec![("Home", 0), ("Refs", 1)]);
        // A click opens it, with the breadcrumb back to Home.
        let p = row_centre(&mut a, refs);
        a.set_time(5_000);
        click(&mut a, p);
        assert_eq!(a.board_id(), refs);
        assert_eq!(a.breadcrumb(), vec!["Home".to_owned(), "Refs".to_owned()]);
        // Double-click renames.
        let p = row_centre(&mut a, refs);
        a.set_time(9_000);
        click(&mut a, p);
        a.set_time(9_100);
        a.pointer_down(Button::Primary, p, Mods::default());
        for _ in 0..4 {
            a.key(Key::Backspace);
        }
        a.text_input("Lighting");
        a.key(Key::Enter);
        assert_eq!(a.board_names[&refs], "Lighting");
        assert_eq!(a.doc.board.name, "Lighting");
        // Search narrows the list.
        a.sidebar.as_mut().unwrap().search = "light".into();
        assert_eq!(a.tree_rows().iter().map(|r| r.id).collect::<Vec<_>>(), vec![refs]);
        // S again closes it and gives the room back.
        let c = a.camera.center[0];
        a.key(Key::Char('s'));
        assert!(a.sidebar.is_none());
        assert_eq!(a.camera.center[0] - c, WIDTH);
    }

    #[test]
    fn dragging_a_row_reorders_sisters_and_dropping_pictures_moves_them() {
        let mut a = app();
        let home = a.board_id();
        a.set_time(10);
        let b1 = a.new_board();
        a.set_time(20);
        let b2 = a.new_board();
        a.toggle_sidebar(Tab::Boards);
        let order = |a: &App| tree_order(&a.board_meta, &a.board_order).into_iter().map(|(b, _)| b).collect::<Vec<_>>();
        assert_eq!(order(&a), vec![home, b1, b2]);
        // Drag Board 2 above Board 1.
        let from = row_centre(&mut a, b2);
        let to = row_centre(&mut a, b1);
        a.pointer_down(Button::Primary, from, Mods::default());
        a.pointer_move([from[0], to[1] - 8.0]);
        a.pointer_up(Button::Primary, [from[0], to[1] - 8.0]);
        assert_eq!(order(&a), vec![home, b2, b1]);
        assert_eq!(a.board_id(), home, "a drag doesn't open the board");

        // A note on the board, dragged onto Board 1's row, moves there.
        a.selection.clear();
        let note = a.doc.add(vec![(Frame { x: 700.0, y: 300.0, w: 200.0, h: 80.0, ..Default::default() }, ItemKind::Note { text: "move me".into(), color: None })])[0];
        let c = a.camera.center;
        let start = [800.0 - c[0] + 600.0, 340.0 - c[1] + 400.0];
        let target = row_centre(&mut a, b1);
        a.pointer_down(Button::Primary, start, Mods::default());
        a.pointer_move([start[0] - 50.0, start[1]]);
        a.pointer_move(target);
        a.pointer_up(Button::Primary, target);
        assert!(a.doc.item(&note).is_none_or(|i| i.deleted), "gone from this board");
        let moved = |a: &App| a.parked[&b1].0.board.live_items().filter(|i| matches!(&i.kind, ItemKind::Note { text, .. } if text == "move me")).count();
        assert_eq!(moved(&a), 1);
        // Undo brings it back here and takes the copy off Board 1; redo moves it again.
        a.key(Key::Undo);
        assert!(a.doc.item(&note).is_some_and(|i| !i.deleted));
        assert_eq!(moved(&a), 0, "no copy left behind");
        a.key(Key::Redo);
        assert!(a.doc.item(&note).is_none_or(|i| i.deleted));
        assert_eq!(moved(&a), 1);
    }

    #[test]
    fn a_board_dragged_onto_another_goes_inside_it_and_the_cross_deletes_it_after_asking() {
        let mut a = app();
        let home = a.board_id();
        a.set_time(10);
        let b1 = a.new_board();
        a.set_time(20);
        let b2 = a.new_board();
        a.open_board(b1, true);
        a.set_time(30);
        let inner = a.new_board();
        a.toggle_sidebar(Tab::Boards);
        let cards = |a: &App, board: Uuid, id: Uuid| -> usize {
            let doc = if a.doc.board.id == board { &a.doc } else { &a.parked[&board].0 };
            doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == id)).count()
        };
        assert_eq!(cards(&a, b1, inner), 1);
        // Drag "inner" onto the middle of Board 2's row: it moves inside.
        let from = row_centre(&mut a, inner);
        let to = row_centre(&mut a, b2);
        a.pointer_down(Button::Primary, from, Mods::default());
        a.pointer_move(to);
        a.pointer_up(Button::Primary, to);
        assert_eq!(a.board_meta[&inner].parent, Some(b2));
        assert_eq!((cards(&a, b1, inner), cards(&a, b2, inner)), (0, 1));
        assert_eq!(a.parked[&inner].0.board.parent, Some(b2), "saved on the board too");
        // Not into a board inside itself, and the top board stays put.
        assert!(!a.move_board(b2, inner));
        assert!(!a.move_board(home, b1));

        // Open "inner", then delete Board 2 from the panel: asks first, then
        // both go and the top board opens.
        a.go_to_board(inner);
        assert_eq!(a.nav, vec![home, b2]);
        let at = row_centre(&mut a, b2);
        a.pointer_move(at);
        let l = a.sidebar_layout().unwrap();
        let cross = l.hits.iter().find(|(_, x)| *x == Action::Delete(b2)).expect("× on hover").0;
        click(&mut a, [cross[0] + 5.0, cross[1] + 5.0]);
        assert_eq!(a.sidebar.as_ref().unwrap().confirm, Some(b2));
        assert!(a.board_meta.contains_key(&b2), "nothing deleted yet");
        let l = a.sidebar_layout().unwrap();
        let yes = l.hits.iter().find(|(_, x)| *x == Action::ConfirmDelete(b2)).unwrap().0;
        click(&mut a, [yes[0] + 5.0, yes[1] + 5.0]);
        assert!(!a.board_meta.contains_key(&b2) && !a.board_meta.contains_key(&inner));
        assert_eq!(a.board_id(), home);
        assert_eq!(cards(&a, home, b2), 0);
        assert!(a.board_meta.contains_key(&b1));
    }

    #[test]
    fn the_rail_moves_by_its_handle_and_folds_away() {
        let mut a = app();
        let grip = a.rail_layout().grip;
        a.pointer_down(Button::Primary, [grip[0] + 5.0, grip[1] + 5.0], Mods::default());
        a.pointer_move([700.0, 400.0]);
        a.pointer_up(Button::Primary, [700.0, 400.0]);
        let r = a.rail_layout().rect;
        assert!((r[0] - 678.0).abs() < 1.0, "moved: {r:?}");
        let setting = a.rail_setting();
        let fold = a.rail_layout().fold;
        click(&mut a, [fold[0] + 5.0, fold[1] + 5.0]);
        assert!(a.rail_layout().hits.is_empty(), "folded: no tools showing");
        let mut b = app();
        b.set_rail_setting(&setting);
        assert_eq!(b.rail_layout().rect, r, "a saved setting puts it back");
        // The rail's boards button opens the panel, and the rail keeps clear.
        let side = b.rail_layout().side;
        click(&mut b, [side[0] + 5.0, side[1] + 5.0]);
        assert_eq!(b.sidebar_tab(), Some(Tab::Boards));
        b.rail_place = crate::toolbar::Placement::default();
        assert!(b.rail_layout().rect[0] >= WIDTH);
    }

    #[test]
    fn tree_lists_sub_boards_under_their_parent_in_the_saved_order() {
        let (root, a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let m: HashMap<Uuid, BoardMeta> = [(root, meta(None, 1)), (a, meta(Some(root), 2)), (b, meta(Some(root), 3)), (c, meta(Some(a), 4))].into();
        assert_eq!(tree_order(&m, &[]), vec![(root, 0), (a, 1), (c, 2), (b, 1)]);
        assert_eq!(tree_order(&m, &[b]), vec![(root, 0), (b, 1), (a, 1), (c, 2)], "listed boards come first");
        // A loop in the parent links doesn't hang or drop boards.
        let m2: HashMap<Uuid, BoardMeta> = [(a, meta(Some(b), 1)), (b, meta(Some(a), 2))].into();
        assert_eq!(tree_order(&m2, &[]).len(), 2);
    }
}
