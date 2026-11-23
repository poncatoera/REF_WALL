//! Dark and light mode: every colour the app draws comes from here, so the
//! whole UI switches at once. The choice is process-wide (one window per
//! app); shells remember it (desktop settings, web localStorage) and set it
//! with [`crate::App::set_light_mode`].

use std::sync::atomic::{AtomicBool, Ordering};

static LIGHT: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

type C = [f32; 4];

pub struct Theme {
    /// The board behind everything.
    pub background: C,
    /// Dots of the board grid.
    pub grid_dot: C,
    /// Notes, to-dos, board cards and picture placeholders without a colour.
    pub card_fill: C,
    pub card_stroke: C,
    /// Text on cards and labels.
    pub text: C,
    /// Text of ticked to-dos, and other quiet text on the board.
    pub text_done: C,
    pub todo_box: C,
    pub arrow: C,
    pub accent: C,
    // Panels, menus, the library drawer.
    pub panel_bg: C,
    pub line: C,
    pub field: C,
    pub ink: C,
    pub dim: C,
    pub chip: C,
    pub menu_bar: C,
    pub menu_drop: C,
    /// Thumbnail tiles in the library drawer.
    pub tile: C,
    /// The search box over the board.
    pub search_bg: C,
    /// A faint wash of the accent: the chosen tool, the open board, a
    /// highlighted menu entry. Text on it uses `accent_text`.
    pub accent_soft: C,
    pub accent_text: C,
    /// Text on a solid `accent` fill (primary buttons, picked chips).
    pub accent_ink: C,
    /// A button or row under the pointer.
    pub hover: C,
    /// Drop shadow under floating panels (see [`shadow`]).
    pub shadow: C,
}

/// Corner radii shared by all the chrome, in screen pixels.
pub mod radius {
    /// Floating panels, menus' drop-downs, cards over the board.
    pub const PANEL: f32 = 12.0;
    /// Fields, buttons, segmented controls.
    pub const CONTROL: f32 = 8.0;
    /// Rows and items inside a control.
    pub const INNER: f32 = 6.0;
    /// Tags and badges.
    pub const PILL: f32 = 999.0;
}

/// Premultiplied colour from 0-255 sRGB and alpha.
const fn rgb(r: u8, g: u8, b: u8, a: f32) -> C {
    [r as f32 / 255.0 * a, g as f32 / 255.0 * a, b as f32 / 255.0 * a, a]
}

/// Graphite: the dark look. Quiet chrome, hairline edges, one blue accent.
pub const DARK: Theme = Theme {
    background: rgb(0x12, 0x13, 0x15, 1.0),
    grid_dot: rgb(0x2a, 0x2c, 0x31, 1.0),
    card_fill: rgb(0x1c, 0x1d, 0x21, 1.0),
    card_stroke: rgb(0x2b, 0x2d, 0x32, 1.0),
    text: rgb(0xec, 0xed, 0xf0, 1.0),
    text_done: rgb(0x80, 0x84, 0x8d, 1.0),
    todo_box: rgb(0x74, 0x78, 0x82, 1.0),
    arrow: rgb(0x8d, 0x91, 0x9b, 1.0),
    accent: rgb(0x6e, 0xa8, 0xff, 1.0),
    panel_bg: rgb(0x1b, 0x1c, 0x1f, 1.0),
    line: rgb(0x2b, 0x2d, 0x32, 1.0),
    field: rgb(0x13, 0x14, 0x16, 1.0),
    ink: rgb(0xec, 0xed, 0xf0, 1.0),
    dim: rgb(0x9a, 0x9e, 0xa8, 1.0),
    chip: rgb(0x26, 0x28, 0x2e, 1.0),
    menu_bar: rgb(0x16, 0x17, 0x1a, 1.0),
    menu_drop: rgb(0x1f, 0x20, 0x24, 1.0),
    tile: rgb(0x13, 0x14, 0x16, 1.0),
    search_bg: rgb(0x1b, 0x1c, 0x1f, 0.97),
    accent_soft: rgb(0x6e, 0xa8, 0xff, 0.16),
    accent_text: rgb(0xa9, 0xca, 0xff, 1.0),
    accent_ink: rgb(0x08, 0x16, 0x2e, 1.0),
    hover: rgb(0xff, 0xff, 0xff, 0.05),
    shadow: rgb(0x00, 0x00, 0x00, 0.38),
};

