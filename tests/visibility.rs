//! Ceilings, visibility and fog: VFR stopping in cloud, the ILS critical area
//! under low-visibility procedures, and minimums, Category III and diversions.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase, Taxi, VfrIntent};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::scoring::{GoAroundReason, SimEvent};
use atcgame::sim::sky::{Sky, SkyPlan};
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{Speaker, TICK, World, WorldConfig};
use glam::Vec2;

const IMC: Sky = Sky { ceiling: Some(700.0), visibility: 2.5 };
const LVP: Sky = Sky { ceiling: Some(400.0), visibility: 1.0 };
const FOG: Sky = Sky { ceiling: Some(100.0), visibility: 0.25 };

/// Harrowgate with the wind from the west: land 27L (south), depart 27R
/// (north), in weather that can change.
fn harrowgate(staffing: Staffing, sky: Sky) -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        staffing,
        weather: WeatherKind::LowIfr,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    world.fix_sky(sky);
    world.tick();
    world
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

fn end(world: &World, id: &str) -> usize {
    world.airport.runway_index(id).unwrap()
}

fn said(world: &World, text: &str) -> bool {
    world.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.contains(text))
}

fn go_arounds(events: &[SimEvent]) -> Vec<(String, GoAroundReason)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::GoAround { callsign, reason } => Some((callsign.clone(), *reason)),
            _ => None,
        })
        .collect()
}

/// An arrival established on the ILS for `runway_id`, `miles` out, with Tower
/// and cleared to land.
fn final_arrival(world: &World, id: u32, name: &str, ty: &str, runway_id: &str, miles: f32) -> Aircraft {
    let rwy = end(world, runway_id);
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, name, Flow::Arrival, aircraft_type(ty).unwrap(), pos, runway.heading, altitude, 150.0);
    ac.target_speed = 150.0;
    ac.target_altitude = altitude;
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.cleared_to_land = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

/// A departure for `runway_id` waiting at its hold line, talking to Tower.
fn departure_at_hold(world: &World, id: u32, name: &str, runway_id: &str) -> Aircraft {
    let map = world.airport.surface.as_ref().unwrap();
    let e = end(world, runway_id);
    let hold = map.departure_hold(e).unwrap();
    let mut ac = Aircraft::new(
        id,
        name,
        Flow::Departure,
        aircraft_type("B738").unwrap(),
        map.nodes[hold].pos,
        0.0,
        world.airport.elevation,
        0.0,
    );
    ac.on_ground = true;
    ac.phase = Phase::Taxiing;
    ac.taxi = Some(Taxi::at(hold));
    ac.departure_end = Some(e);
    ac.surface_freq = Position::Local;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.target_altitude = world.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    ac
}

#[test]
fn the_sky_thresholds_sit_where_the_rules_put_them() {
    let sky = |ceiling: f32, visibility: f32| Sky { ceiling: Some(ceiling), visibility };
    assert!(!sky(1000.0, 3.0).imc() && sky(999.0, 10.0).imc() && sky(5000.0, 2.9).imc());
    assert!(!sky(800.0, 2.0).lvp() && sky(799.0, 10.0).lvp() && sky(5000.0, 1.9).lvp());
    assert!(!sky(200.0, 0.5).below_cat1() && sky(199.0, 10.0).below_cat1() && sky(5000.0, 0.4).below_cat1());
    assert!(!Sky::CLEAR.imc());
}

#[test]
fn the_atis_and_status_line_report_the_fog() {
    let world = harrowgate(Staffing::LOCAL, FOG);
    let atis = world.atis_text();
    assert!(atis.contains("Category III"), "{atis}");
    let line = world.sky_line().unwrap();
    assert!(line.contains("BELOW CAT I"), "{line}");

    let world = harrowgate(Staffing::LOCAL, LVP);
    assert!(world.atis_text().contains("Low visibility procedures"), "{}", world.atis_text());
    assert!(world.sky_line().unwrap().ends_with("LVP"));
}

