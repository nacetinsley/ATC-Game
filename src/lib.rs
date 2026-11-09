//! Top-down air traffic control simulator.
//!
//! `sim` is headless and deterministic for a given seed; `ui` draws it with
//! macroquad; `net` keeps co-op players' sims in step.

pub mod net;
pub mod sim;
pub mod ui;
