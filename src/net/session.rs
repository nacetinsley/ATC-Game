//! The lockstep itself: the host's side and a client's, and the lobby
//! before the shift.
//!
//! Neither draws anything or owns the world: the game passes its world in
//! each frame, and shows the notices that come back.

use super::link::{ClientEvent, ClientLink, HostEvent, HostLink, PeerId};
use super::protocol::{CATCHUP_CHUNK, Msg, PROTOCOL, Positions, game_version};
use crate::sim::airspace::Airport;
use crate::sim::positions::{MAX_SEATS, Position, Seat, Staffing};
use crate::sim::replay::{Action, Command, ShiftCode};
use crate::sim::{TICK, World, WorldConfig};
use serde::{Deserialize, Serialize};
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
/// The host sends the lobby again this often, so quiet lobbies stay open.
pub const LOBBY_S: f32 = 2.0;
/// The most real time one frame may move the host's clock: after a stall,
/// the shift doesn't race to catch up.
const MAX_FRAME_S: f32 = 0.5;

/// The positions a lobby hands out, in the order `Positions` holds them.
pub const POSITIONS: [Position; MAX_SEATS] = [Position::Approach, Position::Local, Position::Ground, Position::Delivery];

/// Who works what, from the lobby's choices. Positions nobody took are
/// automated.
pub fn staffing_of(positions: &Positions) -> Staffing {
    POSITIONS
        .iter()
        .zip(positions)
        .fold(Staffing::NONE, |s, (&p, seat)| match seat {
            Some(seat) => s.with_seat(p, *seat),
            None => s,
        })
}

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Notice {
    /// A player took a seat: new, or back after a drop.
    Joined { seat: Seat, name: String },
    /// A player left the lobby.
    Left { seat: Seat, name: String },
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
    /// The host's link failed: the relay went, and the players with it.
    LinkLost { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SeatState {
    /// Nobody's.
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

enum Phase {
    /// Before the shift: players come and go and pick positions.
    Lobby { positions: Positions, about: String, since_sent: f32 },
    Playing { setup: ShiftSetup },
}

/// The host: the clock, and the one place commands are put in order.
pub struct Host<L: HostLink> {
    link: L,
    phase: Phase,
    seats: [SeatState; MAX_SEATS],
    /// Who last sat in each seat, so a player who drops gets theirs back.
    names: [Option<String>; MAX_SEATS],
    peers: BTreeMap<PeerId, Peer>,
    /// Commands for the next tick: seat, aircraft, action.
    pending: Vec<(Seat, u32, Action)>,
    clock: f32,
    accumulator: f32,
    notices: Vec<Notice>,
    link_failed: bool,
}

impl<L: HostLink> Host<L> {
    fn with(link: L, phase: Phase, name: &str) -> Host<L> {
        let mut seats = [SeatState::Unused; MAX_SEATS];
        seats[HOST as usize] = SeatState::Playing { peer: 0 };
        let mut names: [Option<String>; MAX_SEATS] = Default::default();
        names[HOST as usize] = Some(name.to_string());
        Host {
            link,
            phase,
            seats,
            names,
            peers: BTreeMap::new(),
            pending: Vec::new(),
            clock: 0.0,
            accumulator: 0.0,
            notices: Vec::new(),
            link_failed: false,
        }
    }

    /// Open a lobby. `about` says what the shift will be.
    pub fn lobby(link: L, name: &str, about: &str) -> Host<L> {
        let phase = Phase::Lobby { positions: [None; MAX_SEATS], about: about.into(), since_sent: 0.0 };
        Host::with(link, phase, name)
    }

    /// Host a shift straight away. The other players' seats wait for them
    /// from now.
    pub fn new(link: L, setup: ShiftSetup, name: &str) -> Host<L> {
        let staffing = setup.staffing;
        let mut host = Host::with(link, Phase::Playing { setup }, name);
        for seat in staffing.players().iter().filter(|&s| s != HOST) {
            host.seats[seat as usize] = SeatState::Away { since: 0.0 };
        }
        host
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    /// The shift, once it has started.
    pub fn setup(&self) -> Option<&ShiftSetup> {
        match &self.phase {
            Phase::Playing { setup } => Some(setup),
            Phase::Lobby { .. } => None,
        }
    }

    pub fn in_lobby(&self) -> bool {
        matches!(self.phase, Phase::Lobby { .. })
    }

    /// Everyone here, host first.
    pub fn players(&self) -> Vec<(Seat, String)> {
        (0..MAX_SEATS)
            .filter(|&s| matches!(self.seats[s], SeatState::Playing { .. }))
            .filter_map(|s| Some((s as Seat, self.names[s].clone()?)))
            .collect()
    }

    /// Who has each position in the lobby.
    pub fn positions(&self) -> Positions {
        match &self.phase {
            Phase::Lobby { positions, .. } => *positions,
            Phase::Playing { setup } => POSITIONS.map(|p| setup.staffing.seat(p)),
        }
    }

    /// In the lobby: give a position to a player, or to nobody.
    pub fn give(&mut self, position: usize, seat: Option<Seat>) {
        let here = seat.is_none_or(|s| matches!(self.seats.get(s as usize), Some(SeatState::Playing { .. })));
        if let Phase::Lobby { positions, .. } = &mut self.phase
            && position < MAX_SEATS
            && here
        {
            positions[position] = seat;
            self.send_lobby();
        }
    }

    /// Whether the shift can start: everyone needs a position.
    pub fn can_start(&self) -> Result<(), String> {
        let positions = self.positions();
        for (seat, name) in self.players() {
            if !positions.contains(&Some(seat)) {
                return Err(format!("{name} has no position yet"));
            }
        }
        Ok(())
    }

    /// Start the shift from the lobby: `world` is built from `setup`, whose
    /// staffing is the lobby's.
    pub fn start(&mut self, setup: ShiftSetup, world: &World) {
        let players = self.player_peers();
        self.phase = Phase::Playing { setup };
        for (seat, peer) in players {
            self.send_start(peer, seat, world);
        }
        self.send_roster();
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

    /// Run the lobby for `dt` seconds: players come, go and pick positions.
    pub fn lobby_update(&mut self, dt: f32) -> Vec<Notice> {
        self.clock += dt;
        for event in self.link.poll() {
            self.handle(event, None);
        }
        self.check_link();
        self.check_peers();
        if let Phase::Lobby { since_sent, .. } = &mut self.phase {
            *since_sent += dt;
            if *since_sent >= LOBBY_S {
                self.send_lobby();
            }
        }
        std::mem::take(&mut self.notices)
    }

    /// Run the shift for `dt` seconds of real time: hear from the players,
    /// and run the ticks that are due.
    pub fn update(&mut self, dt: f32, world: &mut World) -> Vec<Notice> {
        self.clock += dt;
        for event in self.link.poll() {
            self.handle(event, Some(world));
        }
        self.check_link();
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

    fn check_link(&mut self) {
        if !self.link_failed
            && let Some(reason) = self.link.failure()
        {
            self.link_failed = true;
            self.notices.push(Notice::LinkLost { reason });
        }
    }

    fn handle(&mut self, event: HostEvent, world: Option<&World>) {
        match event {
            HostEvent::Joined(peer) => {
                self.peers.insert(peer, Peer { name: String::new(), seat: None, heard: self.clock });
            }
            HostEvent::Left(peer, _) => self.gone(peer, false),
            HostEvent::Message(peer, msg) => {
                let Some(p) = self.peers.get_mut(&peer) else { return };
                p.heard = self.clock;
                let seat = p.seat;
                match msg {
                    Msg::Hello { protocol, game, name } if seat.is_none() => {
                        self.hello(peer, protocol, game, name, world)
                    }
                    Msg::Command { id, action } => {
                        let Some(seat) = seat else { return };
                        if self.in_lobby() {
                            return;
                        }
                        match action {
                            // Only the host says who's in which seat.
                            Action::Vacate(_) | Action::Take(_) => {}
                            Action::Leave => self.gone(peer, true),
                            action => self.pending.push((seat, id, action)),
                        }
                    }
                    Msg::Pick { position, take } => {
                        let Some(seat) = seat else { return };
                        if let Phase::Lobby { positions, .. } = &mut self.phase
                            && let Some(slot) = positions.get_mut(position as usize)
                        {
                            match (take, *slot) {
                                (true, None) => *slot = Some(seat),
                                (false, Some(s)) if s == seat => *slot = None,
                                _ => {}
                            }
                            self.send_lobby();
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

    fn hello(&mut self, peer: PeerId, protocol: u32, game: String, name: String, world: Option<&World>) {
        if protocol != PROTOCOL || game != game_version() {
            let reason = format!("the host has version {}, and you have {game}", game_version());
            return self.refuse(peer, name, &reason);
        }
        let name = unique_name(name.trim(), &self.players());
        let seat = match (&self.phase, world) {
            (Phase::Lobby { .. }, _) => {
                let seat = (0..MAX_SEATS).find(|&s| self.seats[s] == SeatState::Unused);
                let Some(seat) = seat else { return self.refuse(peer, name, "the room is full") };
                self.seats[seat] = SeatState::Playing { peer };
                self.names[seat] = Some(name.clone());
                seat as Seat
            }
            (Phase::Playing { .. }, Some(world)) => {
                if world.ended.is_some() {
                    return self.refuse(peer, name, "the shift is over");
                }
                let free = |s: usize| matches!(self.seats[s], SeatState::Away { .. } | SeatState::Vacated);
                // Their own seat back, or the first free one.
                let seat = (0..MAX_SEATS)
                    .find(|&s| free(s) && self.names[s].as_deref() == Some(name.as_str()))
                    .or_else(|| (0..MAX_SEATS).find(|&s| free(s) && self.names[s].is_none()))
                    .or_else(|| (0..MAX_SEATS).find(|&s| free(s)));
                let Some(seat) = seat else { return self.refuse(peer, name, "every seat is taken") };
                if self.seats[seat] == SeatState::Vacated {
                    self.pending.push((HOST, 0, Action::Take(seat as Seat)));
                }
                self.seats[seat] = SeatState::Playing { peer };
                self.names[seat] = Some(name.clone());
                self.send_start(peer, seat as Seat, world);
                seat as Seat
            }
            (Phase::Playing { .. }, None) => return,
        };
        if let Some(p) = self.peers.get_mut(&peer) {
            p.name = name.clone();
            p.seat = Some(seat);
        }
        let notice = Notice::Joined { seat, name };
        if self.in_lobby() {
            self.send_lobby();
        } else {
            self.send_roster();
            self.tell_others(seat, &notice);
        }
        self.notices.push(notice);
    }

    /// The shift, and everything said so far, to a player taking a seat.
    fn send_start(&mut self, peer: PeerId, seat: Seat, world: &World) {
        let Phase::Playing { setup } = &self.phase else { return };
        let start = Msg::Start {
            code: setup.code.clone(),
            staffing: setup.staffing,
            clock_start: setup.clock_start,
            seat,
        };
        self.link.send(peer, &start);
        for chunk in world.commands().chunks(CATCHUP_CHUNK) {
            self.link.send(peer, &Msg::Catchup { commands: chunk.to_vec() });
        }
        self.link.send(peer, &Msg::CaughtUp { tick: world.ticks() });
    }

    fn send_lobby(&mut self) {
        let players = self.players();
        let Phase::Lobby { positions, about, since_sent } = &mut self.phase else { return };
        *since_sent = 0.0;
        let (positions, about) = (*positions, about.clone());
        for (seat, peer) in self.player_peers() {
            let msg = Msg::Lobby { players: players.clone(), positions, about: about.clone(), you: seat };
            self.link.send(peer, &msg);
        }
    }

    fn send_roster(&mut self) {
        let msg = Msg::Roster { players: self.players() };
        for (_, peer) in self.player_peers() {
            self.link.send(peer, &msg);
        }
    }

    /// A notice for every player but the one it's about.
    fn tell_others(&mut self, about: Seat, notice: &Notice) {
        let msg = Msg::Notice(notice.clone());
        for (seat, peer) in self.player_peers() {
            if seat != about {
                self.link.send(peer, &msg);
            }
        }
    }

    /// A peer has gone: said goodbye (`left`), or dropped.
    fn gone(&mut self, peer: PeerId, left: bool) {
        let Some(p) = self.peers.remove(&peer) else { return };
        if left {
            self.link.close(peer);
        }
        let Some(seat) = p.seat else { return };
        let index = seat as usize;
        if self.seats[index] != (SeatState::Playing { peer }) {
            return;
        }
        if let Phase::Lobby { positions, .. } = &mut self.phase {
            // Gone from the lobby: their seat and positions are free.
            self.seats[index] = SeatState::Unused;
            self.names[index] = None;
            for slot in positions.iter_mut().filter(|s| **s == Some(seat)) {
                *slot = None;
            }
            self.notices.push(Notice::Left { seat, name: p.name });
            self.send_lobby();
            return;
        }
        if left {
            self.vacate(seat);
        } else {
            self.seats[index] = SeatState::Away { since: self.clock };
            let notice = Notice::Dropped { seat, name: p.name };
            self.tell_others(seat, &notice);
            self.notices.push(notice);
        }
        self.send_roster();
    }

    fn vacate(&mut self, seat: Seat) {
        self.seats[seat as usize] = SeatState::Vacated;
        self.pending.push((HOST, 0, Action::Vacate(seat)));
        let notice = Notice::Vacated { seat };
        self.tell_others(seat, &notice);
        self.notices.push(notice);
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

    /// The other players' seats and connections.
    fn player_peers(&self) -> Vec<(Seat, PeerId)> {
        (0..MAX_SEATS)
            .filter_map(|s| match self.seats[s] {
                SeatState::Playing { peer } if s as Seat != HOST => Some((s as Seat, peer)),
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
        let peers = self.player_peers();
        let tick = Msg::Tick { n, commands };
        for &(_, peer) in &peers {
            self.link.send(peer, &tick);
        }
        let Msg::Tick { commands, .. } = tick else { unreachable!() };
        apply_tick(world, &commands);
        if world.ticks().is_multiple_of(CHECKSUM_TICKS) {
            let msg = Msg::Checksum { tick: world.ticks(), sum: world.checksum() };
            for &(_, peer) in &peers {
                self.link.send(peer, &msg);
            }
        }
    }
}

/// `name`, or with a number after it if someone here has it already.
fn unique_name(name: &str, here: &[(Seat, String)]) -> String {
    let name = name.trim();
    let name = if name.is_empty() { "Controller" } else { name };
    let name: String = name.chars().take(20).collect();
    if !here.iter().any(|(_, n)| *n == name) {
        return name;
    }
    (2..).map(|n| format!("{name} {n}")).find(|c| !here.iter().any(|(_, n)| n == c)).expect("a free name")
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
    /// Connected; in the lobby, or waiting for it.
    Waiting,
    /// The shift, caught up to now, and this player's seat.
    Ready(Box<World>, Seat),
    Failed(String),
}

/// The lobby as a player sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LobbyView {
    pub players: Vec<(Seat, String)>,
    pub positions: Positions,
    pub about: String,
    /// This player's seat.
    pub seat: Seat,
}

enum ClientState {
    /// Said hello; in the lobby, or waiting for the shift.
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
    lobby: Option<LobbyView>,
    players: Vec<(Seat, String)>,
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
            lobby: None,
            players: Vec::new(),
        }
    }

    pub fn seat(&self) -> Seat {
        self.seat
    }

    pub fn setup(&self) -> Option<&ShiftSetup> {
        self.setup.as_ref()
    }

    /// The lobby, once the host has sent it.
    pub fn lobby(&self) -> Option<&LobbyView> {
        self.lobby.as_ref()
    }

    /// Who's playing, as the host last said.
    pub fn players(&self) -> &[(Seat, String)] {
        &self.players
    }

    /// In the lobby: take a position, or give it up.
    pub fn pick(&mut self, position: usize, take: bool) {
        if matches!(self.state, ClientState::Hello) {
            self.link.send(&Msg::Pick { position: position as u8, take });
        }
    }

    /// Wait for the shift (in the lobby), then build it and catch up.
    pub fn join(&mut self, dt: f32, airports: &[Airport]) -> Joining {
        self.clock += dt;
        if self.clock - self.heard > SILENCE_S {
            return self.fail("the host didn't answer".into());
        }
        if self.clock - self.acked >= ACK_S {
            self.acked = self.clock;
            self.link.send(&Msg::Ack { tick: 0 });
        }
        let mut events = self.link.poll().into_iter();
        while let Some(event) = events.next() {
            self.heard = self.clock;
            let msg = match event {
                ClientEvent::Message(msg) => msg,
                ClientEvent::Closed(why) => return self.fail(format!("the connection closed: {why}")),
            };
            match (&mut self.state, msg) {
                (ClientState::Hello, Msg::Lobby { players, positions, about, you }) => {
                    self.players = players.clone();
                    self.lobby = Some(LobbyView { players, positions, about, seat: you });
                }
                (ClientState::Hello, Msg::Start { code, staffing, clock_start, seat }) => {
                    let setup = ShiftSetup { code, staffing, clock_start };
                    self.state = ClientState::Catching { setup, seat, commands: Vec::new() };
                }
                (_, Msg::Refused { reason }) => return self.fail(reason),
                (_, Msg::Bye { reason }) => return self.fail(reason),
                (_, Msg::Roster { players }) => self.players = players,
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
                    // The rest of this poll is the shift going on, often its
                    // next ticks: keep it for `update`.
                    for event in events {
                        self.backlog.push_back(match event {
                            ClientEvent::Message(msg) => msg,
                            ClientEvent::Closed(why) => Msg::Bye { reason: format!("the connection closed: {why}") },
                        });
                    }
                    return Joining::Ready(Box::new(world), seat);
                }
                (ClientState::Playing, msg) => self.backlog.push_back(msg),
                _ => {}
            }
        }
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
                Msg::Roster { players } => self.players = players,
                Msg::Notice(notice) => notices.push(notice),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_unique() {
        let here = vec![(0, "Sam".to_string()), (1, "Sam 2".to_string())];
        assert_eq!(unique_name("Sam", &here), "Sam 3");
        assert_eq!(unique_name("Alex", &here), "Alex");
        assert_eq!(unique_name("  ", &here), "Controller");
    }

    #[test]
    fn the_lobby_choices_are_a_staffing() {
        let staffing = staffing_of(&[Some(0), Some(1), Some(1), None]);
        assert!(staffing.seat_works(0, Position::Approach));
        assert!(staffing.seat_works(1, Position::Local) && staffing.seat_works(1, Position::Ground));
        assert!(!staffing.delivery);
    }
}
