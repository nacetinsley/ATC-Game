//! What an aircraft is waiting for from the player, for the strip bay's
//! "waiting for you" marker and the attention cue. The conditions follow the
//! ones scoring uses, so the strip warns before a late or missed handoff costs
//! points.

use super::World;
use super::aircraft::{Aircraft, Flow, Leg, Phase, VfrIntent};
use super::positions::Position;
use super::tower::TOWER_HANDOFF_GATE_NM;

/// Inside this distance an arrival without a landing clearance is flagged, NM.
pub const LANDING_CLEARANCE_NM: f32 = 4.0;
/// An arrival on the localizer is flagged for the handoff to Tower this far
/// before the gate where it would be too late, NM.
const TOWER_HANDOFF_WARNING_NM: f32 = 4.0;
/// A departure is flagged for the handoff to Center this close to its exit fix, NM.
const CENTER_HANDOFF_NM: f32 = 12.0;
/// A check-in stays flagged this long if nothing is said, seconds.
const CHECK_IN_S: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaiting {
    /// Center is offering it to you.
    Accept,
    /// An arrival close in without a landing clearance.
    LandingClearance,
    /// It should be handed to the next controller now.
    HandoffDue,
    /// At the hold line, or lined up, for departure.
    ReadyForTakeoff,
    /// Stopped short of a runway it has to cross.
    HoldingShort,
    /// At the gate, or clear of the runway, waiting for taxi instructions.
    ReadyToTaxi,
    /// A VFR departure wanting to leave the frequency.
    FrequencyChange,
    /// Just called you, and hasn't been told anything yet.
    CheckIn,
}

impl Awaiting {
    /// What block 18 of the strip says.
    pub fn word(self) -> &'static str {
        match self {
            Awaiting::Accept => "ACPT",
            Awaiting::LandingClearance => "NO L",
            Awaiting::HandoffDue => "H/O",
            Awaiting::ReadyForTakeoff => "RDY",
            Awaiting::HoldingShort => "HS",
            Awaiting::ReadyToTaxi => "TAXI",
            Awaiting::FrequencyChange => "FREQ",
            Awaiting::CheckIn => "CALL",
        }
    }

    /// About to cost points.
    pub fn urgent(self) -> bool {
        matches!(self, Awaiting::LandingClearance | Awaiting::HandoffDue)
    }
}

impl World {
    /// What this aircraft is waiting for from the player, most pressing first.
    /// `None` for aircraft the player doesn't work.
    pub fn awaiting(&self, ac: &Aircraft) -> Option<Awaiting> {
        let staffing = self.config.staffing;
        if ac.phase == Phase::Offered {
            return staffing.approach.then_some(Awaiting::Accept);
        }
        if !self.is_players(ac) {
            return None;
        }
        let freq = ac.frequency();

        // Close in without a landing clearance.
        if freq == Position::Local {
            let close = match &ac.vfr {
                Some(v) => {
                    v.landing.is_none()
                        && matches!(v.leg, Leg::Base | Leg::Final)
                        && self.airport.surface.as_ref().is_some_and(|map| {
                            let physical = map.runway_of_end[v.end];
                            self.landing_distance(ac, physical)
                                .is_some_and(|d| d < LANDING_CLEARANCE_NM)
                        })
                }
                None => {
                    ac.phase == Phase::TowerHandedOff
                        && !ac.cleared_to_land
                        && ac.approach_runway.is_some_and(|r| {
                            self.airport.runways[r].along(ac.pos) < LANDING_CLEARANCE_NM
                        })
                }
            };
            if close {
                return Some(Awaiting::LandingClearance);
            }
        }

        if self.handoff_due(ac) {
            return Some(Awaiting::HandoffDue);
        }

        if ac.on_ground {
            if let Some(taxi) = &ac.taxi
                && taxi.holding_short.is_some()
            {
                return Some(Awaiting::HoldingShort);
            }
            // On a combined position it stays on one frequency throughout, so
            // what matters is which positions the player works.
            let lined_up = ac.phase == Phase::LinedUp && !ac.cleared_takeoff;
            if staffing.local && (lined_up || self.at_departure_hold(ac)) {
                return Some(Awaiting::ReadyForTakeoff);
            }
            let stopped = ac.taxi.as_ref().is_none_or(|t| t.done());
            let parked_departure = ac.phase == Phase::Parked && ac.flow == Flow::Departure;
            let clear_of_runway = ac.taxi.as_ref().is_some_and(|t| {
                self.airport
                    .surface
                    .as_ref()
                    .is_some_and(|m| m.nodes[t.node].zones.is_empty())
            });
            let arrival_waiting = ac.phase == Phase::Taxiing
                && ac.flow == Flow::Arrival
                && stopped
                && clear_of_runway
                && ac.parking.is_none_or(|p| {
                    self.airport.surface.as_ref().is_some_and(|m| {
                        ac.taxi
                            .as_ref()
                            .is_some_and(|t| t.node != m.parking[p].node)
                    })
                });
            if staffing.ground && (parked_departure || arrival_waiting) {
                return Some(Awaiting::ReadyToTaxi);
            }
        }

        if let Some(v) = &ac.vfr
            && v.intent == VfrIntent::Departure
            && !ac.on_ground
            && !v.released
            && matches!(ac.phase, Phase::TowerRelease | Phase::Pattern)
        {
            return Some(Awaiting::FrequencyChange);
        }

        // Called in and not yet answered with anything.
        let answered = ac
            .strip
            .iter()
            .any(|m| m.time >= ac.freq_since && m.by == freq);
        let fresh = self.time - ac.freq_since < CHECK_IN_S;
        let calls_in = match ac.phase {
            Phase::Controlled => ac.flow == Flow::Departure,
            Phase::TowerHandedOff => true,
            _ => false,
        };
        if calls_in && fresh && !answered && ac.freq_since > 0.0 {
            return Some(Awaiting::CheckIn);
        }
        None
    }

    /// Due to go to the next controller, who isn't the player.
    fn handoff_due(&self, ac: &Aircraft) -> bool {
        let staffing = self.config.staffing;
        // On the surface, between separate Ground and Local: the same wait the
        // missed-handoff check times.
        if ac.on_ground {
            return ac.waiting_since.is_some();
        }
        match (ac.phase, ac.flow) {
            // Approach to Tower before the gate.
            (Phase::Localizer | Phase::Glideslope, Flow::Arrival) if !staffing.local => {
                ac.approach_runway.is_some_and(|r| {
                    self.airport.runways[r].along(ac.pos)
                        < TOWER_HANDOFF_GATE_NM + TOWER_HANDOFF_WARNING_NM
                })
            }
            // Approach to Center near the exit or at its altitude.
            (Phase::Controlled, Flow::Departure) if staffing.approach && ac.vfr.is_none() => {
                let near_exit = ac.exit_fix.is_some_and(|f| {
                    self.airport.fixes[f].pos().distance(ac.pos) < CENTER_HANDOFF_NM
                });
                let high = ac.exit_altitude > 0.0 && ac.altitude >= ac.exit_altitude - 1000.0;
                near_exit || high
            }
            // Tower to Departure once it's airborne.
            (Phase::TowerRelease, Flow::Departure) => ac.vfr.is_none() && !staffing.approach,
            _ => false,
        }
    }
}
