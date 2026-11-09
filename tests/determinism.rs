//! The sim gives the same shift, bit for bit, from the same settings and
//! commands: on a second run, from a replay, and on another machine.
//!
//! Replays, shift codes and co-op play all depend on it. The cross-machine
//! check is a pinned set of checksums in `tests/determinism.txt`:
//!
//!     cargo test --release --test determinism -- --ignored write_checksums
//!
//! writes them (on any machine, after a change to the sim), and
//!
//!     cargo test --release --test determinism -- --ignored matches_the_pinned_checksums
//!
//! checks another machine against them: run it on Windows and on macOS
//! before a release. A difference means some maths gives a different result
//! there (see `sim::math`).

use atcgame::sim::airspace::{Airport, KMEM_AIRPORT, KMEM_GROUND, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::emergency::EmergencyRate;
use atcgame::sim::mistakes::PilotMistakes;
use atcgame::sim::positions::Staffing;
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{World, WorldConfig};
use std::path::Path;

/// Checksums are taken this often, ticks: every two minutes of sim time.
const EVERY: u32 = 480;
const SHIFT_S: f32 = 20.0 * 60.0;

fn airports() -> Vec<(&'static str, Airport)> {
    vec![
        ("fictional", Airport::default_airport()),
        ("ksus", Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()),
        ("kmem", Airport::load(KMEM_AIRPORT, Some(KMEM_GROUND)).unwrap()),
    ]
}

/// Busy shifts with everything on: storms, emergencies and pilot mistakes.
fn shifts() -> Vec<(String, Airport, WorldConfig)> {
    let mut out = Vec::new();
    for (name, airport) in airports() {
        for (label, staffing) in [("approach", Staffing::APPROACH), ("local-ground", Staffing::LOCAL_GROUND), ("all", Staffing::ALL)] {
            let config = WorldConfig {
                seed: 7,
                staffing,
                vfr: true,
                emergencies: EmergencyRate::Frequent,
                mistakes: PilotMistakes::Frequent,
                weather: WeatherKind::Storms,
                shift_seconds: SHIFT_S,
                ..Default::default()
            };
            out.push((format!("{name} {label}"), airport.clone(), config));
        }
    }
    out
}

/// A player who keeps giving instructions, so the shift goes through
/// turns, descents, approaches, taxiing, takeoffs and go-arounds.
fn player(world: &mut World, step: u32) {
    if !step.is_multiple_of(40) {
        return;
    }
    let ends = world.airport.runways.len();
    let fixes = world.airport.fixes.len().max(1);
    let nodes = world.airport.surface.as_ref().map_or(1, |m| m.nodes.len());
    let ids: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
    for (n, id) in ids.into_iter().enumerate() {
        let k = (step / 40) as usize + n * 5 + id as usize;
        let end = k % ends;
        let clearance = match k % 14 {
            0 => Clearance::Heading(((k * 37) % 360) as f32),
            1 => Clearance::Altitude(((k % 9) as f32 + 2.0) * 1000.0),
            2 => Clearance::Speed(170.0 + (k % 6) as f32 * 10.0),
            3 => Clearance::DirectFix(k % fixes),
            4 => Clearance::ClearedIls(end),
            5 => Clearance::HandoffTower,
            6 => Clearance::Taxi { to: k % nodes, via: Vec::new() },
            7 => Clearance::LineUpAndWait(end),
            8 => Clearance::ClearedForTakeoff(end),
            9 => Clearance::ClearedToLand(end),
            10 => Clearance::ContactGround,
            11 => Clearance::ContactDeparture,
            12 => Clearance::GoAround,
            _ => Clearance::ResumeRoute,
        };
        world.transmit(id, vec![clearance]);
    }
}

/// A shift played to the end, with its checksums along the way.
fn play(airport: Airport, config: WorldConfig) -> (World, Vec<u64>) {
    let mut world = World::new(airport, config);
    let mut sums = Vec::new();
    let mut step = 0;
    while world.ended.is_none() {
        player(&mut world, step);
        world.tick();
        world.drain_events();
        step += 1;
        if world.ticks().is_multiple_of(EVERY) {
            sums.push(world.checksum());
        }
    }
    sums.push(world.checksum());
    (world, sums)
}

fn lines() -> Vec<String> {
    shifts()
        .into_iter()
        .map(|(name, airport, config)| {
            let (_, sums) = play(airport, config);
            let sums: Vec<String> = sums.iter().map(|s| format!("{s:016x}")).collect();
            format!("{name}: {}", sums.join(" "))
        })
        .collect()
}

fn pinned_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/determinism.txt")
}

#[test]
fn the_same_shift_twice_is_the_same() {
    let (_, airport, config) = shifts().remove(3);
    assert_eq!(play(airport.clone(), config).1, play(airport, config).1);
}

#[test]
fn a_replay_is_the_same_shift() {
    let (_, airport, config) = shifts().remove(8);
    let (played, _) = play(airport.clone(), config);
    // Rebuild it from its settings and commands alone.
    let mut again = World::new(airport, config);
    let commands = played.commands().to_vec();
    let mut next = 0;
    while again.ended.is_none() {
        while commands.get(next).is_some_and(|c| c.tick == again.ticks()) {
            again.apply(&commands[next]);
            next += 1;
        }
        again.tick();
        again.drain_events();
    }
    assert_eq!(again.checksum(), played.checksum());
}

#[test]
fn a_different_shift_has_a_different_checksum() {
    let (_, airport, config) = shifts().remove(0);
    let other = WorldConfig { seed: config.seed + 1, ..config };
    assert_ne!(play(airport.clone(), config).1, play(airport, other).1);
}

/// Pin this machine's checksums, after a change to the sim.
#[test]
#[ignore]
fn write_checksums() {
    let text = format!(
        "# Written by `cargo test --release --test determinism -- --ignored write_checksums`.\n\
         # Every machine must give these: see tests/determinism.rs.\n{}\n",
        lines().join("\n")
    );
    std::fs::write(pinned_path(), text).expect("write tests/determinism.txt");
}

/// This machine plays the pinned shifts exactly as the one that pinned them.
#[test]
#[ignore]
fn matches_the_pinned_checksums() {
    let pinned = std::fs::read_to_string(pinned_path()).expect("tests/determinism.txt: run write_checksums first");
    let pinned: Vec<&str> = pinned.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    let here = lines();
    assert_eq!(pinned.len(), here.len(), "a different set of shifts: write the checksums again");
    for (p, h) in pinned.iter().zip(&here) {
        assert_eq!(p, h, "this machine plays a shift differently");
    }
}