#[test]
fn in_cloud_no_vfr_traffic_appears() {
    let config = WorldConfig {
        seed: 3,
        vfr: true,
        staffing: Staffing::LOCAL_GROUND,
        weather: WeatherKind::LowIfr,
        shift_seconds: 40.0 * 60.0,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    world.fix_sky(IMC);
    let mut ifr = 0;
    while world.ended.is_none() {
        world.tick();
        world.drain_events();
        assert!(world.aircraft.iter().all(|a| a.vfr.is_none()), "VFR in IMC at {:.0}", world.time);
        ifr = ifr.max(world.aircraft.len());
    }
    assert!(ifr > 0, "IFR traffic carries on");
}

#[test]
fn when_the_cloud_comes_down_vfr_at_the_ramp_cancels_and_the_pattern_lands() {
    let mut world = harrowgate(Staffing::LOCAL_GROUND, Sky::CLEAR);
    let mut events = Vec::new();
    assert!(world.spawn_vfr(VfrIntent::Closed));
    run(&mut world, 1.0, &mut events);
    let id = world.aircraft.iter().find(|a| a.vfr.is_some()).unwrap().id;
    // Get it airborne in the pattern with circuits to go.
    let pattern = world.pattern_runway();
    {
        let ac = world.aircraft.iter_mut().find(|a| a.id == id).unwrap();
        assert!(ac.vfr.as_ref().unwrap().circuits > 0);
        ac.on_ground = false;
        ac.taxi = None;
        ac.phase = Phase::Pattern;
        ac.vfr.as_mut().unwrap().leg = atcgame::sim::aircraft::Leg::Downwind;
        ac.pos = world.airport.runways[pattern].threshold() + Vec2::new(0.0, 1.0);
        ac.altitude = world.airport.elevation + 1000.0;
    }
    assert!(world.spawn_vfr(VfrIntent::Departure));
    run(&mut world, 1.0, &mut events);
    let parked = world.aircraft.iter().find(|a| a.vfr.is_some() && a.id != id).unwrap().id;
    assert_eq!(world.find(parked).unwrap().phase, Phase::Parked);

    world.fix_sky(IMC);
    run(&mut world, 1.0, &mut events);
    assert!(world.find(parked).is_none(), "the parked VFR departure cancelled");
    assert!(said(&world, "we'll cancel"));
    let vfr = world.find(id).unwrap().vfr.clone().unwrap();
    assert_eq!(vfr.circuits, 0);
    assert!(vfr.full_stop());
    assert!(said(&world, "full stop"));
}

/// The automated Tower releases a departure from the arrival runway in front
/// of an arrival 6 miles out in good weather, but not under low-visibility
/// procedures: the arrival is inside the critical area before it's rolling.
#[test]
fn under_lvp_the_automated_tower_keeps_the_critical_area_clear() {
    let released = |sky: Sky| {
        let mut world = harrowgate(Staffing::GROUND, sky);
        let mut events = Vec::new();
        world.add_aircraft(final_arrival(&world, 2, "NVA202", "B738", "27L", 6.0));
        world.add_aircraft(departure_at_hold(&world, 1, "NVA101", "27L"));
        run(&mut world, 10.0, &mut events);
        let ac = world.find(1).unwrap();
        ac.cleared_takeoff || ac.phase != Phase::Taxiing
    };
    assert!(released(Sky::CLEAR), "released in good weather");
    assert!(!released(LVP), "held under LVP");
    assert!(!released(FOG), "held below minimums too");
}

#[test]
fn under_lvp_a_lineup_with_an_arrival_inside_5_miles_sends_it_around() {
    let lined_up = |sky: Sky| {
        let mut world = harrowgate(Staffing::LOCAL, sky);
        let mut events = Vec::new();
        world.add_aircraft(departure_at_hold(&world, 1, "NVA101", "27L"));
        world.add_aircraft(final_arrival(&world, 2, "NVA202", "B738", "27L", 4.5));
        assert!(world.transmit(1, vec![Clearance::LineUpAndWait(end(&world, "27L"))]));
        run(&mut world, 20.0, &mut events);
        (world, events)
    };
    let (world, events) = lined_up(LVP);
    assert_eq!(go_arounds(&events), vec![("NVA202".to_string(), GoAroundReason::IlsInterference)]);
    assert!(
        world.score.incidents.iter().any(|i| i.text.contains("ILS critical area") && i.points < 0),
        "{:?}",
        world.score.incidents
    );
    // In good weather there's still time before it's a problem.
    let (_, events) = lined_up(Sky::CLEAR);
    assert!(go_arounds(&events).is_empty(), "{events:?}");
}

#[test]
fn below_minimums_a_turboprop_goes_around_and_a_jet_autolands() {
    let mut world = harrowgate(Staffing::LOCAL, FOG);
    let mut events = Vec::new();
    world.add_aircraft(final_arrival(&world, 1, "SKW11", "C208", "27L", 2.0));
    world.add_aircraft(final_arrival(&world, 2, "NVA202", "B738", "27L", 6.0));
    // Tower tells Approach about the go-around.
    run_until(&mut world, 120.0, &mut events, |w| w.owes_go_around_call(1));
    assert!(world.call_go_around(1));
    run_until(&mut world, 240.0, &mut events, |w| w.find(2).is_some_and(|a| a.on_ground));

    assert_eq!(go_arounds(&events), vec![("SKW11".to_string(), GoAroundReason::Weather)]);
    assert!(said(&world, "minimums"));
    assert!(events.iter().any(|e| matches!(e, SimEvent::Landed { callsign, .. } if callsign == "NVA202")));
    // Nobody's fault.
    assert!(world.score.incidents.iter().all(|i| i.points >= 0), "{:?}", world.score.incidents);
    assert_eq!(world.find(1).unwrap().weather_go_arounds, 1);
}

#[test]
fn below_minimums_the_ils_is_refused_by_a_turboprop() {
    let mut world = harrowgate(Staffing::APPROACH, FOG);
    let rwy = end(&world, "27L");
    let runway = world.airport.runways[rwy].clone();
    let pos = runway.threshold() - heading_vec(runway.heading) * 15.0 + Vec2::new(0.0, -3.0);
    let mut ac = Aircraft::new(1, "SKW11", Flow::Arrival, aircraft_type("C208").unwrap(), pos, runway.heading + 30.0, 3000.0, 160.0);
    ac.entered_sector = true;
    world.add_aircraft(ac);
    let mut events = Vec::new();
    world.transmit(1, vec![Clearance::ClearedIls(rwy)]);
    run(&mut world, 8.0, &mut events);
    assert!(said(&world, "below our minimums"), "{:?}", world.log.last());
    assert_eq!(world.find(1).unwrap().phase, Phase::Controlled);
}

#[test]
fn an_arrival_that_cant_land_diverts_after_two_go_arounds_or_twenty_minutes() {
    let mut world = harrowgate(Staffing::APPROACH, FOG);
    let mut events = Vec::new();
    let mut ac = Aircraft::new(1, "SKW11", Flow::Arrival, aircraft_type("C208").unwrap(), Vec2::new(10.0, 5.0), 90.0, 4000.0, 160.0);
    ac.entered_sector = true;
    ac.weather_go_arounds = 2;
    world.add_aircraft(ac);
    let mut ac = Aircraft::new(2, "SKW22", Flow::Arrival, aircraft_type("PC12").unwrap(), Vec2::new(-10.0, 5.0), 270.0, 5000.0, 160.0);
    ac.entered_sector = true;
    world.add_aircraft(ac);
    let mut ac = Aircraft::new(3, "NVA303", Flow::Arrival, aircraft_type("A320").unwrap(), Vec2::new(0.0, -12.0), 0.0, 6000.0, 200.0);
    ac.entered_sector = true;
    world.add_aircraft(ac);
    run(&mut world, 2.0, &mut events);
    assert!(world.find(1).is_none());
    assert!(said(&world, "divert"));
    assert!(world.find(2).is_some());

    // Held long enough, the other gives up too. Keep it in the hold-free middle of the sector.
    let since = world.find(2).unwrap().minimums_since.unwrap();
    world.aircraft.iter_mut().find(|a| a.id == 2).unwrap().minimums_since = Some(since - 20.0 * 60.0);
    run(&mut world, 1.0, &mut events);
    assert!(world.find(2).is_none());
    let diverted: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Diverted { callsign } => Some(callsign.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(diverted, ["SKW11", "SKW22"]);
    // The jet can land, so it stays.
    assert!(world.find(3).is_some());
    assert!(world.score.incidents.iter().all(|i| i.points == 0), "{:?}", world.score.incidents);
}

/// A tower shift in thick fog: the automated Approach only sends Category III
/// types down the final, and holds the rest until they give up.
#[test]
fn in_fog_only_category_iii_traffic_lands_on_a_tower_shift() {
    use std::collections::HashMap;
    let config = WorldConfig {
        seed: 11,
        staffing: Staffing::GROUND,
        weather: WeatherKind::LowIfr,
        shift_seconds: 45.0 * 60.0,
        ..Default::default()
    };
    let mut world = World::new(Airport::default_airport(), config);
    world.fix_sky(FOG);
    let mut types: HashMap<String, bool> = HashMap::new();
    let (mut landed_cat3, mut held) = (0, 0);
    while world.ended.is_none() {
        world.tick();
        for ac in &world.aircraft {
            types.insert(ac.callsign.clone(), ac.ty.cat3);
            if ac.flow == Flow::Arrival && !ac.ty.cat3 && matches!(ac.lateral, Lateral::Hold(_)) {
                held += 1;
            }
        }
        for event in world.drain_events() {
            if let SimEvent::Landed { callsign, .. } = &event {
                assert!(types[callsign], "{callsign} landed below minimums");
                landed_cat3 += 1;
            }
            if let SimEvent::GoAround { callsign, reason } = &event {
                panic!("{callsign} went around ({reason:?}): the automated Approach shouldn't send it");
            }
        }
    }
    assert!(landed_cat3 > 3, "{landed_cat3} landed");
    assert!(held > 0, "nothing was held");
}

/// Variable weather that stays good all shift plays out exactly like a Good
/// day: the sky has its own dice, and only the ATIS wording differs.
#[test]
fn weather_that_stays_good_changes_nothing_else() {
    let shift = 30.0 * 60.0;
    let seed = (0..200u64)
        .find(|&s| {
            let plan = SkyPlan::roll(WeatherKind::Variable, s, shift);
            !plan.storms && (0..=60).all(|m| !plan.at(m as f32 * 30.0).imc())
        })
        .expect("a fair-weather Variable seed");
    let play = |weather: WeatherKind| {
        let config = WorldConfig {
            seed,
            vfr: true,
            staffing: Staffing::LOCAL_GROUND,
            weather,
            shift_seconds: shift,
            ..Default::default()
        };
        let mut world = World::new(Airport::default_airport(), config);
        let mut events = Vec::new();
        while world.ended.is_none() {
            world.tick();
            events.extend(world.drain_events());
        }
        let positions: Vec<_> = world.aircraft.iter().map(|a| (a.callsign.clone(), a.pos)).collect();
        (format!("{events:?}"), positions)
    };
    assert_eq!(play(WeatherKind::Good), play(WeatherKind::Variable));
}

// Stress: whole shifts in every kind of weather, at both airports, with the
// positions automated, and with nobody answering Approach's frequency.

#[derive(Debug, Default)]
struct WeatherStats {
    landed: usize,
    diverted: usize,
    deviations: usize,
    weather_go_arounds: usize,
    trouble: usize,
    problems: Vec<String>,
}

fn ksus() -> Airport {
    use atcgame::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

fn weather_shift(airport: &Airport, config: WorldConfig) -> WeatherStats {
    use std::collections::{HashMap, HashSet};
    let mut world = World::new(airport.clone(), config);
    let mut stats = WeatherStats::default();
    let mut deviating_since: HashMap<u32, f32> = HashMap::new();
    let mut still_since: HashMap<u32, (Vec2, f32)> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut flagged: HashSet<u32> = HashSet::new();
    while world.ended.is_none() {
        world.tick();
        for event in world.drain_events() {
            match event {
                SimEvent::Landed { .. } => stats.landed += 1,
                SimEvent::Diverted { .. } => stats.diverted += 1,
                SimEvent::GoAround { reason: GoAroundReason::Weather, .. } => stats.weather_go_arounds += 1,
                SimEvent::NearCollision { .. } | SimEvent::GroundConflict { .. } | SimEvent::RunwayIncursion { .. } => {
                    stats.trouble += 1
                }
                _ => {}
            }
        }
        let now = world.time;
        let imc = world.sky().imc();
        for ac in &world.aircraft {
            if seen.insert(ac.callsign.clone()) && ac.vfr.is_some() && imc {
                stats.problems.push(format!("{} VFR appeared in IMC at {now:.0}", ac.callsign));
            }
            let en_route = !ac.on_ground && matches!(ac.phase, Phase::Controlled | Phase::Offered | Phase::OfferedToCenter);
            let level = world.storms.level_at(ac.pos);
            if en_route && level >= 4.0 && flagged.insert(ac.id) {
                stats.problems.push(format!("{} in level {level:.1} at {now:.0} {:?}", ac.callsign, ac.lateral));
            }
            match ac.deviation {
                Some(_) => {
                    let since = *deviating_since.entry(ac.id).or_insert_with(|| {
                        stats.deviations += 1;
                        now
                    });
                    if now - since > 10.0 * 60.0 && flagged.insert(ac.id) {
                        stats.problems.push(format!("{} deviating since {since:.0}", ac.callsign));
                    }
                }
                None => {
                    deviating_since.remove(&ac.id);
                }
            }
            if ac.on_ground && ac.phase == Phase::Taxiing {
                let entry = still_since.entry(ac.id).or_insert((ac.pos, now));
                if entry.0.distance(ac.pos) > 0.01 {
                    *entry = (ac.pos, now);
                } else if now - entry.1 > 15.0 * 60.0 && flagged.insert(ac.id) {
                    stats.problems.push(format!("{} still on the ground since {:.0}", ac.callsign, entry.1));
                }
            }
        }
    }
    stats
}

const KINDS: [WeatherKind; 4] = [WeatherKind::Good, WeatherKind::Variable, WeatherKind::LowIfr, WeatherKind::Storms];

fn weather_stress(airports: &[Airport], seeds: std::ops::Range<u64>) {
    use atcgame::sim::traffic::Busyness;
    let mut failures = Vec::new();
    let mut totals: Vec<(WeatherKind, WeatherStats)> = KINDS.iter().map(|&k| (k, WeatherStats::default())).collect();
    for airport in airports {
        for seed in seeds.clone() {
            for staffing in [Staffing::NONE, Staffing::APPROACH] {
                let config = |weather| WorldConfig {
                    seed,
                    vfr: true,
                    staffing,
                    weather,
                    busyness: [Busyness::RealWorld, Busyness::Busy][seed as usize % 2],
                    shift_seconds: 45.0 * 60.0,
                    ..Default::default()
                };
                let good = weather_shift(airport, config(WeatherKind::Good));
                for (i, &kind) in KINDS.iter().enumerate() {
                    let s = weather_shift(airport, config(kind));
                    let mut problems = s.problems.clone();
                    if s.trouble > good.trouble + 2 {
                        problems.push(format!("{} conflicts, {} in good weather", s.trouble, good.trouble));
                    }
                    if !problems.is_empty() {
                        failures.push(format!("{} seed {seed} {staffing:?} {kind:?}: {problems:?}", airport.code));
                    }
                    let t = &mut totals[i].1;
                    t.landed += s.landed;
                    t.diverted += s.diverted;
                    t.deviations += s.deviations;
                    t.weather_go_arounds += s.weather_go_arounds;
                    t.trouble += s.trouble;
                }
            }
        }
    }
    for (kind, t) in &totals {
        println!("{kind:?}: {t:?}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn shifts_in_every_kind_of_weather_hold_up() {
    weather_stress(&[Airport::default_airport(), ksus()], 0..2);
}

/// The long run: `cargo test --release --test visibility -- --ignored weather_stress`.
#[test]
#[ignore]
fn weather_stress_one_hundred_shifts() {
    weather_stress(&[Airport::default_airport(), ksus()], 0..13);
}
