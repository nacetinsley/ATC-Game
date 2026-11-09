//! Landline coordination between two players in co-op: what one player asks,
//! the other answers, as an automated controller does for a single player.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::coordination::{ANSWER_S, CoordState};
use atcgame::sim::emergency::{Emergency, EmergencyKind, Trigger};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Seat, Staffing};
use atcgame::sim::replay::Action;
use atcgame::sim::scoring::SimEvent;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{Speaker, TICK, Talker, World, WorldConfig};
use glam::Vec2;

const A: Seat = 0;
const B: Seat = 1;

/// Harrowgate with the wind from the west: land 27L (south), depart 27R
/// (north). Approach in seat A; `tower` are the tower positions in seat B.
fn harrowgate(tower: &[Position]) -> World {
    let staffing = tower.iter().fold(Staffing::NONE.with_seat(Position::Approach, A), |s, &p| s.with_seat(p, B));
    world_with(staffing)
}

fn world_with(staffing: Staffing) -> World {
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

fn hold_27r(world: &World) -> usize {
    world.airport.surface.as_ref().unwrap().departure_hold(end(world, "27R")).unwrap()
}

/// A departure at the cargo ramp, south of 27L, on Ground: bound for 27R
/// across 27L.
fn cargo_departure(world: &World, id: u32) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let node = map.nearest_node(Vec2::new(0.3, -0.85));
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

/// An arrival on final to 27L, with Tower.
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

/// Ground in seat A, Local in seat B: TRL300 taxied from the ramp and
/// stopped short of 27L.
fn ground_and_local_apart() -> (World, Vec<SimEvent>, usize) {
    let staffing = Staffing::NONE.with_seat(Position::Ground, A).with_seat(Position::Local, B);
    let mut world = world_with(staffing);
    let mut events = Vec::new();
    world.add_aircraft(cargo_departure(&world, 1));
    let hold = hold_27r(&world);
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::Taxi { to: hold, via: vec![] }])));
    let south = physical(&world, "27L");
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(1).and_then(|a| a.taxi.as_ref()).is_some_and(|t| t.holding_short == Some(south))
    });
    (world, events, south)
}

#[test]
fn ground_asks_the_other_player_on_local_to_cross() {
    let (mut world, mut events, south) = ground_and_local_apart();
    assert!(world.needs_crossing_approval(1, south));
    assert!(world.act(A, 1, Action::RequestCrossing(south)));
    // It waits for the player on Local: no automated answer.
    run(&mut world, 20.0, &mut events);
    assert!(!world.crossing_approved(1, south));
    assert!(world.incoming_for(B).any(|c| c.aircraft == 1), "Local's to answer");
    assert!(world.incoming_for(A).next().is_none(), "not Ground's");
    assert_eq!(world.awaiting_for(B, world.find(1).unwrap()), Some(Awaiting::Coordination));
    // Both hear the call; the request names who made it.
    let call = world.log.iter().rev().find(|l| l.speaker == Speaker::Landline).unwrap();
    let radio = call.radio.as_ref().unwrap();
    assert_eq!(radio.talker, Talker::Controller(A));
    assert!(call.shown_to(A) && call.shown_to(B));
    // Only Local's player can answer.
    assert!(!world.act(A, 1, Action::Coordinate { approve: true }));
    assert!(world.act(B, 1, Action::Coordinate { approve: true }));
    assert!(world.crossing_approved(1, south));
    assert!(landline(&world).last().unwrap().starts_with("Tower to Ground: TRL300 cross runway 27L, approved"));
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::CrossRunway(south)])));
    run(&mut world, 120.0, &mut events);
    assert!(
        !events.iter().any(|e| matches!(e, SimEvent::UncoordinatedCrossing { .. })),
        "approved, so coordinated: {events:?}"
    );
}

#[test]
fn a_request_the_other_player_leaves_is_scored_late() {
    let (mut world, mut events, south) = ground_and_local_apart();
    assert!(world.act(A, 1, Action::RequestCrossing(south)));
    run(&mut world, ANSWER_S + 2.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::CoordinationLate { .. })), "{events:?}");
}

#[test]
fn turned_down_ground_can_ask_again() {
    let (mut world, mut events, south) = ground_and_local_apart();
    assert!(world.act(A, 1, Action::RequestCrossing(south)));
    assert!(world.act(B, 1, Action::Coordinate { approve: false }));
    assert!(landline(&world).last().unwrap().contains("hold short runway 27L, unable"));
    run(&mut world, 5.0, &mut events);
    // Nobody automated asks again for them: the player does.
    assert!(world.act(A, 1, Action::RequestCrossing(south)));
    assert_eq!(world.coordination_for(1).unwrap().state, CoordState::Pending);
}

#[test]
fn crossing_without_asking_the_other_player_is_scored() {
    let (mut world, mut events, south) = ground_and_local_apart();
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::CrossRunway(south)])));
    run(&mut world, 10.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::UncoordinatedCrossing { .. })), "{events:?}");
}

