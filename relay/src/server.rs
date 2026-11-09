//! The relay: hosts open rooms, players join them by code, and frames pass
//! between a room's host and its players, unread.
//!
//! Each connection has a thread reading it and a thread writing to it, so
//! one slow player holds up nobody else. The rooms are in one shared table.
//! A janitor closes rooms nobody has used for a while.

use crate::wire::{CODE_ALPHABET, CODE_LEN, FrameReader, MAX_PAYLOAD, RelayMsg, VERSION};
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// What the relay allows, as it faces the internet.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Connections at once, from everyone.
    pub max_conns: usize,
    /// Connections at once from one address.
    pub conns_per_ip: usize,
    /// Rooms open at once from one address.
    pub rooms_per_ip: usize,
    /// The host and its players.
    pub players_per_room: usize,
    /// A room nothing has passed through for this long is closed.
    pub idle: Duration,
    /// A new connection has this long to open or join a room.
    pub hello: Duration,
    /// Bytes a second a connection may send, and in a burst.
    pub bytes_per_s: f64,
    pub burst_bytes: f64,
    /// Frames a second a connection may send, and in a burst.
    pub frames_per_s: f64,
    pub burst_frames: f64,
    /// Frames waiting to go out to a connection before it's cut off as too
    /// slow.
    pub backlog: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_conns: 512,
            conns_per_ip: 16,
            rooms_per_ip: 4,
            players_per_room: 4,
            idle: Duration::from_secs(10 * 60),
            hello: Duration::from_secs(10),
            // A shift's ticks are tiny; catching a late joiner up is the
            // biggest burst, a few hundred kilobytes.
            bytes_per_s: 256.0 * 1024.0,
            burst_bytes: 4.0 * 1024.0 * 1024.0,
            frames_per_s: 200.0,
            burst_frames: 1000.0,
            backlog: 4096,
        }
    }
}

type ConnId = u64;

#[derive(Debug, Clone, PartialEq)]
enum Role {
    New,
    Host(String),
    Peer { code: String, peer: u32 },
}

struct Conn {
    ip: IpAddr,
    /// Frames for the writer thread. An empty frame says: send what's
    /// queued, then hang up.
    out: SyncSender<Vec<u8>>,
    stream: TcpStream,
    role: Role,
}

struct Room {
    host: ConnId,
    peers: BTreeMap<u32, ConnId>,
    next_peer: u32,
    last: Instant,
}

struct State {
    conns: HashMap<ConnId, Conn>,
    rooms: HashMap<String, Room>,
    next: ConnId,
    random: u64,
}

