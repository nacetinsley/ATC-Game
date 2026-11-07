//! RNAV and visual approaches: flying the published fixes or vectors to the
//! RNAV final, visual approaches alone and following traffic, the parallel
//! runway, parallel finals, minimums and ILS outages.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::approach::ApproachKind;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::replay::Recording;
use atcgame::sim::scoring::{GoAroundReason, SimEvent};
use atcgame::sim::sky::Sky;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

const IMC: Sky = Sky { ceiling: Some(700.0), visibility: 2.5 };

/// Harrowgate with the wind from the west: land 27L (south), depart 27R
/// (north). The player works Approach; Tower is automated.
fn harrowgate() -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        weather: WeatherKind::Variable,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    world.fix_sky(Sky::CLEAR);
    world.tick();
    world
}

/// KSUS landing west: 26L, with 26R alongside.
fn ksus() -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        wind: Some(Wind { from: 260.0, speed: 0.0 }),
        ..Default::default()
    };
    World::new(Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap(), config)
}

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn fix(world: &World, name: &str) -> usize {
    world.airport.fix_index(name).unwrap()
}

fn arrival(id: u32, ty: &str, pos: Vec2, heading: f32, altitude: f32, speed: f32) -> Aircraft {
    let mut ac = Aircraft::new(id, format!("NVA{id}"), Flow::Arrival, aircraft_type(ty).unwrap(), pos, heading, altitude, speed);
    ac.entered_sector = true;
    ac
}

/// `along` miles out from a runway's threshold and `cross` to the right of
/// its inbound course.
fn at(world: &World, runway: &str, along: f32, cross: f32) -> Vec2 {
    let r = &world.airport.runways[end(world, runway)];
    r.threshold() - heading_vec(r.heading) * along + heading_vec(r.heading + 90.0) * cross
}

/// Established on a runway's final course, with Approach.
fn on_final(world: &World, id: u32, ty: &str, runway: &str, along: f32) -> Aircraft {
    let e = end(world, runway);
    let r = &world.airport.runways[e];
    let altitude = world.airport.glideslope_altitude(e, along);
    let mut ac = arrival(id, ty, at(world, runway, along, 0.0), r.heading, altitude, 160.0);
    ac.phase = Phase::Glideslope;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(e);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac
}

fn said(world: &World, text: &str) -> bool {
    world.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.contains(text))
}

fn last_pilot_line(world: &World) -> String {
    world.log.iter().rev().find(|l| l.speaker == Speaker::Pilot).map(|l| l.text.clone()).unwrap_or_default()
}

fn run(world: &mut World, seconds: f32, events: &mut Vec<SimEvent>) {
    for _ in 0..(seconds / TICK).ceil() as usize {
        world.tick();
        events.extend(world.drain_events());
    }
}

/// Fly every arrival the player has cleared, handing each to Tower as soon
/// as it can go, until `id` lands. Fails on a go-around.
fn fly_to_landing(world: &mut World, id: u32, max_seconds: f32, events: &mut Vec<SimEvent>) {
    for _ in 0..(max_seconds / TICK).ceil() as usize {
        if events.iter().any(|e| matches!(e, SimEvent::Landed { callsign, .. } if *callsign == format!("NVA{id}"))) {
            return;
        }
        if let Some(e) = events.iter().find(|e| matches!(e, SimEvent::GoAround { .. })) {
            panic!("went around: {e:?}");
        }
        hand_off_ready(world);
        world.tick();
        events.extend(world.drain_events());
    }
    panic!("NVA{id} never landed: {events:?}");
}

fn hand_off_ready(world: &mut World) {
    let ready: Vec<u32> = world
        .aircraft
        .iter()
        .filter(|a| {
            let visual = a.phase == Phase::ClearedApproach && matches!(a.lateral, Lateral::Visual(_));
            matches!(a.phase, Phase::Localizer | Phase::Glideslope) || visual
        })
        .map(|a| a.id)
        .collect();
    for id in ready {
        if !world.log.iter().any(|l| l.speaker == Speaker::Controller && l.text.starts_with(&format!("NVA{id}, contact tower"))) {
            world.transmit(id, vec![Clearance::HandoffTower]);
        }
    }
}

