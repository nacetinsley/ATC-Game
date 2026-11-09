//! The co-op lobby, and a whole session through a real relay: players join,
//! pick positions, the host starts, and everyone plays the same shift.

use atcgame::net::link::{ClientLink, HostLink, LoopHost};
use atcgame::net::relay::{RelayClient, RelayHost};
use atcgame::net::session::{Client, HOST, Host, Joining, LobbyView, Notice, ShiftSetup, staffing_of};
use atcgame::sim::TICK;
use atcgame::sim::airspace::Airport;
use atcgame::sim::positions::{Position, Seat};
use atcgame::sim::traffic::Busyness;
use atcgame::sim::{World, WorldConfig};
use std::time::Duration;

fn airports() -> Vec<Airport> {
    vec![Airport::default_airport()]
}

/// The host starts the lobby's shift, as the game does.
fn start<L: HostLink>(host: &mut Host<L>) -> World {
    let config = WorldConfig { seed: 5, busyness: Busyness::VeryBusy, shift_seconds: 20.0 * 60.0, ..Default::default() };
    let staffing = staffing_of(&host.positions());
    let setup = ShiftSetup::new(&config, &airports()[0].code, staffing, 30_000.0);
    let world = setup.world(&airports()).unwrap();
    host.start(setup, &world);
    world
}

/// Run the lobby until `done` says so, or fail after a while.
fn lobby_until<L: HostLink, C: ClientLink>(
    host: &mut Host<L>,
    clients: &mut [Client<C>],
    mut done: impl FnMut(&Host<L>, &[Client<C>]) -> bool,
) {
    for _ in 0..400 {
        host.lobby_update(0.05);
        for client in clients.iter_mut() {
            assert!(matches!(client.join(0.05, &airports()), Joining::Waiting));
        }
        if done(host, clients) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the lobby never got there");
}

fn view<C: ClientLink>(client: &Client<C>) -> LobbyView {
    client.lobby().cloned().expect("in the lobby")
}

#[test]
fn players_pick_positions_and_the_host_starts() {
    let link = LoopHost::default();
    let connector = link.connector();
    let mut host = Host::lobby(link, "Sam", "XHGT, 20 minutes");
    let mut clients = vec![Client::new(connector.connect(), "Alex"), Client::new(connector.connect(), "Sam")];
    lobby_until(&mut host, &mut clients, |h, _| h.players().len() == 3);
    // A second Sam gets told apart.
    let names: Vec<String> = host.players().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["Sam", "Alex", "Sam 2"]);
    lobby_until(&mut host, &mut clients, |_, c| c.iter().all(|c| c.lobby().is_some_and(|l| l.players.len() == 3)));
    let (alex, sam2) = (view(&clients[0]).seat, view(&clients[1]).seat);
    assert_eq!((alex, sam2), (1, 2));
    assert_eq!(view(&clients[0]).about, "XHGT, 20 minutes");

    // Everyone needs a position before the shift can start.
    assert!(host.can_start().is_err());
    host.give(0, Some(HOST));
    clients[0].pick(1, true);
    clients[1].pick(1, true); // taken already: ignored
    clients[1].pick(2, true);
    clients[1].pick(3, true);
    lobby_until(&mut host, &mut clients, |h, _| h.positions() == [Some(0), Some(1), Some(2), Some(2)]);
    // Alex gives Local up; the host moves Delivery to Alex.
    clients[0].pick(1, false);
    lobby_until(&mut host, &mut clients, |h, _| h.positions()[1].is_none());
    host.give(3, Some(alex));
    assert!(host.can_start().is_ok());
    lobby_until(&mut host, &mut clients, |_, c| view(&c[0]).positions == [Some(0), None, Some(2), Some(1)]);

    let mut world = start(&mut host);
    assert!(world.staffing().seat_works(1, Position::Delivery));
    assert!(!world.staffing().local, "nobody took Local: it's automated");
    let mut theirs: Vec<(Client<_>, World, Seat)> = Vec::new();
    for mut client in clients {
        let mut ready = None;
        for _ in 0..50 {
            host.update(0.0, &mut world);
            if let Joining::Ready(w, seat) = client.join(0.0, &airports()) {
                ready = Some((*w, seat));
                break;
            }
        }
        let (w, seat) = ready.expect("the shift reached the player");
        theirs.push((client, w, seat));
    }
    assert_eq!(theirs[0].2, alex);
    for _ in 0..(4 * 60 * 4) {
        host.update(TICK, &mut world);
        for (client, w, _) in &mut theirs {
            client.update(TICK, w);
        }
    }
    for (client, w, _) in &theirs {
        assert_eq!(w.checksum(), world.checksum());
        // Each player's replay: the shift from its start, and the commands.
        let mut again = client.setup().unwrap().world(&airports()).unwrap();
        again.run_to(w.ticks(), w.commands());
        assert_eq!(again.checksum(), w.checksum());
    }
    // In the shift, names come in the roster.
    let names: Vec<String> = theirs[0].0.players().iter().map(|(_, n)| n.clone()).collect();
    assert_eq!(names, ["Sam", "Alex", "Sam 2"]);
}

#[test]
fn leaving_the_lobby_frees_the_seat_and_positions() {
    let link = LoopHost::default();
    let connector = link.connector();
    let mut host = Host::lobby(link, "Sam", "");
    let mut clients = vec![Client::new(connector.connect(), "Alex")];
    lobby_until(&mut host, &mut clients, |h, _| h.players().len() == 2);
    clients[0].pick(0, true);
    lobby_until(&mut host, &mut clients, |h, _| h.positions()[0] == Some(1));
    clients[0].leave();
    let mut notices = Vec::new();
    for _ in 0..5 {
        notices.extend(host.lobby_update(0.05));
    }
    assert!(notices.iter().any(|n| matches!(n, Notice::Left { seat: 1, .. })));
    assert_eq!(host.positions(), [None; 4]);
    assert_eq!(host.players().len(), 1);
}

