//! The tower's surface map: runways, taxiways, hold lines, parking, and every
//! aircraft on or near the field with its tag.

use super::camera::{Camera, clip};
use super::input::UiState;
use crate::sim::mistakes::SlipKind;
use crate::sim::World;
use crate::sim::aircraft::{Aircraft, Flow, Landing, Phase};
use crate::sim::clearance::{Clearance, spoken_runway, taxi_origin};
use crate::sim::ground_map::{EdgeKind, GroundMap};
use crate::sim::performance::WakeCategory;
use crate::sim::units::heading_vec;
use macroquad::prelude::*;
use super::gfx::{draw_circle, draw_circle_lines, draw_line, draw_rectangle, draw_rectangle_lines, draw_text, draw_triangle, measure_text, mouse_position};

// Colours from CRC's ASDE-X DAY scheme, matched to a screenshot: teal
// ground, dark grey pavement, black runways, white targets with orange
// heavies, and green data blocks.
const BACKGROUND: Color = Color::new(0.00, 0.35, 0.47, 1.0);
const RUNWAY: Color = Color::new(0.0, 0.0, 0.0, 1.0);
/// A game aid ASDE-X doesn't have: an occupied runway shows dark red.
const RUNWAY_OCCUPIED: Color = Color::new(0.32, 0.05, 0.05, 1.0);
const RUNWAY_MARK: Color = Color::new(0.90, 0.90, 0.90, 1.0);
const TAXIWAY: Color = Color::new(0.20, 0.20, 0.20, 1.0);
const TAXIWAY_LABEL: Color = Color::new(0.95, 0.80, 0.25, 1.0);
/// Hold bars on a runway no one is landing on or departing from: ASDE-X
/// doesn't draw these, so they're kept faint, just to show where to click.
const HOLD: Color = Color::new(0.32, 0.32, 0.32, 1.0);
/// Hold bars on a runway in use, green as CRC draws them.
const HOLD_ACTIVE: Color = Color::new(0.10, 0.90, 0.10, 1.0);
/// A closed runway: red hatching, and white X's on both ends.
const CLOSED_HATCH: Color = Color::new(0.95, 0.10, 0.10, 1.0);
const CLOSED_X: Color = Color::new(0.95, 0.95, 0.95, 1.0);
const ALERT_BOX: Color = Color::new(0.0, 0.0, 0.0, 0.85);
/// Gates, cyan as in ASDE-X.
const PARKING: Color = Color::new(0.20, 0.85, 0.95, 1.0);
const INSET_BORDER: Color = Color::new(0.40, 0.40, 0.40, 1.0);
const TARGET: Color = Color::new(0.95, 0.95, 0.95, 1.0);
const TARGET_HEAVY: Color = Color::new(1.00, 0.55, 0.10, 1.0);
/// Data blocks: green, brighter for the aircraft you're talking to.
const BLOCK_OWNED: Color = Color::new(0.20, 1.00, 0.20, 1.0);
const BLOCK_OTHER: Color = Color::new(0.12, 0.75, 0.12, 1.0);
const WARNING: Color = Color::new(1.00, 0.85, 0.10, 1.0);
const CRITICAL: Color = Color::new(1.00, 0.20, 0.15, 1.0);
const ROUTE: Color = Color::new(0.30, 0.80, 1.00, 0.85);
const GHOST: Color = Color::new(1.00, 0.95, 0.40, 1.0);
const STATUS: Color = Color::new(0.10, 0.85, 0.10, 1.0);
const LABEL_BOX: Color = Color::new(0.0, 0.0, 0.0, 0.7);

/// Half the width of a runway, NM (about 75 ft).
const RUNWAY_HALF_WIDTH_NM: f32 = 0.0125;
const TAXIWAY_WIDTH_NM: f32 = 0.010;
const HOLD_BAR_NM: f32 = 0.012;
/// Show taxiway letters once zoomed in past this many pixels per NM.
const LABEL_MIN_SCALE: f32 = 250.0;
const TAG_FONT: f32 = 16.0;
/// The data block sits this far up and right of the target, px.
const LEADER_PX: f32 = 22.0;
/// How long each timeshared field shows, seconds.
const TIMESHARE_SECONDS: f64 = 1.5;
/// Ground speed only timeshares into the block above this, knots.
const MOVING_KT: f32 = 5.0;
/// An arrival this close to an occupied or closed runway sets off safety
/// logic, and the spoken alert, NM. Outside the sim's 1 NM go-around check,
/// so there's a few seconds' warning before the pilot goes around.
pub const ARRIVAL_ALERT_NM: f32 = 1.5;
/// A runway is in use, and its hold bars show, while an arrival is this close, NM.
const LANDING_NM: f32 = 2.0;
/// The alert message box holds this many alerts.
const ALERTS_SHOWN: usize = 5;
const AIRCRAFT_HIT_PX: f32 = 14.0;
const PARKING_HIT_PX: f32 = 14.0;
const NODE_HIT_PX: f32 = 9.0;
const TAXIWAY_HIT_PX: f32 = 8.0;
/// Airborne traffic below this height above the field shows on the surface map.
const AIRBORNE_SHOWN_AGL: f32 = 3000.0;

