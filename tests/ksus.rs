//! KSUS: checks the generated FAA data and that the sim works with real geography.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::scoring::{ShiftEnd, SimEvent};
use atcgame::sim::registration::is_valid_n_number;
use atcgame::sim::traffic::Busyness;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

fn ksus() -> Airport {
    Airport::from_toml(KSUS_AIRPORT).expect("generated KSUS data is valid")
}

fn quiet_world(wind_from: f32) -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        wind: Some(Wind { from: wind_from, speed: 0.0 }),
        ..Default::default()
    };
    World::new(ksus(), config)
}

fn fix_pos(airport: &Airport, name: &str) -> Vec2 {
    airport.fixes[airport.fix_index(name).unwrap()].pos()
}

fn jet(id: u32, pos: Vec2, heading: f32, altitude: f32) -> Aircraft {
    let ty = aircraft_type("C56X").unwrap();
    Aircraft::new(id, format!("N{id}"), Flow::Arrival, ty, pos, heading, altitude, 180.0)
}

fn run_until(world: &mut World, seconds: f32, events: &mut Vec<SimEvent>, done: impl Fn(&World) -> bool) {
    for _ in 0..(seconds / TICK) as usize {
        if done(world) {
            return;
        }
        world.tick();
        events.extend(world.drain_events());
    }
    panic!("condition not met in {seconds} s: {events:?}");
}

#[test]
fn published_facts_survive_the_import() {
    let a = ksus();
    assert_eq!(a.code, "KSUS");
    assert_eq!(a.elevation, 463.0);
    assert_eq!(a.magnetic_variation, -1.0);
    assert_eq!(a.go_around_altitude, 2400.0);

    let runway = |id: &str| &a.runways[a.runway_index(id).unwrap()];
    assert!((runway("26L").takeoff_length_ft() - 7486.0).abs() < 25.0);
    assert!((runway("08R").takeoff_length_ft() - 7486.0).abs() < 25.0);
    assert!((runway("26R").takeoff_length_ft() - 5000.0).abs() < 25.0);
    assert!((runway("26L").heading - 258.0).abs() < 0.2);
    assert!((runway("08R").heading - 78.0).abs() < 0.2);

    // Only the long runway has an ILS, at 3 degrees and 55 ft.
    assert!(runway("26L").ils && runway("08R").ils);
    assert!(!runway("26R").ils && !runway("08L").ils);
    assert_eq!(runway("26L").glideslope_deg, 3.0);
    assert_eq!(runway("26L").threshold_crossing_height, 55.0);

    // 26L's displaced threshold: the roll starts about 481 ft before the threshold.
    let displaced = (runway("26L").takeoff_point() - runway("26L").threshold()).length();
    assert!((displaced * 6076.12 - 481.0).abs() < 25.0, "{}", displaced * 6076.12);

    // 08L/26R is the northern runway, as on the airport diagram.
    assert!(runway("26R").y > runway("26L").y);
}

#[test]
fn procedure_fixes_are_where_the_charts_put_them() {
    let a = ksus();
    let distance = |name: &str| fix_pos(&a, name).length();
    assert!((distance("DERDF") - 13.3).abs() < 0.2);
    assert!((distance("EMKAA") - 13.5).abs() < 0.2);
    assert!((distance("BUUDD") - 55.9).abs() < 0.2);
    assert!((distance("DELMA") - 33.8).abs() < 0.2);
    assert!(fix_pos(&a, "CSX").length() > 10.0, "CSX resolved from its DME position");
}

#[test]
fn all_five_stars_exist_in_both_directions_and_every_sid_exits() {
    let a = ksus();
    for config in ["West", "East"] {
        let mut names: Vec<&str> = a
            .stars
            .iter()
            .filter(|s| s.config == config)
            .map(|s| s.name.as_deref().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["BUUDD3", "DELMA4", "DIRTT2", "JHAUN1", "SLVER1"]);
    }
    for sid in &a.sids {
        // Clipped routes end outside the sector, or the aircraft would never leave.
        assert!(!sid.fixes.is_empty());
    }
    let names: Vec<&str> = a.sids.iter().map(|s| s.name.as_deref().unwrap()).collect();
    for expected in ["GATWY1", "LINDY9", "OZARK8", "PLESS5"] {
        assert!(names.contains(&expected), "{expected}");
    }
}

