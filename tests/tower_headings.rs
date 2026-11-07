//! Headings Tower assigns: to a departure, flown once clear of the ground,
//! and to a go-around, passed on to Approach with the go-around call.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::SimEvent;
use atcgame::sim::units::{Wind, format_heading, heading_vec};
use atcgame::sim::{Speaker, TICK, World, WorldConfig};

/// Harrowgate with the wind from the west: land 27L, depart 27R.
fn harrowgate(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        staffing,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

/// An IFR departure at the 27R hold line, talking to Tower.
fn ready_departure(world: &World, id: u32) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let hold = map.departure_hold(end(world, "27R")).unwrap();
    let ty = aircraft_type("B738").unwrap();
    let mut ac = Aircraft::new(id, "DAL20", Flow::Departure, ty, map.nodes[hold].pos, 0.0, world.airport.elevation, 0.0);
    ac.on_ground = true;
    ac.phase = Phase::Taxiing;
    ac.taxi = Some(Taxi::at(hold));
    ac.departure_end = Some(end(world, "27R"));
    ac.surface_freq = Position::Local;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.target_altitude = world.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    ac.route = vec![0];
    ac
}

fn final_arrival(world: &World, id: u32, miles: f32) -> Aircraft {
    let rwy = end(world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), pos, runway.heading, altitude, 160.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

fn run_until(world: &mut World, max_seconds: f32, events: &mut Vec<SimEvent>, done: impl Fn(&World) -> bool) {
    for _ in 0..(max_seconds / TICK).ceil() as usize {
        if done(world) {
            return;
        }
        world.tick();
        events.extend(world.drain_events());
    }
    panic!("condition not met within {max_seconds} s; events: {events:?}; log: {:?}", log(world));
}

fn log(world: &World) -> Vec<&str> {
    world.log.iter().map(|l| l.text.as_str()).collect()
}

fn pilot_said(world: &World, words: &str) -> bool {
    world.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.contains(words))
}

fn heading(world: &World, true_heading: f32) -> String {
    format_heading(world.airport.to_magnetic(true_heading))
}

/// DAL20 cleared for takeoff off 27R with a heading of 240.
fn departed_on_a_heading(staffing: Staffing) -> (World, Vec<SimEvent>) {
    let mut world = harrowgate(staffing);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    let rwy = end(&world, "27R");
    assert!(world.transmit(1, vec![Clearance::Heading(240.0), Clearance::ClearedForTakeoff(rwy)]));
    run_until(&mut world, 10.0, &mut events, |w| w.find(1).unwrap().tower_heading.is_some());
    assert!(pilot_said(&world, &format!("fly heading {}", heading(&world, 240.0))), "{:?}", log(&world));
    (world, events)
}

#[test]
fn a_departure_turns_to_towers_heading_once_clear_of_the_ground() {
    let (mut world, mut events) = departed_on_a_heading(Staffing::LOCAL);
    let elevation = world.airport.elevation;
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().altitude > elevation + 200.0);
    let runway = world.airport.runways[end(&world, "27R")].heading;
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(runway), "straight out at first");
    run_until(&mut world, 60.0, &mut events, |w| w.find(1).unwrap().lateral == Lateral::Heading(240.0));
    assert!(world.find(1).unwrap().altitude >= elevation + 400.0);
    assert!(world.find(1).unwrap().tower_heading.is_none());

    // Approach, automated, takes it on course.
    assert!(world.transmit(1, vec![Clearance::ContactDeparture]));
    run_until(&mut world, 10.0, &mut events, |w| w.find(1).unwrap().frequency() == Position::Approach);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Route);
}

#[test]
fn the_player_working_approach_keeps_towers_heading() {
    let (mut world, mut events) = departed_on_a_heading(Staffing::ALL);
    run_until(&mut world, 240.0, &mut events, |w| w.find(1).unwrap().frequency() == Position::Approach);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(240.0), "the vector is the player's to end");
}

#[test]
fn tower_cannot_turn_a_departure_round_behind_it() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    assert!(world.transmit(1, vec![Clearance::Heading(90.0)]));
    run_until(&mut world, 10.0, &mut events, |w| pilot_said(w, "unable"));
    assert!(pilot_said(&world, "too far off runway 27R"), "{:?}", log(&world));
    assert!(world.find(1).unwrap().tower_heading.is_none());
}

#[test]
fn a_go_around_flies_towers_heading_and_approach_hears_it() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, 4.0));
    assert!(world.transmit(1, vec![Clearance::GoAround, Clearance::Heading(220.0)]));
    run_until(&mut world, 10.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(220.0));
    let call = format!("Tower to Approach: go-around NVA101, runway 27L, heading {}, 3,000.", heading(&world, 220.0));
    assert!(log(&world).contains(&call.as_str()), "{:?}", log(&world));
}

#[test]
fn a_heading_alone_does_not_take_an_arrival_off_final() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, 4.0));
    assert!(world.transmit(1, vec![Clearance::Heading(220.0)]));
    run_until(&mut world, 10.0, &mut events, |w| pilot_said(w, "unable"));
    assert!(pilot_said(&world, "on final"), "{:?}", log(&world));
    assert_eq!(world.find(1).unwrap().phase, Phase::TowerHandedOff);
}
