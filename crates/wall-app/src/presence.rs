//! Who else is on this board: a round badge per person at the top right,
//! and their pointer with their name on the board. The shell (desktop or
//! web) talks to the cloud and hands the list over with [`App::set_peers`].

use crate::panel::{self, PanelBox, TextRun};
use crate::{menu, App};
use wall_render::{BoxInstance, Renderer, ViewUniform};

/// Someone else looking at the board this app has open.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// Their connection, stable while they stay.
    pub id: String,
    pub name: String,
    /// Their pointer on the board (board units), if it's over the board.
    pub at: Option<[f32; 2]>,
    /// The colour they picked (an index into the 8), else one by their id.
    pub color: Option<u8>,
}

impl Peer {
    pub fn paint(&self) -> [f32; 4] {
        pick(self.color).unwrap_or_else(|| color_of(&self.id))
    }
}

/// A picked colour.
pub fn pick(i: Option<u8>) -> Option<[f32; 4]> {
    i.and_then(|i| COLORS.get(i as usize).copied())
}

/// How many colours there are to pick from.
pub const COLOR_COUNT: u8 = 8;

/// Colours people get, by connection id: bright enough on light and dark.
const COLORS: [[f32; 4]; 8] = [
    [0.91, 0.30, 0.24, 1.0],
    [0.95, 0.55, 0.10, 1.0],
    [0.20, 0.66, 0.33, 1.0],
    [0.10, 0.60, 0.75, 1.0],
    [0.30, 0.45, 0.95, 1.0],
    [0.58, 0.36, 0.90, 1.0],
    [0.88, 0.30, 0.62, 1.0],
    [0.45, 0.50, 0.55, 1.0],
];

pub fn color_of(id: &str) -> [f32; 4] {
    // FNV-1a: the same person gets the same colour everywhere.
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    COLORS[(h % COLORS.len() as u32) as usize]
}

/// First letter of a name, upper case, for the badge.
fn initial(name: &str) -> String {
    name.trim().chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

const BADGE: f32 = 24.0;
const MAX_BADGES: usize = 6;
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

pub(crate) struct PresenceLayout {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    /// Our own badge: a click there changes our name.
    pub me: Option<[f32; 4]>,
    /// The card for typing a new name, while it's open.
    pub card: Option<[f32; 4]>,
    /// The colour swatches on the card.
    pub swatches: Vec<([f32; 4], u8)>,
}

/// Ourselves, for the layout: our name, and the text being typed if the
/// name card is open.
pub(crate) struct Me<'a> {
    pub name: &'a str,
    pub editing: Option<&'a str>,
    /// Caret blink: shown or not.
    pub caret: bool,
    pub color: Option<u8>,
    /// The name comes with the link and can't be changed here.
    pub fixed: bool,
}

const CARD_W: f32 = 280.0;

