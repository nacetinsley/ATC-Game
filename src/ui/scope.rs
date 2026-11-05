//! The radar scope: map, targets, data blocks. Targets come from radar returns only.

use super::camera::scope_width;
use super::input::{Drag, UiState};
use crate::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use crate::sim::airspace::Airport;
use crate::sim::units::{format_heading, heading_vec};
use crate::sim::{RadarTrack, World};
use macroquad::prelude::*;

pub const BLOCK_SIZE: Vec2 = vec2(128.0, 46.0);
const DEFAULT_BLOCK_OFFSET: Vec2 = vec2(16.0, -52.0);
const BLOCK_FONT: f32 = 18.0;
const LEADER_SECONDS: f32 = 60.0;
const RING_SPACING_NM: f32 = 10.0;
const RINGS: usize = 7;
const CENTERLINE_NM: f32 = 25.0;
const GHOST_NM: f32 = 10.0;

const BACKGROUND: Color = Color::new(0.02, 0.05, 0.05, 1.0);
const RING: Color = Color::new(0.08, 0.17, 0.16, 1.0);
const MAP: Color = Color::new(0.20, 0.36, 0.34, 1.0);
const ROUTE: Color = Color::new(0.13, 0.25, 0.24, 1.0);
const FIX: Color = Color::new(0.38, 0.55, 0.52, 1.0);
const RUNWAY: Color = Color::new(0.85, 0.90, 0.88, 1.0);
const CENTERLINE: Color = Color::new(0.30, 0.50, 0.55, 1.0);
const MVA: Color = Color::new(0.34, 0.22, 0.10, 1.0);
const CONTROLLED: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const DEPARTURE: Color = Color::new(0.45, 0.80, 1.00, 1.0);
const APPROACH: Color = Color::new(0.30, 0.95, 0.90, 1.0);
const OFFERED: Color = Color::new(1.00, 1.00, 1.00, 1.0);
const OTHER_FREQ: Color = Color::new(0.45, 0.52, 0.52, 1.0);
const WARNING: Color = Color::new(1.00, 0.60, 0.10, 1.0);
const CRITICAL: Color = Color::new(1.00, 0.20, 0.20, 1.0);
const GHOST: Color = Color::new(1.00, 0.95, 0.40, 1.0);

/// Base colour for an aircraft's phase, before any alert overrides it.
pub fn phase_color(ac: &Aircraft) -> Color {
    match ac.phase {
        Phase::Offered => OFFERED,
        Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope => APPROACH,
        Phase::TowerHandedOff | Phase::TowerRelease | Phase::Rollout | Phase::OfferedToCenter => {
            OTHER_FREQ
        }
        Phase::Controlled | Phase::GoAround => {
            if ac.flow == Flow::Departure { DEPARTURE } else { CONTROLLED }
        }
    }
}

fn aircraft_color(world: &World, ac: &Aircraft) -> Color {
    // Flash on the half-second so alerts and new offers catch the eye.
    let flash = (get_time() * 2.0) as i64 % 2 == 0;
    if world.alerts.critical(ac.id) {
        return if flash { CRITICAL } else { WHITE };
    }
    if world.alerts.warning(ac.id) {
        return WARNING;
    }
    if ac.phase == Phase::Offered && !flash {
        return OTHER_FREQ;
    }
    phase_color(ac)
}

pub fn block_origin(ui: &UiState, id: u32, pos: glam::Vec2) -> Vec2 {
    let offset = ui.block_offsets.get(&id).copied().unwrap_or(DEFAULT_BLOCK_OFFSET);
    ui.camera.to_screen(pos) + offset
}

pub fn draw(world: &World, ui: &UiState) {
    clear_background(BACKGROUND);
    draw_map(&world.airport, world, ui);
    for track in &world.radar {
        if let Some(ac) = world.find(track.id) {
            draw_target(world, ui, ac, track);
        }
    }
    draw_pending(world, ui);
    draw_alert_banner(world);
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

fn draw_map(airport: &Airport, world: &World, ui: &UiState) {
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
            draw_text(format!("MVA {:.0}", area.altitude / 100.0), label.x, label.y, 18.0, MVA);
        }
    }

    draw_polygon_outline(ui, &airport.sector, 2.0, MAP);

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
            vec2(p.x, p.y - 4.0),
            vec2(p.x - 4.0, p.y + 3.0),
            vec2(p.x + 4.0, p.y + 3.0),
            1.0,
            FIX,
        );
        draw_text(&fix.name, p.x + 6.0, p.y + 4.0, 14.0, FIX);
    }

    for runway in &airport.runways {
        let a = ui.camera.to_screen(runway.threshold());
        let b = ui.camera.to_screen(runway.far_end());
        draw_line(a.x, a.y, b.x, b.y, 3.0, RUNWAY);
    }

    // Extended centerline with a tick every five miles, on the landing runway.
    let runway = &airport.runways[world.arrival_runway()];
    let outbound = -heading_vec(runway.heading);
    let a = ui.camera.to_screen(runway.threshold());
    let b = ui.camera.to_screen(runway.threshold() + outbound * CENTERLINE_NM);
    draw_dashed(a, b, CENTERLINE);
    let mut miles = 5.0;
    while miles <= CENTERLINE_NM {
        let p = ui.camera.to_screen(runway.threshold() + outbound * miles);
        draw_circle(p.x, p.y, 2.0, CENTERLINE);
        miles += 5.0;
    }
    let label = ui.camera.to_screen(runway.threshold() + outbound * (CENTERLINE_NM + 1.0));
    draw_text(&runway.id, label.x, label.y, 18.0, CENTERLINE);
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

