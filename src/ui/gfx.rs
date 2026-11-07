//! UI scale and colour-blind palettes, applied under every drawing call.
//!
//! The UI is laid out in logical pixels. At a scale other than 100% a camera
//! maps them onto the window, and these wrappers stand in for macroquad's own
//! functions of the same names: screen size and mouse position come back in
//! logical pixels, text is rasterized at the scaled size so it stays sharp,
//! and every colour passes through the palette. Each UI module imports them
//! by name, which takes precedence over macroquad's prelude.

use macroquad::prelude as mq;
use mq::{Color, Font, TextDimensions};
use serde::{Deserialize, Serialize};
use std::cell::Cell;

/// The scales offered, smallest first.
pub const SCALES: [f32; 8] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
/// Above 100%, the UI keeps at least this much room in logical pixels, so the
/// layout still fits: a smaller window gets a smaller scale. The start menu,
/// with its two columns, needs more than the game does.
pub const GAME_ROOM: (f32, f32) = (1150.0, 680.0);
pub const MENU_ROOM: (f32, f32) = (1400.0, 780.0);

/// Colours for colour-blind players. Only the colours that carry meaning
/// move: greens, reds, ambers and blues; the greys and paper stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Palette {
    #[default]
    Standard,
    /// Deuteranopia and protanopia: greens become cyan, reds stay red.
    RedGreen,
    /// Tritanopia: ambers become pink, greens become cyan.
    BlueYellow,
}

impl Palette {
    pub const ALL: [Palette; 3] = [Palette::Standard, Palette::RedGreen, Palette::BlueYellow];

    pub fn label(self) -> &'static str {
        match self {
            Palette::Standard => "Standard",
            Palette::RedGreen => "Red-green",
            Palette::BlueYellow => "Blue-yellow",
        }
    }

    pub fn next(self) -> Palette {
        let i = Palette::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Palette::ALL[(i + 1) % Palette::ALL.len()]
    }

    pub fn previous(self) -> Palette {
        let i = Palette::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Palette::ALL[(i + Palette::ALL.len() - 1) % Palette::ALL.len()]
    }
}

thread_local! {
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    static PALETTE: Cell<Palette> = const { Cell::new(Palette::Standard) };
}

/// The scale the UI is drawn at this frame.
pub fn scale() -> f32 {
    SCALE.with(|s| s.get())
}

pub fn set_palette(palette: Palette) {
    PALETTE.with(|p| p.set(palette));
}

/// The scale to draw at for the chosen one: one of `SCALES`, no bigger than
/// the window leaves `room` for.
pub fn effective_scale(chosen: f32, room: (f32, f32)) -> f32 {
    if chosen <= 1.0 {
        return chosen;
    }
    let fits = (mq::screen_width() / room.0).min(mq::screen_height() / room.1);
    SCALES.iter().copied().filter(|&s| s <= chosen && (s <= fits || s <= 1.0)).fold(1.0, f32::max)
}

/// Set up drawing for a frame at the chosen scale. Returns true when the
/// scale has changed, before anything is drawn, so the font cache can be
/// warmed for it.
pub fn begin_frame(chosen: f32, room: (f32, f32)) -> bool {
    let scale = effective_scale(chosen, room);
    let changed = SCALE.with(|s| s.replace(scale)) != scale;
    if scale == 1.0 {
        mq::set_default_camera();
    } else {
        // Given bottom-up, so the camera puts y = 0 at the top, as on screen.
        let (w, h) = (mq::screen_width() / scale, mq::screen_height() / scale);
        let rect = mq::Rect::new(0.0, h, w, -h);
        mq::set_camera(&mq::Camera2D::from_display_rect(rect));
    }
    changed
}

/// Run `f` with the layout at `scale`, without drawing at it yet: to fit the
/// game's views from a menu frame drawn at another scale.
pub fn laid_out_at<T>(scale: f32, f: impl FnOnce() -> T) -> T {
    let old = SCALE.with(|s| s.replace(scale));
    let result = f();
    SCALE.with(|s| s.set(old));
    result
}

/// The size text of `size` is rasterized at, in points.
pub fn raster_size(size: f32) -> u16 {
    (size * scale()).round() as u16
}

pub fn screen_width() -> f32 {
    mq::screen_width() / scale()
}

pub fn screen_height() -> f32 {
    mq::screen_height() / scale()
}

pub fn mouse_position() -> (f32, f32) {
    let (x, y) = mq::mouse_position();
    (x / scale(), y / scale())
}

pub fn draw_text(text: impl AsRef<str>, x: f32, y: f32, font_size: f32, color: Color) -> TextDimensions {
    let scale = scale();
    let params = mq::TextParams {
        font_size: raster_size(font_size),
        font_scale: 1.0 / scale,
        color: adapt(color),
        ..Default::default()
    };
    mq::draw_text_ex(text, x, y, params)
}

