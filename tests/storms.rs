//! Thunderstorms: pilots ask to go round cells, go anyway if nobody answers,
//! and won't take a vector into one.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::Staffing;
use atcgame::sim::scoring::SimEvent;
use atcgame::sim::storms::{Cell, SEVERE_LEVEL, Storms};
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

/// Approach at Harrowgate, with one cell sitting 20 miles east, 6 miles across.
fn world_with_cell(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        staffing,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    world.storms = Storms::calm();
    world.storms.cells.push(Cell::steady(Vec2::new(20.0, 0.0), 6.0, 5.0));
    world
}

/// Level at 4,000 ft, 34 miles east, heading west toward the cell.
fn westbound(id: u32, name: &str) -> Aircraft {
    let mut ac = Aircraft::new(id, name, Flow::Arrival, aircraft_type("B738").unwrap(), Vec2::new(34.0, 0.5), 270.0, 6000.0, 250.0);
    ac.target_altitude = 6000.0;
    ac.target_speed = 250.0;
    ac.entered_sector = true;
    ac
}

fn run(world: &mut World, seconds: f32, events: &mut Vec<SimEvent>) {
    for _ in 0..(seconds / TICK).ceil() as usize {
        world.tick();
        events.extend(world.drain_events());
    }
}

fn run_until(world: &mut World, max_seconds: f32, events: &mut Vec<SimEvent>, done: impl Fn(&World) -> bool) {
    for _ in 0..(max_seconds / TICK).ceil() as usize {
        if done(world) {
            return;
        }
        world.tick();
        events.extend(world.drain_events());
    }
    panic!("condition not met within {max_seconds} s; events: {events:?}");
}

fn said(world: &World, text: &str) -> bool {
    world.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.contains(text))
}

/// The worst weather the aircraft flies through over `seconds`.
fn worst_flown(world: &mut World, id: u32, seconds: f32, events: &mut Vec<SimEvent>) -> f32 {
    let mut worst: f32 = 0.0;
    for _ in 0..(seconds / TICK).ceil() as usize {
        world.tick();
        events.extend(world.drain_events());
        if let Some(ac) = world.find(id) {
            worst = worst.max(world.storms.level_at(ac.pos));
        }
    }
    worst
}

#[test]
fn a_pilot_asks_to_go_round_a_cell_flies_round_it_and_comes_back() {
    let mut world = world_with_cell(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(westbound(1, "NVA101"));
    run_until(&mut world, 30.0, &mut events, |w| w.find(1).unwrap().deviation.is_some());
    assert!(said(&world, "for weather"), "{:?}", world.log.last());
    let ac = world.find(1).unwrap();
    assert_eq!(world.awaiting(ac), Some(Awaiting::Deviation));
    let wanted = ac.deviation.unwrap().wanted;
    assert!(wanted.abs() >= 20.0);

    // D: "deviation approved, advise clear of weather".
    assert!(world.transmit(1, vec![Clearance::Deviation]));
    run(&mut world, 8.0, &mut events);
    let d = world.find(1).unwrap().deviation.unwrap();
    assert!(d.approved && d.offset == wanted);
    assert_eq!(world.awaiting(world.find(1).unwrap()), None);

    let worst = worst_flown(&mut world, 1, 600.0, &mut events);
    assert!(worst < SEVERE_LEVEL, "flew through level {worst}");
    let ac = world.find(1).unwrap();
    assert!(ac.deviation.is_none(), "still deviating");
    assert!(said(&world, "clear of the weather, resuming heading 270"), "{:?}", world.log);
    assert_eq!(ac.lateral, Lateral::Heading(270.0));
    assert!(world.score.incidents.iter().all(|i| i.points >= 0), "{:?}", world.score.incidents);
}

#[test]
fn a_deviation_nobody_answers_is_flown_anyway_and_costs_points() {
    let mut world = world_with_cell(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(westbound(1, "NVA101"));
    run_until(&mut world, 30.0, &mut events, |w| w.find(1).unwrap().deviation.is_some());
    let worst = worst_flown(&mut world, 1, 600.0, &mut events);
    assert!(worst < SEVERE_LEVEL, "flew through level {worst}");
    assert!(said(&world, "unable to wait"));
    assert_eq!(events.iter().filter(|e| matches!(e, SimEvent::DeviationIgnored { .. })).count(), 1, "{events:?}");
    assert!(world.score.incidents.iter().any(|i| i.points < 0 && i.text.contains("without an answer")));
}

#[test]
fn a_new_vector_clear_of_the_cell_answers_the_request() {
    let mut world = world_with_cell(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(westbound(1, "NVA101"));
    run_until(&mut world, 30.0, &mut events, |w| w.find(1).unwrap().deviation.is_some());
    assert!(world.transmit(1, vec![Clearance::Heading(world.airport.to_true(320.0))]));
    run(&mut world, 10.0, &mut events);
    assert!(world.find(1).unwrap().deviation.is_none());
    run(&mut world, 60.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::DeviationIgnored { .. })), "{events:?}");
}

#[test]
fn a_heading_or_direct_into_a_cell_is_refused() {
    let mut world = world_with_cell(Staffing::APPROACH);
    let mut events = Vec::new();
    // North of the cell, heading north: told to turn south into it.
    let mut ac = westbound(1, "NVA101");
    ac.pos = Vec2::new(20.0, 13.0);
    ac.heading = 0.0;
    ac.lateral = Lateral::Heading(0.0);
    world.add_aircraft(ac);
    world.transmit(1, vec![Clearance::Heading(180.0)]);
    run(&mut world, 8.0, &mut events);
    assert!(said(&world, "weather ahead"), "{:?}", world.log.last());
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(0.0));
    // Away from it is fine.
    world.transmit(1, vec![Clearance::Heading(world.airport.to_true(30.0))]);
    run(&mut world, 8.0, &mut events);
    assert!(!matches!(world.find(1).unwrap().lateral, Lateral::Heading(h) if h == 0.0));
}

#[test]
fn aircraft_nobody_is_working_go_round_by_themselves() {
    let mut world = world_with_cell(Staffing::LOCAL);
    let mut events = Vec::new();
    let mut ac = westbound(1, "NVA101");
    ac.phase = Phase::Controlled;
    world.add_aircraft(ac);
    let worst = worst_flown(&mut world, 1, 300.0, &mut events);
    assert!(worst < SEVERE_LEVEL, "flew through level {worst}");
    assert!(!said(&world, "for weather"), "nobody to ask on the player's frequency");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn a_storms_shift_has_cells_and_the_atis_mentions_them_when_close() {
    let config = WorldConfig { seed: 4, staffing: Staffing::APPROACH, weather: WeatherKind::Storms, ..Default::default() };
    let world = World::new(Airport::default_airport(), config);
    assert!(world.storms.active());
    let mut world = world_with_cell(Staffing::APPROACH);
    world.tick();
    let atis = world.atis_text();
    assert!(atis.contains("Thunderstorm 14 miles east"), "{atis}");
    // A heading that passes clear of it, 10 miles north, isn't refused.
    assert!(!world.storms.blocked(Vec2::new(30.0, 10.0), 270.0, 10.0));
    assert!(world.storms.blocked(Vec2::new(30.0, 0.0), 270.0, 10.0));
    let _ = heading_vec(0.0);
}
