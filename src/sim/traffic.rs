//! Seeded traffic generation and the difficulty ramp.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::holding::{self, HoldState};
use super::performance::{AircraftType, DEFAULT_FLEET_SIZE, TYPES, WakeCategory, aircraft_type};
use super::registration;
use super::tower::{FINAL_SPAWN_NM, queued_departure};
use super::units::{heading_vec, vec_heading};
use super::World;
use glam::Vec2;

/// A held arrival starts this far out on the hold's inbound course, NM.
const HOLD_SPAWN_NM: f32 = 3.0;

/// The default shift for a `WorldConfig`: what tests and headless runs use.
/// The game's menu picks its own length, an hour unless changed.
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
/// Spacing from other arrivals when spawning on final, NM.
const FINAL_SPAWN_CLEAR_NM: f32 = 8.0;
const AIRLINES: [&str; 6] = ["NVA", "BRZ", "TRL", "ORN", "MRD", "CWN"];
/// Callsign prefix in airport data meaning "use the aircraft's registration".
const GENERAL_AVIATION: &str = "N";
/// Share of registration callsigns that are foreign. Only jets fly in from abroad.
const INTERNATIONAL_SHARE: f32 = 0.04;
const INTERNATIONAL_MIN_KT: f32 = 250.0;

/// The airline of a flight-number callsign ("FDX123" is FDX), or None for a
/// registration such as N175FT.
pub fn airline_of(callsign: &str) -> Option<&str> {
    let b = callsign.as_bytes();
    let airline = b.len() > 3 && b[..3].iter().all(u8::is_ascii_uppercase) && b[3..].iter().all(u8::is_ascii_digit);
    airline.then(|| &callsign[..3])
}

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

/// Traffic builds over the shift, but no longer than this; a longer shift
/// holds the peak for the rest, seconds.
pub const MAX_RAMP_SECONDS: f32 = 60.0 * 60.0;

/// How far through the traffic ramp a shift of `shift` seconds is at `time`, 0 to 1.
pub fn ramp_progress(time: f32, shift: f32) -> f32 {
    let ramp = shift.clamp(1.0, MAX_RAMP_SECONDS);
    (time / ramp).clamp(0.0, 1.0)
}

/// Movements per hour at a point in a shift of `shift` seconds. Ramps
/// linearly, then holds.
pub fn movements_per_hour(time: f32, shift: f32) -> f32 {
    START_RATE + (END_RATE - START_RATE) * ramp_progress(time, shift)
}

impl World {
    /// Movements per hour for this airport right now.
    pub fn current_rate(&self) -> f32 {
        let base = match &self.airport.traffic {
            Some(t) => {
                let progress = ramp_progress(self.time, self.config.shift_seconds);
                t.start_rate + (t.end_rate - t.start_rate) * progress
            }
            None => movements_per_hour(self.time, self.config.shift_seconds),
        };
        base * self.config.busyness.multiplier()
    }