pub fn measure_text(text: &str, font: Option<&Font>, font_size: u16, font_scale: f32) -> TextDimensions {
    mq::measure_text(text, font, raster_size(font_size as f32), font_scale / scale())
}

pub fn draw_line(x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Color) {
    mq::draw_line(x1, y1, x2, y2, thickness, adapt(color));
}

pub fn draw_rectangle(x: f32, y: f32, w: f32, h: f32, color: Color) {
    mq::draw_rectangle(x, y, w, h, adapt(color));
}

pub fn draw_rectangle_lines(x: f32, y: f32, w: f32, h: f32, thickness: f32, color: Color) {
    mq::draw_rectangle_lines(x, y, w, h, thickness, adapt(color));
}

pub fn draw_circle(x: f32, y: f32, r: f32, color: Color) {
    mq::draw_circle(x, y, r, adapt(color));
}

pub fn draw_circle_lines(x: f32, y: f32, r: f32, thickness: f32, color: Color) {
    mq::draw_circle_lines(x, y, r, thickness, adapt(color));
}

pub fn draw_triangle(v1: mq::Vec2, v2: mq::Vec2, v3: mq::Vec2, color: Color) {
    mq::draw_triangle(v1, v2, v3, adapt(color));
}

pub fn draw_triangle_lines(v1: mq::Vec2, v2: mq::Vec2, v3: mq::Vec2, thickness: f32, color: Color) {
    mq::draw_triangle_lines(v1, v2, v3, thickness, adapt(color));
}

pub fn clear_background(color: Color) {
    mq::clear_background(adapt(color));
}

/// A colour as the current palette shows it.
pub fn adapt(color: Color) -> Color {
    match PALETTE.with(|p| p.get()) {
        Palette::Standard => color,
        Palette::RedGreen => remap(color, |h| match h {
            // Greens: cyan, kept light, so they stay apart from the deep blues.
            75.0..165.0 => Some(190.0),
            _ => None,
        }),
        Palette::BlueYellow => remap(color, |h| match h {
            // Ambers and yellows: pink, which stays apart from red by lightness.
            35.0..72.0 => Some(325.0),
            75.0..165.0 => Some(180.0),
            _ => None,
        }),
    }
}

/// Move a saturated colour's hue where `to` says, keeping its saturation,
/// lightness and alpha. Greys and pale colours stay as they are.
fn remap(color: Color, to: impl Fn(f32) -> Option<f32>) -> Color {
    let (h, s, v) = hsv(color);
    if s < 0.3 || v < 0.15 {
        return color;
    }
    match to(h) {
        Some(h) => {
            let (r, g, b) = rgb(h, s, v);
            Color::new(r, g, b, color.a)
        }
        None => color,
    }
}

fn hsv(c: Color) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let delta = max - min;
    let h = if delta == 0.0 {
        0.0
    } else if max == c.r {
        60.0 * ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if max == c.g {
        60.0 * ((c.b - c.r) / delta + 2.0)
    } else {
        60.0 * ((c.r - c.g) / delta + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}

fn rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for c in [Color::new(0.1, 0.85, 0.1, 1.0), Color::new(1.0, 0.15, 0.15, 1.0), Color::new(0.25, 0.4, 1.0, 0.5)] {
            let (h, s, v) = hsv(c);
            let (r, g, b) = rgb(h, s, v);
            assert!((r - c.r).abs() < 1e-4 && (g - c.g).abs() < 1e-4 && (b - c.b).abs() < 1e-4, "{c:?}");
        }
    }

    #[test]
    fn palettes_move_only_the_colours_that_mean_something() {
        let green = Color::new(0.10, 0.85, 0.10, 1.0);
        let red = Color::new(1.00, 0.15, 0.15, 1.0);
        let amber = Color::new(1.00, 0.85, 0.10, 1.0);
        let grey = Color::new(0.42, 0.42, 0.42, 1.0);
        let paper = Color::new(0.87, 0.85, 0.77, 1.0);
        set_palette(Palette::RedGreen);
        assert!(adapt(green).b > 0.5 && adapt(green).g > 0.5, "{:?}", adapt(green));
        assert_eq!(adapt(red), red);
        assert_eq!(adapt(grey), grey);
        assert_eq!(adapt(paper), paper);
        set_palette(Palette::BlueYellow);
        let pink = adapt(amber);
        assert!(pink.r > 0.9 && pink.b > 0.3 && pink.g < 0.5, "{pink:?}");
        set_palette(Palette::Standard);
        assert_eq!(adapt(green), green);
    }
}
