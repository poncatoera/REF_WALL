//! Walking people through a board together.
//!
//! - Follow: a click on someone's badge and your view goes where theirs
//!   goes, until you move the board yourself, press Esc or click the banner.
//! - Spotlight (View > Bring everyone to your view): everyone on the board
//!   follows you. Anyone can step out by moving, and back in from the banner.
//! - Present (View > Present this board, Shift P): the board fills the
//!   window with the menus hidden, and → / ← step through its groups and
//!   pictures in reading order, starting and ending on the whole board (or
//!   through the selection, if something is selected). On a shared board
//!   everyone follows along.
//!
//! The shell carries the messages: each frame it reads [`App::shared_view`],
//! [`App::follow_target`] and [`App::spotlight_on`] and sends what changed,
//! hands the others' views over in [`Peer::view`](crate::Peer::view), and
//! calls [`App::peer_went`] when someone we follow opens another board.

use crate::panel::{fit, inside, text_width, PanelBox, TextRun};
use crate::{menu, scene, theme, App, Camera, Key, MAX_ZOOM, MIN_ZOOM};
use uuid::Uuid;
use wall_format::{BoardItem, ItemKind};
use wall_render::{BoxInstance, Renderer, ViewUniform};

/// A followed view that changes glides there this fast (updates come ~10
/// times a second, so this keeps it smooth without lagging behind).
const FOLLOW_MS: u64 = 160;
/// The first jump to someone's view, and each step of a presentation.
const JUMP_MS: u64 = 520;
/// Someone we follow who's gone this long (not just changing boards) is
/// let go.
const LOST_MS: u64 = 5_000;
/// Presentation stops are shown at most this zoom (a small note shouldn't
/// fill the screen with blurry text).
const STOP_MAX_ZOOM: f32 = 4.0;
/// Room around a stop, as a share of the window.
const STOP_MARGIN: f32 = 0.06;

#[derive(Debug, Default)]
pub(crate) struct Together {
    /// Our connection id on the live server (what others' `follow` names).
    pub my_id: Option<String>,
    /// Who we chose to follow (a click on their badge).
    following: Option<String>,
    /// When the person we follow went missing from the board.
    missing_since: Option<u64>,
    /// A spotlight we stepped out of by moving: not followed until asked.
    stepped_out: Option<String>,
    /// Everyone on the board follows us.
    spotlight: bool,
    glide: Option<Glide>,
    /// Where the glide is headed, so a view that didn't change doesn't
    /// start a new one each frame.
    aim: Option<Camera>,
    /// The camera as a glide last left it: if it's anything else, we moved.
    placed: Option<Camera>,
    present: Option<Presentation>,
}

#[derive(Debug, Clone, Copy)]
struct Glide {
    from: Camera,
    to: Camera,
    start_ms: u64,
    ms: u64,
}

impl Glide {
    /// The camera at `now`, and whether it's there.
    fn at(&self, now: u64) -> (Camera, bool) {
        let t = if self.ms == 0 { 1.0 } else { (now.saturating_sub(self.start_ms) as f32 / self.ms as f32).min(1.0) };
        if t >= 1.0 {
            return (self.to, true);
        }
        // Ease out: quick start, soft landing.
        let e = 1.0 - (1.0 - t).powi(3);
        let lerp = |a: f32, b: f32| a + (b - a) * e;
        let zoom = (self.from.zoom.ln() + (self.to.zoom.ln() - self.from.zoom.ln()) * e).exp();
        (Camera { center: [lerp(self.from.center[0], self.to.center[0]), lerp(self.from.center[1], self.to.center[1])], zoom }, false)
    }
}

#[derive(Debug, Clone)]
struct Presentation {
    stops: Vec<Stop>,
    at: usize,
    /// The spotlight as it was before (it's on while presenting).
    spotlight_before: bool,
    /// The window size the current stop was fitted to.
    fitted_to: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stop {
    /// Everything on the board.
    Whole,
    Item(Uuid),
}

/// The camera that shows all of `rect` in `viewport`, `margin` (a share of
/// the window) left around it.
pub(crate) fn fit_rect(rect: [f32; 4], viewport: [f32; 2], margin: f32, max_zoom: f32) -> Camera {
    let room = 1.0 - 2.0 * margin;
    let zoom = (viewport[0] * room / rect[2].max(1.0)).min(viewport[1] * room / rect[3].max(1.0)).clamp(MIN_ZOOM, max_zoom.min(MAX_ZOOM));
    Camera { center: [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0], zoom }
}

/// Whether a shared view changed enough to send again: half a screen pixel
/// at `zoom`, or turning on or off.
pub fn view_moved(new: Option<[f32; 4]>, old: Option<[f32; 4]>, zoom: f32) -> bool {
    match (new, old) {
        (Some(a), Some(b)) => a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() * zoom >= 0.5),
        (a, b) => a.is_some() != b.is_some(),
    }
}

