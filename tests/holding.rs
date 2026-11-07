//! Holding patterns: the instruction, the entry, the racetrack and leaving it.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::{Clearance, phrase};
use atcgame::sim::holding::{HoldLeg, Turns};
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::Staffing;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{TICK, World, WorldConfig};

fn world(airport: Airport, wind: f32) -> World {
    let config = WorldConfig {
        seed: 5,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: wind }),
        staffing: Staffing::APPROACH,
        clock_start: 14.0 * 3600.0,
        ..Default::default()
    };
    World::new(airport, config)
}

/// A jet 10 NM from `fix`, flying towards it on `heading`.
fn inbound(world: &World, fix: usize, heading: f32) -> Aircraft {
    let at = world.airport.fixes[fix].pos() - heading_vec(heading) * 10.0;
    let mut ac = Aircraft::new(1, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), at, heading, 8000.0, 250.0);
    ac.entered_sector = true;
    ac.lateral = Lateral::Direct(fix);
    ac
}

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds / TICK) as usize {
        world.tick();
        world.drain_events();
    }
}

fn hold_state(world: &World) -> atcgame::sim::holding::HoldState {
    match world.find(1).unwrap().lateral {
        Lateral::Hold(state) => state,
        other => panic!("not holding: {other:?}"),
    }
}

#[test]
fn an_arrival_holds_flies_circuits_and_leaves_on_a_vector() {
    let mut w = world(Airport::default_airport(), 20.0);
    let fix = w.airport.fix_index("ALDER").unwrap();
    w.add_aircraft(inbound(&w, fix, 200.0));
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(spec.published);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 20.0 * 60.0);
    let state = hold_state(&w);
    assert!(state.circuits >= 3, "{} circuits", state.circuits);
    let ac = w.find(1).unwrap();
    assert!(ac.ias <= 230.0 + 1.0, "holding at {} kt", ac.ias);
    assert!(ac.pos.distance(w.airport.fixes[fix].pos()) < 9.0, "wandered off");
    // Left there past its EFC: it asked, and being forgotten cost points.
    assert!(w.log.iter().any(|l| l.text.contains("NVA101, holding at ALDER, it's our EFC time")));
    assert_eq!(w.score.incidents.len(), 1, "{:?}", w.score.incidents);
    assert!(w.score.incidents[0].text.contains("past its EFC"));

    assert!(w.transmit(1, vec![Clearance::Heading(270.0)]));
    run(&mut w, 10.0);
    assert_eq!(w.find(1).unwrap().lateral, Lateral::Heading(270.0));
}

#[test]
fn an_unpublished_hold_is_on_the_course_flown_with_right_turns() {
    let mut w = world(Airport::default_airport(), 0.0);
    let fix = w.airport.fix_index("TOPAZ").unwrap();
    w.add_aircraft(inbound(&w, fix, 90.0));
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(!spec.published);
    assert!((spec.inbound - 90.0).abs() < 0.5);
    assert_eq!(spec.turns, Turns::Right);
    assert_eq!(spec.leg, HoldLeg::Standard);
    // Ten minutes on, at a whole minute: 1410 when the shift starts at 1400.
    assert_eq!(spec.efc_hhmm, 1410);
    let words = phrase(&Clearance::Hold(spec), w.find(1).unwrap(), &w.airport, &[]);
    assert_eq!(
        words,
        "hold west of TOPAZ on the 090 course inbound, right turns, 1-minute legs, expect further clearance 1410"
    );
}

#[test]
fn a_published_hold_is_said_as_published() {
    let w = world(Airport::default_airport(), 0.0);
    let fix = w.airport.fix_index("ALDER").unwrap();
    let ac = inbound(&w, fix, 216.0);
    let spec = w.hold_for(&ac, fix);
    let words = phrase(&Clearance::Hold(spec), &ac, &w.airport, &[]);
    assert_eq!(words, "hold northeast of ALDER as published, expect further clearance 1410");
}

#[test]
fn resuming_the_route_leaves_the_hold_for_the_next_fix() {
    let mut w = world(Airport::default_airport(), 10.0);
    let fix = w.airport.fix_index("CEDAR").unwrap();
    let mut ac = inbound(&w, fix, 120.0);
    ac.route = w.airport.route_indices(&["DUNLO".into(), "CEDAR".into(), "ALDER".into()]);
    ac.route_index = 1;
    ac.lateral = Lateral::Route;
    w.add_aircraft(ac);
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 6.0 * 60.0);
    hold_state(&w);
    assert!(w.transmit(1, vec![Clearance::ResumeRoute]));
    run(&mut w, 10.0);
    let ac = w.find(1).unwrap();
    assert_eq!(ac.lateral, Lateral::Route);
    assert_eq!(w.airport.fixes[ac.route[ac.route_index]].name, "ALDER");
}

#[test]
fn an_approach_clearance_from_the_hold_needs_vectors_first() {
    let mut w = world(Airport::default_airport(), 0.0);
    let fix = w.airport.fix_index("ALDER").unwrap();
    w.add_aircraft(inbound(&w, fix, 216.0));
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 30.0);
    let rwy = w.airport.runway_index("27R").unwrap();
    assert!(w.transmit(1, vec![Clearance::ClearedIls(rwy)]));
    run(&mut w, 10.0);
    hold_state(&w);
    let last = &w.log.last().unwrap().text;
    assert!(last.contains("unable") && last.contains("vectors"), "{last}");
}

