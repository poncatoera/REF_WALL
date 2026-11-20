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
}

pub const DARK: Theme = Theme {
    background: [0.075, 0.078, 0.086, 1.0],
    grid_dot: [0.24, 0.25, 0.29, 1.0],
    card_fill: [0.13, 0.135, 0.15, 1.0],
    card_stroke: [0.24, 0.25, 0.28, 1.0],
    text: [0.92, 0.92, 0.94, 1.0],
    text_done: [0.5, 0.52, 0.57, 1.0],
    todo_box: [0.45, 0.47, 0.52, 1.0],
    arrow: [0.62, 0.64, 0.7, 1.0],
    accent: [0.36, 0.62, 1.0, 1.0],
    panel_bg: [0.105, 0.11, 0.125, 1.0],
    line: [0.2, 0.21, 0.24, 1.0],
    field: [0.075, 0.078, 0.086, 1.0],
    ink: [0.92, 0.92, 0.94, 1.0],
    dim: [0.56, 0.58, 0.64, 1.0],
    chip: [0.17, 0.19, 0.24, 1.0],
    menu_bar: [0.09, 0.094, 0.105, 1.0],
    menu_drop: [0.125, 0.13, 0.148, 1.0],
    tile: [0.075, 0.078, 0.086, 1.0],
    search_bg: [0.1, 0.105, 0.12, 0.97],
};

pub const LIGHT_THEME: Theme = Theme {
    background: [0.955, 0.955, 0.962, 1.0],
    grid_dot: [0.76, 0.77, 0.8, 1.0],
    card_fill: [1.0, 1.0, 1.0, 1.0],
    card_stroke: [0.82, 0.83, 0.86, 1.0],
    text: [0.11, 0.115, 0.13, 1.0],
    text_done: [0.55, 0.57, 0.62, 1.0],
    todo_box: [0.6, 0.62, 0.67, 1.0],
    arrow: [0.42, 0.44, 0.5, 1.0],
    accent: [0.2, 0.47, 0.95, 1.0],
    panel_bg: [0.99, 0.99, 0.995, 1.0],
    line: [0.86, 0.87, 0.9, 1.0],
    field: [0.955, 0.955, 0.962, 1.0],
    ink: [0.11, 0.115, 0.13, 1.0],
    dim: [0.45, 0.47, 0.53, 1.0],
    chip: [0.9, 0.92, 0.96, 1.0],
    menu_bar: [0.93, 0.93, 0.945, 1.0],
    menu_drop: [1.0, 1.0, 1.0, 1.0],
    tile: [0.93, 0.93, 0.945, 1.0],
    search_bg: [1.0, 1.0, 1.0, 0.97],
};

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
