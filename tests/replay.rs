//! Replays: a shift rebuilt from its seed and the player's commands is the
//! same shift, and seeking lands on the same world as running straight there.

use atcgame::sim::aircraft::Phase;
use atcgame::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::emergency::EmergencyRate;
use atcgame::sim::mistakes::PilotMistakes;
use atcgame::sim::positions::Staffing;
use atcgame::sim::replay::{Recording, ShiftCode};
use atcgame::sim::scoring::ShiftEnd;
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{World, WorldConfig};

fn ksus() -> Airport {
    Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()
}

fn config() -> WorldConfig {
    WorldConfig {
        seed: 31,
        staffing: Staffing::ALL,
        vfr: true,
        emergencies: EmergencyRate::Frequent,
        mistakes: PilotMistakes::Frequent,
        weather: WeatherKind::Storms,
        shift_seconds: 20.0 * 60.0,
        ..Default::default()
    }
}

/// A player who takes every handoff and gives a few instructions: enough to
/// make a shift depend on what was said.
fn player(world: &mut World, step: u32) {
    if !step.is_multiple_of(40) {
        return;
    }
    let ids: Vec<(u32, Phase, bool)> = world.aircraft.iter().map(|a| (a.id, a.phase, a.on_ground)).collect();
    for (n, (id, phase, on_ground)) in ids.into_iter().enumerate() {
        if phase == Phase::Offered {
            world.accept_handoff(id);
        } else if !on_ground && (n as u32 + step / 40).is_multiple_of(5) {
            let heading = (step * 7 + id * 30) % 360;
            world.transmit(id, vec![Clearance::Heading(heading as f32), Clearance::Altitude(6000.0)]);
        } else if on_ground && (n as u32 + step / 40).is_multiple_of(7) {
            world.transmit(id, vec![Clearance::HoldPosition]);
        }
    }
}

/// Play the shift, recording as the game does: commands land between ticks.
fn play() -> (World, Recording) {
    let mut world = World::new(ksus(), config());
    let mut recording = Recording::new(&world);
    let mut step = 0;
    while world.ended.is_none() {
        player(&mut world, step);
        world.tick();
        recording.update(&world);
        step += 1;
    }
    recording.finish(&world);
    (world, recording)
}

#[test]
fn a_rebuilt_shift_is_the_same_shift() {
    let (played, recording) = play();
    assert!(recording.commands.len() > 20, "the player said {} things", recording.commands.len());

    let mut rebuilt = Recording::from_start(World::new(ksus(), config()), recording.commands.clone());
    let mut chunks = 0;
    while !rebuilt.rebuild_step(500) {
        chunks += 1;
        assert!(chunks < 1000, "the rebuild never finishes");
    }
    assert_eq!(rebuilt.last().state_digest(), played.state_digest());
    assert_eq!(rebuilt.last().summary(), played.summary());
}

#[test]
fn seeking_lands_where_running_straight_there_does() {
    let (_, recording) = play();
    for tick in [0, 1, 239, 240, 241, 1000, 2399, 4000] {
        let mut straight = World::new(ksus(), config());
        straight.run_to(tick, &recording.commands);
        assert_eq!(recording.seek(tick).state_digest(), straight.state_digest(), "tick {tick}");
    }
}

#[test]
fn taking_over_keeps_the_past_and_changes_the_future() {
    let (played, mut recording) = play();
    let at = 2000;
    let mut world = recording.seek(at);
    let before = world.state_digest();
    recording.take_over(&world);
    assert!(recording.commands.iter().all(|c| c.tick < at));
    assert_eq!(recording.seek(at).state_digest(), before, "the past is kept");

    // Play on without saying anything.
    while world.ended.is_none() {
        world.tick();
        recording.update(&world);
    }
    recording.finish(&world);
    assert_ne!(world.state_digest(), played.state_digest());
    assert_eq!(recording.seek(at).state_digest(), before);
    // And the new ending replays too.
    let mut rebuilt = Recording::from_start(World::new(ksus(), config()), recording.commands.clone());
    while !rebuilt.rebuild_step(5000) {}
    assert_eq!(rebuilt.last().state_digest(), world.state_digest());
}

#[test]
fn a_shift_code_gives_the_same_traffic() {
    let code = ShiftCode::parse(&ShiftCode::of(&config(), "KSUS").text()).unwrap();
    let mut a = World::new(ksus(), config());
    let mut b = World::new(ksus(), code.config());
    for _ in 0..2400 {
        a.tick();
        b.tick();
    }
    assert_eq!(a.state_digest(), b.state_digest());
}

/// Quitting to the main menu ends the shift where it was, and the replay
/// stops there too.
#[test]
fn a_shift_left_early_rebuilds_to_where_it_was_left() {
    let mut world = World::new(ksus(), config());
    let mut recording = Recording::new(&world);
    for step in 0..2000 {
        player(&mut world, step);
        world.tick();
        recording.update(&world);
    }
    world.leave_shift();
    recording.finish(&world);
    assert_eq!(world.ended, Some(ShiftEnd::Left));
    // Nothing more happens once it's left.
    world.tick();
    assert_eq!(world.ticks(), 2000);

    let mut rebuilt = Recording::from_start(World::new(ksus(), config()), recording.commands.clone());
    let mut chunks = 0;
    while !rebuilt.rebuild_step(500) {
        chunks += 1;
        assert!(chunks < 1000, "the rebuild never finishes");
    }
    assert_eq!(rebuilt.last().ended, Some(ShiftEnd::Left));
    assert_eq!(rebuilt.last().summary(), world.summary());
    assert_eq!(rebuilt.last().state_digest(), world.state_digest());
}
