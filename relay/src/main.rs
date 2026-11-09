//! The relay server.
//!
//!     atc-relay [--listen 0.0.0.0:7341]
//!
//! or set RELAY_LISTEN. It runs until stopped.

use atc_relay::server::{Limits, serve};
use std::net::TcpListener;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let listen = args
        .iter()
        .position(|a| a == "--listen")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("RELAY_LISTEN").ok())
        .unwrap_or_else(|| "0.0.0.0:7341".into());
    let listener = match TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("atc-relay: can't listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("atc-relay: listening on {listen}");
    serve(listener, Limits::default());
}
