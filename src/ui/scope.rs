//! The radar scope, styled after an FAA STARS display: video map, position
//! symbols, history trails, timeshared data blocks and a system status area.
//! Targets come from radar returns only.

use super::camera::clip;
use super::input::{Drag, UiState, ctrl_down};
use crate::sim::mistakes::SlipKind;
use crate::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use crate::sim::conflict::{Atpa, AtpaStatus};
use crate::sim::performance::WakeCategory;
use crate::sim::airspace::Airport;
use crate::sim::approach::ApproachKind;
use crate::sim::holding::{self, HoldSpec};
use crate::sim::positions::Position;
use crate::sim::units::{format_heading, heading_vec, tas_from_ias};
use crate::sim::{RadarTrack, World};
use macroquad::prelude::*;
use super::gfx::{draw_circle, draw_circle_lines, draw_line, draw_rectangle, draw_rectangle_lines, draw_text, draw_triangle_lines, measure_text, mouse_position};

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
/// STARS draws tracks you own in white and everyone else's in green, and the
/// data block you've highlighted in cyan.
const OWNED: Color = Color::new(0.94, 0.94, 0.94, 1.0);
const UNOWNED: Color = Color::new(0.10, 0.85, 0.10, 1.0);
const SELECTED: Color = Color::new(0.0, 1.0, 1.0, 1.0);
const SELECTED_BOX: Color = Color::new(0.0, 0.6, 0.6, 1.0);
/// History trails, newest first: STARS's five blues.
const HISTORY: [Color; 5] = [
    Color::new(30.0 / 255.0, 80.0 / 255.0, 200.0 / 255.0, 1.0),
    Color::new(70.0 / 255.0, 70.0 / 255.0, 170.0 / 255.0, 1.0),
    Color::new(50.0 / 255.0, 50.0 / 255.0, 130.0 / 255.0, 1.0),
    Color::new(40.0 / 255.0, 40.0 / 255.0, 110.0 / 255.0, 1.0),
    Color::new(30.0 / 255.0, 30.0 / 255.0, 90.0 / 255.0, 1.0),
];
const SSA: Color = Color::new(0.10, 0.85, 0.10, 1.0);
/// The caution colour: yellow.
const WARNING: Color = Color::new(1.00, 0.85, 0.10, 1.0);
/// The alert colour: red.
const CRITICAL: Color = Color::new(1.00, 0.15, 0.15, 1.0);
/// ATPA: the monitor cone in the TPA blue, the alert in orange.
const ATPA_MONITOR: Color = Color::new(30.0 / 255.0, 120.0 / 255.0, 1.0, 0.8);
const ATPA_ALERT: Color = Color::new(1.0, 55.0 / 255.0, 0.0, 1.0);
/// An ATPA cone's half-width at its far end, as a share of its length.
const ATPA_CONE_WIDTH: f32 = 0.1;
/// The assigned altitude shows on line 3 while Mode C is this far from it, feet.
const ASSIGNED_SHOWN_FT: f32 = 150.0;
const REFERENCE: Color = Color::new(0.45, 0.45, 0.55, 1.0);
const GHOST: Color = Color::new(1.00, 0.95, 0.40, 1.0);
/// Published holds on the map, and brighter while someone is in one.
const HOLD: Color = Color::new(0.22, 0.22, 0.26, 1.0);
const HOLD_BUSY: Color = Color::new(0.40, 0.40, 0.55, 1.0);
/// Weather radar returns as STARS shows the six NEXRAD levels: levels 1 to 3
/// in dark teal, 4 to 6 in dark olive, with a light stipple on levels 2 and 5
/// and a dense one on 3 and 6.
const WX_LOW: Color = Color::new(38.0 / 255.0, 77.0 / 255.0, 77.0 / 255.0, 1.0);
const WX_HIGH: Color = Color::new(100.0 / 255.0, 100.0 / 255.0, 51.0 / 255.0, 1.0);
const WX_STIPPLE: Color = Color::new(1.0, 1.0, 1.0, 0.55);
/// Weather is drawn on a grid this fine, NM.
const WX_GRID_NM: f32 = 1.0;
/// Published holds without a minimum altitude are drawn as if flown here, feet.
const HOLD_DRAW_ALTITUDE: f32 = 6000.0;
/// The scratchpad shows EFC from this long before it, seconds.
const EFC_WARNING_S: f32 = 120.0;

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
        Position::Local | Position::Ground | Position::Delivery | Position::Ramp => 'T',
        Position::Center => 'C',
    }
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
    let atpa = world.atpa();
    draw_atpa_cones(world, ui, &atpa);
    for track in &world.radar {
        if let Some(ac) = world.find(track.id) {
            let trailing = atpa.iter().find(|a| a.follower == ac.id);
            draw_target(world, ui, ac, track, trailing);
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

/// Thunderstorm cells, under everything else on the map.
fn draw_weather(world: &World, ui: &UiState) {
    let mut squares = std::collections::BTreeSet::new();
    for cell in &world.storms.cells {
        let lo = ((cell.pos - glam::Vec2::splat(cell.radius)) / WX_GRID_NM).floor();
        let hi = ((cell.pos + glam::Vec2::splat(cell.radius)) / WX_GRID_NM).ceil();
        for x in lo.x as i32..hi.x as i32 {
            for y in lo.y as i32..hi.y as i32 {
                squares.insert((x, y));
            }
        }
    }
    let size = WX_GRID_NM * ui.camera.scale;
    for (x, y) in squares {
        let centre = glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * WX_GRID_NM;
        let level = world.storms.level_at(centre).floor().min(6.0) as i32;
        if level < 1 {
            continue;
        }
        let colour = if level >= 4 { WX_HIGH } else { WX_LOW };
        let p = ui.camera.to_screen(centre);
        draw_rectangle(p.x - size / 2.0, p.y - size / 2.0, size + 0.5, size + 0.5, colour);
        // Light stipple: a dot per square; dense: four.
        let dots: &[(f32, f32)] = match level % 3 {
            2 => &[(0.0, 0.0)],
            0 => &[(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)],
            _ => &[],
        };
        if size >= 6.0 {
            for (dx, dy) in dots {
                draw_rectangle(p.x + dx * size - 0.75, p.y + dy * size - 0.75, 1.5, 1.5, WX_STIPPLE);
            }
        }
    }
}

fn draw_map(airport: &Airport, world: &World, ui: &UiState, inset: bool) {
    draw_weather(world, ui);
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

    for hold in &airport.holds {
        let Some(fix) = airport.fix_index(&hold.fix) else { continue };
        let spec = HoldSpec::published(hold, fix, 0.0);
        let altitude = hold.altitude.unwrap_or(HOLD_DRAW_ALTITUDE);
        let busy = world
            .aircraft
            .iter()
            .any(|a| matches!(a.lateral, Lateral::Hold(h) if h.spec.fix == fix));
        let tas = tas_from_ias(holding::max_speed(altitude), altitude);
        draw_racetrack(ui, airport, &spec, tas, altitude, if busy { HOLD_BUSY } else { HOLD });
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

    // Each landing runway's extended centerline. One with a working ILS gets
    // a tick every mile and a longer one every five; RNAV and visual only,
    // just the dashes.
    for end in world.landing_runways() {
        let runway = &airport.runways[end];
        let outbound = -heading_vec(runway.heading);
        let across = glam::Vec2::new(outbound.y, -outbound.x);
        let a = ui.camera.to_screen(runway.threshold());
        let b = ui.camera.to_screen(runway.threshold() + outbound * CENTERLINE_NM);
        draw_dashed(a, b, CENTERLINE);
        if world.offers(end, ApproachKind::Ils) {
            for mile in 1..=CENTERLINE_NM as i32 {
                let half = if mile % 5 == 0 { 0.6 } else { 0.25 };
                let at = runway.threshold() + outbound * mile as f32;
                let p = ui.camera.to_screen(at - across * half);
                let q = ui.camera.to_screen(at + across * half);
                draw_line(p.x, p.y, q.x, q.y, 1.0, CENTERLINE);
            }
        }
        let label = ui.camera.to_screen(runway.threshold() + outbound * (CENTERLINE_NM + 1.0));
        draw_text(&runway.id, label.x, label.y, 16.0, CENTERLINE);
    }
}

/// The RNAV approach an aircraft is cleared for, or about to be: the fixes
/// still to fly, then the runway.
fn draw_rnav_path(world: &World, ui: &UiState, ac: &Aircraft, start: Vec2) {
    let airport = &world.airport;
    let (end, fixes) = match ui.pending.approach {
        Some((ApproachKind::Rnav, end)) => {
            let fix = ui.pending.direct.or_else(|| ui.pending.heading.is_none().then(|| ac.active_fix()).flatten());
            match fix.and_then(|f| airport.rnav_route_from(end, f)) {
                Some(route) => (end, route),
                None => (end, Vec::new()),
            }
        }
        None if ac.approach == ApproachKind::Rnav && ac.phase == Phase::ClearedApproach => {
            let Some(end) = ac.approach_runway else { return };
            let rest = if ac.lateral == Lateral::Route { ac.route[ac.route_index.min(ac.route.len())..].to_vec() } else { Vec::new() };
            (end, rest)
        }
        _ => return,
    };
    // On vectors there's no path to show: the final course is drawn already.
    if fixes.is_empty() {
        return;
    }
    let mut points = vec![start];
    points.extend(fixes.iter().map(|&f| ui.camera.to_screen(airport.fixes[f].pos())));
    points.push(ui.camera.to_screen(airport.runways[end].threshold()));
    for pair in points.windows(2) {
        draw_dashed(pair[0], pair[1], GHOST);
    }
}

/// A holding pattern's racetrack, for an aircraft at `tas` and `altitude`.
fn draw_racetrack(ui: &UiState, airport: &Airport, spec: &HoldSpec, tas: f32, altitude: f32, color: Color) {
    let points: Vec<Vec2> = spec
        .outline(airport.fixes[spec.fix].pos(), tas, altitude)
        .into_iter()
        .map(|p| ui.camera.to_screen(p))
        .collect();
    for pair in points.windows(2) {
        draw_line(pair[0].x, pair[0].y, pair[1].x, pair[1].y, 1.0, color);
    }
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

/// The scratchpad, filled in as a controller would: up to four characters
/// that time-share with the altitude on the data block's second line. Amber
/// when it shows something wrong: a misheard heading, or a request for weather.
fn scratchpad(world: &World, ac: &Aircraft) -> Option<(String, bool)> {
    let airport = &world.airport;
    if ac.deviation.is_some_and(|d| !d.approved) {
        return Some(("WX".into(), true));
    }
    if let Some(vfr) = &ac.vfr {
        let leg = match crate::sim::vfr::leg_label(vfr) {
            "INBND" => "INB",
            "ENTRY" => "ENT",
            "UPWND" => "UPW",
            "XWIND" => "XWD",
            "DNWND" => "DNW",
            "FINAL" => "FNL",
            other => other,
        };
        return Some((leg.into(), false));
    }
    if let Lateral::Hold(state) = ac.lateral {
        let tag: String = if world.time >= state.spec.efc - EFC_WARNING_S {
            "EFC".into()
        } else {
            airport.fixes[state.spec.fix].name.chars().take(3).collect()
        };
        return Some((tag, false));
    }
    let misheard = matches!(ac.missed_hearback(), Some(SlipKind::Heading { .. }));
    match ac.phase {
        Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff
            if ac.approach_runway.is_some() || ac.phase != Phase::TowerHandedOff =>
        {
            let id = &airport.runways[ac.approach_runway.unwrap_or(world.arrival_runway())].id;
            Some((format!("{}{id}", ac.approach.letter()), false))
        }
        Phase::GoAround => Some(("GA".into(), false)),
        Phase::Controlled | Phase::OfferedToCenter => match ac.assigned_heading() {
            Some(h) => Some((format!("H{}", format_heading(airport.to_magnetic(h))), misheard)),
            None => fix_abbreviation(airport, ac).map(|f| (f, false)),
        },
        _ => fix_abbreviation(airport, ac).map(|f| (f, false)),
    }
}

/// The fix it's flying to, in three letters, as scratchpads abbreviate them.
fn fix_abbreviation(airport: &Airport, ac: &Aircraft) -> Option<String> {
    ac.active_fix().or(ac.exit_fix).map(|f| airport.fixes[f].name.chars().take(3).collect())
}

/// The character after the ground speed: V for VFR, otherwise the wake
/// category (H heavy, F Boeing 757, blank for the rest). HL while holding.
fn category(ac: &Aircraft) -> &'static str {
    if ac.vfr.is_some() {
        "V"
    } else if matches!(ac.lateral, Lateral::Hold(_)) {
        "HL"
    } else if ac.ty.designator.starts_with("B75") {
        "F"
    } else if ac.ty.wake == WakeCategory::Heavy {
        "H"
    } else {
        ""
    }
}

/// The single character between altitude and speed: who a handoff is to.
fn handoff_char(world: &World, ac: &Aircraft) -> char {
    match ac.phase {
        // Inbound: the receiving position, which is the player's.
        Phase::Offered => player_symbol(world),
        Phase::OfferedToCenter => 'C',
        _ => ' ',
    }
}

/// The player's own position symbol.
fn player_symbol(world: &World) -> char {
    if world.staffing().approach { 'A' } else { 'T' }
}

/// The alerts on a data block's top line: MSAW (LA), conflict alert (CA) and
/// the special purpose code, each with whether it's still unacknowledged.
fn alerts(world: &World, ui: &UiState, ac: &Aircraft) -> Vec<(&'static str, bool)> {
    let mut alerts = Vec::new();
    if world.alerts.terrain.contains(&ac.id) {
        alerts.push(("LA", !ui.acknowledged_low.contains(&ac.id)));
    }
    let pairs: Vec<&(u32, u32)> = world
        .alerts
        .separation
        .iter()
        .chain(&world.alerts.conflict)
        .filter(|&&(a, b)| a == ac.id || b == ac.id)
        .collect();
    if !pairs.is_empty() {
        alerts.push(("CA", pairs.iter().any(|p| !ui.acknowledged_pairs.contains(p))));
    }
    if let Some(tag) = ac.emergency.as_ref().filter(|e| e.declared()).map(|e| e.tag()) {
        alerts.push((tag, !ui.acknowledged.contains(&ac.id)));
    }
    alerts
}

/// Which data block a track gets.
#[derive(PartialEq)]
enum Block {
    Full,
    /// A track another position owns: altitude and speed on one line.
    Partial,
}

fn block_kind(world: &World, ui: &UiState, ac: &Aircraft, alerting: bool) -> Block {
    let full = world.is_seats(ui.seat, ac)
        || ac.phase == Phase::Offered
        || ui.selected == Some(ac.id)
        || alerting
        // A tower cab quick-looks the radar positions: their tracks show in full.
        || !ui.staffing.approach;
    if full { Block::Full } else { Block::Partial }
}

fn draw_target(world: &World, ui: &UiState, ac: &Aircraft, track: &RadarTrack, atpa: Option<&Atpa>) {
    let selected = ui.selected == Some(ac.id);
    let owned = world.is_seats(ui.seat, ac);
    let color = if selected {
        SELECTED
    } else if owned {
        OWNED
    } else {
        UNOWNED
    };
    let p = ui.camera.to_screen(track.pos);

    // History: the last returns, in darkening blues.
    for (age, old) in track.history.iter().enumerate() {
        let dot = ui.camera.to_screen(*old);
        draw_circle(dot.x, dot.y, 2.5, HISTORY[age.min(HISTORY.len() - 1)]);
    }

    // Predicted track line, for your own tracks (PTL OWN) and the one selected.
    if owned || selected {
        let ptl = ui.camera.to_screen(
            track.pos + track.velocity / crate::sim::units::SECONDS_PER_HOUR * PTL_SECONDS,
        );
        // Lines start clear of the position symbol so it stays readable.
        let ptl_start = p + (ptl - p).normalize_or_zero() * SYMBOL_CLEAR_PX;
        if ptl.distance(p) > SYMBOL_CLEAR_PX {
            draw_line(ptl_start.x, ptl_start.y, ptl.x, ptl.y, 1.0, color);
        }
    }

    let symbol = position_symbol(ac).to_string();
    let size = measure_text(&symbol, None, SYMBOL_FONT as u16, 1.0);
    draw_text(&symbol, p.x - size.width / 2.0, p.y + size.offset_y / 2.0, SYMBOL_FONT, color);
    // The mouse is over its strip: point it out.
    if ui.hovered == Some(ac.id) {
        draw_circle_lines(p.x, p.y, 14.0, 1.5, Color::new(0.98, 0.95, 0.40, 0.9));
    }

    let alerts = alerts(world, ui, ac);
    let kind = block_kind(world, ui, ac, !alerts.is_empty());
    let origin = block_origin(ui, ac.id, track.pos);

    // Leader line to the first line of the block, stopping at its edge.
    // A target beside the block meets its first line; one above or below it
    // meets its edge, so the line never crosses the text.
    let first_row = if kind == Block::Full { 0.0 } else { 1.0 };
    let rows = if kind == Block::Full { 3.0 } else { 1.0 };
    let (top, bottom) = (origin.y + BLOCK_LINE * first_row, origin.y + BLOCK_LINE * (first_row + rows));
    let beside = p.x < origin.x || p.x > origin.x + BLOCK_SIZE.x;
    let anchor = vec2(
        p.x.clamp(origin.x, origin.x + BLOCK_SIZE.x),
        if beside { p.y.clamp(top, top + BLOCK_LINE) } else { p.y.clamp(top, bottom) },
    );
    let gap = (anchor - p).normalize_or_zero() * SYMBOL_CLEAR_PX;
    draw_line(p.x + gap.x, p.y + gap.y, anchor.x, anchor.y, 1.0, color);
    if selected {
        draw_rectangle_lines(origin.x - 2.0, origin.y - 2.0, BLOCK_SIZE.x, BLOCK_SIZE.y, 1.0, SELECTED_BOX);
    }

    // An inbound handoff blinks until accepted.
    if ac.phase == Phase::Offered && !flash() {
        return;
    }

    let text = |line: &str, x: f32, row: f32, c: Color| {
        draw_text(line, origin.x + x, origin.y + BLOCK_LINE * (row + 1.0) - 3.0, BLOCK_FONT, c);
    };
    let char_width = measure_text("0", None, BLOCK_FONT as u16, 1.0).width;

    // Line 2, which is a partial data block's only line: altitude, timeshared
    // with the scratchpad; the handoff field; ground speed and category,
    // timeshared with the aircraft type.
    let altitude = format!("{:03.0}", track.altitude / 100.0);
    let speed = format!("{:02.0}{}", (track.ground_speed / 10.0).round(), category(ac));
    let pad = scratchpad(world, ac);
    let first_half = timeshare();
    let (left, left_amber) = match &pad {
        Some((pad, amber)) if !first_half => (pad.clone(), *amber),
        _ => (altitude, false),
    };
    let width = pad.as_ref().map_or(3, |(s, _)| s.len().max(3));
    let right = if first_half || kind == Block::Partial { speed } else { ac.ty.designator.to_string() };
    let handoff = if kind == Block::Full { handoff_char(world, ac) } else { ' ' };
    text(&left, 0.0, 1.0, if left_amber { WARNING } else { color });
    text(&format!("{handoff}{right}"), char_width * width as f32, 1.0, color);
    if kind == Block::Partial {
        return;
    }

    // Line 0: alerts, flashing in red until acknowledged.
    let mut x = 0.0;
    for (tag, unacknowledged) in &alerts {
        if !*unacknowledged || flash() {
            text(tag, x, -1.0, CRITICAL);
        }
        x += char_width * (tag.len() + 1) as f32;
    }

    // Line 1: callsign; an asterisk for VFR, which MSAW leaves alone.
    let star = if ac.vfr.is_some() { "*" } else { "" };
    text(&format!("{}{star}", ac.callsign), 0.0, 0.0, color);

    // Line 3: ATPA's in-trail distance, or the code it's squawking when it
    // isn't the one assigned; then the assigned altitude, while it's
    // climbing or descending to it.
    let mut x = 0.0;
    if let Some(atpa) = atpa {
        let c = match atpa.status {
            AtpaStatus::Monitor => color,
            AtpaStatus::Warning => WARNING,
            AtpaStatus::Alert => ATPA_ALERT,
        };
        let distance = format!("{:.2}", atpa.distance);
        text(&distance, 0.0, 2.0, c);
        x = char_width * (distance.len() + 1) as f32;
    } else if let Some(code) = crate::sim::delivery::wrong_code_showing(ac) {
        let assigned = ac
            .flight_plan
            .as_ref()
            .and_then(|p| p.cleared.as_ref())
            .map(|c| crate::sim::delivery::code_text(c.squawk))
            .unwrap_or_default();
        let codes = format!("{} {assigned}", crate::sim::delivery::code_text(code));
        text(&codes, 0.0, 2.0, WARNING);
        x = char_width * (codes.len() + 1) as f32;
    }
    let misheard = matches!(ac.missed_hearback(), Some(SlipKind::Altitude { .. }));
    let assigned = ac.assigned_altitude();
    let on_approach = matches!(ac.phase, Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff);
    if ac.vfr.is_none() && !on_approach && (misheard || (track.altitude - assigned).abs() > ASSIGNED_SHOWN_FT) {
        text(&format!("A{:03.0}", assigned / 100.0), x, 2.0, if misheard { WARNING } else { color });
    }
}

/// ATPA cones: from each trailing aircraft toward the one ahead, as long as
/// the in-trail minimum. Blue while spaced, yellow for a warning, orange for
/// an alert.
fn draw_atpa_cones(world: &World, ui: &UiState, atpa: &[Atpa]) {
    for a in atpa {
        let (Some(follower), Some(leader)) = (world.track(a.follower), world.track(a.leader)) else { continue };
        let shown = world.find(a.follower).is_some_and(|ac| world.is_seats(ui.seat, ac) || !ui.staffing.approach);
        if !shown {
            continue;
        }
        let color = match a.status {
            AtpaStatus::Monitor => ATPA_MONITOR,
            AtpaStatus::Warning => WARNING,
            AtpaStatus::Alert => ATPA_ALERT,
        };
        let toward = (leader.pos - follower.pos).normalize_or_zero();
        let across = glam::Vec2::new(-toward.y, toward.x) * (a.required * ATPA_CONE_WIDTH);
        let tip = ui.camera.to_screen(follower.pos);
        let end = follower.pos + toward * a.required;
        let l = ui.camera.to_screen(end + across);
        let r = ui.camera.to_screen(end - across);
        draw_line(tip.x, tip.y, l.x, l.y, 1.0, color);
        draw_line(tip.x, tip.y, r.x, r.y, 1.0, color);
        draw_line(l.x, l.y, r.x, r.y, 1.0, color);
    }
}

/// Preview of the unsent clearance, and of a drag in progress.
fn draw_pending(world: &World, ui: &UiState) {
    let Some(id) = ui.selected else { return };
    let Some(track) = world.track(id) else { return };
    let start = ui.camera.to_screen(track.pos);

    let Some(ac) = world.find(id) else { return };
    draw_rnav_path(world, ui, ac, start);
    // Its hold, or the one about to be given.
    if let Lateral::Hold(state) = ac.lateral {
        draw_racetrack(ui, &world.airport, &state.spec, ac.tas(), ac.altitude, HOLD_BUSY);
    }
    if let Some(spec) = ui.pending.hold {
        draw_dashed(start, ui.camera.to_screen(world.airport.fixes[spec.fix].pos()), GHOST);
        let color = if world.hold_fits(ac, spec.fix) { GHOST } else { WARNING };
        draw_racetrack(ui, &world.airport, &spec, ac.tas(), ac.altitude, color);
    }

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
                Some(fix) if ctrl_down() => {
                    // Amber when the hold would take it out of the sector.
                    let spec = world.hold_for(ac, fix);
                    let color = if world.hold_fits(ac, fix) { GHOST } else { WARNING };
                    draw_racetrack(ui, &world.airport, &spec, ac.tas(), ac.altitude, color);
                    (
                        ui.camera.to_screen(world.airport.fixes[fix].pos()),
                        format!("HOLD {}", world.airport.fixes[fix].name),
                    )
                }
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
    if let Some(ils) = world.ils_line() {
        line(&ils, WARNING);
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
    // Special purpose codes: "EM NVA101 RF N123AB".
    let codes: Vec<String> = world
        .aircraft
        .iter()
        .filter_map(|a| a.emergency.as_ref().filter(|e| e.declared()).map(|e| format!("{} {}", e.tag(), a.callsign)))
        .collect();
    if !codes.is_empty() {
        line(&codes.join(" "), CRITICAL);
    }
}
