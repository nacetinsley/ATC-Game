//! Emergencies: what each kind does, how the controllers cope, and the score.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::emergency::{DisabledCause, Emergency, EmergencyKind, EmergencyRate, Trigger};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::Staffing;
use atcgame::sim::scoring::{GoAroundReason, SimEvent};
use atcgame::sim::traffic::Busyness;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;
use std::collections::BTreeMap;

// Placing traffic by hand.

/// Harrowgate on the 27s (land 27L, depart 27R), with no traffic of its own.
fn harrowgate(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        wind: Some(Wind {
            from: 270.0,
            speed: 0.0,
        }),
        staffing,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn emergency(kind: EmergencyKind, trigger: Trigger) -> Option<Emergency> {
    Some(Emergency::new(kind, trigger, 120, 90))
}

/// An arrival with Approach, in the sector.
fn arrival(id: u32, name: &str, pos: Vec2, heading: f32, altitude: f32) -> Aircraft {
    let mut ac = Aircraft::new(
        id,
        name,
        Flow::Arrival,
        aircraft_type("B738").unwrap(),
        pos,
        heading,
        altitude,
        250.0,
    );
    ac.entered_sector = true;
    ac
}

/// An arrival established on final, talking to Tower.
fn final_arrival(world: &World, id: u32, name: &str, runway_id: &str, miles: f32) -> Aircraft {
    let rwy = end(world, runway_id);
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = arrival(id, name, pos, runway.heading, altitude);
    ac.ias = 160.0;
    ac.target_speed = 160.0;
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac
}

/// Queue a departure through the normal path and let it be placed.
fn spawn_departure(world: &mut World) -> u32 {
    let route = world
        .airport
        .route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("B738").unwrap(), route, 10000.0);
    world.tick();
    world
        .aircraft
        .iter()
        .find(|a| a.flow == Flow::Departure)
        .expect("placed")
        .id
}

fn run(world: &mut World, seconds: f32, events: &mut Vec<SimEvent>) {
    for _ in 0..(seconds / TICK).ceil() as usize {
        world.tick();
        events.extend(world.drain_events());
    }
}

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
    panic!(
        "condition not met within {max_seconds} s; events: {events:?}; log: {:?}",
        tail(world)
    );
}

fn last_line(world: &World, speaker: Speaker) -> String {
    world
        .log
        .iter()
        .rev()
        .find(|l| l.speaker == speaker)
        .map(|l| l.text.clone())
        .unwrap_or_default()
}

fn declared(world: &World, id: u32) -> bool {
    world.find(id).is_some_and(|a| a.in_emergency())
}

fn landed(events: &[SimEvent], name: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, SimEvent::EmergencyLanded { callsign, .. } if callsign == name))
}

// Engine failure.

