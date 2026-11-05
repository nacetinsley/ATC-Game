//! Tower scenarios: Local and Ground, worked separately or combined, and the
//! automated positions for the ones the player isn't working.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::{GoAroundReason, SimEvent};
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

/// Harrowgate with the wind from the west: land 27L (south), depart 27R (north).
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

fn physical(world: &World, id: &str) -> usize {
    world.airport.surface.as_ref().unwrap().runway_of_end[end(world, id)]
}

fn node_near(world: &World, x: f32, y: f32) -> usize {
    world.airport.surface.as_ref().unwrap().nearest_node(Vec2::new(x, y))
}

fn hold_for(world: &World, id: &str) -> usize {
    world.airport.surface.as_ref().unwrap().departure_hold(end(world, id)).unwrap()
}

/// An arrival established on final to 27L, talking to Tower.
fn final_arrival(world: &World, id: u32, name: &str, miles: f32) -> Aircraft {
    let rwy = end(world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(
        id,
        name,
        Flow::Arrival,
        aircraft_type("B738").unwrap(),
        pos,
        runway.heading,
        altitude,
        160.0,
    );
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

/// A departure waiting at a ground node, talking to Ground.
fn parked_departure(world: &World, id: u32, name: &str, node: usize, runway: &str) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let mut ac = Aircraft::new(
        id,
        name,
        Flow::Departure,
        aircraft_type("B738").unwrap(),
        map.nodes[node].pos,
        0.0,
        world.airport.elevation,
        0.0,
    );
    ac.on_ground = true;
    ac.phase = Phase::Parked;
    ac.taxi = Some(Taxi::at(node));
    ac.departure_end = Some(end(world, runway));
    ac.surface_freq = Position::Ground;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.target_altitude = world.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    ac
}

/// Queue a departure through the normal traffic path and let it be placed.
fn spawn_departure(world: &mut World) -> u32 {
    let route = world.airport.route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("B738").unwrap(), route, 10000.0);
    world.tick();
    world.aircraft.iter().find(|a| a.flow == Flow::Departure).expect("placed").id
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

fn at_end_of_route(world: &World, id: u32) -> bool {
    world.find(id).is_some_and(|a| {
        a.phase == Phase::Taxiing
            && a.ias == 0.0
            && a.taxi.as_ref().is_some_and(|t| t.done())
    })
}

fn missed(events: &[SimEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::MissedHandoff { to, .. } => Some(*to),
            _ => None,
        })
        .collect()
}

#[test]
fn departure_taxis_takes_off_and_is_handed_to_departure() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    let id = spawn_departure(&mut world);
    assert_eq!(world.find(id).unwrap().phase, Phase::Parked);
    assert!(world.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.contains("ready to taxi")));

    let hold = hold_for(&world, "27R");
    assert!(world.transmit(id, vec![Clearance::Taxi { to: hold, via: vec![] }]));
    run_until(&mut world, 600.0, &mut events, |w| at_end_of_route(w, id));
    assert_eq!(world.find(id).unwrap().taxi.as_ref().unwrap().node, hold);
    let readback = &world.log.iter().rev().find(|l| l.speaker == Speaker::Pilot).unwrap().text;
    assert!(readback.starts_with("runway 27R, taxi via"), "{readback}");

    assert!(world.transmit(id, vec![Clearance::ClearedForTakeoff(end(&world, "27R"))]));
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(id).is_some_and(|a| !a.on_ground && a.altitude > w.airport.elevation + 500.0)
    });
    assert!(world.transmit(id, vec![Clearance::ContactDeparture]));
    run_until(&mut world, 30.0, &mut events, |w| w.find(id).unwrap().phase == Phase::Controlled);
    // Nobody works Approach, so it leaves the board once clear of the field.
    run_until(&mut world, 600.0, &mut events, |w| w.find(id).is_none());

    assert!(events.iter().any(|e| matches!(e, SimEvent::Exited { .. })), "{events:?}");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn arrival_lands_exits_and_taxis_in_to_park() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, "NVA101", 6.0));
    assert!(world.transmit(1, vec![Clearance::ClearedToLand(end(&world, "27L"))]));

    run_until(&mut world, 400.0, &mut events, |w| at_end_of_route(w, 1));
    let ac = world.find(1).unwrap();
    let map = world.airport.surface.as_ref().unwrap();
    let south = physical(&world, "27L");
    assert!(!world.occupies(ac, south), "stopped clear of the runway");
    assert_eq!(map.nodes[ac.taxi.as_ref().unwrap().node].hold, Some(south), "at the hold line");

    let spot = map.parking[ac.parking.expect("assigned a spot")].node;
    assert!(world.transmit(1, vec![Clearance::Taxi { to: spot, via: vec![] }]));
    run_until(&mut world, 600.0, &mut events, |w| w.find(1).is_none());

    assert!(events.iter().any(|e| matches!(e, SimEvent::Parked { .. })), "{events:?}");
    assert_eq!((world.score.landed, world.score.parked), (1, 1));
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn landing_without_a_clearance_is_a_go_around() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, "NVA101", 5.0));
    run_until(&mut world, 300.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround { reason: GoAroundReason::NoLandingClearance, .. }
    )));
}

