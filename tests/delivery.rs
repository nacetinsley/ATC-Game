//! Clearance Delivery: flight plans, IFR clearances, readbacks and the
//! handoff to Ground.

use atcgame::sim::aircraft::{Flow, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::awaiting::Awaiting;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::delivery::{self, Readback};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::{self, SimEvent};
use atcgame::sim::units::Wind;
use atcgame::sim::{TICK, World, WorldConfig};

fn harrowgate(staffing: Staffing, seed: u64) -> World {
    let config = WorldConfig {
        seed,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        staffing,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

/// Queue a departure on the configuration's first SID and let it reach the gate.
fn departure(world: &mut World) -> u32 {
    let config = world.config_name().to_string();
    let sid = world.airport.sids.iter().find(|s| s.config == config).unwrap().fixes.clone();
    let route = world.airport.route_indices(&sid);
    world.request_departure(aircraft_type("B738").unwrap(), route, 13000.0);
    let before: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
    world.tick();
    world
        .aircraft
        .iter()
        .find(|a| a.flow == Flow::Departure && !before.contains(&a.id))
        .expect("placed at a gate")
        .id
}

fn run(world: &mut World, seconds: f32) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..(seconds / TICK).ceil() as usize {
        if world.ended.is_some() {
            break;
        }
        world.tick();
        events.extend(world.drain_events());
    }
    events
}

fn said(world: &World, text: &str) -> bool {
    world.log.iter().any(|l| l.text.contains(text))
}

/// The clearance a sharp controller would give: the filed plan, put right.
fn right_clearance(world: &World, id: u32) -> Clearance {
    let ac = world.find(id).unwrap();
    let plan = ac.flight_plan.as_ref().unwrap();
    let config = world.config_name();
    let mut route = plan.route.clone();
    if !delivery::sid_in_use(&world.airport, config, &route) {
        route = delivery::sid_choices(&world.airport, config, &route)[0].clone();
    }
    let course = delivery::course(&world.airport, &route);
    let altitude = delivery::level_for(plan.altitude, course);
    Clearance::Ifr { route, altitude, squawk: plan.squawk }
}

/// The clearance exactly as filed.
fn as_filed(world: &World, id: u32) -> Clearance {
    let plan = world.find(id).unwrap().flight_plan.clone().unwrap();
    Clearance::Ifr { route: plan.route, altitude: plan.altitude, squawk: plan.squawk }
}

fn readback_error(world: &World, id: u32) -> Option<Readback> {
    world.find(id)?.flight_plan.as_ref()?.readback
}

#[test]
fn a_departure_calls_delivery_and_waits_at_the_gate_for_its_clearance() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let ac = world.find(id).unwrap();
    assert_eq!(ac.phase, Phase::Parked);
    assert_eq!(ac.frequency(), Position::Delivery);
    assert!(said(&world, "ready to copy"), "{:?}", world.log.last());
    assert!(said(&world, "IFR to X"), "names its destination: {:?}", world.log.last());
    assert_eq!(world.awaiting(ac), Some(Awaiting::ClearanceRequest));

    // The automated Ground leaves it alone: it has no clearance.
    run(&mut world, 200.0);
    let ac = world.find(id).unwrap();
    assert_eq!(ac.phase, Phase::Parked);
    assert_eq!(world.awaiting(ac), Some(Awaiting::ClearanceLate));
    assert!(Awaiting::ClearanceLate.urgent());
}

#[test]
fn cleared_and_sent_to_ground_it_taxis_and_scores() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let clearance = right_clearance(&world, id);
    assert!(world.transmit(id, vec![clearance.clone()]));
    run(&mut world, 8.0);
    assert!(said(&world, "cleared to X"), "{:?}", world.log);
    assert!(said(&world, "one zero minutes after departure"));
    let ac = world.find(id).unwrap();
    assert!(ac.flight_plan.as_ref().unwrap().is_cleared());
    if readback_error(&world, id).is_some() {
        world.transmit(id, vec![clearance]);
        run(&mut world, 8.0);
        assert!(readback_error(&world, id).is_none());
    }
    // Read back: now it's due to go to Ground.
    assert_eq!(world.awaiting(world.find(id).unwrap()), Some(Awaiting::HandoffDue));

    world.transmit(id, vec![Clearance::ContactGround]);
    let events = run(&mut world, 60.0);
    assert!(events.iter().any(|e| matches!(e, SimEvent::Cleared { .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::PlanMissed { .. } | SimEvent::ReadbackMissed { .. })));
    assert_eq!(world.score.cleared, 1);
    assert!(world.score.points >= scoring::POINTS_CLEARED);
    let ac = world.find(id).unwrap();
    assert_eq!(ac.phase, Phase::Taxiing, "the automated Ground taxied it");
}

#[test]
fn a_clearance_left_on_delivery_is_a_missed_handoff() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    world.transmit(id, vec![right_clearance(&world, id)]);
    run(&mut world, 10.0);
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().readback = None;
    run(&mut world, 20.0);
    assert_eq!(world.awaiting(world.find(id).unwrap()), Some(Awaiting::HandoffDue));
    let events = run(&mut world, 60.0);
    assert!(events.iter().any(|e| matches!(e, SimEvent::MissedHandoff { to: "Ground", .. })), "{events:?}");
    assert_eq!(world.find(id).unwrap().frequency(), Position::Ground);
}

