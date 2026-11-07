//! KMEM: checks the generated FAA and OpenStreetMap data and that a Memphis shift runs.

use atcgame::sim::aircraft::{Aircraft, Flow, Phase};
use atcgame::sim::airspace::{Airport, KMEM_AIRPORT, KMEM_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::ground_map::StandUse;
use atcgame::sim::positions::{Position, Staffing};
use atcgame::sim::performance::{WakeCategory, aircraft_type};
use atcgame::sim::scoring::{ShiftEnd, SimEvent};
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{TICK, World, WorldConfig};

const FT_PER_NM: f32 = 6076.12;

fn kmem() -> Airport {
    Airport::load(KMEM_AIRPORT, Some(KMEM_GROUND)).expect("generated KMEM data is valid")
}

fn quiet_world(wind_from: f32) -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        wind: Some(Wind { from: wind_from, speed: 0.0 }),
        ..Default::default()
    };
    World::new(kmem(), config)
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
    let a = kmem();
    assert_eq!(a.code, "KMEM");
    assert_eq!(a.elevation, 341.0);
    assert_eq!(a.magnetic_variation, -1.0);
    assert_eq!(a.runways.len(), 8);

    let runway = |id: &str| &a.runways[a.runway_index(id).unwrap()];
    let length = |id: &str| runway(id).length * FT_PER_NM;
    // The airport diagram's lengths.
    assert!((length("18C") - 11120.0).abs() < 30.0, "{}", length("18C"));
    assert!((length("18L") - 9000.0).abs() < 30.0, "{}", length("18L"));
    assert!((length("18R") - 9320.0).abs() < 30.0, "{}", length("18R"));
    assert!((length("09") - 8946.0).abs() < 30.0, "{}", length("09"));
    // Every runway end has an ILS.
    assert!(a.runways.iter().all(|r| r.ils));

    // 18R is the western parallel, 18L the eastern; 09/27 lies to the north.
    assert!(runway("18R").x < runway("18C").x && runway("18C").x < runway("18L").x);
    assert!(runway("27").threshold().y > runway("18L").threshold().y);
    // 18L and 18C, about 960 ft apart, share a final for separation; 18R,
    // about 4,320 ft from 18L, takes independent approaches.
    let end = |id: &str| a.runway_index(id).unwrap();
    assert!(a.same_lane(end("18L"), end("18C")));
    let spacing = a.centreline_spacing_ft(end("18L"), end("18R"));
    assert!((4300.0..4400.0).contains(&spacing), "{spacing}");
    assert_eq!(a.parallel_minimum(end("18L"), end("18R")), None);
}

#[test]
fn stars_sids_and_rnav_approaches_exist_for_both_flows() {
    let a = kmem();
    for config in ["South", "North"] {
        let mut stars: Vec<&str> =
            a.stars.iter().filter(|s| s.config == config).map(|s| s.name.as_deref().unwrap()).collect();
        stars.sort_unstable();
        stars.dedup();
        assert_eq!(
            stars,
            ["BLUZZ5", "BRBBQ3", "CONDR4", "DAWGG2", "HOBRK4", "HYTHR3", "MONAA4", "VANZE3", "WLDER1"]
        );
        assert!(a.sids.iter().filter(|s| s.config == config).count() >= 10);
    }
    for sid in &a.sids {
        assert!(!sid.fixes.is_empty());
    }
    for id in ["18L", "18C", "18R", "36L", "36C", "36R", "09", "27"] {
        let end = a.runway_index(id).unwrap();
        assert!(a.rnav(end).is_some(), "RNAV to {id}");
    }
}

#[test]
fn wind_selects_south_or_north_flow() {
    assert_eq!(quiet_world(180.0).config().arrival, "18L");
    assert_eq!(quiet_world(360.0).config().arrival, "36R");
}

