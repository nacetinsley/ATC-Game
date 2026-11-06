//! The radar scope, styled after an FAA STARS display: video map, position
//! symbols, history trails, timeshared data blocks and a system status area.
//! Targets come from radar returns only.

use super::camera::clip;
use super::input::{Drag, UiState};
use crate::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use crate::sim::airspace::Airport;
use crate::sim::positions::Position;
use crate::sim::units::{format_heading, heading_vec};
use crate::sim::{RadarTrack, World};
use macroquad::prelude::*;

pub const BLOCK_SIZE: Vec2 = vec2(112.0, 46.0);
const DEFAULT_BLOCK_OFFSET: Vec2 = vec2(18.0, -50.0);
const BLOCK_FONT: f32 = 18.0;
const BLOCK_LINE: f32 = 15.0;
/// Predicted track line length.
const PTL_SECONDS: f32 = 60.0;
/// Each half of a timeshared data block field shows for this long.
const TIMESHARE_SECONDS: f64 = 1.5;
const RING_SPACING_NM: f32 = 10.0;
const RINGS: usize = 7;
const CENTERLINE_NM: f32 = 25.0;
const GHOST_NM: f32 = 10.0;
const SSA_FONT: f32 = 18.0;
const SYMBOL_FONT: f32 = 24.0;
/// Radius around the position symbol that lines don't cross, pixels.
const SYMBOL_CLEAR_PX: f32 = 9.0;

const BACKGROUND: Color = Color::new(0.0, 0.0, 0.0, 1.0);
const RING: Color = Color::new(0.16, 0.16, 0.16, 1.0);
const MAP: Color = Color::new(0.42, 0.42, 0.42, 1.0);
const ROUTE: Color = Color::new(0.20, 0.20, 0.22, 1.0);
const FIX: Color = Color::new(0.36, 0.36, 0.38, 1.0);
const RUNWAY: Color = Color::new(0.75, 0.75, 0.75, 1.0);
const CENTERLINE: Color = Color::new(0.32, 0.32, 0.32, 1.0);
const MVA: Color = Color::new(0.33, 0.24, 0.12, 1.0);
/// STARS draws tracks you own in white and everyone else's in green.
const OWNED: Color = Color::new(0.94, 0.94, 0.94, 1.0);
const UNOWNED: Color = Color::new(0.10, 0.85, 0.10, 1.0);
const HISTORY: Color = Color::new(0.25, 0.40, 1.00, 1.0);
const SSA: Color = Color::new(0.10, 0.85, 0.10, 1.0);
const WARNING: Color = Color::new(1.00, 0.85, 0.10, 1.0);
const CRITICAL: Color = Color::new(1.00, 0.15, 0.15, 1.0);
const REFERENCE: Color = Color::new(0.45, 0.45, 0.55, 1.0);
const GHOST: Color = Color::new(1.00, 0.95, 0.40, 1.0);

// Strip colours in the side panel, kept distinct by flow and phase.
const STRIP_CONTROLLED: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const STRIP_DEPARTURE: Color = Color::new(0.45, 0.80, 1.00, 1.0);
const STRIP_APPROACH: Color = Color::new(0.30, 0.95, 0.90, 1.0);
const STRIP_OFFERED: Color = Color::new(1.00, 1.00, 1.00, 1.0);
const STRIP_OTHER: Color = Color::new(0.45, 0.52, 0.52, 1.0);

/// Strip colour for an aircraft's phase.
pub fn phase_color(ac: &Aircraft) -> Color {
    match ac.phase {
        Phase::Offered => STRIP_OFFERED,
        Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope => STRIP_APPROACH,
        Phase::TowerHandedOff
        | Phase::TowerRelease
        | Phase::Rollout
        | Phase::OfferedToCenter
        | Phase::Parked
        | Phase::Taxiing
        | Phase::LinedUp
        | Phase::Pattern => STRIP_OTHER,
        Phase::Controlled | Phase::GoAround => {
            if ac.flow == Flow::Departure { STRIP_DEPARTURE } else { STRIP_CONTROLLED }
        }
    }
}

fn flash() -> bool {
    (get_time() * 2.0) as i64 % 2 == 0
}

