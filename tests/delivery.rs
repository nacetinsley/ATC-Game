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
    let words = atcgame::sim::clearance::phrase(&clearance, world.find(id).unwrap(), &world.airport);
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
    let words = atcgame::sim::clearance::phrase(&clearance, world.find(id).unwrap(), &world.airport);
    let exit = sid.last().unwrap();
    assert!(words.contains(&format!("via the LINDY9 departure, {exit} transition, then as filed")), "{words}");
    assert!(words.contains("squawk 0"), "{words}");
}