fn trend(track: &RadarTrack) -> char {
    if track.vertical_speed > 100.0 {
        '^'
    } else if track.vertical_speed < -100.0 {
        'v'
    } else {
        '='
    }
}

/// Short state shown on the third data block line.
fn status(ac: &Aircraft, airport: &Airport) -> String {
    match ac.phase {
        Phase::Offered => "OFFER".into(),
        Phase::ClearedApproach => "ILS".into(),
        Phase::Localizer => "LOC".into(),
        Phase::Glideslope => "GS".into(),
        Phase::TowerHandedOff | Phase::Rollout => "TWR".into(),
        Phase::GoAround => "GA".into(),
        Phase::TowerRelease => "TWR".into(),
        Phase::OfferedToCenter => "CTR".into(),
        Phase::Controlled => match ac.lateral {
            Lateral::Heading(h) => format!("H{}", format_heading(h)),
            _ => ac
                .active_fix()
                .map(|f| airport.fixes[f].name.clone())
                .unwrap_or_default(),
        },
    }
}

fn draw_target(world: &World, ui: &UiState, ac: &Aircraft, track: &RadarTrack) {
    let color = aircraft_color(world, ac);
    let p = ui.camera.to_screen(track.pos);

    for (age, old) in track.history.iter().enumerate() {
        let dot = ui.camera.to_screen(*old);
        let fade = 0.6 * (1.0 - age as f32 / track.history.len() as f32);
        draw_circle(dot.x, dot.y, 1.5, Color::new(color.r, color.g, color.b, fade));
    }

    let leader = ui.camera.to_screen(
        track.pos + track.velocity / crate::sim::units::SECONDS_PER_HOUR * LEADER_SECONDS,
    );
    draw_line(p.x, p.y, leader.x, leader.y, 1.0, color);
    draw_rectangle(p.x - 3.0, p.y - 3.0, 6.0, 6.0, color);

    let selected = ui.selected == Some(ac.id);
    if selected {
        draw_circle_lines(p.x, p.y, 11.0, 1.5, GHOST);
    }

    let origin = block_origin(ui, ac.id, track.pos);
    // The tether stops at the block's nearest edge so it never runs through the text.
    let anchor = vec2(
        p.x.clamp(origin.x, origin.x + BLOCK_SIZE.x),
        p.y.clamp(origin.y, origin.y + BLOCK_SIZE.y),
    );
    draw_line(p.x, p.y, anchor.x, anchor.y, 1.0, Color::new(color.r, color.g, color.b, 0.5));
    if selected {
        draw_rectangle_lines(origin.x, origin.y, BLOCK_SIZE.x, BLOCK_SIZE.y, 1.0, GHOST);
    }

    let line1 = format!("{} {}", ac.callsign, ac.ty.wake.letter());
    let line2 = format!(
        "{:03.0}{}{:03.0} {:03.0}",
        track.altitude / 100.0,
        trend(track),
        ac.target_altitude / 100.0,
        track.ground_speed,
    );
    let line3 = format!("{} {}", ac.ty.designator, status(ac, &world.airport));
    draw_text(&line1, origin.x + 3.0, origin.y + 13.0, BLOCK_FONT, color);
    draw_text(&line2, origin.x + 3.0, origin.y + 28.0, BLOCK_FONT, color);
    draw_text(&line3, origin.x + 3.0, origin.y + 43.0, BLOCK_FONT, color);
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
                        .map(|h| format!("HDG {}", format_heading(h)))
                        .unwrap_or_default(),
                ),
            };
            draw_line(start.x, start.y, end.x, end.y, 1.5, GHOST);
            draw_text(&label, end.x + 10.0, end.y - 8.0, 20.0, GHOST);
        }
}

fn draw_alert_banner(world: &World) {
    let name = |id: u32| world.find(id).map(|a| a.callsign.as_str()).unwrap_or("?");
    let mut y = 24.0;
    let width = scope_width();
    let mut line = |text: String, color: Color| {
        let size = measure_text(&text, None, 22, 1.0);
        draw_text(&text, (width - size.width) / 2.0, y, 22.0, color);
        y += 22.0;
    };
    for &(a, b) in &world.alerts.separation {
        line(format!("SEPARATION LOST  {} / {}", name(a), name(b)), CRITICAL);
    }
    for &id in &world.alerts.terrain {
        line(format!("LOW ALTITUDE  {}", name(id)), CRITICAL);
    }
    for &(a, b) in &world.alerts.conflict {
        line(format!("CONFLICT  {} / {}", name(a), name(b)), WARNING);
    }
    for &(a, b) in &world.alerts.wake {
        line(format!("WAKE SPACING  {} / {}", name(a), name(b)), WARNING);
    }
}
