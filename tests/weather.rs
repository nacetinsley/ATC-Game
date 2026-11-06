//! Runway changes: what happens to traffic already on the move.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Leg, Phase, VfrIntent};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::Staffing;
use atcgame::sim::scoring::SimEvent;
use atcgame::sim::traffic::Busyness;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{TICK, World, WorldConfig};

fn ksus() -> Airport {
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

/// Harrowgate landing 27L and departing 27R, with no traffic of its own.
fn harrowgate(staffing: Staffing, vfr: bool) -> World {
    let config = WorldConfig {
        seed: 3,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 8.0 }),
        staffing,
        vfr,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

fn config_named(world: &World, name: &str) -> usize {
    world.airport.configs.iter().position(|c| c.name == name).unwrap()
}

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn runway_of(world: &World, end: Option<usize>) -> String {
    end.map(|e| world.airport.runways[e].id.clone()).unwrap_or_default()
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

/// Queue a departure leaving by the first SID and let it be placed.
fn spawn_departure(world: &mut World) -> u32 {
    let route = world.airport.route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("B738").unwrap(), route, 10000.0);
    world.tick();
    world.aircraft.iter().find(|a| a.flow == Flow::Departure).expect("placed").id
}

/// An arrival established on final to this runway, talking to Tower.
fn final_arrival(world: &World, id: u32, runway_id: &str, miles: f32) -> Aircraft {
    let rwy = end(world, runway_id);
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let ty = aircraft_type("B738").unwrap();
    let mut ac = Aircraft::new(id, "TRL900", Flow::Arrival, ty, pos, runway.heading, altitude, 160.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

#[test]
fn a_parked_departure_moves_to_the_new_runway_keeping_its_exit() {
    let mut world = harrowgate(Staffing::NONE, false);
    let id = spawn_departure(&mut world);
    let exit = world.find(id).unwrap().exit_fix;
    assert_eq!(runway_of(&world, world.find(id).unwrap().departure_end), "27R");

    world.change_runways(config_named(&world, "East"));
    let ac = world.find(id).unwrap();
    assert_eq!(runway_of(&world, ac.departure_end), "09L");
    assert_eq!(ac.exit_fix, exit, "same exit fix from the other direction");

    // Automated Ground taxis it to the new hold line, and it goes from there.
    let mut events = Vec::new();
    run_until(&mut world, 900.0, &mut events, |w| w.find(id).is_none_or(|a| !a.on_ground));
    let ac = world.find(id).expect("still on the board");
    assert!((ac.heading - 90.0).abs() < 20.0, "departed eastbound, heading {}", ac.heading);
}

#[test]
fn queued_departures_get_a_sid_for_the_new_runways() {
    let mut world = harrowgate(Staffing::NONE, false);
    let west = world.airport.sids.iter().find(|s| s.config == "West").unwrap().fixes.clone();
    let route = world.airport.route_indices(&west);
    let exit = route.last().copied();
    world.request_departure(aircraft_type("B738").unwrap(), route.clone(), 10000.0);
    world.change_runways(config_named(&world, "East"));
    world.tick();
    let ac = world.aircraft.iter().find(|a| a.flow == Flow::Departure).expect("placed");
    assert_ne!(ac.route, route);
    assert_eq!(ac.exit_fix, exit);
    assert_eq!(runway_of(&world, ac.departure_end), "09L");
}

#[test]
fn a_departure_already_cleared_for_takeoff_keeps_its_runway() {
    let mut world = harrowgate(Staffing::NONE, false);
    let id = spawn_departure(&mut world);
    let mut events = Vec::new();
    run_until(&mut world, 900.0, &mut events, |w| w.find(id).is_some_and(|a| a.cleared_takeoff));
    world.change_runways(config_named(&world, "East"));
    assert_eq!(runway_of(&world, world.find(id).unwrap().departure_end), "27R");
    run_until(&mut world, 300.0, &mut events, |w| w.find(id).is_none_or(|a| !a.on_ground));
}

#[test]
fn with_approach_alone_departures_reappear_at_the_new_hold_line() {
    let mut world = harrowgate(Staffing::APPROACH, false);
    let id = spawn_departure(&mut world);
    world.change_runways(config_named(&world, "East"));
    assert!(world.find(id).is_none(), "taken back off the board");
    world.tick();
    let ac = world.find(id).expect("placed again");
    assert_eq!(runway_of(&world, ac.departure_end), "09L");
    let map = world.airport.surface.as_ref().unwrap();
    assert_eq!(map.departure_hold(ac.departure_end.unwrap()), Some(ac.taxi.as_ref().unwrap().node));
}

#[test]
fn nothing_departs_against_the_last_arrival_the_old_way() {
    let mut world = harrowgate(Staffing::NONE, false);
    let arrival = final_arrival(&world, 50, "27L", 6.0);
    world.add_aircraft(arrival);
    let id = spawn_departure(&mut world);
    world.change_runways(config_named(&world, "East"));
    let mut events = Vec::new();
    run_until(&mut world, 900.0, &mut events, |w| w.find(id).is_none_or(|a| a.rolling() || !a.on_ground));
    assert!(events.iter().any(|e| matches!(e, SimEvent::Landed { .. })), "the arrival landed first");
    let landed_at = world.find(50).map(|a| a.on_ground);
    assert_ne!(landed_at, Some(false), "still airborne when the departure rolled");
}

#[test]
fn an_arrival_on_its_star_switches_to_the_new_one_from_the_same_gate() {
    let config = WorldConfig {
        seed: 4,
        wind: Some(Wind { from: 270.0, speed: 8.0 }),
        staffing: Staffing::APPROACH,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    let mut events = Vec::new();
    run_until(&mut world, 600.0, &mut events, |w| w.aircraft.iter().any(|a| a.flow == Flow::Arrival));
    let id = world.aircraft.iter().find(|a| a.flow == Flow::Arrival).unwrap().id;
    let before = world.find(id).unwrap().route.clone();
    world.change_runways(config_named(&world, "East"));
    let after = &world.find(id).unwrap().route;
    assert_eq!(after.first(), before.first(), "same gate");
    let east: Vec<Vec<usize>> = world
        .airport
        .stars
        .iter()
        .filter(|s| s.config == "East")
        .map(|s| world.airport.route_indices(&s.fixes))
        .collect();
    assert!(east.contains(after), "{after:?} is an East STAR");
}

#[test]
fn vfr_pattern_traffic_re_enters_for_the_new_runway() {
    let mut world = harrowgate(Staffing::NONE, true);
    assert!(world.spawn_vfr(VfrIntent::Closed));
    let mut events = Vec::new();
    run_until(&mut world, 10.0, &mut events, |w| !w.aircraft.is_empty());
    let id = world.aircraft.last().unwrap().id;
    run_until(&mut world, 900.0, &mut events, |w| {
        w.find(id).and_then(|a| a.vfr.as_ref()).is_some_and(|v| v.leg == Leg::Downwind)
    });
    world.change_runways(config_named(&world, "East"));
    world.tick();
    let vfr = world.find(id).unwrap().vfr.clone().unwrap();
    assert_eq!(runway_of(&world, Some(vfr.end)), "09L");
    assert_eq!(vfr.leg, Leg::Entry);
    // It joins the new pattern and comes round to land on 09L, with nobody hurt.
    let new_end = end(&world, "09L");
    run_until(&mut world, 1200.0, &mut events, |w| {
        w.find(id).and_then(|a| a.vfr.as_ref()).is_some_and(|v| v.leg == Leg::Final && v.end == new_end)
    });
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn a_whole_shift_through_a_runway_change_is_clean_at_both_airports() {
    for (airport, from, to) in [(Airport::default_airport(), 270.0, 90.0), (ksus(), 250.0, 70.0)] {
        for (seed, vfr) in [(1, false), (2, true), (3, true)] {
            let config = WorldConfig {
                seed,
                wind: Some(Wind { from, speed: 8.0 }),
                staffing: Staffing::NONE,
                busyness: Busyness::Busy,
                vfr,
                ..Default::default()
            };
            let mut world = World::new(airport.clone(), config);
            world.schedule_wind_shift(600.0, Wind { from: to, speed: 12.0 });
            let before = world.config().arrival.clone();
            while world.ended.is_none() {
                world.tick();
                world.drain_events();
            }
            let code = &world.airport.code;
            assert_ne!(world.config().arrival, before, "{code} seed {seed}: no runway change");
            assert!(world.score.incidents.is_empty(), "{code} seed {seed}: {:?}", world.score.incidents);
            assert_eq!(world.score.go_arounds, 0, "{code} seed {seed}");
        }
    }
}
