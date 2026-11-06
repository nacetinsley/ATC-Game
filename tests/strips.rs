//! What each aircraft is waiting for from the player: the strip bay's
//! "waiting for you" marker.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::Staffing;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{TICK, World, WorldConfig};
use glam::Vec2;

fn harrowgate(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind {
            from: 270.0,
            speed: 0.0,
        }),
        staffing,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn final_arrival(world: &World, id: u32, miles: f32) -> Aircraft {
    let rwy = end(world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(
        id,
        "NVA101",
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

fn spawn_departure(world: &mut World) -> u32 {
    let route = world
        .airport
        .route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("B738").unwrap(), route, 10000.0);
    world.tick();
    world
        .aircraft
        .iter()
        .find(|a| a.flow == Flow::Departure)
        .expect("placed")
        .id
}

fn awaiting(world: &World, id: u32) -> Option<Awaiting> {
    world.awaiting(world.find(id).unwrap())
}

fn run_until(world: &mut World, max_seconds: f32, done: impl Fn(&World) -> bool) {
    for _ in 0..(max_seconds / TICK).ceil() as usize {
        if done(world) {
            return;
        }
        world.tick();
        world.drain_events();
    }
    panic!("condition not met within {max_seconds} s");
}

#[test]
fn a_departure_waits_to_taxi_then_for_takeoff_then_for_the_handoff() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let id = spawn_departure(&mut world);
    assert_eq!(awaiting(&world, id), Some(Awaiting::ReadyToTaxi));

    let hold = world
        .airport
        .surface
        .as_ref()
        .unwrap()
        .departure_hold(end(&world, "27R"))
        .unwrap();
    assert!(world.transmit(
        id,
        vec![Clearance::Taxi {
            to: hold,
            via: vec![]
        }]
    ));
    run_until(&mut world, 600.0, |w| {
        let a = w.find(id).unwrap();
        a.phase == Phase::Taxiing
            && a.ias == 0.0
            && a.taxi.as_ref().is_some_and(|t| t.done() && t.node == hold)
    });
    assert_eq!(awaiting(&world, id), Some(Awaiting::ReadyForTakeoff));

    assert!(world.transmit(id, vec![Clearance::ClearedForTakeoff(end(&world, "27R"))]));
    run_until(&mut world, 300.0, |w| !w.find(id).unwrap().on_ground);
    assert_eq!(
        awaiting(&world, id),
        Some(Awaiting::HandoffDue),
        "Departure is automated: hand it off"
    );
    assert!(Awaiting::HandoffDue.urgent());
}

#[test]
fn an_arrival_calls_in_then_needs_a_landing_clearance_close_in() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut ac = final_arrival(&world, 1, 8.0);
    ac.freq_since = 0.5;
    world.add_aircraft(ac);
    world.tick();
    assert_eq!(awaiting(&world, 1), Some(Awaiting::CheckIn));

    run_until(&mut world, 300.0, |w| {
        w.airport.runways[end(w, "27L")].along(w.find(1).unwrap().pos) < 3.9
    });
    assert_eq!(awaiting(&world, 1), Some(Awaiting::LandingClearance));

    assert!(world.transmit(1, vec![Clearance::ClearedToLand(end(&world, "27L"))]));
    run_until(&mut world, 10.0, |w| w.find(1).unwrap().cleared_to_land);
    assert_eq!(awaiting(&world, 1), None);
}

#[test]
fn center_offering_an_arrival_waits_for_approach_to_accept() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut ac = Aircraft::new(
        1,
        "TRL412",
        Flow::Arrival,
        aircraft_type("A320").unwrap(),
        Vec2::new(40.0, 40.0),
        225.0,
        13000.0,
        280.0,
    );
    ac.phase = Phase::Offered;
    world.add_aircraft(ac);
    assert_eq!(awaiting(&world, 1), Some(Awaiting::Accept));
    assert!(world.accept_handoff(1));
    assert_eq!(awaiting(&world, 1), None);
}

#[test]
fn nothing_is_waiting_on_an_automated_position() {
    // Local is automated on a Ground position, so a close-in arrival is its business.
    let mut world = harrowgate(Staffing::GROUND);
    world.add_aircraft(final_arrival(&world, 1, 3.0));
    assert_eq!(awaiting(&world, 1), None);
}

#[test]
fn center_offering_an_arrival_raises_a_handoff_event_at_both_airports() {
    use atcgame::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};
    use atcgame::sim::scoring::SimEvent;
    for airport in [
        Airport::default_airport(),
        Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap(),
    ] {
        let code = airport.code.clone();
        let config = WorldConfig {
            seed: 3,
            staffing: Staffing::APPROACH,
            ..Default::default()
        };
        let mut world = World::new(airport, config);
        let mut offered = false;
        for _ in 0..(900.0 / TICK) as usize {
            world.tick();
            offered |= world
                .drain_events()
                .iter()
                .any(|e| matches!(e, SimEvent::HandoffOffered { .. }));
            if offered {
                break;
            }
        }
        assert!(offered, "{code}: no handoff offered in 15 minutes");
    }
}
