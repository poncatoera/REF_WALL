//! New comments: a comment is new to someone while its newest message is
//! someone else's and came after they last had it open. New ones get a red
//! dot on their pin, the boards they're on get a count in the boards list,
//! and a pill at the top says how many there are and goes to the next one.
//!
//! Nothing is sent anywhere: what each person has seen stays on their own
//! computer or browser, kept by the shell through [`App::seen_setting`].
//! So it costs nothing on the server and works on view links too.

use std::collections::HashMap;

use uuid::Uuid;
use wall_format::{Board, ItemKind};
use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::menu::BAR;
use crate::panel::{self, PanelBox, TextRun};
use crate::{scene, theme, App};

/// The red of the dots and the pill's dot.
pub const RED: [f32; 4] = [0.90, 0.27, 0.27, 1.0];
/// The most comments remembered as seen; the oldest go first.
const KEEP: usize = 4000;

/// An open (not resolved) comment's newest message.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    pub id: Uuid,
    pub at_ms: u64,
    pub by: String,
    /// Where the pin is.
    pub at: [f32; 2],
}

/// The open comments on a board, by their newest message.
pub fn tips(board: &Board) -> Vec<Tip> {
    board
        .live_items()
        .filter_map(|i| match &i.kind {
            ItemKind::Comment { messages, resolved: false } => messages.last().map(|m| Tip { id: i.id, at_ms: m.at_ms, by: m.by.clone(), at: [i.frame.x as f32, i.frame.y as f32] }),
            _ => None,
        })
        .collect()
}

/// What this person has seen.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// Everything before this was there before Wall kept track: not new.
    pub since: u64,
    /// Comment -> its newest message's time when it was last open here.
    pub at: HashMap<Uuid, u64>,
    /// Bumped on every change, so shells know when to save.
    pub stamp: u64,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl Seen {
    pub fn is_new(&self, tip: &Tip, me: &str) -> bool {
        !same_name(&tip.by, me) && tip.at_ms > self.since.max(self.at.get(&tip.id).copied().unwrap_or(0))
    }

    fn mark(&mut self, tip: &Tip) {
        if self.at.get(&tip.id).is_some_and(|t| *t >= tip.at_ms) {
            return;
        }
        self.at.insert(tip.id, tip.at_ms);
        if self.at.len() > KEEP {
            let mut times: Vec<u64> = self.at.values().copied().collect();
            times.sort_unstable();
            let cut = times[self.at.len() - KEEP];
            self.at.retain(|_, t| *t >= cut);
        }
        self.stamp += 1;
    }

    /// "since;id=at,id=at".
    pub fn to_setting(&self) -> String {
        let mut entries: Vec<(&Uuid, &u64)> = self.at.iter().collect();
        entries.sort();
        let list: Vec<String> = entries.iter().map(|(id, at)| format!("{id}={at}")).collect();
        format!("{};{}", self.since, list.join(","))
    }

    pub fn from_setting(s: &str) -> Option<Seen> {
        let (since, list) = s.split_once(';')?;
        let since = since.parse().ok()?;
        let at = list
            .split(',')
            .filter_map(|e| {
                let (id, at) = e.split_once('=')?;
                Some((id.parse().ok()?, at.parse().ok()?))
            })
            .collect();
        Some(Seen { since, at, stamp: 0 })
    }
}

/// The pill at the top.
#[derive(Default)]
pub struct Pill {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
}

impl App {
    fn me_for_comments(&self) -> String {
        self.my_name.clone().unwrap_or_else(|| "Me".into())
    }

    /// What was seen, for the shell to keep ("" until anything was).
    pub fn seen_setting(&self) -> String {
        self.seen.to_setting()
    }

    /// Changes whenever [`Self::seen_setting`] does.
    pub fn seen_stamp(&self) -> u64 {
        self.seen.stamp
    }

    /// Restores [`Self::seen_setting`]. With nothing saved (the first run
    /// with this), whatever is there now counts as seen.
    pub fn set_seen_setting(&mut self, s: Option<&str>, now_ms: u64) {
        self.seen = s.and_then(Seen::from_setting).unwrap_or(Seen { since: now_ms, ..Default::default() });
        self.seen.stamp += 1;
        self.needs_redraw = true;
    }

    /// The new comments on the open board, top to bottom.
    pub fn new_comments_here(&self) -> Vec<Tip> {
        let me = self.me_for_comments();
        let mut new: Vec<Tip> = tips(&self.doc.board).into_iter().filter(|t| self.seen.is_new(t, &me)).collect();
        new.sort_by(|a, b| a.at[1].total_cmp(&b.at[1]).then(a.at[0].total_cmp(&b.at[0])));
        new
    }

