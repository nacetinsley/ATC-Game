//! Pilot mistakes: misheard instructions, busted altitudes, wrong turns, and
//! the rare runway incursion or takeoff without clearance. Each test forces
//! the mistake rather than waiting for the dice.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::mistakes::{Force, PilotMistakes, SlipKind};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::SimEvent;
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

fn last_pilot_line(world: &World) -> String {
    world.log.iter().rev().find(|l| l.speaker == Speaker::Pilot).unwrap().text.clone()
}

fn caught(events: &[SimEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::SlipCaught { what, major, .. } => Some((what.clone(), *major)),
            _ => None,
        })
        .collect()
}

fn missed(events: &[SimEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::SlipMissed { what, major, .. } => Some((what.clone(), *major)),
            _ => None,
        })
        .collect()
}

/// An arrival on Approach's frequency, east of the field and level.
fn radar_arrival(altitude: f32, force: Force) -> Aircraft {
    let mut ac = Aircraft::new(
        1,
        "NVA101",
        Flow::Arrival,
        aircraft_type("B738").unwrap(),
        Vec2::new(28.0, 6.0),
        270.0,
        altitude,
        250.0,
    );
    ac.entered_sector = true;
    ac.force_slip = Some(force);
    ac
}

fn slip(world: &World, id: u32) -> Option<SlipKind> {
    world.find(id).unwrap().slip.as_ref().map(|s| s.kind.clone())
}

#[test]
fn a_misheard_altitude_is_read_back_wrong_and_flown_until_corrected() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(radar_arrival(9000.0, Force::Hearback));
    assert!(world.transmit(1, vec![Clearance::Altitude(7000.0)]));
    run(&mut world, 8.0, &mut events);

    let Some(SlipKind::Altitude { assigned, heard }) = slip(&world, 1) else { panic!("{:?}", slip(&world, 1)) };
    assert_eq!(assigned, 7000.0);
    assert!(heard == 6000.0 || heard == 8000.0, "{heard}");
    let readback = last_pilot_line(&world);
    assert!(readback.contains(&atcgame::sim::clearance::thousands(heard)), "{readback}");
    let ac = world.find(1).unwrap();
    assert_eq!(ac.target_altitude, heard);
    // The scope and strip still show what the controller said.
    assert_eq!(ac.assigned_altitude(), 7000.0);

    // "Negative, descend and maintain 7,000."
    assert!(world.transmit(1, vec![Clearance::Altitude(7000.0)]));
    run(&mut world, 8.0, &mut events);
    assert!(slip(&world, 1).is_none());
    assert_eq!(world.find(1).unwrap().target_altitude, 7000.0);
    assert_eq!(caught(&events).len(), 1, "{events:?}");
    assert!(missed(&events).is_empty(), "{events:?}");
    assert!(world.score.incidents.iter().all(|i| i.points >= 0), "{:?}", world.score.incidents);
}

#[test]
fn a_misheard_altitude_left_alone_is_missed_and_shows_amber() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(radar_arrival(9000.0, Force::Hearback));
    world.transmit(1, vec![Clearance::Altitude(7000.0)]);
    run(&mut world, 75.0, &mut events);

    assert_eq!(missed(&events).len(), 1, "{events:?}");
    let ac = world.find(1).unwrap();
    assert!(ac.missed_hearback().is_some());
    assert_ne!(ac.target_altitude, 7000.0, "still flying what it heard");
    assert!(world.score.incidents.iter().any(|i| i.text.contains("hearback error missed") && i.points < 0));

    // Put right late: no credit, but the amber goes.
    world.transmit(1, vec![Clearance::Altitude(7000.0)]);
    run(&mut world, 8.0, &mut events);
    assert!(world.find(1).unwrap().slip.is_none());
    assert!(caught(&events).is_empty(), "{events:?}");
}

#[test]
fn a_misheard_heading_is_flown_until_a_new_vector() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(radar_arrival(9000.0, Force::Hearback));
    let assigned = world.airport.to_true(240.0);
    world.transmit(1, vec![Clearance::Heading(assigned)]);
    run(&mut world, 8.0, &mut events);

    let Some(SlipKind::Heading { heard, .. }) = slip(&world, 1) else { panic!("{:?}", slip(&world, 1)) };
    let ac = world.find(1).unwrap();
    assert_eq!(ac.lateral, Lateral::Heading(heard));
    assert_eq!(ac.assigned_heading(), Some(assigned));

    world.transmit(1, vec![Clearance::Heading(assigned)]);
    run(&mut world, 8.0, &mut events);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(assigned));
    assert_eq!(caught(&events).len(), 1, "{events:?}");
}

