//! The co-op relay for Air Traffic Control.
//!
//! Players can't usually connect to each other directly: home routers don't
//! let connections in. So the host and the other players all connect out to
//! the relay, which passes their messages between them. It never reads
//! them: it knows rooms, players and frames, nothing about the game.
//!
//! - `wire`: the relay's messages and framing, which the game uses too.
//! - `server`: the relay itself.

pub mod server;
pub mod wire;
