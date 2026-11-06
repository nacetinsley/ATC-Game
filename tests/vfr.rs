//! VFR traffic: the pattern side from the airport data, touch-and-goes,
//! joining the pattern, landing clearances, and clean automated shifts.

use atcgame::sim::aircraft::{Leg, Phase, VfrIntent};
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND, VfrRunway};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::positions::Staffing;
use atcgame::sim::scoring::{GoAroundReason, SimEvent};
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{TICK, World, WorldConfig};

fn ksus() -> Airport {
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

fn world(airport: Airport, wind_from: f32, staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 11,
        traffic: false,
        wind: Some(Wind { from: wind_from, speed: 5.0 }),
        staffing,
        vfr: true,
        // Long enough for a few circuits of Harrowgate's long runway.
        shift_seconds: 7200.0,
        ..Default::default()
    };
    World::new(airport, config)
}

/// Tick until `done`, for at most `limit` sim seconds. Returns whether it got there.
fn run_until(world: &mut World, limit: f32, events: &mut Vec<SimEvent>, mut done: impl FnMut(&World) -> bool) -> bool {
    let end = world.time + limit;
    while world.time < end && world.ended.is_none() {
        world.tick();
        events.extend(world.drain_events());
        if done(world) {
            return true;
        }
    }
    false
}

fn newest(world: &World) -> u32 {
    world.aircraft.iter().map(|a| a.id).max().expect("an aircraft")
}

fn leg(world: &World, id: u32) -> Option<Leg> {
    world.find(id).and_then(|a| a.vfr.as_ref()).map(|v| v.leg)
}

/// A closed-traffic aircraft, every position automated: it taxis out, flies
/// its circuits on the side the data gives, then lands and parks. Returns the
/// downwind's distance right of the runway, at its nearest and furthest.
fn fly_closed_traffic(airport: Airport, wind_from: f32, runway: &str) -> (f32, f32) {
    let mut world = world(airport, wind_from, Staffing::NONE);
    assert!(world.spawn_vfr(VfrIntent::Closed));
    let mut events = Vec::new();
    assert!(run_until(&mut world, 10.0, &mut events, |w| !w.aircraft.is_empty()));
    let id = newest(&world);
    let circuits = world.find(id).unwrap().vfr.as_ref().unwrap().circuits;
    let end = world.pattern_runway();
    assert_eq!(world.airport.runways[end].id, runway);
    let threshold = world.airport.runways[end].threshold();
    let right = heading_vec(world.airport.runways[end].heading + 90.0);

    let (mut low, mut high) = (f32::MAX, f32::MIN);
    let parked = run_until(&mut world, 3600.0, &mut events, |w| {
        if let Some(ac) = w.find(id)
            && leg(w, id) == Some(Leg::Downwind)
            && ac.phase == Phase::Pattern
        {
            let offset = (ac.pos - threshold).dot(right);
            low = low.min(offset);
            high = high.max(offset);
        }
        w.find(id).is_none()
    });
    assert!(parked, "still flying after an hour: {:?}", world.find(id));
    let touch_and_goes = events.iter().filter(|e| matches!(e, SimEvent::TouchAndGo { .. })).count();
    assert_eq!(touch_and_goes as u32, circuits, "{events:?}");
    assert!(events.iter().any(|e| matches!(e, SimEvent::Landed { .. })), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, SimEvent::Parked { .. })), "{events:?}");
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
    (low, high)
}

#[test]
fn ksus_26r_has_right_traffic_from_the_faa_data() {
    let (low, high) = fly_closed_traffic(ksus(), 250.0, "26R");
    assert!(low > 0.3 && high < 1.5, "downwind {low}..{high} NM right of 26R");
}

#[test]
fn ksus_08l_has_the_standard_left_pattern() {
    let (low, high) = fly_closed_traffic(ksus(), 80.0, "08L");
    assert!(high < -0.3 && low > -1.5, "downwind {low}..{high} NM right of 08L, should be left");
}

#[test]
fn harrowgate_patterns_are_flown_outside_the_parallels() {
    // 27R is the north runway: right traffic puts the pattern further north.
    let (low, high) = fly_closed_traffic(Airport::default_airport(), 270.0, "27R");
    assert!(low > 0.3 && high < 1.5, "downwind {low}..{high} NM right of 27R");
}