/// Things worth stopping at in a presentation (not lines, arrows, comments).
fn is_stop(i: &BoardItem) -> bool {
    matches!(
        i.kind,
        ItemKind::Media { .. } | ItemKind::Note { .. } | ItemKind::Label { .. } | ItemKind::Todo { .. } | ItemKind::Group { .. } | ItemKind::Palette { .. } | ItemKind::Link { .. } | ItemKind::Board { .. }
    )
}

/// Items in the order you'd read them: rows from the top, each left to
/// right. An item joins a row if its middle is above that row's first
/// item's bottom.
pub(crate) fn reading_order(mut items: Vec<(Uuid, [f64; 4])>) -> Vec<Uuid> {
    items.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]).then(a.1[0].total_cmp(&b.1[0])));
    let mut rows: Vec<(f64, Vec<(Uuid, [f64; 4])>)> = Vec::new();
    for it in items {
        let middle = it.1[1] + it.1[3] / 2.0;
        match rows.last_mut() {
            Some((bottom, row)) if middle < *bottom => row.push(it),
            _ => rows.push((it.1[1] + it.1[3], vec![it])),
        }
    }
    rows.into_iter()
        .flat_map(|(_, mut row)| {
            row.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
            row.into_iter().map(|(id, _)| id)
        })
        .collect()
}

/// What a click on the banner or the presentation controls does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    StopFollowing,
    Rejoin,
    StopSpotlight,
    Prev,
    Next,
    End,
    /// Part of a control that does nothing (but isn't the board).
    Nothing,
}

#[derive(Default)]
struct Layout {
    boxes: Vec<PanelBox>,
    texts: Vec<TextRun>,
    hits: Vec<([f32; 4], Hit)>,
}

const PILL_H: f32 = 30.0;
const FRAME_W: f32 = 3.0;
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

impl App {
    // ---- what the shell sends ---------------------------------------------

    /// Our id on the live server (from its welcome), or None when offline.
    pub fn set_live_id(&mut self, id: Option<String>) {
        self.together.my_id = id;
    }

    /// The part of the board we see, as [x, y, w, h] in board units, while
    /// someone follows us or we lead (else None: nothing to send).
    pub fn shared_view(&self) -> Option<[f32; 4]> {
        let me = self.together.my_id.as_deref();
        let followed = me.is_some() && self.peers.iter().any(|p| p.follow.as_deref() == me);
        (self.together.spotlight || followed).then(|| self.visible_rect())
    }

    /// The board area the window shows.
    pub(crate) fn visible_rect(&self) -> [f32; 4] {
        let (w, h) = (self.viewport[0] / self.camera.zoom, self.viewport[1] / self.camera.zoom);
        [self.camera.center[0] - w / 2.0, self.camera.center[1] - h / 2.0, w, h]
    }

    /// Who we follow right now (chosen, or someone's spotlight), by id.
    pub fn follow_target(&self) -> Option<String> {
        self.leader().map(|p| p.id.clone())
    }

    /// We bring everyone on the board to our view.
    pub fn spotlight_on(&self) -> bool {
        self.together.spotlight
    }

    /// Showing the board as a presentation.
    pub fn presenting(&self) -> bool {
        self.together.present.is_some()
    }

    /// The person whose view ours follows, if any.
    pub fn leader(&self) -> Option<&crate::Peer> {
        let t = &self.together;
        if t.spotlight || t.present.is_some() {
            return None;
        }
        if let Some(id) = &t.following {
            return self.peers.iter().find(|p| &p.id == id);
        }
        self.peers.iter().find(|p| p.spot && t.stepped_out.as_deref() != Some(p.id.as_str()) && t.my_id.as_deref() != Some(p.id.as_str()))
    }

    /// Someone opened another board. If we follow them, we go too (when
    /// it's a board we have).
    pub fn peer_went(&mut self, id: &str, board: Uuid) {
        if self.leader().map(|p| p.id.as_str()) != Some(id) && self.together.following.as_deref() != Some(id) {
            return;
        }
        if board == self.doc.board.id || !self.board_names.contains_key(&board) {
            return;
        }
        self.open_board(board, false);
        // Not a move of ours: keep following.
        self.together.placed = Some(self.camera);
        self.together.aim = None;
        self.together.glide = None;
        self.together.missing_since = Some(self.now_ms);
    }

