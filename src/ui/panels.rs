//! Side panel: status, flight strips, clearance controls and the comms log.

use super::camera::{PANEL_WIDTH, scope_width};
use super::input::UiState;
use super::strips;
use crate::sim::aircraft::{Aircraft, Flow, Landing, Phase};
use crate::sim::units::{format_heading, heading_diff};
use crate::sim::clearance::{Clearance, phrase};
use crate::sim::delivery;
use crate::sim::positions::Position;
use crate::sim::{Speaker, World};
use macroquad::prelude::*;
use super::gfx::{draw_line, draw_rectangle, draw_rectangle_lines, draw_text, measure_text, mouse_position, screen_height};

const BACKGROUND: Color = Color::new(0.07, 0.09, 0.10, 1.0);
const DIVIDER: Color = Color::new(0.22, 0.27, 0.28, 1.0);
const TEXT: Color = Color::new(0.85, 0.90, 0.88, 1.0);
const DIM: Color = Color::new(0.55, 0.62, 0.62, 1.0);
const BUTTON: Color = Color::new(0.16, 0.21, 0.23, 1.0);
const BUTTON_HOVER: Color = Color::new(0.24, 0.31, 0.34, 1.0);
const PILOT: Color = Color::new(0.55, 0.85, 1.00, 1.0);
const SYSTEM: Color = Color::new(0.95, 0.80, 0.45, 1.0);
const EMERGENCY: Color = Color::new(1.00, 0.35, 0.30, 1.0);
const UNABLE: Color = Color::new(1.00, 0.62, 0.20, 1.0);
/// Controllers on the landline.
const LANDLINE: Color = Color::new(0.80, 0.70, 1.00, 1.0);

const PADDING: f32 = 12.0;
const STATUS_HEIGHT: f32 = 92.0;
const CLEARANCE_HEIGHT: f32 = 212.0;
const LOG_HEIGHT: f32 = 230.0;
const LOG_LINE: f32 = 17.0;
/// Landline requests shown above the log, at most, and the height of each.
const LANDLINE_ROWS: usize = 3;
const LANDLINE_ROW: f32 = 26.0;
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

    // F3 gives the strip bay the whole panel.
    if ui.board.expanded {
        strips::draw(world, ui, x, STATUS_HEIGHT, height);
        return;
    }
    let log_top = height - LOG_HEIGHT;
    let clearance_top = log_top - CLEARANCE_HEIGHT;
    strips::draw(world, ui, x, STATUS_HEIGHT, clearance_top);
    divider(x, clearance_top);
    draw_clearance(world, ui, x, clearance_top);
    divider(x, log_top);
    let landline = draw_landline(world, ui, x, log_top);
    draw_log(world, x, log_top + landline);
}