#[test]
fn ksus_holds_come_from_the_faa_data() {
    let a = Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap();
    assert!(a.holds.len() >= 5, "{} holds", a.holds.len());
    let ftz = a.holds.iter().find(|h| h.fix == "FTZ").expect("FTZ missed-approach hold");
    assert_eq!(ftz.turns, Turns::Right);
    assert_eq!(ftz.leg_minutes, Some(1.0));
    // Every hold flies cleanly at KSUS's low altitudes.
    for hold in &a.holds {
        let mut w = world(a.clone(), 15.0);
        let fix = w.airport.fix_index(&hold.fix).unwrap();
        let mut ac = inbound(&w, fix, hold.inbound + 40.0);
        ac.altitude = hold.altitude.unwrap_or(4000.0).max(4000.0);
        ac.target_altitude = ac.altitude;
        w.add_aircraft(ac);
        let spec = w.hold_for(w.find(1).unwrap(), fix);
        assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
        run(&mut w, 14.0 * 60.0);
        let state = hold_state(&w);
        assert!(state.circuits >= 2, "{}: {} circuits", hold.fix, state.circuits);
        let d = w.find(1).unwrap().pos.distance(w.airport.fixes[fix].pos());
        assert!(d < 10.0, "{}: {d:.1} NM from the fix", hold.fix);
    }
}

#[test]
fn vfr_traffic_and_aircraft_on_the_ground_refuse_a_hold() {
    let mut w = world(Airport::default_airport(), 0.0);
    let fix = w.airport.fix_index("ALDER").unwrap();
    let mut ac = inbound(&w, fix, 216.0);
    ac.on_ground = true;
    w.add_aircraft(ac);
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 10.0);
    assert!(!matches!(w.find(1).unwrap().lateral, Lateral::Hold(_)));
}


#[test]
fn a_new_clearance_before_the_efc_costs_nothing() {
    let mut w = world(Airport::default_airport(), 0.0);
    let fix = w.airport.fix_index("ALDER").unwrap();
    w.add_aircraft(inbound(&w, fix, 216.0));
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 8.0 * 60.0);
    assert_eq!(w.awaiting(w.find(1).unwrap()), None);
    run(&mut w, 70.0);
    assert_eq!(w.awaiting(w.find(1).unwrap()), Some(atcgame::sim::awaiting::Awaiting::Efc));
    // A fresh EFC, said again with the hold.
    let spec = w.hold_for(w.find(1).unwrap(), fix);
    assert!(w.transmit(1, vec![Clearance::Hold(spec)]));
    run(&mut w, 5.0 * 60.0);
    assert!(w.score.incidents.is_empty(), "{:?}", w.score.incidents);
    assert_eq!(w.awaiting(w.find(1).unwrap()), None);
}

#[test]
fn tower_only_traffic_waits_in_holds_when_the_final_is_full_and_lands() {
    let config = WorldConfig {
        seed: 11,
        busyness: atcgame::sim::traffic::Busyness::Busy,
        staffing: Staffing::LOCAL,
        clock_start: 14.0 * 3600.0,
        shift_seconds: 3600.0,
        ..Default::default()
    };
    let mut w = World::new(Airport::default_airport(), config);
    let mut most_held = 0;
    let mut events = Vec::new();
    let mut held_ids = std::collections::HashSet::new();
    let mut released = 0;
    let mut cleared = std::collections::HashSet::new();
    for _ in 0..(40.0 * 60.0 / TICK) as usize {
        w.tick();
        events.extend(w.drain_events());
        // The player clears each arrival to land as it calls.
        let calling: Vec<(u32, usize)> = w
            .aircraft
            .iter()
            .filter(|a| a.phase == Phase::TowerHandedOff && !a.cleared_to_land && !cleared.contains(&a.id))
            .filter_map(|a| a.approach_runway.map(|r| (a.id, r)))
            .collect();
        for (id, rwy) in calling {
            cleared.insert(id);
            w.transmit(id, vec![Clearance::ClearedToLand(rwy)]);
            if held_ids.contains(&id) {
                released += 1;
            }
        }
        for a in &w.aircraft {
            if matches!(a.lateral, Lateral::Hold(_)) {
                held_ids.insert(a.id);
            }
        }
        let held: Vec<_> = w.aircraft.iter().filter(|a| matches!(a.lateral, Lateral::Hold(_))).collect();
        for a in &held {
            assert!(w.airport.in_sector(a.pos), "{} left the sector holding", a.callsign);
            assert!(!w.is_players(a));
        }
        most_held = most_held.max(held.len());
    }
    assert!(most_held >= 1, "nobody held");
    assert!(released >= 3, "{released} released from the hold");
    assert!(most_held <= 8, "the stack kept growing: {most_held}");
    assert!(w.score.landed >= 10, "{} landed", w.score.landed);
    assert!(
        !w.score.incidents.iter().any(|i| i.text.to_lowercase().contains("sector") || i.text.contains("separation")),
        "{:?}",
        w.score.incidents
    );
    println!("most held at once: {most_held}, released {released}, landed: {}", w.score.landed);
}


#[test]
fn hold_skips_the_gate_on_the_sector_edge_for_the_next_fix() {
    let w = world(Airport::default_airport(), 0.0);
    let star = w.airport.stars.iter().find(|s| s.fixes[0] == "WAVRY").expect("a STAR from WAVRY");
    let route = w.airport.route_indices(&star.fixes);
    let gate = route[0];
    let mut ac = inbound(&w, gate, 45.0);
    ac.route = route.clone();
    ac.route_index = 0;
    ac.lateral = Lateral::Route;
    assert!(!w.hold_fits(&ac, gate), "the gate's hold leaves the sector");
    let fix = w.hold_fix_for(&ac).unwrap();
    assert_ne!(fix, gate);
    assert!(route.contains(&fix));
    assert!(w.hold_fits(&ac, fix));
}
