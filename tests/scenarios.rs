//! End-to-end scenarios run through the public sim API, with no window.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::scoring::{GoAroundReason, ShiftEnd, SimEvent};
use atcgame::sim::units::Wind;
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

fn quiet_world(wind_speed: f32) -> World {
    // Wind from the west puts the field on the 27s: land 27L, depart 27R.
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(if wind_speed == 0.0 {
            Wind { from: 270.0, speed: 0.0 }
        } else {
            Wind { from: 270.0, speed: wind_speed }
        }),
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

fn arrival(id: u32, name: &str, pos: Vec2, heading: f32, altitude: f32) -> Aircraft {
    Aircraft::new(
        id,
        name,
        Flow::Arrival,
        aircraft_type("B738").unwrap(),
        pos,
        heading,
        altitude,
        200.0,
    )
}

fn run(world: &mut World, seconds: f32, events: &mut Vec<SimEvent>) {
    for _ in 0..(seconds / TICK).ceil() as usize {
        world.tick();
        events.extend(world.drain_events());
    }
}

/// Run until a condition holds, failing the test if it never does.
fn run_until(
    world: &mut World,
    max_seconds: f32,
    events: &mut Vec<SimEvent>,
    done: impl Fn(&World) -> bool,
) {
    for _ in 0..(max_seconds / TICK).ceil() as usize {
        if done(world) {
            return;
        }
        world.tick();
        events.extend(world.drain_events());
    }
    panic!("condition not met within {max_seconds} s; events: {events:?}");
}

fn runway_27l(world: &World) -> usize {
    world.airport.runway_index("27L").unwrap()
}

fn last_pilot_line(world: &World) -> &str {
    &world.log.iter().rev().find(|l| l.speaker == Speaker::Pilot).unwrap().text
}

#[test]
fn arrival_lands_after_vector_clearance_and_handoff() {
    for wind in [0.0, 25.0] {
        let mut world = quiet_world(wind);
        let mut events = Vec::new();
        let rwy = runway_27l(&world);
        world.add_aircraft(arrival(1, "NVA101", Vec2::new(22.0, 5.0), 240.0, 4000.0));

        assert!(world.transmit(1, vec![Clearance::ClearedIls(rwy)]));
        run_until(&mut world, 240.0, &mut events, |w| {
            matches!(w.find(1).unwrap().phase, Phase::Localizer | Phase::Glideslope)
        });
        assert!(world.transmit(1, vec![Clearance::HandoffTower]));
        run_until(&mut world, 600.0, &mut events, |w| {
            w.find(1).is_some_and(|a| a.phase == Phase::Rollout)
        });

        assert!(
            events.iter().any(|e| matches!(e, SimEvent::Landed { callsign, .. } if callsign == "NVA101")),
            "wind {wind}: {events:?}"
        );
        assert_eq!(world.score.landed, 1);
        assert!(world.score.incidents.is_empty(), "wind {wind}: {:?}", world.score.incidents);

        // It clears the runway and leaves the sim once rollout finishes.
        run(&mut world, 70.0, &mut events);
        assert!(world.find(1).is_none());
    }
}

#[test]
fn pilots_respond_only_after_a_two_to_six_second_delay() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(20.0, 10.0), 90.0, 5000.0));
    world.transmit(1, vec![Clearance::Heading(180.0)]);

    run(&mut world, 1.75, &mut events);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(90.0));
    assert!(world.log.iter().all(|l| l.speaker != Speaker::Pilot));

    run(&mut world, 4.5, &mut events);
    assert_eq!(world.find(1).unwrap().lateral, Lateral::Heading(180.0));
    assert!(last_pilot_line(&world).contains("heading 180"));
}

#[test]
fn steep_intercept_is_refused() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(20.0, 8.0), 180.0, 4000.0));
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_pilot_line(&world).contains("intercept angle too steep"));
    assert_eq!(world.find(1).unwrap().phase, Phase::Controlled);
}

