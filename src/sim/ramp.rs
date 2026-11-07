//! Ramp control. Where the ground layout has ramp spots, some aircraft are
//! handed over at a spot instead of being taxied to their stand:
//!
//! - airlines under the airport's ramp control (FedEx at Memphis) go to that
//!   ramp tower, which picks the stand and brings departures out to a spot,
//!   where they call Ground;
//! - general aviation goes to an FBO, whose pilots park themselves.
//!
//! An aircraft with the ramp is on [`Position::Ramp`]: off the player's
//! frequency, moving on its own.

use super::aircraft::{Aircraft, Flow, Phase, Taxi};
use super::clearance::{AT_NODE_NM, set_route};
use super::ground_map::{Parking, StandUse};
use super::positions::Position;
use super::scoring::SimEvent;
use super::traffic::airline_of;
use super::{Speaker, World};

/// Seconds a cleared ramp departure waits at its stand before the ramp pushes it.
const PUSH_DELAY_S: f32 = 60.0;

/// Who takes an aircraft where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    /// Ground taxis it to its stand.
    Stand,
    /// Handed to the ramp tower at a spot.
    Ramp,
    /// Handed off at the FBO's spot to park itself.
    Fbo,
}

impl Handling {
    fn stands(self) -> Option<StandUse> {
        match self {
            Handling::Stand => None,
            Handling::Ramp => Some(StandUse::Ramp),
            Handling::Fbo => Some(StandUse::Ga),
        }
    }
}

/// A stand behind a ramp spot, of this use.
fn released(p: &Parking, stands: StandUse) -> bool {
    p.stand == Some(stands) && p.spot.is_some()
}

impl World {
    pub fn handling(&self, ac: &Aircraft) -> Handling {
        let Some(map) = self.airport.surface.as_ref() else { return Handling::Stand };
        let any = |stands: StandUse| map.parking.iter().any(|p| released(p, stands));
        let handling = match airline_of(&ac.callsign).filter(|_| ac.vfr.is_none()) {
            Some(airline) => {
                let ramp = self.airport.ramp_control.as_ref().is_some_and(|r| r.airlines.iter().any(|a| a == airline));
                if ramp { Handling::Ramp } else { Handling::Stand }
            }
            None => Handling::Fbo,
        };
        match handling.stands() {
            Some(stands) if any(stands) => handling,
            _ => Handling::Stand,
        }
    }

    /// The spot an aircraft bound for (or leaving) `parking` is handed over
    /// at, if it's one Ground hands off at.
    pub fn ramp_spot_for(&self, ac: &Aircraft, parking: Option<usize>) -> Option<usize> {
        let stands = self.handling(ac).stands()?;
        let map = self.airport.surface.as_ref()?;
        let p = map.parking.get(parking?)?;
        if released(p, stands) { p.spot } else { None }
    }

    pub fn ramp_spot(&self, ac: &Aircraft) -> Option<usize> {
        self.ramp_spot_for(ac, ac.parking)
    }

    /// Where Ground taxis an arrival: its ramp spot, or else its stand.
    pub fn taxi_in_goal(&self, ac: &Aircraft) -> Option<usize> {
        self.taxi_in_goal_for(ac, ac.parking)
    }

    pub(super) fn taxi_in_goal_for(&self, ac: &Aircraft, parking: Option<usize>) -> Option<usize> {
        let map = self.airport.surface.as_ref()?;
        match self.ramp_spot_for(ac, parking) {
            Some(spot) => Some(map.spots[spot].node),
            None => parking.map(|p| map.parking[p].node),
        }
    }

    /// What an arrival says it's taxiing for: "spot 6", "Wilson Air" or its gate.
    pub fn taxi_in_name(&self, ac: &Aircraft) -> Option<String> {
        let map = self.airport.surface.as_ref()?;
        match self.ramp_spot(ac) {
            Some(spot) => Some(map.spots[spot].spoken()),
            None => ac.parking.map(|p| map.parking[p].name.clone()),
        }
    }

    /// A stand for this aircraft, starting from node `from`. Behind a ramp
    /// spot, the ramp tower takes the nearest free one and a general aviation
    /// pilot whichever free one they fancy; otherwise as `free_parking`.
    pub(super) fn pick_parking(&self, ac: &Aircraft, from: usize) -> Option<usize> {
        let handling = self.handling(ac);
        let picked = handling.stands().and_then(|stands| {
            let free = self.free_parking_where(from, |p| released(p, stands));
            match handling {
                Handling::Fbo if !free.is_empty() => Some(free[ac.id as usize % free.len()]),
                _ => free.first().copied(),
            }
        });
        picked.or_else(|| self.free_parking(from, self.stand_for(ac)))
    }