#[test]
fn ground_wont_taxi_an_aircraft_without_its_clearance() {
    let mut world = harrowgate(Staffing::GROUND_DELIVERY, 7);
    let id = departure(&mut world);
    let end = world.find(id).unwrap().departure_end.unwrap();
    let hold = world.airport.surface.as_ref().unwrap().departure_hold(end).unwrap();
    world.transmit(id, vec![Clearance::Taxi { to: hold, via: Vec::new() }]);
    run(&mut world, 8.0);
    assert!(said(&world, "unable"), "{:?}", world.log.last());
    assert!(said(&world, "IFR clearance"));
    assert_eq!(world.find(id).unwrap().phase, Phase::Parked);
    world.transmit(id, vec![Clearance::ContactGround]);
    run(&mut world, 8.0);
    assert!(said(&world, "we still need our clearance"));
}

#[test]
fn on_ground_and_delivery_combined_the_pilot_calls_for_taxi_by_itself() {
    let mut world = harrowgate(Staffing::GROUND_DELIVERY, 7);
    let id = departure(&mut world);
    world.transmit(id, vec![right_clearance(&world, id)]);
    run(&mut world, 10.0);
    assert_eq!(world.find(id).unwrap().frequency(), Position::Delivery);
    let events = run(&mut world, delivery::READY_TO_TAXI_S);
    assert!(said(&world, "ready to taxi"), "{:?}", world.log.last());
    let ac = world.find(id).unwrap();
    assert_eq!(ac.frequency(), Position::Ground);
    assert_eq!(world.awaiting(ac), Some(Awaiting::ReadyToTaxi));
    assert!(events.iter().any(|e| matches!(e, SimEvent::Cleared { .. })));

    // And taxis when told to.
    let end = ac.departure_end.unwrap();
    let hold = world.airport.surface.as_ref().unwrap().departure_hold(end).unwrap();
    world.transmit(id, vec![Clearance::Taxi { to: hold, via: Vec::new() }]);
    run(&mut world, 10.0);
    assert_eq!(world.find(id).unwrap().phase, Phase::Taxiing);
}

#[test]
fn a_taxi_instruction_straight_after_the_clearance_takes_it_to_ground() {
    let mut world = harrowgate(Staffing::GROUND_DELIVERY, 7);
    let id = departure(&mut world);
    world.transmit(id, vec![right_clearance(&world, id)]);
    run(&mut world, 8.0);
    let end = world.find(id).unwrap().departure_end.unwrap();
    let hold = world.airport.surface.as_ref().unwrap().departure_hold(end).unwrap();
    world.transmit(id, vec![Clearance::Taxi { to: hold, via: Vec::new() }]);
    let events = run(&mut world, 8.0);
    let ac = world.find(id).unwrap();
    assert_eq!(ac.phase, Phase::Taxiing);
    assert_eq!(ac.frequency(), Position::Ground);
    assert!(events.iter().any(|e| matches!(e, SimEvent::Cleared { .. })));
}