/// Requests from other controllers waiting on the player, each with Approve
/// and Unable buttons, at the top of the log. Returns the height used.
fn draw_landline(world: &mut World, ui: &mut UiState, x: f32, top: f32) -> f32 {
    use crate::sim::coordination::{CoordKind, CoordState};
    let left = x + PADDING;
    let rows: Vec<(u32, String, bool)> = world
        .incoming()
        .take(LANDLINE_ROWS)
        .map(|c| {
            let callsign = world.find(c.aircraft).map(|a| a.callsign.clone()).unwrap_or_default();
            let what = match c.kind {
                CoordKind::Crossing { runway } => {
                    let map = world.airport.surface.as_ref().expect("crossings need a ground layout");
                    let name = crate::sim::clearance::spoken_runway(&world.airport, map, runway, &world.ends_in_use());
                    format!("{callsign} cross {name}?")
                }
                CoordKind::Release { end } => format!("release {callsign} off {}?", world.airport.runways[end].id),
                CoordKind::GoAround { end } => format!("go-around {callsign} off {}", world.airport.runways[end].id),
            };
            let pending = c.state == CoordState::Pending;
            let text = if pending { format!("{}: {what}", c.from.name()) } else { format!("{}: {what} (unable)", c.from.name()) };
            (c.aircraft, text, pending)
        })
        .collect();
    // Go-arounds the player still has to tell Approach about.
    let owed: Vec<(u32, String)> = world
        .coordination
        .iter()
        .filter(|c| c.state == CoordState::Owed)
        .filter_map(|c| {
            let callsign = &world.find(c.aircraft)?.callsign;
            let CoordKind::GoAround { end } = c.kind else { return None };
            Some((c.aircraft, format!("Call Approach: go-around {callsign} off {}", world.airport.runways[end].id)))
        })
        .collect();
    let releases = release_line(world);
    if rows.is_empty() && owed.is_empty() && releases.is_none() {
        return 0.0;
    }
    let bw = 86.0;
    let mut y = top + 6.0;
    if let Some(text) = releases {
        let staffing = world.staffing();
        let right = x + PANEL_WIDTH - PADDING;
        // Approach, with Tower automated, stops and restarts departures.
        if staffing.approach && !staffing.local {
            let on = world.call_for_release();
            let wide = bw * 1.4;
            if button(if on { "Release all F" } else { "Stop deps F" }, right - wide, y, wide, LANDLINE_ROW - 4.0)
                && !ui.replay
            {
                world.set_call_for_release(!on);
            }
        }
        label(&text, left, y + 17.0, 17.0, LANDLINE);
        y += LANDLINE_ROW;
    }
    for (id, text) in owed.into_iter().take(LANDLINE_ROWS) {
        let selected = ui.selected == Some(id);
        label(&text, left, y + 17.0, 17.0, if selected { TEXT } else { LANDLINE });
        let right = x + PANEL_WIDTH - PADDING;
        if button("Call G", right - bw, y, bw, LANDLINE_ROW - 4.0) && !ui.replay {
            world.call_go_around(id);
        }
        y += LANDLINE_ROW;
    }
    for (id, text, pending) in rows {
        let selected = ui.selected == Some(id);
        label(&text, left, y + 17.0, 17.0, if selected { TEXT } else { LANDLINE });
        let right = x + PANEL_WIDTH - PADDING;
        if button("Unable N", right - bw, y, bw, LANDLINE_ROW - 4.0) && pending && !ui.replay {
            world.answer_coordination(id, false);
        }
        if button("Approve Y", right - bw * 2.0 - 6.0, y, bw, LANDLINE_ROW - 4.0) && !ui.replay {
            world.answer_coordination(id, true);
        }
        y += LANDLINE_ROW;
    }
    divider(x, y + 2.0);
    y + 4.0 - top
}

/// Departure releases, for a player on Local or Approach but not both: on
/// call for release or not, and who's released.
fn release_line(world: &World) -> Option<String> {
    use crate::sim::coordination::{CoordKind, CoordState};
    let staffing = world.staffing();
    if staffing.local == staffing.approach {
        return None;
    }
    let Some(why) = &world.releases.call_for_release else {
        // Approach can stop departures; Local only hears when it does.
        return staffing.approach.then(|| "Departures released automatically".to_string());
    };
    let mut text = if why.is_empty() { "Call for release".to_string() } else { format!("Call for release, {why}") };
    let released: Vec<String> = world
        .coordination
        .iter()
        .filter(|c| matches!(c.kind, CoordKind::Release { .. }))
        .filter_map(|c| match c.state {
            CoordState::Approved { until } => {
                let callsign = &world.find(c.aircraft)?.callsign;
                Some(format!("{callsign} void {}", world.utc(until)))
            }
            _ => None,
        })
        .collect();
    if !released.is_empty() {
        text += &format!(": {}", released.join(", "));
    }
    Some(text)
}

