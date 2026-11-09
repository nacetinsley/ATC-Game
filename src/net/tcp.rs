//! A direct TCP link: the host listens, and clients connect to it. Used by
//! the tests and for LAN play. Everything is non-blocking, so the game polls
//! it once a frame.

use super::link::{ClientEvent, ClientLink, HostEvent, HostLink, PeerId};
use super::protocol::{Msg, encode, parse};
use atc_relay::wire::FrameReader;
use std::collections::BTreeMap;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Unsent bytes a connection may pile up before it's cut off as too slow.
const MAX_BACKLOG: usize = 8 * 1024 * 1024;

/// One end of a TCP connection carrying frames, with what's waiting to go
/// out. The direct link and the relay link both use it.
pub(super) struct RawConn {
    pub stream: TcpStream,
    reader: FrameReader,
    out: Vec<u8>,
}

impl RawConn {
    pub fn new(stream: TcpStream) -> io::Result<RawConn> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(RawConn { stream, reader: FrameReader::default(), out: Vec::new() })
    }

    /// Queue a whole frame, and send what the socket will take now.
    pub fn send(&mut self, frame: &[u8]) {
        self.out.extend_from_slice(frame);
        // Failures show up at the next pump.
        let _ = self.flush();
    }

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

    fn read(&mut self, bodies: &mut Vec<Vec<u8>>) -> Result<(), String> {
        let mut buffer = [0u8; 16 * 1024];
        let closed = loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => break true,
                Ok(n) => self.reader.push(&buffer[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break false,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        };
        while let Some(body) = self.reader.next()? {
            bodies.push(body);
        }
        if closed { Err("the connection closed".into()) } else { Ok(()) }
    }

    /// Read, then send. Frames that arrived before a failure still count.
    pub fn pump(&mut self) -> (Vec<Vec<u8>>, Option<String>) {
        let mut bodies = Vec::new();
        let error = self.read(&mut bodies).err().or_else(|| self.flush().err());
        (bodies, error)
    }

    pub fn close(&mut self) {
        let _ = self.flush();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Connect, waiting at most `timeout`.
pub(super) fn connect(addr: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(ErrorKind::NotFound, format!("no address for {addr}"));
    for socket in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Messages from frame bodies; the first that doesn't make sense is an error.
fn messages(bodies: Vec<Vec<u8>>) -> (Vec<Msg>, Option<String>) {
    let mut msgs = Vec::new();
    for body in bodies {
        match parse(&body) {
            Ok(msg) => msgs.push(msg),
            Err(e) => return (msgs, Some(e.to_string())),
        }
    }
    (msgs, None)
}

/// The host's end: listening, with a connection per peer.
pub struct TcpHost {
    listener: TcpListener,
    peers: BTreeMap<PeerId, RawConn>,
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
        while let Ok((stream, _)) = self.listener.accept() {
            if let Ok(conn) = RawConn::new(stream) {
                let id = self.next;
                self.next += 1;
                self.peers.insert(id, conn);
                events.push(HostEvent::Joined(id));
            }
        }
        let mut gone = Vec::new();
        for (&id, conn) in &mut self.peers {
            let (bodies, error) = conn.pump();
            let (msgs, garbled) = messages(bodies);
            events.extend(msgs.into_iter().map(|m| HostEvent::Message(id, m)));
            if let Some(why) = garbled.or(error) {
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
            conn.send(&encode(msg));
        }
    }

    fn close(&mut self, peer: PeerId) {
        if let Some(mut conn) = self.peers.remove(&peer) {
            conn.close();
        }
    }
}

/// A client's end.
pub struct TcpClient {
    conn: Option<RawConn>,
}

impl TcpClient {
    /// Connect to a host, such as "192.168.1.20:7340". Blocks for up to
    /// `timeout`.
    pub fn connect(addr: &str, timeout: Duration) -> io::Result<TcpClient> {
        Ok(TcpClient { conn: Some(RawConn::new(connect(addr, timeout)?)?) })
    }
}

impl ClientLink for TcpClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        let Some(conn) = &mut self.conn else { return Vec::new() };
        let (bodies, error) = conn.pump();
        let (msgs, garbled) = messages(bodies);
        let mut events: Vec<ClientEvent> = msgs.into_iter().map(ClientEvent::Message).collect();
        if let Some(why) = garbled.or(error) {
            self.conn = None;
            events.push(ClientEvent::Closed(why));
        }
        events
    }

    fn send(&mut self, msg: &Msg) {
        if let Some(conn) = &mut self.conn {
            conn.send(&encode(msg));
        }
    }

    fn close(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            conn.close();
        }
    }
}