#[test]
fn an_engine_failure_after_takeoff_levels_off_slowly_and_comes_back_to_land() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let id = spawn_departure(&mut world);
    world.find_mut(id).unwrap().emergency =
        emergency(EmergencyKind::EngineFailure, Trigger::AtHeight(500.0));
    run_until(&mut world, 900.0, &mut events, |w| declared(w, id));
    let elevation = world.airport.elevation;
    let ac = world.find(id).unwrap();
    assert_eq!(ac.flow, Flow::Arrival);
    assert!(ac.exit_fix.is_none() && ac.route.is_empty());
    assert!(
        ac.target_altitude >= elevation + 3000.0 && ac.target_altitude < elevation + 5000.0,
        "{}",
        ac.target_altitude
    );
    let line = last_line(&world, Speaker::Emergency);
    assert!(
        line.starts_with("Mayday, mayday, mayday")
            && line.contains("engine failure, request return"),
        "{line}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::EmergencyDeclared {
            kind: EmergencyKind::EngineFailure,
            ..
        }
    )));

    // A third of the climb, and no faster than approach speed plus 70.
    let (vref, climb) = (ac.ty.vref, ac.ty.climb_fpm);
    run(&mut world, 20.0, &mut events);
    let ac = world.find(id).unwrap();
    assert!(
        ac.vertical_speed <= climb * 0.35 + 1.0,
        "{} fpm",
        ac.vertical_speed
    );
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(id).unwrap().phase == Phase::Controlled
    });
    run(&mut world, 60.0, &mut events);
    assert!(world.find(id).unwrap().ias <= vref + 71.0);

    // It won't climb high or speed up.
    world.transmit(id, vec![Clearance::Altitude(elevation + 8000.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(
        last_line(&world, Speaker::Pilot).contains("single engine"),
        "{}",
        last_line(&world, Speaker::Pilot)
    );
    world.transmit(id, vec![Clearance::Speed(300.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_line(&world, Speaker::Pilot).contains("single engine"));

    // Vectored back to the ILS, it lands like any arrival, and earns the bonus.
    let rwy = end(&world, "27L");
    let ac = world.find_mut(id).unwrap();
    ac.pos = Vec2::new(18.0, 4.0);
    ac.heading = 240.0;
    ac.lateral = Lateral::Heading(240.0);
    ac.altitude = 3000.0_f32.max(ac.altitude.min(4000.0));
    ac.target_altitude = ac.altitude;
    world.transmit(id, vec![Clearance::ClearedIls(rwy)]);
    run_until(&mut world, 300.0, &mut events, |w| {
        matches!(
            w.find(id).unwrap().phase,
            Phase::Localizer | Phase::Glideslope
        )
    });
    world.transmit(id, vec![Clearance::HandoffTower]);
    run_until(&mut world, 600.0, &mut events, |w| {
        w.find(id).is_none_or(|a| a.on_ground)
    });
    assert!(
        landed(
            &events,
            &world
                .find(id)
                .map(|a| a.callsign.clone())
                .unwrap_or_default()
        ) || events
            .iter()
            .any(|e| matches!(e, SimEvent::EmergencyLanded { .. }))
    );
    assert!(
        world
            .score
            .incidents
            .iter()
            .any(|i| i.text.contains("landed safely with a engine failure") && i.points > 0),
        "{:?}",
        world.score.incidents
    );
}

#[test]
fn with_approach_automated_an_engine_failure_returns_on_final() {
    let mut world = harrowgate(Staffing::NONE);
    let mut events = Vec::new();
    let id = spawn_departure(&mut world);
    let name = world.find(id).unwrap().callsign.clone();
    world.find_mut(id).unwrap().emergency =
        emergency(EmergencyKind::BirdStrike, Trigger::AtHeight(800.0));
    run_until(&mut world, 900.0, &mut events, |w| declared(w, id));
    // The automated Approach takes it off the board, then brings it back.
    run_until(&mut world, 600.0, &mut events, |w| {
        w.emergencies.returning().any(|a| a.id == id)
    });
    run_until(&mut world, 900.0, &mut events, |w| {
        w.find(id).is_some_and(|a| a.phase == Phase::TowerHandedOff)
    });
    run_until(&mut world, 600.0, &mut events, |w| {
        w.find(id).is_none_or(|a| a.on_ground)
    });
    assert!(landed(&events, &name), "{events:?}");
    // Nobody is working: no points either way.
    assert!(
        world.score.incidents.is_empty(),
        "{:?}",
        world.score.incidents
    );
}

// Priority landings.

#[test]
fn low_fuel_wont_slow_down_away_from_the_approach() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let mut ac = arrival(1, "ORN501", Vec2::new(20.0, 10.0), 240.0, 6000.0);
    ac.emergency = emergency(EmergencyKind::LowFuel, Trigger::OnCheckIn);
    world.add_aircraft(ac);
    run(&mut world, 1.0, &mut events);
    assert!(last_line(&world, Speaker::Emergency).contains("emergency fuel, unable to hold"));
    world.transmit(1, vec![Clearance::Speed(180.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(
        last_line(&world, Speaker::Pilot).contains("unable")
            && last_line(&world, Speaker::Pilot).contains("minimum fuel")
    );
    world.transmit(1, vec![Clearance::Speed(220.0)]);
    run(&mut world, 7.0, &mut events);
    assert_eq!(world.find(1).unwrap().target_speed, 220.0);
}

#[test]
fn low_fuel_diverts_after_its_second_go_around() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let rwy = end(&world, "27L");
    let mut ac = arrival(1, "ORN501", Vec2::new(15.0, 3.0), 240.0, 3000.0);
    let mut e = Emergency::new(EmergencyKind::LowFuel, Trigger::OnCheckIn, 120, 20);
    e.go_arounds = 1;
    ac.emergency = Some(e);
    world.add_aircraft(ac);
    // Never handed to Tower: it goes around, and this time it can't stay.
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);
    run_until(&mut world, 400.0, &mut events, |w| w.find(1).is_none());
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround {
            reason: GoAroundReason::LateHandoff,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::EmergencyDiverted { .. }))
    );
    assert!(last_line(&world, Speaker::Emergency).contains("diverting"));
    assert!(
        world
            .score
            .incidents
            .iter()
            .any(|i| i.text.contains("diverted") && i.points < 0)
    );
}

#[test]
fn a_medical_arrival_asks_for_paramedics_with_a_pan_pan() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let mut ac = arrival(1, "MRD722", Vec2::new(20.0, 10.0), 240.0, 6000.0);
    ac.emergency = emergency(EmergencyKind::Medical, Trigger::OnCheckIn);
    world.add_aircraft(ac);
    run(&mut world, 1.0, &mut events);
    let line = last_line(&world, Speaker::Emergency);
    assert!(
        line.starts_with("Pan-pan, pan-pan, pan-pan, Approach, MRD722")
            && line.contains("paramedics"),
        "{line}"
    );
}

// Pressurization.

#[test]
fn a_pressurization_failure_descends_fast_ignores_altitudes_and_costs_no_separation() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let mut ac = arrival(1, "NVA330", Vec2::new(30.0, 20.0), 240.0, 17000.0);
    ac.emergency = emergency(EmergencyKind::Pressurization, Trigger::AfterAccept(5.0));
    world.add_aircraft(ac);
    // Level traffic below, 2 NM to the side, that it descends straight through.
    world.add_aircraft(arrival(2, "TRL440", Vec2::new(31.0, 21.7), 240.0, 13000.0));
    run_until(&mut world, 30.0, &mut events, |w| declared(w, 1));
    assert!(last_line(&world, Speaker::Emergency).contains("emergency descent"));
    let start = world.find(1).unwrap().altitude;
    run(&mut world, 30.0, &mut events);
    assert!(
        start - world.find(1).unwrap().altitude > 2500.0,
        "{}",
        world.find(1).unwrap().altitude
    );
    world.transmit(1, vec![Clearance::Altitude(15000.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_line(&world, Speaker::Pilot).contains("emergency descent"));
    run_until(&mut world, 120.0, &mut events, |w| {
        !w.find(1).unwrap().emergency.as_ref().unwrap().descending
    });
    let mva = world.airport.mva_at(world.find(1).unwrap().pos);
    let level = world.find(1).unwrap().target_altitude;
    assert!(
        level >= 10000.0 && level >= mva && level < 10000.0_f32.max(mva) + 100.0,
        "{level}"
    );
    assert!(last_line(&world, Speaker::Emergency).contains("level"));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::LossOfSeparation { .. })),
        "{events:?}"
    );
    // After the descent it takes altitudes again.
    world.transmit(1, vec![Clearance::Altitude(8000.0)]);
    run(&mut world, 7.0, &mut events);
    assert_eq!(world.find(1).unwrap().target_altitude, 8000.0);
}