/// Paper: the light look. Warm off-white board, white panels, vermilion.
pub const LIGHT_THEME: Theme = Theme {
    background: rgb(0xf3, 0xf1, 0xec, 1.0),
    grid_dot: rgb(0xd3, 0xce, 0xc3, 1.0),
    card_fill: rgb(0xff, 0xfe, 0xfb, 1.0),
    card_stroke: rgb(0xe3, 0xdf, 0xd6, 1.0),
    text: rgb(0x1b, 0x1a, 0x18, 1.0),
    text_done: rgb(0x8f, 0x8a, 0x80, 1.0),
    todo_box: rgb(0x9a, 0x95, 0x8b, 1.0),
    arrow: rgb(0x67, 0x63, 0x5b, 1.0),
    accent: rgb(0xd9, 0x47, 0x2f, 1.0),
    panel_bg: rgb(0xff, 0xff, 0xff, 1.0),
    line: rgb(0xe3, 0xdf, 0xd6, 1.0),
    field: rgb(0xfa, 0xf9, 0xf6, 1.0),
    ink: rgb(0x1b, 0x1a, 0x18, 1.0),
    dim: rgb(0x67, 0x63, 0x5b, 1.0),
    chip: rgb(0xec, 0xe9, 0xe2, 1.0),
    menu_bar: rgb(0xfb, 0xfa, 0xf7, 1.0),
    menu_drop: rgb(0xff, 0xff, 0xff, 1.0),
    tile: rgb(0xf3, 0xf1, 0xec, 1.0),
    search_bg: rgb(0xff, 0xff, 0xff, 0.97),
    accent_soft: rgb(0xd9, 0x47, 0x2f, 0.12),
    accent_text: rgb(0xa8, 0x33, 0x1f, 1.0),
    accent_ink: rgb(0xff, 0xff, 0xff, 1.0),
    hover: rgb(0x3c, 0x2d, 0x14, 0.05),
    shadow: rgb(0x3c, 0x2d, 0x14, 0.13),
};

/// A soft drop shadow for a floating panel at `rect` (screen pixels) with
/// corner `radius`: one feathered box, drawn before the panel. Cheap: a
/// single quad, no blur pass.
pub fn shadow(rect: [f32; 4], radius: f32) -> crate::panel::PanelBox {
    const SPREAD: f32 = 14.0;
    const DROP: f32 = 6.0;
    let c = t().shadow;
    crate::panel::PanelBox {
        rect: [rect[0] - SPREAD, rect[1] - SPREAD + DROP, rect[2] + 2.0 * SPREAD, rect[3] + 2.0 * SPREAD],
        fill: c,
        stroke: c,
        radius: radius + SPREAD,
        stroke_w: -SPREAD,
    }
}

/// The colours in use.
pub fn t() -> &'static Theme {
    if is_light() { &LIGHT_THEME } else { &DARK }
}

fn mix(c: [f32; 3], to: f32, amount: f32, alpha: f32) -> C {
    [c[0] + (to - c[0]) * amount, c[1] + (to - c[1]) * amount, c[2] + (to - c[2]) * amount, alpha]
}

/// Fill and stroke of a note with a colour: deep in dark mode, a pale tint
/// in light mode (mixed toward white), so text stays readable on both.
pub fn tinted_note(c: [f32; 3]) -> (C, C) {
    if is_light() {
        (mix(c, 1.0, 0.68, 1.0), mix(c, 0.0, 0.15, 1.0))
    } else {
        ([c[0] * 0.42, c[1] * 0.42, c[2] * 0.42, 1.0], [c[0] * 0.8, c[1] * 0.8, c[2] * 0.8, 1.0])
    }
}

/// Fill and stroke of a group with a colour.
pub fn tinted_group(c: [f32; 3]) -> (C, C) {
    if is_light() {
        (mix(c, 1.0, 0.9, 0.6), mix(c, 0.0, 0.25, 0.9))
    } else {
        ([c[0] * 0.06, c[1] * 0.06, c[2] * 0.06, 0.6], [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5, 0.9])
    }
}

/// A group's name colour: its colour, a bit darker on a light board.
pub fn group_label(c: [f32; 3]) -> C {
    if is_light() { mix(c, 0.0, 0.3, 1.0) } else { [c[0], c[1], c[2], 1.0] }
}

/// Screen spacing of grid dots and the board spacing to use at `zoom`:
/// a 24-unit grid that doubles or halves so dots stay 16–64 px apart.
pub fn grid_spacing(zoom: f32) -> f32 {
    let mut s = 24.0f32;
    while s * zoom < 16.0 {
        s *= 2.0;
    }
    while s * zoom >= 64.0 {
        s /= 2.0;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_dots_stay_16_to_64_px_apart() {
        for z in [0.01f32, 0.03, 0.1, 0.5, 1.0, 2.0, 7.5, 40.0] {
            let px = grid_spacing(z) * z;
            assert!((16.0..64.0).contains(&px), "zoom {z}: {px}px");
        }
        assert_eq!(grid_spacing(1.0), 24.0);
    }

    #[test]
    fn tints_stay_readable_in_both_modes() {
        // Light-mode tints are pale (text is dark), dark-mode tints deep.
        let red = [0.9, 0.33, 0.29];
        let lum = |c: C| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!(lum(DARK.text) - lum(tinted_note_dark(red)) > 0.5);
        assert!(lum(mix(red, 1.0, 0.68, 1.0)) - lum(LIGHT_THEME.text) > 0.5);
    }

    fn tinted_note_dark(c: [f32; 3]) -> C {
        [c[0] * 0.42, c[1] * 0.42, c[2] * 0.42, 1.0]
    }
}
