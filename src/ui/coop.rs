//! Co-op, as the game shows it: the co-op menu, the lobby, and the session
//! running behind a shift.
//!
//! From the start menu, Co-op (K) offers:
//! - **Host through the relay:** the relay gives a room code for the others.
//! - **Host on this network:** the others join with this computer's address.
//! - **Join:** a room code, or a host's address on the network.
//!
//! In the lobby players pick their positions; the host can move anyone, and
//! starts the shift. The shift is the one set up on the start menu.
//!
//! The command line has shortcuts, for testing:
//!
//! ```text
//! atcgame --host              host on this network
//! atcgame --host-relay        host through the relay
//! atcgame --join CODE|ADDRESS join
//! atcgame --relay ADDRESS     the relay to use (saved)
//! atcgame --name Sam          the name others see (saved)
//! ```

use super::gfx::{clear_background, draw_rectangle, draw_text, measure_text, mouse_position, screen_width};
use super::{BACKGROUND, DIM, TEXT, TITLE};
use crate::net::link::{ClientLink, HostLink};
use crate::net::protocol::Positions;
use crate::net::relay::{RelayClient, RelayHost};
use crate::net::session::{Client, HOST, Host, Joining, Notice, POSITIONS, SeatStatus, ShiftSetup};
use crate::net::tcp::{TcpClient, TcpHost};
use crate::sim::World;
use crate::sim::airspace::Airport;
use crate::sim::positions::{MAX_SEATS, Seat};
use crate::sim::replay::Action;
use atc_relay::wire::normalise_code;
use macroquad::prelude::{Color, KeyCode, MouseButton, Rect, Vec2, YELLOW, get_char_pressed, get_frame_time, get_time, is_key_pressed, is_mouse_button_pressed};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// The port a host on the network listens on.
pub const LAN_PORT: u16 = 7340;
/// How long a notice stays on screen, seconds.
const NOTICE_S: f64 = 8.0;
/// How long to wait for the relay or a host to answer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// How long a player whose connection went keeps trying to get back, and
/// how often.
const RECONNECT_S: f32 = 60.0;
const RETRY_S: f32 = 2.0;

pub type AnyHost = Host<Box<dyn HostLink + Send>>;
pub type AnyClient = Client<Box<dyn ClientLink + Send>>;
pub type AnyLink = Box<dyn ClientLink + Send>;

/// How to reach the host again: the same room on the relay, or the same
/// address. It blocks, so it runs off the main thread.
pub type Dial = Arc<dyn Fn() -> Result<AnyLink, String> + Send + Sync>;

/// The command line's shortcuts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoopArgs {
    pub host_lan: bool,
    pub host_relay: bool,
    pub join: Option<String>,
    pub relay: Option<String>,
    pub name: Option<String>,
}

impl CoopArgs {
    pub fn from_args() -> CoopArgs {
        CoopArgs::parse(std::env::args().skip(1))
    }

    pub fn parse(args: impl Iterator<Item = String>) -> CoopArgs {
        let args: Vec<String> = args.collect();
        let value = |flag: &str| {
            let i = args.iter().position(|a| a == flag)?;
            args.get(i + 1).filter(|v| !v.starts_with("--")).cloned()
        };
        CoopArgs {
            host_lan: args.iter().any(|a| a == "--host"),
            host_relay: args.iter().any(|a| a == "--host-relay"),
            join: value("--join"),
            relay: value("--relay"),
            name: value("--name"),
        }
    }
}

/// "192.168.1.20" with the LAN port added; "host:port" as it is.
fn lan_address(addr: &str) -> String {
    let addr = addr.trim();
    if addr.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
        addr.to_string()
    } else {
        format!("{addr}:{LAN_PORT}")
    }
}

/// This computer's address on its network, as others would reach it.
fn local_ip() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing; it only picks the way out.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// A connection made, off the main thread.
pub enum Connected {
    /// A host's link, and what to tell the others to join with.
    Host(Box<dyn HostLink + Send>, String),
    /// A player's link, and how to make another like it.
    Client(AnyLink, Dial),
}

