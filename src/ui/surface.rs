//! The tower's surface map: runways, taxiways, hold lines, parking, and every
//! aircraft on or near the field with its tag.

use super::camera::{Camera, clip};
use super::input::UiState;
use crate::sim::World;
use crate::sim::aircraft::{Aircraft, Flow, Landing, Phase};
use crate::sim::clearance::{Clearance, spoken_runway, taxi_origin};
use crate::sim::ground_map::{EdgeKind, GroundMap};
use crate::sim::performance::WakeCategory;
use crate::sim::units::heading_vec;
use macroquad::prelude::*;

const BACKGROUND: Color = Color::new(0.04, 0.06, 0.05, 1.0);
const RUNWAY: Color = Color::new(0.30, 0.31, 0.32, 1.0);
const RUNWAY_OCCUPIED: Color = Color::new(0.45, 0.22, 0.20, 1.0);
const RUNWAY_MARK: Color = Color::new(0.85, 0.85, 0.85, 1.0);
const TAXIWAY: Color = Color::new(0.20, 0.23, 0.20, 1.0);
const TAXIWAY_LABEL: Color = Color::new(0.95, 0.80, 0.25, 1.0);
const HOLD: Color = Color::new(0.95, 0.80, 0.25, 1.0);
const PARKING: Color = Color::new(0.40, 0.45, 0.42, 1.0);
const OWNED: Color = Color::new(0.95, 0.95, 0.95, 1.0);
const OTHER: Color = Color::new(0.45, 0.65, 0.50, 1.0);
const WARNING: Color = Color::new(1.00, 0.85, 0.10, 1.0);
const CRITICAL: Color = Color::new(1.00, 0.20, 0.15, 1.0);
const ROUTE: Color = Color::new(0.30, 0.80, 1.00, 0.85);
const GHOST: Color = Color::new(1.00, 0.95, 0.40, 1.0);
const STATUS: Color = Color::new(0.10, 0.85, 0.10, 1.0);
const LABEL_BOX: Color = Color::new(0.0, 0.0, 0.0, 0.7);

/// Half the width of a runway, NM (about 75 ft).
const RUNWAY_HALF_WIDTH_NM: f32 = 0.0125;
const TAXIWAY_WIDTH_NM: f32 = 0.006;
const HOLD_BAR_NM: f32 = 0.012;
/// Show taxiway letters once zoomed in past this many pixels per NM.
const LABEL_MIN_SCALE: f32 = 250.0;
const TAG_FONT: f32 = 16.0;
const AIRCRAFT_HIT_PX: f32 = 14.0;
const PARKING_HIT_PX: f32 = 14.0;
const NODE_HIT_PX: f32 = 9.0;
const TAXIWAY_HIT_PX: f32 = 8.0;
/// Airborne traffic below this height above the field shows on the surface map.
const AIRBORNE_SHOWN_AGL: f32 = 3000.0;

fn flash() -> bool {
    (get_time() * 2.0) as i64 % 2 == 0
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
    if let Some(map) = world.airport.surface.as_ref() {
        draw_layout(world, map, cam);
        draw_routes(world, map, ui);
        draw_hover(world, map, ui);
    }
    for ac in world.aircraft.iter().filter(|a| shown(world, a)) {
        draw_aircraft(world, ui, ac, inset);
    }
    if inset {
        draw_rectangle_lines(view.x, view.y, view.w, view.h, 2.0, PARKING);
        draw_text("SURFACE", view.x + 8.0, view.y + 18.0, 16.0, STATUS);
    } else {
        draw_status(world, ui);
    }
    clip(None);
}

