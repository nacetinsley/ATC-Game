//! The lesson overlay: the instruction box and a ring around what it's about.

use super::camera::{inset_area, scope_area};
use super::input::UiState;
use crate::sim::World;
use crate::sim::tutorial::{Target, Tutorial};
use macroquad::prelude::*;

const BOX: Color = Color::new(0.02, 0.07, 0.05, 0.92);
const EDGE: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const TITLE: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const TEXT: Color = Color::new(0.92, 0.95, 0.92, 1.0);
const DIM: Color = Color::new(0.60, 0.68, 0.66, 1.0);
const RING: Color = Color::new(1.00, 0.85, 0.20, 1.0);
const FONT: f32 = 20.0;
const LINE: f32 = 24.0;
const PAD: f32 = 14.0;
const MAX_WIDTH: f32 = 720.0;

/// Break text into lines no wider than `width` pixels.
fn wrap(text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && measure_text(&candidate, None, FONT as u16, 1.0).width > width {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where on screen a target is, if it's in a view that's showing.
fn target_on_screen(target: Target, world: &World, ui: &UiState) -> Option<Vec2> {
    let (camera, pos) = match target {
        Target::Aircraft(id) => {
            let ac = world.find(id)?;
            if ac.on_ground {
                (&ui.surface, ac.pos)
            } else {
                // The radar draws where the last sweep saw it.
                (&ui.camera, world.track(id).map_or(ac.pos, |t| t.pos))
            }
        }
        Target::Node(node) => (&ui.surface, world.airport.surface.as_ref()?.nodes.get(node)?.pos),
        Target::Point(p) => (&ui.camera, p),
    };
    let surface_shown = ui.has_inset();
    if std::ptr::eq(camera, &ui.surface) && !surface_shown {
        return None;
    }
    let screen = camera.to_screen(pos);
    camera.viewport.contains(screen).then_some(screen)
}

pub fn draw(tutorial: &Tutorial, world: &World, ui: &UiState) {
    if let Some(at) = tutorial.highlight(world).and_then(|t| target_on_screen(t, world, ui)) {
        let pulse = (get_time() as f32 * 4.0).sin() * 0.5 + 0.5;
        draw_circle_lines(at.x, at.y, 18.0 + pulse * 8.0, 2.5, RING);
    }

    let area = scope_area();
    let right = if ui.has_inset() { inset_area().x - 12.0 } else { area.x + area.w - 12.0 };
    let width = (right - area.x - 24.0).clamp(240.0, MAX_WIDTH);
    let (title, body, footer) = match tutorial.current() {
        Some(step) => {
            let (n, of) = tutorial.progress();
            let footer = if step.read { "ENTER  next" } else { "S  show me    ESC  menu" };
            (format!("TUTORIAL  {}   step {n} of {of}", tutorial.kind.label()), step.text, footer)
        }
        None => (
            format!("TUTORIAL  {}", tutorial.kind.label()),
            "Lesson complete. Well done.",
            "ENTER  back to the menu",
        ),
    };
    let lines = wrap(body, width - PAD * 2.0);
    let height = PAD * 2.0 + LINE * (lines.len() as f32 + 2.0);
    let (x, y) = (area.x + 12.0, area.y + area.h - height - 12.0);
    draw_rectangle(x, y, width, height, BOX);
    draw_rectangle_lines(x, y, width, height, 1.5, EDGE);
    let mut line_y = y + PAD + 16.0;
    draw_text(&title, x + PAD, line_y, FONT, TITLE);
    for line in &lines {
        line_y += LINE;
        draw_text(line, x + PAD, line_y, FONT, TEXT);
    }
    draw_text(footer, x + PAD, line_y + LINE, 18.0, DIM);
}