fn flash() -> bool {
    (get_time() * 2.0) as i64 % 2 == 0
}

fn timeshare() -> bool {
    (get_time() / TIMESHARE_SECONDS) as i64 % 2 == 0
}

fn to_screen(cam: &Camera, p: glam::Vec2) -> Vec2 {
    cam.to_screen(p)
}

fn thick(cam: &Camera, nm: f32, min_px: f32) -> f32 {
    (nm * cam.scale).max(min_px)
}

fn shown(world: &World, ac: &Aircraft) -> bool {
    ac.on_ground || ac.altitude - world.airport.elevation < AIRBORNE_SHOWN_AGL
}

pub fn draw(world: &World, ui: &UiState, inset: bool) {
    let cam = &ui.surface;
    let view = cam.viewport;
    clip(Some(view));
    draw_rectangle(view.x, view.y, view.w, view.h, BACKGROUND);
    let alerts = safety_alerts(world);
    if let Some(map) = world.airport.surface.as_ref() {
        draw_layout(world, map, cam);
        draw_routes(world, map, ui);
        draw_hover(world, map, ui);
    }
    let shown_aircraft: Vec<&Aircraft> = world.aircraft.iter().filter(|a| shown(world, a)).collect();
    // Targets first, so no block is hidden under another aircraft.
    for ac in &shown_aircraft {
        draw_target(world, ui, ac, &alerts);
    }
    for ac in &shown_aircraft {
        draw_block(world, ui, ac, &alerts, inset);
    }
    if inset {
        draw_rectangle_lines(view.x, view.y, view.w, view.h, 2.0, INSET_BORDER);
        draw_text("SURFACE", view.x + 8.0, view.y + 18.0, 16.0, STATUS);
    } else {
        draw_status(world, ui);
        draw_alert_box(world, ui, &alerts);
    }
    clip(None);
}

fn draw_layout(world: &World, map: &GroundMap, cam: &Camera) {
    // Taxiways under the runways.
    let width = thick(cam, TAXIWAY_WIDTH_NM, 5.0);
    for edge in &map.edges {
        if let EdgeKind::Taxiway(_) = edge.kind {
            let (a, b) = (to_screen(cam, map.nodes[edge.a].pos), to_screen(cam, map.nodes[edge.b].pos));
            draw_line(a.x, a.y, b.x, b.y, width, TAXIWAY);
            draw_circle(a.x, a.y, width / 2.0, TAXIWAY);
            draw_circle(b.x, b.y, width / 2.0, TAXIWAY);
        }
    }

    for (index, runway) in map.runways.iter().enumerate() {
        let occupied = world.aircraft.iter().any(|a| world.occupies(a, index));
        let dir = (runway.end - runway.start).normalize_or_zero();
        let side = glam::Vec2::new(-dir.y, dir.x) * RUNWAY_HALF_WIDTH_NM;
        let corners = [runway.start + side, runway.end + side, runway.end - side, runway.start - side]
            .map(|p| to_screen(cam, p));
        let color = if occupied { RUNWAY_OCCUPIED } else { RUNWAY };
        draw_triangle(corners[0], corners[1], corners[2], color);
        draw_triangle(corners[0], corners[2], corners[3], color);
        // A closed runway: red hatching along it, and a white X on each end.
        if world.closed_physical() == Some(index) {
            let length = runway.start.distance(runway.end);
            let across = side / RUNWAY_HALF_WIDTH_NM;
            let w = 1.5;
            for (a, b) in [(corners[0], corners[1]), (corners[3], corners[2])] {
                draw_line(a.x, a.y, b.x, b.y, w, CLOSED_HATCH);
            }
            // Diagonals a fixed screen distance apart, edge to edge.
            let (s0, s1) = (to_screen(cam, runway.start), to_screen(cam, runway.end));
            let px_len = s0.distance(s1);
            let step = 14.0_f32.max(px_len / 400.0);
            let slant = (RUNWAY_HALF_WIDTH_NM * 2.0 * cam.scale).max(4.0);
            let mut t = 0.0;
            while t + slant < px_len {
                let (f, g) = (t / px_len, (t + slant) / px_len);
                let a = to_screen(cam, runway.start + (runway.end - runway.start) * f - side);
                let b = to_screen(cam, runway.start + (runway.end - runway.start) * g + side);
                draw_line(a.x, a.y, b.x, b.y, w, CLOSED_HATCH);
                t += step;
            }
            // Wide enough to read when zoomed out, but never past the pavement's length.
            let arm = (12.0 / cam.scale).max(RUNWAY_HALF_WIDTH_NM * 1.6).min(length * 0.08);
            for t in [0.06, 0.94] {
                let at = runway.start + dir * length * t;
                for sign in [1.0, -1.0] {
                    let (a, b) = (to_screen(cam, at + (dir + across * sign) * arm), to_screen(cam, at - (dir + across * sign) * arm));
                    draw_line(a.x, a.y, b.x, b.y, thick(cam, 0.003, 2.5), CLOSED_X);
                }
            }
        }
        // Each end's number at its threshold, just outside the pavement.
        for &end in &runway.ends {
            let r = &world.airport.runways[end];
            let at = to_screen(cam, r.threshold() - heading_vec(r.heading) * 0.04);
            let size = measure_text(&r.id, None, 18, 1.0);
            draw_text(&r.id, at.x - size.width / 2.0, at.y + size.height / 2.0, 18.0, RUNWAY_MARK);
        }
    }

    // Hold bars across the taxiway, at each hold line: lit while the runway
    // is in use, as ASDE-X shows them, and dim the rest of the time.
    let active = active_runways(world, map);
    for (n, node) in map.nodes.iter().enumerate() {
        let Some(runway) = node.hold else { continue };
        let Some(&e) = map.adjacent[n].first() else { continue };
        let other = map.nodes[map.edges[e].other(n)].pos;
        let along = (other - node.pos).normalize_or_zero();
        let across = glam::Vec2::new(-along.y, along.x) * HOLD_BAR_NM;
        let (a, b) = (to_screen(cam, node.pos - across), to_screen(cam, node.pos + across));
        let lit = active.contains(&runway);
        let w = thick(cam, 0.002, if lit { 3.0 } else { 2.0 });
        draw_line(a.x, a.y, b.x, b.y, w, if lit { HOLD_ACTIVE } else { HOLD });
    }

    for spot in &map.parking {
        let p = to_screen(cam, map.nodes[spot.node].pos);
        draw_circle_lines(p.x, p.y, 4.0, 1.0, PARKING);
        if cam.scale > LABEL_MIN_SCALE {
            draw_text(&spot.name, p.x + 6.0, p.y + 12.0, 14.0, PARKING);
        }
    }

    if cam.scale > LABEL_MIN_SCALE {
        draw_taxiway_labels(map, cam);
    }
}