fn fly_ils(wind_from: f32, runway_id: &str) {
    let mut world = quiet_world(wind_from);
    let mut events = Vec::new();
    let rwy = world.airport.runway_index(runway_id).unwrap();
    let runway = world.airport.runways[rwy].clone();

    // 16 NM out and 3 NM off the course on a 20 degree intercept, outboard
    // of the parallels.
    let course = heading_vec(runway.heading);
    let left = -heading_vec(runway.heading + 90.0);
    let pos = runway.threshold() - course * 16.0 + left * 3.0;
    let ty = aircraft_type("MD11").unwrap();
    world.add_aircraft(Aircraft::new(1, "FDX1", Flow::Arrival, ty, pos, runway.heading + 20.0, 4000.0, 180.0));

    assert!(world.transmit(1, vec![Clearance::ClearedIls(rwy)]));
    run_until(&mut world, 400.0, &mut events, |w| {
        matches!(w.find(1).unwrap().phase, Phase::Localizer | Phase::Glideslope)
    });
    assert!(world.transmit(1, vec![Clearance::HandoffTower]));
    run_until(&mut world, 600.0, &mut events, |w| w.find(1).is_some_and(|a| a.phase == Phase::Rollout));
    assert_eq!(world.score.landed, 1, "{events:?}");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn ils_landing_works_on_18l() {
    fly_ils(180.0, "18L");
}

#[test]
fn ils_landing_works_on_36r() {
    fly_ils(360.0, "36R");
}

#[test]
fn every_star_spawns_outside_the_sector() {
    for wind in [180.0, 360.0] {
        let config = WorldConfig { seed: 1, wind: Some(Wind { from: wind, speed: 5.0 }), ..Default::default() };
        let mut world = World::new(kmem(), config);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..(900.0 / TICK) as usize {
            world.tick();
            for ac in world.aircraft.iter().filter(|a| a.flow == Flow::Arrival) {
                if seen.insert(ac.id) {
                    assert!(!world.airport.in_sector(ac.pos), "{} spawned inside at {:?}", ac.callsign, ac.pos);
                }
            }
        }
        assert!(!seen.is_empty());
    }
}

#[test]
fn a_shift_flies_the_fedex_hub_fleet_and_completes() {
    let mut fedex_heavies = 0;
    let mut seen = std::collections::BTreeMap::new();
    for seed in 0..4 {
        let config = WorldConfig {
            seed,
            wind: Some(Wind { from: 180.0, speed: 8.0 }),
            shift_seconds: 1800.0,
            ..Default::default()
        };
        let mut world = World::new(kmem(), config);
        while world.ended.is_none() {
            world.tick();
            world.drain_events();
            for ac in &world.aircraft {
                seen.insert((seed, ac.id), (ac.callsign.clone(), ac.ty.designator));
            }
        }
        assert_ne!(world.ended, Some(ShiftEnd::Decertified), "seed {seed}");
    }
    for (callsign, designator) in seen.values() {
        // Freighters fly as FedEx, the Caravans as Mountain Air Cargo.
        match *designator {
            "B77L" | "MD11" | "B763" | "A306" | "B752" => assert!(callsign.starts_with("FDX"), "{callsign} {designator}"),
            "C208" => assert!(callsign.starts_with("MTN"), "{callsign}"),
            "CRJ9" => assert!(callsign.starts_with("EDV"), "{callsign}"),
            _ => {}
        }
        if aircraft_type(designator).unwrap().wake == WakeCategory::Heavy {
            fedex_heavies += 1;
        }
    }
    assert!(seen.len() > 20, "{} aircraft", seen.len());
    assert!(fedex_heavies * 4 > seen.len(), "{fedex_heavies} heavies of {}", seen.len());
}

#[test]
fn ground_layout_uses_the_mapped_stands_and_reaches_every_departure_runway() {
    let a = kmem();
    let m = a.surface.as_ref().expect("KMEM has a ground layout");
    assert!(m.parking.len() >= 200, "{} parking spots", m.parking.len());
    for config in &a.configs {
        let end = a.runway_index(&config.departure).unwrap();
        let hold = m.departure_hold(end).unwrap();
        let gap = m.nodes[hold].pos.distance(a.runways[end].takeoff_point()) * FT_PER_NM;
        assert!(gap < 900.0, "{}: departure hold is {gap:.0} ft from the takeoff point", config.departure);
        for spot in &m.parking {
            let route = m.route(spot.node, hold, &[]);
            assert!(route.is_some(), "{} can't reach {}", spot.name, config.departure);
        }
    }
    // Gate numbers come from OSM, and each stand knows what it's for.
    assert!(m.parking.iter().any(|p| p.name.chars().all(|c| c.is_ascii_digit())));
    let count = |u| m.parking.iter().filter(|p| p.stand == Some(u)).count();
    assert!(count(StandUse::Gate) >= 30, "{} gates", count(StandUse::Gate));
    assert!(count(StandUse::Ramp) >= 150, "{} ramp stands", count(StandUse::Ramp));
    assert!(count(StandUse::Ga) >= 1, "no FBO spots");
}

#[test]
fn freighters_park_on_the_cargo_ramp_and_airliners_at_the_gates() {
    let mut checked = std::collections::BTreeMap::new();
    for seed in 0..3 {
        let config = WorldConfig {
            seed,
            staffing: Staffing::GROUND,
            wind: Some(Wind { from: 180.0, speed: 8.0 }),
            shift_seconds: 1800.0,
            ..Default::default()
        };
        let mut world = World::new(kmem(), config);
        while world.ended.is_none() {
            world.tick();
            world.drain_events();
            let map = world.airport.surface.as_ref().unwrap();
            for ac in &world.aircraft {
                // Arrivals are given a spot; departures start at theirs.
                let spot = ac.parking.or_else(|| {
                    let parked = ac.flow == Flow::Departure && ac.phase == Phase::Parked;
                    parked.then(|| map.parking.iter().position(|p| map.nodes[p.node].pos == ac.pos)).flatten()
                });
                let Some(spot) = spot else { continue };
                let expected = match ac.callsign.get(..3) {
                    Some("FDX" | "MTN") => StandUse::Ramp,
                    _ if ac.callsign.starts_with('N') => continue,
                    _ => StandUse::Gate,
                };
                let stand = map.parking[spot].stand;
                assert_eq!(stand, Some(expected), "{} at {}", ac.callsign, map.parking[spot].name);
                checked.insert((seed, ac.id), ac.flow);
            }
        }
    }
    let arrivals = checked.values().filter(|&&f| f == Flow::Arrival).count();
    let departures = checked.values().filter(|&&f| f == Flow::Departure).count();
    assert!(arrivals > 5 && departures > 5, "{arrivals} arrivals, {departures} departures");
}

#[test]
fn cargo_and_fbo_stands_are_reached_through_ramp_spots() {
    let a = kmem();
    let m = a.surface.as_ref().unwrap();
    let numbered = m.spots.iter().filter(|s| s.name.chars().all(|c| c.is_ascii_digit())).count();
    assert!(numbered >= 10, "{numbered} numbered spots");
    let wilson = m.spots.iter().position(|s| s.name == "Wilson Air").expect("an FBO spot");
    assert_eq!(m.spots[wilson].spoken(), "Wilson Air");
    assert_eq!(m.spots[0].spoken(), "spot 1");
    for p in &m.parking {
        match p.stand {
            Some(StandUse::Ramp | StandUse::Ga) => {
                let spot = p.spot.unwrap_or_else(|| panic!("{} has no spot", p.name));
                assert!(m.route(m.spots[spot].node, p.node, &[]).is_some(), "{} from its spot", p.name);
            }
            _ => assert!(p.spot.is_none(), "gate {} has a spot", p.name),
        }
    }
    assert!(m.parking.iter().any(|p| p.spot == Some(wilson)));
}

/// A Ground-only world with one arrival about to land on 18L.
fn landing_on_18l(callsign: &str, ty: &str) -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        staffing: Staffing::GROUND,
        wind: Some(Wind { from: 180.0, speed: 0.0 }),
        ..Default::default()
    };
    let mut world = World::new(kmem(), config);
    let rwy = world.airport.runway_index("18L").unwrap();
    let runway = world.airport.runways[rwy].clone();
    let pos = runway.threshold() - heading_vec(runway.heading) * 3.0;
    let ty = aircraft_type(ty).unwrap();
    let mut ac = Aircraft::new(1, callsign, Flow::Arrival, ty, pos, runway.heading, 1300.0, ty.vref);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = atcgame::sim::aircraft::Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    world.add_aircraft(ac);
    world
}

