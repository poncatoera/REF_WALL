//! Share (File > Share this board…): a card over the board that makes
//! private links to the open board, to view or to edit, copies them, and
//! lists the board's links so they can be turned off.
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
    Make { board: Option<Uuid>, edit: bool, label: String },
    Revoke(Uuid),
}

/// A link, as the share card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShareLink {
    pub id: Uuid,
    pub edit: bool,
    pub board: Option<Uuid>,
    pub label: String,
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShareHit {
    Make { edit: bool },
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
    let (fill, stroke, ink) = if primary { (t.accent, [0.0; 4], t.panel_bg) } else { (if over { t.field } else { [0.0; 4] }, t.line, t.ink) };
    l.boxes.push(PanelBox { rect: r, fill, stroke, radius: 7.0, stroke_w: if primary { 0.0 } else { 1.0 } });
    let w = text_width(label, 12.5);
    l.texts.push(TextRun { text: label.into(), at: [r[0] + (r[2] - w) / 2.0, r[1] + r[3] / 2.0 + 4.5], size: 12.5, color: ink });
    l.hits.push((r, hit));
}

fn mb(bytes: u64) -> String {
    if bytes >= 1 << 30 { format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64) } else { format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64) }
}

pub fn layout(card: &ShareCard, name: &str, hover: [f32; 2], viewport: [f32; 2], text_width: &mut dyn FnMut(&str, f32) -> f32) -> ShareLayout {
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
        wrap(text_width, &mut texts, &mut y, &format!("Anyone with a link can open {opens}, and boards you put inside later, until you turn the link off. They can't reach your other boards."), t.dim, x0 + PAD, inner);
        y += 6.0;
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
        texts.push(TextRun { text: if mine.is_empty() { "NO LINKS OPEN THIS BOARD".into() } else { "LINKS THAT OPEN THIS BOARD".into() }, at: [x0 + PAD, y + 8.0], size: 11.0, color: t.dim });
        y += 16.0;
        for (k, through) in mine.iter().take(8) {
            let what = if k.edit { "Can edit" } else { "View only" };
            let label = match through {
                Some(name) => format!("{what}  ·  through {name}"),
                None if k.label.is_empty() => what.to_owned(),
                None => format!("{what}  ·  {}", k.label),
            };
            texts.push(TextRun { text: panel::fit_with(text_width, &label, 12.5, inner - 90.0), at: [x0 + PAD, y + ROW / 2.0 + 4.5], size: 12.5, color: t.ink });
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
    l.boxes.push(PanelBox { rect: l.rect, fill: t.panel_bg, stroke: t.line, radius: 12.0, stroke_w: 1.0 });
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
                card.note = Some(format!("{} link copied. Paste it to whoever should {}.", if edit { "Edit" } else { "View" }, if edit { "edit this board" } else { "see this board" }));
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
        Some(layout(&card, &name, self.cursor, self.viewport, &mut |s, size| panel::text_width(font, s, size)))
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
        match hit {
            ShareHit::Close => self.share = None,
            _ if busy => {}
            ShareHit::Make { edit } => {
                let board = self.doc.board.id;
                let label = self.doc.board.name.clone();
                if let Some(c) = &mut self.share {
                    c.busy = true;
                    c.note = None;
                }
                self.shell_commands.push(ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit, label }));
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

    pub(crate) fn draw_share(&mut self, renderer: &mut Renderer) {
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
        let other = ShareLink { id: Uuid::new_v4(), edit: true, board: Some(Uuid::new_v4()), label: String::new() };
        a.cloud_answer(CloudAnswer::Links { links: vec![ShareLink { id: old, edit: false, board: Some(board), label: "client".into() }, other], used: Some(150 << 20), max: Some(4 << 30) });
        let l = a.share_layout().unwrap();
        assert_eq!(l.hits.iter().filter(|(_, h)| matches!(h, ShareHit::Revoke(_))).count(), 1, "only this board's links");
        assert!(l.texts.iter().any(|t| t.text.contains("150 MB of 4.0 GB")));

        press(&mut a, &ShareHit::Make { edit: true });
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::Cloud(CloudAsk::Make { board: Some(board), edit: true, label: "Moodboard".into() })]);
        a.cloud_answer(CloudAnswer::Made { url: "https://wall.example/#share=abc".into(), edit: true });
        let cmds = a.take_shell_commands();
        assert_eq!(cmds[0], ShellCommand::CopyText("https://wall.example/#share=abc".into()));
        assert_eq!(cmds[1], ShellCommand::Cloud(CloudAsk::Links));

        a.cloud_answer(CloudAnswer::Links { links: vec![ShareLink { id: old, edit: false, board: Some(board), label: String::new() }], used: None, max: None });
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
        let k = |b: Option<Uuid>| ShareLink { id: Uuid::new_v4(), edit: false, board: b, label: "x".into() };
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