/// One letter per taxiway, on its longest stretch.
fn draw_taxiway_labels(map: &GroundMap, cam: &Camera) {
    let mut best: Vec<(&str, f32, glam::Vec2)> = Vec::new();
    for edge in &map.edges {
        let Some(name) = edge.taxiway().filter(|n| !n.is_empty()) else { continue };
        let mid = (map.nodes[edge.a].pos + map.nodes[edge.b].pos) / 2.0;
        match best.iter_mut().find(|b| b.0 == name) {
            Some(b) if b.1 < edge.length => *b = (name, edge.length, mid),
            Some(_) => {}
            None => best.push((name, edge.length, mid)),
        }
    }
    for (name, _, mid) in best {
        let p = to_screen(cam, mid);
        let size = measure_text(name, None, 16, 1.0);
        draw_rectangle(p.x - size.width / 2.0 - 3.0, p.y - size.height / 2.0 - 3.0, size.width + 6.0, size.height + 6.0, LABEL_BOX);
        draw_text(name, p.x - size.width / 2.0, p.y + size.height / 2.0, 16.0, TAXIWAY_LABEL);
    }
}

fn polyline(cam: &Camera, points: &[glam::Vec2], width: f32, color: Color, dashed: bool) {
    for pair in points.windows(2) {
        let (a, b) = (to_screen(cam, pair[0]), to_screen(cam, pair[1]));
        if !dashed {
            draw_line(a.x, a.y, b.x, b.y, width, color);
            continue;
        }
        let length = a.distance(b);
        let step = (b - a) / length.max(1.0);
        let mut t = 0.0;
        while t < length {
            let (p, q) = (a + step * t, a + step * (t + 6.0).min(length));
            draw_line(p.x, p.y, q.x, q.y, width, color);
            t += 11.0;
        }
    }
}

