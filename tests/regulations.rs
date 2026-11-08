//! The rules the game takes from the regulations, each pinned to its source,
//! so a change that drifts from them fails here. Sources: FAA Order
//! JO 7110.65BB Change 3, the AIM, FAA Order JO 7360.1K and 14 CFR, as
//! audited in November 2026.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::Airport;
use atcgame::sim::approach::{INDEPENDENT_FT, SAME_LANE_FT, dependent_diagonal_nm};
use atcgame::sim::clearance::{Clearance, MAX_INTERCEPT_DEG, MAX_INTERCEPT_NEAR_GATE_DEG, NO_SPEED_INSIDE_NM, phrase};
use atcgame::sim::conflict::{SEPARATION_FT, SEPARATION_NM};
use atcgame::sim::holding;
use atcgame::sim::performance::{Cwt, aircraft_type, wake_advisory, wake_interval_s, wake_required_nm};
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::SimEvent;
use atcgame::sim::units::{Wind, format_heading, heading_vec};
use atcgame::sim::vfr::PATTERN_AGL;
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

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
    panic!("condition not met within {max_seconds} s; events: {events:?}; log: {:?}", log(world));
}

fn log(world: &World) -> Vec<&str> {
    world.log.iter().map(|l| l.text.as_str()).collect()
}

fn said(world: &World, speaker: Speaker, words: &str) -> bool {
    world.log.iter().any(|l| l.speaker == speaker && l.text.contains(words))
}

/// An IFR departure of this type at the 27R hold line, talking to Tower.
fn ready_departure(world: &World, id: u32, callsign: &str, ty: &str) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let hold = map.departure_hold(end(world, "27R")).unwrap();
    let mut ac = Aircraft::new(id, callsign, Flow::Departure, aircraft_type(ty).unwrap(), map.nodes[hold].pos, 0.0, world.airport.elevation, 0.0);
    ac.on_ground = true;
    ac.phase = Phase::Taxiing;
    ac.taxi = Some(Taxi::at(hold));
    ac.departure_end = Some(end(world, "27R"));
    ac.surface_freq = Position::Local;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.target_altitude = world.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    ac
}

