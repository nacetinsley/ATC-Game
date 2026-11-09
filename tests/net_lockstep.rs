//! Co-op over a connection: the host and its clients run the same shift in
//! lockstep, players join late, drop and come back, and a sim that goes out
//! of step is caught.

use atcgame::net::link::{ClientLink, LoopClient, LoopConnector, LoopHost};
use atcgame::net::protocol::{Msg, PROTOCOL};
use atcgame::net::session::{CHECKSUM_TICKS, Client, GRACE_S, Host, Joining, Notice, SILENCE_S, ShiftSetup};
use atcgame::net::tcp::{TcpClient, TcpHost};
use atcgame::sim::TICK;
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::positions::{Position, Seat, Staffing};
use atcgame::sim::replay::Action;
use atcgame::sim::traffic::Busyness;
use atcgame::sim::{World, WorldConfig};
use std::time::Duration;

const MINUTE: u32 = 240;

/// Approach in seat 0 (the host), Local in 1, Ground and Delivery in 2.
fn three_way() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, 0)
        .with_seat(Position::Local, 1)
        .with_seat(Position::Ground, 2)
        .with_seat(Position::Delivery, 2)
}

fn airports() -> Vec<Airport> {
    vec![Airport::default_airport()]
}

fn setup(staffing: Staffing) -> ShiftSetup {
    let config = WorldConfig { seed: 5, busyness: Busyness::VeryBusy, shift_seconds: 30.0 * 60.0, ..Default::default() };
    ShiftSetup::new(&config, &airports()[0].code, staffing, 45_000.0)
}

/// Instructions from a player in `seat` to their own traffic, now and then.
fn says(world: &World, seat: Seat, step: u32) -> Vec<(u32, Action)> {
    if !step.is_multiple_of(16) {
        return Vec::new();
    }
    let arrival = world.airport.runway_index(world.runway_ids().0).unwrap();
    world
        .aircraft
        .iter()
        .filter(|a| world.is_seats(seat, a) || (seat == 0 && a.phase == atcgame::sim::aircraft::Phase::Offered))
        .enumerate()
        .map(|(n, a)| {
            let k = step as usize / 16 + n + a.id as usize;
            let action = match k % 6 {
                0 => Action::AcceptHandoff,
                1 => Action::Transmit(vec![Clearance::Heading((k * 41 % 360) as f32)]),
                2 => Action::Transmit(vec![Clearance::ClearedIls(arrival)]),
                3 => Action::Transmit(vec![Clearance::HandoffTower]),
                4 => Action::Transmit(vec![Clearance::ClearedToLand(arrival)]),
                _ => Action::Transmit(vec![Clearance::Taxi { to: k % 40, via: Vec::new() }]),
            };
            (a.id, action)
        })
        .collect()
}

struct Player {
    client: Client<LoopClient>,
    world: World,
    notices: Vec<Notice>,
}

/// A host on the in-memory link, and its players.
struct Table {
    connector: LoopConnector,
    host: Host<LoopHost>,
    world: World,
    players: Vec<Player>,
    notices: Vec<Notice>,
    step: u32,
}

impl Table {
    fn new(staffing: Staffing) -> Table {
        let setup = setup(staffing);
        let world = setup.world(&airports()).unwrap();
        let link = LoopHost::default();
        let connector = link.connector();
        let host = Host::new(link, setup, "host");
        Table { connector, host, world, players: Vec::new(), notices: Vec::new(), step: 0 }
    }

    /// A new player connects, and joins.
    fn join(&mut self, name: &str) -> usize {
        let link = self.connector.connect();
        let mut client = Client::new(link, name);
        for _ in 0..20 {
            self.notices.extend(self.host.update(0.0, &mut self.world));
            match client.join(0.0, &airports()) {
                Joining::Waiting => {}
                Joining::Ready(world, _) => {
                    self.players.push(Player { client, world: *world, notices: Vec::new() });
                    return self.players.len() - 1;
                }
                Joining::Failed(why) => panic!("{name} couldn't join: {why}"),
            }
        }
        panic!("{name} never joined");
    }