    /// How many new comments a board has.
    pub fn new_comments_on(&self, board: Uuid) -> usize {
        if board == self.doc.board.id {
            return self.new_comments_here().len();
        }
        let me = self.me_for_comments();
        self.board_meta.get(&board).map_or(0, |m| m.tips.iter().filter(|t| self.seen.is_new(t, &me)).count())
    }

    /// Other boards with new comments, the newest first: (board, how many).
    fn new_elsewhere(&self) -> Vec<(Uuid, usize, u64)> {
        let me = self.me_for_comments();
        let here = self.doc.board.id;
        let mut out: Vec<(Uuid, usize, u64)> = self
            .board_meta
            .iter()
            .filter(|(id, _)| **id != here)
            .filter_map(|(id, m)| {
                let new: Vec<&Tip> = m.tips.iter().filter(|t| self.seen.is_new(t, &me)).collect();
                let newest = new.iter().map(|t| t.at_ms).max()?;
                Some((*id, new.len(), newest))
            })
            .collect();
        out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        out
    }

    /// The comment open on the card has been seen, as it is now.
    pub(crate) fn mark_open_comment_seen(&mut self) {
        let Some(id) = self.open_comment() else { return };
        if let Some(tip) = tips(&self.doc.board).into_iter().find(|t| t.id == id) {
            let before = self.seen.stamp;
            self.seen.mark(&tip);
            if self.seen.stamp != before {
                self.needs_redraw = true;
            }
        }
    }

    /// Goes to the next new comment: on this board, else on the board
    /// with the newest. Opens it, which marks it seen.
    pub fn show_next_new_comment(&mut self) -> bool {
        if self.new_comments_here().is_empty() {
            let Some((board, ..)) = self.new_elsewhere().into_iter().next() else { return false };
            self.go_to_board(board);
            if self.doc.board.id != board {
                return false;
            }
        }
        let Some(tip) = self.new_comments_here().into_iter().next() else { return false };
        self.camera.center = tip.at;
        self.camera.zoom = self.camera.zoom.max(0.5);
        self.comment_card = Some(crate::comments::CommentCard { id: Some(tip.id), ..Default::default() });
        self.selection.clear();
        self.mark_open_comment_seen();
        self.needs_redraw = true;
        true
    }

    pub(crate) fn unread_pill_layout(&mut self) -> Option<Pill> {
        let here = self.new_comments_here().len();
        let text = if here > 0 {
            if here == 1 { "1 new comment".to_owned() } else { format!("{here} new comments") }
        } else {
            let elsewhere = self.new_elsewhere();
            let (board, n, _) = elsewhere.first()?;
            let total: usize = elsewhere.iter().map(|e| e.1).sum();
            let name = self.board_names.get(board).cloned().unwrap_or_else(|| "another board".into());
            if elsewhere.len() == 1 {
                format!("{} on “{name}”", if *n == 1 { "1 new comment".to_owned() } else { format!("{n} new comments") })
            } else {
                format!("{total} new comments on {} boards", elsewhere.len())
            }
        };
        let t = theme::t();
        let left = self.left_inset();
        let right = self.viewport[0];
        let font = &mut self.font;
        let mut width = |s: &str, size: f32| panel::text_width(font, s, size);
        let text = panel::fit_with(&mut width, &text, 12.5, (right - left - 160.0).max(80.0));
        let show = "Show";
        let w = 30.0 + width(&text, 12.5) + 14.0 + width(show, 12.5) + 14.0;
        let y = BAR + 8.0 + if self.tool == crate::toolbar::Tool::Stamp { 54.0 } else { 0.0 };
        let x = (left + (right - left - w) / 2.0).max(left + 8.0);
        let rect = [x, y, w, 30.0];
        let mut p = Pill { rect, ..Default::default() };
        p.boxes.push(theme::shadow(rect, 15.0));
        let over = panel::inside(rect, self.cursor);
        p.boxes.push(PanelBox { rect, fill: if over { t.hover } else { t.panel_bg }, stroke: t.line, radius: 15.0, stroke_w: 1.0 });
        p.boxes.push(PanelBox { rect: [x + 12.0, y + 11.0, 8.0, 8.0], fill: RED, stroke: [0.0; 4], radius: 4.0, stroke_w: 0.0 });
        let tw = width(&text, 12.5);
        p.texts.push(TextRun { text, at: [x + 28.0, y + 19.5], size: 12.5, color: t.ink });
        p.texts.push(TextRun { text: show.into(), at: [x + 28.0 + tw + 14.0, y + 19.5], size: 12.5, color: t.accent_text });
        Some(p)
    }

