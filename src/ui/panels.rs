//! Side panel: status, flight strips, clearance controls and the comms log.

use super::camera::{PANEL_WIDTH, scope_width};
use super::input::UiState;
use super::scope::phase_color;
use crate::sim::aircraft::{Aircraft, Flow, Phase};
use crate::sim::clearance::phrase;
use crate::sim::{Speaker, World};
use macroquad::prelude::*;

const BACKGROUND: Color = Color::new(0.07, 0.09, 0.10, 1.0);
const DIVIDER: Color = Color::new(0.22, 0.27, 0.28, 1.0);
const TEXT: Color = Color::new(0.85, 0.90, 0.88, 1.0);
const DIM: Color = Color::new(0.55, 0.62, 0.62, 1.0);
const BUTTON: Color = Color::new(0.16, 0.21, 0.23, 1.0);
const BUTTON_HOVER: Color = Color::new(0.24, 0.31, 0.34, 1.0);
const SELECTED_ROW: Color = Color::new(0.20, 0.24, 0.14, 1.0);
const PILOT: Color = Color::new(0.55, 0.85, 1.00, 1.0);
const SYSTEM: Color = Color::new(0.95, 0.80, 0.45, 1.0);

const PADDING: f32 = 12.0;
const STATUS_HEIGHT: f32 = 92.0;
const CLEARANCE_HEIGHT: f32 = 212.0;
const LOG_HEIGHT: f32 = 230.0;
const ROW_HEIGHT: f32 = 22.0;
const LOG_LINE: f32 = 17.0;
const LOG_FONT: f32 = 16.0;
/// Approximate width of one character at `LOG_FONT`, for wrapping.
const LOG_CHAR_WIDTH: f32 = 7.2;

fn divider(x: f32, y: f32) {
    draw_line(x, y, x + PANEL_WIDTH, y, 1.0, DIVIDER);
}

fn label(text: &str, x: f32, y: f32, size: f32, color: Color) {
    draw_text(text, x, y, size, color);
}

/// Immediate-mode button. Returns true on the frame it's clicked.
fn button(text: &str, x: f32, y: f32, w: f32, h: f32) -> bool {
    let rect = Rect::new(x, y, w, h);
    let hover = rect.contains(Vec2::from(mouse_position()));
    draw_rectangle(x, y, w, h, if hover { BUTTON_HOVER } else { BUTTON });
    draw_rectangle_lines(x, y, w, h, 1.0, DIVIDER);
    let size = measure_text(text, None, 18, 1.0);
    draw_text(text, x + (w - size.width) / 2.0, y + h / 2.0 + 5.0, 18.0, TEXT);
    hover && is_mouse_button_pressed(MouseButton::Left)
}

pub fn draw(world: &mut World, ui: &mut UiState) {
    let x = scope_width();
    let height = screen_height();
    draw_rectangle(x, 0.0, PANEL_WIDTH, height, BACKGROUND);
    draw_line(x, 0.0, x, height, 2.0, DIVIDER);

    draw_status(world, ui, x);
    divider(x, STATUS_HEIGHT);

    let log_top = height - LOG_HEIGHT;
    let clearance_top = log_top - CLEARANCE_HEIGHT;
    draw_strips(world, ui, x, STATUS_HEIGHT, clearance_top);
    divider(x, clearance_top);
    draw_clearance(world, ui, x, clearance_top);
    divider(x, log_top);
    draw_log(world, x, log_top);
}