#[test]
fn a_bust_caught_on_mode_c_earns_credit() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(radar_arrival(10000.0, Force::Bust));
    world.transmit(1, vec![Clearance::Altitude(7000.0)]);
    run(&mut world, 8.0, &mut events);
    // The readback was right.
    assert!(last_pilot_line(&world).contains("7,000"), "{}", last_pilot_line(&world));
    let Some(SlipKind::Bust { peak, .. }) = slip(&world, 1) else { panic!("{:?}", slip(&world, 1)) };
    assert!((6000.0..=6600.0).contains(&peak), "{peak}");

    run_until(&mut world, 300.0, &mut events, |w| w.find(1).unwrap().altitude < 6750.0);
    world.transmit(1, vec![Clearance::Altitude(7000.0)]);
    run(&mut world, 8.0, &mut events);
    assert_eq!(caught(&events).len(), 1, "{events:?}");
    assert!(missed(&events).is_empty());
    assert_eq!(world.find(1).unwrap().target_altitude, 7000.0);
}

#[test]
fn a_bust_left_alone_is_corrected_by_the_pilot_and_missed() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(radar_arrival(10000.0, Force::Bust));
    world.transmit(1, vec![Clearance::Altitude(7000.0)]);
    run(&mut world, 8.0, &mut events);
    assert!(world.find(1).unwrap().slip.is_some());
    run_until(&mut world, 400.0, &mut events, |w| w.find(1).unwrap().slip.is_none());

    assert_eq!(missed(&events).len(), 1, "{events:?}");
    assert!(last_pilot_line(&world).contains("back to 7,000"), "{}", last_pilot_line(&world));
    assert_eq!(world.find(1).unwrap().target_altitude, 7000.0);
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

/// A departure waiting at a ground node, talking to Ground.
fn parked_departure(world: &World, id: u32, name: &str, node: usize) -> Aircraft {
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
    ac.departure_end = Some(end(world, "27R"));
    ac.surface_freq = Position::Ground;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.target_altitude = world.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    ac
}

/// A departure that takes a wrong turn on its way to 27R.
fn wrong_turn_world() -> (World, Vec<usize>, usize) {
    let mut world = harrowgate(Staffing::GROUND);
    let gate = world.airport.surface.as_ref().unwrap().parking[0].node;
    let mut ac = parked_departure(&world, 1, "NVA101", gate);
    ac.force_slip = Some(Force::WrongTurn);
    world.add_aircraft(ac);
    let hold = hold_for(&world, "27R");
    assert!(world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]));
    let mut events = Vec::new();
    run(&mut world, 8.0, &mut events);
    let Some(SlipKind::WrongTurn { intended, junction, .. }) = slip(&world, 1) else {
        panic!("no wrong turn: {:?}", slip(&world, 1))
    };
    assert_eq!(intended.last(), Some(&hold));
    (world, intended, junction)
}

#[test]
fn a_wrong_turn_caught_with_hold_position_scores_and_the_aircraft_stops() {
    let (mut world, _, junction) = wrong_turn_world();
    let mut events = Vec::new();
    run_until(&mut world, 600.0, &mut events, |w| w.find(1).unwrap().taxi.as_ref().unwrap().index > junction);
    world.transmit(1, vec![Clearance::HoldPosition]);
    run(&mut world, 60.0, &mut events);
    assert_eq!(caught(&events).len(), 1, "{events:?}");
    let ac = world.find(1).unwrap();
    assert!(ac.slip.is_none());
    assert_eq!(ac.ias, 0.0);

    // A new route takes it to the runway after all.
    let hold = hold_for(&world, "27R");
    world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    run_until(&mut world, 900.0, &mut events, |w| {
        w.find(1).unwrap().taxi.as_ref().is_some_and(|t| t.done() && t.node == hold)
    });
    assert!(missed(&events).is_empty(), "{events:?}");
}

#[test]
fn a_wrong_turn_nobody_notices_ends_with_a_call_for_progressive_taxi() {
    let (mut world, intended, _) = wrong_turn_world();
    let mut events = Vec::new();
    run_until(&mut world, 900.0, &mut events, |w| w.find(1).unwrap().slip.is_none());
    assert_eq!(missed(&events).len(), 1, "{events:?}");
    assert!(last_pilot_line(&world).contains("missed our turn"), "{}", last_pilot_line(&world));
    let ac = world.find(1).unwrap();
    // Off its route, short of the runway, waiting for help.
    assert!(!intended.contains(&ac.taxi.as_ref().unwrap().node));
    assert_eq!(world.awaiting(ac), Some(Awaiting::ReadyToTaxi));
}