#[test]
fn approach_from_above_the_glideslope_is_refused() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    // 10 NM out the glideslope is near 3,200 ft; this aircraft is far above it.
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(12.0, 2.0), 250.0, 8000.0));
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_pilot_line(&world).contains("above the glideslope"));
}

#[test]
fn heading_pointing_away_from_the_localizer_is_refused() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    // North of a westbound final, the aircraft must head south of west to reach it.
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(20.0, 5.0), 290.0, 4000.0));
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_pilot_line(&world).contains("won't intercept"));
}

#[test]
fn vector_and_approach_clearance_work_in_one_call() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(22.0, 6.0), 180.0, 4000.0));
    world.transmit(1, vec![Clearance::Heading(240.0), Clearance::ClearedIls(rwy)]);
    run(&mut world, 7.0, &mut events);
    assert_eq!(world.find(1).unwrap().phase, Phase::ClearedApproach);
}

#[test]
fn late_tower_handoff_causes_a_go_around() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(20.0, 3.0), 240.0, 3000.0));
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);

    run_until(&mut world, 400.0, &mut events, |w| w.find(1).unwrap().phase == Phase::GoAround);

    assert!(events.iter().any(|e| matches!(e, SimEvent::LateHandoff { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround { reason: GoAroundReason::LateHandoff, .. }
    )));

    // The aircraft climbs out and comes back to the player for another try.
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().phase == Phase::Controlled);
    assert_eq!(world.find(1).unwrap().target_altitude, world.airport.go_around_altitude);
}

#[test]
fn occupied_runway_sends_the_next_arrival_around() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    // 2.6 NM apart: the follower reaches the 1 NM check about 40 s after the leader
    // touches down, inside its 45 s rollout.
    let mut leader = arrival(1, "NVA101", Vec2::new(4.0, -0.6), 270.0, 800.0);
    let mut follower = arrival(2, "NVA102", Vec2::new(6.6, -0.6), 270.0, 1700.0);
    for ac in [&mut leader, &mut follower] {
        ac.phase = Phase::TowerHandedOff;
        ac.lateral = Lateral::Localizer;
        ac.approach_runway = Some(rwy);
        ac.on_glideslope = true;
    }
    follower.target_speed = 140.0;
    world.add_aircraft(leader);
    world.add_aircraft(follower);

    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(2).is_some_and(|a| a.phase == Phase::GoAround)
    });
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround { reason: GoAroundReason::RunwayOccupied, .. }
    )));
}

#[test]
fn asdex_alerts_on_an_arrival_to_an_occupied_runway() {
    use atcgame::ui::surface::safety_alerts;
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let rwy = runway_27l(&world);
    let mut leader = arrival(1, "NVA101", Vec2::new(4.0, -0.6), 270.0, 800.0);
    let mut follower = arrival(2, "NVA102", Vec2::new(6.6, -0.6), 270.0, 1700.0);
    for ac in [&mut leader, &mut follower] {
        ac.phase = Phase::TowerHandedOff;
        ac.lateral = Lateral::Localizer;
        ac.approach_runway = Some(rwy);
        ac.on_glideslope = true;
    }
    follower.target_speed = 140.0;
    world.add_aircraft(leader);
    world.add_aircraft(follower);

    // Nothing while the leader is still flying.
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).is_some_and(|a| a.on_ground));
    assert!(safety_alerts(&world).is_empty());
    run_until(&mut world, 300.0, &mut events, |w| !safety_alerts(w).is_empty());
    let alert = &safety_alerts(&world)[0];
    assert_eq!(alert.area, "27L");
    assert_eq!(alert.ids, vec![2, 1]);
    assert_eq!(alert.text, "ARRIVAL RWY OCCUPIED");
}