/// The session behind a co-op shift.
pub enum Session {
    /// `label` says how the others reach it.
    Host { host: AnyHost, label: String },
    Client(Player),
}

/// A player's side of the session: the client, and getting back to the
/// host when the connection goes.
pub struct Player {
    client: AnyClient,
    dial: Dial,
    redial: Option<Redial>,
}

/// Trying to get back to the host.
#[derive(Default)]
struct Redial {
    elapsed: f32,
    next: f32,
    /// Why the last try failed.
    last: String,
    answer: Option<Receiver<Result<AnyLink, String>>>,
}

impl Player {
    pub fn new(client: AnyClient, dial: Dial) -> Player {
        Player { client, dial, redial: None }
    }

    pub fn client(&self) -> &AnyClient {
        &self.client
    }

    pub fn command(&mut self, id: u32, action: Action) {
        self.client.command(id, action);
    }

    /// Follow the host; when the connection goes, keep trying to get back,
    /// and after a reconnect or a desync, catch up. `world` is replaced by
    /// the host's once it has come again.
    pub fn update(&mut self, dt: f32, world: &mut World, airports: &[Airport]) -> Vec<Notice> {
        let mut notices = Vec::new();
        if self.client.playing() {
            notices.extend(self.client.update(dt, world));
            // The desync report is of this world: catch up from next frame.
            if notices.iter().any(|n| matches!(n, Notice::Desync { .. })) {
                return notices;
            }
        }
        if let Some(reason) = self.client.lost().map(str::to_string) {
            if world.ended.is_some() {
                self.client.leave();
                notices.push(Notice::HostGone { reason });
                return notices;
            }
            if let Some(why) = self.redial(dt, reason) {
                self.client.leave();
                self.redial = None;
                notices.push(Notice::HostGone { reason: format!("couldn't get back ({why})") });
                return notices;
            }
        }
        if self.client.rejoining() {
            match self.client.join(dt, airports) {
                Joining::Waiting => {}
                Joining::Ready(caught_up, _) => {
                    *world = *caught_up;
                    self.redial = None;
                    notices.push(Notice::CaughtUp);
                }
                Joining::Failed(why) => notices.push(Notice::HostGone { reason: why }),
            }
        }
        notices
    }

    /// Dial the host again now and then. Gives the reason when it's time to
    /// stop trying.
    fn redial(&mut self, dt: f32, reason: String) -> Option<String> {
        let r = self.redial.get_or_insert_with(|| Redial { last: reason, ..Default::default() });
        r.elapsed += dt;
        if let Some(answer) = &r.answer {
            match answer.try_recv() {
                Err(TryRecvError::Empty) => return None,
                Ok(Ok(link)) => {
                    r.answer = None;
                    self.client.reconnect(link);
                    return None;
                }
                Ok(Err(why)) => r.last = why,
                Err(TryRecvError::Disconnected) => r.last = "the connection attempt failed".into(),
            }
            r.answer = None;
            // The relay saying the room has gone is final.
            if r.last.contains("no room") {
                return Some(r.last.clone());
            }
        }
        if r.elapsed >= RECONNECT_S {
            return Some(r.last.clone());
        }
        if r.elapsed >= r.next {
            r.next = r.elapsed + RETRY_S;
            let (tx, answer) = channel();
            let dial = self.dial.clone();
            std::thread::spawn(move || {
                let _ = tx.send(dial());
            });
            r.answer = Some(answer);
        }
        None
    }

    pub fn status(&self, mine: &str) -> String {
        if self.client.playing() {
            format!("CO-OP. You: {mine}")
        } else if let Some(r) = self.redial.as_ref().filter(|_| self.client.lost().is_some()) {
            format!("CO-OP: connection lost, trying to get back ({:.0}s)", (RECONNECT_S - r.elapsed).max(0.0))
        } else if self.client.rejoining() {
            "CO-OP: catching up with the host".into()
        } else {
            "CO-OP: disconnected".into()
        }
    }
}

impl Session {
    pub fn command(&mut self, id: u32, action: Action) {
        match self {
            Session::Host { host, .. } => host.command(id, action),
            Session::Client(player) => player.client.command(id, action),
        }
    }