fn draw_status(world: &World, ui: &UiState, x: f32) {
    let left = x + PADDING;
    let remaining = (world.shift_seconds() - world.time).max(0.0);
    let clock = format!("{:02}:{:02}", (remaining / 60.0) as i32, (remaining % 60.0) as i32);
    let rate = if ui.paused { "PAUSED".to_string() } else { format!("x{:.0}", ui.rate) };

    label(&world.airport.code, left, 24.0, 24.0, TEXT);
    label(&format!("{clock} left   {rate}"), left + 80.0, 24.0, 22.0, TEXT);
    label(&format!("Score {}", world.score.points), left + 280.0, 24.0, 22.0, TEXT);

    let config = world.config();
    label(
        &format!(
            "Wind {:03.0}/{:.0}   Land {}   Depart {}",
            world.airport.to_magnetic(world.wind.from),
            world.wind.speed,
            config.arrival,
            config.departure
        ),
        left,
        48.0,
        18.0,
        DIM,
    );
    label(
        &format!(
            "Landed {}   Handed off {}   Go-arounds {}",
            world.score.landed, world.score.exited, world.score.go_arounds
        ),
        left,
        66.0,
        18.0,
        DIM,
    );
    label(
        &format!(
            "Separation losses {}   Departures holding {}",
            world.score.separation_losses,
            world.departures_waiting()
        ),
        left,
        84.0,
        18.0,
        DIM,
    );
}

fn strip_text(world: &World, ac: &Aircraft) -> String {
    let flow = if ac.flow == Flow::Arrival { "ARR" } else { "DEP" };
    let phase = match ac.phase {
        Phase::Offered => "offered",
        Phase::Controlled => "radar",
        Phase::ClearedApproach => "cleared",
        Phase::Localizer => "loc",
        Phase::Glideslope => "gs",
        Phase::TowerHandedOff | Phase::Rollout | Phase::TowerRelease => "tower",
        Phase::GoAround => "go-around",
        Phase::OfferedToCenter => "center",
    };
    let exit = match (ac.flow, ac.exit_fix) {
        (Flow::Departure, Some(fix)) => {
            format!(" {} {:.0}", world.airport.fixes[fix].name, ac.exit_altitude / 100.0)
        }
        _ => String::new(),
    };
    format!(
        "{flow} {:<7}{}/{} {:03.0}>{:03.0} {phase}{exit}",
        ac.callsign,
        ac.ty.designator,
        ac.ty.wake.letter(),
        ac.altitude / 100.0,
        ac.target_altitude / 100.0,
    )
}

fn draw_strips(world: &World, ui: &mut UiState, x: f32, top: f32, bottom: f32) {
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut y = top + 4.0;
    let mut hidden = 0;

    let rows = world
        .aircraft
        .iter()
        .filter(|a| a.flow == Flow::Arrival)
        .chain(world.aircraft.iter().filter(|a| a.flow == Flow::Departure));
    for ac in rows {
        if y + ROW_HEIGHT > bottom {
            hidden += 1;
            continue;
        }
        let rect = Rect::new(x, y, PANEL_WIDTH, ROW_HEIGHT);
        if ui.selected == Some(ac.id) {
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, SELECTED_ROW);
        }
        if clicked && rect.contains(mouse) {
            ui.select(Some(ac.id));
        }
        label(&strip_text(world, ac), x + PADDING, y + 16.0, 17.0, phase_color(ac));
        y += ROW_HEIGHT;
    }
    if hidden > 0 {
        label(&format!("+{hidden} more"), x + PANEL_WIDTH - 80.0, bottom - 6.0, 16.0, DIM);
    }
}

