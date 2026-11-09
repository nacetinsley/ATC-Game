//! Co-op over a connection, as the game shows it: hosting or joining from
//! the command line (the lobby comes later), the session running behind the
//! screen, and what it has to say.
//!
//! ```text
//! atcgame --host [port]          host each shift; others join it
//! atcgame --join address[:port]  join a host's shift
//! atcgame --name Sam             the name others see
//! ```
//!
//! A hosted shift is split as in hot seat: Approach for the host, the tower
//! cab for the player who joins.

use super::gfx::{clear_background, draw_text};
use super::{BACKGROUND, DIM, TEXT};
use crate::net::session::{Client, Host, Notice, SeatStatus, ShiftSetup};
use crate::net::tcp::{TcpClient, TcpHost};
use crate::sim::World;
use crate::sim::positions::Seat;
use crate::sim::replay::Action;
use macroquad::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 7340;
/// How long a notice stays on screen, seconds.
const NOTICE_S: f64 = 8.0;

/// Co-op as asked for on the command line.
#[derive(Debug, Clone, Default)]
pub struct CoopArgs {
    /// Host shifts on this port.
    pub host: Option<u16>,
    /// Join the host at this address.
    pub join: Option<String>,
    pub name: String,
}

impl CoopArgs {
    pub fn from_args() -> CoopArgs {
        CoopArgs::parse(std::env::args().skip(1))
    }

    pub fn parse(args: impl Iterator<Item = String>) -> CoopArgs {
        let args: Vec<String> = args.collect();
        let after = |flag: &str| {
            let i = args.iter().position(|a| a == flag)?;
            Some(args.get(i + 1).filter(|v| !v.starts_with("--")).cloned())
        };
        let host = after("--host").map(|port| port.and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT));
        let join = after("--join").flatten().map(|addr| {
            if addr.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
                addr
            } else {
                format!("{addr}:{DEFAULT_PORT}")
            }
        });
        let name = after("--name")
            .flatten()
            .or_else(|| std::env::var("USERNAME").ok())
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "Controller".into());
        CoopArgs { host, join, name }
    }
}

/// Connect to a host, as `--join` asks. Blocks for a few seconds at most.
pub fn connect(addr: &str, name: &str) -> Result<Client<TcpClient>, String> {
    let link = TcpClient::connect(addr, Duration::from_secs(5)).map_err(|e| format!("Couldn't reach {addr}: {e}"))?;
    Ok(Client::new(link, name))
}

/// Listen for players, as `--host` asks.
pub fn listen(port: u16, setup: ShiftSetup, name: &str) -> Result<Host<TcpHost>, String> {
    let link = TcpHost::listen(("0.0.0.0", port)).map_err(|e| format!("Couldn't host on port {port}: {e}"))?;
    Ok(Host::new(link, setup, name))
}

/// The session behind a co-op shift.
pub enum Session {
    Host { host: Host<TcpHost>, port: u16 },
    Client(Client<TcpClient>),
}

impl Session {
    pub fn command(&mut self, id: u32, action: Action) {
        match self {
            Session::Host { host, .. } => host.command(id, action),
            Session::Client(client) => client.command(id, action),
        }
    }

    /// Hear from the others and run the ticks that are due.
    pub fn update(&mut self, dt: f32, world: &mut World) -> Vec<Notice> {
        match self {
            Session::Host { host, .. } => host.update(dt, world),
            Session::Client(client) => client.update(dt, world),
        }
    }

    /// Leave: a host ends the session for everyone.
    pub fn leave(&mut self, why: &str) {
        match self {
            Session::Host { host, .. } => host.close(why),
            Session::Client(client) => client.leave(),
        }
    }

    pub fn setup(&self) -> Option<&ShiftSetup> {
        match self {
            Session::Host { host, .. } => Some(host.setup()),
            Session::Client(client) => client.setup(),
        }
    }

    /// One line on who's where.
    pub fn status(&self, world: &World, seat: Seat) -> String {
        let mine = world.seated().of_seat(seat).label();
        match self {
            Session::Host { host, port } => {
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
                format!("CO-OP host, port {port}. You: {mine}. {}", others.join(", "))
            }
            Session::Client(client) if client.playing() => format!("CO-OP seat {seat}. You: {mine}"),
            Session::Client(_) => "CO-OP: disconnected".into(),
        }
    }
}

pub fn notice_text(notice: &Notice, world: &World) -> String {
    let positions = |seat: Seat| world.seated().of_seat(seat).label();
    match notice {
        Notice::Joined { seat, name } => format!("{name} has joined on {}.", positions(*seat)),
        Notice::Dropped { seat, name } => {
            format!("{name} has dropped. {} waits {:.0} seconds for them.", positions(*seat), crate::net::session::GRACE_S)
        }
        Notice::Vacated { seat } => format!("{} is automated now.", positions(*seat)),
        Notice::Refused { name, reason } => format!("{name} couldn't join: {reason}."),
        Notice::Desync { tick } => format!("This shift went out of step with the host's at tick {tick}."),
        Notice::HostGone { reason } => format!("The host's shift is gone: {reason}."),
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

/// The joining screen: who we're joining, and how it's going.
pub fn draw_joining(addr: &str, status: &str) {
    clear_background(BACKGROUND);
    let x = 80.0;
    draw_text("JOINING A CO-OP SHIFT", x, 140.0, 36.0, TEXT);
    draw_text(&format!("Host  {addr}"), x, 200.0, 24.0, TEXT);
    draw_text(status, x, 240.0, 24.0, TEXT);
    draw_text("ESC back to the menu", x, 300.0, 18.0, DIM);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> CoopArgs {
        CoopArgs::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn command_line() {
        assert_eq!(parse(&["--host"]).host, Some(DEFAULT_PORT));
        assert_eq!(parse(&["--host", "9000"]).host, Some(9000));
        assert_eq!(parse(&["--host", "--name", "Sam"]).host, Some(DEFAULT_PORT));
        assert_eq!(parse(&["--host", "--name", "Sam"]).name, "Sam");
        assert_eq!(parse(&["--join", "192.168.1.5"]).join.as_deref(), Some("192.168.1.5:7340"));
        assert_eq!(parse(&["--join", "box.local:9000"]).join.as_deref(), Some("box.local:9000"));
        assert_eq!(parse(&[]).host, None);
        assert_eq!(parse(&[]).join, None);
    }
}