    /// Hear from the others and run the ticks that are due. A player's
    /// `world` may be replaced, after a reconnect or a desync
    /// (`Notice::CaughtUp`).
    pub fn update(&mut self, dt: f32, world: &mut World, airports: &[Airport]) -> Vec<Notice> {
        match self {
            Session::Host { host, .. } => host.update(dt, world),
            Session::Client(player) => player.update(dt, world, airports),
        }
    }

    /// Leave: a host ends the session for everyone.
    pub fn leave(&mut self, why: &str) {
        match self {
            Session::Host { host, .. } => host.close(why),
            Session::Client(player) => player.client.leave(),
        }
    }

    pub fn setup(&self) -> Option<&ShiftSetup> {
        match self {
            Session::Host { host, .. } => host.setup(),
            Session::Client(player) => player.client.setup(),
        }
    }

    /// Each seat's player's name.
    pub fn names(&self) -> [Option<String>; MAX_SEATS] {
        let players = match self {
            Session::Host { host, .. } => host.players(),
            Session::Client(player) => player.client.players().to_vec(),
        };
        let mut names: [Option<String>; MAX_SEATS] = Default::default();
        for (seat, name) in players {
            if let Some(slot) = names.get_mut(seat as usize) {
                *slot = Some(name);
            }
        }
        names
    }

    /// One line on who's where.
    pub fn status(&self, world: &World, seat: Seat) -> String {
        let mine = world.seated().of_seat(seat).label();
        match self {
            Session::Host { host, label } => {
                let others: Vec<String> = host
                    .seats()
                    .into_iter()
                    .filter(|s| s.status != SeatStatus::Host)
                    .map(|s| {
                        let who = s.name.unwrap_or_else(|| format!("seat {}", s.seat));
                        let positions = world.seated().of_seat(s.seat).label();
                        match s.status {
                            SeatStatus::Playing => format!("{who} on {positions}"),
                            SeatStatus::Waiting(left) => format!("{positions}: waiting {left:.0}s"),
                            SeatStatus::Automated | SeatStatus::Host => format!("{positions} automated"),
                        }
                    })
                    .collect();
                format!("CO-OP host ({label}). You: {mine}. {}", others.join(", "))
            }
            Session::Client(player) => player.status(mine),
        }
    }
}

pub fn notice_text(notice: &Notice, world: &World) -> String {
    let positions = |seat: Seat| world.seated().of_seat(seat).label();
    match notice {
        Notice::Joined { seat, name } => format!("{name} is on {}.", positions(*seat)),
        Notice::Left { name, .. } => format!("{name} has left."),
        Notice::Dropped { seat, name } => {
            format!("{name} has dropped. {} waits {:.0} seconds for them.", positions(*seat), crate::net::session::GRACE_S)
        }
        Notice::Vacated { seat } => format!("{} is automated now.", positions(*seat)),
        Notice::Refused { name, reason } => format!("{name} couldn't join: {reason}."),
        Notice::Desync { tick } => {
            format!("This shift went out of step with the host's at tick {tick}. Catching up again.")
        }
        Notice::HostGone { reason } => format!("The host's shift is gone: {reason}."),
        Notice::LinkLost { reason } => format!("Lost the connection: {reason}."),
        Notice::CaughtUp => "Back in step with the host.".into(),
    }
}

/// Notices on screen for a few seconds.
#[derive(Default)]
pub struct Banner {
    lines: Vec<(f64, String)>,
}

