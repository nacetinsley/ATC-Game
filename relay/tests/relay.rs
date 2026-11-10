//! The relay over real sockets: rooms, passing frames, and the limits.

use atc_relay::server::{Limits, spawn};
use atc_relay::wire::{RelayMsg, VERSION, read_frame};
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn relay(limits: Limits) -> SocketAddr {
    spawn("127.0.0.1:0", limits).unwrap()
}

struct Client(TcpStream);

impl Client {
    fn connect(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Client(stream)
    }

    fn send(&mut self, msg: RelayMsg) {
        let _ = self.0.write_all(&msg.encode());
    }

    fn recv(&mut self) -> RelayMsg {
        RelayMsg::decode(&read_frame(&mut self.0).expect("a message")).unwrap()
    }

    /// The relay has hung up (after any last messages).
    fn closed(&mut self) -> bool {
        loop {
            match read_frame(&mut self.0) {
                Ok(_) => continue,
                Err(e) => return !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            }
        }
    }

    fn open(addr: SocketAddr) -> (Client, String) {
        let (host, code, _) = Client::open_keyed(addr);
        (host, code)
    }

    /// A room, and its key.
    fn open_keyed(addr: SocketAddr) -> (Client, String, String) {
        let mut host = Client::connect(addr);
        host.send(RelayMsg::Open { version: VERSION });
        match host.recv() {
            RelayMsg::Opened { code, key } => (host, code, key),
            other => panic!("{other:?}"),
        }
    }

    /// Try for a room, and get the relay's answer.
    fn ask(addr: SocketAddr, msg: RelayMsg) -> RelayMsg {
        let mut client = Client::connect(addr);
        client.send(msg);
        client.recv()
    }

    fn join(addr: SocketAddr, code: &str) -> Client {
        let mut peer = Client::connect(addr);
        peer.send(RelayMsg::Join { version: VERSION, code: code.into() });
        assert_eq!(peer.recv(), RelayMsg::Joined);
        peer
    }
}

#[test]
fn frames_pass_between_host_and_players() {
    let addr = relay(Limits::default());
    let (mut host, code) = Client::open(addr);
    assert_eq!(code.len(), 6);
    let mut a = Client::join(addr, &code);
    assert_eq!(host.recv(), RelayMsg::PeerJoined { peer: 1 });
    let mut b = Client::join(addr, &code);
    assert_eq!(host.recv(), RelayMsg::PeerJoined { peer: 2 });

    a.send(RelayMsg::Data { payload: b"from a".to_vec() });
    assert_eq!(host.recv(), RelayMsg::PeerData { peer: 1, payload: b"from a".to_vec() });
    host.send(RelayMsg::Send { to: 2, payload: b"to b".to_vec() });
    assert_eq!(b.recv(), RelayMsg::Data { payload: b"to b".to_vec() });

    // A leaves; the host hears.
    drop(a);
    assert_eq!(host.recv(), RelayMsg::PeerLeft { peer: 1 });
    // The host kicks B.
    host.send(RelayMsg::Kick { peer: 2 });
    assert!(b.closed());
}

#[test]
fn a_room_waits_for_its_host_to_come_back() {
    let addr = relay(Limits::default());
    let (host, code, key) = Client::open_keyed(addr);
    assert_eq!(key.len(), atc_relay::wire::KEY_LEN);
    let mut a = Client::join(addr, &code);
    drop(host);
    // The players are hung up on; their games call back.
    assert_eq!(a.recv(), RelayMsg::Error { reason: "the host's connection went".into() });
    assert!(a.closed());
    let join = || RelayMsg::Join { version: VERSION, code: code.clone() };
    assert!(matches!(Client::ask(addr, join()), RelayMsg::Error { reason } if reason.contains("reconnecting")));
    // Only the key reopens it.
    let wrong = RelayMsg::Reopen { version: VERSION, code: code.clone(), key: "ACDEFHJKMNPRTUVW".into() };
    assert!(matches!(Client::ask(addr, wrong), RelayMsg::Error { reason } if reason.contains("key")));
    let mut host = Client::connect(addr);
    host.send(RelayMsg::Reopen { version: VERSION, code: code.clone(), key: key.clone() });
    assert_eq!(host.recv(), RelayMsg::Opened { code: code.clone(), key: key.clone() });
    // A second host can't take it.
    let again = RelayMsg::Reopen { version: VERSION, code: code.clone(), key };
    assert!(matches!(Client::ask(addr, again), RelayMsg::Error { reason } if reason.contains("has a host")));
    // The player gets back in, under a new number.
    let mut a = Client::join(addr, &code);
    assert_eq!(host.recv(), RelayMsg::PeerJoined { peer: 2 });
    a.send(RelayMsg::Data { payload: b"back".to_vec() });
    assert_eq!(host.recv(), RelayMsg::PeerData { peer: 2, payload: b"back".to_vec() });
}