#[test]
fn converging_traffic_alerts_before_separation_is_lost() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(-15.0, 20.0), 90.0, 6000.0));
    world.add_aircraft(arrival(2, "NVA102", Vec2::new(15.0, 20.0), 270.0, 6000.0));

    run_until(&mut world, 300.0, &mut events, |w| w.score.separation_losses > 0);

    let alert = events.iter().position(|e| matches!(e, SimEvent::ConflictAlert { .. }));
    let lost = events.iter().position(|e| matches!(e, SimEvent::LossOfSeparation { .. }));
    assert!(alert.is_some() && alert < lost, "{events:?}");
    assert_eq!(world.score.points, atcgame::sim::scoring::PENALTY_SEPARATION);
}

#[test]
fn near_collision_ends_the_shift() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(-15.0, 20.0), 90.0, 6000.0));
    world.add_aircraft(arrival(2, "NVA102", Vec2::new(15.0, 20.0), 270.0, 6000.0));
    run_until(&mut world, 300.0, &mut events, |w| w.ended.is_some());
    assert_eq!(world.ended, Some(ShiftEnd::NearCollision));
}

#[test]
fn wake_spacing_is_flagged_behind_a_heavy_but_not_when_legal() {
    let setup = |gap: f32| {
        let mut world = quiet_world(0.0);
        let rwy = runway_27l(&world);
        let x = world.airport.runways[rwy].threshold().x + 10.0;
        let mut leader = Aircraft::new(
            1,
            "NVA101",
            Flow::Arrival,
            aircraft_type("B77W").unwrap(),
            Vec2::new(x, -0.6),
            270.0,
            3200.0,
            160.0,
        );
        let mut follower = arrival(2, "NVA102", Vec2::new(x + gap, -0.6), 270.0, 5000.0);
        for ac in [&mut leader, &mut follower] {
            ac.phase = Phase::Glideslope;
            ac.lateral = Lateral::Localizer;
            ac.approach_runway = Some(rwy);
            ac.on_glideslope = true;
        }
        world.add_aircraft(leader);
        world.add_aircraft(follower);
        world.tick();
        world.drain_events()
    };
    let wake = |events: &[SimEvent]| events.iter().any(|e| matches!(e, SimEvent::WakeViolation { .. }));
    assert!(wake(&setup(4.5)), "large 4.5 NM behind heavy is too close");
    assert!(!wake(&setup(5.5)), "large 5.5 NM behind heavy is legal");
}

#[test]
fn atpa_warns_and_alerts_as_a_follower_closes_on_the_minimum() {
    use atcgame::sim::conflict::AtpaStatus;
    // A large closing on a heavy, which needs 5 NM: about 45 kt faster.
    let status = |gap: f32| {
        let mut world = quiet_world(0.0);
        let rwy = runway_27l(&world);
        let x = world.airport.runways[rwy].threshold().x + 10.0;
        let mut leader = Aircraft::new(
            1,
            "NVA101",
            Flow::Arrival,
            aircraft_type("B77W").unwrap(),
            Vec2::new(x, -0.6),
            270.0,
            3200.0,
            160.0,
        );
        let mut follower = arrival(2, "NVA102", Vec2::new(x + gap, -0.6), 270.0, 5000.0);
        for ac in [&mut leader, &mut follower] {
            ac.phase = Phase::Glideslope;
            ac.lateral = Lateral::Localizer;
            ac.approach_runway = Some(rwy);
            ac.on_glideslope = true;
        }
        world.add_aircraft(leader);
        world.add_aircraft(follower);
        world.tick();
        world.tick();
        let atpa = world.atpa();
        assert_eq!(atpa.len(), 1);
        let a = atpa[0];
        assert_eq!((a.leader, a.follower, a.required), (1, 2, 5.0));
        assert!((a.distance - gap).abs() < 0.1, "{a:?}");
        a.status
    };
    assert_eq!(status(8.0), AtpaStatus::Monitor);
    assert_eq!(status(5.45), AtpaStatus::Warning, "the minimum goes inside 45 s");
    assert_eq!(status(5.2), AtpaStatus::Alert, "inside 24 s");
    assert_eq!(status(4.5), AtpaStatus::Alert, "already lost");
}

