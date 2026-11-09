//! Co-op players dropping out of a shift and coming back: their positions
//! are automated meanwhile, and the shift carries on.

use atcgame::sim::aircraft::{Flow, Lateral, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::emergency::EmergencyRate;
use atcgame::sim::positions::{Position, Seat, Staffing};
use atcgame::sim::replay::Action;
use atcgame::sim::traffic::Busyness;
use atcgame::sim::{World, WorldConfig};

const A: Seat = 0;
const B: Seat = 1;
const MINUTE: u32 = 240;

/// Approach in seat A, the tower cab in seat B.
fn split() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, A)
        .with_seat(Position::Local, B)
        .with_seat(Position::Ground, B)
        .with_seat(Position::Delivery, B)
}

fn busy(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 11,
        staffing,
        busyness: Busyness::VeryBusy,
        emergencies: EmergencyRate::Frequent,
        shift_seconds: 40.0 * 60.0,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

/// A player in `seat` who keeps talking to their own traffic: takes
/// handoffs, lands arrivals, and otherwise gives a spread of instructions,
/// so there's traffic in every state when someone drops.
fn player(world: &mut World, seat: Seat, step: u32) {
    if !step.is_multiple_of(20) {
        return;
    }
    let ends = world.airport.runways.len();
    let arrival = world.airport.runway_index(world.runway_ids().0).unwrap();
    let nodes = world.airport.surface.as_ref().map_or(1, |m| m.nodes.len());
    let mine: Vec<(u32, Phase)> =
        world.aircraft.iter().filter(|a| world.is_seats(seat, a) || a.phase == Phase::Offered).map(|a| (a.id, a.phase)).collect();
    for (n, (id, phase)) in mine.into_iter().enumerate() {
        let sensible = match phase {
            Phase::Offered => Some(Action::AcceptHandoff),
            Phase::Localizer | Phase::Glideslope => Some(Action::Transmit(vec![Clearance::HandoffTower])),
            Phase::TowerHandedOff => Some(Action::Transmit(vec![Clearance::ClearedToLand(arrival)])),
            _ => None,
        };
        if let Some(action) = sensible {
            world.act(seat, id, action);
            continue;
        }
        let k = (step / 20) as usize + n * 3 + id as usize;
        let clearance = match k % 9 {
            0 => Clearance::Heading(((k * 37) % 360) as f32),
            1 => Clearance::Altitude(((k % 6) as f32 + 3.0) * 1000.0),
            2 | 3 => Clearance::ClearedIls(arrival),
            4 => Clearance::HandoffTower,
            5 => Clearance::Taxi { to: k % nodes, via: Vec::new() },
            6 => Clearance::ClearedForTakeoff(k % ends),
            7 => Clearance::ClearedToLand(arrival),
            _ => Clearance::ContactDeparture,
        };
        world.act(seat, id, Action::Transmit(vec![clearance]));
    }
}

/// Run `ticks` with these seats playing.
fn run(world: &mut World, seats: &[Seat], ticks: u32, step: &mut u32) {
    for _ in 0..ticks {
        if world.ended.is_some() {
            return;
        }
        for &seat in seats {
            player(world, seat, *step);
        }
        world.tick();
        world.drain_events();
        *step += 1;
    }
}

/// Airborne IFR arrivals Approach would be working, not in a hold.
fn worked_by_approach(world: &World) -> Vec<String> {
    world
        .aircraft
        .iter()
        .filter(|a| {
            !a.on_ground
                && a.vfr.is_none()
                && a.flow == Flow::Arrival
                && matches!(a.phase, Phase::Offered | Phase::Controlled | Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope)
                && !matches!(a.lateral, Lateral::Hold(_))
        })
        .map(|a| a.callsign.clone())
        .collect()
}

#[test]
fn approach_drops_and_comes_back() {
    let mut world = busy(split());
    let mut step = 0;
    run(&mut world, &[A, B], 10 * MINUTE, &mut step);
    assert!(!worked_by_approach(&world).is_empty(), "nothing for Approach to hand over");

    assert!(world.act(B, 0, Action::Vacate(A)));
    assert!(!world.staffing().approach);
    assert!(world.staffing().seat_works(B, Position::Local));
    assert_eq!(worked_by_approach(&world), Vec::<String>::new(), "the automated Approach holds them");
    assert!(world.log.iter().any(|l| l.text.contains("Approach is automated")));

    let landed = world.score.landed;
    run(&mut world, &[B], 10 * MINUTE, &mut step);
    assert!(world.score.landed >= landed + 3, "arrivals keep landing: {} then {}", landed, world.score.landed);

    assert!(world.act(B, 0, Action::Take(A)));
    assert!(world.staffing().seat_works(A, Position::Approach));
    // The holds are Approach's again, with an EFC still to come.
    for a in &world.aircraft {
        if let Lateral::Hold(state) = &a.lateral {
            assert!(state.spec.efc > world.time, "{} EFC {} at {}", a.callsign, state.spec.efc, world.time);
        }
    }
    run(&mut world, &[A, B], 5 * MINUTE, &mut step);
}

#[test]
fn the_tower_drops_and_is_automated() {
    let mut world = busy(split());
    let mut step = 0;
    run(&mut world, &[A, B], 10 * MINUTE, &mut step);
    assert!(world.act(A, 0, Action::Vacate(B)));
    let staffing = world.staffing();
    assert!(staffing.approach && !staffing.local && !staffing.ground && !staffing.delivery);
    assert!(world.log.iter().any(|l| l.text.contains("Tower, Ground and Clearance are automated")));

    let landed = world.score.landed;
    let mut took_off = Vec::new();
    for _ in 0..15 {
        run(&mut world, &[A], MINUTE, &mut step);
        for a in &world.aircraft {
            if a.flow == Flow::Departure && !a.on_ground && !took_off.contains(&a.callsign) {
                took_off.push(a.callsign.clone());
            }
        }
    }
    assert!(world.score.landed > landed, "the automated tower lands arrivals");
    assert!(took_off.len() >= 3, "and gets departures off: {took_off:?}");
}

#[test]
fn only_a_seat_with_positions_drops_or_comes_back() {
    let mut world = busy(split());
    assert!(!world.act(A, 0, Action::Take(B)), "B hasn't gone");
    assert!(!world.act(A, 0, Action::Vacate(2)), "nobody is in seat 2");
    assert!(world.act(A, 0, Action::Vacate(B)));
    assert!(!world.act(A, 0, Action::Vacate(B)), "already gone");
    assert!(world.act(A, 0, Action::Take(B)));
    assert_eq!(world.staffing(), split());
    assert_eq!(world.seated(), split());
}

#[test]
fn a_shift_with_a_drop_replays_the_same() {
    let mut world = busy(split());
    let mut step = 0;
    run(&mut world, &[A, B], 6 * MINUTE, &mut step);
    world.act(B, 0, Action::Vacate(A));
    run(&mut world, &[B], 4 * MINUTE, &mut step);
    world.act(B, 0, Action::Take(A));
    run(&mut world, &[A, B], 4 * MINUTE, &mut step);

    let mut again = busy(split());
    let commands = world.commands().to_vec();
    again.run_to(world.ticks(), &commands);
    assert_eq!(again.checksum(), world.checksum());
    assert_eq!(again.staffing(), world.staffing());
}

#[test]
fn a_coop_replay_taken_over_alone_plays_back() {
    let mut world = busy(split());
    let start = world.clone();
    let mut step = 0;
    run(&mut world, &[A, B], 6 * MINUTE, &mut step);
    // B has dropped when the replay's taken over: their positions come back.
    assert!(world.act(A, 0, Action::Vacate(B)));
    run(&mut world, &[A], 2 * MINUTE, &mut step);
    assert!(!world.staffing().local);
    assert!(world.act(A, 0, Action::Solo));
    assert_eq!(world.staffing(), Staffing::ALL);
    assert!(!world.seated().is_coop());
    assert!(!world.act(A, 0, Action::Solo), "once is enough");
    // One player works the lot from here.
    run(&mut world, &[A], 6 * MINUTE, &mut step);
    assert!(world.aircraft.iter().filter(|a| a.frequency() == Position::Ground).all(|a| world.is_seats(A, a)));
    // The saved commands rebuild it, Solo and all.
    let mut again = start.clone();
    again.run_to(world.ticks(), world.commands());
    assert_eq!(again.checksum(), world.checksum());
    assert_eq!(again.staffing(), Staffing::ALL);
}

#[test]
fn solo_is_only_for_coop() {
    let mut world = busy(Staffing::ALL);
    assert!(!world.act(0, 0, Action::Solo));
}

#[test]
fn the_team_score_is_split_between_the_players_positions() {
    let mut world = busy(split());
    let mut step = 0;
    run(&mut world, &[A, B], 8 * MINUTE, &mut step);
    // B drops: what happens on their positions is still put down to them.
    assert!(world.act(A, 0, Action::Vacate(B)));
    run(&mut world, &[A], 8 * MINUTE, &mut step);
    let score = &world.score;
    assert_eq!(score.by_position.iter().sum::<i32>(), score.points);
    assert!(score.by_position[0] != 0 && score.by_position[1..].iter().any(|&p| p != 0), "{:?}", score.by_position);
    for incident in &score.incidents {
        assert!(world.seated().works(incident.position), "{incident:?}");
    }
}