/// The selected aircraft's route, and the one it would get if the player transmitted now.
fn draw_routes(world: &World, map: &GroundMap, ui: &UiState) {
    let Some(ac) = ui.selected.and_then(|id| world.find(id)) else { return };
    if !ac.on_ground {
        return;
    }
    let cam = &ui.surface;
    if let Some(taxi) = &ac.taxi {
        let mut points = vec![ac.pos];
        // The route it was cleared, even if the pilot has turned off it.
        let (path, from) = match ac.slip.as_ref().map(|s| &s.kind) {
            Some(SlipKind::WrongTurn { intended, junction, .. }) => (intended, if taxi.index > *junction { *junction } else { taxi.index }),
            _ => (&taxi.path, taxi.index),
        };
        points.extend(path.iter().skip(from).map(|&n| map.nodes[n].pos));
        polyline(cam, &points, 2.0, ROUTE, false);
    }
    for clearance in &ui.pending.tower {
        let origin = taxi_origin(ac, map);
        let nodes = match clearance {
            Clearance::Taxi { to, via } => map.route(origin, *to, via).map(|r| r.nodes),
            Clearance::LineUpAndWait(e) | Clearance::ClearedForTakeoff(e) => {
                map.lineup_route(&world.airport.runways, *e, origin, ac.ty.min_runway_ft)
            }
            _ => continue,
        };
        if let Some(nodes) = nodes {
            let points: Vec<glam::Vec2> = nodes.iter().map(|&n| map.nodes[n].pos).collect();
            polyline(cam, &points, 2.5, GHOST, true);
        }
    }
}

/// Where a click would send the selected aircraft.
fn draw_hover(world: &World, map: &GroundMap, ui: &UiState) {
    let selected = ui.selected.and_then(|id| world.find(id)).filter(|a| a.on_ground);
    if selected.is_none() || !ui.surface.contains_mouse() {
        return;
    }
    let mouse = Vec2::from(mouse_position());
    if is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl) {
        if let Some(name) = taxiway_at(map, &ui.surface, mouse) {
            draw_text(format!("via {name}"), mouse.x + 12.0, mouse.y - 10.0, 18.0, GHOST);
        }
    } else if let Some(node) = node_at(map, &ui.surface, mouse) {
        let p = to_screen(&ui.surface, map.nodes[node].pos);
        draw_circle_lines(p.x, p.y, 7.0, 1.5, GHOST);
    }
}

/// One ASDE-X safety logic alert: the runway or taxiway, who's involved, and
/// what's wrong, the three lines of CRC's Alert Message Box.
#[derive(Debug, Clone, PartialEq)]
pub struct SafetyAlert {
    pub area: String,
    pub ids: Vec<u32>,
    pub text: &'static str,
}

impl SafetyAlert {
    fn involves(&self, id: u32) -> bool {
        self.ids.contains(&id)
    }
}

fn involved(alerts: &[SafetyAlert], id: u32) -> bool {
    alerts.iter().any(|a| a.involves(id))
}

/// The runway an airborne aircraft is about to land on, if it's close in.
fn landing_on(world: &World, ac: &Aircraft, within_nm: f32) -> Option<usize> {
    if ac.on_ground {
        return None;
    }
    let end = match ac.phase {
        Phase::TowerHandedOff => ac.approach_runway,
        Phase::Pattern => ac.vfr.as_ref().filter(|v| v.landing.is_some()).map(|v| v.end),
        _ => None,
    }?;
    let runway = &world.airport.runways[end];
    let along = runway.along(ac.pos);
    (along < within_nm && along > -runway.length && runway.cross(ac.pos).abs() < 0.5).then_some(end)
}

/// Physical runways someone is landing on or departing from: their hold
/// bars light up.
fn active_runways(world: &World, map: &GroundMap) -> Vec<usize> {
    let mut out = Vec::new();
    for ac in &world.aircraft {
        let end = match ac.phase {
            Phase::LinedUp => ac.departure_end,
            Phase::TowerRelease if ac.on_ground => ac.departure_end,
            Phase::Rollout => ac.approach_runway,
            _ => landing_on(world, ac, LANDING_NM),
        };
        if let Some(e) = end {
            out.push(map.runway_of_end[e]);
        }
    }
    out
}