#[test]
fn wind_selects_the_ils_runway_for_that_direction() {
    assert_eq!(quiet_world(250.0).config().arrival, "26L");
    assert_eq!(quiet_world(90.0).config().arrival, "08R");
}

#[test]
fn every_star_spawns_outside_the_sector() {
    let mut gates = std::collections::BTreeSet::new();
    for seed in 0..40 {
        for wind in [250.0, 90.0] {
            let config = WorldConfig {
                seed,
                wind: Some(Wind { from: wind, speed: 5.0 }),
                ..Default::default()
            };
            let mut world = World::new(ksus(), config);
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..(600.0 / TICK) as usize {
                world.tick();
                for ac in world.aircraft.iter().filter(|a| a.flow == Flow::Arrival) {
                    if seen.insert(ac.id) {
                        assert!(
                            !world.airport.in_sector(ac.pos),
                            "{} spawned inside at {:?}",
                            ac.callsign,
                            ac.pos
                        );
                        gates.insert(world.airport.fixes[ac.route[0]].name.clone());
                    }
                }
            }
        }
    }
    let expected = ["BUUDD", "DELMA", "DIRTT", "JHAUN", "SLVER"].map(String::from);
    assert_eq!(gates.into_iter().collect::<Vec<_>>(), expected);
}

fn fly_ils(wind_from: f32, runway_id: &str) {
    let mut world = quiet_world(wind_from);
    let mut events = Vec::new();
    let rwy = world.airport.runway_index(runway_id).unwrap();
    assert_eq!(world.arrival_runway(), rwy);
    let runway = world.airport.runways[rwy].clone();

    // 16 NM out and 3 NM left of the course, on a 20 degree intercept.
    let course = heading_vec(runway.heading);
    let left = -heading_vec(runway.heading + 90.0);
    let pos = runway.threshold() - course * 16.0 + left * 3.0;
    world.add_aircraft(jet(1, pos, runway.heading + 20.0, 3200.0));

    assert!(world.transmit(1, vec![Clearance::ClearedIls(rwy)]));
    run_until(&mut world, 400.0, &mut events, |w| {
        matches!(w.find(1).unwrap().phase, Phase::Localizer | Phase::Glideslope)
    });
    assert!(world.transmit(1, vec![Clearance::HandoffTower]));
    run_until(&mut world, 600.0, &mut events, |w| {
        w.find(1).is_some_and(|a| a.phase == Phase::Rollout)
    });
    assert_eq!(world.score.landed, 1, "{events:?}");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
    assert_eq!(world.find(1).unwrap().altitude, 463.0);
}

#[test]
fn ils_landing_works_on_26l() {
    fly_ils(250.0, "26L");
}

#[test]
fn ils_landing_works_on_08r() {
    fly_ils(90.0, "08R");
}

#[test]
fn glideslope_matches_the_published_final_approach_fix_altitude() {
    let a = ksus();
    let rwy = a.runway_index("26L").unwrap();
    // CIFP: glideslope intercept altitude at EAVES is 1,900 ft, at 5.0 NM from the field.
    // EAVES isn't in the game data, so use its real position directly.
    let eaves_x = (-(90.0 + 32.0 / 60.0 + 50.75 / 3600.0) + (90.0 + 39.0 / 60.0 + 7.37 / 3600.0)) * 60.0
        * 38.662_f32.to_radians().cos();
    let eaves_y = ((38.0 + 40.0 / 60.0 + 34.51 / 3600.0) - (38.0 + 39.0 / 60.0 + 43.62 / 3600.0)) * 60.0;
    let along = a.runways[rwy].along(Vec2::new(eaves_x, eaves_y));
    let altitude = a.glideslope_altitude(rwy, along);
    assert!((altitude - 1900.0).abs() < 60.0, "{altitude}");
}