/// Make the filed plan's altitude wrong for its direction of flight.
fn spoil_altitude(world: &mut World, id: u32) -> f32 {
    let route = world.find(id).unwrap().flight_plan.as_ref().unwrap().route.clone();
    let course = delivery::course(&world.airport, &route);
    let wrong = delivery::level_for(35000.0, course) + 1000.0;
    assert!(!delivery::direction_ok(wrong, course));
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().altitude = wrong;
    wrong
}

fn clear_and_hand_off(world: &mut World, id: u32, clearance: Clearance) -> Vec<SimEvent> {
    world.transmit(id, vec![clearance]);
    let mut events = run(world, 8.0);
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().readback = None;
    world.transmit(id, vec![Clearance::ContactGround]);
    events.extend(run(world, 8.0));
    events
}

#[test]
fn clearing_a_wrong_altitude_as_filed_is_an_incident() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    spoil_altitude(&mut world, id);
    let filed = as_filed(&world, id);
    let events = clear_and_hand_off(&mut world, id, filed);
    let missed = events.iter().find_map(|e| match e {
        SimEvent::PlanMissed { problem, .. } => Some(problem.clone()),
        _ => None,
    });
    let problem = missed.expect("missed the altitude");
    assert!(problem.contains("bound:"), "{problem}");
    assert!(world.score.incidents.iter().any(|i| i.points == scoring::PENALTY_PLAN_MISSED), "{:?}", world.score.incidents);
}

#[test]
fn amending_a_wrong_altitude_earns_points() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    spoil_altitude(&mut world, id);
    let fixed = right_clearance(&world, id);
    let events = clear_and_hand_off(&mut world, id, fixed);
    assert!(events.iter().any(|e| matches!(e, SimEvent::PlanCaught { .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::PlanMissed { .. })));
    assert!(world.score.incidents.iter().any(|i| i.points == scoring::POINTS_PLAN_CAUGHT));
    assert!(said(&world, "amended") || said(&world, "cleared to"));
}

#[test]
fn a_sid_from_the_other_runways_must_be_amended() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let config = world.config_name().to_string();
    let exit = world.find(id).unwrap().exit_fix;
    let other = world
        .airport
        .sids
        .iter()
        .filter(|s| s.config != config)
        .map(|s| world.airport.route_indices(&s.fixes))
        .find(|r| r.last().copied() == exit && !delivery::sid_in_use(&world.airport, &config, r));
    let Some(other) = other else { panic!("Harrowgate has a SID per exit in each configuration") };
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().route = other.clone();
    assert!(delivery::problem(&world.airport, &config, &other, 35000.0).is_some());

    // S-style amendment: the SID in use from the same exit fix.
    let fixed = right_clearance(&world, id);
    let Clearance::Ifr { route, .. } = fixed.clone() else { unreachable!() };
    assert_eq!(route.last().copied(), exit);
    assert!(delivery::sid_in_use(&world.airport, &config, &route));
    let events = clear_and_hand_off(&mut world, id, fixed);
    assert!(events.iter().any(|e| matches!(e, SimEvent::PlanCaught { .. })), "{events:?}");
    assert_eq!(world.find(id).unwrap().route, route);
}

#[test]
fn a_wrong_readback_put_right_with_a_repeat_costs_nothing() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let clearance = right_clearance(&world, id);
    let Clearance::Ifr { squawk, .. } = clearance else { unreachable!() };
    world.transmit(id, vec![clearance.clone()]);
    run(&mut world, 8.0);
    // The pilot mishears the code.
    let wrong = squawk ^ 0o11;
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().readback = Some(Readback::Squawk(wrong));

    // Pressing I again repeats the altitude and the code.
    let words = atcgame::sim::clearance::phrase(&clearance, world.find(id).unwrap(), &world.airport, &world.ends_in_use());
    assert!(words.starts_with("I say again"), "{words}");
    world.transmit(id, vec![clearance]);
    run(&mut world, 8.0);
    assert!(readback_error(&world, id).is_none());
    world.transmit(id, vec![Clearance::ContactGround]);
    let events = run(&mut world, 8.0);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::ReadbackMissed { .. })), "{events:?}");
}

