//! Wall's own menu bar, drawn inside the window so desktop and web get the
//! same menus. Each entry names a [`Command`]; most run inside the app, and
//! the few that need the system (file picker, clipboard) are handed to the
//! shell as [`ShellCommand`]s.

use crate::panel::{fit, text_width, PanelBox, TextRun};
use wall_text::Font;

// Colours come from the theme (dark / light mode).
#[allow(non_snake_case)]
fn BAR_BG() -> [f32; 4] {
    crate::theme::t().menu_bar
}
#[allow(non_snake_case)]
fn DROP_BG() -> [f32; 4] {
    crate::theme::t().menu_drop
}
#[allow(non_snake_case)]
fn LINE() -> [f32; 4] {
    crate::theme::t().line
}
#[allow(non_snake_case)]
fn HOVER() -> [f32; 4] {
    crate::theme::t().accent
}
#[allow(non_snake_case)]
fn INK() -> [f32; 4] {
    crate::theme::t().ink
}
#[allow(non_snake_case)]
fn DIM() -> [f32; 4] {
    crate::theme::t().dim
}

/// Height of the menu bar in screen pixels.
pub const BAR: f32 = 30.0;
const ENTRY_H: f32 = 28.0;
const DROP_W: f32 = 236.0;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Import,
    OpenLibrary,
    NewLibrary,
    ImportOldWall,
    ExportImage,
    Share,
    NewNote,
    NewBoard,
    NewTodo,
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
    ToggleBoards,
    ToggleDrawing,
    ToggleTheme,
    ToggleGrid,
    GoUp,
    RotateRight,
    RotateLeft,
    Mirror,
    FlipVertical,
    BlackAndWhite,
}

/// Commands the shell carries out, because they need the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCommand {
    /// Pick files to import.
    Import,
    /// Copy the selection to the clipboard (and delete it for Cut).
    Copy { cut: bool },
    /// Paste from the clipboard.
    Paste,
    /// Pick a library folder and open it, or (`new`) create one.
    OpenLibrary { new: bool },
    /// Save the whole board as a PNG picture.
    ExportImage,
    /// Pick an old (Electron) Wall library and bring its boards in.
    ImportOldWall,
    /// Open a web address in the browser.
    OpenUrl(String),
    /// Put this text on the clipboard (a palette colour's hex code).
    CopyText(String),
    /// Something to do in the cloud; answer with `App::cloud_answer`.
    Cloud(crate::share::CloudAsk),
    /// The name others on a shared board see for this person changed: keep
    /// it and tell the live connection.
    SetName(String),
    /// The colour others see for this person (one of 8).
    SetColor(u8),
}