#[test]
fn a_clearance_is_spoken_in_magnetic() {
    let mut world = quiet_world(250.0);
    let mut events = Vec::new();
    world.add_aircraft(jet(1, Vec2::new(20.0, 20.0), 180.0, 5000.0));
    // True 258 is magnetic 259, the published runway heading.
    world.transmit(1, vec![Clearance::Heading(258.0)]);
    let said = world.log.iter().rev().find(|l| l.speaker == Speaker::Controller).unwrap();
    assert!(said.text.contains("heading 259"), "{}", said.text);
    run_until(&mut world, 10.0, &mut events, |w| {
        w.find(1).unwrap().lateral == Lateral::Heading(258.0)
    });
}

#[test]
fn star_ends_on_its_published_vector_heading() {
    let mut world = quiet_world(250.0);
    let mut events = Vec::new();
    let star = world
        .airport
        .stars
        .iter()
        .find(|s| s.name.as_deref() == Some("SLVER1") && s.config == "West")
        .unwrap()
        .clone();
    // The chart's heading after EMKAA is 346 magnetic, so 345 true.
    assert_eq!(star.final_heading, Some(345.0));

    let emkaa = fix_pos(&world.airport, "EMKAA");
    let mut ac = jet(1, emkaa + Vec2::new(3.0, -3.0), 315.0, 5000.0);
    ac.route = world.airport.route_indices(&star.fixes);
    ac.route_index = ac.route.len() - 1;
    ac.lateral = Lateral::Route;
    ac.route_end_heading = star.final_heading;
    world.add_aircraft(ac);
    run_until(&mut world, 200.0, &mut events, |w| {
        w.find(1).unwrap().lateral == Lateral::Heading(345.0)
    });
}

fn exit_via(airport: &Airport, names: &[&str], pos: Vec2, altitude: f32) -> Aircraft {
    let route: Vec<usize> = names.iter().map(|n| airport.fix_index(n).unwrap()).collect();
    let mut ac = jet(1, pos, 120.0, altitude);
    ac.flow = Flow::Departure;
    ac.phase = Phase::OfferedToCenter;
    ac.entered_sector = true;
    ac.exit_fix = route.last().copied();
    ac.exit_altitude = 10000.0;
    ac.route = route;
    ac.route_index = 1;
    ac.lateral = Lateral::Route;
    ac
}

