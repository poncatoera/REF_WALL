//! Share (File > Share this board…): a card over the board that makes
//! private links to the open board, to view or to edit, copies them, and
//! lists the board's links so they can be turned off. A link can be made
//! to stop by itself after a day, a week or a month.
//!
//! The app doesn't talk to the server itself: it asks the shell with
//! [`ShellCommand::Cloud`] and the shell answers with
//! [`App::cloud_answer`]. Only the desktop shell has the owner key.

use uuid::Uuid;
use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::menu::{ShellCommand, BAR};
use crate::panel::{self, PanelBox, TextRun};
use crate::{scene, theme, App};

/// What the app asks the shell to do in the cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudAsk {
    /// The library's links and how much room it uses.
    Links,
    /// A new link to a board (None: the whole library).
    /// `person`: the name the one who opens it shows as ("" lets them type their own).
    /// `days`: it stops working that many days from now (None: never).
    Make { board: Option<Uuid>, edit: bool, label: String, person: String, days: Option<u32> },
    Revoke(Uuid),
}

/// A link, as the share card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShareLink {
    pub id: Uuid,
    pub edit: bool,
    pub board: Option<Uuid>,
    pub label: String,
    /// Made for this person ("" for anyone).
    pub person: String,
    /// When it stops working (ms since 1970), 0 for never.
    pub expires_ms: u64,
}

/// The shell's answer to a [`CloudAsk`].
#[derive(Debug, Clone, PartialEq)]
pub enum CloudAnswer {
    /// This library has no cloud copy (yet).
    NotConnected,
    Links { links: Vec<ShareLink>, used: Option<u64>, max: Option<u64> },
    /// A new link's address (shown this once).
    Made { url: String, edit: bool },
    Revoked(Uuid),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct ShareCard {
    pub board: Uuid,
    /// The boards this one is inside, with their names: their links open it too.
    pub above: Vec<(Uuid, String)>,
    /// There are boards inside this one, which its links open as well.
    pub has_inside: bool,
    /// None until the shell answers.
    pub links: Option<Vec<ShareLink>>,
    pub not_connected: bool,
    pub used: Option<(u64, u64)>,
    pub busy: bool,
    /// The last thing that happened ("View link copied", an error).
    pub note: Option<String>,
    /// The newest link made, shown so it can be copied again.
    pub made: Option<String>,
    /// Who the next link is for (their name on the board), typed on the card.
    pub person: String,
    /// The keyboard types into `person`.
    pub typing: bool,
    /// How long new links work for (None: until turned off). Kept while
    /// the card is open, so several links can be made the same way.
    pub days: Option<u32>,
}

/// The choices for how long a new link works.
pub const LASTS: [(Option<u32>, &str); 4] = [(None, "Until turned off"), (Some(1), "1 day"), (Some(7), "7 days"), (Some(30), "30 days")];

/// "ends in 3 days", "ends in 5 hours", "ended": for a link's row.
pub fn ends(now_ms: u64, expires_ms: u64) -> Option<String> {
    if expires_ms == 0 {
        return None;
    }
    if expires_ms <= now_ms {
        return Some("ended".into());
    }
    let left = expires_ms - now_ms;
    let hours = left.div_ceil(3_600_000);
    Some(match hours {
        0..=1 => "ends within the hour".into(),
        2..=47 => format!("ends in {hours} hours"),
        _ => format!("ends in {} days", left.div_ceil(86_400_000)),
    })
}

/// The longest name a link can carry.
const MAX_PERSON: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub enum ShareHit {
    Make { edit: bool },
    Person,
    Lasts(Option<u32>),
    CopyAgain,
    Revoke(Uuid),
    Close,
}

#[derive(Default)]
pub struct ShareLayout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], ShareHit)>,
}

const W: f32 = 400.0;
const PAD: f32 = 18.0;
const ROW: f32 = 34.0;

fn button(l: &mut ShareLayout, r: [f32; 4], label: &str, primary: bool, hover: [f32; 2], hit: ShareHit, text_width: &mut dyn FnMut(&str, f32) -> f32) {
    let t = theme::t();
    let over = panel::inside(r, hover);
    let (fill, stroke, ink) = if primary { (t.accent, [0.0; 4], t.accent_ink) } else { (if over { t.hover } else { [0.0; 4] }, t.line, t.ink) };
    l.boxes.push(PanelBox { rect: r, fill, stroke, radius: crate::theme::radius::CONTROL, stroke_w: if primary { 0.0 } else { 1.0 } });
    let w = text_width(label, 12.5);
    l.texts.push(TextRun { text: label.into(), at: [r[0] + (r[2] - w) / 2.0, r[1] + r[3] / 2.0 + 4.5], size: 12.5, color: ink });
    l.hits.push((r, hit));
}