#[test]
fn crossing_in_front_of_an_arrival_on_short_final_is_an_incursion() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    // The cargo ramp is south of 27L, so the way to 27R crosses it.
    let ramp = node_near(&world, 0.3, -0.85);
    world.add_aircraft(parked_departure(&world, 1, "TRL300", ramp, "27R"));
    let hold = hold_for(&world, "27R");
    assert!(world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]));
    let south = physical(&world, "27L");
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(1).unwrap().taxi.as_ref().unwrap().holding_short == Some(south)
    });
    let readback = &world.log.iter().rev().find(|l| l.speaker == Speaker::Pilot).unwrap().text;
    assert!(readback.contains("hold short of runway 27L"), "{readback}");

    let mut arrival = final_arrival(&world, 2, "NVA101", 1.9);
    arrival.cleared_to_land = true;
    world.add_aircraft(arrival);
    assert!(world.transmit(1, vec![Clearance::CrossRunway(south)]));
    run(&mut world, 10.0, &mut events);

    assert!(events.iter().any(|e| matches!(e, SimEvent::RunwayIncursion { callsign } if callsign == "TRL300")), "{events:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround { callsign, reason: GoAroundReason::RunwayOccupied } if callsign == "NVA101"
    )));
}

#[test]
fn crossing_with_the_runway_clear_is_fine() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    let ramp = node_near(&world, 0.3, -0.85);
    world.add_aircraft(parked_departure(&world, 1, "TRL300", ramp, "27R"));
    let hold = hold_for(&world, "27R");
    world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    let south = physical(&world, "27L");
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(1).unwrap().taxi.as_ref().unwrap().holding_short == Some(south)
    });
    // Stays put without a crossing clearance.
    run(&mut world, 30.0, &mut events);
    assert!(!world.occupies(world.find(1).unwrap(), south));
    world.transmit(1, vec![Clearance::CrossRunway(south)]);
    run_until(&mut world, 900.0, &mut events, |w| at_end_of_route(w, 1));
    assert_eq!(world.find(1).unwrap().taxi.as_ref().unwrap().node, hold);
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn nose_to_nose_on_one_taxiway_is_a_deadlock_not_a_collision() {
    let mut world = harrowgate(Staffing::GROUND);
    let mut events = Vec::new();
    let west = node_near(&world, -0.75, -0.4);
    let east = node_near(&world, 0.75, -0.4);
    world.add_aircraft(parked_departure(&world, 1, "NVA101", west, "27R"));
    world.add_aircraft(parked_departure(&world, 2, "NVA102", east, "27R"));
    let far_east = node_near(&world, 1.5, -0.4);
    let far_west = node_near(&world, -1.5, -0.4);
    world.transmit(1, vec![Clearance::Taxi { to: far_east, via: vec!["A".into()] }]);
    world.transmit(2, vec![Clearance::Taxi { to: far_west, via: vec!["A".into()] }]);
    run(&mut world, 240.0, &mut events);

    assert!(events.iter().any(|e| matches!(e, SimEvent::TaxiDeadlock { .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::GroundConflict { .. })), "{events:?}");
    assert!(world.alerts.warning(1) && world.alerts.warning(2));
}

#[test]
fn split_tower_must_hand_arrivals_to_ground_but_a_combined_position_needn_t() {
    for staffing in [Staffing::LOCAL, Staffing::LOCAL_GROUND] {
        let mut world = harrowgate(staffing);
        let mut events = Vec::new();
        world.add_aircraft(final_arrival(&world, 1, "NVA101", 6.0));
        world.transmit(1, vec![Clearance::ClearedToLand(end(&world, "27L"))]);
        run_until(&mut world, 400.0, &mut events, |w| at_end_of_route(w, 1));
        run(&mut world, 90.0, &mut events);
        if staffing == Staffing::LOCAL {
            assert_eq!(missed(&events), vec!["Ground"]);
            // The automated Ground then brings it in.
            run_until(&mut world, 600.0, &mut events, |w| w.find(1).is_none());
            assert_eq!(world.score.parked, 1);
        } else {
            assert!(missed(&events).is_empty(), "{events:?}");
        }
    }
}

#[test]
fn ground_hands_a_departure_to_tower_which_releases_it() {
    let mut world = harrowgate(Staffing::GROUND);
    let mut events = Vec::new();
    let id = spawn_departure(&mut world);
    let hold = hold_for(&world, "27R");
    world.transmit(id, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    run_until(&mut world, 600.0, &mut events, |w| at_end_of_route(w, id));
    // Tower isn't the player's, so Ground can't clear it onto the runway.
    world.transmit(id, vec![Clearance::ClearedForTakeoff(end(&world, "27R"))]);
    run(&mut world, 8.0, &mut events);
    assert!(world.log.iter().rev().any(|l| l.text.contains("not on your frequency")));

    world.transmit(id, vec![Clearance::ContactTower]);
    run_until(&mut world, 300.0, &mut events, |w| w.find(id).is_none_or(|a| !a.on_ground));
    assert!(world.log.iter().any(|l| l.text.starts_with("Tower: ") && l.text.contains("cleared for takeoff")));
    assert!(missed(&events).is_empty(), "{events:?}");
}

#[test]
fn ground_that_never_calls_tower_is_penalised_once() {
    let mut world = harrowgate(Staffing::GROUND);
    let mut events = Vec::new();
    let id = spawn_departure(&mut world);
    let hold = hold_for(&world, "27R");
    world.transmit(id, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    run_until(&mut world, 600.0, &mut events, |w| at_end_of_route(w, id));
    run_until(&mut world, 300.0, &mut events, |w| w.find(id).is_none_or(|a| !a.on_ground));
    assert_eq!(missed(&events), vec!["Tower"]);
}

#[test]
fn approach_and_tower_combined_needs_no_tower_handoff() {
    let mut world = harrowgate(Staffing::ALL);
    let mut events = Vec::new();
    let mut ac = final_arrival(&world, 1, "NVA101", 9.0);
    ac.phase = Phase::Glideslope;
    world.add_aircraft(ac);
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().phase == Phase::TowerHandedOff);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::LateHandoff { .. })), "{events:?}");
    world.transmit(1, vec![Clearance::ClearedToLand(end(&world, "27L"))]);
    run_until(&mut world, 300.0, &mut events, |w| w.find(1).unwrap().on_ground);
    assert_eq!(world.score.landed, 1);
}