#[test]
fn a_vfr_arrival_holds_outside_until_told_to_enter_then_lands_when_cleared() {
    let mut world = world(ksus(), 250.0, Staffing::LOCAL);
    assert!(world.spawn_vfr(VfrIntent::Arrival));
    let id = newest(&world);
    assert!(world.log.iter().any(|l| l.text.contains("inbound for landing")), "{:?}", world.log);
    let mut events = Vec::new();

    // Nobody tells it how to enter: it orbits outside.
    let mut closest = f32::MAX;
    run_until(&mut world, 600.0, &mut events, |w| {
        closest = closest.min(w.find(id).unwrap().pos.length());
        false
    });
    assert_eq!(leg(&world, id), Some(Leg::Holding));
    assert!(closest > 3.0, "came within {closest} NM without an entry instruction");
    assert!(world.log.iter().any(|l| l.text.contains("holding outside")), "{:?}", world.log);

    let end = world.pattern_runway();
    assert!(world.transmit(id, vec![Clearance::EnterDownwind(end)]));
    assert!(run_until(&mut world, 600.0, &mut events, |w| leg(w, id) == Some(Leg::Downwind)));
    assert!(world.transmit(id, vec![Clearance::ClearedToLand(end)]));
    assert!(run_until(&mut world, 600.0, &mut events, |w| w.score.landed == 1), "{:?}", world.find(id));
    assert!(!events.iter().any(|e| matches!(e, SimEvent::GoAround { .. })), "{events:?}");
}

#[test]
fn no_landing_clearance_sends_it_around_and_back_into_the_pattern() {
    let mut world = world(Airport::default_airport(), 270.0, Staffing::LOCAL);
    assert!(world.spawn_vfr(VfrIntent::Arrival));
    let id = newest(&world);
    let end = world.pattern_runway();
    assert!(world.transmit(id, vec![Clearance::EnterDownwind(end)]));
    let mut events = Vec::new();
    let went_around = run_until(&mut world, 1200.0, &mut events, |w| {
        w.score.go_arounds > 0
    });
    assert!(went_around, "{:?}", world.find(id));
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::GoAround { reason: GoAroundReason::NoLandingClearance, .. }
    )));
    let ac = world.find(id).unwrap();
    assert_eq!(ac.phase, Phase::Pattern);
    assert_eq!(leg(&world, id), Some(Leg::Upwind));
    assert!(ac.target_altitude < world.airport.elevation + 1500.0, "climbs to pattern altitude, not the missed approach");
}

#[test]
fn extend_downwind_holds_off_the_base_turn_until_called() {
    let mut world = world(Airport::default_airport(), 270.0, Staffing::LOCAL);
    assert!(world.spawn_vfr(VfrIntent::Arrival));
    let id = newest(&world);
    let end = world.pattern_runway();
    let runway = world.airport.runways[end].clone();
    assert!(world.transmit(id, vec![Clearance::EnterDownwind(end)]));
    let mut events = Vec::new();
    assert!(run_until(&mut world, 900.0, &mut events, |w| leg(w, id) == Some(Leg::Downwind)));
    assert!(world.transmit(id, vec![Clearance::ExtendDownwind]));

    // Past the normal base turn, 1.2 NM beyond the threshold, it keeps going.
    let behind = |w: &World| runway.along(w.find(id).unwrap().pos);
    assert!(run_until(&mut world, 600.0, &mut events, |w| behind(w) > 2.5));
    assert_eq!(leg(&world, id), Some(Leg::Downwind));
    assert!(world.transmit(id, vec![Clearance::TurnBase]));
    assert!(run_until(&mut world, 30.0, &mut events, |w| leg(w, id) == Some(Leg::Base)));
}

