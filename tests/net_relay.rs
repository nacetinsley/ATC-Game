//! Co-op through a real relay on a bad network: every connection goes through
//! a proxy that holds data back by a varying delay, and can cut it. Players
//! stay in step, drop, get back by themselves or come back to their own
//! seats, and hear when the host goes. The players are the game's own
//! (`ui::coop::Player`), which redials when its connection goes.

use atcgame::net::relay::{RelayClient, RelayHost};
use atcgame::net::session::{Client, GRACE_S, Host, Joining, Notice, ShiftSetup};
use atcgame::ui::coop::{self, AnyLink, Dial};
use atcgame::sim::airspace::Airport;
use atcgame::sim::clearance::Clearance;
use atcgame::sim::positions::{Position, Seat, Staffing};
use atcgame::sim::replay::Action;
use atcgame::sim::traffic::Busyness;
use atcgame::sim::{TICK, World, WorldConfig};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Sim seconds to a wall-clock second, so a test takes seconds, not minutes.
/// The network's delay counts that much more against the sim.
const SPEED: f32 = 12.0;
const MINUTE: u32 = 240;

/// A TCP proxy in front of the relay. Each chunk is held back by `delay`
/// plus up to `jitter`, in order, as a real connection would be.
struct Lag {
    addr: String,
    live: Arc<Mutex<Vec<TcpStream>>>,
    /// Turns new connections away, as if the machine were gone.
    down: Arc<AtomicBool>,
}

impl Lag {
    fn new(to: &str, delay: Duration, jitter: Duration) -> Lag {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let live: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let to = to.to_string();
        let keep = live.clone();
        let down: Arc<AtomicBool> = Arc::default();
        let refuse = down.clone();
        thread::spawn(move || {
            for (n, near) in listener.incoming().enumerate() {
                let Ok(near) = near else { return };
                if refuse.load(Ordering::Relaxed) {
                    continue;
                }
                let Ok(far) = TcpStream::connect(&to) else { return };
                let _ = near.set_nodelay(true);
                let _ = far.set_nodelay(true);
                keep.lock().unwrap().extend([near.try_clone().unwrap(), far.try_clone().unwrap()]);
                let seed = n as u64 * 2 + 1;
                pipe(near.try_clone().unwrap(), far.try_clone().unwrap(), delay, jitter, seed);
                pipe(far, near, delay, jitter, seed + 1);
            }
        });
        Lag { addr, live, down }
    }