#[test]
fn a_wrong_readback_left_alone_is_an_incident() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let clearance = right_clearance(&world, id);
    world.transmit(id, vec![clearance]);
    run(&mut world, 8.0);
    let initial = world.find(id).unwrap().flight_plan.as_ref().unwrap().initial;
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().readback = Some(Readback::Altitude(initial + 1000.0));
    world.find_mut(id).unwrap().target_altitude = initial + 1000.0;
    world.transmit(id, vec![Clearance::ContactGround]);
    let events = run(&mut world, 8.0);
    let what = events.iter().find_map(|e| match e {
        SimEvent::ReadbackMissed { what, .. } => Some(what.clone()),
        _ => None,
    });
    assert!(what.expect("readback error scored").contains("climbing to"));
    // Ground puts it right.
    assert_eq!(world.find(id).unwrap().target_altitude, initial);
}

#[test]
fn a_wrong_code_left_alone_shows_on_the_scope_until_departure_resets_it() {
    let mut world = harrowgate(Staffing::DELIVERY, 7);
    let id = departure(&mut world);
    let clearance = right_clearance(&world, id);
    let Clearance::Ifr { squawk, .. } = clearance else { unreachable!() };
    world.transmit(id, vec![clearance]);
    run(&mut world, 8.0);
    let wrong = squawk ^ 0o11;
    world.find_mut(id).unwrap().flight_plan.as_mut().unwrap().readback = Some(Readback::Squawk(wrong));
    world.transmit(id, vec![Clearance::ContactGround]);
    let events = run(&mut world, 8.0);
    assert!(events.iter().any(|e| matches!(e, SimEvent::ReadbackMissed { .. })), "{events:?}");
    // Still at the gate: nothing on the scope yet.
    assert_eq!(delivery::wrong_code_showing(world.find(id).unwrap()), None);

    // Airborne, the wrong code shows until the automated Departure has it reset.
    let mut shown = false;
    for _ in 0..(1200.0 / TICK) as usize {
        world.tick();
        world.drain_events();
        let Some(ac) = world.find(id) else { break };
        if delivery::wrong_code_showing(ac) == Some(wrong) {
            shown = true;
        } else if shown {
            break;
        }
    }
    assert!(shown, "the wrong code should show once airborne");
    let ac = world.find(id).expect("still in the sector");
    assert_eq!(delivery::wrong_code_showing(ac), None);
    assert!(said(&world, &format!("reset transponder, squawk {}", delivery::code_text(squawk))), "{:?}", world.log);
}

#[test]
fn a_player_working_approach_resets_the_code_themselves() {
    let mut world = harrowgate(Staffing::ALL, 7);
    let id = departure(&mut world);
    let clearance = right_clearance(&world, id);
    let Clearance::Ifr { squawk, .. } = clearance else { unreachable!() };
    world.transmit(id, vec![clearance]);
    run(&mut world, 8.0);
    let ac = world.find_mut(id).unwrap();
    let plan = ac.flight_plan.as_mut().unwrap();
    plan.squawking = Some(squawk ^ 0o11);
    // On the ground the pilot can't help.
    world.transmit(id, vec![Clearance::Squawk(squawk)]);
    run(&mut world, 8.0);
    assert!(said(&world, "unable"), "{:?}", world.log.last());
    // Airborne, it can.
    let ac = world.find_mut(id).unwrap();
    ac.on_ground = false;
    ac.altitude += 3000.0;
    ac.phase = Phase::Controlled;
    world.transmit(id, vec![Clearance::Squawk(squawk)]);
    run(&mut world, 8.0);
    assert!(said(&world, "reset transponder"));
    assert_eq!(world.find(id).unwrap().flight_plan.as_ref().unwrap().squawking, None);
}