    /// One tick of real time, with everyone saying things.
    fn step(&mut self, talk: bool) {
        if talk {
            for (id, action) in says(&self.world, 0, self.step) {
                self.host.command(id, action);
            }
            for p in &mut self.players {
                if p.client.playing() {
                    for (id, action) in says(&p.world, p.client.seat(), self.step) {
                        p.client.command(id, action);
                    }
                }
            }
        }
        self.notices.extend(self.host.update(TICK, &mut self.world));
        self.world.drain_events();
        for p in &mut self.players {
            p.notices.extend(p.client.update(TICK, &mut p.world));
            p.world.drain_events();
        }
        self.step += 1;
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step(true);
        }
    }

    /// Everyone still playing has the host's world.
    fn assert_in_step(&self) {
        for (i, p) in self.players.iter().enumerate() {
            if !p.client.playing() {
                continue;
            }
            assert!(!p.notices.iter().any(|n| matches!(n, Notice::Desync { .. })), "player {i}: {:?}", p.notices);
            assert_eq!(p.world.ticks(), self.world.ticks(), "player {i} ticks");
            assert_eq!(p.world.checksum(), self.world.checksum(), "player {i} checksum");
        }
    }
}

#[test]
fn three_players_stay_in_step() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.join("ground");
    assert_eq!(table.players[0].client.seat(), 1);
    assert_eq!(table.players[1].client.seat(), 2);
    table.run(12 * MINUTE);
    table.assert_in_step();
    // Commands made it in, under their own seats. (These players never get
    // an arrival to Local, so seat 1 has nothing it can say.)
    let seats: Vec<Seat> = table.world.commands().iter().map(|c| c.seat).collect();
    for seat in [0, 2] {
        assert!(seats.contains(&seat), "nothing from seat {seat}");
    }
    assert!(table.world.aircraft.len() > 3);
}

#[test]
fn a_player_can_join_late() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run(6 * MINUTE);
    table.join("ground");
    table.assert_in_step();
    table.run(3 * MINUTE);
    table.assert_in_step();
}

#[test]
fn a_seat_nobody_takes_is_automated_after_the_grace() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run((GRACE_S / TICK) as u32 + 4);
    assert!(table.notices.contains(&Notice::Vacated { seat: 2 }), "{:?}", table.notices);
    assert!(!table.world.staffing().ground && !table.world.staffing().delivery);
    assert!(table.players[0].world.staffing() == table.world.staffing());
    table.assert_in_step();

    // They turn up after all, and take their seat.
    let late = table.join("ground");
    assert_eq!(table.players[late].client.seat(), 2);
    table.run(4);
    assert!(table.world.staffing().seat_works(2, Position::Ground));
    table.assert_in_step();
}

#[test]
fn a_dropped_player_gets_their_seat_back() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.join("ground");
    table.run(2 * MINUTE);
    // Ground's connection goes.
    let gone = table.players.remove(1);
    drop(gone);
    table.run(8);
    assert!(table.notices.iter().any(|n| matches!(n, Notice::Dropped { seat: 2, .. })), "{:?}", table.notices);
    assert!(table.world.staffing().ground, "still in the grace period");
    table.run((GRACE_S / TICK) as u32);
    assert!(!table.world.staffing().ground, "automated after the grace");

    let back = table.join("ground");
    assert_eq!(table.players[back].client.seat(), 2);
    table.run(4);
    assert!(table.world.staffing().seat_works(2, Position::Ground));
    table.run(MINUTE);
    table.assert_in_step();
}

#[test]
fn a_player_who_leaves_is_automated_at_once() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run(MINUTE);
    table.players[0].client.leave();
    table.run(2);
    assert!(table.notices.contains(&Notice::Vacated { seat: 1 }));
    assert!(!table.world.staffing().local);
    assert!(table.world.ended.is_none(), "the shift goes on");
}

#[test]
fn a_silent_player_is_dropped() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run(MINUTE);
    // Their machine stops answering: still connected, but nothing comes.
    let frozen = table.players.remove(0);
    for _ in 0..((SILENCE_S / TICK) as u32 + 4) {
        table.step(true);
    }
    assert!(table.notices.iter().any(|n| matches!(n, Notice::Dropped { seat: 1, .. })), "{:?}", table.notices);
    drop(frozen);
}