impl Banner {
    pub fn push(&mut self, text: String) {
        self.lines.push((get_time(), text));
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// The notices still showing, above the status line at `y`.
    pub fn draw(&mut self, status: &str, y: f32) {
        let now = get_time();
        self.lines.retain(|(at, _)| now - at < NOTICE_S);
        if !status.is_empty() {
            draw_text(status, 12.0, y, 20.0, YELLOW);
        }
        for (n, (_, text)) in self.lines.iter().rev().enumerate() {
            draw_text(text, 12.0, y - 24.0 * (n + 1) as f32, 20.0, YELLOW);
        }
    }
}

/// Write what's needed to look into a desync: the shift and every command.
pub fn save_desync_report(world: &World, setup: Option<&ShiftSetup>, tick: u32) -> Option<PathBuf> {
    let dir = super::settings::Settings::path()?.parent()?.to_path_buf();
    let path = dir.join(format!("desync-{tick}.txt"));
    let mut text = format!("Desync at tick {tick}\n{setup:?}\nchecksum here {:016x}\n", world.checksum());
    for command in world.commands() {
        text.push_str(&format!("{command:?}\n"));
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// A text field on the co-op menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    Name,
    Relay,
    /// What to join: a room code, or an address.
    Join,
}

/// Where the player is in co-op, before the shift.
pub enum Flow {
    Menu { choice: usize },
    Typing { field: Field, text: String },
    Connecting { what: String, answer: Receiver<Result<Connected, String>> },
    HostLobby { host: Box<AnyHost>, label: String },
    PlayerLobby { client: Box<AnyClient>, dial: Dial },
    Failed(String),
}

/// What the game needs to know after a co-op frame.
pub enum Outcome {
    Stay,
    /// Back to the start menu.
    Menu,
    /// The name or relay changed: save the settings.
    Save,
    /// The host started the shift: build it and play.
    Start(Box<AnyHost>, String),
    /// The host started the shift, and this player has caught up.
    Joined(Box<AnyClient>, Box<World>, Seat, Dial),
}

/// What a co-op frame works with.
pub struct Ctx<'a> {
    pub name: &'a mut String,
    pub relay: &'a mut String,
    pub airports: &'a [Airport],
    /// What the shift will be, from the start menu.
    pub about: String,
}

const MENU: [&str; 6] = ["Host through the relay", "Host on this network", "Join a shift", "Name", "Relay", "Back"];

impl Flow {
    /// Start connecting on another thread; the flow waits for it.
    fn connect(what: String, make: impl FnOnce() -> Result<Connected, String> + Send + 'static) -> Flow {
        let (tx, answer) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(make());
        });
        Flow::Connecting { what, answer }
    }

    /// Host through the relay at `relay`.
    pub fn host_relay(relay: &str) -> Flow {
        if relay.trim().is_empty() {
            return Flow::Failed("Set the relay's address first, on the co-op menu.".into());
        }
        let relay = relay.to_string();
        Flow::connect(format!("Opening a room on the relay at {relay}"), move || {
            let host = RelayHost::open(&relay, CONNECT_TIMEOUT)?;
            let label = format!("room {}", host.code());
            Ok(Connected::Host(Box::new(host), label))
        })
    }

    /// Host on this network.
    pub fn host_lan() -> Flow {
        Flow::connect("Opening the room on this network".into(), || {
            let host = TcpHost::listen(("0.0.0.0", LAN_PORT))
                .map_err(|e| format!("couldn't listen on port {LAN_PORT}: {e}"))?;
            let at = local_ip().map(|ip| ip.to_string()).unwrap_or_else(|| "this computer's address".into());
            Ok(Connected::Host(Box::new(host), at))
        })
    }

    /// Join with a room code, through the relay, or an address on the network.
    pub fn join(what: &str, relay: &str) -> Flow {
        if let Some(code) = normalise_code(what) {
            if relay.trim().is_empty() {
                return Flow::Failed("Set the relay's address first, on the co-op menu.".into());
            }
            let relay = relay.to_string();
            let what = format!("Joining room {code}");
            let dial: Dial = Arc::new(move || Ok(Box::new(RelayClient::join(&relay, &code, CONNECT_TIMEOUT)?) as AnyLink));
            return Flow::connect(what, move || Ok(Connected::Client(dial()?, dial)));
        }
        if what.trim().is_empty() {
            return Flow::Failed("Type a room code, or the host's address.".into());
        }
        let addr = lan_address(what);
        let what = format!("Connecting to {addr}");
        let dial: Dial = Arc::new(move || {
            let link = TcpClient::connect(&addr, CONNECT_TIMEOUT).map_err(|e| format!("couldn't reach {addr}: {e}"))?;
            Ok(Box::new(link) as AnyLink)
        });
        Flow::connect(what, move || Ok(Connected::Client(dial()?, dial)))
    }
}

