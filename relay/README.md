# atc-relay

The co-op relay. It lets players reach each other over the internet without
anyone opening ports on their router. The host opens a room and gets a
six-letter code. Players join with the code, and the relay passes the
game's messages between them. It knows nothing about the game itself: it
only forwards frames, so a new game version never needs a new relay.

It's meant to run on a Linux server. It uses only the Rust standard
library, with no system libraries beyond libc, and very little memory and
CPU. A small VPS can carry hundreds of players. It runs on Windows and
macOS too, which is handy for testing.

## Limits

These are built in, and set in `Limits` in `src/server.rs`:

| Limit | Default |
| --- | --- |
| Connections in all | 512 |
| Connections from one IP | 16 |
| Rooms open from one IP | 4 |
| Players in a room, host included | 4 |
| Largest message | 64 KB |
| Traffic from one connection | 256 KB/s, with bursts up to 4 MB |
| Messages from one connection | 200/s, with bursts up to 1000 |
| A room with no traffic closes after | 10 minutes |
| A connection must say hello within | 10 seconds |

A connection that breaks a limit is told why and closed. Room codes use
only letters that can't be mistaken for each other (no O and 0, no I and
1, and so on). Codes are case-insensitive.

## Running it

```sh
cargo run --release                         # listens on 0.0.0.0:7341
cargo run --release -- --listen 0.0.0.0:9000
RELAY_LISTEN=0.0.0.0:9000 cargo run --release
```

It logs to stderr and runs until stopped. In the game, set **Co-op →
Relay** to the server's address, such as `relay.example.com` or
`203.0.113.5:7341`. The port defaults to 7341.

Each connection holds three file handles. Linux shells often allow only
1024 open files, so running it by hand that way caps it at about 340
connections. Run `ulimit -n 4096` first to allow the full 512. The systemd
unit below sets this, and Docker's default limit is already high enough.

## Deploying

### Docker

From this folder:

```sh
docker build -t atc-relay .
docker run -d --name atc-relay --restart unless-stopped -p 7341:7341 atc-relay
```

The image runs as `nobody` and holds nothing but the binary.

### A plain Linux server (systemd)

Build on the server, or cross-build:

```sh
cargo build --release
sudo cp target/release/atc-relay /usr/local/bin/
```

Then add `/etc/systemd/system/atc-relay.service`:

```ini
[Unit]
Description=Air Traffic Control co-op relay
After=network-online.target

[Service]
ExecStart=/usr/local/bin/atc-relay --listen 0.0.0.0:7341
Restart=always
DynamicUser=yes
# Three file handles per connection, and 512 connections at most.
LimitNOFILE=4096

[Install]
WantedBy=multi-user.target
```

Then enable and start it:

```sh
sudo systemctl enable --now atc-relay
journalctl -u atc-relay -f     # watch the log
```

### Hosted platforms (fly.io and similar)

The relay is a plain TCP service, not HTTP. On fly.io, use the Dockerfile
with a `[[services]]` block that has `protocol = "tcp"` and
`internal_port = 7341`, with no HTTP handlers. It needs a dedicated IPv4
address. Platforms that only route HTTP(S), as many free tiers do, won't
work.

### Firewall

Open TCP port 7341 inbound, for example `sudo ufw allow 7341/tcp`, or the
cloud provider's security group. Nothing else needs to be reachable.

## Testing

```sh
cargo test --release
```

The game's own tests (`tests/net_lobby.rs`) also run whole co-op sessions
through a relay started in the test.