#[test]
fn a_sim_out_of_step_is_caught() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run(2 * MINUTE);
    // Something on the player's machine comes out differently.
    let ac = table.players[0].world.aircraft.first_mut().expect("traffic");
    ac.altitude += 100.0;
    table.run(CHECKSUM_TICKS + 1);
    let notices = &table.players[0].notices;
    assert!(notices.iter().any(|n| matches!(n, Notice::Desync { .. })), "{notices:?}");
    assert!(!table.players[0].client.playing());
}

#[test]
fn the_host_ending_the_shift_ends_it_for_everyone() {
    let mut table = Table::new(three_way());
    table.join("local");
    table.run(MINUTE);
    table.host.command(0, Action::Leave);
    table.run(2);
    assert!(table.world.ended.is_some());
    assert!(table.players[0].world.ended.is_some());
    table.host.close("the host left");
    table.run(1);
    assert!(table.players[0].notices.iter().any(|n| matches!(n, Notice::HostGone { .. })));
}

#[test]
fn another_build_is_refused() {
    let mut table = Table::new(three_way());
    let mut link = table.connector.connect();
    link.send(&Msg::Hello { protocol: PROTOCOL, game: "0.0.0-other".into(), name: "old".into() });
    table.run(1);
    assert!(table.notices.iter().any(|n| matches!(n, Notice::Refused { .. })));
    let got = link.poll();
    assert!(got.iter().any(|e| matches!(e, atcgame::net::link::ClientEvent::Message(Msg::Refused { .. }))), "{got:?}");
}

#[test]
fn a_full_shift_turns_players_away() {
    let mut table = Table::new(Staffing::NONE.with_seat(Position::Approach, 0).with_seat(Position::Local, 1));
    table.join("local");
    let link = table.connector.connect();
    let mut extra = Client::new(link, "extra");
    let mut result = Joining::Waiting;
    for _ in 0..5 {
        table.step(false);
        result = extra.join(0.0, &airports());
        if !matches!(result, Joining::Waiting) {
            break;
        }
    }
    assert!(matches!(result, Joining::Failed(ref why) if why.contains("taken")));
}

#[test]
fn lockstep_over_tcp() {
    // Approach on the host; the client has the tower cab.
    let split = Staffing::NONE
        .with_seat(Position::Approach, 0)
        .with_seat(Position::Local, 1)
        .with_seat(Position::Ground, 1)
        .with_seat(Position::Delivery, 1);
    let setup = setup(split);
    let mut world = setup.world(&airports()).unwrap();
    let link = TcpHost::listen("127.0.0.1:0").unwrap();
    let addr = link.local_addr().unwrap().to_string();
    let mut host = Host::new(link, setup, "host");
    let mut client = Client::new(TcpClient::connect(&addr, Duration::from_secs(5)).unwrap(), "local");

    let mut joined = None;
    for _ in 0..500 {
        host.update(0.0, &mut world);
        match client.join(0.0, &airports()) {
            Joining::Waiting => std::thread::sleep(Duration::from_millis(2)),
            Joining::Ready(w, seat) => {
                joined = Some((*w, seat));
                break;
            }
            Joining::Failed(why) => panic!("{why}"),
        }
    }
    let (mut theirs, seat) = joined.expect("joined over TCP");
    assert_eq!(seat, 1);

    for step in 0..(3 * MINUTE) {
        for (id, action) in says(&world, 0, step) {
            host.command(id, action);
        }
        for (id, action) in says(&theirs, seat, step) {
            client.command(id, action);
        }
        host.update(TICK, &mut world);
        world.drain_events();
        // Let the socket deliver this tick before going on. The client's
        // clock moves a tick too, so it keeps telling the host it's there.
        for attempt in 0..200 {
            let dt = if attempt == 0 { TICK } else { 0.0 };
            let notices = client.update(dt, &mut theirs);
            assert!(notices.is_empty(), "{notices:?}");
            if theirs.ticks() == world.ticks() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        theirs.drain_events();
    }
    assert_eq!(theirs.ticks(), world.ticks());
    assert_eq!(theirs.checksum(), world.checksum());
    assert!(world.commands().iter().any(|c| c.seat == 1), "the client's commands reached the host");
}