fn draw_status(world: &World, ui: &UiState, x: f32) {
    let left = x + PADDING;
    let remaining = (world.shift_seconds() - world.time).max(0.0);
    let clock = format!("{:02}:{:02}", (remaining / 60.0) as i32, (remaining % 60.0) as i32);
    let rate = if ui.paused { "PAUSED".to_string() } else { format!("x{:.0}", ui.rate) };

    label(&world.airport.code, left, 24.0, 24.0, TEXT);
    // A lesson has no shift clock to beat.
    let time = if ui.lesson { "LESSON".to_string() } else { format!("{clock} left") };
    label(&format!("{time}   {rate}"), left + 80.0, 24.0, 22.0, TEXT);
    label(&format!("Score {}", world.score.points), left + 280.0, 24.0, 22.0, TEXT);

    let (arrival, departure, vfr) = world.runway_ids();
    label(
        &format!(
            "ATIS {}  Wind {:03.0}/{:.0}  Land {arrival}  Depart {departure}{}",
            world.weather.letter(),
            world.airport.to_magnetic(world.wind.from),
            world.wind.speed,
            vfr.map(|v| format!("  VFR {v}")).unwrap_or_default()
        ),
        left,
        48.0,
        18.0,
        DIM,
    );
    let mut parked = if world.staffing().surface_traffic() {
        format!("   Parked {}", world.score.parked)
    } else {
        String::new()
    };
    // With Delivery's count too, the line needs shorter words.
    let (off, go_arounds) = if world.staffing().delivery {
        parked += &format!("   Cleared {}", world.score.cleared);
        ("Out", "G/A")
    } else {
        ("Handed off", "Go-arounds")
    };
    label(
        &format!(
            "Landed {}{parked}   {off} {}   {go_arounds} {}",
            world.score.landed, world.score.exited, world.score.go_arounds
        ),
        left,
        66.0,
        18.0,
        DIM,
    );
    // An announced runway change, a closed runway, or poor weather takes the bottom line.
    match world.runway_change_line().or_else(|| world.closure_line()).or_else(|| world.ils_line()).or_else(|| world.sky_line()) {
        Some(change) => label(&change, left, 84.0, 18.0, SYSTEM),
        None => label(
            &format!(
                "Separation losses {}   Departures holding {}",
                world.score.separation_losses,
                world.departures_waiting()
            ),
            left,
            84.0,
            18.0,
            DIM,
        ),
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

    let at_delivery = ac.on_ground
        && ac.phase == Phase::Parked
        && ac.frequency() == Position::Delivery
        && ac.flight_plan.is_some();
    if at_delivery {
        draw_delivery(world, ui, &ac, left, top);
        return;
    }
    let tower = ac.on_ground
        || (world.staffing().tower() && matches!(ac.frequency(), Position::Local | Position::Ground));
    if tower {
        draw_tower_buttons(world, ui, left, top);
        return;
    }

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
    if button("Appr C", left + action_w + 6.0, y, action_w, bh) {
        ui.cycle_approach(world);
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

    draw_pending_words(world, ui, left, y + 44.0, 2);
}

/// Clearance Delivery: the flight plan as filed, and the clearance being built.
/// The course is a fact to check the altitude against, never a verdict.
fn draw_delivery(world: &mut World, ui: &mut UiState, ac: &Aircraft, left: f32, top: f32) {
    let Some(plan) = ac.flight_plan.as_ref() else { return };
    let airport = &world.airport;
    let dest = if plan.destination.is_empty() { "destination" } else { plan.destination.as_str() };
    let filed = format!(
        "IFR to {dest}   filed {}  {}",
        delivery::route_short(airport, &plan.route),
        delivery::altitude_text(plan.altitude)
    );
    label(&filed, left, top + 42.0, 17.0, TEXT);
    let status = match &plan.cleared {
        Some(c) => format!("Cleared {}  squawk {}", world.utc(c.time), delivery::code_text(c.squawk)),
        None => {
            let waited = plan.called_at.map_or(0.0, |t| world.time - t);
            format!("Waiting {}:{:02}", (waited / 60.0) as i32, (waited % 60.0) as i32)
        }
    };
    label(&status, left, top + 60.0, 16.0, DIM);
    if ui.course_hint {
        // The route being cleared, if amended: that's the course that counts.
        let route = ui
            .pending
            .tower
            .iter()
            .find_map(|c| match c {
                Clearance::Ifr { route, .. } => Some(route.clone()),
                _ => None,
            })
            .unwrap_or_else(|| plan.route.clone());
        let course = delivery::course(airport, &route);
        let (way, levels) = if delivery::eastbound(course) { ("eastbound", "odd") } else { ("westbound", "even") };
        let hint = format!("Course {:03.0} M, {way}: {levels}   (F4 hides)", course.round() % 360.0);
        label(&hint, left + 190.0, top + 60.0, 16.0, DIM);
    }

    let bh = 24.0;
    let w = (PANEL_WIDTH - PADDING * 2.0 - 18.0) / 4.0;
    let col = |i: f32| left + (w + 6.0) * i;
    let y = top + 70.0;
    if button("IFR I", col(0.0), y, w, bh) {
        ui.ifr(world);
    }
    if button("SID S", col(1.0), y, w, bh) {
        ui.cycle_sid(world);
    }
    if button("Alt Down", col(2.0), y, w, bh) {
        ui.amend_altitude(world, -1);
    }
    if button("Alt Up", col(3.0), y, w, bh) {
        ui.amend_altitude(world, 1);
    }
    let y = y + 28.0;
    let third = (PANEL_WIDTH - PADDING * 2.0 - 12.0) / 3.0;
    if button("Ground T", left, y, third, bh) {
        ui.handoff(world);
    }
    if button("Transmit Enter", left + third + 6.0, y, third, bh) {
        ui.transmit(world);
    }
    if button("Cancel Bksp", left + (third + 6.0) * 2.0, y, third, bh) {
        ui.cancel();
    }
    draw_pending_words(world, ui, left, y + 46.0, 5);
}

/// Runway and surface instructions, for an aircraft on a tower frequency.
fn draw_tower_buttons(world: &mut World, ui: &mut UiState, left: f32, top: f32) {
    let bh = 24.0;
    let w = (PANEL_WIDTH - PADDING * 2.0 - 18.0) / 4.0;
    let col = |i: f32| left + (w + 6.0) * i;
    let y = top + 36.0;
    let selected = ui.selected.and_then(|id| world.find(id));
    let in_pattern = selected.is_some_and(|a| a.vfr.is_some() && !a.on_ground);
    let tower_heading = selected.and_then(|a| tower_heading_shown(world, a, ui.pending.heading, ui.pending.go_around));
    if let Some(shown) = tower_heading {
        // An IFR departure with Tower, or a go-around: Tower's heading.
        label(&shown, col(0.0), y + 26.0, 17.0, TEXT);
        if button("Hdg -5", col(2.0), y + 8.0, w, bh) {
            ui.nudge_heading(world, -5.0);
        }
        if button("Hdg +5", col(3.0), y + 8.0, w, bh) {
            ui.nudge_heading(world, 5.0);
        }
    } else if in_pattern {
        // VFR in the air: pattern instructions where the taxi hints would be.
        if button("Enter/Ext E", col(0.0), y + 8.0, w, bh) {
            ui.enter_or_extend(world);
        }
        if button("Base B", col(1.0), y + 8.0, w, bh) {
            ui.turn_base(world);
        }
        if button("Option O", col(2.0), y + 8.0, w, bh) {
            ui.vfr_landing(world, Landing::Option);
        }
        if button("T&G U", col(3.0), y + 8.0, w, bh) {
            ui.vfr_landing(world, Landing::TouchAndGo);
        }
    } else {
        label("Click a hold line, gate or junction to taxi there.", left, y + 14.0, 16.0, DIM);
        let via = if ui.via.is_empty() { "Ctrl-click taxiways to route via them.".to_string() } else { format!("Via {}", ui.via.join(", ")) };
        label(&via, left, y + 32.0, 16.0, DIM);
    }

    let y = y + 44.0;
    if button("Hold H", col(0.0), y, w, bh) {
        ui.hold(world);
    }
    let cross = ui.cross_label(world);
    if button(cross, col(1.0), y, w, bh) {
        ui.cross(world);
    }
    if button("Line up W", col(2.0), y, w, bh) {
        ui.departure(world, false);
    }
    if button(ui.takeoff_label(world), col(3.0), y, w, bh) {
        ui.departure(world, true);
    }
    let y = y + 28.0;
    if button("Land L", col(0.0), y, w, bh) {
        ui.land(world);
    }
    if button(ui.go_around_label(world), col(1.0), y, w, bh) {
        ui.go_around(world);
    }
    if button("Contact T", col(2.0), y, w, bh) {
        ui.handoff(world);
    }
    if button("Clear via", col(3.0), y, w, bh) {
        ui.via.clear();
    }
    let y = y + 28.0;
    let wide = (PANEL_WIDTH - PADDING * 2.0 - 6.0) / 2.0;
    if button("Transmit Enter", left, y, wide, bh) {
        ui.transmit(world);
    }
    if button("Cancel Bksp", left + wide + 6.0, y, wide, bh) {
        ui.cancel();
    }
    draw_pending_words(world, ui, left, y + 44.0, 2);
}

/// For an IFR departure with Tower, or one going around: the heading it will
/// fly (or is about to be given), for the heading buttons. `None` for anything else.
fn tower_heading_shown(world: &World, ac: &Aircraft, pending: Option<f32>, go_around: bool) -> Option<String> {
    let heading = pending.or(ac.assigned_heading());
    if ac.vfr.is_some() || ac.frequency() != Position::Local {
        return None;
    }
    let runway = match ac.phase {
        Phase::LinedUp | Phase::Taxiing | Phase::TowerRelease if ac.flow == Flow::Departure => ac.departure_end?,
        Phase::GoAround => return Some(heading_words(world, heading, None)),
        // A go-around about to be sent: the heading to go with it.
        Phase::TowerHandedOff if go_around && !ac.on_ground => {
            let runway = ac.approach_runway.map(|r| world.airport.runways[r].heading);
            return Some(heading_words(world, pending, runway));
        }
        _ => return None,
    };
    let after = if ac.on_ground { " after takeoff" } else { "" };
    let heading = heading_words(world, heading, Some(world.airport.runways[runway].heading));
    Some(format!("{heading}{after}"))
}

fn heading_words(world: &World, heading: Option<f32>, runway: Option<f32>) -> String {
    match heading {
        Some(h) if runway.is_none_or(|r| heading_diff(r, h).abs() > 1.0) => {
            format!("Heading {}", format_heading(world.airport.to_magnetic(h)))
        }
        _ => "Runway heading".to_string(),
    }
}

/// What will be said if the player transmits now.
fn draw_pending_words(world: &World, ui: &UiState, left: f32, y: f32, lines: usize) {
    let pending = ui.pending.to_clearances();
    if pending.is_empty() {
        label("No clearance pending", left, y, 16.0, DIM);
    } else if let Some(ac) = ui.selected.and_then(|id| world.find(id)) {
        let words: Vec<String> =
            pending.iter().map(|c| phrase(c, ac, &world.airport, &world.ends_in_use())).collect();
        let text = words.join(", ");
        for (i, line) in wrap(&text, 52).iter().take(lines).enumerate() {
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
            // A pilot refusing an instruction stands out.
            Speaker::Pilot if entry.text.contains("unable") => UNABLE,
            Speaker::Pilot => PILOT,
            Speaker::System => SYSTEM,
            Speaker::Emergency => EMERGENCY,
            Speaker::Landline => LANDLINE,
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