/// One frame of co-op before the shift.
pub fn frame(flow: &mut Flow, ctx: Ctx) -> Outcome {
    let dt = get_frame_time();
    let escape = is_key_pressed(KeyCode::Escape);
    let enter = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter);
    match flow {
        Flow::Menu { choice } => {
            clear_background(BACKGROUND);
            let count = MENU.len();
            if is_key_pressed(KeyCode::Up) {
                *choice = (*choice + count - 1) % count;
            }
            if is_key_pressed(KeyCode::Down) {
                *choice = (*choice + 1) % count;
            }
            let relay_shown = if ctx.relay.is_empty() { "not set".to_string() } else { ctx.relay.clone() };
            let labels: Vec<String> = MENU
                .iter()
                .map(|&item| match item {
                    "Name" => format!("Name: {}", ctx.name),
                    "Relay" => format!("Relay: {relay_shown}"),
                    other => other.to_string(),
                })
                .collect();
            let (hovered, clicked) = super::draw_menu_list("CO-OP", *choice, &labels, "Up/Down  Enter choose  ESC back");
            if let Some(index) = hovered {
                *choice = index;
            }
            let note = format!("The shift: {}. Change it on the start menu.", ctx.about);
            let width = measure_text(&note, None, 18, 1.0).width;
            draw_text(&note, (screen_width() - width) / 2.0, 60.0, 18.0, DIM);
            if escape {
                return Outcome::Menu;
            }
            if !(enter || clicked) {
                return Outcome::Stay;
            }
            *flow = match MENU[*choice] {
                "Host through the relay" => Flow::host_relay(ctx.relay),
                "Host on this network" => Flow::host_lan(),
                "Join a shift" => Flow::Typing { field: Field::Join, text: String::new() },
                "Name" => Flow::Typing { field: Field::Name, text: ctx.name.clone() },
                "Relay" => Flow::Typing { field: Field::Relay, text: ctx.relay.clone() },
                _ => return Outcome::Menu,
            };
            Outcome::Stay
        }
        Flow::Typing { field, text } => {
            while let Some(c) = get_char_pressed() {
                if !c.is_control() && text.chars().count() < 60 {
                    text.push(c);
                }
            }
            if is_key_pressed(KeyCode::Backspace) {
                text.pop();
            }
            let (title, hint) = match field {
                Field::Name => ("YOUR NAME", "The name the other players see"),
                Field::Relay => ("RELAY", "The relay server's address, such as relay.example.com or 203.0.113.5:7341"),
                Field::Join => ("JOIN A SHIFT", "A room code from the host, or the host's address on your network"),
            };
            draw_typing(title, text, hint);
            if escape {
                *flow = Flow::Menu { choice: 0 };
                return Outcome::Stay;
            }
            if !enter {
                return Outcome::Stay;
            }
            let typed = text.trim().to_string();
            match field {
                Field::Name => {
                    if !typed.is_empty() {
                        *ctx.name = typed.chars().take(20).collect();
                    }
                    *flow = Flow::Menu { choice: 3 };
                    Outcome::Save
                }
                Field::Relay => {
                    *ctx.relay = typed;
                    *flow = Flow::Menu { choice: 4 };
                    Outcome::Save
                }
                Field::Join => {
                    *flow = Flow::join(&typed, ctx.relay);
                    Outcome::Stay
                }
            }
        }
        Flow::Connecting { what, answer } => {
            draw_message("CO-OP", &format!("{what}..."), "ESC cancel");
            if escape {
                *flow = Flow::Menu { choice: 0 };
                return Outcome::Stay;
            }
            *flow = match answer.try_recv() {
                Err(TryRecvError::Empty) => return Outcome::Stay,
                Err(TryRecvError::Disconnected) => Flow::Failed("The connection attempt failed.".into()),
                Ok(Err(why)) => Flow::Failed(capitalise(&why)),
                Ok(Ok(Connected::Host(link, label))) => {
                    Flow::HostLobby { host: Box::new(Host::lobby(link, ctx.name, &ctx.about)), label }
                }
                Ok(Ok(Connected::Client(link, dial))) => {
                    Flow::PlayerLobby { client: Box::new(Client::new(link, ctx.name)), dial }
                }
            };
            Outcome::Stay
        }
        Flow::HostLobby { host, label } => {
            for notice in host.lobby_update(dt) {
                if let Notice::LinkLost { reason } = notice {
                    *flow = Flow::Failed(format!("The room closed: {reason}."));
                    return Outcome::Stay;
                }
            }
            let players = host.players();
            let positions = host.positions();
            let ready = host.can_start();
            let footer = match &ready {
                Ok(()) => "1-4 take a position  Click one to give it to someone  ENTER start the shift  ESC close the room".to_string(),
                Err(why) => format!("1-4 take a position  Click one to give it to someone  ({why})  ESC close the room"),
            };
            let joined_with = if let Some(code) = label.strip_prefix("room ") {
                format!("Others join with the code  {code}")
            } else {
                format!("Others join with this address  {label}")
            };
            let clicked = draw_lobby(&joined_with, &ctx.about, &players, &positions, HOST, &footer);
            if let Some(index) = number_key() {
                let mine = positions[index] == Some(HOST);
                host.give(index, if mine { None } else { Some(HOST) });
            }
            if let Some(index) = clicked {
                // Round the players, then nobody.
                let next = match positions[index] {
                    None => players.first().map(|(s, _)| *s),
                    Some(seat) => {
                        let at = players.iter().position(|(s, _)| *s == seat);
                        at.and_then(|i| players.get(i + 1)).map(|(s, _)| *s)
                    }
                };
                host.give(index, next);
            }
            if escape {
                host.close("the host closed the room");
                *flow = Flow::Menu { choice: 0 };
                return Outcome::Stay;
            }
            if enter && ready.is_ok() {
                let Flow::HostLobby { host, label } = std::mem::replace(flow, Flow::Menu { choice: 0 }) else {
                    unreachable!()
                };
                return Outcome::Start(host, label);
            }
            Outcome::Stay
        }
        Flow::PlayerLobby { client, .. } => {
            match client.join(dt, ctx.airports) {
                Joining::Waiting => {}
                Joining::Ready(world, seat) => {
                    let Flow::PlayerLobby { client, dial } = std::mem::replace(flow, Flow::Menu { choice: 0 }) else {
                        unreachable!()
                    };
                    return Outcome::Joined(client, world, seat, dial);
                }
                Joining::Failed(why) => {
                    *flow = Flow::Failed(capitalise(&format!("{why}.")));
                    return Outcome::Stay;
                }
            }
            match client.lobby().cloned() {
                Some(lobby) => {
                    let clicked = draw_lobby(
                        "In the host's room",
                        &lobby.about,
                        &lobby.players,
                        &lobby.positions,
                        lobby.seat,
                        "1-4 or click to take a position or give it up  The host starts the shift  ESC leave",
                    );
                    if let Some(index) = number_key().or(clicked) {
                        let mine = lobby.positions[index] == Some(lobby.seat);
                        client.pick(index, !mine);
                    }
                }
                None => draw_message("CO-OP", "Waiting for the host...", "ESC leave"),
            }
            if escape {
                client.leave();
                *flow = Flow::Menu { choice: 0 };
            }
            Outcome::Stay
        }
        Flow::Failed(why) => {
            draw_message("CO-OP", why, "ENTER or ESC back");
            if enter || escape {
                *flow = Flow::Menu { choice: 0 };
            }
            Outcome::Stay
        }
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// 1 to 4: a position.
fn number_key() -> Option<usize> {
    [KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4].iter().position(|&k| is_key_pressed(k))
}

fn centred(text: &str, y: f32, size: f32, color: Color) {
    let width = measure_text(text, None, size as u16, 1.0).width;
    draw_text(text, (screen_width() - width) / 2.0, y, size, color);
}

fn draw_message(title: &str, text: &str, hint: &str) {
    clear_background(BACKGROUND);
    centred(title, 160.0, 36.0, TITLE);
    centred(text, 230.0, 24.0, TEXT);
    centred(hint, 290.0, 18.0, DIM);
}

fn draw_typing(title: &str, text: &str, hint: &str) {
    clear_background(BACKGROUND);
    centred(title, 160.0, 36.0, TITLE);
    let caret = if (get_time() * 2.0) as i64 % 2 == 0 { "_" } else { " " };
    centred(&format!("{text}{caret}"), 230.0, 28.0, TEXT);
    centred(hint, 280.0, 18.0, DIM);
    centred("ENTER done  ESC cancel", 320.0, 18.0, DIM);
}

/// The lobby: how to join, the shift, who's here, and who has which
/// position. Returns the position row clicked, if one was.
fn draw_lobby(
    join: &str,
    about: &str,
    players: &[(Seat, String)],
    positions: &Positions,
    me: Seat,
    footer: &str,
) -> Option<usize> {
    clear_background(BACKGROUND);
    let x = 120.0;
    draw_text("CO-OP LOBBY", x, 90.0, 36.0, TITLE);
    draw_text(join, x, 140.0, 28.0, YELLOW);
    draw_text(format!("The shift: {about}"), x, 180.0, 20.0, TEXT);

    let name_of = |seat: Seat| {
        let name = players.iter().find(|(s, _)| *s == seat).map(|(_, n)| n.as_str()).unwrap_or("?");
        if seat == me { format!("{name} (you)") } else { name.to_string() }
    };
    draw_text("PLAYERS", x, 240.0, 20.0, DIM);
    for (n, (seat, _)) in players.iter().enumerate() {
        let host = if *seat == HOST { "  host" } else { "" };
        draw_text(format!("{}{host}", name_of(*seat)), x, 272.0 + 28.0 * n as f32, 22.0, TEXT);
    }

    let px = x + 420.0;
    draw_text("POSITIONS", px, 240.0, 20.0, DIM);
    let mouse = Vec2::from(mouse_position());
    let mut clicked = None;
    for (index, position) in POSITIONS.iter().enumerate() {
        let y = 250.0 + 40.0 * index as f32;
        let row = Rect::new(px - 8.0, y, 460.0, 34.0);
        let hovered = row.contains(mouse);
        if hovered {
            draw_rectangle(row.x, row.y, row.w, row.h, Color::new(0.10, 0.22, 0.12, 1.0));
            if is_mouse_button_pressed(MouseButton::Left) {
                clicked = Some(index);
            }
        }
        let who = match positions[index] {
            Some(seat) => name_of(seat),
            None => "automated".into(),
        };
        let color = if positions[index] == Some(me) { TITLE } else if positions[index].is_some() { TEXT } else { DIM };
        draw_text(format!("{}  {:<10} {who}", index + 1, position.name()), px, y + 24.0, 22.0, color);
    }
    draw_text("Positions nobody takes are worked by the automation.", px, 430.0, 18.0, DIM);
    centred(footer, 520.0, 18.0, DIM);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> CoopArgs {
        CoopArgs::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn command_line() {
        assert!(parse(&["--host"]).host_lan);
        assert!(parse(&["--host-relay", "--relay", "r.example.com"]).host_relay);
        assert_eq!(parse(&["--relay", "r.example.com"]).relay.as_deref(), Some("r.example.com"));
        assert_eq!(parse(&["--join", "ACDEFH", "--name", "Sam"]).join.as_deref(), Some("ACDEFH"));
        assert_eq!(parse(&["--join", "ACDEFH", "--name", "Sam"]).name.as_deref(), Some("Sam"));
        assert_eq!(parse(&["--join", "--name", "Sam"]).join, None);
        assert_eq!(parse(&[]), CoopArgs::default());
    }

    #[test]
    fn network_addresses() {
        assert_eq!(lan_address("192.168.1.5"), "192.168.1.5:7340");
        assert_eq!(lan_address(" box.local:9000 "), "box.local:9000");
    }
}