// Radio failure.

#[test]
fn a_radio_failure_ignores_calls_and_flies_itself_onto_the_ils_to_land() {
    let mut world = harrowgate(Staffing::APPROACH);
    let mut events = Vec::new();
    let mut ac = arrival(1, "BRZ610", Vec2::new(20.0, 14.0), 240.0, 6000.0);
    ac.emergency = emergency(EmergencyKind::RadioFailure, Trigger::OnCheckIn);
    world.add_aircraft(ac);
    run(&mut world, 1.0, &mut events);
    assert!(last_line(&world, Speaker::Emergency).contains("squawking 7600"));
    world.transmit(1, vec![Clearance::Heading(90.0)]);
    run(&mut world, 7.0, &mut events);
    assert!(last_line(&world, Speaker::System).contains("No reply from BRZ610"));
    assert_ne!(world.find(1).unwrap().lateral, Lateral::Heading(90.0));
    // Lost-comms procedure: base point, intercept, and the automated Tower's light gun.
    run_until(&mut world, 900.0, &mut events, |w| {
        w.find(1).is_none_or(|a| a.on_ground)
    });
    assert!(landed(&events, "BRZ610"), "{events:?}");
    assert!(
        world.score.incidents.iter().all(|i| i.points > 0),
        "{:?}",
        world.score.incidents
    );
}

