//! Landline coordination between a position the player works and an
//! automated one: runway crossings between Ground and Local, and arrivals
//! Local leaves holding short between runways, and departure releases
//! between Local and Approach, and go-arounds Tower tells Approach about.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::coordination::{ANSWER_S, APPROVAL_S, CALL_FOR_RELEASE_MIN_S, CoordState, GO_AROUND_CALL_S, RELEASE_VOID_S};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::replay::Action;
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

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn physical(world: &World, id: &str) -> usize {
    world.airport.surface.as_ref().unwrap().runway_of_end[end(world, id)]
}

/// The cargo ramp, south of 27L: the way north from it crosses 27L.
fn ramp(world: &World) -> usize {
    world.airport.surface.as_ref().unwrap().nearest_node(Vec2::new(0.3, -0.85))
}

fn hold_27r(world: &World) -> usize {
    world.airport.surface.as_ref().unwrap().departure_hold(end(world, "27R")).unwrap()
}

fn final_arrival(world: &World, id: u32, name: &str, miles: f32) -> Aircraft {
    let rwy = end(world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, name, Flow::Arrival, aircraft_type("B738").unwrap(), pos, runway.heading, altitude, 160.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

/// A departure at the cargo ramp, on Ground, bound for 27R across 27L.
fn cargo_departure(world: &World, id: u32) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let node = ramp(world);
    let mut ac = Aircraft::new(id, "TRL300", Flow::Departure, aircraft_type("B738").unwrap(), map.nodes[node].pos, 0.0, world.airport.elevation, 0.0);
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
    panic!("condition not met within {max_seconds} s; events: {events:?}; log: {:?}", landline(world));
}

fn landline(world: &World) -> Vec<&str> {
    world.log.iter().filter(|l| l.speaker == Speaker::Landline).map(|l| l.text.as_str()).collect()
}

fn holding_short(world: &World, id: u32, runway: usize) -> bool {
    world.find(id).and_then(|a| a.taxi.as_ref()).is_some_and(|t| t.holding_short == Some(runway))
}

/// On Ground with Tower automated: TRL300 taxied from the ramp and stopped short of 27L.
fn ground_with_a_crossing() -> (World, Vec<SimEvent>, usize) {
    let mut world = harrowgate(Staffing::GROUND);
    let mut events = Vec::new();
    world.add_aircraft(cargo_departure(&world, 1));
    let hold = hold_27r(&world);
    assert!(world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]));
    let south = physical(&world, "27L");
    run_until(&mut world, 300.0, &mut events, |w| holding_short(w, 1, south));
    (world, events, south)
}

#[test]
fn ground_asks_tower_then_crosses() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    assert!(world.needs_crossing_approval(1, south));
    assert!(world.request_crossing(1, south));
    assert!(!world.request_crossing(1, south), "asked once");
    assert!(landline(&world).last().unwrap().starts_with("Ground to Tower: request crossing runway 27L for TRL300"));
    run_until(&mut world, 20.0, &mut events, |w| w.crossing_approved(1, south));
    assert!(landline(&world).last().unwrap().contains("TRL300 cross runway 27L, approved"));

    assert!(world.transmit(1, vec![Clearance::CrossRunway(south)]));
    let hold = hold_27r(&world);
    run_until(&mut world, 600.0, &mut events, |w| w.find(1).and_then(|a| a.taxi.as_ref()).is_some_and(|t| t.done() && t.node == hold));
    assert!(world.coordination.is_empty(), "done once across: {:?}", world.coordination);
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn tower_says_unable_with_traffic_close_in_and_calls_back() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    world.add_aircraft(final_arrival(&world, 2, "NVA101", 3.0));
    world.request_crossing(1, south);
    run(&mut world, 10.0, &mut events);
    let refusal = landline(&world).last().unwrap().to_string();
    assert!(refusal.contains("hold short runway 27L, traffic 3 mile final. I'll call you"), "{refusal}");
    assert!(!world.crossing_approved(1, south));
    // Pressing for it again doesn't help: Tower will call.
    assert!(!world.request_crossing(1, south));
    run_until(&mut world, 300.0, &mut events, |w| w.crossing_approved(1, south));
    assert!(world.find(2).is_none_or(|a| !world.occupies(a, south)), "after the arrival is off");
}

#[test]
fn crossing_without_tower_s_approval_is_scored() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    assert!(world.transmit(1, vec![Clearance::CrossRunway(south)]));
    run(&mut world, 10.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::UncoordinatedCrossing { callsign } if callsign == "TRL300")), "{events:?}");
    assert!(world.find(1).unwrap().taxi.as_ref().unwrap().cleared.contains(&south), "it still crosses");
}