#[test]
fn about_one_plan_in_six_is_filed_wrong_and_one_readback_in_ten_comes_back_wrong() {
    let (mut wrong_plans, mut wrong_readbacks, mut n) = (0, 0, 0);
    for seed in 0..360 {
        let mut world = harrowgate(Staffing::DELIVERY, seed);
        let id = departure(&mut world);
        n += 1;
        let plan = world.find(id).unwrap().flight_plan.clone().unwrap();
        if delivery::problem(&world.airport, world.config_name(), &plan.route, plan.altitude).is_some() {
            wrong_plans += 1;
        }
        world.transmit(id, vec![right_clearance(&world, id)]);
        run(&mut world, 8.0);
        if readback_error(&world, id).is_some() {
            wrong_readbacks += 1;
        }
    }
    let plans = wrong_plans as f32 / n as f32;
    let readbacks = wrong_readbacks as f32 / n as f32;
    assert!((0.10..0.24).contains(&plans), "{wrong_plans} of {n} plans wrong");
    assert!((0.05..0.16).contains(&readbacks), "{wrong_readbacks} of {n} readbacks wrong");
}

#[test]
fn with_delivery_automated_departures_are_cleared_at_the_gate_with_no_mistakes() {
    for seed in 0..60 {
        let mut world = harrowgate(Staffing::GROUND, seed);
        let id = departure(&mut world);
        let ac = world.find(id).unwrap();
        assert_eq!(ac.frequency(), Position::Ground);
        let plan = ac.flight_plan.as_ref().unwrap();
        assert!(plan.is_cleared() && plan.done);
        assert!(delivery::problem(&world.airport, world.config_name(), &plan.route, plan.altitude).is_none());
    }
}

/// A whole shift on Delivery, cleared by a careful controller: every plan put
/// right, every readback listened to, every aircraft sent to Ground.
#[test]
fn a_busy_delivery_shift_goes_cleanly() {
    let config = WorldConfig {
        seed: 21,
        traffic: true,
        busyness: atcgame::sim::traffic::Busyness::Busy,
        staffing: Staffing::DELIVERY,
        shift_seconds: 40.0 * 60.0,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    let mut events = Vec::new();
    while world.ended.is_none() {
        world.tick();
        events.extend(world.drain_events());
        let ids: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
        for id in ids {
            let Some(ac) = world.find(id) else { continue };
            if ac.frequency() != Position::Delivery || ac.phase != Phase::Parked {
                continue;
            }
            let plan = ac.flight_plan.clone().unwrap();
            let waited = world.time - plan.called_at.unwrap();
            if world.find(id).is_some_and(|a| world.awaiting(a).is_some()) && waited >= 20.0 {
                match &plan.cleared {
                    None => {
                        world.transmit(id, vec![right_clearance(&world, id)]);
                    }
                    Some(c) if plan.readback.is_some() && world.time - c.time > 8.0 => {
                        let c = c.clone();
                        world.transmit(id, vec![Clearance::Ifr { route: c.route, altitude: c.altitude, squawk: c.squawk }]);
                    }
                    Some(c) if plan.readback.is_none() && world.time - c.time > 8.0 => {
                        world.transmit(id, vec![Clearance::ContactGround]);
                    }
                    _ => {}
                }
            }
        }
    }
    let cleared = events.iter().filter(|e| matches!(e, SimEvent::Cleared { .. })).count();
    let caught = events.iter().filter(|e| matches!(e, SimEvent::PlanCaught { .. })).count();
    let bad: Vec<&SimEvent> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                SimEvent::PlanMissed { .. }
                    | SimEvent::ReadbackMissed { .. }
                    | SimEvent::NearCollision { .. }
                    | SimEvent::GroundConflict { .. }
                    | SimEvent::RunwayIncursion { .. }
            )
        })
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
    assert!(cleared >= 15, "only {cleared} cleared");
    assert!(caught >= 1, "no flight plan errors caught in {cleared}");
    // As many get away as with every position automated (6 on this seed).
    assert!(world.score.exited >= 6, "{} departures got away", world.score.exited);
    assert!(world.score.points > 0, "{}", world.score.points);
}