/// Send a call and return the pilot's reply.
fn answer(world: &mut World, id: u32, clearances: Vec<Clearance>) -> String {
    assert!(world.transmit(id, clearances));
    let mut events = Vec::new();
    run(world, 7.0, &mut events);
    last_pilot_line(world)
}

#[test]
fn harrowgate_offers_rnav_and_visual_on_both_westbound_runways() {
    let world = harrowgate();
    let (l, r) = (end(&world, "27L"), end(&world, "27R"));
    assert_eq!(world.landing_runways(), vec![l, r]);
    let options = world.approach_options();
    assert_eq!(&options[..3], &[(ApproachKind::Ils, l), (ApproachKind::Rnav, l), (ApproachKind::Visual, l)]);
    assert!(options.contains(&(ApproachKind::Rnav, r)) && options.contains(&(ApproachKind::Visual, r)));
    assert!(world.atis_text().contains("Visual approaches in use"));
}

#[test]
fn rnav_from_a_transition_fix_steps_down_and_lands() {
    let mut world = harrowgate();
    let brick = fix(&world, "BRICK");
    world.add_aircraft(arrival(1, "B738", Vec2::new(30.0, -18.0), 290.0, 5000.0, 210.0));
    let l = end(&world, "27L");
    let line = answer(&mut world, 1, vec![Clearance::DirectFix(brick), Clearance::ClearedRnav(l)]);
    assert!(!line.starts_with("unable") && line.contains("cleared RNAV runway 27L approach"), "{line}");
    let ac = world.find(1).unwrap();
    assert_eq!(ac.phase, Phase::ClearedApproach);
    assert_eq!(ac.route, vec![brick, fix(&world, "SOLAN"), fix(&world, "TELLO")]);

    // Down to 3,000 for SOLAN while on the transition, and no lower.
    let mut events = Vec::new();
    let mut lowest_before_solan = f32::MAX;
    for _ in 0..(600.0 / TICK) as usize {
        let ac = world.find(1).unwrap();
        if ac.lateral != Lateral::Route {
            break;
        }
        if ac.active_fix() == Some(fix(&world, "SOLAN")) {
            lowest_before_solan = lowest_before_solan.min(ac.altitude);
        }
        world.tick();
        events.extend(world.drain_events());
    }
    assert!((lowest_before_solan - 3000.0).abs() < 1.0, "{lowest_before_solan}");
    fly_to_landing(&mut world, 1, 600.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::Msaw { .. })), "{events:?}");
}

#[test]
fn rnav_on_vectors_joins_the_final_and_lands() {
    let mut world = harrowgate();
    world.add_aircraft(arrival(1, "B738", at(&world, "27L", 14.0, 2.6), 240.0, 3000.0, 200.0));
    let l = end(&world, "27L");
    let line = answer(&mut world, 1, vec![Clearance::ClearedRnav(l)]);
    assert!(!line.starts_with("unable") && line.contains("cleared RNAV"), "{line}");
    let mut events = Vec::new();
    fly_to_landing(&mut world, 1, 600.0, &mut events);
    assert_eq!(world.find(1).map(|a| a.approach), Some(ApproachKind::Rnav));
}

#[test]
fn rnav_to_the_departure_runway_mixes_with_its_departures() {
    let mut world = harrowgate();
    let alder = fix(&world, "ALDER");
    let r = end(&world, "27R");
    world.add_aircraft(arrival(1, "B738", Vec2::new(28.0, 18.0), 250.0, 5000.0, 210.0));
    // A departure waiting for 27R, which the automated Tower has to fit in.
    let map = world.airport.surface.as_ref().unwrap();
    let hold = map.departure_hold(r).unwrap();
    let mut dep = Aircraft::new(2, "NVA2", Flow::Departure, aircraft_type("B738").unwrap(), map.nodes[hold].pos, 0.0, 0.0, 0.0);
    dep.on_ground = true;
    dep.phase = Phase::Taxiing;
    dep.taxi = Some(Taxi::at(hold));
    dep.departure_end = Some(r);
    dep.surface_freq = Position::Local;
    dep.target_altitude = world.airport.departure_initial_altitude;
    dep.target_speed = 250.0;
    world.add_aircraft(dep);

    answer(&mut world, 1, vec![Clearance::DirectFix(alder), Clearance::ClearedRnav(r)]);
    assert_eq!(world.find(1).unwrap().approach_runway, Some(r));
    let mut events = Vec::new();
    fly_to_landing(&mut world, 1, 900.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::RunwaySeparation { .. })), "{events:?}");
}