#[test]
fn an_unused_approval_lapses() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    world.request_crossing(1, south);
    run_until(&mut world, 20.0, &mut events, |w| w.crossing_approved(1, south));
    run(&mut world, APPROVAL_S + 1.0, &mut events);
    assert!(!world.crossing_approved(1, south), "{:?}", world.coordination);
    assert!(world.coordination.is_empty());
    // Ground asks again.
    assert!(world.request_crossing(1, south));
    assert!(events.iter().all(|e| !matches!(e, SimEvent::MissedHandoff { .. })), "{events:?}");
    assert!(landline(&world).iter().any(|l| l.contains("cancel crossing runway 27L for TRL300.")));
}

#[test]
fn an_approved_crossing_holds_the_runway_until_it_is_used() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    world.request_crossing(1, south);
    run_until(&mut world, 20.0, &mut events, |w| w.crossing_approved(1, south));
    assert!(world.coordination.iter().any(|c| c.live()));
    // Once the crossing goes out, the promise stands until it's across.
    world.transmit(1, vec![Clearance::CrossRunway(south)]);
    run(&mut world, 5.0, &mut events);
    assert_eq!(world.coordination[0].state, CoordState::InUse);
    assert!(world.crossing_approved(1, south));
}

#[test]
fn ground_asks_the_player_on_local_and_waits() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(cargo_departure(&world, 1));
    let south = physical(&world, "27L");
    run_until(&mut world, 400.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationRequest { .. })));
    assert_eq!(world.awaiting(world.find(1).unwrap()), Some(Awaiting::Coordination));
    // It stays on Ground's frequency, holding short.
    assert_eq!(world.find(1).unwrap().frequency(), Position::Ground);
    run(&mut world, 20.0, &mut events);
    assert!(holding_short(&world, 1, south), "no crossing without the player");

    assert!(world.answer_coordination(1, true));
    run_until(&mut world, 30.0, &mut events, |w| w.find(1).unwrap().taxi.as_ref().unwrap().cleared.contains(&south));
    assert_eq!(world.find(1).unwrap().frequency(), Position::Ground, "Ground crossed it");
    assert!(world.log.iter().any(|l| l.text.starts_with("Ground: TRL300") && l.text.contains("cross runway 27L")));
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn ground_asks_again_after_unable_and_scores_a_late_answer() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(cargo_departure(&world, 1));
    run_until(&mut world, 400.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    assert!(world.answer_coordination(1, false));
    assert!(landline(&world).last().unwrap().contains("hold short runway 27L, unable"));
    run_until(&mut world, 70.0, &mut events, |w| w.coordination_for(1).is_some_and(|c| c.state == CoordState::Pending));
    assert!(landline(&world).last().unwrap().contains("still holding short runway 27L"));
    run(&mut world, ANSWER_S + 1.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationLate { .. })), "{events:?}");
}

#[test]
fn combined_positions_need_no_landline() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND);
    let mut events = Vec::new();
    world.add_aircraft(cargo_departure(&world, 1));
    let hold = hold_27r(&world);
    world.transmit(1, vec![Clearance::Taxi { to: hold, via: vec![] }]);
    let south = physical(&world, "27L");
    run_until(&mut world, 300.0, &mut events, |w| holding_short(w, 1, south));
    assert!(!world.needs_crossing_approval(1, south));
    assert!(!world.request_crossing(1, south));
    world.transmit(1, vec![Clearance::CrossRunway(south)]);
    run(&mut world, 10.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UncoordinatedCrossing { .. })));
    assert!(landline(&world).is_empty());
}