    /// Follow someone (or stop, with None).
    pub fn follow(&mut self, id: Option<String>) {
        self.together.following = id;
        self.together.missing_since = None;
        self.together.aim = None;
        self.together.glide = None;
        if self.together.following.is_some() {
            self.together.spotlight = false;
            self.together.stepped_out = None;
        }
        self.needs_redraw = true;
    }

    pub fn following(&self) -> Option<&str> {
        self.together.following.as_deref()
    }

    /// Bring everyone on the board to our view (or stop).
    pub fn set_spotlight(&mut self, on: bool) {
        // Only on a shared board, and not from a view-only link.
        let on = on && self.my_name.is_some() && !self.view_only;
        self.together.spotlight = on;
        if on {
            self.together.following = None;
        }
        self.needs_redraw = true;
    }

    // ---- presenting --------------------------------------------------------

    fn present_stops(&self) -> Vec<Stop> {
        let picked = !self.selection.is_empty();
        let items: Vec<&BoardItem> = self.doc.board.live_items().filter(|i| is_stop(i) && (!picked || self.selection.contains(&i.id))).collect();
        let groups: Vec<(Uuid, [f64; 4])> = items.iter().filter(|i| matches!(i.kind, ItemKind::Group { .. })).map(|i| (i.id, [i.frame.x, i.frame.y, i.frame.w, i.frame.h])).collect();
        // What's inside a group is shown with it.
        let in_group = |i: &BoardItem| {
            let (cx, cy) = (i.frame.x + i.frame.w / 2.0, i.frame.y + i.frame.h / 2.0);
            groups.iter().any(|(g, r)| *g != i.id && cx >= r[0] && cx <= r[0] + r[2] && cy >= r[1] && cy <= r[1] + r[3])
        };
        let order = reading_order(items.iter().filter(|i| !in_group(i)).map(|i| (i.id, [i.frame.x, i.frame.y, i.frame.w, i.frame.h])).collect());
        let mut stops: Vec<Stop> = order.into_iter().map(Stop::Item).collect();
        if !picked && !stops.is_empty() {
            stops.insert(0, Stop::Whole);
            stops.push(Stop::Whole);
        }
        stops
    }

    /// Starts presenting this board (or the selection).
    pub fn start_presenting(&mut self) {
        if self.together.present.is_some() {
            return;
        }
        let stops = self.present_stops();
        if stops.is_empty() {
            return;
        }
        self.end_edit();
        self.search = None;
        self.menu_open = None;
        self.share = None;
        self.selection.clear();
        self.together.following = None;
        let before = self.together.spotlight;
        self.set_spotlight(true);
        self.together.present = Some(Presentation { stops, at: 0, spotlight_before: before, fitted_to: self.viewport });
        self.shell_commands.push(menu::ShellCommand::Fullscreen(true));
        self.show_stop();
    }

    pub fn stop_presenting(&mut self) {
        let Some(p) = self.together.present.take() else { return };
        self.together.spotlight = p.spotlight_before && self.my_name.is_some();
        self.together.glide = None;
        self.shell_commands.push(menu::ShellCommand::Fullscreen(false));
        self.needs_redraw = true;
    }

    /// Steps the presentation forward (1) or back (-1); past the end, it ends.
    pub fn present_step(&mut self, by: i32) {
        let Some(p) = &mut self.together.present else { return };
        let next = p.at as i64 + by as i64;
        if next >= p.stops.len() as i64 {
            self.stop_presenting();
            return;
        }
        p.at = next.max(0) as usize;
        self.show_stop();
    }

    fn present_go(&mut self, at: usize) {
        let Some(p) = &mut self.together.present else { return };
        p.at = at.min(p.stops.len().saturating_sub(1));
        self.show_stop();
    }

    /// (stop number from 1, how many).
    pub fn present_position(&self) -> Option<(usize, usize)> {
        self.together.present.as_ref().map(|p| (p.at + 1, p.stops.len()))
    }