fn said(world: &World, text: &str) -> bool {
    world.log.iter().any(|l| l.text.contains(text))
}

/// Land, taxi to the spot it asks for, and see it handed over.
fn hand_over_at_its_spot(callsign: &str, ty: &str, asks_for: &str, stands: StandUse) {
    let mut world = landing_on_18l(callsign, ty);
    let mut events = Vec::new();
    run_until(&mut world, 400.0, &mut events, |w| w.find(1).is_some_and(|a| w.is_players(a)));
    let ac = world.find(1).unwrap();
    assert!(world.ramp_spot(ac).is_some(), "{callsign} should be bound for a ramp spot");
    let goal = world.taxi_in_goal(ac).unwrap();
    run_until(&mut world, 120.0, &mut events, |w| said(w, &format!("for {asks_for}")));
    assert!(world.transmit(1, vec![Clearance::Taxi { to: goal, via: vec![] }]));
    run_until(&mut world, 900.0, &mut events, |w| w.find(1).is_some_and(|a| a.frequency() == Position::Ramp));
    assert_eq!(world.score.parked, 1, "counted as parked at the spot: {events:?}");
    let stand = world.find(1).unwrap().parking.unwrap();
    let map = world.airport.surface.as_ref().unwrap();
    assert_eq!(map.parking[stand].stand, Some(stands));
    assert_eq!(world.find(1).unwrap().taxi.as_ref().unwrap().path.last(), Some(&map.parking[stand].node));
    run_until(&mut world, 900.0, &mut events, |w| w.find(1).is_none());
    assert_eq!(world.score.parked, 1, "parked once, not again at the stand");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
}

