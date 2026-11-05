//! Seeded traffic generation and the difficulty ramp.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::performance::{AircraftType, DEFAULT_FLEET_SIZE, TYPES, WakeCategory, aircraft_type};
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
/// Margin outside the sector for a gate that sits well inside the boundary, NM.
const SPAWN_OUTSIDE_SECTOR_NM: f32 = 8.0;
/// Do not spawn on top of traffic near the same gate.
const SPAWN_CLEAR_NM: f32 = 8.0;
const SPAWN_CLEAR_FT: f32 = 1500.0;
const SPAWN_RETRY_S: f32 = 15.0;
const AIRLINES: [&str; 6] = ["NVA", "BRZ", "TRL", "ORN", "MRD", "CWN"];

/// How busy the airspace is, as a multiple of the airport's normal rate.
/// The middle level is the airport's own rate: the real-world estimate for
/// data-backed airports, the designed ramp for fictional ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Busyness {
    VeryQuiet,
    Quiet,
    #[default]
    RealWorld,
    Busy,
    VeryBusy,
}

impl Busyness {
    pub const ALL: [Busyness; 5] = [
        Busyness::VeryQuiet,
        Busyness::Quiet,
        Busyness::RealWorld,
        Busyness::Busy,
        Busyness::VeryBusy,
    ];

    pub fn multiplier(self) -> f32 {
        match self {
            Busyness::VeryQuiet => 0.25,
            Busyness::Quiet => 0.5,
            Busyness::RealWorld => 1.0,
            Busyness::Busy => 2.0,
            Busyness::VeryBusy => 4.0,
        }
    }

    /// Menu label. `real_data` names the middle level after real-world traffic.
    pub fn label(self, real_data: bool) -> &'static str {
        match self {
            Busyness::VeryQuiet => "Very quiet",
            Busyness::Quiet => "Quiet",
            Busyness::RealWorld if real_data => "Real-world",
            Busyness::RealWorld => "Normal",
            Busyness::Busy => "Busy",
            Busyness::VeryBusy => "Very busy",
        }
    }

    pub fn index(self) -> usize {
        Busyness::ALL.iter().position(|&b| b == self).expect("listed")
    }

    /// One step quieter (`-1`) or busier (`+1`), stopping at the ends.
    pub fn step(self, by: i32) -> Busyness {
        let index = (self.index() as i32 + by).clamp(0, Busyness::ALL.len() as i32 - 1);
        Busyness::ALL[index as usize]
    }
}

/// Movements per hour at a point in the shift. Ramps linearly, then holds.
pub fn movements_per_hour(time: f32) -> f32 {
    let t = (time / SHIFT_SECONDS).clamp(0.0, 1.0);
    START_RATE + (END_RATE - START_RATE) * t
}

impl World {
    /// Movements per hour for this airport right now.
    pub fn current_rate(&self) -> f32 {
        let base = match &self.airport.traffic {
            Some(t) => {
                let progress = (self.time / SHIFT_SECONDS).clamp(0.0, 1.0);
                t.start_rate + (t.end_rate - t.start_rate) * progress
            }
            None => movements_per_hour(self.time),
        };
        base * self.config.busyness.multiplier()
    }

    pub(super) fn run_traffic(&mut self) {
        if self.time < self.next_spawn_time {
            return;
        }
        let share = self.airport.traffic.as_ref().map_or(ARRIVAL_SHARE, |t| t.arrival_share);
        let spawned = if self.rng.f32() < share {
            self.spawn_arrival()
        } else {
            self.queue_departure();
            true
        };
        self.next_spawn_time = if spawned {
            let interval = 3600.0 / self.current_rate();
            let jitter = 0.75 + self.rng.f32() * 0.5;
            self.time + interval * jitter
        } else {
            self.time + SPAWN_RETRY_S
        };
    }

    fn pick_type(&mut self) -> &'static AircraftType {
        if let Some(traffic) = &self.airport.traffic {
            let total: f32 = traffic.fleet_weights.iter().sum();
            let mut roll = self.rng.f32() * total;
            for (name, weight) in traffic.fleet.iter().zip(&traffic.fleet_weights) {
                roll -= weight;
                if roll <= 0.0 {
                    return aircraft_type(name).expect("validated at load");
                }
            }
            let last = traffic.fleet.last().expect("validated at load");
            return aircraft_type(last).expect("validated at load");
        }

        // Mostly narrow-bodies, as at a busy hub: 10% small, 20% heavy.
        let roll = self.rng.f32();
        let wake = if roll < 0.1 {
            WakeCategory::Small
        } else if roll < 0.3 {
            WakeCategory::Heavy
        } else {
            WakeCategory::Large
        };
        let matching: Vec<&'static AircraftType> =
            TYPES[..DEFAULT_FLEET_SIZE].iter().filter(|t| t.wake == wake).collect();
        matching[self.rng.usize(0..matching.len())]
    }

    fn new_callsign(&mut self) -> String {
        loop {
            let airline = match &self.airport.traffic {
                Some(t) => t.callsign_prefixes[self.rng.usize(0..t.callsign_prefixes.len())].clone(),
                None => AIRLINES[self.rng.usize(0..AIRLINES.len())].to_string(),
            };
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
        let stars: Vec<(Vec<usize>, Option<f32>)> = self
            .airport
            .stars
            .iter()
            .filter(|s| s.config == config)
            .map(|s| (self.airport.route_indices(&s.fixes), s.final_heading))
            .collect();
        let (route, final_heading) = stars[self.rng.usize(0..stars.len())].clone();
        let ty = self.pick_type();

        let gate = self.airport.fixes[route[0]].pos();
        let next = route.get(1).map(|&f| self.airport.fixes[f].pos()).unwrap_or(Vec2::ZERO);
        let inbound = (next - gate).normalize_or_zero();
        // Real STARs can start well inside the sector, so back up past the boundary.
        let mut to_boundary = 0.0;
        while to_boundary < 200.0 && self.airport.in_sector(gate - inbound * to_boundary) {
            to_boundary += 1.0;
        }
        let back = SPAWN_BEFORE_GATE_NM.max(to_boundary + SPAWN_OUTSIDE_SECTOR_NM);
        let pos = gate - inbound * back;

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
        ac.route_end_heading = final_heading;
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
        self.request_departure(ty, route, exit_altitude);
    }

    /// Add a departure to Tower's queue.
    pub fn request_departure(
        &mut self,
        ty: &'static AircraftType,
        route: Vec<usize>,
        exit_altitude: f32,
    ) {
        let callsign = self.new_callsign();
        let id = self.next_id();
        let mut ac =
            Aircraft::new(id, callsign, Flow::Departure, ty, Vec2::ZERO, 0.0, 0.0, 0.0);
        ac.exit_fix = route.last().copied();
        // Piston singles can't reach a jet's handoff altitude in this sector.
        let has_profile = self.airport.traffic.is_some();
        ac.exit_altitude =
            if has_profile { exit_altitude.min(ty.arrival_altitude) } else { exit_altitude };
        ac.route = route;
        let initial = self.airport.departure_initial_altitude;
        let initial = if has_profile { initial.min(ty.arrival_altitude) } else { initial };
        self.departure_queue.push_back(queued_departure(ac, initial));
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