#[test]
fn tower_lands_a_radio_failure_with_a_steady_green_light() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    let mut ac = final_arrival(&world, 1, "BRZ610", "27L", 5.0);
    ac.emergency = emergency(EmergencyKind::RadioFailure, Trigger::OnCheckIn);
    world.add_aircraft(ac);
    run(&mut world, 1.0, &mut events);
    world.transmit(1, vec![Clearance::ClearedToLand(end(&world, "27L"))]);
    run(&mut world, 7.0, &mut events);
    assert!(last_line(&world, Speaker::System).contains("steady green light to BRZ610"));
    run_until(&mut world, 300.0, &mut events, |w| {
        w.find(1).is_none_or(|a| a.on_ground)
    });
    assert!(landed(&events, "BRZ610"), "{events:?}");
}

#[test]
fn a_radio_failure_without_a_light_goes_around_silently() {
    let mut world = harrowgate(Staffing::LOCAL);
    let mut events = Vec::new();
    let mut ac = final_arrival(&world, 1, "BRZ610", "27L", 4.0);
    ac.emergency = emergency(EmergencyKind::RadioFailure, Trigger::OnCheckIn);
    world.add_aircraft(ac);
    run_until(&mut world, 200.0, &mut events, |w| {
        w.find(1).unwrap().phase == Phase::GoAround
    });
    assert!(last_line(&world, Speaker::System).contains("BRZ610 going around"));
    assert!(!last_line(&world, Speaker::Pilot).contains("BRZ610"));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::EmergencyGoAround { .. }))
    );
}

// Disabled on the runway.

#[test]
fn a_disabled_aircraft_closes_its_runway_until_the_tow_clears_it() {
    let mut world = harrowgate(Staffing::NONE);
    let mut events = Vec::new();
    let mut ac = final_arrival(&world, 1, "CWN905", "27L", 4.0);
    ac.emergency = emergency(
        EmergencyKind::Disabled(DisabledCause::Gear),
        Trigger::OnCheckIn,
    );
    world.add_aircraft(ac);
    let atis = world.atis_text();
    run_until(&mut world, 400.0, &mut events, |w| {
        w.emergencies.closure.is_some()
    });
    // It stopped on the runway, and everything moves to the parallel.
    let stopped = world.find(1).unwrap();
    assert!(stopped.on_ground && stopped.ias < 1.0 && stopped.phase == Phase::Rollout);
    assert_eq!(world.closed_runway_name().as_deref(), Some("27L/09R"));
    assert_eq!(world.airport.runways[world.arrival_runway()].id, "27R");
    assert_eq!(world.airport.runways[world.departure_runway()].id, "27R");
    let closed_atis = world.atis_text();
    assert_ne!(closed_atis, atis);
    assert!(
        closed_atis.contains("Runway 27L/09R closed."),
        "{closed_atis}"
    );
    assert!(
        world
            .closure_line()
            .unwrap()
            .starts_with("RWY 27L/09R CLSD")
    );

    // A new arrival lands on the parallel meanwhile.
    world.add_aircraft(final_arrival(&world, 2, "TRL220", "27R", 6.0));
    run_until(&mut world, 700.0, &mut events, |w| {
        w.emergencies.closure.is_none()
    });
    assert!(world.find(1).is_none());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::Landed { callsign, .. } if callsign == "TRL220"))
    );
    assert_eq!(world.airport.runways[world.arrival_runway()].id, "27L");
    assert!(!world.atis_text().contains("closed"));
    assert_ne!(world.atis_text(), closed_atis);
    assert!(
        world.score.incidents.is_empty(),
        "{:?}",
        world.score.incidents
    );
}