#[test]
fn approach_puts_the_other_players_departures_on_call_for_release() {
    let mut world = harrowgate(&[Position::Local, Position::Ground]);
    let mut events = Vec::new();
    world.add_aircraft(ready_departure(&world, 1));
    run(&mut world, 1.0, &mut events);
    assert!(world.releases_coordinated());
    assert!(!world.call_for_release(), "nobody automated puts them on call");
    assert!(!world.act(B, 0, Action::CallForRelease { on: true }), "Approach's call");
    assert!(world.act(A, 0, Action::CallForRelease { on: true }));
    assert!(world.needs_release(1));
    assert!(world.act(B, 1, Action::RequestRelease));
    run(&mut world, 10.0, &mut events);
    assert!(!world.release_approved(1), "waits for the player on Approach");
    assert!(world.incoming_for(A).any(|c| c.aircraft == 1));
    assert!(world.act(A, 1, Action::Coordinate { approve: true }));
    assert!(world.release_approved(1));
    assert!(landline(&world).last().unwrap().contains("DAL20 released"));
    assert!(world.act(A, 0, Action::CallForRelease { on: false }));
    assert!(!world.call_for_release());
}

#[test]
fn tower_tells_the_player_on_approach_about_a_go_around() {
    let mut world = harrowgate(&[Position::Local, Position::Ground]);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, 0.9));
    run_until(&mut world, 5.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);
    assert!(world.owes_go_around_call_for(B, 1));
    assert!(!world.owes_go_around_call_for(A, 1));
    assert!(world.act(B, 1, Action::CallGoAround));
    assert!(landline(&world).last().unwrap().starts_with("Tower to Approach: go-around NVA101"));
    run(&mut world, 10.0, &mut events);
    assert!(world.incoming_for(A).any(|c| c.aircraft == 1), "Approach acknowledges it");
    assert_eq!(world.awaiting_for(A, world.find(1).unwrap()), Some(Awaiting::Coordination));
    assert!(!world.act(A, 1, Action::Coordinate { approve: false }), "nothing to refuse");
    assert!(world.act(A, 1, Action::Coordinate { approve: true }));
    assert!(landline(&world).last().unwrap().starts_with("Approach to Tower: NVA101, approved"));
}

#[test]
fn an_emergency_is_called_ahead_to_the_other_player() {
    let mut world = harrowgate(&[Position::Local, Position::Ground]);
    let mut events = Vec::new();
    let rwy = end(&world, "27L");
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * 12.0;
    let altitude = world.airport.glideslope_altitude(rwy, 12.0);
    let mut ac = Aircraft::new(1, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), pos, runway.heading, altitude, 180.0);
    ac.phase = Phase::Localizer;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.emergency = Some(Emergency::new(EmergencyKind::Medical, Trigger::OnCheckIn, 120, 90));
    world.add_aircraft(ac);
    run(&mut world, 1.0, &mut events);
    assert_eq!(world.owed_emergency_call_for(A, 1), Some(Position::Local));
    assert_eq!(world.owed_emergency_call_for(B, 1), None);
    assert!(!world.act(B, 1, Action::CallEmergency), "Approach's call to make");
    assert!(world.act(A, 1, Action::CallEmergency));
    run(&mut world, 10.0, &mut events);
    assert!(world.incoming_for(B).any(|c| c.aircraft == 1), "Tower acknowledges it");
    assert!(world.act(B, 1, Action::Coordinate { approve: true }));
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::HandoffTower])));
    run(&mut world, 15.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::UncoordinatedEmergency { .. })), "{events:?}");
}

#[test]
fn approach_handing_off_late_to_the_other_player_is_a_go_around() {
    let mut world = harrowgate(&[Position::Local]);
    let mut events = Vec::new();
    let mut ac = final_arrival(&world, 1, 6.0);
    ac.phase = Phase::Localizer;
    world.add_aircraft(ac);
    run(&mut world, 120.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::LateHandoff { .. })), "{events:?}");
}

#[test]
fn local_hands_a_landed_arrival_to_the_other_player_on_ground() {
    let staffing = Staffing::NONE.with_seat(Position::Local, A).with_seat(Position::Ground, B);
    let mut world = world_with(staffing);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, 3.0));
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::ClearedToLand(end(&world, "27L"))])));
    run_until(&mut world, 300.0, &mut events, |w| w.awaiting_for(A, w.find(1).unwrap()) == Some(Awaiting::HandoffDue));
    assert!(world.act(A, 1, Action::Transmit(vec![Clearance::ContactGround])));
    run_until(&mut world, 20.0, &mut events, |w| w.find(1).unwrap().frequency() == Position::Ground);
    assert!(world.is_seats(B, world.find(1).unwrap()));
    assert!(!events.iter().any(|e| matches!(e, SimEvent::MissedHandoff { .. })), "{events:?}");
}

#[test]
fn both_players_traffic_is_the_players() {
    // The sim runs and scores every player's aircraft alike: separation,
    // pilot mistakes, weather requests.
    let mut world = harrowgate(&[Position::Local]);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, 3.0));
    run(&mut world, 1.0, &mut events);
    let ac = world.find(1).unwrap();
    assert!(world.is_players(ac));
    assert!(world.is_seats(B, ac) && !world.is_seats(A, ac));
}