fn draw_clearance(world: &mut World, ui: &mut UiState, x: f32, top: f32) {
    let left = x + PADDING;
    let Some(ac) = ui.selected.and_then(|id| world.find(id)).cloned() else {
        label("Select an aircraft on the scope or strips", left, top + 24.0, 18.0, DIM);
        return;
    };

    label(
        &format!(
            "{}  {} {}  IAS {:.0}",
            ac.callsign,
            ac.ty.designator,
            ac.ty.wake.letter(),
            ac.ias
        ),
        left,
        top + 22.0,
        20.0,
        TEXT,
    );

    // Adjustment rows: heading, altitude, speed.
    let row = |name: &str, y: f32| label(name, left, y + 18.0, 18.0, DIM);
    let (bx, bw, bh) = (left + 60.0, 58.0, 24.0);
    let gap = bw + 6.0;

    let y = top + 32.0;
    row("HDG", y);
    if button("-5", bx, y, bw, bh) {
        ui.nudge_heading(world, -5.0);
    }
    if button("+5", bx + gap, y, bw, bh) {
        ui.nudge_heading(world, 5.0);
    }
    let y = y + 28.0;
    row("ALT", y);
    if button("-1000", bx, y, bw, bh) {
        ui.nudge_altitude(world, -1000.0);
    }
    if button("+1000", bx + gap, y, bw, bh) {
        ui.nudge_altitude(world, 1000.0);
    }
    let y = y + 28.0;
    row("SPD", y);
    if button("-10", bx, y, bw, bh) {
        ui.nudge_speed(world, -10.0);
    }
    if button("+10", bx + gap, y, bw, bh) {
        ui.nudge_speed(world, 10.0);
    }

    // Action rows.
    let y = y + 32.0;
    let action_w = (PANEL_WIDTH - PADDING * 2.0 - 18.0) / 4.0;
    let handoff = if ac.phase == Phase::Offered { "Accept T" } else { "Handoff T" };
    if button(handoff, left, y, action_w, bh) {
        ui.handoff(world);
    }
    if button("ILS C", left + action_w + 6.0, y, action_w, bh) {
        ui.clear_for_ils(world);
    }
    if button("Route R", left + (action_w + 6.0) * 2.0, y, action_w, bh) {
        ui.pending.set_resume();
    }
    if button("G/A G", left + (action_w + 6.0) * 3.0, y, action_w, bh) {
        ui.pending.go_around = true;
    }
    let y = y + 28.0;
    let wide = (PANEL_WIDTH - PADDING * 2.0 - 6.0) / 2.0;
    if button("Transmit Enter", left, y, wide, bh) {
        ui.transmit(world);
    }
    if button("Cancel Bksp", left + wide + 6.0, y, wide, bh) {
        ui.cancel();
    }

    // What will be said if the player transmits now.
    let y = y + 44.0;
    let pending = ui.pending.to_clearances();
    if pending.is_empty() {
        label("No clearance pending", left, y, 16.0, DIM);
    } else if let Some(ac) = ui.selected.and_then(|id| world.find(id)) {
        let words: Vec<String> =
            pending.iter().map(|c| phrase(c, ac, &world.airport)).collect();
        let text = words.join(", ");
        for (i, line) in wrap(&text, 52).iter().take(2).enumerate() {
            label(line, left, y - 6.0 + i as f32 * 15.0, 16.0, TEXT);
        }
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + word.len() + 1 > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn draw_log(world: &World, x: f32, top: f32) {
    let width = ((PANEL_WIDTH - PADDING * 2.0) / LOG_CHAR_WIDTH) as usize;
    let mut y = screen_height() - 8.0;
    // Newest at the bottom, working up until the panel is full.
    'entries: for entry in world.log.iter().rev() {
        let color = match entry.speaker {
            Speaker::Controller => TEXT,
            Speaker::Pilot => PILOT,
            Speaker::System => SYSTEM,
        };
        for line in wrap(&entry.text, width).iter().rev() {
            if y < top + LOG_LINE {
                break 'entries;
            }
            draw_text(line, x + PADDING, y, LOG_FONT, color);
            y -= LOG_LINE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wrap_keeps_words_whole_and_lines_short() {
        let lines = wrap("turn left heading 270, descend and maintain 3,000", 20);
        assert!(lines.iter().all(|l| l.len() <= 20), "{lines:?}");
        assert_eq!(lines.join(" "), "turn left heading 270, descend and maintain 3,000");
    }

    #[test]
    fn wrap_of_nothing_is_nothing() {
        assert!(wrap("", 10).is_empty());
    }
}