#[test]
fn ksus_clearances_name_the_sid_and_transition() {
    use atcgame::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};
    let airport = Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap();
    let config = WorldConfig {
        seed: 3,
        traffic: false,
        wind: Some(Wind { from: 260.0, speed: 0.0 }),
        staffing: Staffing::DELIVERY,
        ..Default::default()
    };
    let mut world = World::new(airport, config);
    let config = world.config_name().to_string();
    let sid = world
        .airport
        .sids
        .iter()
        .find(|s| s.config == config && s.name.as_deref() == Some("LINDY9"))
        .unwrap()
        .fixes
        .clone();
    let route = world.airport.route_indices(&sid);
    world.request_departure(aircraft_type("B738").unwrap(), route, 10000.0);
    world.tick();
    let id = world.aircraft.iter().find(|a| a.flow == Flow::Departure).unwrap().id;
    let clearance = right_clearance(&world, id);
    let words = atcgame::sim::clearance::phrase(&clearance, world.find(id).unwrap(), &world.airport, &world.ends_in_use());
    let exit = sid.last().unwrap();
    assert!(words.contains(&format!("via the LINDY9 departure, {exit} transition, then as filed")), "{words}");
    assert!(words.contains("squawk 0"), "{words}");
}

// Stress: whole shifts on Delivery, played carefully and sloppily.

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bot {
    /// Clears every plan right, re-clears one a runway change has made wrong,
    /// repeats every wrong readback, then hands off.
    Careful,
    /// Clears every plan as filed and never listens to the readback.
    Sloppy,
}

#[derive(Debug, Default)]
struct ShiftStats {
    cleared: usize,
    caught: usize,
    missed_plans: usize,
    missed_readbacks: usize,
    wrong_codes_reset: usize,
    exited: usize,
    /// Anything that shouldn't happen, for the assertion message.
    problems: Vec<String>,
}

/// The clearance the careful bot gives: the filed plan put right for the
/// runways in use now, or `None` if no SID in use serves its exit.
fn careful_clearance(world: &World, id: u32) -> Option<Clearance> {
    let plan = world.find(id)?.flight_plan.as_ref()?;
    let config = world.config_name();
    let route = if delivery::sid_in_use(&world.airport, config, &plan.route) {
        plan.route.clone()
    } else {
        delivery::sid_choices(&world.airport, config, &plan.route).into_iter().next()?
    };
    let course = delivery::course(&world.airport, &route);
    Some(Clearance::Ifr { altitude: delivery::level_for(plan.altitude, course), route, squawk: plan.squawk })
}

/// Standing still this long on the ground after Delivery is gridlock, seconds.
/// (KSUS landing and departing 08R on a very busy shift can keep a departure
/// at the hold line for 11 minutes waiting for a gap: that's the tower's.)
const STUCK_AFTER_DELIVERY_S: f32 = 15.0 * 60.0;

