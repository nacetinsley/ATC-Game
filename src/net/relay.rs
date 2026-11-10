//! Links through the relay server. The host opens a room and gets its code;
//! the others join with the code. The relay passes the game's messages on
//! unread, so the sessions work just as over a direct connection.

use super::link::{ClientEvent, ClientLink, HostEvent, HostLink, PeerId};
use super::protocol::{Msg, body, parse};
use super::tcp::{RawConn, connect};
use atc_relay::wire::{RelayMsg, VERSION, read_frame};
use std::collections::BTreeSet;
use std::time::Duration;

/// The relay's port, when an address doesn't give one.
pub const DEFAULT_RELAY_PORT: u16 = 7341;

/// The relay players use when they haven't set one under Co-op, Relay.
/// Empty: there's no public relay yet, so each player sets one.
pub const DEFAULT_RELAY: &str = "";

/// The relay to use: the player's own, or else the default. Empty if
/// neither is set.
pub fn relay_or_default(set: &str) -> &str {
    if set.trim().is_empty() { DEFAULT_RELAY } else { set.trim() }
}

/// "relay.example.com" with the default port added; "host:port" as it is.
pub fn with_port(addr: &str) -> String {
    let addr = addr.trim();
    if addr.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_RELAY_PORT}")
    }
}

/// Connect to the relay, say `hello`, and wait for its answer.
fn handshake(addr: &str, hello: RelayMsg, timeout: Duration) -> Result<(RawConn, RelayMsg), String> {
    let addr = with_port(addr);
    let mut stream = connect(&addr, timeout).map_err(|e| format!("couldn't reach the relay at {addr}: {e}"))?;
    use std::io::Write;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.write_all(&hello.encode()).map_err(|e| format!("the relay hung up: {e}"))?;
    let answer = read_frame(&mut stream).map_err(|e| format!("the relay didn't answer: {e}"))?;
    let answer = RelayMsg::decode(&answer)?;
    if let RelayMsg::Error { reason } = answer {
        return Err(reason);
    }
    let conn = RawConn::new(stream).map_err(|e| e.to_string())?;
    Ok((conn, answer))
}

/// The host's end: a room on the relay.
pub struct RelayHost {
    conn: Option<RawConn>,
    code: String,
    /// The room's secret, to reopen it.
    key: String,
    peers: BTreeSet<PeerId>,
    failure: Option<String>,
}

impl RelayHost {
    /// Open a room on the relay at `addr`. Blocks for up to `timeout`.
    pub fn open(addr: &str, timeout: Duration) -> Result<RelayHost, String> {
        RelayHost::opened(handshake(addr, RelayMsg::Open { version: VERSION }, timeout)?)
    }

    /// Back to the room after the connection went, with its key. The relay
    /// keeps it a while (and its players call back).
    pub fn reopen(addr: &str, code: &str, key: &str, timeout: Duration) -> Result<RelayHost, String> {
        let hello = RelayMsg::Reopen { version: VERSION, code: code.into(), key: key.into() };
        RelayHost::opened(handshake(addr, hello, timeout)?)
    }

    fn opened(answer: (RawConn, RelayMsg)) -> Result<RelayHost, String> {
        match answer {
            (conn, RelayMsg::Opened { code, key }) => {
                Ok(RelayHost { conn: Some(conn), code, key, peers: BTreeSet::new(), failure: None })
            }
            (_, other) => Err(format!("the relay answered {other:?}")),
        }
    }

    /// The room's code, for the others to join with.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The room's key, to reopen it. Only the host has it.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn fail(&mut self, why: String, events: &mut Vec<HostEvent>) {
        if let Some(mut conn) = self.conn.take() {
            conn.close();
        }
        for peer in std::mem::take(&mut self.peers) {
            events.push(HostEvent::Left(peer, "the relay went".into()));
        }
        self.failure.get_or_insert(why);
    }
}

impl HostLink for RelayHost {
    fn poll(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        let Some(conn) = &mut self.conn else { return events };
        let (bodies, error) = conn.pump();
        for body in bodies {
            match RelayMsg::decode(&body) {
                Ok(RelayMsg::PeerJoined { peer }) => {
                    self.peers.insert(peer);
                    events.push(HostEvent::Joined(peer));
                }
                Ok(RelayMsg::PeerData { peer, payload }) if self.peers.contains(&peer) => match parse(&payload) {
                    Ok(msg) => events.push(HostEvent::Message(peer, msg)),
                    Err(e) => {
                        self.peers.remove(&peer);
                        events.push(HostEvent::Left(peer, e.to_string()));
                        if let Some(conn) = &mut self.conn {
                            conn.send(&RelayMsg::Kick { peer }.encode());
                        }
                    }
                },
                Ok(RelayMsg::PeerLeft { peer }) => {
                    if self.peers.remove(&peer) {
                        events.push(HostEvent::Left(peer, "left the room".into()));
                    }
                }
                Ok(RelayMsg::Error { reason }) => {
                    self.fail(reason, &mut events);
                    return events;
                }
                Ok(_) => {}
                Err(why) => {
                    self.fail(why, &mut events);
                    return events;
                }
            }
        }
        if let Some(why) = error {
            self.fail(format!("lost the relay: {why}"), &mut events);
        }
        events
    }

    fn send(&mut self, to: PeerId, msg: &Msg) {
        if let Some(conn) = &mut self.conn
            && self.peers.contains(&to)
        {
            conn.send(&RelayMsg::Send { to, payload: body(msg) }.encode());
        }
    }

    fn close(&mut self, peer: PeerId) {
        if self.peers.remove(&peer)
            && let Some(conn) = &mut self.conn
        {
            conn.send(&RelayMsg::Kick { peer }.encode());
        }
    }

    fn failure(&self) -> Option<String> {
        self.failure.clone()
    }
}

/// A player's end: in a room on the relay.
pub struct RelayClient {
    conn: Option<RawConn>,
}

impl RelayClient {
    /// Join the room with this code on the relay at `addr`. Blocks for up
    /// to `timeout`.
    pub fn join(addr: &str, code: &str, timeout: Duration) -> Result<RelayClient, String> {
        match handshake(addr, RelayMsg::Join { version: VERSION, code: code.into() }, timeout)? {
            (conn, RelayMsg::Joined) => Ok(RelayClient { conn: Some(conn) }),
            (_, other) => Err(format!("the relay answered {other:?}")),
        }
    }
}

impl ClientLink for RelayClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        let Some(conn) = &mut self.conn else { return events };
        let (bodies, error) = conn.pump();
        let mut closed = error;
        for body in bodies {
            match RelayMsg::decode(&body) {
                Ok(RelayMsg::Data { payload }) => match parse(&payload) {
                    Ok(msg) => events.push(ClientEvent::Message(msg)),
                    Err(e) => {
                        closed = Some(e.to_string());
                        break;
                    }
                },
                Ok(RelayMsg::Error { reason }) => {
                    closed = Some(reason);
                    break;
                }
                Ok(_) => {}
                Err(why) => {
                    closed = Some(why);
                    break;
                }
            }
        }
        if let Some(why) = closed {
            if let Some(mut conn) = self.conn.take() {
                conn.close();
            }
            events.push(ClientEvent::Closed(why));
        }
        events
    }

    fn send(&mut self, msg: &Msg) {
        if let Some(conn) = &mut self.conn {
            conn.send(&RelayMsg::Data { payload: body(msg) }.encode());
        }
    }

    fn close(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            conn.close();
        }
    }
}