/// An arrival on final to `runway`, cleared to land and talking to Tower.
fn final_arrival(world: &World, id: u32, runway: &str, miles: f32) -> Aircraft {
    let rwy = end(world, runway);
    let r = &world.airport.runways[rwy];
    let pos = r.threshold() - heading_vec(r.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, "BRZ202", Flow::Arrival, aircraft_type("B738").unwrap(), pos, r.heading, altitude, 150.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.cleared_to_land = true;
    ac
}

/// TRL300 taxis from the cargo ramp to 27R, crossing 27L, and doesn't stop
/// at the 27L hold line. With `traffic`, an arrival is on short final.
fn incursion_world(traffic: bool) -> (World, Vec<SimEvent>) {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    let ramp = node_near(&world, 0.3, -0.85);
    let mut ac = parked_departure(&world, 1, "TRL300", ramp);
    ac.force_slip = Some(Force::Incursion);
    world.add_aircraft(ac);
    let hold = hold_for(&world, "27R");
    world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    let south = physical(&world, "27L");
    // The hold line: the last node before the runway zone.
    run_until(&mut world, 300.0, &mut events, |w| {
        let ac = w.find(1).unwrap();
        let map = w.airport.surface.as_ref().unwrap();
        let taxi = ac.taxi.as_ref().unwrap();
        taxi.next().is_some_and(|n| map.nodes[n].hold == Some(south))
            && ac.pos.distance(map.nodes[taxi.next().unwrap()].pos) < 0.02
    });
    if traffic {
        world.add_aircraft(final_arrival(&world, 2, "27L", 1.8));
    }
    run_until(&mut world, 60.0, &mut events, |w| w.occupies(w.find(1).unwrap(), south));
    run(&mut world, 1.0, &mut events);
    (world, events)
}

#[test]
fn an_incursion_handled_with_a_go_around_scores_and_is_not_the_controllers_incursion() {
    let (mut world, mut events) = incursion_world(true);
    assert!(events.iter().any(|e| matches!(e, SimEvent::PilotDeviation { .. })), "{events:?}");
    world.transmit(2, vec![Clearance::GoAround]);
    run(&mut world, 30.0, &mut events);
    assert_eq!(caught(&events), vec![("runway incursion handled".to_string(), true)], "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::RunwayIncursion { .. })), "{events:?}");
}

#[test]
fn an_incursion_ignored_is_a_major_miss_but_still_not_the_controllers_incursion() {
    let (mut world, mut events) = incursion_world(true);
    run(&mut world, 30.0, &mut events);
    assert_eq!(missed(&events), vec![("runway incursion not acted on".to_string(), true)], "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::RunwayIncursion { .. })), "{events:?}");
}

#[test]
fn an_incursion_with_nobody_coming_is_only_logged() {
    let (mut world, mut events) = incursion_world(false);
    run(&mut world, 60.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::PilotDeviation { .. })), "{events:?}");
    assert!(caught(&events).is_empty() && missed(&events).is_empty(), "{events:?}");
    assert!(world.score.incidents.iter().all(|i| i.points == 0), "{:?}", world.score.incidents);
}

/// NVA101 lined up on 27R without a takeoff clearance, about to roll anyway.
/// With `traffic`, an arrival for 27R is close in when it does.
fn early_roll_world(traffic: bool) -> (World, Vec<SimEvent>) {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    let hold = hold_for(&world, "27R");
    let mut ac = parked_departure(&world, 1, "NVA101", hold);
    ac.phase = Phase::Taxiing;
    ac.surface_freq = Position::Local;
    ac.force_slip = Some(Force::EarlyRoll);
    world.add_aircraft(ac);
    world.transmit(1, vec![Clearance::LineUpAndWait(end(&world, "27R"))]);
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().phase == Phase::LinedUp);
    let Some(SlipKind::EarlyRoll { due, .. }) = slip(&world, 1) else { panic!("{:?}", slip(&world, 1)) };
    run_until(&mut world, 120.0, &mut events, |w| w.time > due - 1.0);
    if traffic {
        world.add_aircraft(final_arrival(&world, 2, "27R", 1.5));
    }
    run_until(&mut world, 5.0, &mut events, |w| w.find(1).unwrap().phase == Phase::TowerRelease);
    assert!(last_pilot_line(&world).contains("rolling"), "{}", last_pilot_line(&world));
    (world, events)
}

#[test]
fn a_takeoff_without_clearance_stopped_in_time_scores() {
    let (mut world, mut events) = early_roll_world(true);
    world.transmit(1, vec![Clearance::StopTakeoff]);
    run(&mut world, 8.0, &mut events);
    let ac = world.find(1).unwrap();
    assert_eq!(ac.phase, Phase::LinedUp, "{events:?}");
    assert_eq!(caught(&events), vec![("takeoff without clearance stopped".to_string(), true)]);
}