    /// Glides to the current stop.
    fn show_stop(&mut self) {
        let Some(p) = &mut self.together.present else { return };
        p.fitted_to = self.viewport;
        let stop = p.stops[p.at];
        let to = match stop {
            Stop::Whole => self.board_bounds().map(|r| fit_rect(r, self.viewport, 0.0, 1.0)),
            Stop::Item(id) => self.doc.item(&id).map(|i| {
                let r = [i.frame.x as f32, i.frame.y as f32, i.frame.w as f32, i.frame.h as f32];
                fit_rect(r, self.viewport, STOP_MARGIN, STOP_MAX_ZOOM)
            }),
        };
        if let Some(to) = to {
            self.glide_to(to, JUMP_MS);
        }
        self.needs_redraw = true;
    }

    fn glide_to(&mut self, to: Camera, ms: u64) {
        self.together.glide = Some(Glide { from: self.camera, to, start_ms: self.now_ms, ms });
        self.together.placed = Some(self.camera);
    }

    // ---- each frame ----------------------------------------------------------

    /// Moves the camera along (following, presenting). True while it's still
    /// moving and needs another frame.
    pub(crate) fn together_tick(&mut self) -> bool {
        let now = self.now_ms;
        // We moved the board ourselves since the glide placed it.
        if let Some(p) = self.together.placed {
            if p != self.camera {
                self.together.placed = None;
                self.together.glide = None;
                self.together.aim = None;
                if let Some(id) = self.leader().map(|p| p.id.clone()) {
                    if self.together.following.as_deref() == Some(id.as_str()) {
                        self.together.following = None;
                    } else {
                        self.together.stepped_out = Some(id);
                    }
                }
            }
        }
        // A spotlight that ended: the next one catches us again.
        if let Some(s) = &self.together.stepped_out {
            if !self.peers.iter().any(|p| &p.id == s && p.spot) {
                self.together.stepped_out = None;
            }
        }
        // Someone we follow who left (not just for another board).
        if let Some(f) = &self.together.following {
            if self.peers.iter().any(|p| &p.id == f) {
                self.together.missing_since = None;
            } else {
                let since = *self.together.missing_since.get_or_insert(now);
                if now.saturating_sub(since) > LOST_MS {
                    self.together.following = None;
                    self.together.missing_since = None;
                }
            }
        }
        // The window changed size mid-presentation: fit the stop again.
        if let Some(p) = &self.together.present {
            if p.fitted_to != self.viewport {
                self.show_stop();
            }
        }
        let target = self.leader().and_then(|p| p.view).map(|v| fit_rect(v, self.viewport, 0.0, MAX_ZOOM));
        match target {
            Some(to) if self.together.aim != Some(to) => {
                let first = self.together.aim.is_none() && self.together.glide.is_none();
                self.together.aim = Some(to);
                self.glide_to(to, if first { JUMP_MS } else { FOLLOW_MS });
            }
            Some(_) => {}
            None if self.together.present.is_none() => self.together.aim = None,
            None => {}
        }
        let Some(g) = self.together.glide else { return false };
        let (cam, done) = g.at(now);
        self.camera = cam;
        self.together.placed = Some(cam);
        if done {
            self.together.glide = None;
        }
        !done
    }

    // ---- input -------------------------------------------------------------

    /// Keys while presenting or following. True when used.
    pub(crate) fn together_key(&mut self, key: Key) -> bool {
        if self.together.present.is_some() {
            match key {
                Key::Right | Key::Enter | Key::Space { pressed: true } | Key::Char(' ') => self.present_step(1),
                Key::Left | Key::Backspace => self.present_step(-1),
                Key::Home => self.present_go(0),
                Key::End => self.present_go(usize::MAX),
                Key::Escape | Key::Char('P') => self.stop_presenting(),
                Key::Char('f' | 'F') => self.present_go(0),
                // Nothing else while presenting: no edits by accident.
                _ => {}
            }
            self.needs_redraw = true;
            return true;
        }
        if self.is_editing() {
            return false;
        }
        match key {
            Key::Escape if self.leader().is_some() => {
                self.step_out();
                true
            }
            Key::Escape if self.together.spotlight => {
                self.set_spotlight(false);
                true
            }
            Key::Char('P') => {
                self.start_presenting();
                true
            }
            _ => false,
        }
    }

    /// Stop following whoever we follow now.
    fn step_out(&mut self) {
        let Some(id) = self.leader().map(|p| p.id.clone()) else { return };
        if self.together.following.as_deref() == Some(id.as_str()) {
            self.together.following = None;
        } else {
            self.together.stepped_out = Some(id);
        }
        self.together.glide = None;
        self.together.aim = None;
        self.needs_redraw = true;
    }