fn play_delivery_shift(airport: &Airport, config: WorldConfig, bot: Bot) -> ShiftStats {
    use std::collections::HashMap;
    let mut world = World::new(airport.clone(), WorldConfig { staffing: Staffing::DELIVERY, ..config });
    let mut stats = ShiftStats::default();
    let mut last_sent: HashMap<u32, f32> = HashMap::new();
    let mut showed_wrong_code: HashMap<u32, bool> = HashMap::new();
    // Where each departure on the ground after Delivery last moved, and when.
    let mut last_moved: HashMap<u32, (glam::Vec2, f32)> = HashMap::new();
    while world.ended.is_none() {
        world.tick();
        for event in world.drain_events() {
            match event {
                SimEvent::Cleared { .. } => stats.cleared += 1,
                SimEvent::PlanCaught { .. } => stats.caught += 1,
                SimEvent::PlanMissed { .. } => stats.missed_plans += 1,
                SimEvent::ReadbackMissed { .. } => stats.missed_readbacks += 1,
                SimEvent::MissedHandoff { callsign, to, .. } => {
                    stats.problems.push(format!("missed handoff of {callsign} to {to}"))
                }
                SimEvent::NearCollision { .. } | SimEvent::GroundConflict { .. } | SimEvent::RunwayIncursion { .. } => {
                    stats.problems.push(format!("{event:?}"))
                }
                _ => {}
            }
        }
        let now = world.time;
        // Nobody leaves the sector still squawking the wrong code.
        for (id, showing) in showed_wrong_code.iter_mut() {
            if *showing && world.find(*id).is_none() {
                stats.problems.push(format!("aircraft {id} left still squawking a wrong code"));
                *showing = false;
            }
        }
        let ids: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
        for id in ids {
            let Some(ac) = world.find(id) else { continue };
            // Handed on, a departure keeps moving. Long taxis and queues at
            // the hold line are the airfield's; standing still for minutes
            // on end is gridlock.
            let done = ac.flight_plan.as_ref().is_some_and(|p| p.done && p.called_at.is_some());
            if done && ac.on_ground {
                let (at, since) = *last_moved.entry(id).or_insert((ac.pos, now));
                if ac.pos.distance(at) > 0.01 {
                    last_moved.insert(id, (ac.pos, now));
                } else if now - since > STUCK_AFTER_DELIVERY_S {
                    stats.problems.push(format!("{} standing still since {since:.0} ({:?})", ac.callsign, ac.phase));
                    last_moved.insert(id, (ac.pos, f32::INFINITY));
                }
            }
            // A wrong code on the scope is reset soon after Departure has it.
            if let Some(code) = delivery::wrong_code_showing(ac) {
                showed_wrong_code.insert(id, true);
                let on_departure = ac.frequency() == Position::Approach;
                if on_departure && now - ac.freq_since > delivery::SQUAWK_RESET_S + 30.0 {
                    stats.problems.push(format!("{} still squawking {code:o} at {now:.0}", ac.callsign));
                    showed_wrong_code.insert(id, false);
                }
            } else if showed_wrong_code.get(&id) == Some(&true) {
                showed_wrong_code.insert(id, false);
                stats.wrong_codes_reset += 1;
            }
            if ac.frequency() != Position::Delivery || ac.phase != Phase::Parked {
                continue;
            }
            let Some(plan) = ac.flight_plan.clone() else { continue };
            let called = plan.called_at.unwrap_or(now);
            // Nobody waits long at the gate for a bot that answers in seconds.
            if now - called > 300.0 && !last_sent.contains_key(&(id + 1_000_000)) {
                stats.problems.push(format!("{} waiting at the gate since {called:.0}", ac.callsign));
                last_sent.insert(id + 1_000_000, now);
            }
            if now - called < 15.0 || now - last_sent.get(&id).copied().unwrap_or(f32::MIN) < 10.0 {
                continue;
            }
            let clearance = match (bot, &plan.cleared) {
                (Bot::Careful, None) => careful_clearance(&world, id),
                (Bot::Careful, Some(c)) => {
                    let config = world.config_name();
                    let still_right = delivery::problem(&world.airport, config, &c.route, c.altitude).is_none();
                    if !still_right {
                        // The runways changed under it: an amended clearance.
                        careful_clearance(&world, id)
                    } else if plan.readback.is_some() {
                        Some(Clearance::Ifr { route: c.route.clone(), altitude: c.altitude, squawk: c.squawk })
                    } else {
                        Some(Clearance::ContactGround)
                    }
                }
                (Bot::Sloppy, None) => Some(as_filed(&world, id)),
                (Bot::Sloppy, Some(_)) => Some(Clearance::ContactGround),
            };
            if let Some(clearance) = clearance {
                world.transmit(id, vec![clearance]);
                last_sent.insert(id, now);
            }
        }
    }
    stats.exited = world.score.exited as usize;
    if bot == Bot::Careful {
        let refused = world.log.iter().filter(|l| l.text.contains("unable")).map(|l| l.text.clone());
        stats.problems.extend(refused);
    }
    stats
}