/// An arrival of this type established on the 27L localizer.
fn on_final(world: &World, id: u32, callsign: &str, ty: &str, miles: f32) -> Aircraft {
    let rwy = end(world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, callsign, Flow::Arrival, aircraft_type(ty).unwrap(), pos, runway.heading, altitude, 160.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

// ---- Separation ----

/// 7110.65 5-5-4: 3 miles terminal radar separation; 4-5-1: 1,000 feet vertical.
#[test]
fn radar_minima() {
    assert_eq!(SEPARATION_NM, 3.0);
    assert_eq!(SEPARATION_FT, 1000.0);
}

/// 7110.65 5-5-4, TBL 5-5-4: wake turbulence separation on approach by CWT category.
#[test]
fn wake_minima_on_approach() {
    use Cwt::*;
    assert_eq!(wake_required_nm(A, B), 5.0);
    assert_eq!(wake_required_nm(A, I), 8.0);
    assert_eq!(wake_required_nm(B, B), 3.0);
    assert_eq!(wake_required_nm(B, C), 4.0);
    assert_eq!(wake_required_nm(B, F), 5.0);
    assert_eq!(wake_required_nm(B, I), 6.0);
    assert_eq!(wake_required_nm(C, F), 3.5);
    assert_eq!(wake_required_nm(C, H), 5.0);
    assert_eq!(wake_required_nm(D, H), 6.0);
    assert_eq!(wake_required_nm(E, I), 4.0);
    assert_eq!(wake_required_nm(F, I), 4.0);
    // No wake minimum: the radar minimum.
    assert_eq!(wake_required_nm(F, F), 3.0);
    assert_eq!(wake_required_nm(I, B), 3.0);
}

/// 7110.65 3-9-6f-h: wake turbulence intervals between takeoffs.
#[test]
fn wake_intervals_between_takeoffs() {
    use Cwt::*;
    assert_eq!(wake_interval_s(A, B), Some(180.0));
    assert_eq!(wake_interval_s(B, F), Some(120.0));
    assert_eq!(wake_interval_s(D, B), Some(120.0));
    assert_eq!(wake_interval_s(C, E), Some(120.0));
    assert_eq!(wake_interval_s(C, D), None);
    assert_eq!(wake_interval_s(E, I), Some(120.0));
    assert_eq!(wake_interval_s(E, H), None);
    assert_eq!(wake_interval_s(F, I), None);
}

/// 7110.65 3-10-3: who gets a wake turbulence advisory.
#[test]
fn wake_advisories() {
    use Cwt::*;
    assert!(wake_advisory(B, F));
    assert!(wake_advisory(D, B));
    assert!(wake_advisory(C, G));
    assert!(!wake_advisory(C, D));
    assert!(wake_advisory(E, I));
    assert!(!wake_advisory(F, I));
}

/// FAA Order JO 7360.1K: type designators and CWT categories.
#[test]
fn type_designators_and_cwt_categories() {
    use Cwt::*;
    let expected = [
        ("C208", I), ("E75L", G), ("A320", F), ("B738", F), ("A359", B), ("B77W", B), ("C172", I),
        ("PC12", I), ("C56X", H), ("CL35", H), ("GLF6", F), ("P28A", I), ("SR22", I), ("B77L", B),
        ("MD11", C), ("B763", C), ("A306", C), ("B752", E), ("CRJ9", G),
    ];
    for (designator, cwt) in expected {
        assert_eq!(aircraft_type(designator).map(|t| t.cwt), Some(cwt), "{designator}");
    }
}

/// 7110.65 5-9-6 and 5-9-7: dependent diagonals and independent spacing;
/// 5-5-4: parallels under 2,500 feet are one runway for wake.
#[test]
fn parallel_approach_minima() {
    assert_eq!(SAME_LANE_FT, 2500.0);
    assert_eq!(INDEPENDENT_FT, 3600.0);
    assert_eq!(dependent_diagonal_nm(2800.0), Some(1.0));
    assert_eq!(dependent_diagonal_nm(3600.0), Some(1.0));
    assert_eq!(dependent_diagonal_nm(5000.0), Some(1.5));
    assert_eq!(dependent_diagonal_nm(8500.0), Some(2.0));
    assert_eq!(dependent_diagonal_nm(9500.0), None);
}

/// 7110.65 3-9-6: a takeoff inside the wake interval is scored, and the
/// automated Tower waits it out.
#[test]
fn a_takeoff_too_soon_behind_a_heavy_is_scored() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    let rwy = end(&world, "27R");
    world.add_aircraft(ready_departure(&world, 1, "BAW1", "B77W"));
    assert!(world.transmit(1, vec![Clearance::ClearedForTakeoff(rwy)]));
    run_until(&mut world, 180.0, &mut events, |w| !w.find(1).unwrap().on_ground);
    world.add_aircraft(ready_departure(&world, 2, "DAL20", "B738"));
    assert!(world.wake_wait(world.find(2).unwrap()).is_some(), "2 minutes behind a category B");
    assert!(world.transmit(2, vec![Clearance::ClearedForTakeoff(rwy)]));
    run_until(&mut world, 90.0, &mut events, |w| w.find(2).unwrap().rolling());
    assert!(
        events.iter().any(|e| matches!(e, SimEvent::WakeDeparture { leader, follower } if leader == "BAW1" && follower == "DAL20")),
        "{events:?}"
    );
    // The clearance warned of it.
    assert!(said(&world, Speaker::Controller, "cleared for takeoff, caution wake turbulence"), "{:?}", log(&world));
}

#[test]
fn no_wake_interval_behind_a_lighter_aircraft() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    let rwy = end(&world, "27R");
    world.add_aircraft(ready_departure(&world, 1, "DAL10", "B738"));
    assert!(world.transmit(1, vec![Clearance::ClearedForTakeoff(rwy)]));
    run_until(&mut world, 180.0, &mut events, |w| !w.find(1).unwrap().on_ground);
    world.add_aircraft(ready_departure(&world, 2, "DAL20", "A320"));
    assert!(world.wake_wait(world.find(2).unwrap()).is_none());
}