#[test]
fn a_takeoff_roll_too_fast_to_stop_carries_on() {
    let (mut world, mut events) = early_roll_world(true);
    run_until(&mut world, 60.0, &mut events, |w| w.find(1).unwrap().ias > 90.0);
    world.transmit(1, vec![Clearance::StopTakeoff]);
    run(&mut world, 8.0, &mut events);
    assert!(last_pilot_line(&world).contains("too fast to stop"), "{}", last_pilot_line(&world));
    run(&mut world, 10.0, &mut events);
    assert_eq!(missed(&events), vec![("takeoff without clearance not acted on".to_string(), true)]);
}

#[test]
fn a_takeoff_without_clearance_into_an_empty_sky_is_only_logged() {
    let (mut world, mut events) = early_roll_world(false);
    run(&mut world, 30.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::PilotDeviation { .. })), "{events:?}");
    assert!(caught(&events).is_empty() && missed(&events).is_empty(), "{events:?}");
    assert!(world.find(1).unwrap().slip.is_none());
}

#[test]
fn automated_controllers_never_meet_a_mistake() {
    // A whole automated shift with mistakes on Frequent plays exactly as with them off.
    let digest = |mistakes| {
        let config = WorldConfig { seed: 11, mistakes, staffing: Staffing::NONE, ..Default::default() };
        let mut world = World::new(Airport::default_airport(), config);
        while world.ended.is_none() {
            world.tick();
        }
        world.state_digest()
    };

    assert_eq!(digest(PilotMistakes::Off), digest(PilotMistakes::Frequent));
}

// Stress: whole shifts on Ground with pilots making mistakes, played by a
// bot that watches the taxi routes and one that doesn't.

#[derive(Debug, Clone, Copy, PartialEq)]
enum GroundBot {
    /// Sees an aircraft off its route as soon as it turns, and re-routes it.
    Careful,
    /// Only acts on what it's asked for.
    Sloppy,
}

#[derive(Debug, Default)]
struct GroundStats {
    taxied: usize,
    caught: usize,
    missed: usize,
    deviations: usize,
    exited: usize,
    /// Deadlocks and conflicts on the surface.
    trouble: usize,
    incursions: usize,
}

/// Where Ground sends this aircraft: its hold line or its parking spot.
fn ground_goal(world: &World, ac: &Aircraft) -> Option<usize> {
    let map = world.airport.surface.as_ref()?;
    match ac.flow {
        Flow::Departure => map.departure_hold(ac.departure_end?),
        Flow::Arrival => Some(map.parking[ac.parking?].node),
    }
}

fn play_ground_shift(airport: &Airport, config: WorldConfig, bot: GroundBot) -> GroundStats {
    use std::collections::HashMap;
    let mut world = World::new(airport.clone(), WorldConfig { staffing: Staffing::GROUND, ..config });
    let mut stats = GroundStats::default();
    let mut last_sent: HashMap<u32, f32> = HashMap::new();
    while world.ended.is_none() {
        world.tick();
        for event in world.drain_events() {
            match event {
                SimEvent::SlipCaught { .. } => stats.caught += 1,
                SimEvent::SlipMissed { major: false, .. } => stats.missed += 1,
                SimEvent::PilotDeviation { .. } => stats.deviations += 1,
                SimEvent::GroundConflict { .. } | SimEvent::TaxiDeadlock { .. } | SimEvent::NearCollision { .. } => {
                    stats.trouble += 1
                }
                SimEvent::RunwayIncursion { .. } => stats.incursions += 1,
                _ => {}
            }
        }
        let now = world.time;
        let ids: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
        for id in ids {
            let Some(ac) = world.find(id) else { continue };
            if ac.frequency() != Position::Ground || !world.is_players(ac) {
                continue;
            }
            let Some(goal) = ground_goal(&world, ac) else { continue };
            let taxi = ac.taxi.as_ref();
            let off_route = matches!(
                ac.slip.as_ref().map(|s| &s.kind),
                Some(SlipKind::WrongTurn { turned: true, .. })
            );
            let waited = now - last_sent.get(&id).copied().unwrap_or(f32::MIN) >= 10.0;
            if !(waited || (off_route && bot == GroundBot::Careful && !world_call_pending(&world, id))) {
                continue;
            }
            let clearance = if taxi.is_some_and(|t| t.holding_short.is_some()) || world.at_departure_hold(ac) {
                Some(Clearance::ContactTower)
            } else if bot == GroundBot::Careful && off_route {
                Some(Clearance::Taxi { to: goal, via: Vec::new() })
            } else if world.awaiting(ac) == Some(Awaiting::ReadyToTaxi) {
                stats.taxied += 1;
                Some(Clearance::Taxi { to: goal, via: Vec::new() })
            } else {
                None
            };
            if let Some(clearance) = clearance {
                world.transmit(id, vec![clearance]);
                last_sent.insert(id, now);
            }
        }
    }
    stats.exited = world.score.exited as usize;
    stats
}