/// Which half of a timeshared field to show.
fn timeshare() -> bool {
    (get_time() / TIMESHARE_SECONDS) as i64 % 2 == 0
}

/// Who owns the track, as the letter STARS puts on the target.
fn position_symbol(ac: &Aircraft) -> char {
    match ac.frequency() {
        Position::Approach => 'A',
        Position::Local | Position::Ground => 'T',
        Position::Center => 'C',
    }
}

/// Alert shown above the data block, STARS style.
fn alert_tag(world: &World, id: u32) -> Option<(&'static str, Color)> {
    let has = |pairs: &std::collections::BTreeSet<(u32, u32)>| {
        pairs.iter().any(|&(a, b)| a == id || b == id)
    };
    if world.alerts.terrain.contains(&id) {
        Some(("LA", CRITICAL))
    } else if has(&world.alerts.separation) || has(&world.alerts.conflict) {
        Some(("CA", CRITICAL))
    } else if has(&world.alerts.wake) {
        Some(("WK", WARNING))
    } else {
        None
    }
}

fn track_color(world: &World, ac: &Aircraft) -> Color {
    if world.alerts.critical(ac.id) {
        return if flash() { CRITICAL } else { OWNED };
    }
    if world.alerts.warning(ac.id) {
        return WARNING;
    }
    if world.is_players(ac) { OWNED } else { UNOWNED }
}

/// Where a data block's top-left corner goes: its offset from the target,
/// kept inside the view so a block near the edge can still be read and clicked.
pub fn block_origin(ui: &UiState, id: u32, pos: glam::Vec2) -> Vec2 {
    let offset = ui.block_offsets.get(&id).copied().unwrap_or(DEFAULT_BLOCK_OFFSET);
    let origin = ui.camera.to_screen(pos) + offset;
    let view = ui.camera.viewport;
    let max = vec2(view.x + view.w - BLOCK_SIZE.x, view.y + view.h - BLOCK_SIZE.y);
    vec2(origin.x.clamp(view.x, max.x.max(view.x)), origin.y.clamp(view.y, max.y.max(view.y)))
}

/// The scope in its camera's viewport. An inset leaves out the status area.
pub fn draw(world: &World, ui: &UiState, inset: bool) {
    let view = ui.camera.viewport;
    clip(Some(view));
    draw_rectangle(view.x, view.y, view.w, view.h, BACKGROUND);
    draw_map(&world.airport, world, ui, inset);
    for track in &world.radar {
        if let Some(ac) = world.find(track.id) {
            draw_target(world, ui, ac, track);
        }
    }
    draw_pending(world, ui);
    if inset {
        draw_rectangle_lines(view.x, view.y, view.w, view.h, 2.0, MAP);
        let label = format!("RADAR  RNG {:.0}", ui.camera.range_nm());
        draw_text(&label, view.x + 8.0, view.y + 18.0, 16.0, SSA);
    } else {
        draw_status_area(world, ui);
    }
    clip(None);
}

fn draw_polygon_outline(ui: &UiState, points: &[[f32; 2]], thickness: f32, color: Color) {
    let screen: Vec<Vec2> =
        points.iter().map(|p| ui.camera.to_screen(glam::Vec2::new(p[0], p[1]))).collect();
    for i in 0..screen.len() {
        let (a, b) = (screen[i], screen[(i + 1) % screen.len()]);
        draw_line(a.x, a.y, b.x, b.y, thickness, color);
    }
}

fn draw_route(ui: &UiState, airport: &Airport, names: &[String]) {
    let points: Vec<Vec2> = airport
        .route_indices(names)
        .iter()
        .map(|&f| ui.camera.to_screen(airport.fixes[f].pos()))
        .collect();
    for pair in points.windows(2) {
        draw_line(pair[0].x, pair[0].y, pair[1].x, pair[1].y, 1.0, ROUTE);
    }
}