#[test]
fn tower_leaves_an_arrival_holding_short_and_tells_ground() {
    let mut world = harrowgate(Staffing::GROUND);
    let mut events = Vec::new();
    let south = physical(&world, "27L");
    let map = world.airport.surface.as_ref().unwrap().clone();
    let from = ramp(&world);
    // A gate whose way from the ramp crosses 27L.
    let gate = map
        .parking
        .iter()
        .map(|p| p.node)
        .find(|&n| map.route(from, n, &[]).is_some_and(|r| r.runways_entered.contains(&south)))
        .expect("a gate across 27L");
    let route = map.route(from, gate, &[]).unwrap();
    // Stopped at the last node before 27L.
    let k = route.nodes.iter().position(|&n| map.in_zone(n, south)).unwrap() - 1;
    let at = route.nodes[k];
    let mut ac = cargo_departure(&world, 1);
    ac.pos = map.nodes[at].pos;
    ac.flow = Flow::Arrival;
    ac.departure_end = None;
    ac.phase = Phase::Taxiing;
    ac.surface_freq = Position::Local;
    ac.taxi = Some(Taxi { path: route.nodes, index: k + 1, node: at, holding_short: Some(south), ..Default::default() });
    world.add_aircraft(ac);
    world.add_aircraft(final_arrival(&world, 2, "NVA101", 3.0));
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().frequency() == Position::Ground);
    assert!(landline(&world).iter().any(|l| l.starts_with("Tower to Ground: TRL300, holding short runway 27L. I'll call you")), "{:?}", landline(&world));
    assert!(world.log.iter().any(|l| l.text.contains("Ground, TRL300, holding short runway 27L")));
    assert!(!world.request_crossing(1, south), "Tower will call");
    run_until(&mut world, 300.0, &mut events, |w| w.crossing_approved(1, south));
    world.transmit(1, vec![Clearance::CrossRunway(south)]);
    run(&mut world, 10.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UncoordinatedCrossing { .. })), "{events:?}");
}

#[test]
fn landline_actions_replay() {
    let (mut world, mut events, south) = ground_with_a_crossing();
    world.request_crossing(1, south);
    run_until(&mut world, 20.0, &mut events, |w| w.crossing_approved(1, south));
    world.transmit(1, vec![Clearance::CrossRunway(south)]);
    run(&mut world, 60.0, &mut events);
    let commands = world.commands().to_vec();
    assert!(commands.iter().any(|c| c.action == Action::RequestCrossing(south)));

    let mut again = harrowgate(Staffing::GROUND);
    again.add_aircraft(cargo_departure(&again, 1));
    again.run_to(world.ticks(), &commands);
    assert_eq!(again.state_digest(), world.state_digest());
}

#[test]
fn runways_are_named_by_the_end_in_use() {
    use atcgame::sim::clearance::spoken_runway;
    for (wind, south, north) in [(270.0, "27L", "27R"), (90.0, "09R", "09L")] {
        let config = WorldConfig {
            seed: 7,
            traffic: false,
            wind: Some(Wind { from: wind, speed: 10.0 }),
            ..Default::default()
        };
        let world = World::new(Airport::default_airport(), config);
        let map = world.airport.surface.as_ref().unwrap();
        let in_use = world.ends_in_use();
        let name = |id| spoken_runway(&world.airport, map, physical(&world, id), &in_use);
        assert_eq!(name("27L"), south, "wind {wind}");
        assert_eq!(name("27R"), north, "wind {wind}");
    }
}

/// An IFR departure at the 27R hold line, with Tower.
fn ready_departure(world: &World, id: u32) -> Aircraft {
    let hold = hold_27r(world);
    let mut ac = cargo_departure(world, id);
    ac.callsign = "DAL20".into();
    ac.pos = world.airport.surface.as_ref().unwrap().nodes[hold].pos;
    ac.phase = Phase::Taxiing;
    ac.taxi = Some(Taxi::at(hold));
    ac.surface_freq = Position::Local;
    ac
}

/// An IFR arrival going around off 27L, low over the field.
fn going_around(world: &World, id: u32) -> Aircraft {
    let mut ac = final_arrival(world, id, "NVA7", 0.5);
    ac.phase = Phase::GoAround;
    ac.on_glideslope = false;
    ac.lateral = Lateral::Heading(270.0);
    ac.target_altitude = world.airport.go_around_altitude;
    ac.altitude = world.airport.elevation + 400.0;
    ac
}

fn set_phase(world: &mut World, id: u32, phase: Phase) {
    world.aircraft.iter_mut().find(|a| a.id == id).unwrap().phase = phase;
}

fn rolling(world: &World, id: u32) -> bool {
    world.find(id).is_none_or(|a| a.phase == Phase::TowerRelease || !a.on_ground)
}

/// On Local with Approach automated: a go-around puts departures on call for release.
fn local_on_call_for_release() -> (World, Vec<SimEvent>, usize) {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    world.add_aircraft(going_around(&world, 2));
    run(&mut world, 1.0, &mut events);
    let end = end(&world, "27R");
    (world, events, end)
}