#[test]
fn a_room_whose_host_never_comes_back_closes() {
    let addr = relay(Limits { host_grace: Duration::from_millis(200), ..Limits::default() });
    let (host, code, key) = Client::open_keyed(addr);
    drop(host);
    std::thread::sleep(Duration::from_millis(700));
    let join = RelayMsg::Join { version: VERSION, code: code.clone() };
    assert!(matches!(Client::ask(addr, join), RelayMsg::Error { reason } if reason.contains("no room")));
    let reopen = RelayMsg::Reopen { version: VERSION, code, key };
    assert!(matches!(Client::ask(addr, reopen), RelayMsg::Error { reason } if reason.contains("no room")));
}

#[test]
fn with_no_grace_the_host_leaving_closes_the_room() {
    let addr = relay(Limits { host_grace: Duration::ZERO, ..Limits::default() });
    let (host, code) = Client::open(addr);
    let mut a = Client::join(addr, &code);
    drop(host);
    assert_eq!(a.recv(), RelayMsg::Error { reason: "the host has left".into() });
    assert!(a.closed());
    let mut late = Client::connect(addr);
    late.send(RelayMsg::Join { version: VERSION, code: code.clone() });
    assert!(matches!(late.recv(), RelayMsg::Error { reason } if reason.contains("no room")));
}

#[test]
fn rooms_hold_four() {
    let addr = relay(Limits::default());
    let (_host, code) = Client::open(addr);
    let _players: Vec<Client> = (0..3).map(|_| Client::join(addr, &code)).collect();
    let mut fifth = Client::connect(addr);
    fifth.send(RelayMsg::Join { version: VERSION, code });
    assert!(matches!(fifth.recv(), RelayMsg::Error { reason } if reason.contains("full")));
}

#[test]
fn one_address_opens_only_a_few_rooms() {
    let addr = relay(Limits { rooms_per_ip: 2, ..Limits::default() });
    let _one = Client::open(addr);
    let _two = Client::open(addr);
    let mut three = Client::connect(addr);
    three.send(RelayMsg::Open { version: VERSION });
    assert!(matches!(three.recv(), RelayMsg::Error { reason } if reason.contains("too many rooms")));
}

#[test]
fn one_address_makes_only_a_few_connections() {
    let addr = relay(Limits { conns_per_ip: 2, ..Limits::default() });
    let _a = Client::connect(addr);
    let _b = Client::connect(addr);
    std::thread::sleep(Duration::from_millis(100));
    let mut c = Client::connect(addr);
    assert!(matches!(c.recv(), RelayMsg::Error { reason } if reason.contains("too many connections")));
}

#[test]
fn a_frame_too_big_is_cut_off() {
    let addr = relay(Limits::default());
    let (mut host, _) = Client::open(addr);
    host.0.write_all(&(1_000_000u32).to_le_bytes()).unwrap();
    assert!(matches!(host.recv(), RelayMsg::Error { reason } if reason.contains("too big")));
    assert!(host.closed());
}

#[test]
fn sending_too_fast_is_cut_off() {
    let addr = relay(Limits { frames_per_s: 10.0, burst_frames: 20.0, ..Limits::default() });
    let (mut host, code) = Client::open(addr);
    let mut a = Client::join(addr, &code);
    for _ in 0..50 {
        a.send(RelayMsg::Data { payload: vec![1] });
    }
    assert!(matches!(a.recv(), RelayMsg::Error { reason } if reason.contains("too fast")));
    assert!(a.closed());
    // The host heard some of it, then that A had gone.
    loop {
        match host.recv() {
            RelayMsg::PeerJoined { .. } | RelayMsg::PeerData { .. } => {}
            RelayMsg::PeerLeft { peer: 1 } => break,
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn an_idle_room_is_closed() {
    let addr = relay(Limits { idle: Duration::from_millis(300), ..Limits::default() });
    let (mut host, code) = Client::open(addr);
    let mut a = Client::join(addr, &code);
    assert!(matches!(host.recv(), RelayMsg::PeerJoined { .. }));
    assert_eq!(host.recv(), RelayMsg::Error { reason: "the room was idle too long".into() });
    assert!(host.closed());
    assert_eq!(a.recv(), RelayMsg::Error { reason: "the room was idle too long".into() });
    // Gone for good, not waiting for its host.
    let join = RelayMsg::Join { version: VERSION, code };
    assert!(matches!(Client::ask(addr, join), RelayMsg::Error { reason } if reason.contains("no room")));
}

#[test]
fn a_connection_that_never_says_hello_is_closed() {
    let addr = relay(Limits { hello: Duration::from_millis(200), ..Limits::default() });
    let mut quiet = Client::connect(addr);
    assert!(matches!(quiet.recv(), RelayMsg::Error { reason } if reason.contains("in time")));
}

#[test]
fn a_player_cant_act_as_host() {
    let addr = relay(Limits::default());
    let (_host, code) = Client::open(addr);
    let mut a = Client::join(addr, &code);
    a.send(RelayMsg::Kick { peer: 1 });
    assert!(matches!(a.recv(), RelayMsg::Error { reason } if reason.contains("isn't allowed")));
    let mut b = Client::connect(addr);
    b.send(RelayMsg::Data { payload: vec![] });
    assert!(matches!(b.recv(), RelayMsg::Error { .. }));
}
