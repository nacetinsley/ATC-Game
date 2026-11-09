//! The lockstep itself: the host's side and a client's.
//!
//! Neither draws anything or owns the world: the game passes its world in
//! each frame, and shows the notices that come back.

use super::link::{ClientEvent, ClientLink, HostEvent, HostLink, PeerId};
use super::protocol::{CATCHUP_CHUNK, Msg, PROTOCOL, game_version};
use crate::sim::airspace::Airport;
use crate::sim::positions::{MAX_SEATS, Seat, Staffing};
use crate::sim::replay::{Action, Command, ShiftCode};
use crate::sim::{TICK, World, WorldConfig};
use std::collections::{BTreeMap, VecDeque};

/// The host plays in seat 0.
pub const HOST: Seat = 0;
/// The host sends its checksum this often, ticks: every 15 seconds.
pub const CHECKSUM_TICKS: u32 = 60;
/// How long a dropped player's seat waits for them before it's automated.
pub const GRACE_S: f32 = 30.0;
/// Nothing heard for this long, and the other end is taken to be gone.
pub const SILENCE_S: f32 = 10.0;
/// A client says it's still there this often.
pub const ACK_S: f32 = 1.0;
/// The most real time one frame may move the host's clock: after a stall,
/// the shift doesn't race to catch up.
const MAX_FRAME_S: f32 = 0.5;

/// What a co-op shift is built from, the same on every machine.
#[derive(Debug, Clone, PartialEq)]
pub struct ShiftSetup {
    /// The shift code: airport, seed and settings. Its staffing isn't used.
    pub code: String,
    /// Who works what.
    pub staffing: Staffing,
    pub clock_start: f64,
}

impl ShiftSetup {
    pub fn new(config: &WorldConfig, airport: &str, staffing: Staffing, clock_start: f64) -> ShiftSetup {
        ShiftSetup { code: ShiftCode::of(config, airport).text(), staffing, clock_start }
    }

    /// The world's settings. The host builds its world from these too, so
    /// every machine's is the same.
    pub fn config(&self) -> Result<WorldConfig, String> {
        let code = ShiftCode::parse(&self.code).map_err(String::from)?;
        Ok(WorldConfig { staffing: self.staffing, clock_start: self.clock_start, ..code.config() })
    }

    /// The airport the shift is at, from those this build has.
    pub fn airport<'a>(&self, airports: &'a [Airport]) -> Result<&'a Airport, String> {
        let code = ShiftCode::parse(&self.code).map_err(String::from)?;
        airports.iter().find(|a| a.code == code.airport).ok_or_else(|| format!("no airport {}", code.airport))
    }

    /// The world at the start of the shift.
    pub fn world(&self, airports: &[Airport]) -> Result<World, String> {
        Ok(World::new(self.airport(airports)?.clone(), self.config()?))
    }
}