#[test]
fn a_visual_is_refused_in_cloud_and_beyond_sight_of_the_field() {
    let mut world = harrowgate();
    let l = end(&world, "27L");
    world.add_aircraft(arrival(1, "B738", at(&world, "27L", 8.0, -3.0), 0.0, 3000.0, 200.0));
    world.fix_sky(IMC);
    let line = answer(&mut world, 1, vec![Clearance::ClearedVisual { runway: l, follow: None }]);
    assert!(line.contains("not in visual conditions"), "{line}");

    // Five miles' visibility: the field, ten miles away, can't be seen.
    world.fix_sky(Sky { ceiling: Some(5000.0), visibility: 5.0 });
    world.find_mut(1).unwrap().pos = at(&world, "27L", 10.0, -3.0);
    let line = answer(&mut world, 1, vec![Clearance::ClearedVisual { runway: l, follow: None }]);
    assert!(line.contains("field not in sight"), "{line}");
    assert_eq!(world.find(1).unwrap().phase, Phase::Controlled);
}

#[test]
fn a_visual_from_a_base_closer_than_the_ils_allows_lands() {
    let mut world = harrowgate();
    let l = end(&world, "27L");
    world.add_aircraft(arrival(1, "B738", at(&world, "27L", 4.0, -4.0), 0.0, 2000.0, 180.0));
    let line = answer(&mut world, 1, vec![Clearance::ClearedIls(l)]);
    assert!(line.contains("position not suitable"), "{line}");
    let line = answer(&mut world, 1, vec![Clearance::ClearedVisual { runway: l, follow: None }]);
    assert!(!line.starts_with("unable") && line.contains("cleared visual approach runway 27L"), "{line}");
    let mut events = Vec::new();
    fly_to_landing(&mut world, 1, 300.0, &mut events);
}

#[test]
fn a_visual_far_too_high_goes_around_unstable() {
    let mut world = harrowgate();
    let l = end(&world, "27L");
    world.add_aircraft(arrival(1, "B738", at(&world, "27L", 4.0, -4.0), 0.0, 9000.0, 180.0));
    answer(&mut world, 1, vec![Clearance::ClearedVisual { runway: l, follow: None }, Clearance::HandoffTower]);
    let mut events = Vec::new();
    for _ in 0..(300.0 / TICK) as usize {
        world.tick();
        events.extend(world.drain_events());
        if events.iter().any(|e| matches!(e, SimEvent::GoAround { .. } | SimEvent::Landed { .. })) {
            break;
        }
    }
    assert!(
        events.iter().any(|e| matches!(e, SimEvent::GoAround { reason: GoAroundReason::Unstable, .. })),
        "{events:?}"
    );
}

