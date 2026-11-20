//! Wall's own menu bar, drawn inside the window so desktop and web get the
//! same menus. Each entry names a [`Command`]; most run inside the app, and
//! the few that need the system (file picker, clipboard) are handed to the
//! shell as [`ShellCommand`]s.

use crate::panel::{fit, text_width, PanelBox, TextRun};
use wall_text::Font;

/// Height of the menu bar in screen pixels.
pub const BAR: f32 = 30.0;
const ENTRY_H: f32 = 28.0;
const DROP_W: f32 = 236.0;

const BAR_BG: [f32; 4] = [0.09, 0.094, 0.105, 1.0];
const DROP_BG: [f32; 4] = [0.125, 0.13, 0.148, 1.0];
const LINE: [f32; 4] = [0.2, 0.21, 0.24, 1.0];
const HOVER: [f32; 4] = [0.36, 0.62, 1.0, 1.0];
const INK: [f32; 4] = [0.9, 0.9, 0.92, 1.0];
const DIM: [f32; 4] = [0.55, 0.57, 0.63, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Import,
    NewNote,
    NewBoard,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    Find,
    BringToFront,
    SendToBack,
    FitBoard,
    ZoomIn,
    ZoomOut,
    ActualSize,
    TogglePanel,
    ToggleLibrary,
    GoUp,
}

/// Commands the shell carries out, because they need the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommand {
    /// Pick files to import.
    Import,
    /// Copy the selection to the clipboard (and delete it for Cut).
    Copy { cut: bool },
    /// Paste from the clipboard.
    Paste,
}

pub struct Menu {
    pub title: &'static str,
    /// Label, shortcut, command. An empty label is a separator.
    pub entries: &'static [(&'static str, &'static str, Option<Command>)],
}

const SEP: (&str, &str, Option<Command>) = ("", "", None);

pub const MENUS: &[Menu] = &[
    Menu {
        title: "File",
        entries: &[
            ("Import pictures or videos…", "", Some(Command::Import)),
            SEP,
            ("New note", "Double-click", Some(Command::NewNote)),
            ("New board", "B", Some(Command::NewBoard)),
        ],
    },
    Menu {
        title: "Edit",
        entries: &[
            ("Undo", "Ctrl Z", Some(Command::Undo)),
            ("Redo", "Ctrl Y", Some(Command::Redo)),
            SEP,
            ("Cut", "Ctrl X", Some(Command::Cut)),
            ("Copy", "Ctrl C", Some(Command::Copy)),
            ("Paste", "Ctrl V", Some(Command::Paste)),
            ("Duplicate", "Ctrl D", Some(Command::Duplicate)),
            ("Delete", "Del", Some(Command::Delete)),
            SEP,
            ("Select all", "Ctrl A", Some(Command::SelectAll)),
            ("Find", "Ctrl F", Some(Command::Find)),
        ],
    },
    Menu {
        title: "Arrange",
        entries: &[
            ("Bring to front", "Ctrl ]", Some(Command::BringToFront)),
            ("Send to back", "Ctrl [", Some(Command::SendToBack)),
            SEP,
            ("Move without snapping", "Ctrl-drag", None),
            ("Draw an arrow", "Alt-drag", None),
            ("Crop a picture", "Ctrl-drag corner", None),
            ("Colour a note or group", "1 – 7, 0 clears", None),
        ],
    },
    Menu {
        title: "View",
        entries: &[
            ("Fit board", "F", Some(Command::FitBoard)),
            ("Zoom in", "+", Some(Command::ZoomIn)),
            ("Zoom out", "−", Some(Command::ZoomOut)),
            ("Actual size", "Z", Some(Command::ActualSize)),
            SEP,
            ("Show or hide side panel", "P", Some(Command::TogglePanel)),
            ("Library and trash", "L", Some(Command::ToggleLibrary)),
            ("Up to the parent board", "Esc", Some(Command::GoUp)),
        ],
    },
    Menu {
        title: "Video",
        entries: &[
            ("Play or pause", "K", None),
            ("Step a frame back / forward", ", / .", None),
            ("Slower / faster", "[ / ]", None),
            ("Set in point / out point", "I / O", None),
            ("Clear in and out", "Side panel", None),
            ("Scrub", "Drag the bar", None),
        ],
    },
];

#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    /// A menu title: opens that menu.
    Title(usize),
    Entry(Command),
    /// Part of the bar or a menu that does nothing.
    None,
}

#[derive(Default)]
pub struct Layout {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Hit)>,
}