#[test]
fn a_touch_and_go_is_refused_by_a_pilot_who_wants_a_full_stop() {
    let mut world = world(Airport::default_airport(), 270.0, Staffing::LOCAL);
    assert!(world.spawn_vfr(VfrIntent::Arrival));
    let id = newest(&world);
    let end = world.pattern_runway();
    world.transmit(id, vec![Clearance::EnterDownwind(end)]);
    let mut events = Vec::new();
    assert!(run_until(&mut world, 900.0, &mut events, |w| leg(w, id) == Some(Leg::Downwind)));
    world.transmit(id, vec![Clearance::ClearedTouchAndGo(end)]);
    run_until(&mut world, 10.0, &mut events, |_| false);
    assert!(world.log.iter().any(|l| l.text.contains("unable") && l.text.contains("full stop")), "{:?}", world.log);
    assert!(world.find(id).unwrap().vfr.as_ref().unwrap().landing.is_none());
}

#[test]
fn approach_alone_never_gets_vfr_traffic() {
    let config = WorldConfig { seed: 3, vfr: true, staffing: Staffing::APPROACH, ..Default::default() };
    let mut world = World::new(Airport::default_airport(), config);
    for _ in 0..(1200.0 / TICK) as usize {
        world.tick();
        assert!(world.aircraft.iter().all(|a| a.vfr.is_none()));
    }
}

/// A whole shift, every position automated, VFR on.
fn automated_shift(airport: Airport, wind_from: f32, seed: u64) -> World {
    let config = WorldConfig {
        seed,
        wind: Some(Wind { from: wind_from, speed: 8.0 }),
        staffing: Staffing::NONE,
        vfr: true,
        ..Default::default()
    };
    let mut world = World::new(airport, config);
    while world.ended.is_none() {
        world.tick();
        world.drain_events();
    }
    world
}

#[test]
fn automated_shifts_with_vfr_traffic_are_clean() {
    let mut touch_and_goes = 0;
    for (airport, wind, seed) in [
        (Airport::default_airport(), 270.0, 0),
        (Airport::default_airport(), 90.0, 1),
        (ksus(), 250.0, 2),
        (ksus(), 80.0, 3),
    ] {
        let code = airport.code.clone();
        let world = automated_shift(airport, wind, seed);
        assert!(world.score.incidents.is_empty(), "{code} seed {seed}: {:?}", world.score.incidents);
        touch_and_goes += world.score.touch_and_goes;
    }
    assert!(touch_and_goes > 4, "{touch_and_goes} touch-and-goes");
}

/// With VFR on, the pattern has the configured departure runway to itself and
/// IFR departures share the arrival runway; with it off, nothing moves.
#[test]
fn vfr_traffic_gets_the_runway_its_airport_gives_it() {
    for airport in [Airport::default_airport(), ksus()] {
        let code = airport.code.clone();
        let with = world(airport.clone(), 250.0, Staffing::LOCAL_GROUND);
        let ids = |w: &World, end: usize| w.airport.runways[end].id.clone();
        assert_eq!(ids(&with, with.pattern_runway()), with.config().departure, "{code}");
        match airport.vfr_runway {
            // KSUS: VFR takes the parallel, IFR departures share the arrival runway.
            VfrRunway::Own => {
                assert_eq!(with.departure_runway(), with.arrival_runway(), "{code}");
                assert_ne!(with.pattern_runway(), with.departure_runway(), "{code}");
            }
            // Harrowgate: the jets keep their departure runway and VFR shares it.
            VfrRunway::Shared => assert_eq!(with.pattern_runway(), with.departure_runway(), "{code}"),
        }

        let mut config = WorldConfig { seed: 11, traffic: false, staffing: Staffing::LOCAL_GROUND, ..Default::default() };
        config.wind = Some(Wind { from: 250.0, speed: 5.0 });
        let without = World::new(airport, config);
        assert_eq!(ids(&without, without.departure_runway()), without.config().departure, "{code}");
    }
    assert_eq!(Airport::default_airport().vfr_runway, VfrRunway::Shared);
    assert_eq!(ksus().vfr_runway, VfrRunway::Own);
}

#[test]
fn harrowgate_sees_half_the_light_aircraft_of_ksus() {
    assert_eq!(ksus().vfr_rate, atcgame::sim::vfr::VFR_RATE);
    assert_eq!(Airport::default_airport().vfr_rate, ksus().vfr_rate / 2.0);
}
