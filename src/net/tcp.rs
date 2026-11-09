//! A direct TCP link: the host listens, and clients connect to it. Used by
//! the tests and for LAN play. Everything is non-blocking, so the game polls
//! it once a frame.

use super::link::{ClientEvent, ClientLink, HostEvent, HostLink, PeerId};
use super::protocol::{Decoder, Msg, encode};
use std::collections::BTreeMap;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Unsent bytes a connection may pile up before it's cut off as too slow.
const MAX_BACKLOG: usize = 8 * 1024 * 1024;

/// One end of a TCP connection, with what's waiting to go out.
struct Conn {
    stream: TcpStream,
    decoder: Decoder,
    out: Vec<u8>,
}

impl Conn {
    fn new(stream: TcpStream) -> io::Result<Conn> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Conn { stream, decoder: Decoder::default(), out: Vec::new() })
    }

    fn queue(&mut self, msg: &Msg) {
        self.out.extend_from_slice(&encode(msg));
    }

    /// Send what the socket will take now.
    fn flush(&mut self) -> Result<(), String> {
        while !self.out.is_empty() {
            match self.stream.write(&self.out) {
                Ok(0) => return Err("the connection closed".into()),
                Ok(n) => {
                    self.out.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if self.out.len() > MAX_BACKLOG {
            return Err("the connection fell too far behind".into());
        }
        Ok(())
    }

    /// Every whole message that has arrived.
    fn read(&mut self) -> Result<Vec<Msg>, String> {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err("the connection closed".into()),
                Ok(n) => self.decoder.push(&buffer[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut msgs = Vec::new();
        while let Some(msg) = self.decoder.next().map_err(|e| e.to_string())? {
            msgs.push(msg);
        }
        Ok(msgs)
    }

    /// Read, then send. Messages that arrived before a failure still count.
    fn pump(&mut self) -> (Vec<Msg>, Option<String>) {
        let (msgs, read_error) = match self.read() {
            Ok(msgs) => (msgs, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let error = read_error.or_else(|| self.flush().err());
        (msgs, error)
    }
}

/// The host's end: listening, with a connection per peer.
pub struct TcpHost {
    listener: TcpListener,
    peers: BTreeMap<PeerId, Conn>,
    next: PeerId,
}

impl TcpHost {
    /// Listen on `addr`, such as "0.0.0.0:7340", or port 0 for any free port.
    pub fn listen(addr: impl ToSocketAddrs) -> io::Result<TcpHost> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(TcpHost { listener, peers: BTreeMap::new(), next: 1 })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl HostLink for TcpHost {
    fn poll(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Ok(conn) = Conn::new(stream) {
                        let id = self.next;
                        self.next += 1;
                        self.peers.insert(id, conn);
                        events.push(HostEvent::Joined(id));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        let mut gone = Vec::new();
        for (&id, conn) in &mut self.peers {
            let (msgs, error) = conn.pump();
            events.extend(msgs.into_iter().map(|m| HostEvent::Message(id, m)));
            if let Some(why) = error {
                gone.push(id);
                events.push(HostEvent::Left(id, why));
            }
        }
        for id in gone {
            self.peers.remove(&id);
        }
        events
    }

    fn send(&mut self, to: PeerId, msg: &Msg) {
        if let Some(conn) = self.peers.get_mut(&to) {
            conn.queue(msg);
            // Failures show up at the next poll.
            let _ = conn.flush();
        }
    }

    fn close(&mut self, peer: PeerId) {
        if let Some(mut conn) = self.peers.remove(&peer) {
            let _ = conn.flush();
            let _ = conn.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A client's end.
pub struct TcpClient {
    conn: Option<Conn>,
}

impl TcpClient {
    /// Connect to a host, such as "192.168.1.20:7340". Blocks for up to
    /// `timeout`.
    pub fn connect(addr: &str, timeout: Duration) -> io::Result<TcpClient> {
        let mut last = io::Error::new(ErrorKind::NotFound, format!("no address for {addr}"));
        for socket in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&socket, timeout) {
                Ok(stream) => return Ok(TcpClient { conn: Some(Conn::new(stream)?) }),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

impl ClientLink for TcpClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        let Some(conn) = &mut self.conn else { return Vec::new() };
        let (msgs, error) = conn.pump();
        let mut events: Vec<ClientEvent> = msgs.into_iter().map(ClientEvent::Message).collect();
        if let Some(why) = error {
            self.conn = None;
            events.push(ClientEvent::Closed(why));
        }
        events
    }

    fn send(&mut self, msg: &Msg) {
        if let Some(conn) = &mut self.conn {
            conn.queue(msg);
            let _ = conn.flush();
        }
    }

    fn close(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            let _ = conn.flush();
            let _ = conn.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}