#[test]
fn descending_below_the_mva_triggers_a_terrain_alert() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(-30.0, 30.0), 90.0, 6000.0));
    world.transmit(1, vec![Clearance::Altitude(3000.0)]);
    run_until(&mut world, 120.0, &mut events, |w| !w.alerts.terrain.is_empty());
    assert!(events.iter().any(|e| matches!(e, SimEvent::Msaw { .. })));
}

#[test]
fn offered_aircraft_must_be_accepted_before_it_will_listen() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let mut ac = arrival(1, "NVA101", Vec2::new(70.0, 0.0), 270.0, 12000.0);
    ac.phase = Phase::Offered;
    world.add_aircraft(ac);

    world.transmit(1, vec![Clearance::Heading(180.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_pilot_line(&world).contains("not on your frequency"));

    assert!(world.accept_handoff(1));
    assert_eq!(world.find(1).unwrap().phase, Phase::Controlled);
    assert!(!world.accept_handoff(1), "can't accept twice");
}

#[test]
fn unaccepted_aircraft_entering_the_sector_is_penalised() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let mut ac = arrival(1, "NVA101", Vec2::new(54.0, 0.0), 270.0, 12000.0);
    ac.phase = Phase::Offered;
    world.add_aircraft(ac);
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).unwrap().entered_sector);
    assert!(events.iter().any(|e| matches!(e, SimEvent::LateAccept { .. })));
    assert_eq!(world.find(1).unwrap().phase, Phase::Controlled);
}

/// Traffic world with no arrival noise: forces one departure through Tower.
fn departure_world() -> World {
    let config = WorldConfig {
        seed: 3,
        traffic: true,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

#[test]
fn departure_climbs_checks_in_and_exits_with_a_handoff() {
    let mut world = departure_world();
    let mut events = Vec::new();
    let mut id = None;
    // Run until Tower releases a departure and it checks in with the player.
    for _ in 0..(1200.0 / TICK) as usize {
        world.tick();
        events.extend(world.drain_events());
        id = world
            .aircraft
            .iter()
            .find(|a| a.flow == Flow::Departure && a.phase == Phase::Controlled)
            .map(|a| a.id);
        if id.is_some() {
            break;
        }
    }
    let id = id.expect("a departure checked in");
    let exit_altitude = world.find(id).unwrap().exit_altitude;
    let callsign = world.find(id).unwrap().callsign.clone();
    assert!(world.find(id).unwrap().altitude >= 1500.0);
    assert!(world.find(id).unwrap().active_fix().is_some());

    world.transmit(id, vec![Clearance::Altitude(exit_altitude), Clearance::HandoffCenter]);
    events.clear();
    for _ in 0..(900.0 / TICK) as usize {
        world.tick();
        events.extend(world.drain_events());
        if world.find(id).is_none() {
            break;
        }
    }
    assert!(world.find(id).is_none(), "departure left the sim");
    assert!(events.iter().any(|e| matches!(e, SimEvent::Exited { callsign: c } if *c == callsign)));
    assert!(!events.iter().any(|e| matches!(
        e,
        SimEvent::SectorBust { callsign: c } | SimEvent::WrongExit { callsign: c } | SimEvent::LowExit { callsign: c }
        if *c == callsign
    )));
}

#[test]
fn departure_leaving_without_handoff_is_a_sector_bust() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let mut ac = arrival(1, "NVA101", Vec2::new(0.0, 51.0), 0.0, 13000.0);
    ac.flow = Flow::Departure;
    ac.entered_sector = true;
    world.add_aircraft(ac);
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).is_none());
    assert!(events.iter().any(|e| matches!(e, SimEvent::SectorBust { .. })));
}