#[test]
fn following_traffic_on_a_visual_closes_inside_three_miles_without_a_loss() {
    let mut world = harrowgate();
    let l = end(&world, "27L");
    let mut leader = on_final(&world, 1, "B738", "27L", 7.0);
    leader.target_speed = 160.0;
    world.add_aircraft(leader);
    world.add_aircraft(arrival(2, "B738", at(&world, "27L", 10.0, -1.0), 280.0, 3000.0, 220.0));
    let line = answer(
        &mut world,
        2,
        vec![Clearance::ClearedVisual { runway: l, follow: Some((1, "B738 on final".into())) }],
    );
    assert!(line.starts_with("follow the B738 on final, cleared visual approach runway 27L"), "{line}");
    assert_eq!(world.find(2).unwrap().visual_follow, Some(1));

    let mut events = Vec::new();
    let mut closest = f32::MAX;
    let mut slowed = false;
    for _ in 0..(400.0 / TICK) as usize {
        let (Some(a), Some(b)) = (world.find(1), world.find(2)) else { break };
        if a.on_ground || b.on_ground {
            break;
        }
        closest = closest.min(a.pos.distance(b.pos));
        slowed |= b.target_speed < 220.0;
        hand_off_ready(&mut world);
        world.tick();
        events.extend(world.drain_events());
    }
    assert!(closest < 3.0, "never closer than {closest}");
    assert!(slowed, "the follower never slowed for its leader");
    assert!(!events.iter().any(|e| matches!(
        e,
        SimEvent::LossOfSeparation { .. } | SimEvent::ConflictAlert { .. } | SimEvent::WakeViolation { .. }
    )), "{events:?}");
}

#[test]
fn wake_still_applies_behind_a_heavy_on_a_visual() {
    let mut world = harrowgate();
    let mut heavy = on_final(&world, 1, "B77W", "27L", 4.0);
    heavy.phase = Phase::TowerHandedOff;
    world.add_aircraft(heavy);
    let mut follower = on_final(&world, 2, "B738", "27L", 7.5);
    follower.visual_follow = Some(1);
    follower.approach = ApproachKind::Visual;
    world.add_aircraft(follower);
    world.tick();
    assert!(world.alerts.wake.contains(&(1, 2)), "3.5 miles behind a heavy");

    // Behind another large jet, 2.5 miles is room enough on a visual, though
    // not without one.
    let mut world = harrowgate();
    let mut leader = on_final(&world, 1, "B738", "27L", 4.0);
    leader.phase = Phase::TowerHandedOff;
    world.add_aircraft(leader);
    let mut follower = on_final(&world, 2, "B738", "27L", 6.5);
    follower.visual_follow = Some(1);
    world.add_aircraft(follower);
    world.tick();
    assert!(world.alerts.wake.is_empty());
    world.find_mut(2).unwrap().visual_follow = None;
    world.tick();
    assert!(world.alerts.wake.contains(&(1, 2)));
}

#[test]
fn independent_parallel_finals_need_no_separation() {
    let mut world = harrowgate();
    let mut events = Vec::new();
    world.add_aircraft(on_final(&world, 1, "B738", "27L", 14.0));
    world.add_aircraft(on_final(&world, 2, "B738", "27R", 14.0));
    run(&mut world, 5.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::LossOfSeparation { .. })), "{events:?}");
}

#[test]
fn dependent_parallel_finals_need_a_mile_and_a_half_diagonal() {
    let mut world = ksus();
    let spacing = world.airport.centreline_spacing_ft(end(&world, "26L"), end(&world, "26R"));
    assert!((2500.0..4300.0).contains(&spacing), "{spacing}");
    let mut events = Vec::new();
    world.add_aircraft(on_final(&world, 1, "B738", "26L", 14.0));
    world.add_aircraft(on_final(&world, 2, "B738", "26R", 17.0));
    run(&mut world, 2.0, &mut events);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::LossOfSeparation { .. })), "{events:?}");

    let mut world = ksus();
    let mut events = Vec::new();
    world.add_aircraft(on_final(&world, 1, "B738", "26L", 14.0));
    world.add_aircraft(on_final(&world, 2, "B738", "26R", 14.5));
    run(&mut world, 2.0, &mut events);
    assert!(events.iter().any(|e| matches!(e, SimEvent::LossOfSeparation { .. })), "{events:?}");
}

#[test]
fn ksus_gives_rnav_to_26r_which_has_no_ils() {
    let mut world = ksus();
    let r = end(&world, "26R");
    world.add_aircraft(arrival(1, "B738", at(&world, "26R", 12.0, -2.0), 278.0, 3000.0, 200.0));
    let line = answer(&mut world, 1, vec![Clearance::ClearedIls(r)]);
    assert!(line.contains("no ILS to runway 26R"), "{line}");
    let line = answer(&mut world, 1, vec![Clearance::ClearedRnav(r)]);
    assert!(!line.starts_with("unable") && line.contains("cleared RNAV runway 26R approach"), "{line}");
    let mut events = Vec::new();
    fly_to_landing(&mut world, 1, 600.0, &mut events);
}