impl State {
    fn send(&self, to: ConnId, msg: &RelayMsg) {
        if let Some(conn) = self.conns.get(&to)
            && conn.out.try_send(msg.encode()).is_err()
        {
            // Too far behind, or gone: cut it off.
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }

    /// Send what's queued, then hang up.
    fn close(&self, id: ConnId) {
        if let Some(conn) = self.conns.get(&id)
            && conn.out.try_send(Vec::new()).is_err()
        {
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }

    fn hang_up(&self, id: ConnId) {
        if let Some(conn) = self.conns.get(&id) {
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }

    fn conns_from(&self, ip: IpAddr) -> usize {
        self.conns.values().filter(|c| c.ip == ip).count()
    }

    fn rooms_from(&self, ip: IpAddr) -> usize {
        self.rooms.values().filter(|r| self.conns.get(&r.host).is_some_and(|c| c.ip == ip)).count()
    }

    /// A code no open room has.
    fn new_code(&mut self) -> String {
        loop {
            let mut code = String::with_capacity(CODE_LEN);
            for _ in 0..CODE_LEN {
                // xorshift64*
                self.random ^= self.random >> 12;
                self.random ^= self.random << 25;
                self.random ^= self.random >> 27;
                let n = self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32;
                code.push(CODE_ALPHABET[(n % CODE_ALPHABET.len() as u64) as usize] as char);
            }
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }
}

/// A relay, serving on its own threads.
pub struct Relay {
    state: Arc<Mutex<State>>,
    limits: Limits,
}

impl Relay {
    fn new(limits: Limits) -> Relay {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        let state = State { conns: HashMap::new(), rooms: HashMap::new(), next: 1, random: hasher.finish() | 1 };
        Relay { state: Arc::new(Mutex::new(state)), limits }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Serve on `listener` until the process ends.
pub fn serve(listener: TcpListener, limits: Limits) {
    let relay = Arc::new(Relay::new(limits));
    {
        let relay = relay.clone();
        thread::spawn(move || janitor(&relay));
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let relay = relay.clone();
        thread::spawn(move || accept(&relay, stream));
    }
}

/// Serve on a new thread, as the tests and a local game do. Returns the
/// address it's listening on.
pub fn spawn(addr: impl ToSocketAddrs, limits: Limits) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    thread::spawn(move || serve(listener, limits));
    Ok(local)
}

/// Close rooms that have gone idle.
fn janitor(relay: &Relay) {
    let every = (relay.limits.idle / 4).clamp(Duration::from_millis(20), Duration::from_secs(15));
    loop {
        thread::sleep(every);
        let state = relay.lock();
        for room in state.rooms.values() {
            if room.last.elapsed() >= relay.limits.idle {
                state.send(room.host, &RelayMsg::Error { reason: "the room was idle too long".into() });
                state.close(room.host);
            }
        }
    }
}

fn accept(relay: &Relay, mut stream: TcpStream) {
    let Ok(peer_addr) = stream.peer_addr() else { return };
    let ip = peer_addr.ip();
    let _ = stream.set_nodelay(true);
    let refuse = |stream: &mut TcpStream, reason: &str| {
        let _ = stream.write_all(&RelayMsg::Error { reason: reason.into() }.encode());
        let _ = stream.shutdown(Shutdown::Both);
    };
    let (id, rx) = {
        let mut state = relay.lock();
        if state.conns.len() >= relay.limits.max_conns {
            drop(state);
            return refuse(&mut stream, "the relay is full");
        }
        if state.conns_from(ip) >= relay.limits.conns_per_ip {
            drop(state);
            return refuse(&mut stream, "too many connections from your address");
        }
        let Ok(writer) = stream.try_clone() else { return };
        let (out, rx) = sync_channel(relay.limits.backlog);
        let id = state.next;
        state.next += 1;
        state.conns.insert(id, Conn { ip, out, stream: writer, role: Role::New });
        (id, rx)
    };
    if let Ok(writer) = stream.try_clone() {
        thread::spawn(move || write_loop(writer, rx));
    }
    let why = read_loop(relay, id, &mut stream);
    if let Some(why) = why {
        let state = relay.lock();
        state.send(id, &RelayMsg::Error { reason: why });
    }
    cleanup(relay, id);
}

fn write_loop(mut stream: TcpStream, rx: Receiver<Vec<u8>>) {
    for frame in rx {
        if frame.is_empty() || stream.write_all(&frame).is_err() {
            break;
        }
    }
    let _ = stream.flush();
    // Close gently: say we're done, then let the other end's last bytes
    // drain for a moment. Closing with unread bytes waiting resets the
    // connection, and the reset can overtake the last message we sent.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let started = Instant::now();
    let mut sink = [0u8; 4096];
    while started.elapsed() < Duration::from_secs(1) {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Token buckets for what a connection sends.
struct Allowance {
    bytes: f64,
    frames: f64,
    last: Instant,
}

impl Allowance {
    fn new(limits: &Limits) -> Allowance {
        Allowance { bytes: limits.burst_bytes, frames: limits.burst_frames, last: Instant::now() }
    }

    fn refill(&mut self, limits: &Limits) {
        let dt = self.last.elapsed().as_secs_f64();
        self.last = Instant::now();
        self.bytes = (self.bytes + dt * limits.bytes_per_s).min(limits.burst_bytes);
        self.frames = (self.frames + dt * limits.frames_per_s).min(limits.burst_frames);
    }
}

/// Read and act on what the connection sends, until it goes or breaks a
/// rule. Returns why it was cut off, if it was.
fn read_loop(relay: &Relay, id: ConnId, stream: &mut TcpStream) -> Option<String> {
    let limits = &relay.limits;
    let _ = stream.set_read_timeout(Some(limits.hello));
    let mut reader = FrameReader::default();
    let mut allowance = Allowance::new(limits);
    let mut buffer = vec![0u8; 16 * 1024];
    let mut greeted = false;
    loop {
        let n = match stream.read(&mut buffer) {
            Ok(0) => return None,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if !greeted {
                    return Some("no room was opened or joined in time".into());
                }
                continue;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        allowance.refill(limits);
        allowance.bytes -= n as f64;
        if allowance.bytes < 0.0 {
            return Some("sending too fast".into());
        }
        reader.push(&buffer[..n]);
        loop {
            let body = match reader.next() {
                Ok(Some(body)) => body,
                Ok(None) => break,
                Err(why) => return Some(why),
            };
            allowance.frames -= 1.0;
            if allowance.frames < 0.0 {
                return Some("sending too fast".into());
            }
            let msg = match RelayMsg::decode(&body) {
                Ok(msg) => msg,
                Err(why) => return Some(why),
            };
            if let Err(why) = act(relay, id, msg) {
                return Some(why);
            }
            if !greeted {
                greeted = true;
                // In a room: quiet spells are fine now. The janitor watches
                // the room instead.
                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
            }
        }
    }
}

/// One message from a connection.
fn act(relay: &Relay, id: ConnId, msg: RelayMsg) -> Result<(), String> {
    let limits = &relay.limits;
    let mut state = relay.lock();
    let Some(conn) = state.conns.get(&id) else { return Err("gone".into()) };
    let (role, ip) = (conn.role.clone(), conn.ip);
    match (role, msg) {
        (Role::New, RelayMsg::Open { version }) => {
            if version != VERSION {
                return Err("the relay is a different version".into());
            }
            if state.rooms_from(ip) >= limits.rooms_per_ip {
                return Err("too many rooms open from your address".into());
            }
            let code = state.new_code();
            state.rooms.insert(code.clone(), Room { host: id, peers: BTreeMap::new(), next_peer: 1, last: Instant::now() });
            state.conns.get_mut(&id).expect("checked").role = Role::Host(code.clone());
            state.send(id, &RelayMsg::Opened { code });
        }
        (Role::New, RelayMsg::Join { version, code }) => {
            if version != VERSION {
                return Err("the relay is a different version".into());
            }
            let Some(room) = state.rooms.get_mut(&code) else {
                return Err(format!("there's no room {code}"));
            };
            if room.peers.len() + 1 >= limits.players_per_room {
                return Err("the room is full".into());
            }
            let peer = room.next_peer;
            room.next_peer += 1;
            room.peers.insert(peer, id);
            room.last = Instant::now();
            let host = room.host;
            state.conns.get_mut(&id).expect("checked").role = Role::Peer { code, peer };
            state.send(id, &RelayMsg::Joined);
            state.send(host, &RelayMsg::PeerJoined { peer });
        }
        (Role::Host(code), RelayMsg::Send { to, payload }) => {
            if payload.len() > MAX_PAYLOAD {
                return Err("a message too big".into());
            }
            let room = state.rooms.get_mut(&code).ok_or("the room has closed")?;
            room.last = Instant::now();
            if let Some(&peer) = room.peers.get(&to) {
                state.send(peer, &RelayMsg::Data { payload });
            }
        }
        (Role::Host(code), RelayMsg::Kick { peer }) => {
            let room = state.rooms.get_mut(&code).ok_or("the room has closed")?;
            if let Some(gone) = room.peers.remove(&peer) {
                state.hang_up(gone);
            }
        }
        (Role::Peer { code, peer }, RelayMsg::Data { payload }) => {
            if payload.len() > MAX_PAYLOAD {
                return Err("a message too big".into());
            }
            let room = state.rooms.get_mut(&code).ok_or("the room has closed")?;
            room.last = Instant::now();
            let host = room.host;
            state.send(host, &RelayMsg::PeerData { peer, payload });
        }
        _ => return Err("that message isn't allowed here".into()),
    }
    Ok(())
}

/// A connection has gone: tidy up its room.
fn cleanup(relay: &Relay, id: ConnId) {
    let mut state = relay.lock();
    let Some(conn) = state.conns.get(&id) else { return };
    match conn.role.clone() {
        Role::New => {}
        Role::Host(code) => {
            if let Some(room) = state.rooms.remove(&code) {
                for &peer in room.peers.values() {
                    state.send(peer, &RelayMsg::Error { reason: "the host has left".into() });
                    state.close(peer);
                }
            }
        }
        Role::Peer { code, peer } => {
            if let Some(room) = state.rooms.get_mut(&code)
                && room.peers.get(&peer) == Some(&id)
            {
                room.peers.remove(&peer);
                let host = room.host;
                state.send(host, &RelayMsg::PeerLeft { peer });
            }
        }
    }
    // Dropping its sender lets the writer send what's left, then hang up.
    state.close(id);
    state.conns.remove(&id);
}