#[test]
fn approach_puts_departures_on_call_for_release_and_releases_them() {
    let (mut world, mut events, end) = local_on_call_for_release();
    assert!(world.call_for_release());
    assert!(
        landline(&world).contains(&"Approach to Tower: departures off 27R call for release, missed approach in progress."),
        "{:?}",
        landline(&world)
    );
    assert!(world.needs_release(1));
    assert!(world.request_release(1));
    assert!(!world.request_release(1), "asked once");
    assert!(landline(&world).last().unwrap().ends_with("Tower to Approach: request release DAL20, runway 27R."));
    // Still sorting out the missed approach: hold.
    run(&mut world, 8.0, &mut events);
    let last = landline(&world).last().unwrap().to_string();
    assert!(last.contains("DAL20 hold for release, missed approach in progress. I'll call you"), "{last}");
    // The go-around is with Approach now: the release comes.
    set_phase(&mut world, 2, Phase::Controlled);
    run_until(&mut world, 10.0, &mut events, |w| w.release_approved(1));
    assert!(landline(&world).last().unwrap().contains("DAL20 released, void if not off by"));
    assert!(!world.needs_release(1));

    assert!(world.transmit(1, vec![Clearance::ClearedForTakeoff(end)]));
    run_until(&mut world, 60.0, &mut events, |w| rolling(w, 1));
    run(&mut world, 1.0, &mut events);
    assert!(world.coordination_for(1).is_none(), "done once rolling");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UnreleasedDeparture { .. })), "{events:?}");

    // Departures go back to normal a while after the reason has gone.
    run(&mut world, CALL_FOR_RELEASE_MIN_S, &mut events);
    assert!(!world.call_for_release());
    assert!(landline(&world).last().unwrap().contains("departures released, resume normal"));
}

#[test]
fn a_takeoff_without_a_release_is_scored() {
    let (mut world, mut events, end) = local_on_call_for_release();
    assert!(world.transmit(1, vec![Clearance::ClearedForTakeoff(end)]));
    run(&mut world, 10.0, &mut events);
    assert!(
        events.iter().any(|e| matches!(e, SimEvent::UnreleasedDeparture { callsign } if callsign == "DAL20")),
        "{events:?}"
    );
}

#[test]
fn an_unused_release_is_void() {
    let (mut world, mut events, _) = local_on_call_for_release();
    set_phase(&mut world, 2, Phase::Controlled);
    world.request_release(1);
    run_until(&mut world, 10.0, &mut events, |w| w.release_approved(1));
    run(&mut world, RELEASE_VOID_S + 1.0, &mut events);
    assert!(world.call_for_release());
    assert!(!world.release_approved(1));
    assert!(landline(&world).last().unwrap().contains("DAL20 release void"), "{:?}", landline(&world));
    assert!(world.needs_release(1));
}

#[test]
fn departures_go_automatically_without_a_reason() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    run(&mut world, 5.0, &mut events);
    assert!(!world.call_for_release());
    assert!(!world.needs_release(1));
    assert!(!world.request_release(1));
    assert!(landline(&world).is_empty());
}

#[test]
fn tower_asks_the_player_on_approach_for_releases() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    assert!(world.set_call_for_release(true));
    assert!(landline(&world).last().unwrap().ends_with("Approach to Tower: departures off 27R call for release."));
    run_until(&mut world, 10.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationRequest { .. })));
    run(&mut world, 30.0, &mut events);
    assert!(!rolling(&world, 1), "Tower holds it for release");

    assert!(world.answer_coordination(1, true));
    assert!(landline(&world).last().unwrap().contains("DAL20 released."));
    run_until(&mut world, 60.0, &mut events, |w| rolling(w, 1));
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);

    assert!(world.set_call_for_release(false));
    assert!(!world.set_call_for_release(false), "already released");
    run(&mut world, 1.0, &mut events);
    let commands = world.commands().to_vec();
    assert!(commands.iter().any(|c| c.action == Action::CallForRelease { on: true }));

    let mut again = harrowgate(Staffing::APPROACH);
    again.add_aircraft(ready_departure(&again, 1));
    again.run_to(world.ticks(), &commands);
    assert_eq!(again.state_digest(), world.state_digest());
}

#[test]
fn tower_asks_again_when_approach_holds_a_departure() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    world.set_call_for_release(true);
    run_until(&mut world, 10.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    assert!(world.answer_coordination(1, false));
    assert!(landline(&world).last().unwrap().contains("DAL20 hold for release, unable"));
    run_until(&mut world, 70.0, &mut events, |w| w.coordination_for(1).is_some_and(|c| c.state == CoordState::Pending));
    assert!(landline(&world).last().unwrap().contains("DAL20 still waiting for release"));
    // Letting departures go drops the request: off it goes.
    world.set_call_for_release(false);
    run_until(&mut world, 60.0, &mut events, |w| rolling(w, 1));
}