// The automated Tower.

#[test]
fn the_automated_tower_holds_departures_for_an_emergency_about_to_land() {
    // Arrival on 9 NM final to the departure runway, departure ready to go.
    let first_takeoff_vs_landing = |with_emergency: bool| {
        let mut world = harrowgate(Staffing::NONE);
        let mut events = Vec::new();
        let dep = spawn_departure(&mut world);
        run_until(&mut world, 900.0, &mut events, |w| {
            w.find(dep).is_some_and(|a| {
                a.phase == Phase::Taxiing && a.taxi.as_ref().is_some_and(|t| t.done())
            })
        });
        let mut ac = final_arrival(&world, 100, "MRD100", "27R", 9.0);
        if with_emergency {
            ac.emergency = emergency(EmergencyKind::Medical, Trigger::OnCheckIn);
        }
        world.add_aircraft(ac);
        let mut took_off = None;
        let mut landed = None;
        for _ in 0..(400.0 / TICK) as usize {
            world.tick();
            events.extend(world.drain_events());
            if took_off.is_none() && world.find(dep).is_some_and(|a| a.rolling() || !a.on_ground) {
                took_off = Some(world.time);
            }
            if landed.is_none() && world.find(100).is_none_or(|a| a.on_ground) {
                landed = Some(world.time);
            }
        }
        (took_off.unwrap_or(f32::MAX), landed.expect("landed"))
    };
    let (takeoff, landing) = first_takeoff_vs_landing(false);
    assert!(
        takeoff < landing,
        "without an emergency the departure goes first: {takeoff} {landing}"
    );
    let (takeoff, landing) = first_takeoff_vs_landing(true);
    assert!(
        takeoff > landing,
        "held for the emergency: {takeoff} {landing}"
    );
}

// The rate setting.

#[test]
fn emergencies_dont_change_the_traffic() {
    let offers = |rate: EmergencyRate| {
        let config = WorldConfig {
            seed: 9,
            emergencies: rate,
            ..Default::default()
        };
        let mut world = World::new(Airport::default_airport(), config);
        for _ in 0..(1800.0 / TICK) as usize {
            world.tick();
            world.drain_events();
            world.ended = None;
        }
        let offered: Vec<String> = world
            .log
            .iter()
            .filter(|l| l.text.starts_with("Center: offering"))
            .map(|l| l.text.clone())
            .collect();
        offered
    };
    let off = offers(EmergencyRate::Off);
    assert!(off.len() > 5);
    assert_eq!(off, offers(EmergencyRate::Intense));
}

