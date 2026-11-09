//! The transport, as the sessions see it. The host talks to any number of
//! peers; a client to the host. Direct TCP, the relay and the in-memory
//! loopback here all carry the same frames.

use super::protocol::{Decoder, Msg, encode};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// A connection to the host, numbered by the link.
pub type PeerId = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Joined(PeerId),
    Message(PeerId, Msg),
    /// Gone: closed, cut off, or sent nonsense.
    Left(PeerId, String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    Message(Msg),
    /// The connection to the host is gone.
    Closed(String),
}

pub trait HostLink {
    /// What has happened since the last poll, in order.
    fn poll(&mut self) -> Vec<HostEvent>;
    fn send(&mut self, to: PeerId, msg: &Msg);
    /// Hang up on a peer. No `Left` follows for it.
    fn close(&mut self, peer: PeerId);
}

pub trait ClientLink {
    fn poll(&mut self) -> Vec<ClientEvent>;
    fn send(&mut self, msg: &Msg);
    fn close(&mut self);
}

/// Frames between the loopback ends; `None` hangs up.
type Wire = Option<Vec<u8>>;

/// The host's end of an in-memory link. Its `connector` makes clients.
pub struct LoopHost {
    inbox: Receiver<(PeerId, Wire)>,
    joins: Receiver<(PeerId, Sender<Wire>)>,
    connector: LoopConnector,
    peers: BTreeMap<PeerId, (Sender<Wire>, Decoder)>,
}

impl Default for LoopHost {
    fn default() -> Self {
        let (to_inbox, inbox) = channel();
        let (to_joins, joins) = channel();
        let connector = LoopConnector { to_inbox, to_joins, next: Arc::new(AtomicU32::new(1)) };
        LoopHost { inbox, joins, connector, peers: BTreeMap::new() }
    }
}

impl LoopHost {
    /// Something to connect clients with, kept apart from the host.
    pub fn connector(&self) -> LoopConnector {
        self.connector.clone()
    }
}

/// Connects in-memory clients to a `LoopHost`.
#[derive(Clone)]
pub struct LoopConnector {
    to_inbox: Sender<(PeerId, Wire)>,
    to_joins: Sender<(PeerId, Sender<Wire>)>,
    next: Arc<AtomicU32>,
}

impl LoopConnector {
    /// A new client, connected.
    pub fn connect(&self) -> LoopClient {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (to_client, inbox) = channel();
        let _ = self.to_joins.send((id, to_client));
        LoopClient { id, to_host: self.to_inbox.clone(), inbox, decoder: Decoder::default(), closed: false }
    }
}

impl HostLink for LoopHost {
    fn poll(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        while let Ok((peer, to_client)) = self.joins.try_recv() {
            self.peers.insert(peer, (to_client, Decoder::default()));
            events.push(HostEvent::Joined(peer));
        }
        while let Ok((peer, wire)) = self.inbox.try_recv() {
            let Some((_, decoder)) = self.peers.get_mut(&peer) else { continue };
            let Some(bytes) = wire else {
                self.peers.remove(&peer);
                events.push(HostEvent::Left(peer, "hung up".into()));
                continue;
            };
            decoder.push(&bytes);
            loop {
                match decoder.next() {
                    Ok(Some(msg)) => events.push(HostEvent::Message(peer, msg)),
                    Ok(None) => break,
                    Err(e) => {
                        self.peers.remove(&peer);
                        events.push(HostEvent::Left(peer, e.to_string()));
                        break;
                    }
                }
            }
        }
        events
    }

    fn send(&mut self, to: PeerId, msg: &Msg) {
        if let Some((sender, _)) = self.peers.get(&to) {
            let _ = sender.send(Some(encode(msg)));
        }
    }

    fn close(&mut self, peer: PeerId) {
        if let Some((sender, _)) = self.peers.remove(&peer) {
            let _ = sender.send(None);
        }
    }
}

/// A client's end of an in-memory link.
pub struct LoopClient {
    id: PeerId,
    to_host: Sender<(PeerId, Wire)>,
    inbox: Receiver<Wire>,
    decoder: Decoder,
    closed: bool,
}

impl ClientLink for LoopClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        while !self.closed {
            let Ok(wire) = self.inbox.try_recv() else { break };
            let Some(bytes) = wire else {
                self.closed = true;
                events.push(ClientEvent::Closed("the host hung up".into()));
                break;
            };
            self.decoder.push(&bytes);
            loop {
                match self.decoder.next() {
                    Ok(Some(msg)) => events.push(ClientEvent::Message(msg)),
                    Ok(None) => break,
                    Err(e) => {
                        self.closed = true;
                        events.push(ClientEvent::Closed(e.to_string()));
                        break;
                    }
                }
            }
        }
        events
    }

    fn send(&mut self, msg: &Msg) {
        if !self.closed {
            let _ = self.to_host.send((self.id, Some(encode(msg))));
        }
    }

    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.to_host.send((self.id, None));
        }
    }
}

impl Drop for LoopClient {
    fn drop(&mut self) {
        self.close();
    }
}