fn mb(bytes: u64) -> String {
    if bytes >= 1 << 30 { format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64) } else { format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64) }
}

pub fn layout(card: &ShareCard, name: &str, hover: [f32; 2], caret: bool, viewport: [f32; 2], now_ms: u64, text_width: &mut dyn FnMut(&str, f32) -> f32) -> ShareLayout {
    let t = theme::t();
    let mut l = ShareLayout::default();
    let x0 = ((viewport[0] - W) / 2.0).max(8.0);
    let y0 = BAR + 60.0;
    let inner = W - 2.0 * PAD;
    let mut y = y0 + PAD + 16.0;
    let mut texts = Vec::new();
    texts.push(TextRun { text: panel::fit_with(text_width, &format!("Share “{name}”"), 16.0, inner - 30.0), at: [x0 + PAD, y], size: 16.0, color: t.ink });
    let close = [x0 + W - PAD - 22.0, y0 + 12.0, 22.0, 22.0];
    texts.push(TextRun { text: "×".into(), at: [close[0] + 5.0, close[1] + 16.0], size: 18.0, color: t.dim });
    y += 22.0;
    fn wrap(tw: &mut dyn FnMut(&str, f32) -> f32, texts: &mut Vec<TextRun>, y: &mut f32, s: &str, color: [f32; 4], x: f32, inner: f32) {
        for line in panel::wrap_with(tw, s, 12.5, inner) {
            texts.push(TextRun { text: line, at: [x, *y], size: 12.5, color });
            *y += 18.0;
        }
    }
    let mut buttons: Vec<([f32; 4], &str, bool, ShareHit)> = Vec::new();
    if card.not_connected {
        wrap(text_width, &mut texts, &mut y, "This library isn't in the cloud yet. Once it is, links made here open this board in a browser.", t.dim, x0 + PAD, inner);
    } else if card.links.is_none() {
        if card.busy {
            wrap(text_width, &mut texts, &mut y, "Asking the cloud…", t.dim, x0 + PAD, inner);
        }
    } else {
        let opens = if card.has_inside { "this board and the boards inside it" } else { "this board" };
        let until = if card.days.is_some() { "until it ends or you turn it off" } else { "until you turn the link off" };
        wrap(text_width, &mut texts, &mut y, &format!("Anyone with a link can open {opens}, and boards you put inside later, {until}. They can't reach your other boards."), t.dim, x0 + PAD, inner);
        y += 6.0;
        // Who it's for: their name on the board, so they don't type one.
        texts.push(TextRun { text: "FOR (OPTIONAL)".into(), at: [x0 + PAD, y + 8.0], size: 11.0, color: t.dim });
        y += 14.0;
        let field = [x0 + PAD, y, inner, 30.0];
        l.boxes.push(PanelBox { rect: field, fill: t.field, stroke: if card.typing { t.accent } else { t.line }, radius: crate::theme::radius::CONTROL, stroke_w: if card.typing { 1.5 } else { 1.0 } });
        let (shown, ink) = if card.person.is_empty() && !card.typing { ("Anyone: they type their own name".to_owned(), t.dim) } else { (panel::fit_with(text_width, &card.person, 13.0, inner - 20.0), t.ink) };
        let tw = if card.person.is_empty() { 0.0 } else { text_width(&shown, 13.0) };
        texts.push(TextRun { text: if card.person.is_empty() && card.typing { String::new() } else { shown }, at: [field[0] + 9.0, field[1] + 20.0], size: 13.0, color: ink });
        if card.typing && caret {
            l.boxes.push(PanelBox { rect: [field[0] + 10.0 + tw, field[1] + 7.0, 1.5, 16.0], fill: t.ink, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
        }
        l.hits.push((field, ShareHit::Person));
        y += 30.0 + 10.0;
        // How long it works: one row of choices.
        texts.push(TextRun { text: "WORKS FOR".into(), at: [x0 + PAD, y + 8.0], size: 11.0, color: t.dim });
        y += 14.0;
        let mut cx = x0 + PAD;
        for (days, label) in LASTS {
            let w = text_width(label, 12.0) + 18.0;
            let r = [cx, y, w, 26.0];
            let on = card.days == days;
            let fill = if on { t.chip } else if panel::inside(r, hover) { t.hover } else { [0.0; 4] };
            l.boxes.push(PanelBox { rect: r, fill, stroke: if on { t.accent } else { t.line }, radius: crate::theme::radius::CONTROL, stroke_w: if on { 1.5 } else { 1.0 } });
            texts.push(TextRun { text: label.into(), at: [cx + 9.0, y + 17.0], size: 12.0, color: if on { t.ink } else { t.dim } });
            l.hits.push((r, ShareHit::Lasts(days)));
            cx += w + 6.0;
        }
        y += 26.0 + 12.0;
        let bw = (inner - 10.0) / 2.0;
        buttons.push(([x0 + PAD, y, bw, 32.0], "Copy view link", true, ShareHit::Make { edit: false }));
        buttons.push(([x0 + PAD + bw + 10.0, y, bw, 32.0], "Copy edit link", false, ShareHit::Make { edit: true }));
        y += 32.0 + 14.0;
        if let Some(url) = &card.made {
            let fit = panel::fit_with(text_width, url, 12.0, inner - 70.0);
            texts.push(TextRun { text: fit, at: [x0 + PAD, y + 4.0], size: 12.0, color: t.accent });
            buttons.push(([x0 + W - PAD - 60.0, y - 12.0, 60.0, 26.0], "Copy", false, ShareHit::CopyAgain));
            y += 26.0;
        }
        // Links that open this board: its own, those of the boards it's in,
        // and those to the whole library.
        let via = |k: &ShareLink| match k.board {
            None => Some(Some("the whole library".to_owned())),
            Some(b) if b == card.board => Some(None),
            Some(b) => card.above.iter().find(|(id, _)| *id == b).map(|(_, name)| Some(name.clone())),
        };
        let mine: Vec<(&ShareLink, Option<String>)> = card.links.iter().flatten().filter_map(|k| Some((k, via(k)?))).collect();
        texts.push(TextRun { text: if mine.is_empty() { "No links open this board".into() } else { "Links that open this board".into() }, at: [x0 + PAD, y + 8.0], size: 11.0, color: t.dim });
        y += 16.0;
        for (k, through) in mine.iter().take(8) {
            let what = if k.edit { "Can edit" } else { "View only" };
            let mut label = match through {
                Some(name) => format!("{what}  ·  through {name}"),
                None if k.label.is_empty() => what.to_owned(),
                None => format!("{what}  ·  {}", k.label),
            };
            if !k.person.is_empty() {
                label = format!("{label}  ·  for {}", k.person);
            }
            let ended = k.expires_ms != 0 && k.expires_ms <= now_ms;
            if let Some(e) = ends(now_ms, k.expires_ms) {
                label = format!("{label}  ·  {e}");
            }
            texts.push(TextRun { text: panel::fit_with(text_width, &label, 12.5, inner - 90.0), at: [x0 + PAD, y + ROW / 2.0 + 4.5], size: 12.5, color: if ended { t.dim } else { t.ink } });
            buttons.push(([x0 + W - PAD - 76.0, y + 4.0, 76.0, ROW - 8.0], "Turn off", false, ShareHit::Revoke(k.id)));
            y += ROW;
        }
        if mine.len() > 8 {
            texts.push(TextRun { text: format!("and {} more", mine.len() - 8), at: [x0 + PAD, y + 14.0], size: 11.0, color: t.dim });
            y += 20.0;
        }
        if let Some((used, max)) = card.used {
            y += 6.0;
            texts.push(TextRun { text: format!("Cloud space: {} of {} (free)", mb(used), mb(max)), at: [x0 + PAD, y + 12.0], size: 11.0, color: t.dim });
            y += 18.0;
        }
    }
    if card.busy {
        texts.push(TextRun { text: "Working…".into(), at: [x0 + PAD, y + 16.0], size: 12.0, color: t.dim });
        y += 22.0;
    } else if let Some(n) = &card.note {
        y += 4.0;
        wrap(text_width, &mut texts, &mut y, n, t.ink, x0 + PAD, inner);
    }
    y += PAD;
    l.rect = [x0, y0, W, y - y0];
    l.boxes.insert(0, PanelBox { rect: l.rect, fill: t.panel_bg, stroke: t.line, radius: crate::theme::radius::PANEL, stroke_w: 1.0 });
    l.boxes.insert(0, crate::theme::shadow(l.rect, crate::theme::radius::PANEL));
    l.texts = texts;
    for (r, label, primary, hit) in buttons {
        button(&mut l, r, label, primary, hover, hit, text_width);
    }
    l.hits.push((close, ShareHit::Close));
    l
}

impl App {
    /// File > Share this board…: opens the card and asks for the links.
    pub fn open_share(&mut self) {
        let board = self.doc.board.id;
        let mut above: Vec<(Uuid, String)> = Vec::new();
        let mut at = self.doc.board.parent;
        while let Some(p) = at {
            if p == board || above.iter().any(|(id, _)| *id == p) {
                break;
            }
            above.push((p, self.board_names.get(&p).cloned().unwrap_or_else(|| "a board above".into())));
            at = self.board_meta.get(&p).and_then(|m| m.parent);
        }
        let has_inside = self.board_meta.values().any(|m| m.parent == Some(board));
        self.share = Some(ShareCard { board, above, has_inside, busy: true, ..Default::default() });
        self.shell_commands.push(ShellCommand::Cloud(CloudAsk::Links));
        self.needs_redraw = true;
    }

    pub fn share_card(&self) -> Option<&ShareCard> {
        self.share.as_ref()
    }

    /// The shell's answer to a [`CloudAsk`].
    pub fn cloud_answer(&mut self, answer: CloudAnswer) {
        self.needs_redraw = true;
        let Some(card) = &mut self.share else {
            // A link made while the card was closed: still copy it.
            if let CloudAnswer::Made { url, .. } = answer {
                self.shell_commands.push(ShellCommand::CopyText(url));
            }
            return;
        };
        card.busy = false;
        match answer {
            CloudAnswer::NotConnected => card.not_connected = true,
            CloudAnswer::Links { links, used, max } => {
                card.links = Some(links);
                card.used = used.zip(max);
            }
            CloudAnswer::Made { url, edit } => {
                let lasts = match card.days {
                    None => String::new(),
                    Some(1) => " It stops working in a day.".into(),
                    Some(d) => format!(" It stops working in {d} days."),
                };
                card.note = Some(format!("{} link copied. Paste it to whoever should {}.{lasts}", if edit { "Edit" } else { "View" }, if edit { "edit this board" } else { "see this board" }));
                card.made = Some(url.clone());
                card.busy = true;
                self.shell_commands.push(ShellCommand::CopyText(url));
                self.shell_commands.push(ShellCommand::Cloud(CloudAsk::Links));
            }
            CloudAnswer::Revoked(id) => {
                if let Some(links) = &mut card.links {
                    links.retain(|k| k.id != id);
                }
                card.note = Some("Link turned off. Anyone who had it can't open the board any more.".into());
            }
            CloudAnswer::Failed(e) => card.note = Some(format!("That didn't work: {e}")),
        }
    }

    fn share_layout(&mut self) -> Option<ShareLayout> {
        let card = self.share.clone()?;
        let name = self.doc.board.name.clone();
        let font = &mut self.font;
        let caret = (self.now_ms / 530) % 2 == 0;
        Some(layout(&card, &name, self.cursor, caret, self.viewport, self.now_ms, &mut |s, size| panel::text_width(font, s, size)))
    }

    /// A press while the card is open. True when the card took it (a press
    /// outside closes the card and goes no further).
    pub(crate) fn share_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(l) = self.share_layout() else { return false };
        self.needs_redraw = true;
        if !panel::inside(l.rect, pos) {
            self.share = None;
            return true;
        }
        let Some(hit) = l.hits.iter().rev().find(|(r, _)| panel::inside(*r, pos)).map(|(_, h)| h.clone()) else { return true };
        let busy = self.share.as_ref().is_some_and(|c| c.busy);
        if let Some(c) = &mut self.share {
            c.typing = hit == ShareHit::Person;
        }
        match hit {
            ShareHit::Person => {}
            ShareHit::Lasts(days) => {
                if let Some(c) = &mut self.share {
                    c.days = days;
                }
            }
            ShareHit::Close => self.share = None,
            _ if busy => {}
            ShareHit::Make { edit } => {
                let board = self.doc.board.id;
                let label = self.doc.board.name.clone();
                let mut person = String::new();
                let mut days = None;
                if let Some(c) = &mut self.share {
                    c.busy = true;
                    c.note = None;
                    person = std::mem::take(&mut c.person).trim().to_owned();
                    days = c.days;
                }
                self.shell_commands.push(ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit, label, person, days }));
            }
            ShareHit::CopyAgain => {
                if let Some(url) = self.share.as_ref().and_then(|c| c.made.clone()) {
                    self.shell_commands.push(ShellCommand::CopyText(url));
                    if let Some(c) = &mut self.share {
                        c.note = Some("Copied again.".into());
                    }
                }
            }
            ShareHit::Revoke(id) => {
                if let Some(c) = &mut self.share {
                    c.busy = true;
                    c.note = None;
                }
                self.shell_commands.push(ShellCommand::Cloud(CloudAsk::Revoke(id)));
            }
        }
        true
    }

    /// Typing the name of who a link is for.
    pub(crate) fn share_typing(&self) -> bool {
        self.share.as_ref().is_some_and(|c| c.typing)
    }

    pub(crate) fn share_key(&mut self, key: crate::Key) -> bool {
        use crate::Key;
        let Some(c) = self.share.as_mut().filter(|c| c.typing) else { return false };
        match key {
            Key::Escape | Key::Enter => c.typing = false,
            Key::Backspace => {
                c.person.pop();
            }
            Key::Char(ch) if !ch.is_control() && c.person.chars().count() < MAX_PERSON => c.person.push(ch),
            _ => {}
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn share_text(&mut self, s: &str) -> bool {
        let Some(c) = self.share.as_mut().filter(|c| c.typing) else { return false };
        for ch in s.chars().filter(|ch| !ch.is_control()) {
            if c.person.chars().count() >= MAX_PERSON {
                break;
            }
            c.person.push(ch);
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn draw_share(&mut self, renderer: &mut Renderer) {
        if self.share_typing() {
            self.needs_redraw = true; // the caret blinks
        }
        let Some(l) = self.share_layout() else { return };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Key, Mods};
    use wall_format::Board;

    fn app() -> App {
        let mut a = App::new(Board::new("Moodboard", 0), "test");
        a.resize([1200.0, 800.0]);
        a
    }

    fn press(a: &mut App, hit: &ShareHit) {
        let l = a.share_layout().unwrap();
        let r = l.hits.iter().find(|(_, h)| h == hit).unwrap_or_else(|| panic!("no {hit:?}")).0;
        a.pointer_down(Button::Primary, [r[0] + 4.0, r[1] + 4.0], Mods::default());
        a.pointer_up(Button::Primary, [r[0] + 4.0, r[1] + 4.0]);
    }

    #[test]
    fn share_asks_the_shell_copies_new_links_and_turns_them_off() {
        let mut a = app();
        let board = a.board_id();
        a.run_command(crate::menu::Command::Share);
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Links)]);
        let old = Uuid::new_v4();
        let other = ShareLink { id: Uuid::new_v4(), edit: true, board: Some(Uuid::new_v4()), label: String::new(), person: String::new(), expires_ms: 0 };
        a.cloud_answer(CloudAnswer::Links { links: vec![ShareLink { id: old, edit: false, board: Some(board), label: "client".into(), person: String::new(), expires_ms: 0 }, other], used: Some(150 << 20), max: Some(4 << 30) });
        let l = a.share_layout().unwrap();
        assert_eq!(l.hits.iter().filter(|(_, h)| matches!(h, ShareHit::Revoke(_))).count(), 1, "only this board's links");
        assert!(l.texts.iter().any(|t| t.text.contains("150 MB of 4.0 GB")));

        press(&mut a, &ShareHit::Make { edit: true });
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit: true, label: "Moodboard".into(), person: String::new(), days: None })]);
        a.cloud_answer(CloudAnswer::Made { url: "https://wall.example/#share=abc".into(), edit: true });
        let cmds = a.take_shell_commands();
        assert_eq!(cmds[0], ShellCommand::CopyText("https://wall.example/#share=abc".into()));
        assert_eq!(cmds[1], ShellCommand::Cloud(CloudAsk::Links));

        // A link for one person: their name goes with it.
        press(&mut a, &ShareHit::Person);
        assert!(a.is_editing());
        a.text_input("Mia ");
        a.key(Key::Char('K'));
        a.key(Key::Backspace);
        a.key(Key::Enter);
        assert!(!a.is_editing());
        a.cloud_answer(CloudAnswer::Links { links: vec![], used: None, max: None });
        press(&mut a, &ShareHit::Make { edit: false });
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit: false, label: "Moodboard".into(), person: "Mia".into(), days: None })]);
        assert_eq!(a.share_card().unwrap().person, "", "the next link is for anyone again");
        a.cloud_answer(CloudAnswer::Links { links: vec![ShareLink { id: Uuid::new_v4(), edit: false, board: Some(board), label: String::new(), person: "Mia".into(), expires_ms: 0 }], used: None, max: None });
        assert!(a.share_layout().unwrap().texts.iter().any(|t| t.text.ends_with("for Mia")));

        a.cloud_answer(CloudAnswer::Links { links: vec![ShareLink { id: old, edit: false, board: Some(board), label: String::new(), person: String::new(), expires_ms: 0 }], used: None, max: None });
        press(&mut a, &ShareHit::Revoke(old));
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Revoke(old))]);
        a.cloud_answer(CloudAnswer::Revoked(old));
        assert!(a.share_layout().unwrap().hits.iter().all(|(_, h)| !matches!(h, ShareHit::Revoke(_))));

        // A board inside another: the other's links, and the library's, open
        // it too and are listed; a sister's aren't.
        a.key(Key::Escape);
        let child = a.new_board();
        a.open_board(child, true);
        a.run_command(crate::menu::Command::Share);
        a.take_shell_commands();
        let k = |b: Option<Uuid>| ShareLink { id: Uuid::new_v4(), edit: false, board: b, label: "x".into(), person: String::new(), expires_ms: 0 };
        a.cloud_answer(CloudAnswer::Links { links: vec![k(Some(board)), k(None), k(Some(Uuid::new_v4()))], used: None, max: None });
        let l = a.share_layout().unwrap();
        assert_eq!(l.hits.iter().filter(|(_, h)| matches!(h, ShareHit::Revoke(_))).count(), 2);
        assert!(l.texts.iter().any(|t| t.text.contains("through Moodboard")) && l.texts.iter().any(|t| t.text.contains("whole library")));
        a.key(Key::Escape);
        a.open_board(board, false);
        a.run_command(crate::menu::Command::Share);
        a.cloud_answer(CloudAnswer::Links { links: vec![], used: None, max: None });
        let said = a.share_layout().unwrap().texts.iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ");
        assert!(said.contains("boards inside it"), "says what the link opens: {said}");
        // An error isn't hidden behind "Asking the cloud…".
        a.run_command(crate::menu::Command::Share);
        a.cloud_answer(CloudAnswer::Failed("offline".into()));
        let l = a.share_layout().unwrap();
        assert!(l.texts.iter().all(|t| !t.text.contains("Asking")) && l.texts.iter().any(|t| t.text.contains("offline")));
        a.take_shell_commands();

        // A link that ends by itself: the choice goes with it, stays for
        // the next one, and the list says when each ends.
        a.cloud_answer(CloudAnswer::Links { links: vec![], used: None, max: None });
        press(&mut a, &ShareHit::Lasts(Some(7)));
        press(&mut a, &ShareHit::Make { edit: false });
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit: false, label: "Moodboard".into(), person: String::new(), days: Some(7) })]);
        a.cloud_answer(CloudAnswer::Made { url: "https://wall.example/#share=week".into(), edit: false });
        assert!(a.share_card().unwrap().note.as_deref().unwrap().contains("in 7 days"));
        assert_eq!(a.share_card().unwrap().days, Some(7));
        a.take_shell_commands();
        a.now_ms = 1_700_000_000_000;
        let now = a.now_ms;
        let k = |id, expires_ms| ShareLink { id, edit: false, board: Some(board), label: String::new(), person: String::new(), expires_ms };
        a.cloud_answer(CloudAnswer::Links { links: vec![k(Uuid::new_v4(), now + 3 * 86_400_000 - 5), k(Uuid::new_v4(), now - 1), k(Uuid::new_v4(), 0)], used: None, max: None });
        let said = a.share_layout().unwrap().texts.iter().map(|t| t.text.clone()).collect::<Vec<_>>().join("|");
        assert!(said.contains("ends in 3 days") && said.contains("ended"), "{said}");
        assert!(said.contains("until it ends or you turn it"), "{said}");

        // Esc closes it; so does a click outside.
        a.key(Key::Escape);
        assert!(a.share_card().is_none());
        a.run_command(crate::menu::Command::Share);
        a.cloud_answer(CloudAnswer::NotConnected);
        assert!(a.share_layout().unwrap().texts.iter().any(|t| t.text.contains("isn't in the cloud")));
        a.pointer_down(Button::Primary, [5.0, 790.0], Mods::default());
        assert!(a.share_card().is_none());
    }
}
