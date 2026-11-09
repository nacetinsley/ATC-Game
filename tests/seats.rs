//! Players in seats: in co-op each player has their own positions, says
//! things only as those, and hears only their own frequencies. Single
//! player is one seat working every staffed position.

use atcgame::sim::aircraft::{Aircraft, Flow, Lateral, Phase};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::performance::aircraft_type;
use atcgame::sim::positions::{Position, SOLO, Seat, Staffing};
use atcgame::sim::replay::Action;
use atcgame::sim::units::{Wind, heading_vec};
use atcgame::sim::{Speaker, TICK, Talker, World, WorldConfig};

const APPROACH_SEAT: Seat = 0;
const TOWER_SEAT: Seat = 1;

/// Approach in one seat, Local and Ground in the other.
fn split() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, APPROACH_SEAT)
        .with_seat(Position::Local, TOWER_SEAT)
        .with_seat(Position::Ground, TOWER_SEAT)
}

fn world(staffing: Staffing) -> World {
    let config = WorldConfig {
        seed: 7,
        traffic: false,
        wind: Some(Wind { from: 270.0, speed: 0.0 }),
        staffing,
        ..Default::default()
    };
    World::new(Airport::default_airport(), config)
}

/// An arrival on the 27L localizer, still with Approach.
fn on_the_localizer(world: &World, id: u32, miles: f32) -> Aircraft {
    let rwy = world.airport.runway_index("27L").unwrap();
    let runway = &world.airport.runways[rwy];
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = world.airport.glideslope_altitude(rwy, miles);
    let mut ac = Aircraft::new(id, "NVA101", Flow::Arrival, aircraft_type("B738").unwrap(), pos, runway.heading, altitude, 180.0);
    ac.phase = Phase::Localizer;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac
}

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds / TICK) as u32 {
        world.tick();
    }
}

#[test]
fn a_player_speaks_only_as_their_own_positions() {
    let mut w = world(split());
    let ac = on_the_localizer(&w, 1, 10.0);
    w.add_aircraft(ac);
    let ac = w.find(1).unwrap().clone();
    let heading = Clearance::Heading(250.0);
    assert_eq!(w.issuer_as(APPROACH_SEAT, &ac, &heading), Some(Position::Approach));
    assert_eq!(w.issuer_as(TOWER_SEAT, &ac, &heading), None, "Tower can't vector Approach's traffic");
    // Tower can try, but the pilot isn't listening to them.
    assert!(w.act(TOWER_SEAT, 1, Action::Transmit(vec![heading.clone()])));
    run(&mut w, 8.0);
    assert!(w.log.iter().any(|l| l.text.contains("not on your frequency")), "{:#?}", w.log);
    assert_ne!(w.find(1).unwrap().assigned_heading(), Some(250.0));
}

#[test]
fn only_approach_accepts_centers_offer() {
    let mut w = world(split());
    let mut ac = on_the_localizer(&w, 1, 30.0);
    ac.phase = Phase::Offered;
    w.add_aircraft(ac);
    assert!(!w.act(TOWER_SEAT, 1, Action::AcceptHandoff));
    assert_eq!(w.find(1).unwrap().phase, Phase::Offered);
    assert!(w.act(APPROACH_SEAT, 1, Action::AcceptHandoff));
    assert_eq!(w.find(1).unwrap().phase, Phase::Controlled);
}

#[test]
fn each_player_hears_their_own_frequency() {
    let mut w = world(split());
    let ac = on_the_localizer(&w, 1, 10.0);
    w.add_aircraft(ac);
    assert!(w.act(APPROACH_SEAT, 1, Action::Transmit(vec![Clearance::Speed(170.0)])));
    run(&mut w, 8.0);
    let mine: Vec<_> = w.log.iter().filter(|l| l.radio.is_some()).collect();
    assert!(mine.len() >= 2, "the call and the readback: {mine:#?}");
    for line in mine {
        assert!(line.shown_to(APPROACH_SEAT), "{line:?}");
        assert!(!line.shown_to(TOWER_SEAT), "Tower isn't on Approach's frequency: {line:?}");
    }
    let call = w.log.iter().find(|l| l.speaker == Speaker::Controller).unwrap();
    assert_eq!(call.radio.as_ref().unwrap().talker, Talker::Controller(APPROACH_SEAT));
}