fn draw_layout(world: &World, map: &GroundMap, cam: &Camera) {
    // Taxiways under the runways.
    let width = thick(cam, TAXIWAY_WIDTH_NM, 2.0);
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
        // Dashed centreline once there's room for it.
        if cam.scale > 400.0 {
            let (a, b) = (to_screen(cam, runway.start), to_screen(cam, runway.end));
            let length = a.distance(b);
            let step = (b - a) / length.max(1.0);
            let mut t = 30.0;
            while t + 12.0 < length - 30.0 {
                let (p, q) = (a + step * t, a + step * (t + 12.0));
                draw_line(p.x, p.y, q.x, q.y, 1.0, RUNWAY_MARK);
                t += 24.0;
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

    // Hold bars across the taxiway, at each hold line.
    for (n, node) in map.nodes.iter().enumerate() {
        if node.hold.is_none() {
            continue;
        }
        let Some(&e) = map.adjacent[n].first() else { continue };
        let other = map.nodes[map.edges[e].other(n)].pos;
        let along = (other - node.pos).normalize_or_zero();
        let across = glam::Vec2::new(-along.y, along.x) * HOLD_BAR_NM;
        let (a, b) = (to_screen(cam, node.pos - across), to_screen(cam, node.pos + across));
        let w = thick(cam, 0.002, 2.0);
        draw_line(a.x, a.y, b.x, b.y, w, HOLD);
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
        points.extend(taxi.path.iter().skip(taxi.index).map(|&n| map.nodes[n].pos));
        polyline(cam, &points, 2.0, ROUTE, false);
    }
    for clearance in &ui.pending.tower {
        let to = match clearance {
            Clearance::Taxi { to, .. } => *to,
            Clearance::LineUpAndWait(e) | Clearance::ClearedForTakeoff(e) => map.lineup_node[*e],
            _ => continue,
        };
        let via = match clearance {
            Clearance::Taxi { via, .. } => via.clone(),
            _ => Vec::new(),
        };
        if let Some(route) = map.route(taxi_origin(ac, map), to, &via) {
            let points: Vec<glam::Vec2> = route.nodes.iter().map(|&n| map.nodes[n].pos).collect();
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

fn aircraft_color(world: &World, ac: &Aircraft) -> Color {
    if world.alerts.critical(ac.id) {
        return if flash() { CRITICAL } else { OWNED };
    }
    if world.alerts.warning(ac.id) {
        return WARNING;
    }
    if world.is_players(ac) { OWNED } else { OTHER }
}

/// What the aircraft is doing, in a few letters.
pub fn status(world: &World, ac: &Aircraft) -> String {
    let airport = &world.airport;
    let Some(map) = airport.surface.as_ref() else { return String::new() };
    let runway = |e: Option<usize>| e.map(|e| airport.runways[e].id.clone()).unwrap_or_default();
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

fn draw_aircraft(world: &World, ui: &UiState, ac: &Aircraft, inset: bool) {
    let cam = &ui.surface;
    let p = to_screen(cam, ac.pos);
    if !cam.viewport.contains(p) {
        return;
    }
    let color = aircraft_color(world, ac);
    let span_nm = match ac.ty.wake {
        WakeCategory::Small => 0.007,
        WakeCategory::Large => 0.018,
        WakeCategory::Heavy => 0.032,
    };
    let length = (span_nm * cam.scale).max(9.0);
    let fwd = heading_vec(ac.heading);
    let fwd = vec2(fwd.x, -fwd.y);
    let right = vec2(-fwd.y, fwd.x);
    let nose = p + fwd * length * 0.5;
    let tail = p - fwd * length * 0.5;
    let wing = p + fwd * length * 0.05;
    let stab = p - fwd * length * 0.38;
    let w = (length / 7.0).max(1.5);
    draw_line(nose.x, nose.y, tail.x, tail.y, w, color);
    let (wl, wr) = (wing - right * length * 0.5, wing + right * length * 0.5);
    draw_line(wl.x, wl.y, wr.x, wr.y, w, color);
    let (sl, sr) = (stab - right * length * 0.2, stab + right * length * 0.2);
    draw_line(sl.x, sl.y, sr.x, sr.y, w, color);

    let selected = ui.selected == Some(ac.id);
    if selected {
        let r = length * 0.7 + 4.0;
        draw_rectangle_lines(p.x - r, p.y - r, r * 2.0, r * 2.0, 1.0, GHOST);
    }
    if inset && !selected {
        return;
    }
    let owner = match ac.frequency() {
        crate::sim::positions::Position::Ground => "G",
        crate::sim::positions::Position::Local => "T",
        crate::sim::positions::Position::Approach => "A",
        crate::sim::positions::Position::Center => "C",
    };
    let x = p.x + length * 0.6 + 6.0;
    draw_text(format!("{} {owner}", ac.callsign), x, p.y - 4.0, TAG_FONT, color);
    draw_text(format!("{} {}", ac.ty.designator, status(world, ac)), x, p.y + 12.0, TAG_FONT, color);
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
            "WIND {:03.0}/{:02.0}  ARR {arrival} DEP {departure}{}",
            world.airport.to_magnetic(world.wind.from),
            world.wind.speed,
            vfr.map(|v| format!(" VFR {v}")).unwrap_or_default()
        ),
        STATUS,
    );
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