#[test]
fn departure_to_an_exit_fix_beyond_the_sector_is_not_penalised() {
    let mut world = quiet_world(250.0);
    let mut events = Vec::new();
    // PLESS is 94 NM out, 34 NM beyond the boundary.
    let start = fix_pos(&world.airport, "STL");
    let ac = exit_via(&world.airport, &["STL", "PLESS"], start, 10000.0);
    world.add_aircraft(ac);
    run_until(&mut world, 1500.0, &mut events, |w| w.find(1).is_none());
    assert!(events.iter().any(|e| matches!(e, SimEvent::Exited { .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::WrongExit { .. })), "{events:?}");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn leaving_far_from_the_route_to_an_outside_exit_is_still_penalised() {
    let mut world = quiet_world(250.0);
    let mut events = Vec::new();
    // Route points southeast to PLESS, but this aircraft leaves the sector to the north.
    let mut ac = exit_via(&world.airport, &["STL", "PLESS"], Vec2::new(0.0, 58.0), 10000.0);
    ac.lateral = Lateral::Heading(0.0);
    ac.heading = 0.0;
    world.add_aircraft(ac);
    run_until(&mut world, 200.0, &mut events, |w| w.find(1).is_none());
    assert!(events.iter().any(|e| matches!(e, SimEvent::WrongExit { .. })), "{events:?}");
}

#[test]
fn runway_choice_follows_aircraft_length_requirements() {
    let world = quiet_world(250.0);
    let id = |ty: &str| {
        let rwy = world.departure_runway_for(aircraft_type(ty).unwrap());
        world.airport.runways[rwy].id.clone()
    };
    assert_eq!(id("C172"), "26R");
    assert_eq!(id("C56X"), "26R");
    assert_eq!(id("CL35"), "26L");
    assert_eq!(id("GLF6"), "26L");
    let world = quiet_world(90.0);
    let rwy = world.departure_runway_for(aircraft_type("GLF6").unwrap());
    assert_eq!(world.airport.runways[rwy].id, "08R");
}

#[test]
fn no_jet_needing_the_long_runway_ever_rolls_on_26r() {
    let mut released = 0;
    for seed in 0..15 {
        let config = WorldConfig {
            seed,
            wind: Some(Wind { from: 250.0, speed: 5.0 }),
            ..Default::default()
        };
        let mut world = World::new(ksus(), config);
        let long = world.airport.runway_index("26L").unwrap();
        for _ in 0..(1800.0 / TICK) as usize {
            world.tick();
            for ac in world.aircraft.iter().filter(|a| a.on_ground && a.phase == Phase::TowerRelease) {
                released += 1;
                let on_long = world.airport.runways[long].cross(ac.pos).abs() < 0.1;
                assert_eq!(on_long, ac.ty.min_runway_ft > 5000.0, "{} {:?}", ac.ty.designator, ac.pos);
            }
        }
    }
    assert!(released > 0, "no departures were released in any seed");
}

#[test]
fn departure_waits_for_an_arrival_on_short_final_of_a_shared_runway() {
    let mut world = quiet_world(250.0);
    let mut events = Vec::new();
    let rwy = world.airport.runway_index("26L").unwrap();
    let runway = world.airport.runways[rwy].clone();

    let pos = runway.threshold() - heading_vec(runway.heading) * 3.0;
    let mut arrival = jet(1, pos, runway.heading, 1400.0);
    arrival.phase = Phase::TowerHandedOff;
    arrival.lateral = Lateral::Localizer;
    arrival.approach_runway = Some(rwy);
    arrival.on_glideslope = true;
    world.add_aircraft(arrival);

    let route = world.airport.route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("GLF6").unwrap(), route, 10000.0);

    world.tick();
    assert_eq!(world.departures_waiting(), 1, "held while the arrival is on short final");
    run_until(&mut world, 400.0, &mut events, |w| w.departures_waiting() == 0);
    assert_eq!(world.score.landed, 1, "the arrival landed first: {events:?}");
    assert!(!events.iter().any(|e| matches!(e, SimEvent::GoAround { .. })), "{events:?}");
}

#[test]
fn mva_comes_from_the_chart_and_honours_holes() {
    let a = ksus();
    let mut holes = 0;
    for area in &a.mva {
        assert!((2000.0..=3100.0).contains(&area.altitude));
        for hole in &area.holes {
            let n = hole.len() as f32;
            let (x, y) = hole.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
            let center = Vec2::new(x / n, y / n);
            if atcgame::sim::airspace::point_in_polygon(center, hole) {
                assert!(!area.contains(center), "a hole doesn't belong to its own sector");
                holes += 1;
            }
        }
    }
    assert!(holes > 0, "expected the chart's inner rings to be imported");

    // Values read directly from the raw T75_MVA_FUS3 chart, by lat/long.
    assert_eq!(a.mva_at(Vec2::ZERO), 2000.0, "field");
    assert_eq!(a.mva_at(Vec2::new(12.5, -4.9)), 2600.0, "EMKAA");
    assert_eq!(a.mva_at(Vec2::new(-11.1, -7.3)), 2300.0, "DERDF");
    assert_eq!(a.mva_at(Vec2::new(-30.0, 0.0)), 2600.0, "30 NM west");

    // The published 2,500 ft ILS intermediate fix must be legal against the chart.
    let rwy = a.runway_index("26L").unwrap();
    let runway = &a.runways[rwy];
    let horus = runway.threshold() - heading_vec(runway.heading) * 14.0;
    assert!(a.mva_at(horus) <= 2500.0, "{}", a.mva_at(horus));
}

#[test]
fn a_shift_is_quiet_uses_the_real_fleet_and_completes() {
    let mut totals = Vec::new();
    for seed in 0..20 {
        let config = WorldConfig {
            seed,
            wind: Some(Wind { from: 250.0, speed: 8.0 }),
            ..Default::default()
        };
        let mut world = World::new(ksus(), config);
        let mut seen = std::collections::BTreeSet::new();
        while world.ended.is_none() {
            world.tick();
            world.drain_events();
            for ac in &world.aircraft {
                seen.insert(ac.id);
                assert!(
                    ["C172", "PC12", "C56X", "CL35", "GLF6"].contains(&ac.ty.designator),
                    "{}",
                    ac.ty.designator
                );
            }
        }
        assert_ne!(world.ended, Some(ShiftEnd::Decertified), "seed {seed}");
        totals.push(seen.len());
    }
    let mean = totals.iter().sum::<usize>() as f32 / totals.len() as f32;
    assert!((1.0..=8.0).contains(&mean), "mean {mean}, {totals:?}");
}

#[test]
fn general_aviation_in_a_shift_uses_legal_n_numbers() {
    let mut seen = std::collections::BTreeMap::new();
    for seed in 0..20 {
        let config = WorldConfig {
            seed,
            wind: Some(Wind { from: 250.0, speed: 8.0 }),
            busyness: Busyness::VeryBusy,
            ..Default::default()
        };
        let mut world = World::new(ksus(), config);
        while world.ended.is_none() {
            world.tick();
            world.drain_events();
            for ac in &world.aircraft {
                seen.insert((seed, ac.id), (ac.callsign.clone(), ac.ty.max_speed));
            }
        }
    }
    let registered: Vec<_> = seen
        .values()
        .filter(|(c, _)| !c.starts_with("SPT") && !c.starts_with("GTW"))
        .collect();
    let foreign: Vec<_> = registered.iter().filter(|(c, _)| !c.starts_with('N')).collect();
    for (callsign, max_speed) in &registered {
        if callsign.starts_with('N') {
            assert!(is_valid_n_number(callsign), "{callsign}");
        } else {
            assert!(*max_speed >= 250.0, "foreign {callsign} should be a jet");
        }
    }
    let share = foreign.len() as f32 / registered.len() as f32;
    assert!(share < 0.1, "{} of {}", foreign.len(), registered.len());
}

#[test]
fn busyness_slider_has_real_world_in_the_middle_and_stops_at_the_ends() {
    assert_eq!(Busyness::ALL.len(), 5);
    assert_eq!(Busyness::ALL[2], Busyness::RealWorld);
    assert_eq!(Busyness::default(), Busyness::RealWorld);
    assert_eq!(Busyness::RealWorld.label(true), "Real-world");
    assert_eq!(Busyness::VeryQuiet.step(-1), Busyness::VeryQuiet);
    assert_eq!(Busyness::VeryBusy.step(1), Busyness::VeryBusy);
    assert_eq!(Busyness::Quiet.step(1), Busyness::RealWorld);
    let multipliers: Vec<f32> = Busyness::ALL.iter().map(|b| b.multiplier()).collect();
    assert!(multipliers.windows(2).all(|w| w[0] < w[1]), "{multipliers:?}");
}

#[test]
fn busyness_scales_the_real_world_rate() {
    let world_at = |busyness| {
        let config = WorldConfig { seed: 3, traffic: false, busyness, ..Default::default() };
        World::new(ksus(), config)
    };
    let real = world_at(Busyness::RealWorld).current_rate();
    assert!((real - 4.0).abs() < 1e-3, "KSUS starts at its estimated 4/hr, got {real}");
    for level in Busyness::ALL {
        let rate = world_at(level).current_rate();
        assert!((rate - real * level.multiplier()).abs() < 1e-3, "{level:?}: {rate}");
    }
}

#[test]
fn a_very_busy_shift_sees_more_traffic_than_a_very_quiet_one() {
    let count = |busyness| {
        let mut total = 0;
        for seed in 0..5 {
            let config = WorldConfig {
                seed,
                wind: Some(Wind { from: 250.0, speed: 8.0 }),
                shift_seconds: 900.0,
                busyness,
                ..Default::default()
            };
            let mut world = World::new(ksus(), config);
            let mut seen = std::collections::BTreeSet::new();
            while world.ended.is_none() {
                world.tick();
                world.drain_events();
                seen.extend(world.aircraft.iter().map(|ac| ac.id));
            }
            total += seen.len() + world.departures_waiting();
        }
        total
    };
    let quiet = count(Busyness::VeryQuiet);
    let busy = count(Busyness::VeryBusy);
    assert!(busy >= quiet * 3, "very quiet {quiet}, very busy {busy}");
}