    pub(super) fn run_traffic(&mut self) {
        if self.time < self.next_spawn_time {
            return;
        }
        let share = self.airport.traffic.as_ref().map_or(ARRIVAL_SHARE, |t| t.arrival_share);
        let spawned = if self.rng.f32() < share {
            if self.config.staffing.approach { self.spawn_arrival() } else { self.spawn_final_arrival() }
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

    fn new_callsign(&mut self, ty: &AircraftType) -> String {
        loop {
            let airline = match &self.airport.traffic {
                Some(t) => {
                    // The type's own operators where the airport lists them.
                    let own = t
                        .fleet
                        .iter()
                        .position(|name| name == ty.designator)
                        .and_then(|i| t.fleet_airlines.get(i))
                        .filter(|list| !list.is_empty());
                    let prefixes = own.unwrap_or(&t.callsign_prefixes);
                    prefixes[self.rng.usize(0..prefixes.len())].clone()
                }
                None => AIRLINES[self.rng.usize(0..AIRLINES.len())].to_string(),
            };
            // "N" in the data means a privately registered aircraft, not an airline.
            let callsign = if airline == GENERAL_AVIATION {
                if ty.max_speed >= INTERNATIONAL_MIN_KT && self.rng.f32() < INTERNATIONAL_SHARE {
                    registration::foreign(&mut self.rng)
                } else {
                    registration::n_number(&mut self.rng)
                }
            } else {
                format!("{airline}{}", self.rng.u32(100..1000))
            };
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

        let callsign = self.new_callsign(ty);
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
        self.roll_arrival_emergency(&mut ac, false);
        let line = format!("Center: offering {} via {}.", ac.callsign, self.airport.fixes[ac.route[0]].name);
        self.say(super::Speaker::System, line);
        if self.config.staffing.approach {
            self.emit(super::scoring::SimEvent::HandoffOffered { callsign: ac.callsign.clone() });
        }
        self.add_aircraft(ac);
        true
    }

    /// Whether an arrival can be put on final to the arrival runway now: clear
    /// of other arrivals near the spawn point, and of VFR traffic already on
    /// base or final for the same runway (as when a closure puts IFR arrivals
    /// on the pattern runway).
    pub(super) fn room_on_final(&self) -> bool {
        // After a runway change, the old flow lands before the new one starts.
        if self.landing_against_flow() {
            return false;
        }
        let rwy = self.arrival_runway();
        let runway = &self.airport.runways[rwy];
        let physical = self.airport.surface.as_ref().map(|m| m.runway_of_end[rwy]);
        let clear_nm =
            if self.weather.sky.lvp() { super::visibility::LVP_FINAL_CLEAR_NM } else { FINAL_SPAWN_CLEAR_NM };
        !self.aircraft.iter().any(|a| {
            let spacing = a.approach_runway == Some(rwy)
                && !a.on_ground
                && (runway.along(a.pos) - FINAL_SPAWN_NM).abs() < clear_nm;
            let vfr_landing =
                a.vfr.is_some() && physical.is_some_and(|p| self.landing_distance(a, p).is_some());
            spacing || vfr_landing
        })
    }

    /// With nobody working Approach, arrivals turn up established on the ILS,
    /// already talking to Tower. While the runway can't take them, or the
    /// final is full, the automated Approach holds them instead.
    fn spawn_final_arrival(&mut self) -> bool {
        let ty = self.pick_type();
        // Below minimums, only Category III types go onto the final.
        if self.arrivals_held() || !self.room_on_final() || !self.can_land(ty) {
            return self.spawn_holding_arrival(ty);
        }
        let callsign = self.new_callsign(ty);
        let id = self.next_id();
        let time = self.time;
        self.place_on_final(id, callsign, ty, time, true);
        true
    }

    /// Put an arrival on a 10-mile final, talking to Tower.
    pub(super) fn place_on_final(
        &mut self,
        id: u32,
        callsign: String,
        ty: &'static AircraftType,
        spawn_time: f32,
        roll_emergency: bool,
    ) {
        let rwy = self.arrival_runway();
        let runway = self.airport.runways[rwy].clone();
        let pos = runway.threshold() - heading_vec(runway.heading) * FINAL_SPAWN_NM;
        // The ILS, or the RNAV while it's out.
        let approach = self.default_approach(rwy).unwrap_or_default();
        let altitude = self.airport.approach_altitude(rwy, FINAL_SPAWN_NM, approach);
        let speed = ty.max_speed.min(180.0);
        let mut ac = Aircraft::new(id, callsign, Flow::Arrival, ty, pos, runway.heading, altitude, speed);
        ac.approach = approach;
        ac.phase = Phase::TowerHandedOff;
        ac.lateral = Lateral::Localizer;
        ac.approach_runway = Some(rwy);
        ac.on_glideslope = true;
        ac.spawn_time = spawn_time;
        ac.msaw_armed = false;
        ac.entered_sector = self.airport.in_sector(pos);
        ac.freq_since = self.time;
        if roll_emergency {
            self.roll_arrival_emergency(&mut ac, true);
        }
        // The automated Approach gives an emergency a short final if there's a gap.
        if ac.emergency.is_some() {
            self.priority_final(&mut ac, rwy);
        }
        // Radio failures say nothing, and emergencies make their own call.
        if self.config.staffing.local && ac.emergency.is_none() {
            let line = format!(
                "Tower, {}, {FINAL_SPAWN_NM:.0} mile final runway {}, with {}.",
                ac.callsign,
                runway.id,
                self.weather.information()
            );
            self.pilot_say(super::Speaker::Pilot, ac.callsign.clone(), line);
        }
        self.add_aircraft(ac);
    }

    /// A new arrival for the automated Approach to hold: at the bottom free
    /// level of the least busy published hold it can use.
    fn spawn_holding_arrival(&mut self, ty: &'static AircraftType) -> bool {
        let Some((fix, altitude)) = self.automated_hold_slot() else { return false };
        let callsign = self.new_callsign(ty);
        let id = self.next_id();
        let spec = self.airport.published_hold(fix).expect("slots are published holds");
        let inbound = spec.inbound;
        let pos = self.airport.fixes[fix].pos() - heading_vec(inbound) * HOLD_SPAWN_NM;
        let speed = holding::max_speed(altitude).min(ty.max_speed);
        let mut ac = Aircraft::new(id, callsign, Flow::Arrival, ty, pos, inbound, altitude, speed);
        ac.spawn_time = self.time;
        ac.entered_sector = true;
        ac.freq_since = self.time;
        let spec = self.hold_for(&ac, fix);
        ac.lateral = Lateral::Hold(HoldState::new(spec));
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
        self.roll_departure_emergency();
    }

    /// A SID from the runway configuration in use: one leaving by `exit` if
    /// there is one, otherwise any. Returns the route and its exit altitude.
    pub(super) fn sid_for_exit(&mut self, exit: Option<usize>) -> (Vec<usize>, f32) {
        let config = self.config().name.clone();
        let sids: Vec<(Vec<usize>, f32)> = self
            .airport
            .sids
            .iter()
            .filter(|s| s.config == config)
            .map(|s| (self.airport.route_indices(&s.fixes), s.exit_altitude))
            .collect();
        if let Some(same) = sids.iter().find(|(route, _)| exit.is_some() && route.last().copied() == exit) {
            return same.clone();
        }
        sids[self.rng.usize(0..sids.len())].clone()
    }

    /// The configuration in use's STAR from this gate, if it has one.
    pub(super) fn star_from_gate(&self, gate: usize) -> Option<(Vec<usize>, Option<f32>)> {
        let config = &self.config().name;
        self.airport
            .stars
            .iter()
            .filter(|s| &s.config == config)
            .map(|s| (self.airport.route_indices(&s.fixes), s.final_heading))
            .find(|(route, _)| route.first() == Some(&gate))
    }

    /// Add a departure to Tower's queue.
    pub fn request_departure(
        &mut self,
        ty: &'static AircraftType,
        route: Vec<usize>,
        exit_altitude: f32,
    ) {
        let callsign = self.new_callsign(ty);
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
        let mut ac = queued_departure(ac, initial);
        self.file_flight_plan(&mut ac);
        self.departure_queue.push_back(ac);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airline_of_tells_flight_numbers_from_registrations() {
        assert_eq!(airline_of("FDX1234"), Some("FDX"));
        assert_eq!(airline_of("N175FT"), None);
        assert_eq!(airline_of("N12345"), None);
        assert_eq!(airline_of("CGABC"), None);
        assert_eq!(airline_of("FDX"), None);
    }

    #[test]
    fn registration_callsigns_are_mostly_n_numbers_with_a_few_foreign_jets() {
        let airport = crate::sim::airspace::Airport::load(crate::sim::airspace::KSUS_AIRPORT, Some(crate::sim::airspace::KSUS_GROUND))
            .expect("valid");
        let mut world = World::new(airport, crate::sim::WorldConfig::default());
        let jet = aircraft_type("C56X").unwrap();
        let piston = aircraft_type("C172").unwrap();
        let (mut registered, mut foreign) = (0, 0);
        for _ in 0..4000 {
            let callsign = world.new_callsign(jet);
            if callsign.starts_with("SPT") || callsign.starts_with("GTW") {
                continue;
            }
            registered += 1;
            if !callsign.starts_with('N') {
                foreign += 1;
            } else {
                assert!(registration::is_valid_n_number(&callsign), "{callsign}");
            }
        }
        let share = foreign as f32 / registered as f32;
        assert!((0.02..0.07).contains(&share), "{foreign} of {registered}");
        for _ in 0..2000 {
            let callsign = world.new_callsign(piston);
            assert!(
                ["N", "SPT", "GTW"].iter().any(|p| callsign.starts_with(p)),
                "a C172 shouldn't arrive from abroad: {callsign}"
            );
        }
    }

    #[test]
    fn traffic_ramps_from_start_to_end_rate() {
        assert_eq!(movements_per_hour(0.0, SHIFT_SECONDS), 20.0);
        assert_eq!(movements_per_hour(SHIFT_SECONDS, SHIFT_SECONDS), 45.0);
        assert_eq!(movements_per_hour(SHIFT_SECONDS / 2.0, SHIFT_SECONDS), 32.5);
        assert_eq!(movements_per_hour(SHIFT_SECONDS * 2.0, SHIFT_SECONDS), 45.0);
    }

    #[test]
    fn traffic_ramps_over_the_shift_up_to_an_hour_then_holds() {
        let two_hours = 2.0 * MAX_RAMP_SECONDS;
        assert_eq!(movements_per_hour(MAX_RAMP_SECONDS / 2.0, two_hours), 32.5);
        assert_eq!(movements_per_hour(MAX_RAMP_SECONDS, two_hours), 45.0);
        assert_eq!(movements_per_hour(two_hours, two_hours), 45.0);
        assert_eq!(movements_per_hour(150.0, 300.0), 32.5);
    }
}