/// What ASDE-X's safety logic is alerting on now: arrivals to occupied or
/// closed runways, departures rolling with the runway ahead occupied or
/// closed, and aircraft onto a runway without a clearance.
pub fn safety_alerts(world: &World) -> Vec<SafetyAlert> {
    let Some(map) = world.airport.surface.as_ref() else { return Vec::new() };
    let airport = &world.airport;
    let closed = world.closed_physical();
    let mut out: Vec<SafetyAlert> = Vec::new();
    let mut push = |alert: SafetyAlert| {
        if !out.contains(&alert) {
            out.push(alert);
        }
    };
    for ac in &world.aircraft {
        if let Some(end) = landing_on(world, ac, ARRIVAL_ALERT_NM) {
            let physical = map.runway_of_end[end];
            let area = airport.runways[end].id.clone();
            if closed == Some(physical) {
                push(SafetyAlert { area, ids: vec![ac.id], text: "ARRIVAL CLOSED RWY" });
            } else {
                let mut ids = vec![ac.id];
                ids.extend(world.aircraft.iter().filter(|o| o.id != ac.id && world.occupies(o, physical)).map(|o| o.id));
                if ids.len() > 1 {
                    push(SafetyAlert { area, ids, text: "ARRIVAL RWY OCCUPIED" });
                }
            }
        }
        if ac.on_ground && ac.phase == Phase::TowerRelease {
            let Some(end) = ac.departure_end else { continue };
            let physical = map.runway_of_end[end];
            let runway = &airport.runways[end];
            let area = runway.id.clone();
            if closed == Some(physical) {
                push(SafetyAlert { area, ids: vec![ac.id], text: "DEPARTURE CLOSED RWY" });
                continue;
            }
            // Only what's ahead of the roll: the next departure lined up
            // behind it is no hazard.
            let mine = runway.along(ac.pos);
            let mut ids = vec![ac.id];
            ids.extend(
                world
                    .aircraft
                    .iter()
                    .filter(|o| o.id != ac.id && world.occupies(o, physical) && runway.along(o.pos) < mine)
                    .map(|o| o.id),
            );
            if ids.len() > 1 {
                push(SafetyAlert { area, ids, text: "DEPARTURE RWY OCCUPIED" });
            }
        }
        if let Some(SlipKind::Incursion { runway, conflict: true, entered: true }) = ac.slip.as_ref().map(|s| &s.kind) {
            push(SafetyAlert { area: spoken_runway(airport, map, *runway), ids: vec![ac.id], text: "RWY INCURSION" });
        }
    }
    out
}

/// The target's colour: white, orange for the heavy icon.
fn aircraft_color(world: &World, ac: &Aircraft, alerts: &[SafetyAlert]) -> Color {
    let normal = if heavy_icon(ac) { TARGET_HEAVY } else { TARGET };
    // Safety logic: flashes between red and the target's normal colour.
    if involved(alerts, ac.id) || world.alerts.critical(ac.id) {
        return if flash() { CRITICAL } else { normal };
    }
    if world.alerts.warning(ac.id) {
        return WARNING;
    }
    normal
}

/// The ASDE-X category letter, for a non-CWT facility as CRC documents it:
/// T (TCAS), B (heavy), L (B757), J (super), V (VFR), W (heavy VFR). A piston
/// single without TCAS has none.
fn category(ac: &Aircraft) -> &'static str {
    let d = ac.ty.designator;
    let heavy = ac.ty.wake == WakeCategory::Heavy;
    let vfr = ac.vfr.is_some();
    if matches!(d, "A388" | "A124" | "A225") {
        "J"
    } else if d.starts_with("B75") {
        "L"
    } else if heavy && vfr {
        "W"
    } else if heavy {
        "B"
    } else if vfr {
        "V"
    } else if piston(ac) {
        ""
    } else {
        "T"
    }
}

fn piston(ac: &Aircraft) -> bool {
    matches!(ac.ty.designator, "C172" | "C182" | "PA28" | "PA32" | "PA44" | "SR20" | "SR22" | "BE36" | "BE58" | "DA40" | "DA42")
}

/// Heavies, B757s and supers get ASDE-X's heavy target icon.
fn heavy_icon(ac: &Aircraft) -> bool {
    matches!(category(ac), "J" | "L" | "B" | "W")
}

