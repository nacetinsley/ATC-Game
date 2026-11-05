//! The automated Tower: approach gates, landings, runway occupancy and departure release.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::autopilot::should_capture_localizer;
use super::scoring::{GoAroundReason, SimEvent};
use super::{Speaker, World};

/// Must be with Tower by this distance on final.
pub const TOWER_HANDOFF_GATE_NM: f32 = 5.0;
/// Tower checks the runway is clear at this distance.
const RUNWAY_CHECK_NM: f32 = 1.0;
const ROLLOUT_SECONDS: f32 = 45.0;
const ROLLOUT_SECONDS_HEAVY: f32 = 55.0;
/// Departures and go-arounds come to the player above this height over the field.
const CHECK_IN_AGL: f32 = 1500.0;
/// Tower won't line a departure up in front of an arrival closer than this.
const SHORT_FINAL_HOLD_NM: f32 = 4.0;
/// Minimum time between departures, seconds.
pub const DEPARTURE_INTERVAL: f32 = 90.0;
pub const DEPARTURE_INTERVAL_AFTER_HEAVY: f32 = 150.0;
/// How long a departure holds the runway from line-up to rotation.
const TAKEOFF_OCCUPANCY: f32 = 45.0;

impl World {
    pub(super) fn run_approaches(&mut self) {
        for index in 0..self.aircraft.len() {
            match self.aircraft[index].phase {
                Phase::ClearedApproach => self.step_cleared(index),
                Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff => {
                    self.step_final(index)
                }
                Phase::GoAround => {
                    let check_in = self.airport.elevation + CHECK_IN_AGL;
                    let ac = &mut self.aircraft[index];
                    if ac.altitude >= check_in {
                        ac.phase = Phase::Controlled;
                    }
                }
                Phase::TowerRelease => self.step_departure_check_in(index),
                _ => {}
            }
        }
    }

    fn step_cleared(&mut self, index: usize) {
        let ac = &mut self.aircraft[index];
        let Some(rwy) = ac.approach_runway else { return };
        let runway = &self.airport.runways[rwy];
        if should_capture_localizer(ac, runway) {
            ac.lateral = Lateral::Localizer;
            ac.phase = Phase::Localizer;
        } else if runway.along(ac.pos) < RUNWAY_CHECK_NM {
            // Flew past without ever establishing: the clearance is void.
            ac.phase = Phase::Controlled;
            ac.approach_runway = None;
            let line = format!("{}, missed the localizer, request vectors.", ac.callsign);
            self.say(Speaker::Pilot, line);
        }
    }

    fn step_final(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(rwy) = ac.approach_runway else { return };
        let along = self.airport.runways[rwy].along(ac.pos);
        let gs_altitude = self.airport.glideslope_altitude(rwy, along);
        let phase = ac.phase;
        let on_glideslope = ac.on_glideslope;
        let captures_glideslope = !on_glideslope && ac.altitude + 20.0 >= gs_altitude;

        if captures_glideslope {
            let ac = &mut self.aircraft[index];
            ac.on_glideslope = true;
            if ac.phase == Phase::Localizer {
                ac.phase = Phase::Glideslope;
            }
        }

        if phase != Phase::TowerHandedOff {
            if along < TOWER_HANDOFF_GATE_NM {
                let callsign = self.aircraft[index].callsign.clone();
                self.emit(SimEvent::LateHandoff { callsign });
                self.go_around(index, GoAroundReason::LateHandoff);
            }
            return;
        }

        if along < RUNWAY_CHECK_NM {
            if !(on_glideslope || captures_glideslope) {
                self.go_around(index, GoAroundReason::Unstable);
                return;
            }
            if self.runway_occupied(rwy) {
                self.go_around(index, GoAroundReason::RunwayOccupied);
                return;
            }
        }
        if along <= 0.0 {
            self.touch_down(index);
        }
    }