#[test]
fn off_means_no_emergencies() {
    let config = WorldConfig {
        seed: 4,
        staffing: Staffing::NONE,
        busyness: Busyness::Busy,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    while world.ended.is_none() {
        world.tick();
        assert!(
            !world
                .drain_events()
                .iter()
                .any(|e| matches!(e, SimEvent::EmergencyDeclared { .. }))
        );
    }
}

// Whole shifts.

fn ksus() -> Airport {
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

/// What happened to each emergency in a shift.
#[derive(Default, Debug)]
struct Outcomes {
    declared: BTreeMap<String, EmergencyKind>,
    landed: Vec<String>,
    diverted: Vec<String>,
}

/// Every position automated, emergencies on.
fn automated_shift(airport: Airport, config: WorldConfig) -> (World, Outcomes) {
    let mut world = World::new(
        airport,
        WorldConfig {
            staffing: Staffing::NONE,
            ..config
        },
    );
    let mut outcomes = Outcomes::default();
    while world.ended.is_none() {
        world.tick();
        for event in world.drain_events() {
            match event {
                SimEvent::EmergencyDeclared { callsign, kind } => {
                    outcomes.declared.insert(callsign, kind);
                }
                SimEvent::EmergencyLanded { callsign, .. } => outcomes.landed.push(callsign),
                SimEvent::EmergencyDiverted { callsign } => outcomes.diverted.push(callsign),
                _ => {}
            }
        }
    }
    (world, outcomes)
}

/// Run automated shifts and check each is clean, and that every emergency
/// declared early enough to finish landed or diverted.
fn stress(
    airport: &Airport,
    configs: impl Iterator<Item = WorldConfig>,
) -> BTreeMap<String, usize> {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for config in configs {
        let (world, outcomes) = automated_shift(airport.clone(), config);
        let what = format!(
            "{} seed {} {:?}",
            airport.code, config.seed, config.busyness
        );
        if !world.score.incidents.is_empty() {
            // Only fail on what emergencies cause: the same shift without them
            // has its own (rare) trouble with busy VFR traffic.
            let off = WorldConfig {
                emergencies: EmergencyRate::Off,
                ..config
            };
            let (baseline, _) = automated_shift(airport.clone(), off);
            assert!(
                !baseline.score.incidents.is_empty(),
                "{what}: {:?}",
                world.score.incidents
            );
            println!(
                "{what}: also without emergencies: {:?}",
                baseline.score.incidents
            );
        }
        for (callsign, kind) in &outcomes.declared {
            *kinds.entry(kind.label().to_string()).or_default() += 1;
            let finished =
                outcomes.landed.contains(callsign) || outcomes.diverted.contains(callsign);
            // Still flying at the end of the shift is fine; anything else isn't.
            let flying = world
                .aircraft
                .iter()
                .chain(world.emergencies.returning())
                .any(|a| &a.callsign == callsign);
            assert!(
                finished || flying,
                "{what}: {callsign} ({kind:?}) vanished; log {:?}",
                tail(&world)
            );
        }
    }
    kinds
}

fn tail(world: &World) -> Vec<String> {
    world
        .log
        .iter()
        .rev()
        .take(30)
        .map(|l| l.text.clone())
        .collect()
}

fn intense(seed: u64, wind_from: f32, busyness: Busyness) -> WorldConfig {
    WorldConfig {
        seed,
        wind: Some(Wind {
            from: wind_from,
            speed: 8.0,
        }),
        busyness,
        emergencies: EmergencyRate::Intense,
        ..Default::default()
    }
}

#[test]
fn automated_shifts_on_intense_are_clean_at_harrowgate() {
    let airport = Airport::default_airport();
    let kinds = stress(
        &airport,
        (0..8).map(|seed| intense(seed, 270.0, Busyness::default())),
    );
    assert!(kinds.values().sum::<usize>() >= 8, "{kinds:?}");
}

#[test]
fn automated_shifts_on_intense_are_clean_at_ksus() {
    let airport = ksus();
    // A quiet field when automated: few movements, so few emergencies.
    let configs = (0..8).map(|seed| {
        intense(
            seed,
            if seed % 2 == 0 { 250.0 } else { 80.0 },
            Busyness::Busy,
        )
    });
    stress(&airport, configs);
}

/// The long run from the plan: `cargo test --release -- --ignored stress`.
#[test]
#[ignore]
fn stress_six_hundred_intense_shifts() {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let busy = [Busyness::Quiet, Busyness::RealWorld, Busyness::Busy];
    for (airport, winds) in [
        (Airport::default_airport(), [270.0, 90.0]),
        (ksus(), [250.0, 80.0]),
    ] {
        let configs = (0..300u64).map(|seed| {
            let mut config = intense(seed, winds[seed as usize % 2], busy[seed as usize % 3]);
            config.vfr = seed % 2 == 0;
            // Every fourth shift has changing weather.
            if seed % 4 == 0 {
                config.wind = None;
            }
            config
        });
        for (k, n) in stress(&airport, configs) {
            *kinds.entry(k).or_default() += n;
        }
    }
    println!("{kinds:?}");
}