/// Badges and pointers for `peers`, in screen pixels. The badges sit in the
/// menu bar and end at `right` (left of the board's name).
pub(crate) fn layout(peers: &[Peer], me: Option<Me>, to_screen: &dyn Fn([f32; 2]) -> [f32; 2], viewport: [f32; 2], right: f32, width: &mut dyn FnMut(&str, f32) -> f32) -> PresenceLayout {
    let mut boxes = Vec::new();
    let mut texts = Vec::new();
    let mut me_rect = None;
    let mut card = None;
    let mut swatches = Vec::new();

    // Pointers first, so the badges stay on top.
    for p in peers {
        let Some(at) = p.at else { continue };
        let s = to_screen(at);
        if s[0] < -40.0 || s[1] < menu::BAR || s[0] > viewport[0] + 4.0 || s[1] > viewport[1] + 4.0 {
            continue;
        }
        let color = p.paint();
        boxes.push(PanelBox { rect: [s[0] - 6.0, s[1] - 6.0, 12.0, 12.0], fill: color, stroke: WHITE, radius: 6.0, stroke_w: 2.0 });
        let name = panel::fit_with(width, &p.name, 12.0, 160.0);
        let w = width(&name, 12.0);
        let tag = [s[0] + 9.0, s[1] + 9.0, w + 14.0, 20.0];
        boxes.push(PanelBox { rect: tag, fill: color, stroke: [0.0; 4], radius: crate::theme::radius::CONTROL, stroke_w: 0.0 });
        texts.push(TextRun { text: name, at: [tag[0] + 7.0, tag[1] + 14.0], size: 12.0, color: WHITE });
    }

    // Badges, right to left: ours first, set a little apart.
    let shown = peers.len().min(MAX_BADGES);
    let y = (menu::BAR - BADGE) / 2.0;
    let mut right = right;
    if let Some(me) = &me {
        let r = [right - BADGE, y, BADGE, BADGE];
        let fill = pick(me.color).unwrap_or([0.16, 0.17, 0.2, 1.0]);
        boxes.push(PanelBox { rect: r, fill, stroke: crate::theme::t().menu_bar, radius: BADGE / 2.0, stroke_w: 2.0 });
        let i = initial(me.name);
        texts.push(TextRun { at: [r[0] + (BADGE - width(&i, 13.0)) / 2.0, y + 16.5], text: i, size: 13.0, color: WHITE });
        me_rect = Some(r);
        right = r[0] - 10.0;
        if let Some(text) = me.editing {
            let c = [(r[0] + BADGE - CARD_W).max(8.0), menu::BAR + 6.0, CARD_W, 128.0];
            let th = crate::theme::t();
            let dim = th.dim;
            boxes.push(crate::theme::shadow(c, crate::theme::radius::PANEL));
            boxes.push(PanelBox { rect: c, fill: th.panel_bg, stroke: th.line, radius: crate::theme::radius::PANEL, stroke_w: 1.0 });
            let field = [c[0] + 12.0, c[1] + 32.0, c[2] - 24.0, 28.0];
            if me.fixed {
                texts.push(TextRun { text: "Your name comes with your link".into(), at: [c[0] + 14.0, c[1] + 22.0], size: 12.0, color: dim });
                let t = panel::fit_with(width, me.name, 14.0, field[2] - 8.0);
                texts.push(TextRun { text: t, at: [field[0] + 2.0, field[1] + 19.0], size: 14.0, color: th.ink });
            } else {
                texts.push(TextRun { text: "Your name, as others see it".into(), at: [c[0] + 14.0, c[1] + 22.0], size: 12.0, color: dim });
                boxes.push(PanelBox { rect: field, fill: th.field, stroke: th.accent, radius: crate::theme::radius::CONTROL, stroke_w: 1.5 });
                let shown_text = panel::fit_with(width, text, 14.0, field[2] - 20.0);
                let tw = width(&shown_text, 14.0);
                texts.push(TextRun { text: shown_text, at: [field[0] + 8.0, field[1] + 19.0], size: 14.0, color: th.ink });
                if me.caret {
                    boxes.push(PanelBox { rect: [field[0] + 9.0 + tw, field[1] + 6.0, 1.5, 16.0], fill: th.ink, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
                }
            }
            // Colour: 8 swatches; the picked one has a ring.
            let sw = 22.0;
            let gap = (field[2] - sw * COLOR_COUNT as f32) / (COLOR_COUNT as f32 - 1.0);
            for i in 0..COLOR_COUNT {
                let rr = [field[0] + i as f32 * (sw + gap), c[1] + 72.0, sw, sw];
                let on = me.color == Some(i);
                boxes.push(PanelBox { rect: rr, fill: COLORS[i as usize], stroke: if on { th.ink } else { [0.0; 4] }, radius: sw / 2.0, stroke_w: if on { 2.5 } else { 0.0 } });
                swatches.push((rr, i));
            }
            let hint = if me.fixed { "Pick a colour. Esc to close" } else { "Enter to keep, Esc to cancel" };
            texts.push(TextRun { text: hint.into(), at: [c[0] + 14.0, c[1] + 116.0], size: 11.0, color: th.dim });
            card = Some(c);
        }
    }
    let mut x = right - BADGE;
    if peers.len() > shown {
        let more = format!("+{}", peers.len() - shown);
        let w = width(&more, 12.0).max(BADGE - 10.0) + 10.0;
        let r = [right - w, y, w, BADGE];
        boxes.push(PanelBox { rect: r, fill: crate::theme::t().chip, stroke: crate::theme::t().menu_bar, radius: BADGE / 2.0, stroke_w: 2.0 });
        texts.push(TextRun { text: more.clone(), at: [r[0] + (w - width(&more, 12.0)) / 2.0, y + 16.5], size: 12.0, color: crate::theme::t().ink });
        x = r[0] - BADGE + 6.0;
    }
    for p in peers.iter().take(shown) {
        boxes.push(PanelBox { rect: [x, y, BADGE, BADGE], fill: p.paint(), stroke: crate::theme::t().menu_bar, radius: BADGE / 2.0, stroke_w: 2.0 });
        let i = initial(&p.name);
        texts.push(TextRun { at: [x + (BADGE - width(&i, 13.0)) / 2.0, y + 16.5], text: i, size: 13.0, color: WHITE });
        // Overlap a little, like a stack of people.
        x -= BADGE - 6.0;
    }
    PresenceLayout { boxes, texts, me: me_rect, card, swatches }
}

/// Names are one line, at most this many characters (the server cuts there too).
const MAX_NAME: usize = 40;

impl App {
    /// The people on this board besides us, from the shell's cloud link.
    pub fn set_peers(&mut self, peers: Vec<Peer>) {
        if self.peers != peers {
            self.peers = peers;
            self.needs_redraw = true;
        }
    }

    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// Where our pointer is on the board, to show the others; None when it's
    /// off the window or over the menu bar.
    pub fn pointer_on_board(&self) -> Option<[f32; 2]> {
        let c = self.cursor;
        let inside = !self.cursor_out && c[0] >= 0.0 && c[1] >= menu::BAR && c[0] < self.viewport[0] && c[1] < self.viewport[1];
        inside.then(|| self.camera.screen_to_board(c, self.viewport))
    }

    /// The pointer left the window.
    pub fn pointer_left(&mut self) {
        self.cursor_out = true;
    }

    /// Our name on shared boards (None: not connected, no badge of our own).
    pub fn set_my_name(&mut self, name: Option<String>) {
        if self.my_name != name {
            self.my_name = name;
            self.needs_redraw = true;
        }
    }

    pub fn my_name(&self) -> Option<&str> {
        self.my_name.as_deref()
    }

    /// Our picked colour, if any.
    pub fn set_my_color(&mut self, color: Option<u8>) {
        let color = color.filter(|c| *c < COLOR_COUNT);
        if self.my_color != color {
            self.my_color = color;
            self.needs_redraw = true;
        }
    }

    pub fn my_color(&self) -> Option<u8> {
        self.my_color
    }

    /// The name comes with the link (it can't be typed over).
    pub fn set_name_fixed(&mut self, fixed: bool) {
        self.name_fixed = fixed;
    }

    /// The name card is open (the shell shouldn't overwrite the name now).
    pub fn name_typing(&self) -> bool {
        self.name_edit.is_some()
    }

    /// A press on our badge opens the name card; a press outside it keeps
    /// what was typed. True when the press was used up.
    pub(crate) fn presence_press(&mut self, pos: [f32; 2]) -> bool {
        if self.my_name.is_none() {
            return false;
        }
        let l = self.presence_layout();
        if let Some(c) = l.card {
            if let Some((_, i)) = l.swatches.iter().find(|(r, _)| panel::inside(*r, pos)) {
                if self.my_color != Some(*i) {
                    self.my_color = Some(*i);
                    self.shell_commands.push(crate::ShellCommand::SetColor(*i));
                }
                self.needs_redraw = true;
                return true;
            }
            if panel::inside(c, pos) {
                return true;
            }
            self.commit_name();
            return l.me.is_some_and(|r| panel::inside(r, pos));
        }
        if l.me.is_some_and(|r| panel::inside(r, pos)) {
            self.name_edit = self.my_name.clone();
            self.needs_redraw = true;
            return true;
        }
        false
    }

    fn commit_name(&mut self) {
        let Some(text) = self.name_edit.take() else { return };
        if self.name_fixed {
            self.needs_redraw = true;
            return;
        }
        let name: String = text.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(MAX_NAME).collect();
        self.needs_redraw = true;
        if name.is_empty() || Some(&name) == self.my_name.as_ref() {
            return;
        }
        self.my_name = Some(name.clone());
        self.shell_commands.push(crate::ShellCommand::SetName(name));
    }

    /// Keys while the name card is open. True when used.
    pub(crate) fn name_key(&mut self, key: crate::Key) -> bool {
        use crate::Key;
        let fixed = self.name_fixed;
        let Some(text) = &mut self.name_edit else { return false };
        match key {
            Key::Escape => self.name_edit = None,
            Key::Enter => self.commit_name(),
            Key::Backspace if !fixed => {
                text.pop();
            }
            Key::Char(c) if !fixed && !c.is_control() && text.chars().count() < MAX_NAME => text.push(c),
            _ => {}
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn name_text(&mut self, s: &str) -> bool {
        let fixed = self.name_fixed;
        let Some(text) = &mut self.name_edit else { return false };
        if fixed {
            return true;
        }
        for c in s.chars().filter(|c| !c.is_control()) {
            if text.chars().count() >= MAX_NAME {
                break;
            }
            text.push(c);
        }
        self.needs_redraw = true;
        true
    }

    fn presence_layout(&mut self) -> PresenceLayout {
        let cam = self.camera;
        let vp = self.viewport;
        let to_screen = move |p: [f32; 2]| [(p[0] - cam.center[0]) * cam.zoom + vp[0] * 0.5, (p[1] - cam.center[1]) * cam.zoom + vp[1] * 0.5];
        let caret = (self.now_ms / 530) % 2 == 0;
        let me = self.my_name.as_deref().map(|name| Me { name, editing: self.name_edit.as_deref(), caret, color: self.my_color, fixed: self.name_fixed });
        let font = &mut self.font;
        // At the right of the menu bar, left of the Share button.
        layout(&self.peers, me, &to_screen, vp, menu::badges_right(vp, self.desktop), &mut |s, size| panel::text_width(font, s, size))
    }

    pub(crate) fn draw_presence(&mut self, renderer: &mut Renderer) {
        if self.peers.is_empty() && self.my_name.is_none() {
            return;
        }
        if self.name_edit.is_some() {
            // The caret blinks.
            self.needs_redraw = true;
        }
        let l = self.presence_layout();
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<BoxInstance> = l.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            crate::scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
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

    fn width(s: &str, size: f32) -> f32 {
        s.chars().count() as f32 * size * 0.55
    }

    fn peer(id: &str, name: &str, at: Option<[f32; 2]>) -> Peer {
        Peer { id: id.into(), name: name.into(), at, color: None }
    }

    #[test]
    fn each_person_gets_a_badge_and_a_named_pointer_where_they_point() {
        let peers = [peer("a1", "Mia", Some([100.0, 200.0])), peer("b2", "jon", None)];
        let l = layout(&peers, None, &|p| p, [1000.0, 700.0], 900.0, &mut width);
        // Mia: dot + name tag + badge; Jon: badge only (pointer off the board).
        assert_eq!(l.boxes.len(), 4);
        assert!(l.texts.iter().any(|t| t.text == "Mia" && t.at[0] > 100.0 && t.at[1] > 200.0), "name beside the pointer");
        let badges: Vec<&str> = l.texts.iter().filter(|t| t.size == 13.0).map(|t| t.text.as_str()).collect();
        assert_eq!(badges, ["M", "J"]);
        assert_eq!(l.boxes[0].fill, color_of("a1"), "same colour for dot and badge");
        assert_eq!(l.boxes[2].fill, color_of("a1"));
        assert_eq!(color_of("a1"), color_of("a1"));
    }

    #[test]
    fn a_crowd_shows_six_badges_and_a_count_and_off_screen_pointers_are_left_out() {
        let peers: Vec<Peer> = (0..9).map(|i| peer(&format!("p{i}"), &format!("P{i}"), Some([5000.0, 5000.0]))).collect();
        let l = layout(&peers, None, &|p| p, [1000.0, 700.0], 900.0, &mut width);
        assert_eq!(l.boxes.len(), 7, "no pointers off screen, 6 badges, 1 count");
        assert!(l.texts.iter().any(|t| t.text == "+3"));
    }

    #[test]
    fn clicking_your_own_badge_lets_you_type_a_new_name() {
        use crate::{Button, Key, Mods, ShellCommand};
        let mut a = App::new(wall_format::Board::new("B", 0), "test");
        a.resize([1000.0, 700.0]);
        let press = |a: &mut App, p: [f32; 2]| {
            a.pointer_down(Button::Primary, p, Mods::default());
            a.pointer_up(Button::Primary, p);
        };
        assert!(a.presence_layout().me.is_none(), "no badge without a live connection");
        a.set_my_name(Some("Owner".into()));
        let r = a.presence_layout().me.unwrap();
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        assert!(a.is_editing());
        for _ in 0.."Owner".len() {
            a.key(Key::Backspace);
        }
        a.text_input("Paul\u{7}");
        a.key(Key::Char('!'));
        assert!(a.presence_layout().card.is_some());
        a.key(Key::Enter);
        assert_eq!(a.my_name(), Some("Paul!"));
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::SetName("Paul!".into())]);
        assert!(!a.is_editing());

        // Esc keeps the old name; an empty one isn't taken; a click outside keeps what's typed.
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        a.text_input(" x");
        a.key(Key::Escape);
        assert_eq!(a.my_name(), Some("Paul!"));
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        for _ in 0..10 {
            a.key(Key::Backspace);
        }
        a.key(Key::Enter);
        assert_eq!(a.my_name(), Some("Paul!"));
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        a.text_input("o");
        press(&mut a, [500.0, 400.0]);
        assert_eq!(a.my_name(), Some("Paul!o"));
        assert!(a.take_shell_commands().contains(&ShellCommand::SetName("Paul!o".into())));

        // A colour from the card.
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        let (sw, i) = a.presence_layout().swatches[3];
        press(&mut a, [sw[0] + 5.0, sw[1] + 5.0]);
        assert_eq!((i, a.my_color()), (3, Some(3)));
        assert_eq!(a.take_shell_commands(), vec![ShellCommand::SetColor(3)]);
        assert!(a.presence_layout().card.is_some(), "the card stays open for the name");
        a.key(Key::Escape);

        // A name that comes with the link can't be typed over.
        a.set_name_fixed(true);
        a.set_my_name(Some("Mia".into()));
        press(&mut a, [r[0] + 5.0, r[1] + 5.0]);
        a.key(Key::Backspace);
        a.text_input("xx");
        a.key(Key::Enter);
        assert_eq!(a.my_name(), Some("Mia"));
        assert!(a.take_shell_commands().is_empty());
    }

    #[test]
    fn the_app_redraws_when_people_come_and_says_where_its_own_pointer_is() {
        let mut a = App::new(wall_format::Board::new("B", 0), "test");
        a.resize([800.0, 600.0]);
        a.needs_redraw = false;
        a.set_peers(vec![peer("x", "Ann", Some([0.0, 0.0]))]);
        assert!(a.needs_redraw);
        assert_eq!(a.peers().len(), 1);
        a.pointer_move([400.0, 300.0]);
        let at = a.pointer_on_board().unwrap();
        assert!((at[0] - a.camera.center[0]).abs() < 1e-3 && (at[1] - a.camera.center[1]).abs() < 1e-3);
        a.pointer_move([400.0, 10.0]);
        assert_eq!(a.pointer_on_board(), None, "over the menu bar");
        a.pointer_move([400.0, 300.0]);
        a.pointer_left();
        assert_eq!(a.pointer_on_board(), None);
        a.pointer_move([300.0, 300.0]);
        assert!(a.pointer_on_board().is_some());
    }
}