    fn touch_down(&mut self, index: usize) {
        let now = self.time;
        let elevation = self.airport.elevation;
        let ac = &mut self.aircraft[index];
        ac.altitude = elevation;
        ac.on_ground = true;
        ac.on_glideslope = false;
        ac.phase = Phase::Rollout;
        let heavy = ac.ty.wake == super::performance::WakeCategory::Heavy;
        let occupancy = if heavy { ROLLOUT_SECONDS_HEAVY } else { ROLLOUT_SECONDS };
        ac.rollout_until = now + occupancy;
        let minutes = (now - ac.spawn_time) / 60.0;
        let callsign = ac.callsign.clone();
        if let Some(rwy) = ac.approach_runway {
            self.runway_occupied_until[rwy] = now + occupancy;
        }
        self.emit(SimEvent::Landed { callsign, minutes });
    }

    fn runway_occupied(&self, rwy: usize) -> bool {
        self.time < self.runway_occupied_until[rwy]
    }

    fn step_departure_check_in(&mut self, index: usize) {
        let check_in = self.airport.elevation + CHECK_IN_AGL;
        let ac = &mut self.aircraft[index];
        if ac.on_ground || ac.altitude < check_in {
            return;
        }
        ac.phase = Phase::Controlled;
        ac.lateral = Lateral::Route;
        let exit = ac.exit_fix.map(|f| self.airport.fixes[f].name.clone()).unwrap_or_default();
        let line = format!(
            "{}, airborne, climbing to {:.0}, departure via {exit}.",
            ac.callsign, ac.target_altitude
        );
        self.say(Speaker::Pilot, line);
    }

    /// Line up the next departure if the runway and wake timing allow.
    pub(super) fn run_departure_release(&mut self) {
        if self.time < self.next_departure_time || self.departure_queue.is_empty() {
            return;
        }
        let Some(ty) = self.departure_queue.front().map(|a| a.ty) else { return };
        let rwy = self.departure_runway_for(ty);
        if self.runway_occupied(rwy) || self.arrival_on_short_final(rwy) {
            return;
        }
        let Some(mut ac) = self.departure_queue.pop_front() else { return };
        let runway = &self.airport.runways[rwy];
        ac.pos = runway.takeoff_point();
        ac.heading = runway.heading;
        ac.lateral = Lateral::Heading(runway.heading);
        ac.flow = Flow::Departure;
        ac.phase = Phase::TowerRelease;
        ac.on_ground = true;
        ac.ias = 0.0;
        ac.altitude = self.airport.elevation;
        ac.spawn_time = self.time;
        ac.msaw_armed = false;
        ac.entered_sector = true;

        let heavy = ac.ty.wake == super::performance::WakeCategory::Heavy;
        self.next_departure_time =
            self.time + if heavy { DEPARTURE_INTERVAL_AFTER_HEAVY } else { DEPARTURE_INTERVAL };
        self.runway_occupied_until[rwy] = self.time + TAKEOFF_OCCUPANCY;
        self.add_aircraft(ac);
    }

    fn arrival_on_short_final(&self, rwy: usize) -> bool {
        self.aircraft.iter().any(|a| {
            a.on_final()
                && a.approach_runway == Some(rwy)
                && self.airport.runways[rwy].along(a.pos) < SHORT_FINAL_HOLD_NM
        })
    }

    /// Remove aircraft that have finished rolling out.
    pub(super) fn clear_finished_rollouts(&mut self) {
        let now = self.time;
        self.aircraft
            .retain(|a: &Aircraft| !(a.phase == Phase::Rollout && now >= a.rollout_until));
    }
}

/// Departure ready for release; position and phase are set when it lines up.
pub(super) fn queued_departure(mut ac: Aircraft, initial_altitude: f32) -> Aircraft {
    ac.target_altitude = initial_altitude;
    ac.target_speed = ac.ty.max_speed.min(250.0);
    ac
}