// ---- Approaches and speeds ----

/// 7110.65 5-9-2, TBL 5-9-1: 30 degrees, or 20 within 2 miles of the gate;
/// 5-7-2: no speed adjustments inside 5 miles.
#[test]
fn intercept_and_speed_limits() {
    assert_eq!(MAX_INTERCEPT_DEG, 30.0);
    assert_eq!(MAX_INTERCEPT_NEAR_GATE_DEG, 20.0);
    assert_eq!(NO_SPEED_INSIDE_NM, 5.0);
}

/// Joining the final 30 degrees just outside the approach gate is refused;
/// at 20 degrees it's fine.
#[test]
fn a_steep_intercept_near_the_gate_is_refused() {
    for (cut, refused) in [(30.0, true), (20.0, false)] {
        let mut world = harrowgate(Staffing::APPROACH);
        let mut events = Vec::new();
        let rwy = end(&world, "27L");
        let runway = world.airport.runways[rwy].clone();
        let along = world.airport.approach_gate_nm(rwy) + 2.5;
        let side = Vec2::new(0.0, 1.0);
        let pos = runway.threshold() - heading_vec(runway.heading) * along + side * 0.5;
        // Right of the course turns left onto it, and vice versa.
        let heading = if runway.cross(pos) > 0.0 { runway.heading - cut } else { runway.heading + cut };
        let altitude = world.airport.glideslope_altitude(rwy, along) - 300.0;
        let mut ac = Aircraft::new(1, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), pos, heading, altitude, 200.0);
        ac.lateral = Lateral::Heading(heading);
        ac.target_altitude = altitude;
        ac.entered_sector = true;
        world.add_aircraft(ac);
        assert!(world.transmit(1, vec![Clearance::ClearedIls(rwy)]));
        run(&mut world, 8.0, &mut events);
        assert_eq!(said(&world, Speaker::Pilot, "intercept angle too steep"), refused, "{cut}: {:?}", log(&world));
    }
}

#[test]
fn no_speed_assignment_inside_five_miles() {
    // Combined, so the player can speak to an aircraft with Tower.
    let mut world = harrowgate(Staffing::ALL);
    let mut events = Vec::new();
    world.add_aircraft(on_final(&world, 1, "NVA101", "B738", 4.0));
    assert!(world.transmit(1, vec![Clearance::Speed(160.0)]));
    run(&mut world, 8.0, &mut events);
    assert!(said(&world, Speaker::Pilot, "inside the final approach fix"), "{:?}", log(&world));
}

/// 14 CFR 91.117(b): 200 knots below 2,500 feet within 4 miles of a Class C
/// or D airport. Harrowgate is Class C.
#[test]
fn departures_keep_to_200_knots_near_a_class_c_airport() {
    let mut world = harrowgate(Staffing::LOCAL);
    world.add_aircraft(ready_departure(&world, 1, "DAL20", "B738"));
    let rwy = end(&world, "27R");
    let elevation = world.airport.elevation;
    assert!(world.transmit(1, vec![Clearance::ClearedForTakeoff(rwy)]));
    let mut sampled = false;
    for _ in 0..(240.0 / TICK) as usize {
        world.tick();
        let Some(a) = world.find(1) else { break };
        if a.altitude - elevation <= 2500.0 && a.pos.length() <= 4.0 && !a.on_ground {
            assert!(a.ias <= 200.5, "{} kt at {} ft", a.ias, a.altitude);
            sampled |= a.ias > 190.0;
        }
    }
    assert!(sampled, "it reached the limit inside the area");
}

/// AIM 5-3-8: maximum holding speeds.
#[test]
fn holding_speeds() {
    assert_eq!(holding::max_speed(6000.0), 200.0);
    assert_eq!(holding::max_speed(14000.0), 230.0);
    assert_eq!(holding::max_speed(15000.0), 265.0);
}

/// AIM 4-3-3: the traffic pattern at 1,000 feet.
#[test]
fn pattern_altitude() {
    assert_eq!(PATTERN_AGL, 1000.0);
}

// ---- Phraseology ----