    /// A press on the banner or the presentation controls. True when used.
    pub(crate) fn together_press(&mut self, pos: [f32; 2]) -> bool {
        let l = self.together_layout();
        let Some(hit) = l.hits.iter().find(|(r, _)| inside(*r, pos)).map(|(_, h)| *h) else { return false };
        match hit {
            Hit::StopFollowing => self.step_out(),
            Hit::Rejoin => {
                self.together.stepped_out = None;
                self.together.aim = None;
            }
            Hit::StopSpotlight => self.set_spotlight(false),
            Hit::Prev => self.present_step(-1),
            Hit::Next => self.present_step(1),
            Hit::End => self.stop_presenting(),
            Hit::Nothing => {}
        }
        self.needs_redraw = true;
        true
    }

    /// A click on someone's badge: follow them, or stop if we already do.
    pub(crate) fn badge_follow(&mut self, id: String) {
        if self.leader().map(|p| p.id.as_str()) == Some(id.as_str()) {
            self.step_out();
        } else {
            self.follow(Some(id));
        }
    }

    // ---- drawing -----------------------------------------------------------

    fn together_layout(&mut self) -> Layout {
        let mut l = Layout::default();
        let vp = self.viewport;
        let th = theme::t();
        if let Some(p) = &self.together.present {
            let (at, n) = (p.at + 1, p.stops.len());
            let watching = if self.together.spotlight && !self.peers.is_empty() { format!("{} watching", self.peers.len()) } else { String::new() };
            controls(&mut self.font, &mut l, vp, self.cursor, at, n, &watching);
            return l;
        }
        let top = menu::BAR + 10.0;
        if let Some(p) = self.leader() {
            let c = p.paint();
            let what = if p.spot && self.together.following.is_none() { format!("{} is presenting", p.name) } else { format!("Following {}", p.name) };
            // A frame in their colour round the window: you're seeing what they see.
            let (w, h) = (vp[0], vp[1] - menu::BAR);
            for r in [[0.0, menu::BAR, w, FRAME_W], [0.0, vp[1] - FRAME_W, w, FRAME_W], [0.0, menu::BAR, FRAME_W, h], [w - FRAME_W, menu::BAR, FRAME_W, h]] {
                l.boxes.push(PanelBox { rect: r, fill: c, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
            }
            pill(&mut self.font, &mut l, vp, &what, "Stop", c, WHITE, [1.0, 1.0, 1.0, 0.85], Some(Hit::StopFollowing), top);
            return l;
        }
        if let Some(s) = self.together.stepped_out.as_ref().and_then(|s| self.peers.iter().find(|p| &p.id == s)) {
            let what = format!("{} is presenting", s.name);
            pill(&mut self.font, &mut l, vp, &what, "Follow", th.panel_bg, th.ink, th.accent_text, Some(Hit::Rejoin), top);
            return l;
        }
        if self.together.spotlight {
            let fill = crate::presence::pick(self.my_color).unwrap_or(th.accent);
            let what = match self.peers.len() {
                0 => "Everyone who comes here will follow you".to_owned(),
                1 => format!("{} is following you", self.peers[0].name),
                n => format!("Everyone here ({n}) is following you"),
            };
            pill(&mut self.font, &mut l, vp, &what, "Stop", fill, WHITE, [1.0, 1.0, 1.0, 0.85], Some(Hit::StopSpotlight), top);
            return l;
        }
        let me = self.together.my_id.as_deref();
        let followers: Vec<&str> = self.peers.iter().filter(|p| me.is_some() && p.follow.as_deref() == me).map(|p| p.name.as_str()).collect();
        let what = match followers[..] {
            [] => return l,
            [a] => format!("{a} is following you"),
            [a, b] => format!("{a} and {b} are following you"),
            _ => format!("{} people are following you", followers.len()),
        };
        pill(&mut self.font, &mut l, vp, &what, "", th.panel_bg, th.dim, th.dim, None, top);
        l
    }

    pub(crate) fn draw_together(&mut self, renderer: &mut Renderer) {
        if self.together.present.is_none() && self.peers.is_empty() && !self.together.spotlight {
            return;
        }
        let l = self.together_layout();
        if l.boxes.is_empty() {
            return;
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

/// A pill in the middle at `y`: the text, then an action after a divider.
#[allow(clippy::too_many_arguments)]
fn pill(font: &mut wall_text::Font, l: &mut Layout, vp: [f32; 2], main: &str, action: &str, fill: [f32; 4], ink: [f32; 4], act_ink: [f32; 4], hit: Option<Hit>, y: f32) {
    let th = theme::t();
    let size = 12.5;
    let main = fit(font, main, size, (vp[0] - 120.0).max(80.0));
    let mw = text_width(font, &main, size);
    let aw = if action.is_empty() { 0.0 } else { text_width(font, action, size) + 19.0 };
    let w = mw + aw + 28.0;
    let r = [(vp[0] - w) / 2.0, y, w, PILL_H];
    l.boxes.push(theme::shadow(r, PILL_H / 2.0));
    let stroke = if fill == th.panel_bg { th.line } else { [0.0; 4] };
    l.boxes.push(PanelBox { rect: r, fill, stroke, radius: PILL_H / 2.0, stroke_w: 1.0 });
    l.texts.push(TextRun { text: main, at: [r[0] + 14.0, r[1] + 19.5], size, color: ink });
    if !action.is_empty() {
        let x = r[0] + 14.0 + mw + 19.0;
        l.boxes.push(PanelBox { rect: [x - 10.0, r[1] + 8.0, 1.0, PILL_H - 16.0], fill: [ink[0], ink[1], ink[2], 0.35], stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
        l.texts.push(TextRun { text: action.into(), at: [x, r[1] + 19.5], size, color: act_ink });
    }
    if let Some(h) = hit {
        l.hits.push((r, h));
    }
}

/// The presentation controls at the bottom middle: ‹  3 / 12  ›  |  End.
fn controls(font: &mut wall_text::Font, l: &mut Layout, vp: [f32; 2], hover: [f32; 2], at: usize, n: usize, watching: &str) {
    let th = theme::t();
    let size = 13.0;
    let btn = 34.0;
    let count = format!("{at} / {n}");
    let cw = text_width(font, "88 / 88", size).max(text_width(font, &count, size)) + 16.0;
    let ww = if watching.is_empty() { 0.0 } else { text_width(font, watching, 12.0) + 22.0 };
    let ew = text_width(font, "End", size) + 26.0;
    let w = btn * 2.0 + cw + ww + ew + 15.0;
    let r = [(vp[0] - w) / 2.0, vp[1] - 20.0 - 36.0, w, 36.0];
    l.boxes.push(theme::shadow(r, 18.0));
    l.boxes.push(PanelBox { rect: r, fill: th.panel_bg, stroke: th.line, radius: 18.0, stroke_w: 1.0 });
    let mut x = r[0] + 6.0;
    let mut button = |l: &mut Layout, w: f32, text: &str, hit: Option<Hit>, ink: [f32; 4], size: f32| {
        let b = [x, r[1] + 3.0, w, 30.0];
        if hit.is_some() && inside(b, hover) {
            l.boxes.push(PanelBox { rect: b, fill: th.hover, stroke: [0.0; 4], radius: 15.0, stroke_w: 0.0 });
        }
        let tw = text_width(font, text, size);
        l.texts.push(TextRun { text: text.into(), at: [b[0] + (w - tw) / 2.0, b[1] + 15.0 + size * 0.36], size, color: ink });
        if let Some(h) = hit {
            l.hits.push((b, h));
        }
        x += w;
    };
    button(l, btn, "‹", Some(Hit::Prev), if at > 1 { th.ink } else { th.dim }, 20.0);
    button(l, cw, &count, None, th.ink, size);
    button(l, btn, "›", Some(Hit::Next), th.ink, 20.0);
    if !watching.is_empty() {
        button(l, ww, watching, None, th.dim, 12.0);
    }
    let divider = r[0] + 6.0 + btn * 2.0 + cw + ww + 1.0;
    l.boxes.push(PanelBox { rect: [divider, r[1] + 9.0, 1.0, 18.0], fill: th.line, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
    x = divider + 2.0;
    let b = [x, r[1] + 3.0, ew, 30.0];
    if inside(b, hover) {
        l.boxes.push(PanelBox { rect: b, fill: th.hover, stroke: [0.0; 4], radius: 15.0, stroke_w: 0.0 });
    }
    let tw = text_width(font, "End", size);
    l.texts.push(TextRun { text: "End".into(), at: [b[0] + (ew - tw) / 2.0, b[1] + 15.0 + size * 0.36], size, color: th.accent_text });
    l.hits.push((b, Hit::End));
    // The rest of the bar takes the click too, so a near miss doesn't hit the board.
    l.hits.push((r, Hit::Nothing));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Mods, Peer};
    use wall_format::{Board, Frame};

    fn app() -> App {
        let mut a = App::new(Board::new("t", 0), "test");
        a.resize([1000.0, 800.0]);
        a.camera = Camera { center: [500.0, 400.0], zoom: 1.0 };
        a.set_time(1_000);
        a
    }

    fn note(a: &mut App, x: f64, y: f64, w: f64, h: f64) -> Uuid {
        let f = Frame { x, y, w, h, ..Default::default() };
        a.doc.add(vec![(f, ItemKind::Note { text: "n".into(), color: None })])[0]
    }

    /// Runs frames until the camera stops moving (time moves 50 ms a frame).
    fn settle(a: &mut App) {
        for _ in 0..100 {
            let now = a.now_ms + 50;
            a.set_time(now);
            if !a.together_tick() {
                return;
            }
        }
        panic!("still moving");
    }

    fn mia(view: Option<[f32; 4]>, spot: bool) -> Peer {
        Peer { id: "mia".into(), name: "Mia".into(), view, spot, ..Default::default() }
    }

    #[test]
    fn reading_order_goes_in_rows_left_to_right() {
        let [a, b, c, d] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        // b is a bit lower than a but on the same row; d starts a new row.
        let order = reading_order(vec![(d, [0.0, 300.0, 100.0, 100.0]), (b, [200.0, 20.0, 100.0, 100.0]), (a, [0.0, 0.0, 100.0, 100.0]), (c, [400.0, 0.0, 50.0, 50.0])]);
        assert_eq!(order, vec![a, b, c, d]);
    }

    #[test]
    fn presenting_steps_through_groups_and_items_from_the_whole_board() {
        let mut a = app();
        let n1 = note(&mut a, 0.0, 0.0, 100.0, 100.0);
        let g = a.add_group("G", [300.0, 0.0, 400.0, 300.0]);
        let inside = note(&mut a, 350.0, 80.0, 100.0, 100.0);
        let n2 = note(&mut a, 0.0, 600.0, 200.0, 100.0);
        assert_eq!(a.present_stops(), vec![Stop::Whole, Stop::Item(n1), Stop::Item(g), Stop::Item(n2), Stop::Whole], "what's in a group goes with it");
        let _ = inside;
        a.start_presenting();
        assert!(a.presenting());
        assert!(a.take_shell_commands().contains(&menu::ShellCommand::Fullscreen(true)));
        assert_eq!(a.present_position(), Some((1, 5)));
        a.key(Key::Right);
        settle(&mut a);
        assert_eq!(a.present_position(), Some((2, 5)));
        // The note fills the window, a little room around it, no more than 4×.
        assert!((a.camera.center[0] - 50.0).abs() < 0.01 && (a.camera.center[1] - 50.0).abs() < 0.01);
        assert!((a.camera.zoom - 4.0).abs() < 0.01);
        a.key(Key::Right);
        settle(&mut a);
        let want = fit_rect([300.0, 0.0, 400.0, 300.0], [1000.0, 800.0], STOP_MARGIN, STOP_MAX_ZOOM);
        assert!((a.camera.zoom - want.zoom).abs() < 1e-3, "the group");
        // Keys don't edit while presenting; ← goes back; Esc ends.
        a.key(Key::Delete);
        assert_eq!(a.doc.board.live_items().count(), 4);
        a.key(Key::Left);
        assert_eq!(a.present_position(), Some((2, 5)));
        // A click doesn't select anything.
        a.pointer_down(Button::Primary, [500.0, 400.0], Mods::default());
        a.pointer_up(Button::Primary, [500.0, 400.0]);
        assert!(a.selection.is_empty());
        a.key(Key::Escape);
        assert!(!a.presenting());
        assert!(a.take_shell_commands().contains(&menu::ShellCommand::Fullscreen(false)));

        // With a selection: just those, no whole-board stops; past the end it ends.
        a.selection = [n2, n1].into();
        a.key(Key::Char('P'));
        assert_eq!(a.present_position(), Some((1, 2)));
        a.key(Key::Space { pressed: true });
        a.key(Key::Space { pressed: true });
        assert!(!a.presenting());
    }

    #[test]
    fn following_someone_glides_to_their_view_until_you_move() {
        let mut a = app();
        a.set_live_id(Some("me".into()));
        a.set_peers(vec![mia(Some([1000.0, 1000.0, 500.0, 400.0]), false)]);
        assert_eq!(a.follow_target(), None);
        a.badge_follow("mia".into());
        assert_eq!(a.follow_target().as_deref(), Some("mia"));
        settle(&mut a);
        assert!((a.camera.center[0] - 1250.0).abs() < 0.01 && (a.camera.zoom - 2.0).abs() < 1e-3);
        // They move: we go with them.
        a.set_peers(vec![mia(Some([0.0, 0.0, 1000.0, 800.0]), false)]);
        settle(&mut a);
        assert!((a.camera.center[0] - 500.0).abs() < 0.01 && (a.camera.zoom - 1.0).abs() < 1e-3);
        // We scroll: that's us stepping out.
        a.wheel(crate::Wheel::Lines(1.0));
        a.together_tick();
        assert_eq!(a.follow_target(), None);
        let cam = a.camera;
        a.set_peers(vec![mia(Some([5000.0, 0.0, 1000.0, 800.0]), false)]);
        settle(&mut a);
        assert_eq!(a.camera, cam, "not following any more");
        // Esc stops too.
        a.follow(Some("mia".into()));
        a.together_tick();
        a.key(Key::Escape);
        assert_eq!(a.follow_target(), None);
    }

    #[test]
    fn a_spotlight_brings_everyone_and_they_can_step_out_and_back() {
        let mut a = app();
        a.set_live_id(Some("me".into()));
        a.set_peers(vec![mia(Some([1000.0, 1000.0, 1000.0, 800.0]), true)]);
        assert_eq!(a.follow_target().as_deref(), Some("mia"), "followed without asking");
        settle(&mut a);
        assert!((a.camera.center[0] - 1500.0).abs() < 0.01);
        a.camera.pan(100.0, 0.0);
        a.together_tick();
        assert_eq!(a.follow_target(), None, "stepped out");
        assert!(a.together_layout().hits.iter().any(|(_, h)| *h == Hit::Rejoin));
        let r = a.together_layout().hits[0].0;
        a.pointer_down(Button::Primary, [r[0] + 4.0, r[1] + 4.0], Mods::default());
        assert_eq!(a.follow_target().as_deref(), Some("mia"), "back in from the banner");
        // When their spotlight ends, the next one catches us again.
        a.camera.pan(100.0, 0.0);
        a.together_tick();
        a.set_peers(vec![mia(None, false)]);
        a.together_tick();
        a.set_peers(vec![mia(None, true)]);
        assert_eq!(a.follow_target().as_deref(), Some("mia"));
    }

    #[test]
    fn our_view_is_shared_only_while_someone_follows_or_we_lead() {
        let mut a = app();
        a.set_live_id(Some("me".into()));
        a.set_peers(vec![Peer { id: "jon".into(), name: "Jon".into(), ..Default::default() }]);
        assert_eq!(a.shared_view(), None);
        a.set_peers(vec![Peer { id: "jon".into(), name: "Jon".into(), follow: Some("me".into()), ..Default::default() }]);
        assert_eq!(a.shared_view(), Some([0.0, 0.0, 1000.0, 800.0]));
        assert!(a.together_layout().texts.iter().any(|t| t.text == "Jon is following you"));
        // Spotlight needs a live connection, and not a view-only link.
        a.set_peers(Vec::new());
        a.set_spotlight(true);
        assert!(!a.spotlight_on());
        a.set_my_name(Some("Paul".into()));
        a.set_view_only(true);
        a.run_command(menu::Command::Spotlight);
        assert!(!a.spotlight_on());
        a.set_view_only(false);
        a.run_command(menu::Command::Spotlight);
        assert!(a.spotlight_on());
        assert!(a.shared_view().is_some());
        a.run_command(menu::Command::Spotlight);
        assert!(!a.spotlight_on());
        // Presenting turns it on, and ending puts it back.
        note(&mut a, 0.0, 0.0, 10.0, 10.0);
        a.start_presenting();
        assert!(a.spotlight_on());
        a.stop_presenting();
        assert!(!a.spotlight_on());
    }

    #[test]
    fn going_to_another_board_with_the_person_we_follow() {
        let mut a = app();
        let other = a.new_board();
        let home = a.board_id();
        assert_ne!(other, home);
        a.set_peers(vec![mia(None, false)]);
        a.follow(Some("mia".into()));
        a.peer_went("mia", other);
        assert_eq!(a.board_id(), other);
        a.together_tick();
        assert_eq!(a.following(), Some("mia"), "the board change isn't us moving");
        a.peer_went("someone-else", home);
        assert_eq!(a.board_id(), other);
    }
}