fn draw_map(airport: &Airport, world: &World, ui: &UiState, inset: bool) {
    let center = ui.camera.to_screen(glam::Vec2::ZERO);
    for ring in 1..=RINGS {
        let radius = ring as f32 * RING_SPACING_NM * ui.camera.scale;
        draw_circle_lines(center.x, center.y, radius, 1.0, RING);
    }

    if ui.show_mva {
        for area in &airport.mva {
            draw_polygon_outline(ui, &area.polygon, 1.0, MVA);
            let n = area.polygon.len() as f32;
            let (sx, sy) =
                area.polygon.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
            let label = ui.camera.to_screen(glam::Vec2::new(sx / n, sy / n));
            // Charted in hundreds of feet, as on the MVA video map.
            draw_text(format!("{:.0}", area.altitude / 100.0), label.x, label.y, 18.0, MVA);
        }
    }

    draw_polygon_outline(ui, &airport.sector, 1.5, MAP);

    let config = world.config().name.clone();
    for star in airport.stars.iter().filter(|s| s.config == config) {
        draw_route(ui, airport, &star.fixes);
    }
    for sid in airport.sids.iter().filter(|s| s.config == config) {
        draw_route(ui, airport, &sid.fixes);
    }

    for fix in &airport.fixes {
        let p = ui.camera.to_screen(fix.pos());
        draw_triangle_lines(
            vec2(p.x, p.y - 3.5),
            vec2(p.x - 3.5, p.y + 2.5),
            vec2(p.x + 3.5, p.y + 2.5),
            1.0,
            FIX,
        );
        draw_text(&fix.name, p.x + 6.0, p.y + 4.0, 14.0, FIX);
    }

    for reference in &airport.references {
        let p = ui.camera.to_screen(glam::Vec2::new(reference.x, reference.y));
        draw_line(p.x - 5.0, p.y, p.x + 5.0, p.y, 1.5, REFERENCE);
        draw_line(p.x, p.y - 5.0, p.x, p.y + 5.0, 1.5, REFERENCE);
        draw_text(&reference.name, p.x + 8.0, p.y - 6.0, 16.0, REFERENCE);
    }

    if let Some(source) = airport.data_source.as_ref().filter(|_| !inset) {
        let view = ui.camera.viewport;
        draw_text(source, view.x + 10.0, view.y + view.h - 10.0, 16.0, MAP);
    }

    for runway in &airport.runways {
        let a = ui.camera.to_screen(runway.takeoff_point());
        let b = ui.camera.to_screen(runway.far_end());
        draw_line(a.x, a.y, b.x, b.y, 2.5, RUNWAY);
    }

    // Extended centerline with a tick every mile and a longer one every five.
    let runway = &airport.runways[world.arrival_runway()];
    let outbound = -heading_vec(runway.heading);
    let across = glam::Vec2::new(outbound.y, -outbound.x);
    let a = ui.camera.to_screen(runway.threshold());
    let b = ui.camera.to_screen(runway.threshold() + outbound * CENTERLINE_NM);
    draw_dashed(a, b, CENTERLINE);
    for mile in 1..=CENTERLINE_NM as i32 {
        let half = if mile % 5 == 0 { 0.6 } else { 0.25 };
        let at = runway.threshold() + outbound * mile as f32;
        let p = ui.camera.to_screen(at - across * half);
        let q = ui.camera.to_screen(at + across * half);
        draw_line(p.x, p.y, q.x, q.y, 1.0, CENTERLINE);
    }
    let label = ui.camera.to_screen(runway.threshold() + outbound * (CENTERLINE_NM + 1.0));
    draw_text(&runway.id, label.x, label.y, 16.0, CENTERLINE);
}

fn draw_dashed(a: Vec2, b: Vec2, color: Color) {
    let length = a.distance(b);
    if length < 1.0 {
        return;
    }
    let dir = (b - a) / length;
    let mut t = 0.0;
    while t < length {
        let start = a + dir * t;
        let end = a + dir * (t + 8.0).min(length);
        draw_line(start.x, start.y, end.x, end.y, 1.0, color);
        t += 14.0;
    }
}

