//! Co-op over a connection: every player runs the same sim, in lockstep.
//!
//! The host is the clock. Each tick it sends `Tick { n, commands }`, and every
//! machine, the host's included, applies those commands and runs tick `n`.
//! The sim is deterministic, so all the worlds stay the same; a checksum now
//! and then proves it. Players' commands go to the host, which puts them in
//! the next tick.
//!
//! - `protocol`: the messages and how they're framed on the wire.
//! - `link`: the transport, as traits, with an in-memory one for tests.
//! - `tcp`: a direct TCP connection (tests and LAN play).
//! - `session`: the host's and a client's side of the lockstep.

pub mod link;
pub mod protocol;
pub mod session;
pub mod tcp;