#[test]
fn a_quiet_lobby_stays_open() {
    let link = LoopHost::default();
    let connector = link.connector();
    let mut host = Host::lobby(link, "Sam", "");
    let mut client = Client::new(connector.connect(), "Alex");
    // A minute with nothing happening: the host's lobby keeps both ends
    // hearing from each other.
    for _ in 0..(60 * 20) {
        host.lobby_update(0.05);
        assert!(matches!(client.join(0.05, &airports()), Joining::Waiting));
    }
    assert_eq!(host.players().len(), 2);
}

#[test]
fn a_whole_session_through_the_relay() {
    let relay = atc_relay::server::spawn("127.0.0.1:0", Default::default()).unwrap().to_string();
    let link = RelayHost::open(&relay, Duration::from_secs(5)).unwrap();
    let code = link.code().to_string();
    let mut host = Host::lobby(link, "Sam", "XHGT");
    let player = RelayClient::join(&relay, &code.to_lowercase().to_uppercase(), Duration::from_secs(5)).unwrap();
    let mut clients = vec![Client::new(player, "Alex")];
    lobby_until(&mut host, &mut clients, |_, c| c[0].lobby().is_some());
    host.give(0, Some(HOST));
    clients[0].pick(1, true);
    clients[0].pick(2, true);
    clients[0].pick(3, true);
    lobby_until(&mut host, &mut clients, |h, _| h.positions() == [Some(0), Some(1), Some(1), Some(1)]);

    let mut world = start(&mut host);
    let mut client = clients.remove(0);
    let mut joined = None;
    for _ in 0..500 {
        host.update(0.0, &mut world);
        match client.join(0.0, &airports()) {
            Joining::Ready(w, seat) => {
                joined = Some((*w, seat));
                break;
            }
            Joining::Failed(why) => panic!("{why}"),
            Joining::Waiting => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    let (mut theirs, seat) = joined.expect("joined through the relay");
    assert_eq!(seat, 1);

    // Four minutes, the player saying something now and then.
    for step in 0..(4 * 60 * 4) {
        if step % 40 == 0 {
            // Something the sim takes, tried on a copy as the game does.
            let nodes = theirs.airport.surface.as_ref().map_or(0, |m| m.nodes.len());
            let mine: Vec<u32> = theirs.aircraft.iter().filter(|a| theirs.is_seats(seat, a)).map(|a| a.id).collect();
            let said = mine.iter().flat_map(|&id| (0..nodes).map(move |to| (id, to))).find_map(|(id, to)| {
                let action = atcgame::sim::replay::Action::Transmit(vec![
                    atcgame::sim::clearance::Clearance::Taxi { to, via: Vec::new() },
                ]);
                theirs.clone().act(seat, id, action.clone()).then_some((id, action))
            });
            if let Some((id, action)) = said {
                client.command(id, action);
            }
        }
        host.update(TICK, &mut world);
        for attempt in 0..300 {
            let notices = client.update(if attempt == 0 { TICK } else { 0.0 }, &mut theirs);
            assert!(notices.is_empty(), "{notices:?}");
            if theirs.ticks() == world.ticks() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(theirs.ticks(), world.ticks());
    assert_eq!(theirs.checksum(), world.checksum());
    assert!(world.commands().iter().any(|c| c.seat == 1), "the player's commands came through the relay");

    // The host leaves: the relay tells the player.
    host.close("the host left");
    drop(host);
    let mut gone = false;
    for _ in 0..500 {
        if client.update(0.01, &mut theirs).iter().any(|n| matches!(n, Notice::HostGone { .. })) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(gone);
}

#[test]
fn the_host_hears_when_the_relay_closes_the_room() {
    let limits = atc_relay::server::Limits { idle: Duration::from_millis(200), ..Default::default() };
    let relay = atc_relay::server::spawn("127.0.0.1:0", limits).unwrap().to_string();
    let link = RelayHost::open(&relay, Duration::from_secs(5)).unwrap();
    let mut host = Host::lobby(link, "Sam", "");
    let mut lost = None;
    for _ in 0..300 {
        for notice in host.lobby_update(0.01) {
            if let Notice::LinkLost { reason } = notice {
                lost = Some(reason);
            }
        }
        if lost.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(lost.as_deref(), Some("the room was idle too long"));
}

#[test]
fn a_room_that_isnt_there() {
    let relay = atc_relay::server::spawn("127.0.0.1:0", Default::default()).unwrap().to_string();
    let err = RelayClient::join(&relay, "ACDEFH", Duration::from_secs(5)).err().expect("no such room");
    assert!(err.contains("no room"), "{err}");
    let err = RelayHost::open("127.0.0.1:1", Duration::from_secs(2)).err().expect("nothing listening");
    assert!(err.contains("couldn't reach the relay"), "{err}");
}

#[test]
fn picks_go_over_a_real_connection() {
    use atcgame::net::tcp::{TcpClient, TcpHost};
    let link = TcpHost::listen("127.0.0.1:0").unwrap();
    let addr = link.local_addr().unwrap().to_string();
    let mut host = Host::lobby(link, "Sam", "");
    let mut clients = vec![Client::new(TcpClient::connect(&addr, Duration::from_secs(5)).unwrap(), "Alex")];
    lobby_until(&mut host, &mut clients, |_, c| c[0].lobby().is_some());
    clients[0].pick(1, true);
    clients[0].pick(2, true);
    lobby_until(&mut host, &mut clients, |h, _| h.positions() == [None, Some(1), Some(1), None]);
}