/// 7110.65 5-9-4: an altitude to maintain until established, then "cleared
/// ILS runway (n) approach".
#[test]
fn ils_approach_clearance_wording() {
    let world = harrowgate(Staffing::APPROACH);
    let rwy = end(&world, "27L");
    let mut ac = Aircraft::new(1, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), Vec2::new(14.0, 2.0), 240.0, 3000.0, 200.0);
    ac.lateral = Lateral::Heading(240.0);
    ac.target_altitude = 3000.0;
    assert_eq!(
        phrase(&Clearance::ClearedIls(rwy), &ac, &world.airport, &[]),
        "maintain 3,000 until established on the localizer, cleared ILS runway 27L approach"
    );
}

/// 7110.65 3-10-5 and 3-9-4: the runway first.
#[test]
fn landing_and_line_up_wording() {
    let world = harrowgate(Staffing::LOCAL);
    let ac = on_final(&world, 1, "NVA101", "B738", 4.0);
    assert_eq!(phrase(&Clearance::ClearedToLand(end(&world, "27L")), &ac, &world.airport, &[]), "runway 27L, cleared to land");
    let dep = ready_departure(&world, 2, "DAL20", "B738");
    assert_eq!(phrase(&Clearance::LineUpAndWait(end(&world, "27R")), &dep, &world.airport, &[]), "runway 27R, line up and wait");
}

/// 7110.65 5-8-2 and 3-9-10: "turn left heading (n)" or "fly runway heading",
/// then "runway (n), cleared for takeoff".
#[test]
fn takeoff_with_a_heading_wording() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1, "DAL20", "B738"));
    let rwy = end(&world, "27R");
    let runway_heading = world.airport.runways[rwy].heading;
    assert_eq!(
        phrase(&Clearance::Heading(runway_heading), world.find(1).unwrap(), &world.airport, &[]),
        "fly runway heading"
    );
    assert!(world.transmit(1, vec![Clearance::Heading(240.0), Clearance::ClearedForTakeoff(rwy)]));
    run(&mut world, 1.0, &mut events);
    let heading = format_heading(world.airport.to_magnetic(240.0));
    let expected = format!("DAL20, turn left heading {heading}, runway 27R, cleared for takeoff.");
    assert!(log(&world).contains(&expected.as_str()), "{:?}", log(&world));
}

/// 7110.65 3-7-2: "cross runway (n) at (taxiway)".
#[test]
fn runway_crossing_names_the_taxiway() {
    let mut world = harrowgate(Staffing::ALL);
    let mut events = Vec::new();
    let map = world.airport.surface.as_ref().unwrap();
    let node = map.nearest_node(Vec2::new(0.3, -0.85));
    let hold = map.departure_hold(end(&world, "27R")).unwrap();
    let south = map.runway_of_end[end(&world, "27L")];
    let mut ac = Aircraft::new(1, "TRL300", Flow::Departure, aircraft_type("B738").unwrap(), map.nodes[node].pos, 0.0, world.airport.elevation, 0.0);
    ac.on_ground = true;
    ac.phase = Phase::Parked;
    ac.taxi = Some(Taxi::at(node));
    ac.departure_end = Some(end(&world, "27R"));
    ac.surface_freq = Position::Ground;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    world.add_aircraft(ac);
    assert!(world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]));
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(1).and_then(|a| a.taxi.as_ref()).is_some_and(|t| t.holding_short == Some(south))
    });
    let words = phrase(&Clearance::CrossRunway(south), world.find(1).unwrap(), &world.airport, &[]);
    assert!(words.starts_with("cross runway 27L at "), "{words}");
}

/// 7110.65 3-10-3: a landing clearance behind a heavy carries the caution.
#[test]
fn landing_behind_a_heavy_is_cautioned() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(on_final(&world, 1, "BAW1", "B77W", 3.0));
    world.add_aircraft(on_final(&world, 2, "NVA101", "B738", 8.5));
    assert!(world.transmit(2, vec![Clearance::ClearedToLand(end(&world, "27L"))]));
    run(&mut world, 1.0, &mut events);
    assert!(said(&world, Speaker::Controller, "NVA101, runway 27L, cleared to land, caution wake turbulence"), "{:?}", log(&world));
}