#[test]
fn low_exit_and_wrong_exit_cost_points_but_still_count() {
    let mut world = quiet_world(0.0);
    let mut events = Vec::new();
    let mut ac = arrival(1, "NVA101", Vec2::new(0.0, 51.0), 0.0, 8000.0);
    ac.flow = Flow::Departure;
    ac.phase = Phase::OfferedToCenter;
    ac.entered_sector = true;
    ac.exit_fix = world.airport.fix_index("SEGUN"); // the far side of the sector
    ac.exit_altitude = 13000.0;
    world.add_aircraft(ac);
    run_until(&mut world, 120.0, &mut events, |w| w.find(1).is_none());
    assert!(events.iter().any(|e| matches!(e, SimEvent::WrongExit { .. })));
    assert!(events.iter().any(|e| matches!(e, SimEvent::LowExit { .. })));
    assert!(events.iter().any(|e| matches!(e, SimEvent::Exited { .. })));
}

#[test]
fn radar_only_updates_on_the_sweep() {
    let mut world = quiet_world(0.0);
    world.add_aircraft(arrival(1, "NVA101", Vec2::new(20.0, 20.0), 270.0, 6000.0));
    world.tick();
    let first = world.track(1).unwrap().pos;
    for _ in 0..8 {
        world.tick();
    }
    assert_eq!(world.track(1).unwrap().pos, first, "no new return mid-sweep");
    assert_ne!(world.find(1).unwrap().pos, first, "the aircraft itself keeps moving");
    for _ in 0..12 {
        world.tick();
    }
    let track = world.track(1).unwrap();
    assert_ne!(track.pos, first);
    assert_eq!(track.history.front(), Some(&first));
}

#[test]
fn same_seed_and_commands_give_identical_shifts() {
    let play = |seed: u64| {
        let config = WorldConfig { seed, ..Default::default() };
        let mut world = World::new(Airport::default_airport(), config);
        for step in 0..(600.0 / TICK) as usize {
            world.tick();
            // A fixed scripted input: accept everything, descend once a minute.
            if step % 240 == 0 {
                let offered: Vec<u32> = world
                    .aircraft
                    .iter()
                    .filter(|a| a.phase == Phase::Offered)
                    .map(|a| a.id)
                    .collect();
                for id in offered {
                    world.accept_handoff(id);
                    world.transmit(id, vec![Clearance::Altitude(8000.0)]);
                }
            }
        }
        world.state_digest()
    };
    assert_eq!(play(42), play(42));
    assert_ne!(play(42), play(43));
}

#[test]
fn unattended_shift_ends_without_panicking() {
    for seed in 0..5 {
        let config = WorldConfig { seed, ..Default::default() };
        let mut world = World::new(Airport::default_airport(), config);
        for _ in 0..(2000.0 / TICK) as usize {
            world.tick();
            world.drain_events();
        }
        assert!(world.ended.is_some(), "seed {seed}");
    }
}

#[test]
fn shift_completes_on_time_when_nothing_goes_wrong() {
    let config = WorldConfig { seed: 1, traffic: false, shift_seconds: 60.0, ..Default::default() };
    let mut world = World::new(Airport::default_airport(), config);
    for _ in 0..(61.0 / TICK) as usize {
        world.tick();
    }
    assert_eq!(world.ended, Some(ShiftEnd::Completed));
}

#[test]
fn traffic_ramps_up_over_the_shift() {
    // Count spawn announcements and queued departures in the first and last ten minutes.
    let config = WorldConfig {
        seed: 11,
        wind: Some(Wind { from: 270.0, speed: 10.0 }),
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    let mut ids_by_time: Vec<(f32, u32)> = Vec::new();
    for _ in 0..(1800.0 / TICK) as usize {
        let before = world.aircraft.len() + world.departures_waiting();
        world.tick();
        world.drain_events();
        // Keep the shift alive so spawning runs its whole course.
        world.score.separation_losses = 0;
        world.ended = None;
        let after = world.aircraft.len() + world.departures_waiting();
        if after > before {
            ids_by_time.push((world.time, 0));
        }
    }
    let early = ids_by_time.iter().filter(|(t, _)| *t < 600.0).count();
    let late = ids_by_time.iter().filter(|(t, _)| *t >= 1200.0).count();
    assert!(early >= 2, "early {early}");
    assert!(late > early, "early {early}, late {late}");
}