/// Conflicts in the same shift with every position automated: those aren't
/// Delivery's doing. (Its exits aren't comparable: every radio call draws on
/// the sim's random numbers, so the traffic itself differs.)
fn automated_trouble(airport: &Airport, config: WorldConfig) -> Vec<String> {
    let mut world = World::new(airport.clone(), WorldConfig { staffing: Staffing::NONE, ..config });
    let mut trouble = Vec::new();
    while world.ended.is_none() {
        world.tick();
        for event in world.drain_events() {
            if matches!(
                event,
                SimEvent::NearCollision { .. } | SimEvent::GroundConflict { .. } | SimEvent::RunwayIncursion { .. }
            ) {
                trouble.push(format!("{event:?}"));
            }
        }
    }
    trouble
}

fn delivery_stress(shifts: impl Iterator<Item = (Airport, WorldConfig)>) {
    let mut totals = [ShiftStats::default(), ShiftStats::default()];
    let mut failures = Vec::new();
    for (airport, config) in shifts {
        let what = format!("{} seed {} {:?}", airport.code, config.seed, config.busyness);
        let baseline_trouble = automated_trouble(&airport, config);
        for (i, bot) in [Bot::Careful, Bot::Sloppy].into_iter().enumerate() {
            let s = play_delivery_shift(&airport, config, bot);
            let mut problems = s.problems.clone();
            // Trouble the automated tower has on its own isn't Delivery's doing.
            if !baseline_trouble.is_empty() {
                problems.retain(|p| !p.starts_with("NearCollision") && !p.starts_with("GroundConflict") && !p.starts_with("RunwayIncursion"));
            }
            if bot == Bot::Careful {
                if s.missed_plans + s.missed_readbacks > 0 {
                    problems.push(format!("{} plans and {} readbacks missed", s.missed_plans, s.missed_readbacks));
                }
            } else if s.caught > 0 {
                problems.push(format!("{} caught by a bot that never checks", s.caught));
            }
            if !problems.is_empty() {
                failures.push(format!("{what} {bot:?}: {problems:?}"));
            }
            let t = &mut totals[i];
            t.cleared += s.cleared;
            t.caught += s.caught;
            t.missed_plans += s.missed_plans;
            t.missed_readbacks += s.missed_readbacks;
            t.wrong_codes_reset += s.wrong_codes_reset;
            t.exited += s.exited;
        }
    }
    println!("careful: {:?}", totals[0]);
    println!("sloppy:  {:?}", totals[1]);
    assert!(failures.is_empty(), "{failures:#?}");
    let [careful, sloppy] = &totals;
    assert!(careful.cleared > 0 && careful.caught > 0, "{careful:?}");
    assert!(sloppy.missed_plans > 0, "{sloppy:?}");
}

fn ksus() -> Airport {
    use atcgame::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

/// Seed `seed`'s shift: the airport's two flows, and every third shift with
/// changing weather (and so runway changes mid-shift).
fn stress_config(seed: u64, winds: [f32; 2]) -> WorldConfig {
    use atcgame::sim::traffic::Busyness;
    let busy = [Busyness::Quiet, Busyness::RealWorld, Busyness::Busy, Busyness::VeryBusy];
    WorldConfig {
        seed,
        wind: (!seed.is_multiple_of(3)).then(|| Wind { from: winds[seed as usize % 2], speed: 8.0 }),
        busyness: busy[seed as usize % 4],
        shift_seconds: 45.0 * 60.0,
        ..Default::default()
    }
}

#[test]
fn delivery_shifts_played_carefully_and_sloppily_hold_up() {
    let shifts = (0..3u64)
        .map(|seed| (Airport::default_airport(), stress_config(seed, [270.0, 90.0])))
        .chain((0..3u64).map(|seed| (ksus(), stress_config(seed, [250.0, 80.0]))));
    delivery_stress(shifts);
}

/// The long run: `cargo test --release -- --ignored delivery_stress`.
#[test]
#[ignore]
fn delivery_stress_two_hundred_shifts() {
    let shifts = (0..100u64)
        .map(|seed| (Airport::default_airport(), stress_config(seed, [270.0, 90.0])))
        .chain((0..100u64).map(|seed| (ksus(), stress_config(seed, [250.0, 80.0]))));
    delivery_stress(shifts);
}
