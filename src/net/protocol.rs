//! The messages players' machines send each other, and their framing: a
//! 4-byte little-endian length, then the message in `postcard`.

use super::session::Notice;
use crate::sim::positions::{MAX_SEATS, Seat, Staffing};
use crate::sim::replay::{Action, Command};
use serde::{Deserialize, Serialize};

/// Bumped when a message changes shape.
pub const PROTOCOL: u32 = 3;

/// Who has each position in the lobby: Approach, Local, Ground, Delivery.
pub type Positions = [Option<Seat>; MAX_SEATS];

/// The largest frame either side accepts. A catch-up's commands go in
/// chunks that fit.
pub const MAX_FRAME: usize = 64 * 1024;

/// Commands in one catch-up chunk: well inside a frame.
pub const CATCHUP_CHUNK: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// Client to host, first: who's asking, and which build. Lockstep needs
    /// the same sim on both ends, so the game version has to match.
    Hello { protocol: u32, game: String, name: String },
    /// Host to client: it can't join.
    Refused { reason: String },
    /// Host to client, before the shift: who's here, who has which
    /// position, what the shift will be, and the client's own seat. Sent
    /// again now and then.
    Lobby { players: Vec<(Seat, String)>, positions: Positions, about: String, you: Seat },
    /// Client to host, in the lobby: take or give up a position (0
    /// Approach, 1 Local, 2 Ground, 3 Delivery).
    Pick { position: u8, take: bool },
    /// Host to client, in the shift: who's playing, when that changes.
    Roster { players: Vec<(Seat, String)> },
    /// Host to client: something to tell the player, such as someone
    /// dropping.
    Notice(Notice),
    /// Host to client: the shift, and the client's seat. `code` is the shift
    /// code (its staffing isn't used: `staffing` is the co-op split).
    /// `token` is the seat's secret, for `Rejoin`.
    Start { code: String, staffing: Staffing, clock_start: f64, seat: Seat, token: u64 },
    /// Host to client: commands so far, in order, in chunks.
    Catchup { commands: Vec<Command> },
    /// Host to client: the catch-up is complete. Run the shift to `tick` with
    /// those commands, then follow the ticks.
    CaughtUp { tick: u32 },
    /// Host to client: run tick `n` after applying these commands.
    Tick { n: u32, commands: Vec<Command> },
    /// Client to host: something this player said, for the next tick.
    Command { id: u32, action: Action },
    /// Host to client: the world's checksum once it has run `tick` ticks.
    Checksum { tick: u32, sum: u64 },
    /// Client to host, every second: still here, and this far along.
    Ack { tick: u32 },
    /// Either way: leaving, and why.
    Bye { reason: String },
    /// Client to host, first, instead of `Hello`: a player whose connection
    /// went, back for their seat. The host sends `Start` and a catch-up.
    Rejoin { protocol: u32, game: String, seat: Seat, token: u64 },
    /// Client to host: this machine went out of step. The host sends `Start`
    /// and a catch-up again.
    Resync,
}

/// The version string a build sends in `Hello`.
pub fn game_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    TooBig(usize),
    Garbled(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FrameError::TooBig(n) => write!(f, "a message of {n} bytes is too big"),
            FrameError::Garbled(why) => write!(f, "a message didn't make sense: {why}"),
        }
    }
}

/// A message as a frame's body: what the relay carries.
pub fn body(msg: &Msg) -> Vec<u8> {
    let body = postcard::to_allocvec(msg).expect("messages serialise");
    assert!(body.len() <= MAX_FRAME, "a {}-byte message: chunk it", body.len());
    body
}

/// A frame's body, back into a message.
pub fn parse(body: &[u8]) -> Result<Msg, FrameError> {
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooBig(body.len()));
    }
    postcard::from_bytes(body).map_err(|e| FrameError::Garbled(e.to_string()))
}

/// A message as a frame for the wire.
pub fn encode(msg: &Msg) -> Vec<u8> {
    let body = body(msg);
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(&body);
    frame
}

/// Bytes off the wire, back into messages.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole message, if one has arrived.
    pub fn next_msg(&mut self) -> Result<Option<Msg>, FrameError> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err(FrameError::TooBig(len));
        }
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }
        let msg = postcard::from_bytes(&self.buffer[4..4 + len]).map_err(|e| FrameError::Garbled(e.to_string()));
        self.buffer.drain(..4 + len);
        msg.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::clearance::Clearance;

    #[test]
    fn messages_round_trip_through_frames_split_anywhere() {
        let msgs = vec![
            Msg::Hello { protocol: PROTOCOL, game: game_version(), name: "Sam".into() },
            Msg::Start { code: "XHGT-0000-0000-00000".into(), staffing: Staffing::ALL, clock_start: 1234.5, seat: 1, token: 7 },
            Msg::Tick {
                n: 7,
                commands: vec![Command {
                    tick: 7,
                    id: 3,
                    action: Action::Transmit(vec![Clearance::Heading(270.0), Clearance::Altitude(4000.0)]),
                    seat: 1,
                }],
            },
            Msg::Command { id: 0, action: Action::Vacate(2) },
            Msg::Checksum { tick: 60, sum: u64::MAX - 5 },
            Msg::Bye { reason: "done".into() },
        ];
        let wire: Vec<u8> = msgs.iter().flat_map(encode).collect();
        for split in [1, 3, 5, wire.len()] {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            for chunk in wire.chunks(split) {
                decoder.push(chunk);
                while let Some(msg) = decoder.next_msg().unwrap() {
                    out.push(msg);
                }
            }
            assert_eq!(out, msgs, "in chunks of {split}");
        }
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        let mut decoder = Decoder::default();
        decoder.push(&((MAX_FRAME as u32) + 1).to_le_bytes());
        assert!(matches!(decoder.next_msg(), Err(FrameError::TooBig(_))));
    }
}