    /// Hand arrivals over at their spot, and bring ramp departures out to theirs.
    pub(super) fn run_ramp(&mut self) {
        if self.airport.surface.as_ref().is_none_or(|m| m.spots.is_empty()) {
            return;
        }
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            if !ac.on_ground || !matches!(ac.phase, Phase::Parked | Phase::Taxiing) {
                continue;
            }
            let Some(spot) = self.ramp_spot(ac) else { continue };
            let map = self.surface();
            let spot_node = map.spots[spot].node;
            let stand_node = map.parking[ac.parking.expect("has a spot")].node;
            let Some(taxi) = ac.taxi.as_ref() else { continue };
            let stopped_at = |node: usize| taxi.done() && taxi.node == node && ac.pos.distance(map.nodes[node].pos) < AT_NODE_NM;
            let on_ramp = ac.surface_freq == Position::Ramp;
            match (ac.flow, ac.phase) {
                (Flow::Arrival, Phase::Taxiing) if !on_ramp && stopped_at(spot_node) => {
                    self.hand_to_ramp(index, stand_node);
                }
                // Cleared at the stand: the ramp takes it from Ground or Delivery.
                (Flow::Departure, Phase::Parked)
                    if !on_ramp
                        && ac.surface_freq == Position::Ground
                        && stopped_at(stand_node) =>
                {
                    let now = self.time;
                    let ac = &mut self.aircraft[index];
                    ac.surface_freq = Position::Ramp;
                    ac.freq_since = now;
                }
                (Flow::Departure, Phase::Parked)
                    if on_ramp
                        && self.time - ac.spawn_time >= PUSH_DELAY_S
                        && ac.flight_plan.as_ref().is_none_or(|p| p.is_cleared()) =>
                {
                    let Some(route) = map.route(taxi.node, spot_node, &[]) else { continue };
                    let ac = &mut self.aircraft[index];
                    set_route(ac, self.airport.surface.as_ref().expect("checked"), taxi_node(ac), route.nodes);
                    ac.phase = Phase::Taxiing;
                }
                (Flow::Departure, Phase::Taxiing) if on_ramp && stopped_at(spot_node) => {
                    self.release_to_ground(index, spot_node, spot);
                }
                _ => {}
            }
        }
    }

    /// An arrival reached its spot: the ramp or its pilot takes it to the stand.
    fn hand_to_ramp(&mut self, index: usize, stand_node: usize) {
        let ac = &self.aircraft[index];
        let map = self.surface();
        let from = ac.taxi.as_ref().expect("taxiing").node;
        let Some(route) = map.route(from, stand_node, &[]) else { return };
        let spot = self.taxi_in_name(ac).unwrap_or_default();
        let line = match self.handling(ac) {
            Handling::Ramp => {
                let ramp = self.airport.ramp_control.as_ref().map(|r| r.name.as_str()).unwrap_or("ramp");
                format!("{}, at {spot}, over to {ramp}.", ac.callsign)
            }
            _ => format!("{}, at {spot}, we'll find our own spot. Good day.", ac.callsign),
        };
        let speaks = self.is_players(ac);
        let snapshot = ac.clone();
        self.emergency_parked(&snapshot);
        let now = self.time;
        let ac = &mut self.aircraft[index];
        ac.surface_freq = Position::Ramp;
        ac.freq_since = now;
        set_route(ac, self.airport.surface.as_ref().expect("has a surface"), from, route.nodes);
        if speaks {
            self.say(Speaker::Pilot, line);
        }
        // Ground's job is done: it counts as parked.
        self.emit(SimEvent::Parked { callsign: snapshot.callsign });
    }

    /// A ramp departure reached its spot: it's Ground's now.
    fn release_to_ground(&mut self, index: usize, node: usize, spot: usize) {
        let now = self.time;
        let name = self.surface().spots[spot].spoken();
        let ac = &mut self.aircraft[index];
        ac.surface_freq = Position::Ground;
        ac.freq_since = now;
        ac.phase = Phase::Parked;
        ac.taxi = Some(Taxi::at(node));
        // Its stand is free again.
        ac.parking = None;
        let line = format!("Ground, {}, at {name}, ready to taxi.", ac.callsign);
        if self.config.staffing.ground {
            self.say(Speaker::Pilot, line);
        }
    }
}

fn taxi_node(ac: &Aircraft) -> usize {
    ac.taxi.as_ref().expect("taxiing").node
}