#[test]
fn releases_need_no_landline_on_a_combined_position() {
    let mut world = harrowgate(Staffing::ALL);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    world.add_aircraft(going_around(&world, 2));
    run(&mut world, 5.0, &mut events);
    assert!(!world.call_for_release());
    assert!(!world.set_call_for_release(true));
    assert!(!world.needs_release(1));
}

/// Inside a mile on final with no landing clearance: it goes around at once.
fn uncleared_short_final(world: &World, id: u32) -> Aircraft {
    final_arrival(world, id, "NVA101", 0.9)
}

fn went_around_with_local() -> (World, Vec<SimEvent>) {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(uncleared_short_final(&world, 1));
    run_until(&mut world, 5.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    (world, events)
}

#[test]
fn local_calls_approach_about_a_go_around() {
    let (mut world, mut events) = went_around_with_local();
    assert!(world.owes_go_around_call(1));
    assert_eq!(world.awaiting(world.find(1).unwrap()), Some(Awaiting::Coordination));
    assert!(world.call_go_around(1));
    assert!(!world.call_go_around(1), "called once");
    assert_eq!(*landline(&world).last().unwrap(), "Tower to Approach: go-around NVA101, runway 27L, runway heading, 3,000.");
    run_until(&mut world, 10.0, &mut events, |w| landline(w).last().unwrap().contains("NVA101, approved"));
    run_until(&mut world, 180.0, &mut events, |w| w.find(1).is_none_or(|a| a.frequency() == Position::Approach));
    run(&mut world, 1.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UncoordinatedGoAround { .. })), "{events:?}");
    assert!(world.coordination.is_empty(), "{:?}", world.coordination);

    let commands = world.commands().to_vec();
    assert!(commands.iter().any(|c| c.action == Action::CallGoAround));
    let mut again = harrowgate(Staffing::LOCAL);
    again.add_aircraft(uncleared_short_final(&again, 1));
    again.run_to(world.ticks(), &commands);
    assert_eq!(again.state_digest(), world.state_digest());
}

#[test]
fn a_go_around_not_called_within_a_minute_is_scored() {
    let (mut world, mut events) = went_around_with_local();
    run(&mut world, GO_AROUND_CALL_S - 2.0, &mut events);
    assert!(world.owes_go_around_call(1), "still time, even with Approach");
    run(&mut world, 3.0, &mut events);
    assert!(!world.owes_go_around_call(1));
    assert!(
        events.iter().any(|e| matches!(e, SimEvent::UncoordinatedGoAround { callsign } if callsign == "NVA101")),
        "{events:?}"
    );
}

#[test]
fn a_go_around_local_orders_is_called_through_at_once() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, "NVA101", 4.0));
    assert!(world.transmit(1, vec![Clearance::GoAround]));
    run_until(&mut world, 10.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    assert!(!world.owes_go_around_call(1));
    assert!(landline(&world).iter().any(|l| l.starts_with("Tower to Approach: go-around NVA101")), "{:?}", landline(&world));
    run_until(&mut world, 180.0, &mut events, |w| w.find(1).is_none_or(|a| a.frequency() == Position::Approach));
    run(&mut world, 1.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UncoordinatedGoAround { .. })), "{events:?}");
}

#[test]
fn tower_tells_the_player_on_approach_about_a_go_around() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(uncleared_short_final(&world, 1));
    run_until(&mut world, 5.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    assert!(landline(&world).last().unwrap().starts_with("Tower to Approach: go-around NVA101, runway 27L"));
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationRequest { .. })));
    assert!(!world.answer_coordination(1, false), "nothing to refuse");
    assert!(world.answer_coordination(1, true));
    assert_eq!(*landline(&world).last().unwrap(), "Approach to Tower: NVA101, approved.");
    assert!(world.coordination.is_empty());
}

#[test]
fn an_unanswered_go_around_call_is_scored() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    world.add_aircraft(uncleared_short_final(&world, 1));
    run_until(&mut world, 5.0, &mut events, |w| w.incoming().any(|c| c.aircraft == 1));
    run(&mut world, ANSWER_S + 1.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationLate { .. })), "{events:?}");
}

#[test]
fn go_arounds_need_no_landline_on_a_combined_position() {
    let mut world = harrowgate(Staffing::ALL);
    let mut events = Vec::new();
    world.add_aircraft(uncleared_short_final(&world, 1));
    run_until(&mut world, 5.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    assert!(!world.owes_go_around_call(1));
    assert!(world.coordination.is_empty());
}