#[test]
fn below_rnav_minimums_only_the_ils_will_do() {
    let mut world = harrowgate();
    world.fix_sky(Sky { ceiling: Some(250.0), visibility: 0.75 });
    world.add_aircraft(arrival(1, "B738", at(&world, "27L", 14.0, 2.6), 240.0, 3000.0, 200.0));
    let l = end(&world, "27L");
    let line = answer(&mut world, 1, vec![Clearance::ClearedRnav(l)]);
    assert!(line.contains("below our minimums"), "{line}");
    let line = answer(&mut world, 1, vec![Clearance::ClearedIls(l)]);
    assert!(!line.starts_with("unable") && line.contains("cleared ILS"), "{line}");
}

#[test]
fn when_the_ils_fails_arrivals_break_off_and_the_rnav_takes_over() {
    let mut world = harrowgate();
    let l = end(&world, "27L");
    world.schedule_ils_outage(5.0, 2000.0);
    world.add_aircraft(on_final(&world, 1, "B738", "27L", 12.0));
    let mut short = on_final(&world, 2, "B738", "27L", 3.0);
    short.phase = Phase::TowerHandedOff;
    short.cleared_to_land = true;
    world.add_aircraft(short);
    let mut events = Vec::new();
    run(&mut world, 6.0, &mut events);

    assert_eq!(world.ils_line().as_deref(), Some("ILS 27L OTS"));
    assert!(world.atis_text().contains("ILS runway 27L out of service"), "{}", world.atis_text());
    assert!(said(&world, "localizer's gone"));
    let first = world.find(1).unwrap();
    assert_eq!(first.phase, Phase::Controlled);
    assert!(matches!(first.lateral, Lateral::Heading(_)));
    assert!(events.iter().any(|e| matches!(e, SimEvent::GoAround { reason: GoAroundReason::IlsOutage, .. })));
    assert!(!world.approach_options().contains(&(ApproachKind::Ils, l)));

    world.find_mut(1).unwrap().lateral = Lateral::Heading(240.0);
    world.find_mut(1).unwrap().heading = 240.0;
    let line = answer(&mut world, 1, vec![Clearance::ClearedIls(l)]);
    assert!(line.contains("the ILS is out of service"), "{line}");
    let line = answer(&mut world, 1, vec![Clearance::ClearedRnav(l)]);
    assert!(!line.starts_with("unable") && line.contains("cleared RNAV"), "{line}");
}

#[test]
fn a_shift_with_the_new_clearances_replays_the_same() {
    let config = WorldConfig {
        seed: 7,
        traffic: true,
        wind: Some(Wind { from: 270.0, speed: 8.0 }),
        staffing: Staffing::APPROACH,
        shift_seconds: 8.0 * 60.0,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    let mut recording = Recording::new(&world);
    let mut step = 0u32;
    while world.ended.is_none() {
        if step.is_multiple_of(80) {
            let ids: Vec<(u32, Phase)> = world.aircraft.iter().map(|a| (a.id, a.phase)).collect();
            for (n, (id, phase)) in ids.into_iter().enumerate() {
                let l = world.arrival_runway();
                match (phase, n % 3) {
                    (Phase::Offered, _) => {
                        world.accept_handoff(id);
                    }
                    (Phase::Controlled, 0) => {
                        world.transmit(id, vec![Clearance::ClearedRnav(l)]);
                    }
                    (Phase::Controlled, 1) => {
                        world.transmit(id, vec![Clearance::ClearedVisual { runway: l, follow: None }]);
                    }
                    _ => {}
                }
            }
        }
        world.tick();
        recording.update(&world);
        step += 1;
    }
    recording.finish(&world);
    let mut rebuilt = Recording::from_start(World::new(Airport::default_airport(), config), recording.commands.clone());
    while !rebuilt.rebuild_step(500) {}
    assert_eq!(rebuilt.last().state_digest(), world.state_digest());
}