/// What the aircraft is doing, in a few letters.
pub fn status(world: &World, ac: &Aircraft) -> String {
    let airport = &world.airport;
    let Some(map) = airport.surface.as_ref() else { return String::new() };
    let runway = |e: Option<usize>| e.map(|e| airport.runways[e].id.clone()).unwrap_or_default();
    if ac.on_ground && ac.emergency.as_ref().is_some_and(|e| e.stops_on_runway()) && ac.phase == Phase::Rollout {
        return "DISABLED".into();
    }
    match ac.phase {
        Phase::Parked => match ac.taxi.as_ref().and_then(|t| map.parking.iter().find(|p| p.node == t.node)) {
            Some(spot) => format!("{} {}", spot.name, runway(ac.departure_end)),
            None => "PARKED".into(),
        },
        Phase::Taxiing => {
            let Some(taxi) = &ac.taxi else { return "TAXI".into() };
            if let Some(r) = taxi.holding_short {
                return format!("HOLD {}", spoken_runway(airport, map, r));
            }
            if taxi.holding {
                return "HOLD POSN".into();
            }
            // Bound for another runway's hold line: a runway change moved it.
            let goal = taxi.path.last().copied().unwrap_or(taxi.node);
            let own_hold = ac.departure_end.and_then(|e| map.departure_hold(e));
            let stale = ac.flow == Flow::Departure
                && own_hold != Some(goal)
                && (0..airport.runways.len()).any(|e| map.departure_hold(e) == Some(goal));
            if stale {
                return format!("RE-TAXI {}", runway(ac.departure_end));
            }
            if taxi.done() {
                let at_hold = ac.departure_end.is_some_and(|e| map.departure_hold(e) == Some(taxi.node));
                return if at_hold { format!("RDY {}", runway(ac.departure_end)) } else { "STOPPED".into() };
            }
            if taxi.blocked_by.is_some() {
                return "WAIT".into();
            }
            match ac.flow {
                Flow::Departure => format!("TAXI {}", runway(ac.departure_end)),
                Flow::Arrival => match ac.parking {
                    Some(p) => format!("TAXI {}", map.parking[p].name),
                    None => "TAXI".into(),
                },
            }
        }
        Phase::LinedUp => format!("LUAW {}", runway(ac.departure_end)),
        Phase::TowerRelease if ac.on_ground => format!("ROLL {}", runway(ac.departure_end)),
        Phase::Rollout => "ROLLOUT".into(),
        Phase::Pattern => {
            let Some(vfr) = &ac.vfr else { return String::new() };
            let mark = match (vfr.landing, vfr.extend, vfr.follow) {
                (Some(Landing::FullStop), ..) => " FS",
                (Some(Landing::TouchAndGo), ..) => " TG",
                (Some(Landing::Option), ..) => " OP",
                (None, true, _) => " EX",
                (None, false, Some(_)) => " FL",
                _ => "",
            };
            format!("{} {}{mark}", crate::sim::vfr::leg_label(vfr), runway(Some(vfr.end)))
        }
        _ if ac.flow == Flow::Arrival && ac.approach_runway.is_some() => {
            let rwy = ac.approach_runway.expect("checked");
            let miles = airport.runways[rwy].along(ac.pos).max(0.0);
            let clearance = if ac.cleared_to_land { "CLR" } else { "" };
            format!("{} {miles:.1} {clearance}", airport.runways[rwy].id)
        }
        _ => format!("{:03.0}", ac.altitude / 100.0),
    }
}

/// The target's size on screen, px: ASDE-X draws two icons, not one per type.
fn target_length(cam: &Camera, ac: &Aircraft) -> f32 {
    if heavy_icon(ac) { (0.034 * cam.scale).max(17.0) } else { (0.020 * cam.scale).max(12.0) }
}

/// A filled aircraft silhouette pointing along the track, and the red octagon
/// round it when safety logic alerts.
fn draw_target(world: &World, ui: &UiState, ac: &Aircraft, alerts: &[SafetyAlert]) {
    let cam = &ui.surface;
    let p = to_screen(cam, ac.pos);
    if !cam.viewport.contains(p) {
        return;
    }
    let color = aircraft_color(world, ac, alerts);
    let length = target_length(cam, ac);
    let heavy = heavy_icon(ac);
    let fwd = heading_vec(ac.heading);
    let fwd = vec2(fwd.x, -fwd.y);
    let right = vec2(-fwd.y, fwd.x);
    let at = |f: f32, r: f32| p + fwd * length * f + right * length * r;

    // Fuselage.
    let body = if heavy { 0.07 } else { 0.06 };
    let n = at(0.5, 0.0);
    draw_triangle(at(0.42, body), at(0.42, -body), at(-0.45, body * 0.6), color);
    draw_triangle(at(0.42, -body), at(-0.45, -body * 0.6), at(-0.45, body * 0.6), color);
    draw_triangle(n, at(0.42, body), at(0.42, -body), color);
    // Swept wings: the heavy icon's are broader, with longer chord.
    let (span, chord, sweep) = if heavy { (0.52, 0.22, 0.24) } else { (0.48, 0.15, 0.14) };
    for side in [1.0, -1.0] {
        let root_front = at(0.14, 0.0);
        let root_back = at(0.14 - chord, 0.0);
        let tip = at(0.14 - sweep, span * side);
        let tip_back = at(0.14 - sweep - chord * 0.35, span * side);
        draw_triangle(root_front, tip, root_back, color);
        draw_triangle(root_back, tip, tip_back, color);
        // Tailplane.
        draw_triangle(at(-0.30, 0.0), at(-0.48, 0.2 * side), at(-0.44, 0.0), color);
        if heavy {
            // Engine pods under each wing.
            let pod = at(0.04, span * 0.45 * side);
            draw_circle(pod.x, pod.y, (length * 0.04).max(1.2), color);
        }
    }

    if involved(alerts, ac.id) {
        let r = length * 0.75 + 6.0;
        let w = if flash() { 2.5 } else { 1.5 };
        for i in 0..8 {
            let angle = |k: i32| (k as f32 * 45.0 + 22.5).to_radians();
            let (a, b) = (angle(i), angle(i + 1));
            let (pa, pb) = (p + vec2(a.cos(), a.sin()) * r, p + vec2(b.cos(), b.sin()) * r);
            draw_line(pa.x, pa.y, pb.x, pb.y, w, CRITICAL);
        }
    }
    if ui.selected == Some(ac.id) {
        let r = length * 0.7 + 4.0;
        draw_rectangle_lines(p.x - r, p.y - r, r * 2.0, r * 2.0, 1.0, GHOST);
    }
    // The mouse is over its strip: point it out.
    if ui.hovered == Some(ac.id) {
        draw_circle_lines(p.x, p.y, length + 8.0, 1.5, Color::new(0.98, 0.95, 0.40, 0.9));
    }
}