#[test]
fn handed_to_tower_the_pilot_checks_in_with_the_other_player() {
    let mut w = world(split());
    let ac = on_the_localizer(&w, 1, 10.0);
    w.add_aircraft(ac);
    assert!(w.act(APPROACH_SEAT, 1, Action::Transmit(vec![Clearance::HandoffTower])));
    run(&mut w, 15.0);
    assert_eq!(w.find(1).unwrap().frequency(), Position::Local);
    let check_in = w
        .log
        .iter()
        .find(|l| l.speaker == Speaker::Pilot && l.text.starts_with("Tower,"))
        .unwrap_or_else(|| panic!("a check-in with Tower: {:#?}", w.log));
    assert!(check_in.shown_to(TOWER_SEAT) && !check_in.shown_to(APPROACH_SEAT), "{check_in:?}");
}

#[test]
fn one_player_on_both_gets_no_check_in() {
    // Combined, it's the same controller: the pilot is already talking to them.
    let mut w = world(Staffing::ALL);
    let ac = on_the_localizer(&w, 1, 10.0);
    w.add_aircraft(ac);
    assert!(w.transmit(1, vec![Clearance::HandoffTower]));
    run(&mut w, 15.0);
    assert_eq!(w.find(1).unwrap().frequency(), Position::Local);
    assert!(!w.log.iter().any(|l| l.speaker == Speaker::Pilot && l.text.starts_with("Tower,")), "{:#?}", w.log);
}

#[test]
fn commands_remember_who_said_them_and_replay_the_same() {
    let mut w = world(split());
    let ac = on_the_localizer(&w, 1, 10.0);
    w.add_aircraft(ac.clone());
    let start = w.clone();
    assert!(w.act(APPROACH_SEAT, 1, Action::Transmit(vec![Clearance::HandoffTower])));
    run(&mut w, 15.0);
    assert!(w.act(TOWER_SEAT, 1, Action::Transmit(vec![Clearance::ClearedToLand(w.airport.runway_index("27L").unwrap())])));
    run(&mut w, 15.0);
    let seats: Vec<Seat> = w.commands().iter().map(|c| c.seat).collect();
    assert_eq!(seats, [APPROACH_SEAT, TOWER_SEAT]);
    // Rebuilt from the same start and the same commands, it's the same shift.
    let mut again = start;
    let commands = w.commands().to_vec();
    again.run_to(w.ticks(), &commands);
    assert_eq!(again.checksum(), w.checksum());
}

#[test]
fn a_seat_can_only_answer_and_ask_for_its_own_positions() {
    let mut w = world(split());
    // Ground's request, from the Approach seat: refused, and not recorded.
    assert!(!w.act(APPROACH_SEAT, 1, Action::RequestCrossing(0)));
    assert!(!w.act(APPROACH_SEAT, 1, Action::RequestRelease));
    assert!(!w.act(APPROACH_SEAT, 1, Action::CallGoAround));
    assert!(w.commands().is_empty());
    // A single player is seat 0 and works everything staffed.
    let solo = world(Staffing::ALL);
    assert_eq!(solo.staffing().seat(Position::Ground), Some(SOLO));
}

#[test]
fn a_command_saved_before_seats_is_the_single_players() {
    let old = "tick = 3\nid = 9\naction = \"AcceptHandoff\"\n";
    let command: atcgame::sim::replay::Command = toml::from_str(old).expect("an old command loads");
    assert_eq!(command.seat, SOLO);
}