#[test]
fn a_fedex_arrival_is_handed_to_the_ramp_at_its_spot() {
    hand_over_at_its_spot("FDX2", "B763", "spot ", StandUse::Ramp);
}

#[test]
fn general_aviation_parks_itself_at_the_fbo() {
    hand_over_at_its_spot("N123AB", "C56X", "Wilson Air", StandUse::Ga);
}

#[test]
fn the_ramp_brings_a_fedex_departure_out_to_a_spot() {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        staffing: Staffing::GROUND,
        wind: Some(Wind { from: 180.0, speed: 0.0 }),
        ..Default::default()
    };
    let mut world = World::new(kmem(), config);
    let mut events = Vec::new();
    let route = world.airport.route_indices(&world.airport.sids[0].fixes.clone());
    world.request_departure(aircraft_type("MD11").unwrap(), route, 10000.0);
    run_until(&mut world, 30.0, &mut events, |w| !w.aircraft.is_empty());
    let ac = &world.aircraft[0];
    assert!(ac.callsign.starts_with("FDX"));
    assert_eq!(ac.frequency(), Position::Ramp, "with the ramp at its stand");
    assert!(!said(&world, "ready to taxi"), "no call to Ground from the stand");
    let id = ac.id;
    run_until(&mut world, 900.0, &mut events, |w| said(w, "ready to taxi"));
    let ac = world.find(id).unwrap();
    let map = world.airport.surface.as_ref().unwrap();
    let node = ac.taxi.as_ref().unwrap().node;
    let spot = map.spots.iter().find(|s| s.node == node).expect("waiting at a spot");
    assert!(said(&world, &format!("at {}, ready to taxi", spot.spoken())));
    assert_eq!(ac.phase, Phase::Parked);
    assert!(world.is_players(ac));
    assert!(ac.parking.is_none(), "its stand is free again");
}

#[test]
fn an_automated_shift_moves_everyone_through_the_ramps() {
    let config = WorldConfig {
        seed: 3,
        staffing: Staffing::NONE,
        wind: Some(Wind { from: 180.0, speed: 8.0 }),
        shift_seconds: 3600.0,
        ..Default::default()
    };
    let mut world = World::new(kmem(), config);
    let mut most_on_ground = 0;
    let mut handed = std::collections::BTreeSet::new();
    while world.ended.is_none() {
        world.tick();
        world.drain_events();
        most_on_ground = most_on_ground.max(world.aircraft.iter().filter(|a| a.on_ground).count());
        handed.extend(world.aircraft.iter().filter(|a| a.frequency() == Position::Ramp).map(|a| a.id));
    }
    assert!(world.score.parked >= 8, "{} parked", world.score.parked);
    assert!(handed.len() >= 8, "{} went through the ramp", handed.len());
    assert!(most_on_ground < 30, "{most_on_ground} on the ground at once: something is stuck");
}

#[test]
fn the_map_names_ramps_instead_of_their_stands() {
    let a = kmem();
    let m = a.surface.as_ref().unwrap();
    assert!(m.ramp_labels.iter().any(|l| l.fbo.as_deref() == Some("Wilson Air")), "{:?}", m.ramp_labels);
    let cargo = m.ramp_labels.iter().filter(|l| l.fbo.is_none()).count();
    assert!((1..=8).contains(&cargo), "{cargo} FedEx ramp labels");
    let lane = |name: &str| m.edges.iter().filter(|e| e.taxiway() == Some(name)).all(|e| e.ramp);
    assert!(lane("Winchester") && lane("700"), "FBO and FedEx lanes are the ramps'");
    for name in ["A", "M", "S", "V"] {
        assert!(m.edges.iter().filter(|e| e.taxiway() == Some(name)).all(|e| !e.ramp), "{name} is Ground's");
    }
}

#[test]
fn fbo_stands_go_by_the_fbo_name() {
    let a = kmem();
    let m = a.surface.as_ref().unwrap();
    let g = m.parking.iter().position(|p| p.stand == Some(StandUse::Ga) && p.spot.is_some()).unwrap();
    assert_eq!(m.stand_name(g), "Wilson Air");
    let gate = m.parking.iter().position(|p| p.stand == Some(StandUse::Gate)).unwrap();
    assert_eq!(m.stand_name(gate), m.parking[gate].name);
}