#[test]
fn approach_alone_never_sees_the_surface_or_hears_the_tower() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, "NVA101", 6.0));
    let id = spawn_departure(&mut world);
    // Departures start at the hold line, not at a gate.
    assert_eq!(world.find(id).unwrap().taxi.as_ref().unwrap().node, hold_for(&world, "27R"));
    run_until(&mut world, 400.0, &mut events, |w| w.find(1).is_none());
    assert_eq!(world.score.landed, 1);
    assert!(!world.log.iter().any(|l| l.text.starts_with("Tower:")), "{:?}", world.log);
}

/// Every position automated: arrivals on final, departures from the gates.
fn automated_shift(airport: Airport, wind_from: f32, seed: u64) -> World {
    let config = WorldConfig {
        seed,
        wind: Some(Wind { from: wind_from, speed: 8.0 }),
        staffing: Staffing::NONE,
        ..Default::default()
    };
    let mut world = World::new(airport, config);
    while world.ended.is_none() {
        world.tick();
        world.drain_events();
    }
    world
}

#[test]
fn the_automated_positions_run_a_clean_shift_at_harrowgate() {
    let (mut landed, mut parked, mut exited) = (0, 0, 0);
    for seed in 0..4 {
        let world = automated_shift(Airport::default_airport(), 270.0, seed);
        assert!(world.score.incidents.is_empty(), "seed {seed}: {:?}", world.score.incidents);
        landed += world.score.landed;
        parked += world.score.parked;
        exited += world.score.exited;
    }
    assert!(landed > 10 && parked > 5 && exited > 10, "{landed} landed, {parked} parked, {exited} exited");
}

#[test]
fn the_automated_positions_run_a_clean_shift_at_ksus() {
    let ksus = Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap();
    let (mut landed, mut parked, mut exited) = (0, 0, 0);
    for (seed, wind) in [(0, 250.0), (1, 80.0), (2, 250.0)] {
        let world = automated_shift(ksus.clone(), wind, seed);
        assert!(world.score.incidents.is_empty(), "seed {seed}: {:?}", world.score.incidents);
        landed += world.score.landed;
        parked += world.score.parked;
        exited += world.score.exited;
    }
    assert!(landed > 0 && parked > 0 && exited > 0, "{landed} landed, {parked} parked, {exited} exited");
}