impl Layout {
    /// What a click at a screen point lands on, or None outside the menus.
    pub fn hit_at(&self, p: [f32; 2]) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| crate::panel::inside(*r, p)).map(|(_, h)| h.clone())
    }

    fn rect(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w: if stroke[3] > 0.0 { 1.0 } else { 0.0 } });
    }

    fn text(&mut self, text: impl Into<String>, at: [f32; 2], size: f32, color: [f32; 4]) {
        self.texts.push(TextRun { text: text.into(), at, size, color });
    }
}

/// The menu bar, with menu `open` dropped down and the entry under `hover`
/// highlighted. `title` is shown at the right of the bar.
pub fn layout(font: &mut Font, open: Option<usize>, hover: [f32; 2], title: &str, viewport: [f32; 2]) -> Layout {
    let mut l = Layout::default();
    let bar = [0.0, 0.0, viewport[0], BAR];
    l.rect(bar, BAR_BG, [0.0; 4], 0.0);
    l.rect([0.0, BAR - 1.0, viewport[0], 1.0], LINE, [0.0; 4], 0.0);
    l.hits.push((bar, Hit::None));
    let mut x = 8.0;
    let mut title_x = Vec::new();
    for (i, m) in MENUS.iter().enumerate() {
        let w = text_width(font, m.title, 13.0) + 20.0;
        let r = [x, 3.0, w, BAR - 6.0];
        if open == Some(i) {
            l.rect(r, DROP_BG, [0.0; 4], 5.0);
        }
        l.text(m.title, [x + 10.0, 19.5], 13.0, INK);
        l.hits.push((r, Hit::Title(i)));
        title_x.push(x);
        x += w;
    }
    let t = fit(font, title, 12.5, (viewport[0] - x - 24.0).max(0.0));
    let tw = text_width(font, &t, 12.5);
    l.text(t, [viewport[0] - 12.0 - tw, 19.5], 12.5, DIM);

    if let Some(i) = open.filter(|i| *i < MENUS.len()) {
        let m = &MENUS[i];
        let h: f32 = m.entries.iter().map(|e| if e.0.is_empty() { 9.0 } else { ENTRY_H }).sum::<f32>() + 8.0;
        let x0 = title_x[i].min(viewport[0] - DROP_W - 4.0).max(0.0);
        let drop = [x0, BAR, DROP_W, h];
        l.rect(drop, DROP_BG, LINE, 6.0);
        l.hits.push((drop, Hit::None));
        let mut y = BAR + 4.0;
        for (label, keys, cmd) in m.entries {
            if label.is_empty() {
                l.rect([x0 + 8.0, y + 4.0, DROP_W - 16.0, 1.0], LINE, [0.0; 4], 0.0);
                y += 9.0;
                continue;
            }
            let r = [x0 + 4.0, y, DROP_W - 8.0, ENTRY_H];
            let hot = cmd.is_some() && crate::panel::inside(r, hover);
            if hot {
                l.rect(r, HOVER, [0.0; 4], 4.0);
            }
            let ink = if cmd.is_none() { DIM } else if hot { [1.0; 4] } else { INK };
            let kw = text_width(font, keys, 12.0);
            let label = fit(font, label, 13.0, DROP_W - 36.0 - kw);
            l.text(label, [r[0] + 10.0, y + 18.5], 13.0, ink);
            l.text(*keys, [r[0] + r[2] - 10.0 - kw, y + 18.5], 12.0, if hot { [1.0; 4] } else { DIM });
            if let Some(c) = cmd {
                l.hits.push((r, Hit::Entry(*c)));
            }
            y += ENTRY_H;
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_open_and_entries_click() {
        let mut f = Font::new(wall_text::INTER_REGULAR).unwrap();
        let closed = layout(&mut f, None, [0.0, 0.0], "Board 1", [1200.0, 800.0]);
        let edit = closed.hits.iter().find(|(_, h)| *h == Hit::Title(1)).unwrap().0;
        let c = |r: [f32; 4]| [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        assert_eq!(closed.hit_at(c(edit)), Some(Hit::Title(1)));
        assert_eq!(closed.hit_at([600.0, 15.0]), Some(Hit::None), "empty bar");
        assert_eq!(closed.hit_at([600.0, 300.0]), None, "the board");
        let open = layout(&mut f, Some(1), [0.0, 0.0], "Board 1", [1200.0, 800.0]);
        let undo = open.hits.iter().find(|(_, h)| *h == Hit::Entry(Command::Undo)).unwrap().0;
        assert!(undo[1] >= BAR);
        assert_eq!(open.hit_at(c(undo)), Some(Hit::Entry(Command::Undo)));
        // Hovering highlights the entry.
        let hovered = layout(&mut f, Some(1), c(undo), "Board 1", [1200.0, 800.0]);
        assert!(hovered.boxes.iter().any(|b| b.fill == HOVER));
    }
}