/// Controller's working data on the third line: assigned altitude and intent.
fn scratchpad(world: &World, ac: &Aircraft) -> String {
    let airport = &world.airport;
    if let Some(vfr) = &ac.vfr {
        return format!("VFR {}", crate::sim::vfr::leg_label(vfr));
    }
    let intent = match ac.phase {
        Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope => {
            format!("I{}", airport.runways[world.arrival_runway()].id)
        }
        Phase::GoAround => "GA".into(),
        Phase::Controlled => match ac.lateral {
            Lateral::Heading(h) => format!("H{}", format_heading(airport.to_magnetic(h))),
            _ => active_fix_name(airport, ac),
        },
        _ => active_fix_name(airport, ac),
    };
    format!("{:03.0} {intent}", ac.target_altitude / 100.0)
}

fn active_fix_name(airport: &Airport, ac: &Aircraft) -> String {
    ac.active_fix()
        .or(ac.exit_fix)
        .map(|f| airport.fixes[f].name.chars().take(5).collect())
        .unwrap_or_default()
}

fn draw_target(world: &World, ui: &UiState, ac: &Aircraft, track: &RadarTrack) {
    let color = track_color(world, ac);
    let p = ui.camera.to_screen(track.pos);

    // History trail: older returns in fading blue.
    for (age, old) in track.history.iter().enumerate() {
        let dot = ui.camera.to_screen(*old);
        let fade = 0.85 * (1.0 - age as f32 / (track.history.len() as f32 + 1.0));
        draw_circle(dot.x, dot.y, 2.0, Color::new(HISTORY.r, HISTORY.g, HISTORY.b, fade));
    }

    // Lines start clear of the position symbol so it stays readable.
    let ptl = ui.camera.to_screen(
        track.pos + track.velocity / crate::sim::units::SECONDS_PER_HOUR * PTL_SECONDS,
    );
    let ptl_start = p + (ptl - p).normalize_or_zero() * SYMBOL_CLEAR_PX;
    if ptl.distance(p) > SYMBOL_CLEAR_PX {
        let c = Color::new(color.r, color.g, color.b, 0.7);
        draw_line(ptl_start.x, ptl_start.y, ptl.x, ptl.y, 1.0, c);
    }

    let symbol = position_symbol(ac).to_string();
    let size = measure_text(&symbol, None, SYMBOL_FONT as u16, 1.0);
    draw_text(&symbol, p.x - size.width / 2.0, p.y + size.offset_y / 2.0, SYMBOL_FONT, color);

    let selected = ui.selected == Some(ac.id);
    let origin = block_origin(ui, ac.id, track.pos);
    let altitude = format!("{:03.0}", track.altitude / 100.0);
    let speed = format!("{:02.0}", (track.ground_speed / 10.0).round());

    // Tower's traffic drops to a limited data block: altitude and speed only.
    let emergency = ac.emergency.as_ref().filter(|e| e.declared()).map(|e| e.tag());
    if matches!(ac.phase, Phase::TowerHandedOff | Phase::Rollout) && !selected {
        draw_text(format!("{altitude} {speed}"), p.x + 10.0, p.y - 6.0, BLOCK_FONT, color);
        if let Some(tag) = emergency.filter(|_| ui.acknowledged.contains(&ac.id) || flash()) {
            draw_text(tag, p.x + 10.0, p.y - 6.0 - BLOCK_LINE, BLOCK_FONT, CRITICAL);
        }
        return;
    }

    // Leader line to the first line of the block, stopping at its edge.
    let anchor = vec2(
        p.x.clamp(origin.x, origin.x + BLOCK_SIZE.x),
        p.y.clamp(origin.y, origin.y + BLOCK_LINE),
    );
    let gap = (anchor - p).normalize_or_zero() * SYMBOL_CLEAR_PX;
    draw_line(p.x + gap.x, p.y + gap.y, anchor.x, anchor.y, 1.0, color);
    if selected {
        draw_rectangle_lines(origin.x - 2.0, origin.y - 2.0, BLOCK_SIZE.x, BLOCK_SIZE.y, 1.0, GHOST);
    }

    // An inbound handoff blinks until accepted.
    if ac.phase == Phase::Offered && !flash() {
        return;
    }

    let text = |line: &str, row: f32, c: Color| {
        draw_text(line, origin.x, origin.y + BLOCK_LINE * (row + 1.0) - 3.0, BLOCK_FONT, c);
    };
    if let Some((tag, tag_color)) = alert_tag(world, ac.id) {
        text(tag, -1.0, tag_color);
    }
    text(&ac.callsign, 0.0, color);
    let second = if timeshare() {
        format!("{altitude} {speed}")
    } else {
        format!("{altitude} {}", ac.ty.designator)
    };
    text(&second, 1.0, color);
    // An emergency code blinks in red until the aircraft is selected, then
    // timeshares with the scratchpad.
    let acknowledged = ui.acknowledged.contains(&ac.id);
    match emergency {
        Some(tag) if !acknowledged => {
            if flash() {
                text(tag, 2.0, CRITICAL);
            }
        }
        Some(tag) if timeshare() => text(tag, 2.0, CRITICAL),
        _ => text(&scratchpad(world, ac), 2.0, color),
    }
}