impl Command {
    /// Commands only a desktop shell can carry out; the web menus leave them out.
    pub fn desktop_only(self) -> bool {
        matches!(self, Command::OpenLibrary | Command::NewLibrary | Command::ImportOldWall | Command::Share)
    }
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
            ("Open library…", "", Some(Command::OpenLibrary)),
            ("New library…", "", Some(Command::NewLibrary)),
            ("Import old Wall library…", "", Some(Command::ImportOldWall)),
            SEP,
            ("Export board as picture…", "", Some(Command::ExportImage)),
            ("Share this board…", "", Some(Command::Share)),
            SEP,
            ("New note", "Double-click", Some(Command::NewNote)),
            ("New to-do list", "T", Some(Command::NewTodo)),
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
            ("Turn right", "R", Some(Command::RotateRight)),
            ("Turn left", "", Some(Command::RotateLeft)),
            ("Mirror", "H", Some(Command::Mirror)),
            ("Flip upside down", "V", Some(Command::FlipVertical)),
            ("Black and white", "", Some(Command::BlackAndWhite)),
            SEP,
            ("Move without snapping", "Ctrl-drag", None),
            ("Draw an arrow", "Alt-drag", None),
            ("Crop a picture", "Ctrl-drag corner", None),
            ("Colour a note, group or drawing", "1 – 7, 0 clears", None),
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
            ("All boards", "S", Some(Command::ToggleBoards)),
            ("Draw: pen, pencil, brush, marker, eraser", "D", Some(Command::ToggleDrawing)),
            ("Library and trash", "L", Some(Command::ToggleLibrary)),
            ("Show or hide dots", "G", Some(Command::ToggleGrid)),
            ("Light or dark mode", "", Some(Command::ToggleTheme)),
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
            ("Add or remove a marker", "M", None),
            ("Loop back and forth", "J", None),
            ("Onion skin (frames before and after)", "U", None),
            ("Previous / next marker", "; / '", None),
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

/// The entries of menu `i` this shell shows: desktop-only ones are left out
/// on the web, and so is a separator that would end up doubled or first.
fn entries(i: usize, desktop: bool) -> Vec<(&'static str, &'static str, Option<Command>)> {
    let mut out: Vec<(&str, &str, Option<Command>)> = Vec::new();
    for e in MENUS[i].entries {
        if !desktop && e.2.is_some_and(Command::desktop_only) {
            continue;
        }
        if e.0.is_empty() && out.last().is_none_or(|l| l.0.is_empty()) {
            continue;
        }
        out.push(*e);
    }
    if out.last().is_some_and(|l| l.0.is_empty()) {
        out.pop();
    }
    out
}

/// The menu bar, with menu `open` dropped down and the entry under `hover`
/// highlighted. `title` is shown at the right of the bar. `desktop` shows
/// the entries only a desktop shell can carry out.
pub fn layout(font: &mut Font, open: Option<usize>, hover: [f32; 2], title: &str, viewport: [f32; 2], desktop: bool) -> Layout {
    let mut l = Layout::default();
    let bar = [0.0, 0.0, viewport[0], BAR];
    l.rect(bar, BAR_BG(), [0.0; 4], 0.0);
    l.rect([0.0, BAR - 1.0, viewport[0], 1.0], LINE(), [0.0; 4], 0.0);
    l.hits.push((bar, Hit::None));
    let mut x = 8.0;
    let mut title_x = Vec::new();
    for (i, m) in MENUS.iter().enumerate() {
        let w = text_width(font, m.title, 13.0) + 20.0;
        let r = [x, 3.0, w, BAR - 6.0];
        if open == Some(i) {
            l.rect(r, DROP_BG(), [0.0; 4], 5.0);
        }
        l.text(m.title, [x + 10.0, 19.5], 13.0, INK());
        l.hits.push((r, Hit::Title(i)));
        title_x.push(x);
        x += w;
    }
    let t = fit(font, title, 12.5, (viewport[0] - x - 24.0).max(0.0));
    let tw = text_width(font, &t, 12.5);
    l.text(t, [viewport[0] - 12.0 - tw, 19.5], 12.5, DIM());

    if let Some(i) = open.filter(|i| *i < MENUS.len()) {
        let m = entries(i, desktop);
        let h: f32 = m.iter().map(|e| if e.0.is_empty() { 9.0 } else { ENTRY_H }).sum::<f32>() + 8.0;
        let x0 = title_x[i].min(viewport[0] - DROP_W - 4.0).max(0.0);
        let drop = [x0, BAR, DROP_W, h];
        l.rect(drop, DROP_BG(), LINE(), 6.0);
        l.hits.push((drop, Hit::None));
        let mut y = BAR + 4.0;
        for (label, keys, cmd) in &m {
            if label.is_empty() {
                l.rect([x0 + 8.0, y + 4.0, DROP_W - 16.0, 1.0], LINE(), [0.0; 4], 0.0);
                y += 9.0;
                continue;
            }
            let r = [x0 + 4.0, y, DROP_W - 8.0, ENTRY_H];
            let hot = cmd.is_some() && crate::panel::inside(r, hover);
            if hot {
                l.rect(r, HOVER(), [0.0; 4], 4.0);
            }
            let ink = if cmd.is_none() { DIM() } else if hot { [1.0; 4] } else { INK() };
            let kw = text_width(font, keys, 12.0);
            let label = fit(font, label, 13.0, DROP_W - 36.0 - kw);
            l.text(label, [r[0] + 10.0, y + 18.5], 13.0, ink);
            l.text(*keys, [r[0] + r[2] - 10.0 - kw, y + 18.5], 12.0, if hot { [1.0; 4] } else { DIM() });
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
        let closed = layout(&mut f, None, [0.0, 0.0], "Board 1", [1200.0, 800.0], false);
        let edit = closed.hits.iter().find(|(_, h)| *h == Hit::Title(1)).unwrap().0;
        let c = |r: [f32; 4]| [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        assert_eq!(closed.hit_at(c(edit)), Some(Hit::Title(1)));
        assert_eq!(closed.hit_at([600.0, 15.0]), Some(Hit::None), "empty bar");
        assert_eq!(closed.hit_at([600.0, 300.0]), None, "the board");
        let open = layout(&mut f, Some(1), [0.0, 0.0], "Board 1", [1200.0, 800.0], false);
        let undo = open.hits.iter().find(|(_, h)| *h == Hit::Entry(Command::Undo)).unwrap().0;
        assert!(undo[1] >= BAR);
        assert_eq!(open.hit_at(c(undo)), Some(Hit::Entry(Command::Undo)));
        // Hovering highlights the entry.
        let hovered = layout(&mut f, Some(1), c(undo), "Board 1", [1200.0, 800.0], false);
        assert!(hovered.boxes.iter().any(|b| b.fill == HOVER()));
    }

    #[test]
    fn library_entries_only_on_desktop() {
        let mut f = Font::new(wall_text::INTER_REGULAR).unwrap();
        let has = |l: &Layout| l.hits.iter().any(|(_, h)| *h == Hit::Entry(Command::OpenLibrary));
        let web = layout(&mut f, Some(0), [0.0, 0.0], "B", [1200.0, 800.0], false);
        let desk = layout(&mut f, Some(0), [0.0, 0.0], "B", [1200.0, 800.0], true);
        assert!(!has(&web) && has(&desk));
        // The web File menu doesn't get two separators in a row.
        let seps = |d: bool| entries(0, d).windows(2).filter(|w| w[0].0.is_empty() && w[1].0.is_empty()).count();
        assert_eq!((seps(false), seps(true)), (0, 0));
        assert!(desk.boxes.len() > web.boxes.len());
    }
}