    /// The connection goes, without a goodbye.
    fn cut(&self) {
        for stream in self.live.lock().unwrap().drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// The connection goes, and no new one gets through.
    fn down(&self) {
        self.down.store(true, Ordering::Relaxed);
        self.cut();
    }
}

impl Drop for Lag {
    fn drop(&mut self) {
        self.cut();
    }
}

/// Copy `from` to `to`, each chunk late.
fn pipe(mut from: TcpStream, mut to: TcpStream, delay: Duration, jitter: Duration, seed: u64) {
    let (send, recv) = mpsc::channel::<(Instant, Vec<u8>)>();
    thread::spawn(move || {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut last = Instant::now();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let extra = jitter.mul_f64((state % 1000) as f64 / 1000.0);
                    // Never ahead of what went before: TCP keeps order.
                    last = last.max(Instant::now() + delay + extra);
                    if send.send((last, buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    thread::spawn(move || {
        for (due, bytes) in recv {
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
            if to.write_all(&bytes).is_err() {
                break;
            }
        }
        let _ = to.shutdown(Shutdown::Write);
    });
}

fn airports() -> Vec<Airport> {
    vec![Airport::default_airport()]
}

/// A relay that lets the sped-up shift through: at `SPEED` the host sends
/// more frames a second than a real one would.
fn relay() -> String {
    let limits = atc_relay::server::Limits { frames_per_s: 5000.0, burst_frames: 10_000.0, ..Default::default() };
    atc_relay::server::spawn("127.0.0.1:0", limits).unwrap().to_string()
}

fn setup(staffing: Staffing) -> ShiftSetup {
    let config = WorldConfig { seed: 9, busyness: Busyness::VeryBusy, shift_seconds: 30.0 * 60.0, ..Default::default() };
    ShiftSetup::new(&config, &airports()[0].code, staffing, 45_000.0)
}

/// Instructions from a player in `seat` to their own traffic, now and then.
fn says(world: &World, seat: Seat, step: u32) -> Vec<(u32, Action)> {
    if !step.is_multiple_of(12) {
        return Vec::new();
    }
    let arrival = world.airport.runway_index(world.runway_ids().0).unwrap();
    world
        .aircraft
        .iter()
        .filter(|a| world.is_seats(seat, a) || (seat == 0 && a.phase == atcgame::sim::aircraft::Phase::Offered))
        .enumerate()
        .map(|(n, a)| {
            let k = step as usize / 12 + n + a.id as usize;
            let action = match k % 6 {
                0 => Action::AcceptHandoff,
                1 => Action::Transmit(vec![Clearance::Heading((k * 37 % 360) as f32)]),
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
    name: String,
    player: coop::Player,
    world: World,
    notices: Vec<Notice>,
    // Last, so the connection goes after the client.
    lag: Lag,
}

/// A host on the relay, behind its own bad connection, and its players.
struct Table {
    relay: String,
    code: String,
    host: Host<RelayHost>,
    world: World,
    notices: Vec<Notice>,
    players: Vec<Player>,
    delay: Duration,
    jitter: Duration,
    clock: Instant,
    host_lag: Lag,
}

impl Table {
    fn new(staffing: Staffing, delay: Duration, jitter: Duration) -> Table {
        let relay = relay();
        let host_lag = Lag::new(&relay, delay, jitter);
        let link = RelayHost::open(&host_lag.addr, Duration::from_secs(5)).unwrap();
        let code = link.code().to_string();
        let setup = setup(staffing);
        let world = setup.world(&airports()).unwrap();
        let host = Host::new(link, setup, "host");
        Table { relay, code, host, world, notices: Vec::new(), players: Vec::new(), delay, jitter, clock: Instant::now(), host_lag }
    }

    /// Sim time since the last call. A busy test machine can stall for a
    /// second, which at `SPEED` would be long enough for the host to drop a
    /// player as silent: a stall only slows the shift.
    fn dt(&mut self) -> f32 {
        let now = Instant::now();
        let dt = (now - self.clock).as_secs_f32() * SPEED;
        self.clock = now;
        dt.min(0.5)
    }

    /// Everyone's machine, once: the host runs as much of the shift as has
    /// passed, and the players follow what has reached them.
    fn frame(&mut self, talk: bool) {
        let dt = self.dt();
        let before = self.world.ticks();
        self.notices.extend(self.host.update(dt, &mut self.world));
        self.world.drain_events();
        let talk = talk && self.world.ticks() != before;
        if talk {
            for (id, action) in says(&self.world, 0, self.world.ticks()) {
                self.host.command(id, action);
            }
        }
        for p in &mut self.players {
            let ticks = p.world.ticks();
            p.notices.extend(p.player.update(dt, &mut p.world, &airports()));
            p.world.drain_events();
            if talk && p.player.client().playing() && p.world.ticks() != ticks {
                for (id, action) in says(&p.world, p.player.client().seat(), p.world.ticks()) {
                    p.player.command(id, action);
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }

    /// The shift runs on for `ticks`.
    fn run(&mut self, ticks: u32) {
        let end = self.world.ticks() + ticks;
        let deadline = Instant::now() + Duration::from_secs_f32(ticks as f32 * TICK / SPEED * 3.0 + 10.0);
        while self.world.ticks() < end {
            assert!(Instant::now() < deadline, "the shift fell behind");
            self.frame(true);
        }
    }

    /// A player connects through their own bad connection, and joins.
    fn join(&mut self, name: &str) -> Seat {
        let lag = Lag::new(&self.relay, self.delay, self.jitter);
        // As the game dials: the same room, through this player's network.
        let (addr, code) = (lag.addr.clone(), self.code.clone());
        let dial: Dial =
            Arc::new(move || Ok(Box::new(RelayClient::join(&addr, &code, Duration::from_secs(5))?) as AnyLink));
        let mut client = Client::new(dial().unwrap(), name);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "{name} never joined");
            self.frame(true);
            match client.join(0.0, &airports()) {
                Joining::Waiting => {}
                Joining::Ready(world, seat) => {
                    let player = coop::Player::new(client, dial);
                    self.players.push(Player { name: name.into(), player, world: *world, notices: Vec::new(), lag });
                    return seat;
                }
                Joining::Failed(why) => panic!("{name} couldn't join: {why}"),
            }
        }
    }

    fn player(&mut self, name: &str) -> usize {
        self.players.iter().position(|p| p.name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    /// Hold the host's clock, let everyone catch up, and check they all
    /// have the host's world.
    fn assert_in_step(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.dt();
            self.notices.extend(self.host.update(0.0, &mut self.world));
            for p in &mut self.players {
                p.notices.extend(p.player.update(0.0, &mut p.world, &airports()));
            }
            let ticks = self.world.ticks();
            if self.players.iter().all(|p| p.player.client().playing() && p.world.ticks() == ticks) {
                break;
            }
            assert!(Instant::now() < deadline, "the players never caught up: {:?}", self.notices);
            thread::sleep(Duration::from_millis(2));
        }
        for p in &self.players {
            assert!(p.player.client().playing(), "{} isn't playing: {:?}", p.name, p.notices);
            assert!(!p.notices.iter().any(|n| matches!(n, Notice::Desync { .. })), "{}: {:?}", p.name, p.notices);
            assert_eq!(p.world.checksum(), self.world.checksum(), "{}'s world", p.name);
        }
        self.dt();
    }

    /// Run until the host hears something.
    fn until(&mut self, what: &str, max_ticks: u32, heard: impl Fn(&Notice) -> bool) {
        let end = self.world.ticks() + max_ticks;
        while !self.notices.iter().any(&heard) {
            assert!(self.world.ticks() < end, "never heard {what}: {:?}", self.notices);
            self.frame(true);
        }
    }
}

/// One player on each position.
fn four_way() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, 0)
        .with_seat(Position::Local, 1)
        .with_seat(Position::Ground, 2)
        .with_seat(Position::Delivery, 3)
}

/// Approach on the host, Local, and Ground with Delivery.
fn three_way() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, 0)
        .with_seat(Position::Local, 1)
        .with_seat(Position::Ground, 2)
        .with_seat(Position::Delivery, 2)
}

#[test]
fn four_players_stay_in_step_on_a_bad_network() {
    let mut table = Table::new(four_way(), Duration::from_millis(30), Duration::from_millis(40));
    assert_eq!(table.join("local"), 1);
    assert_eq!(table.join("ground"), 2);
    assert_eq!(table.join("delivery"), 3);
    table.run(4 * MINUTE);
    table.assert_in_step();
    let seats: Vec<Seat> = table.world.commands().iter().map(|c| c.seat).collect();
    // (Not every seat has something it can say yet.)
    assert!(seats.contains(&0), "nothing from the host");
    assert!(seats.iter().any(|&s| s != 0), "the players' commands came through the relay");
    assert!(table.world.staffing() == four_way(), "nobody dropped");
}

#[test]
fn dropped_players_come_back_to_their_own_seats() {
    let mut table = Table::new(three_way(), Duration::from_millis(20), Duration::from_millis(30));
    table.join("local");
    table.join("ground");
    table.run(MINUTE);
    table.assert_in_step();

    // Both machines go, and the players come back the other way round.
    for name in ["local", "ground"] {
        let i = table.player(name);
        table.players[i].lag.down();
    }
    table.until("both drop", MINUTE, |n| matches!(n, Notice::Dropped { seat: 2, .. }));
    table.until("both drop", MINUTE, |n| matches!(n, Notice::Dropped { seat: 1, .. }));
    table.players.clear();
    assert!(table.world.staffing() == three_way(), "still in the grace period");
    assert_eq!(table.join("ground"), 2);
    assert_eq!(table.join("local"), 1);
    table.run(MINUTE);
    table.assert_in_step();
    assert!(table.world.staffing() == three_way());

    // Ground goes for longer than the grace: automated, then theirs again.
    let i = table.player("ground");
    table.players[i].lag.down();
    table.until("Ground automated", (GRACE_S / TICK) as u32 + MINUTE, |n| *n == Notice::Vacated { seat: 2 });
    // The vacate goes in with the next tick.
    table.run(2);
    assert!(!table.world.staffing().ground && !table.world.staffing().delivery);
    table.players.remove(i);
    table.assert_in_step();
    assert_eq!(table.join("ground"), 2);
    table.run(MINUTE);
    table.assert_in_step();
    assert!(table.world.staffing() == three_way(), "Ground and Delivery taken back");
}

#[test]
fn a_player_whose_connection_drops_gets_back_by_themselves() {
    let mut table = Table::new(three_way(), Duration::from_millis(20), Duration::from_millis(30));
    table.join("local");
    table.join("ground");
    table.run(MINUTE);
    table.assert_in_step();
    // Ground's connection drops; their game redials the room.
    let i = table.player("ground");
    table.players[i].lag.cut();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !table.players[i].notices.contains(&Notice::CaughtUp) {
        assert!(Instant::now() < deadline, "never got back: {:?}", table.players[i].notices);
        table.frame(true);
    }
    assert!(table.players[i].notices.iter().any(|n| matches!(n, Notice::LinkLost { .. })));
    assert_eq!(table.players[i].player.client().seat(), 2);
    table.players[i].notices.clear();
    table.run(MINUTE);
    table.assert_in_step();
    assert!(table.world.staffing() == three_way(), "back inside the grace: never automated");

    // Again, for longer than the grace: their positions are automated, and
    // they take them back when they get through.
    table.players[i].lag.down();
    table.until("Ground automated", (GRACE_S / TICK) as u32 + MINUTE, |n| *n == Notice::Vacated { seat: 2 });
    table.players[i].lag.down.store(false, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !table.players[i].notices.contains(&Notice::CaughtUp) {
        assert!(Instant::now() < deadline, "never got back: {:?}", table.players[i].notices);
        table.frame(true);
    }
    table.players[i].notices.clear();
    table.run(MINUTE);
    table.assert_in_step();
    assert!(table.world.staffing() == three_way(), "Ground and Delivery taken back");
}

#[test]
fn a_player_joining_late_catches_up_over_a_slow_link() {
    let mut table = Table::new(three_way(), Duration::from_millis(60), Duration::from_millis(60));
    table.join("local");
    table.run(5 * MINUTE);
    // A catch-up with every command so far, through the delay.
    assert_eq!(table.join("ground"), 2);
    table.run(MINUTE);
    table.assert_in_step();
}

#[test]
fn the_host_vanishing_ends_the_shift_for_everyone() {
    let mut table = Table::new(three_way(), Duration::from_millis(20), Duration::from_millis(20));
    table.join("local");
    table.join("ground");
    table.run(MINUTE);
    table.assert_in_step();
    // The host's connection goes, without a goodbye. The players try to
    // get back, and the relay says the room has gone.
    table.host_lag.cut();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !table.players.iter().all(|p| p.notices.iter().any(|n| matches!(n, Notice::HostGone { .. }))) {
        assert!(Instant::now() < deadline, "the players never heard: {:?}", table.players[0].notices);
        table.frame(false);
    }
    for p in &table.players {
        assert!(p.notices.iter().any(|n| matches!(n, Notice::HostGone { .. })), "{}: {:?}", p.name, p.notices);
    }
    assert!(table.notices.iter().any(|n| matches!(n, Notice::LinkLost { .. })), "{:?}", table.notices);
}