/// Whether a call to this aircraft is still waiting for the pilot: the
/// log's last line to it is from the controller.
fn world_call_pending(world: &World, id: u32) -> bool {
    let Some(ac) = world.find(id) else { return false };
    world.log.iter().rev().find(|l| l.text.contains(&ac.callsign)).is_some_and(|l| l.speaker == Speaker::Controller)
}

fn ground_stress(shifts: impl Iterator<Item = (Airport, WorldConfig)>) {
    let mut totals = [GroundStats::default(), GroundStats::default()];
    let mut clean_trouble = 0;
    let mut failures = Vec::new();
    for (airport, config) in shifts {
        let what = format!("{} seed {}", airport.code, config.seed);
        // The same bot with nobody making mistakes: trouble it has anyway
        // isn't the mistakes' doing.
        let clean = play_ground_shift(&airport, WorldConfig { mistakes: PilotMistakes::Off, ..config }, GroundBot::Careful);
        clean_trouble += clean.trouble;
        for (i, bot) in [GroundBot::Careful, GroundBot::Sloppy].into_iter().enumerate() {
            let s = play_ground_shift(&airport, config, bot);
            let mut problems = Vec::new();
            // Wrong turns stop short of the runways, so never make an incursion.
            if s.incursions > clean.incursions {
                problems.push(format!("{} incursions, {} with no mistakes", s.incursions, clean.incursions));
            }
            if bot == GroundBot::Careful && s.missed > 0 {
                problems.push(format!("{} wrong turns missed by the careful bot", s.missed));
            }
            if s.exited + 3 < clean.exited * 3 / 4 {
                problems.push(format!("{} departures out, {} with no mistakes", s.exited, clean.exited));
            }
            if !problems.is_empty() {
                failures.push(format!("{what} {bot:?}: {problems:?}"));
            }
            let t = &mut totals[i];
            t.taxied += s.taxied;
            t.caught += s.caught;
            t.missed += s.missed;
            t.deviations += s.deviations;
            t.exited += s.exited;
            t.trouble += s.trouble;
            t.incursions += s.incursions;
        }
    }
    println!("careful: {:?}", totals[0]);
    println!("sloppy:  {:?}", totals[1]);
    println!("no mistakes: {clean_trouble} deadlocks or conflicts");
    // The bot never untangles a deadlock, so one can snowball; any run of a
    // seed can differ. Taken together, mistakes mustn't make it much worse.
    for t in &totals {
        assert!(t.trouble <= clean_trouble * 3 / 2 + 5, "{} deadlocks or conflicts, {clean_trouble} with no mistakes", t.trouble);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(totals[0].caught > 0, "{:?}", totals[0]);
    assert!(totals[1].missed > 0, "{:?}", totals[1]);
}

fn ground_stress_config(seed: u64) -> WorldConfig {
    use atcgame::sim::traffic::Busyness;
    WorldConfig {
        seed,
        wind: (!seed.is_multiple_of(3)).then_some(Wind { from: if seed.is_multiple_of(2) { 270.0 } else { 90.0 }, speed: 8.0 }),
        busyness: [Busyness::RealWorld, Busyness::Busy][seed as usize % 2],
        mistakes: PilotMistakes::Frequent,
        shift_seconds: 45.0 * 60.0,
        ..Default::default()
    }
}

fn ksus() -> Airport {
    use atcgame::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

#[test]
fn ground_shifts_with_pilot_mistakes_hold_up() {
    let shifts = (0..3u64)
        .map(|seed| (Airport::default_airport(), ground_stress_config(seed)))
        .chain((0..3u64).map(|seed| (ksus(), ground_stress_config(seed))));
    ground_stress(shifts);
}

/// The long run: `cargo test --release -- --ignored ground_stress`.
#[test]
#[ignore]
fn ground_stress_one_hundred_shifts() {
    let shifts = (0..50u64)
        .map(|seed| (Airport::default_airport(), ground_stress_config(seed)))
        .chain((0..50u64).map(|seed| (ksus(), ground_stress_config(seed))));
    ground_stress(shifts);
}