/// The ASDE-X data block, as CRC shows it with the altitude, sensor and
/// category fields turned off (the usual tower setup):
///
/// ```text
/// EM                 line 0: track status (here, an emergency)
/// NVA101             line 1: callsign, and altitude in hundreds once airborne
/// B738 TAXI 27L      line 2: type, then ground speed in tens of knots
///                    timeshared with the scratchpad
/// ```
///
/// The scratchpad is the game's status ("HOLD 27L", "RDY 27L"), and holds
/// steady while the aircraft is stopped, when it matters most. While safety
/// logic alerts, everyone not involved drops to a partial block: the callsign.
fn draw_block(world: &World, ui: &UiState, ac: &Aircraft, alerts: &[SafetyAlert], inset: bool) {
    let cam = &ui.surface;
    let p = to_screen(cam, ac.pos);
    if !cam.viewport.contains(p) {
        return;
    }
    let selected = ui.selected == Some(ac.id);
    let alerting = involved(alerts, ac.id);
    if inset && !selected && !alerting {
        return;
    }
    let full = selected || alerting || alerts.is_empty();
    let color = if world.is_players(ac) || selected { BLOCK_OWNED } else { BLOCK_OTHER };

    let mut lines: Vec<(String, Color)> = Vec::new();
    if full {
        if let Some(tag) = ac.emergency.as_ref().map(|e| e.tag()) {
            lines.push((tag.to_string(), CRITICAL));
        }
        if ac.on_ground {
            lines.push((ac.callsign.clone(), color));
        } else {
            let altitude = (ac.altitude / 100.0).round().max(0.0) as i32;
            lines.push((format!("{} {altitude:03}", ac.callsign), color));
        }
        let gs = ac.ground_speed();
        let field = if gs > MOVING_KT && !timeshare() {
            format!("{:02.0}", (gs / 10.0).round())
        } else {
            status(world, ac)
        };
        lines.push((format!("{} {field}", ac.ty.designator), color));
    } else {
        lines.push((ac.callsign.clone(), color));
    }

    // Up and to the right of the target, on a leader line.
    let line_h = TAG_FONT;
    let r = target_length(cam, ac) * 0.5;
    // It flips left, or below, rather than run off the edge of the view.
    let width = lines.iter().map(|(t, _)| measure_text(t, None, TAG_FONT as u16, 1.0).width).fold(0.0, f32::max);
    let height = lines.len() as f32 * line_h;
    let view = cam.viewport;
    let right = p.x + r + LEADER_PX + width <= view.x + view.w - 4.0;
    let up = p.y - r - LEADER_PX * 0.6 - height / 2.0 >= view.y + 4.0;
    let x = if right { p.x + r + LEADER_PX } else { p.x - r - LEADER_PX - width };
    let middle = if up { p.y - r - LEADER_PX * 0.6 } else { p.y + r + LEADER_PX * 0.6 };
    let top = middle - height / 2.0;
    let toward = vec2(if right { 1.0 } else { -1.0 }, if up { -1.0 } else { 1.0 }).normalize();
    let from = p + toward * (r + 2.0);
    let to_x = if right { x - 3.0 } else { x + width + 3.0 };
    draw_line(from.x, from.y, to_x, middle, 1.0, color);
    for (i, (text, c)) in lines.iter().enumerate() {
        draw_text(text, x, top + (i as f32 + 1.0) * line_h - 3.0, TAG_FONT, *c);
    }
}

