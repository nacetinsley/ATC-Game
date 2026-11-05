//! Seeded traffic generation and the difficulty ramp.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::performance::{TYPES, WakeCategory};
use super::tower::queued_departure;
use super::units::vec_heading;
use super::World;
use glam::Vec2;

pub const SHIFT_SECONDS: f32 = 30.0 * 60.0;
const START_RATE: f32 = 20.0;
const END_RATE: f32 = 45.0;
const ARRIVAL_SHARE: f32 = 0.55;
/// How far outside the gate an arrival appears, NM.
const SPAWN_BEFORE_GATE_NM: f32 = 10.0;
/// Do not spawn on top of traffic near the same gate.
const SPAWN_CLEAR_NM: f32 = 8.0;
const SPAWN_CLEAR_FT: f32 = 1500.0;
const SPAWN_RETRY_S: f32 = 15.0;
const AIRLINES: [&str; 6] = ["NVA", "BRZ", "TRL", "ORN", "MRD", "CWN"];

/// Movements per hour at a point in the shift. Ramps linearly, then holds.
pub fn movements_per_hour(time: f32) -> f32 {
    let t = (time / SHIFT_SECONDS).clamp(0.0, 1.0);
    START_RATE + (END_RATE - START_RATE) * t
}

impl World {
    pub(super) fn run_traffic(&mut self) {
        if self.time < self.next_spawn_time {
            return;
        }
        let spawned = if self.rng.f32() < ARRIVAL_SHARE {
            self.spawn_arrival()
        } else {
            self.queue_departure();
            true
        };
        self.next_spawn_time = if spawned {
            let interval = 3600.0 / movements_per_hour(self.time);
            let jitter = 0.75 + self.rng.f32() * 0.5;
            self.time + interval * jitter
        } else {
            self.time + SPAWN_RETRY_S
        };
    }

    fn pick_type(&mut self) -> &'static super::performance::AircraftType {
        // Mostly narrow-bodies, as at a busy hub: 10% small, 20% heavy.
        let roll = self.rng.f32();
        let wake = if roll < 0.1 {
            WakeCategory::Small
        } else if roll < 0.3 {
            WakeCategory::Heavy
        } else {
            WakeCategory::Large
        };
        let matching: Vec<&'static super::performance::AircraftType> =
            TYPES.iter().filter(|t| t.wake == wake).collect();
        matching[self.rng.usize(0..matching.len())]
    }

    fn new_callsign(&mut self) -> String {
        loop {
            let airline = AIRLINES[self.rng.usize(0..AIRLINES.len())];
            let callsign = format!("{airline}{}", self.rng.u32(100..1000));
            let taken = self.aircraft.iter().any(|a| a.callsign == callsign)
                || self.departure_queue.iter().any(|a| a.callsign == callsign);
            if !taken {
                return callsign;
            }
        }
    }

    /// Returns false if the gate is busy, in which case the caller retries shortly.
    fn spawn_arrival(&mut self) -> bool {
        let config = self.config().name.clone();
        let stars: Vec<Vec<usize>> = self
            .airport
            .stars
            .iter()
            .filter(|s| s.config == config)
            .map(|s| self.airport.route_indices(&s.fixes))
            .collect();
        let route = stars[self.rng.usize(0..stars.len())].clone();
        let ty = self.pick_type();

        let gate = self.airport.fixes[route[0]].pos();
        let next = route.get(1).map(|&f| self.airport.fixes[f].pos()).unwrap_or(Vec2::ZERO);
        let inbound = (next - gate).normalize_or_zero();
        let pos = gate - inbound * SPAWN_BEFORE_GATE_NM;

        let busy = self.aircraft.iter().any(|a| {
            (a.pos - pos).length() < SPAWN_CLEAR_NM
                && (a.altitude - ty.arrival_altitude).abs() < SPAWN_CLEAR_FT
        });
        if busy {
            return false;
        }

        let callsign = self.new_callsign();
        let speed = ty.max_speed.min(280.0);
        let heading = vec_heading(gate - pos);
        let id = self.next_id();
        let mut ac =
            Aircraft::new(id, callsign, Flow::Arrival, ty, pos, heading, ty.arrival_altitude, speed);
        ac.phase = Phase::Offered;
        ac.lateral = Lateral::Route;
        ac.route = route;
        ac.spawn_time = self.time;
        let line = format!("Center: offering {} via {}.", ac.callsign, self.airport.fixes[ac.route[0]].name);
        self.say(super::Speaker::System, line);
        self.add_aircraft(ac);
        true
    }

    fn queue_departure(&mut self) {
        let config = self.config().name.clone();
        let sids: Vec<(Vec<usize>, f32)> = self
            .airport
            .sids
            .iter()
            .filter(|s| s.config == config)
            .map(|s| (self.airport.route_indices(&s.fixes), s.exit_altitude))
            .collect();
        let (route, exit_altitude) = sids[self.rng.usize(0..sids.len())].clone();
        let ty = self.pick_type();
        let callsign = self.new_callsign();
        let id = self.next_id();
        let mut ac =
            Aircraft::new(id, callsign, Flow::Departure, ty, Vec2::ZERO, 0.0, 0.0, 0.0);
        ac.exit_fix = route.last().copied();
        ac.exit_altitude = exit_altitude;
        ac.route = route;
        self.departure_queue.push_back(queued_departure(ac));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_ramps_from_start_to_end_rate() {
        assert_eq!(movements_per_hour(0.0), 20.0);
        assert_eq!(movements_per_hour(SHIFT_SECONDS), 45.0);
        assert_eq!(movements_per_hour(SHIFT_SECONDS / 2.0), 32.5);
        assert_eq!(movements_per_hour(SHIFT_SECONDS * 2.0), 45.0);
    }
}