    /// A press on the pill: the next new comment.
    pub(crate) fn unread_pill_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(p) = self.unread_pill_layout() else { return false };
        if !panel::inside(p.rect, pos) {
            return false;
        }
        self.show_next_new_comment();
        true
    }

    pub(crate) fn draw_unread_pill(&mut self, renderer: &mut Renderer) {
        let Some(p) = self.unread_pill_layout() else { return };
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<BoxInstance> = p.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &p.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Mods};
    use wall_format::{CommentMessage, Frame};

    fn app() -> App {
        let mut a = App::new(Board::new("Moodboard", 0), "test");
        a.resize([1200.0, 800.0]);
        a.set_my_name(Some("Paul".into()));
        a.set_seen_setting(None, 1_000);
        a
    }

    fn comment(a: &mut App, at: [f64; 2], by: &str, ms: u64) -> Uuid {
        let frame = Frame { x: at[0], y: at[1], w: 0.0, h: 0.0, ..Default::default() };
        a.doc.add(vec![(frame, ItemKind::Comment { messages: vec![CommentMessage { by: by.into(), text: "hi".into(), at_ms: ms }], resolved: false })])[0]
    }

    fn reply(a: &mut App, id: Uuid, by: &str, ms: u64) {
        let mut it = a.doc.item(&id).cloned().unwrap();
        if let ItemKind::Comment { messages, .. } = &mut it.kind {
            messages.push(CommentMessage { by: by.into(), text: "more".into(), at_ms: ms });
        }
        a.doc.replace(vec![it]);
    }

    #[test]
    fn new_comments_are_counted_shown_and_marked_seen() {
        let mut a = app();
        let old = comment(&mut a, [0.0, 0.0], "Mia", 500);
        assert!(a.new_comments_here().is_empty(), "there before Wall kept track");
        let mine = comment(&mut a, [10.0, 10.0], "paul ", 2_000);
        let low = comment(&mut a, [100.0, 300.0], "Mia", 2_000);
        let high = comment(&mut a, [400.0, -200.0], "Client", 3_000);
        assert_eq!(a.new_comments_here().iter().map(|t| t.id).collect::<Vec<_>>(), [high, low], "top to bottom, mine aren't new");
        let pill = a.unread_pill_layout().unwrap();
        assert!(pill.texts[0].text == "2 new comments");

        // The pill opens the first and marks it seen.
        let r = pill.rect;
        a.pointer_down(Button::Primary, [r[0] + 10.0, r[1] + 10.0], Mods::default());
        a.pointer_up(Button::Primary, [r[0] + 10.0, r[1] + 10.0]);
        assert_eq!(a.open_comment(), Some(high));
        assert_eq!(a.camera.center, [400.0, -200.0]);
        assert_eq!(a.new_comments_here().len(), 1);

        // A reply from someone else makes it new again; my reply doesn't.
        a.comment_card = None;
        reply(&mut a, high, "Client", 4_000);
        assert_eq!(a.new_comments_here().len(), 2);
        reply(&mut a, low, "Paul", 4_100);
        assert_eq!(a.new_comments_here().iter().map(|t| t.id).collect::<Vec<_>>(), [high]);
        let _ = (old, mine);

        // Kept by the shell and restored.
        let s = a.seen_setting();
        let mut b = app();
        b.set_seen_setting(Some(&s), 99_999);
        assert_eq!(b.seen.since, 1_000);
        assert_eq!(b.seen.at.get(&high), Some(&3_000));
        assert!(Seen::from_setting("junk").is_none());
    }

    #[test]
    fn other_boards_count_in_the_list_and_the_pill_goes_there() {
        let mut a = app();
        let home = a.board_id();
        let other = a.new_board();
        a.open_board(other, true);
        let c = comment(&mut a, [50.0, 50.0], "Mia", 5_000);
        a.comment_card = None;
        a.mark_open_comment_seen();
        a.open_board(home, false);
        assert_eq!(a.new_comments_on(other), 1);
        assert_eq!(a.new_comments_on(home), 0);
        let pill = a.unread_pill_layout().unwrap();
        assert!(pill.texts[0].text.starts_with("1 new comment on"), "{}", pill.texts[0].text);
        assert!(a.show_next_new_comment());
        assert_eq!(a.board_id(), other);
        assert_eq!(a.open_comment(), Some(c));
        assert_eq!(a.new_comments_on(other), 0);
        a.comment_card = None;
        assert!(a.unread_pill_layout().is_none());
        // Resolved comments aren't new.
        let d = comment(&mut a, [0.0, 0.0], "Mia", 6_000);
        let mut it = a.doc.item(&d).cloned().unwrap();
        if let ItemKind::Comment { resolved, .. } = &mut it.kind {
            *resolved = true;
        }
        a.doc.replace(vec![it]);
        assert!(a.new_comments_here().is_empty());
    }

    #[test]
    fn the_seen_list_stays_small() {
        let mut s = Seen::default();
        for k in 0..(KEEP as u64 + 50) {
            s.mark(&Tip { id: Uuid::new_v4(), at_ms: k, by: "x".into(), at: [0.0; 2] });
        }
        assert!(s.at.len() <= KEEP);
        assert!(s.at.values().all(|t| *t >= 50));
    }
}