/// Something to tell the player.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// A player took a seat: new, or back after a drop.
    Joined { seat: Seat, name: String },
    /// A player's connection went: their seat waits `GRACE_S` for them.
    Dropped { seat: Seat, name: String },
    /// A player left, or never came back: their positions are automated.
    Vacated { seat: Seat },
    /// Someone couldn't join.
    Refused { name: String, reason: String },
    /// This machine's sim no longer matches the host's.
    Desync { tick: u32 },
    /// The host is gone, or ended the session.
    HostGone { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SeatState {
    /// Not a player's seat.
    Unused,
    /// Waiting for its player, since this time: at the start, or after a drop.
    Away { since: f32 },
    Playing { peer: PeerId },
    /// Its player is gone, and its positions are automated.
    Vacated,
}

struct Peer {
    name: String,
    seat: Option<Seat>,
    heard: f32,
}

/// A seat, as the host's screen shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SeatInfo {
    pub seat: Seat,
    pub name: Option<String>,
    pub status: SeatStatus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeatStatus {
    Host,
    Playing,
    /// Waiting for its player, with this many seconds of grace left.
    Waiting(f32),
    Automated,
}

/// The host: the clock, and the one place commands are put in order.
pub struct Host<L: HostLink> {
    link: L,
    setup: ShiftSetup,
    seats: [SeatState; MAX_SEATS],
    /// Who last sat in each seat, so a player who drops gets theirs back.
    names: [Option<String>; MAX_SEATS],
    peers: BTreeMap<PeerId, Peer>,
    /// Commands for the next tick: seat, aircraft, action.
    pending: Vec<(Seat, u32, Action)>,
    clock: f32,
    accumulator: f32,
    notices: Vec<Notice>,
}

impl<L: HostLink> Host<L> {
    /// Host a shift. The other players' seats wait for them from now.
    pub fn new(link: L, setup: ShiftSetup, name: &str) -> Host<L> {
        let mut seats = [SeatState::Unused; MAX_SEATS];
        for seat in setup.staffing.players().iter() {
            seats[seat as usize] = if seat == HOST { SeatState::Playing { peer: 0 } } else { SeatState::Away { since: 0.0 } };
        }
        let mut names: [Option<String>; MAX_SEATS] = Default::default();
        names[HOST as usize] = Some(name.to_string());
        Host {
            link,
            setup,
            seats,
            names,
            peers: BTreeMap::new(),
            pending: Vec::new(),
            clock: 0.0,
            accumulator: 0.0,
            notices: Vec::new(),
        }
    }

    pub fn setup(&self) -> &ShiftSetup {
        &self.setup
    }

    /// Something the host's own player said: it goes in the next tick, the
    /// same as anyone's.
    pub fn command(&mut self, id: u32, action: Action) {
        self.pending.push((HOST, id, action));
    }

    /// Every seat with positions, and who's in it.
    pub fn seats(&self) -> Vec<SeatInfo> {
        (0..MAX_SEATS)
            .filter_map(|i| {
                let status = match self.seats[i] {
                    SeatState::Unused => return None,
                    _ if i as Seat == HOST => SeatStatus::Host,
                    SeatState::Playing { .. } => SeatStatus::Playing,
                    SeatState::Away { since } => SeatStatus::Waiting((GRACE_S - (self.clock - since)).max(0.0)),
                    SeatState::Vacated => SeatStatus::Automated,
                };
                Some(SeatInfo { seat: i as Seat, name: self.names[i].clone(), status })
            })
            .collect()
    }

    /// Run the session for `dt` seconds of real time: hear from the
    /// players, and run the ticks that are due.
    pub fn update(&mut self, dt: f32, world: &mut World) -> Vec<Notice> {
        self.clock += dt;
        for event in self.link.poll() {
            self.handle(event, world);
        }
        self.check_peers();
        if world.ended.is_none() {
            self.accumulator += dt.min(MAX_FRAME_S);
            while self.accumulator >= TICK && world.ended.is_none() {
                self.accumulator -= TICK;
                self.run_tick(world);
            }
        }
        std::mem::take(&mut self.notices)
    }

    /// Tell everyone the session is over, and hang up.
    pub fn close(&mut self, reason: &str) {
        let peers: Vec<PeerId> = self.peers.keys().copied().collect();
        for peer in peers {
            self.link.send(peer, &Msg::Bye { reason: reason.into() });
            self.link.close(peer);
        }
        self.peers.clear();
    }

    fn handle(&mut self, event: HostEvent, world: &World) {
        match event {
            HostEvent::Joined(peer) => {
                self.peers.insert(peer, Peer { name: String::new(), seat: None, heard: self.clock });
            }
            HostEvent::Left(peer, _) => self.gone(peer, false),
            HostEvent::Message(peer, msg) => {
                let Some(p) = self.peers.get_mut(&peer) else { return };
                p.heard = self.clock;
                match msg {
                    Msg::Hello { protocol, game, name } => self.hello(peer, protocol, game, name, world),
                    Msg::Command { id, action } => {
                        let Some(seat) = p.seat else { return };
                        match action {
                            // Only the host says who's in which seat.
                            Action::Vacate(_) | Action::Take(_) => {}
                            Action::Leave => self.gone(peer, true),
                            action => self.pending.push((seat, id, action)),
                        }
                    }
                    Msg::Bye { .. } => self.gone(peer, true),
                    _ => {}
                }
            }
        }
    }

    fn refuse(&mut self, peer: PeerId, name: String, reason: &str) {
        self.link.send(peer, &Msg::Refused { reason: reason.into() });
        self.link.close(peer);
        self.peers.remove(&peer);
        self.notices.push(Notice::Refused { name, reason: reason.into() });
    }

    fn hello(&mut self, peer: PeerId, protocol: u32, game: String, name: String, world: &World) {
        if protocol != PROTOCOL || game != game_version() {
            let reason = format!("the host has version {}, and you have {game}", game_version());
            return self.refuse(peer, name, &reason);
        }
        if world.ended.is_some() {
            return self.refuse(peer, name, "the shift is over");
        }
        let free = |s: usize| !matches!(self.seats[s], SeatState::Unused | SeatState::Playing { .. });
        // Their own seat back, or the first free one.
        let seat = (0..MAX_SEATS)
            .find(|&s| free(s) && self.names[s].as_deref() == Some(name.as_str()))
            .or_else(|| (0..MAX_SEATS).find(|&s| free(s) && self.names[s].is_none()))
            .or_else(|| (0..MAX_SEATS).find(|&s| free(s)));
        let Some(seat) = seat else {
            return self.refuse(peer, name, "every seat is taken");
        };
        let seat_id = seat as Seat;
        let send = |link: &mut L, msg: Msg| link.send(peer, &msg);
        send(
            &mut self.link,
            Msg::Start {
                code: self.setup.code.clone(),
                staffing: self.setup.staffing,
                clock_start: self.setup.clock_start,
                seat: seat_id,
            },
        );
        for chunk in world.commands().chunks(CATCHUP_CHUNK) {
            send(&mut self.link, Msg::Catchup { commands: chunk.to_vec() });
        }
        send(&mut self.link, Msg::CaughtUp { tick: world.ticks() });
        if self.seats[seat] == SeatState::Vacated {
            self.pending.push((HOST, 0, Action::Take(seat_id)));
        }
        self.seats[seat] = SeatState::Playing { peer };
        self.names[seat] = Some(name.clone());
        if let Some(p) = self.peers.get_mut(&peer) {
            p.name = name.clone();
            p.seat = Some(seat_id);
        }
        self.notices.push(Notice::Joined { seat: seat_id, name });
    }

    /// A peer has gone: said goodbye (`left`), or dropped.
    fn gone(&mut self, peer: PeerId, left: bool) {
        let Some(p) = self.peers.remove(&peer) else { return };
        if left {
            self.link.close(peer);
        }
        let Some(seat) = p.seat else { return };
        if self.seats[seat as usize] != (SeatState::Playing { peer }) {
            return;
        }
        if left {
            self.vacate(seat);
        } else {
            self.seats[seat as usize] = SeatState::Away { since: self.clock };
            self.notices.push(Notice::Dropped { seat, name: p.name });
        }
    }

    fn vacate(&mut self, seat: Seat) {
        self.seats[seat as usize] = SeatState::Vacated;
        self.pending.push((HOST, 0, Action::Vacate(seat)));
        self.notices.push(Notice::Vacated { seat });
    }

    /// Hang up on peers gone quiet, and automate seats whose grace is up.
    fn check_peers(&mut self) {
        let quiet: Vec<PeerId> =
            self.peers.iter().filter(|(_, p)| self.clock - p.heard > SILENCE_S).map(|(&id, _)| id).collect();
        for peer in quiet {
            self.link.close(peer);
            self.gone(peer, false);
        }
        for seat in 0..MAX_SEATS {
            if let SeatState::Away { since } = self.seats[seat]
                && self.clock - since >= GRACE_S
            {
                self.vacate(seat as Seat);
            }
        }
    }

    fn players(&self) -> Vec<PeerId> {
        self.seats
            .iter()
            .filter_map(|s| match s {
                SeatState::Playing { peer } if *peer != 0 => Some(*peer),
                _ => None,
            })
            .collect()
    }

    fn run_tick(&mut self, world: &mut World) {
        let n = world.ticks();
        let commands: Vec<Command> = self
            .pending
            .drain(..)
            .map(|(seat, id, action)| Command { tick: n, id, action, seat })
            .collect();
        let players = self.players();
        let tick = Msg::Tick { n, commands };
        for &peer in &players {
            self.link.send(peer, &tick);
        }
        let Msg::Tick { commands, .. } = tick else { unreachable!() };
        apply_tick(world, &commands);
        if world.ticks().is_multiple_of(CHECKSUM_TICKS) {
            let msg = Msg::Checksum { tick: world.ticks(), sum: world.checksum() };
            for &peer in &players {
                self.link.send(peer, &msg);
            }
        }
    }
}

/// One tick, as every machine runs it.
fn apply_tick(world: &mut World, commands: &[Command]) {
    for command in commands {
        world.apply(command);
    }
    world.tick();
}

/// Where a client is in joining.
pub enum Joining {
    Waiting,
    /// The shift, caught up to now, and this player's seat.
    Ready(Box<World>, Seat),
    Failed(String),
}

enum ClientState {
    /// Said hello; waiting for the shift.
    Hello,
    /// Taking in the commands so far.
    Catching { setup: ShiftSetup, seat: Seat, commands: Vec<Command> },
    Playing,
    Over,
}

/// A player's side: follows the host's ticks.
pub struct Client<L: ClientLink> {
    link: L,
    state: ClientState,
    /// Messages that came after the catch-up, before play began.
    backlog: VecDeque<Msg>,
    clock: f32,
    heard: f32,
    acked: f32,
    seat: Seat,
    setup: Option<ShiftSetup>,
}

impl<L: ClientLink> Client<L> {
    /// Connect as `name`.
    pub fn new(mut link: L, name: &str) -> Client<L> {
        link.send(&Msg::Hello { protocol: PROTOCOL, game: game_version(), name: name.into() });
        Client {
            link,
            state: ClientState::Hello,
            backlog: VecDeque::new(),
            clock: 0.0,
            heard: 0.0,
            acked: 0.0,
            seat: 0,
            setup: None,
        }
    }

    pub fn seat(&self) -> Seat {
        self.seat
    }

    pub fn setup(&self) -> Option<&ShiftSetup> {
        self.setup.as_ref()
    }

    /// Wait for the shift, then build it and catch up.
    pub fn join(&mut self, dt: f32, airports: &[Airport]) -> Joining {
        self.clock += dt;
        if self.clock - self.heard > SILENCE_S {
            return self.fail("the host didn't answer".into());
        }
        for event in self.link.poll() {
            self.heard = self.clock;
            let msg = match event {
                ClientEvent::Message(msg) => msg,
                ClientEvent::Closed(why) => return self.fail(format!("the connection closed: {why}")),
            };
            match (&mut self.state, msg) {
                (ClientState::Hello, Msg::Start { code, staffing, clock_start, seat }) => {
                    let setup = ShiftSetup { code, staffing, clock_start };
                    self.state = ClientState::Catching { setup, seat, commands: Vec::new() };
                }
                (_, Msg::Refused { reason }) => return self.fail(reason),
                (_, Msg::Bye { reason }) => return self.fail(reason),
                (ClientState::Catching { commands, .. }, Msg::Catchup { commands: more }) => commands.extend(more),
                (ClientState::Catching { .. }, Msg::CaughtUp { tick }) => {
                    let ClientState::Catching { setup, seat, commands } =
                        std::mem::replace(&mut self.state, ClientState::Playing)
                    else {
                        unreachable!()
                    };
                    let mut world = match setup.world(airports) {
                        Ok(world) => world,
                        Err(why) => return self.fail(why),
                    };
                    world.run_to(tick, &commands);
                    if world.ticks() != tick {
                        return self.fail(format!("the shift ended before tick {tick}"));
                    }
                    self.seat = seat;
                    self.setup = Some(setup);
                    return Joining::Ready(Box::new(world), seat);
                }
                (ClientState::Playing, msg) => self.backlog.push_back(msg),
                _ => {}
            }
        }
        // Ticks that came in the same poll as the catch-up are kept for
        // `update`: the loop above returns as soon as the world is ready.
        Joining::Waiting
    }

    fn fail(&mut self, why: String) -> Joining {
        self.state = ClientState::Over;
        self.link.close();
        Joining::Failed(why)
    }

    /// Something this player said: it comes back in a tick.
    pub fn command(&mut self, id: u32, action: Action) {
        if matches!(self.state, ClientState::Playing) {
            self.link.send(&Msg::Command { id, action });
        }
    }

    /// Leave the session.
    pub fn leave(&mut self) {
        if !matches!(self.state, ClientState::Over) {
            self.link.send(&Msg::Bye { reason: "left".into() });
            self.link.close();
            self.state = ClientState::Over;
        }
    }

    /// Follow the host: run the ticks that have come in.
    pub fn update(&mut self, dt: f32, world: &mut World) -> Vec<Notice> {
        if !matches!(self.state, ClientState::Playing) {
            return Vec::new();
        }
        self.clock += dt;
        let mut notices = Vec::new();
        let mut msgs: VecDeque<Msg> = std::mem::take(&mut self.backlog);
        for event in self.link.poll() {
            self.heard = self.clock;
            match event {
                ClientEvent::Message(msg) => msgs.push_back(msg),
                ClientEvent::Closed(why) => {
                    msgs.push_back(Msg::Bye { reason: format!("the connection closed: {why}") });
                }
            }
        }
        for msg in msgs {
            match msg {
                Msg::Tick { n, commands } => {
                    if n != world.ticks() {
                        notices.push(Notice::Desync { tick: world.ticks() });
                        self.end();
                        break;
                    }
                    apply_tick(world, &commands);
                }
                Msg::Checksum { tick, sum } => {
                    if world.ticks() == tick && world.checksum() != sum {
                        notices.push(Notice::Desync { tick });
                        self.end();
                        break;
                    }
                }
                Msg::Bye { reason } => {
                    notices.push(Notice::HostGone { reason });
                    self.end();
                    break;
                }
                _ => {}
            }
        }
        if matches!(self.state, ClientState::Playing) {
            if self.clock - self.heard > SILENCE_S {
                notices.push(Notice::HostGone { reason: "the host stopped answering".into() });
                self.end();
            } else if self.clock - self.acked >= ACK_S {
                self.acked = self.clock;
                self.link.send(&Msg::Ack { tick: world.ticks() });
            }
        }
        notices
    }

    fn end(&mut self) {
        self.state = ClientState::Over;
        self.link.close();
    }

    /// Following the host still.
    pub fn playing(&self) -> bool {
        matches!(self.state, ClientState::Playing)
    }
}