/// CRC's Alert Message Box: up to five alerts, three lines each, at the
/// bottom left of the surface view, clear of the status lines and the radar inset.
fn draw_alert_box(world: &World, ui: &UiState, alerts: &[SafetyAlert]) {
    if alerts.is_empty() {
        return;
    }
    let view = ui.surface.viewport;
    let size = 18.0;
    let name = |id: u32| world.find(id).map(|a| a.callsign.clone()).unwrap_or_else(|| "UNKN".into());
    let entries: Vec<[String; 3]> = alerts
        .iter()
        .take(ALERTS_SHOWN)
        .map(|a| [a.area.clone(), a.ids.iter().map(|&id| name(id)).collect::<Vec<_>>().join(" "), a.text.to_string()])
        .collect();
    let width = entries
        .iter()
        .flatten()
        .map(|l| measure_text(l, None, size as u16, 1.0).width)
        .fold(0.0, f32::max)
        + 20.0;
    let height = entries.len() as f32 * (3.0 * size + 8.0) + 8.0;
    let (x, y) = (view.x + 12.0, view.y + view.h - height - 12.0);
    draw_rectangle(x, y, width, height, ALERT_BOX);
    draw_rectangle_lines(x, y, width, height, 2.0, CRITICAL);
    let mut ty = y + 4.0;
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            draw_line(x + 6.0, ty, x + width - 6.0, ty, 1.0, Color::new(0.5, 0.1, 0.1, 1.0));
        }
        for line in entry {
            ty += size;
            draw_text(line, x + 10.0, ty - 2.0, size, CRITICAL);
        }
        ty += 8.0;
    }
}

fn draw_status(world: &World, ui: &UiState) {
    let view = ui.surface.viewport;
    let mut y = view.y + 22.0;
    let mut line = |text: &str, color: Color| {
        draw_text(text, view.x + 12.0, y, 18.0, color);
        y += 18.0;
    };
    let utc = (ui.clock_start + world.time as f64).rem_euclid(86_400.0) as u64;
    line(
        &format!(
            "{:02}{:02}/{:02}  {}  {}",
            utc / 3600,
            utc / 60 % 60,
            utc % 60,
            world.airport.code,
            world.staffing().label().to_uppercase()
        ),
        STATUS,
    );
    let (arrival, departure, vfr) = world.runway_ids();
    line(
        &format!(
            "ATIS {}  WIND {:03.0}/{:02.0}  ARR {arrival} DEP {departure}{}",
            world.weather.letter(),
            world.airport.to_magnetic(world.wind.from),
            world.wind.speed,
            vfr.map(|v| format!(" VFR {v}")).unwrap_or_default()
        ),
        STATUS,
    );
    if let Some(change) = world.runway_change_line() {
        line(&change, WARNING);
    }
    if let Some(closed) = world.closure_line() {
        line(&closed, WARNING);
    }
    let paused = if ui.paused { "  PAUSED" } else { "" };
    line(&format!("x{:.0}{paused}", ui.rate), STATUS);
    if !ui.via.is_empty() {
        line(&format!("VIA {}", ui.via.join(" ")), GHOST);
    }
    let names = |pairs: &std::collections::BTreeSet<(u32, u32)>, tag: &str, color: Color, line: &mut dyn FnMut(&str, Color)| {
        for &(a, b) in pairs {
            let name = |id: u32| world.find(id).map(|a| a.callsign.clone()).unwrap_or_default();
            line(&format!("{}*{} {tag}", name(a), name(b)), color);
        }
    };
    names(&world.alerts.ground, "CONFLICT", CRITICAL, &mut line);
    names(&world.alerts.deadlock, "NOSE TO NOSE", WARNING, &mut line);
}

/// The aircraft under the cursor, nearest first.
pub fn aircraft_at(world: &World, cam: &Camera, mouse: Vec2) -> Option<u32> {
    world
        .aircraft
        .iter()
        .filter(|a| shown(world, a))
        .map(|a| (cam.to_screen(a.pos).distance(mouse), a.id))
        .filter(|(d, _)| *d <= AIRCRAFT_HIT_PX)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// A taxi destination under the cursor: a parking spot or hold line first, then any node.
pub fn node_at(map: &GroundMap, cam: &Camera, mouse: Vec2) -> Option<usize> {
    let near = |n: usize, px: f32| {
        let d = cam.to_screen(map.nodes[n].pos).distance(mouse);
        (d <= px).then_some(d)
    };
    let preferred = map
        .parking
        .iter()
        .map(|p| p.node)
        .chain((0..map.nodes.len()).filter(|&n| map.nodes[n].hold.is_some()))
        .filter_map(|n| near(n, PARKING_HIT_PX).map(|d| (d, n)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    preferred
        .or_else(|| {
            (0..map.nodes.len())
                .filter_map(|n| near(n, NODE_HIT_PX).map(|d| (d, n)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
        })
        .map(|(_, n)| n)
}

/// The named taxiway under the cursor.
pub fn taxiway_at(map: &GroundMap, cam: &Camera, mouse: Vec2) -> Option<String> {
    map.edges
        .iter()
        .filter_map(|e| {
            let name = e.taxiway().filter(|n| !n.is_empty())?;
            let (a, b) = (cam.to_screen(map.nodes[e.a].pos), cam.to_screen(map.nodes[e.b].pos));
            let ab = b - a;
            let t = ((mouse - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = (a + ab * t).distance(mouse);
            (d <= TAXIWAY_HIT_PX).then(|| (d, name.to_string()))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, name)| name)
}