/// Preview of the unsent clearance, and of a drag in progress.
fn draw_pending(world: &World, ui: &UiState) {
    let Some(id) = ui.selected else { return };
    let Some(track) = world.track(id) else { return };
    let start = ui.camera.to_screen(track.pos);

    if let Some(fix) = ui.pending.direct {
        draw_dashed(start, ui.camera.to_screen(world.airport.fixes[fix].pos()), GHOST);
    } else if let Some(heading) = ui.pending.heading {
        let end = track.pos + heading_vec(heading) * GHOST_NM;
        draw_dashed(start, ui.camera.to_screen(end), GHOST);
    }

    if let Drag::Vector { id: drag_id, active: true, .. } = ui.drag
        && drag_id == id {
            let mouse = Vec2::from(mouse_position());
            let (end, label) = match ui.fix_under(world, mouse) {
                Some(fix) => (
                    ui.camera.to_screen(world.airport.fixes[fix].pos()),
                    format!("DIRECT {}", world.airport.fixes[fix].name),
                ),
                None => (
                    mouse,
                    ui.drag_heading(world, id, mouse)
                        .map(|h| format!("HDG {}", format_heading(world.airport.to_magnetic(h))))
                        .unwrap_or_default(),
                ),
            };
            draw_line(start.x, start.y, end.x, end.y, 1.5, GHOST);
            draw_text(&label, end.x + 10.0, end.y - 8.0, 20.0, GHOST);
        }
}

/// Top-left system status area: clock, airport and runways, range, and alert lists.
fn draw_status_area(world: &World, ui: &UiState) {
    let name = |id: u32| world.find(id).map(|a| a.callsign.as_str()).unwrap_or("?");
    let mut y = 22.0;
    let mut line = |text: &str, color: Color| {
        draw_text(text, 12.0, y, SSA_FONT, color);
        y += SSA_FONT;
    };

    let utc = (ui.clock_start + world.time as f64).rem_euclid(86_400.0) as u64;
    line(&format!("{:02}{:02}/{:02}", utc / 3600, utc / 60 % 60, utc % 60), SSA);
    let wind = &world.wind;
    let (arrival, departure, vfr) = world.runway_ids();
    line(
        &format!(
            "{} {} {:03.0}/{:02.0}  ARR {arrival} DEP {departure}{}",
            world.airport.code,
            world.weather.letter(),
            world.airport.to_magnetic(wind.from),
            wind.speed,
            vfr.map(|v| format!(" VFR {v}")).unwrap_or_default()
        ),
        SSA,
    );
    if let Some(change) = world.runway_change_line() {
        line(&change, WARNING);
    }
    if let Some(closed) = world.closure_line() {
        line(&closed, WARNING);
    }
    let range = ui.camera.range_nm();
    let paused = if ui.paused { "  PAUSED" } else { "" };
    line(&format!("RNG {range:.0}  PTL 1.0  x{:.0}{paused}", ui.rate), SSA);

    // STARS lists active alerts by callsign pair.
    for &(a, b) in world.alerts.separation.iter().chain(&world.alerts.conflict) {
        line(&format!("{}*{} CA", name(a), name(b)), CRITICAL);
    }
    for &id in &world.alerts.terrain {
        line(&format!("{} LA", name(id)), CRITICAL);
    }
    for &(a, b) in &world.alerts.wake {
        line(&format!("{}*{} WK", name(a), name(b)), WARNING);
    }
}
