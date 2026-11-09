//! The relay's own messages, and the framing they share with the game: a
//! 4-byte little-endian length, then the body. A body's first byte says
//! what it is. Payloads are the game's messages, passed on unread.

use std::io::{self, Read};

/// Bumped when a relay message changes shape.
pub const VERSION: u8 = 1;
/// The largest payload: one game message.
pub const MAX_PAYLOAD: usize = 64 * 1024;
/// The largest frame: a payload and its header.
pub const MAX_FRAME: usize = MAX_PAYLOAD + 16;
/// Room codes are this long...
pub const CODE_LEN: usize = 6;
/// ...in letters that can't be taken for each other or for digits: no B
/// (8), G (6), I or L (1), O or Q (0), S (5), Z (2).
pub const CODE_ALPHABET: &[u8] = b"ACDEFHJKMNPRTUVWXY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayMsg {
    // A client to the relay.
    /// Open a room, as its host.
    Open { version: u8 },
    /// Join the room with this code.
    Join { version: u8, code: String },
    /// The host, to one of its peers.
    Send { to: u32, payload: Vec<u8> },
    /// A peer to the host; or the relay, to a peer, from the host.
    Data { payload: Vec<u8> },
    /// The host hangs up on a peer.
    Kick { peer: u32 },

    // The relay to a client.
    /// The room is open: tell the others this code.
    Opened { code: String },
    /// In the room.
    Joined,
    /// To the host: someone came in.
    PeerJoined { peer: u32 },
    /// To the host: from a peer.
    PeerData { peer: u32, payload: Vec<u8> },
    /// To the host: a peer has gone.
    PeerLeft { peer: u32 },
    /// Turned down, or cut off, and why.
    Error { reason: String },
}

impl RelayMsg {
    /// The message as a frame for the wire.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        match self {
            RelayMsg::Open { version } => body.extend([1, *version]),
            RelayMsg::Join { version, code } => {
                body.extend([2, *version]);
                body.extend(code.as_bytes());
            }
            RelayMsg::Send { to, payload } => {
                body.push(3);
                body.extend(to.to_le_bytes());
                body.extend(payload);
            }
            RelayMsg::Data { payload } => {
                body.push(4);
                body.extend(payload);
            }
            RelayMsg::Kick { peer } => {
                body.push(5);
                body.extend(peer.to_le_bytes());
            }
            RelayMsg::Opened { code } => {
                body.push(10);
                body.extend(code.as_bytes());
            }
            RelayMsg::Joined => body.push(11),
            RelayMsg::PeerJoined { peer } => {
                body.push(12);
                body.extend(peer.to_le_bytes());
            }
            RelayMsg::PeerData { peer, payload } => {
                body.push(13);
                body.extend(peer.to_le_bytes());
                body.extend(payload);
            }
            RelayMsg::PeerLeft { peer } => {
                body.push(14);
                body.extend(peer.to_le_bytes());
            }
            RelayMsg::Error { reason } => {
                body.push(15);
                body.extend(reason.as_bytes());
            }
        }
        frame(&body)
    }

    /// A frame's body, read back.
    pub fn decode(body: &[u8]) -> Result<RelayMsg, String> {
        let (&kind, rest) = body.split_first().ok_or("an empty message")?;
        let u32_then = |rest: &[u8]| -> Result<(u32, Vec<u8>), String> {
            if rest.len() < 4 {
                return Err("a message too short".into());
            }
            Ok((u32::from_le_bytes(rest[..4].try_into().unwrap()), rest[4..].to_vec()))
        };
        let text = |rest: &[u8]| String::from_utf8(rest.to_vec()).map_err(|_| "text that isn't UTF-8".to_string());
        let byte = |rest: &[u8]| rest.first().copied().ok_or_else(|| "a message too short".to_string());
        Ok(match kind {
            1 => RelayMsg::Open { version: byte(rest)? },
            2 => RelayMsg::Join { version: byte(rest)?, code: text(&rest[1..])? },
            3 => {
                let (to, payload) = u32_then(rest)?;
                RelayMsg::Send { to, payload }
            }
            4 => RelayMsg::Data { payload: rest.to_vec() },
            5 => RelayMsg::Kick { peer: u32_then(rest)?.0 },
            10 => RelayMsg::Opened { code: text(rest)? },
            11 => RelayMsg::Joined,
            12 => RelayMsg::PeerJoined { peer: u32_then(rest)?.0 },
            13 => {
                let (peer, payload) = u32_then(rest)?;
                RelayMsg::PeerData { peer, payload }
            }
            14 => RelayMsg::PeerLeft { peer: u32_then(rest)?.0 },
            15 => RelayMsg::Error { reason: text(rest)? },
            other => return Err(format!("a message of kind {other}")),
        })
    }
}

/// A body as a frame.
pub fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// Bytes in, frame bodies out.
#[derive(Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole frame's body, if one has arrived.
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err(format!("a frame of {len} bytes is too big"));
        }
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buffer[4..4 + len].to_vec();
        self.buffer.drain(..4 + len);
        Ok(Some(body))
    }
}

/// Read one frame's body, waiting for it.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes is too big")));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// A room code as typed: upper case, spaces and dashes gone. None if it
/// can't be one.
pub fn normalise_code(typed: &str) -> Option<String> {
    let code: String =
        typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
    (code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let all = [
            RelayMsg::Open { version: VERSION },
            RelayMsg::Join { version: VERSION, code: "ACDEFH".into() },
            RelayMsg::Send { to: 3, payload: vec![1, 2, 3] },
            RelayMsg::Data { payload: vec![] },
            RelayMsg::Kick { peer: 9 },
            RelayMsg::Opened { code: "XYWVUT".into() },
            RelayMsg::Joined,
            RelayMsg::PeerJoined { peer: 1 },
            RelayMsg::PeerData { peer: 2, payload: vec![9; 100] },
            RelayMsg::PeerLeft { peer: 7 },
            RelayMsg::Error { reason: "full".into() },
        ];
        let mut reader = FrameReader::default();
        for msg in &all {
            reader.push(&msg.encode());
        }
        for msg in &all {
            let body = reader.next().unwrap().unwrap();
            assert_eq!(&RelayMsg::decode(&body).unwrap(), msg);
        }
        assert_eq!(reader.next(), Ok(None));
    }

    #[test]
    fn codes_as_typed() {
        assert_eq!(normalise_code("acd-efh").as_deref(), Some("ACDEFH"));
        assert_eq!(normalise_code(" XY WV UT ").as_deref(), Some("XYWVUT"));
        assert_eq!(normalise_code("ACDEF"), None);
        assert_eq!(normalise_code("ACDEF0"), None, "no look-alikes");
        assert_eq!(normalise_code("ACDEFB"), None);
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(RelayMsg::decode(&[]).is_err());
        assert!(RelayMsg::decode(&[99]).is_err());
        assert!(RelayMsg::decode(&[3, 1]).is_err());
        let mut reader = FrameReader::default();
        reader.push(&(MAX_FRAME as u32 + 1).to_le_bytes());
        assert!(reader.next().is_err());
    }
}
