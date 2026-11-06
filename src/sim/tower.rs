//! Local control: approach gates, landings, runway occupancy and takeoffs, plus
//! the automated Tower, Ground and Approach positions for whatever the player
//! isn't working.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::autopilot::should_capture_localizer;
use super::clearance::{AT_NODE_NM, Clearance, PILOT_DELAY, issuer_for};
use super::ground_map::GroundMap;
use super::performance::WakeCategory;
use super::positions::Position;
use super::scoring::{GoAroundReason, SimEvent};
use super::{Speaker, World};

/// Must be with Tower by this distance on final.
pub const TOWER_HANDOFF_GATE_NM: f32 = 5.0;
/// Tower checks the runway and landing clearance at this distance.
pub(super) const RUNWAY_CHECK_NM: f32 = 1.0;
/// Departures and go-arounds go to Approach above this height over the field.
const CHECK_IN_AGL: f32 = 1500.0;
/// The automated Tower clears arrivals to land inside this distance.
const LANDING_CLEARANCE_NM: f32 = 8.0;
/// Tower never launches a departure in front of an arrival closer than this,
/// whatever its speed.
const SHORT_FINAL_HOLD_NM: f32 = 2.5;
/// Or send an aircraft across a runway with an arrival closer than this.
const CROSSING_HOLD_NM: f32 = 5.0;
/// Minimum time between departures, seconds.
pub const DEPARTURE_INTERVAL: f32 = 90.0;
pub const DEPARTURE_INTERVAL_AFTER_HEAVY: f32 = 150.0;
/// Entering a runway with an arrival closer than this is an incursion.
pub const INCURSION_FINAL_NM: f32 = 2.0;
/// A departure should be talking to Departure by this far past the runway end.
const DEPARTURE_HANDOFF_NM: f32 = 3.0;
/// How long an aircraft waits for a handoff before it counts as missed, seconds.
const HANDOFF_GRACE_S: f32 = 60.0;
/// The automated Approach takes departures and go-arounds off the board here.
const AUTOMATED_APPROACH_RANGE_NM: f32 = 6.0;
/// Choosing parking, a runway crossing costs as much as this much taxiing, NM.
const PARKING_CROSSING_NM: f32 = 2.0;
/// Arrivals in tower-only modes appear this far out on final.
pub const FINAL_SPAWN_NM: f32 = 10.0;
/// Entering a runway with VFR traffic this close on base or final is an incursion.
pub const VFR_INCURSION_NM: f32 = 1.0;
/// Tower never launches a departure in front of VFR traffic closer than this:
/// the incursion distance, plus how far it flies while the departure gets onto
/// the runway, NM.
const VFR_RELEASE_NM: f32 = 1.6;
/// Time a crossing takes, with margin, for VFR traffic about to land, seconds.
const VFR_CROSSING_S: f32 = 75.0;
/// A departure must be airborne this long before the next arrival lands, seconds.
const TAKEOFF_MARGIN_S: f32 = 15.0;
/// Time from the hold line to starting the roll, seconds.
const LINE_UP_S: f32 = 30.0;
/// Minimum time between a VFR departure and the next, seconds.
const VFR_DEPARTURE_INTERVAL: f32 = 45.0;

impl World {
    pub(super) fn surface(&self) -> &GroundMap {
        self.airport.surface.as_ref().expect("airport has a ground layout")
    }

    pub(super) fn run_approaches(&mut self) {
        for index in 0..self.aircraft.len() {
            match self.aircraft[index].phase {
                Phase::ClearedApproach => self.step_cleared(index),
                Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff => {
                    if !self.aircraft[index].on_ground {
                        self.step_final(index)
                    }
                }
                Phase::GoAround => {
                    let check_in = self.airport.elevation + CHECK_IN_AGL;
                    let ac = &mut self.aircraft[index];
                    if ac.altitude >= check_in {
                        ac.phase = Phase::Controlled;
                    }
                }
                Phase::TowerRelease => self.step_departure(index),
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
            if along >= TOWER_HANDOFF_GATE_NM {
                return;
            }
            let staffing = self.config.staffing;
            if staffing.approach && staffing.local {
                // One controller works both: there's nobody to hand off to.
                self.aircraft[index].phase = Phase::TowerHandedOff;
            } else {
                let callsign = self.aircraft[index].callsign.clone();
                self.emit(SimEvent::LateHandoff { callsign });
                self.go_around(index, GoAroundReason::LateHandoff);
                return;
            }
        }

        if along < RUNWAY_CHECK_NM {
            if !(on_glideslope || captures_glideslope) {
                self.go_around(index, GoAroundReason::Unstable);
                return;
            }
            if !self.aircraft[index].cleared_to_land {
                self.go_around(index, GoAroundReason::NoLandingClearance);
                return;
            }
            let physical = self.surface().runway_of_end[rwy];
            if self.runway_occupied(physical, self.aircraft[index].id) {
                self.go_around(index, GoAroundReason::RunwayOccupied);
                return;
            }
        }
        if along <= 0.0 {
            self.touch_down(index);
        }
    }

    pub(super) fn touch_down(&mut self, index: usize) {
        let now = self.time;
        let elevation = self.airport.elevation;
        let ac = &mut self.aircraft[index];
        ac.altitude = elevation;
        ac.on_ground = true;
        ac.on_glideslope = false;
        ac.vertical_speed = 0.0;
        ac.phase = Phase::Rollout;
        if let Some(rwy) = ac.approach_runway {
            ac.heading = self.airport.runways[rwy].heading;
        }
        // Pattern work isn't a delay.
        let minutes = if ac.vfr.is_some() { 0.0 } else { (now - ac.spawn_time) / 60.0 };
        let callsign = ac.callsign.clone();
        self.emit(SimEvent::Landed { callsign, minutes });
        self.emergency_landed(index);
    }

    /// Does this aircraft occupy the physical runway?
    pub fn occupies(&self, ac: &Aircraft, physical: usize) -> bool {
        if !ac.on_ground {
            return false;
        }
        let map = self.surface();
        let on = |end: Option<usize>| end.is_some_and(|e| map.runway_of_end[e] == physical);
        match ac.phase {
            Phase::Rollout => on(ac.approach_runway),
            Phase::TowerRelease | Phase::LinedUp => on(ac.departure_end),
            Phase::Taxiing => ac.taxi.as_ref().is_some_and(|t| {
                map.in_zone(t.node, physical)
                    || t.next().is_some_and(|n| {
                        map.in_zone(n, physical)
                            && ac.pos.distance(map.nodes[t.node].pos) > AT_NODE_NM
                    })
            }),
            _ => false,
        }
    }

    /// Anything other than `except` on the runway or past its hold lines.
    pub fn runway_occupied(&self, physical: usize, except: u32) -> bool {
        self.aircraft.iter().any(|a| a.id != except && self.occupies(a, physical))
    }

    /// Airborne aircraft about to land on either end of the runway: IFR
    /// arrivals closer than `ifr_nm` on final, VFR closer than `vfr_nm`.
    pub(super) fn arrivals_within(&self, physical: usize, ifr_nm: f32, vfr_nm: f32) -> Vec<usize> {
        (0..self.aircraft.len())
            .filter(|&i| {
                let a = &self.aircraft[i];
                let limit = if a.vfr.is_some() { vfr_nm } else { ifr_nm };
                self.landing_distance(a, physical).is_some_and(|d| d < limit)
            })
            .collect()
    }

    /// Climbing out with Tower.
    fn step_departure(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        if ac.on_ground {
            return;
        }
        if ac.vfr.is_some() {
            self.vfr_airborne(index);
            return;
        }
        let staffing = self.config.staffing;
        let high = ac.altitude >= self.airport.elevation + CHECK_IN_AGL;
        if !staffing.local || staffing.approach {
            // The automated Tower switches it, or one controller works both.
            if high {
                self.hand_to_approach(index);
            }
            return;
        }
        let far = ac
            .departure_end
            .is_some_and(|e| self.airport.runways[e].far_end().distance(ac.pos) > DEPARTURE_HANDOFF_NM);
        if far {
            let callsign = ac.callsign.clone();
            self.emit(SimEvent::MissedHandoff { callsign, to: "Departure" });
            self.hand_to_approach(index);
        }
    }

    /// Switch a departure to Approach.
    pub(super) fn hand_to_approach(&mut self, index: usize) {
        let ac = &mut self.aircraft[index];
        ac.phase = Phase::Controlled;
        ac.freq_since = self.time;
        if ac.flow == Flow::Departure && ac.route_index < ac.route.len() {
            ac.lateral = Lateral::Route;
        }
        if !self.config.staffing.local {
            self.check_in(index, Position::Approach);
        }
    }

    /// The pilot's first call on a frequency the player works.
    pub(super) fn check_in(&mut self, index: usize, frequency: Position) {
        if !self.config.staffing.works(frequency) {
            return;
        }
        let ac = &self.aircraft[index];
        let line = match frequency {
            Position::Approach => {
                let exit =
                    ac.exit_fix.map(|f| self.airport.fixes[f].name.clone()).unwrap_or_default();
                format!(
                    "{}, airborne, climbing to {:.0}, departure via {exit}.",
                    ac.callsign, ac.target_altitude
                )
            }
            Position::Local => match self.holding_short_of(ac) {
                Some(runway) => format!("Tower, {}, holding short runway {runway}.", ac.callsign),
                None => format!("Tower, {}, with you.", ac.callsign),
            },
            Position::Ground => match ac.parking.map(|p| self.surface().parking[p].name.clone()) {
                Some(spot) => format!("Ground, {}, clear of the runway, for {spot}.", ac.callsign),
                None => format!("Ground, {}, with you.", ac.callsign),
            },
            Position::Center => return,
        };
        self.say(Speaker::Pilot, line);
    }

    fn holding_short_of(&self, ac: &Aircraft) -> Option<String> {
        let map = self.surface();
        let taxi = ac.taxi.as_ref()?;
        if let Some(r) = taxi.holding_short {
            return Some(super::clearance::spoken_runway(&self.airport, map, r));
        }
        let end = ac.departure_end?;
        (taxi.done() && map.departure_hold(end) == Some(taxi.node))
            .then(|| self.airport.runways[end].id.clone())
    }

    /// A takeoff roll has begun: space the next one, and check the runway was clear.
    pub(super) fn begin_takeoff(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(end) = ac.departure_end else { return };
        let physical = self.surface().runway_of_end[end];
        let interval = departure_interval(ac);
        self.next_departure_time = self.next_departure_time.max(self.time + interval);
        let (id, callsign) = (ac.id, ac.callsign.clone());
        if self.runway_occupied(physical, id) {
            self.emit(SimEvent::RunwaySeparation { callsign });
        }
    }

    fn at_departure_hold(&self, ac: &Aircraft) -> bool {
        let (Some(taxi), Some(end)) = (&ac.taxi, ac.departure_end) else { return false };
        let map = self.surface();
        ac.flow == Flow::Departure
            && ac.phase == Phase::Taxiing
            && taxi.done()
            && map.departure_hold(end) == Some(taxi.node)
            && ac.pos.distance(map.nodes[taxi.node].pos) < AT_NODE_NM
    }

    /// How long the runway must stay clear for a departure ready at this
    /// physical runway's hold line, seconds, if one is waiting there.
    pub(super) fn departure_window(&self, physical: usize) -> Option<f32> {
        let map = self.surface();
        self.aircraft
            .iter()
            .filter(|a| self.at_departure_hold(a) && a.departure_end.is_some_and(|e| map.runway_of_end[e] == physical))
            .map(takeoff_window)
            .max_by(f32::total_cmp)
    }

    /// A departure from this physical runway is on its way: cleared and
    /// lining up, or rolling, but not yet airborne.
    pub(super) fn departure_under_way(&self, physical: usize) -> bool {
        let map = self.surface();
        self.aircraft.iter().any(|a| {
            let mine = a.departure_end.is_some_and(|e| map.runway_of_end[e] == physical);
            let going = match a.phase {
                Phase::LinedUp | Phase::TowerRelease => true,
                // Cleared onto it: for takeoff, or (briefly, and harmlessly) across it.
                Phase::Taxiing => a.taxi.as_ref().is_some_and(|t| t.cleared.contains(&physical)),
                _ => false,
            };
            a.flow == Flow::Departure && a.on_ground && mine && going
        })
    }

    /// Taxiing clear of every runway zone, and not about to enter one.
    fn clear_of_runways(&self, ac: &Aircraft) -> bool {
        let map = self.surface();
        ac.phase == Phase::Taxiing
            && ac.taxi.as_ref().is_some_and(|t| {
                map.nodes[t.node].zones.is_empty()
                    && t.next().is_none_or(|n| map.nodes[n].zones.is_empty())
            })
    }

    /// A departure on its way to its own hold line stays with Tower after crossing.
    fn bound_for_departure_hold(&self, ac: &Aircraft) -> bool {
        let map = self.surface();
        ac.flow == Flow::Departure
            && ac.departure_end.is_some_and(|e| {
                let hold = map.departure_hold(e);
                ac.taxi.as_ref().is_some_and(|t| t.path.last().copied() == hold || t.node == hold.unwrap_or(usize::MAX))
            })
    }

    fn local_decision(&self, ac: &Aircraft, launched: bool) -> Option<Clearance> {
        if !ac.on_ground {
            if let Some(vfr) = &ac.vfr {
                return self.local_vfr_decision(ac, vfr);
            }
            let end = ac.approach_runway?;
            let close = self.airport.runways[end].along(ac.pos) < LANDING_CLEARANCE_NM;
            return (ac.phase == Phase::TowerHandedOff && !ac.cleared_to_land && close)
                .then_some(Clearance::ClearedToLand(end));
        }
        let map = self.surface();
        let taxi = ac.taxi.as_ref().filter(|_| ac.phase == Phase::Taxiing)?;
        if let Some(r) = taxi.holding_short {
            let safe = !self.runway_occupied(r, ac.id)
                && self.arrivals_within(r, CROSSING_HOLD_NM, 0.0).is_empty()
                && !self.landing_within(r, VFR_CROSSING_S, false);
            return safe.then_some(Clearance::CrossRunway(r));
        }
        if self.at_departure_hold(ac) {
            let end = ac.departure_end?;
            let r = map.runway_of_end[end];
            let safe = !launched
                && self.time >= self.next_departure_time
                && !self.runway_occupied(r, ac.id)
                && self.arrivals_within(r, SHORT_FINAL_HOLD_NM, VFR_RELEASE_NM).is_empty()
                // Airborne with a margin before anyone lands.
                && !self.landing_within(r, takeoff_window(ac), true)
                // A jet would run up the back of a light aircraft still on upwind.
                && (ac.vfr.is_some() || !self.upwind_traffic(r))
                // After a runway change, the last arrival the old way lands first.
                && !self.landing_against_flow()
                // An emergency about to land gets the runway to itself.
                && !self.emergency_inbound(r);
            return safe.then_some(Clearance::ClearedForTakeoff(end));
        }
        // Clear of the runways: over to Ground, if anyone is working it.
        let to_ground = self.config.staffing.surface_traffic()
            && self.clear_of_runways(ac)
            && !self.bound_for_departure_hold(ac);
        to_ground.then_some(Clearance::ContactGround)
    }

    /// The automated Tower, when the player isn't working Local.
    pub(super) fn run_local(&mut self) {
        if self.config.staffing.local {
            return;
        }
        let mut calls = Vec::new();
        let mut launched = false;
        for ac in &self.aircraft {
            if ac.frequency() != Position::Local || self.call_pending(ac.id) {
                continue;
            }
            if let Some(c) = self.local_decision(ac, launched) {
                launched |= matches!(c, Clearance::ClearedForTakeoff(_));
                calls.push((ac.id, c, departure_interval(ac)));
            }
        }
        for (id, clearance, interval) in calls {
            if matches!(clearance, Clearance::ClearedForTakeoff(_)) {
                // Hold the next release until this one is rolling.
                self.next_departure_time = self.time + interval;
            }
            self.issue(id, vec![clearance], Position::Local);
        }
    }

    fn ground_decision(&self, ac: &Aircraft) -> Option<Clearance> {
        let map = self.surface();
        match (ac.phase, ac.flow) {
            (Phase::Parked, Flow::Departure) => {
                let hold = map.departure_hold(ac.departure_end?)?;
                Some(Clearance::Taxi { to: hold, via: Vec::new() })
            }
            (Phase::Taxiing, flow) => {
                let taxi = ac.taxi.as_ref()?;
                if taxi.holding_short.is_some() || self.at_departure_hold(ac) {
                    return Some(Clearance::ContactTower);
                }
                let goal = match flow {
                    Flow::Departure => map.departure_hold(ac.departure_end?)?,
                    Flow::Arrival => map.parking[ac.parking?].node,
                };
                let heading_there = taxi.path.last() == Some(&goal) || taxi.node == goal;
                (!heading_there).then_some(Clearance::Taxi { to: goal, via: Vec::new() })
            }
            _ => None,
        }
    }

    /// The automated Ground, when the player isn't working it.
    pub(super) fn run_ground(&mut self) {
        if self.config.staffing.ground {
            return;
        }
        self.assign_parking();
        let mut calls = Vec::new();
        for ac in &self.aircraft {
            if !ac.on_ground || ac.frequency() != Position::Ground || self.call_pending(ac.id) {
                continue;
            }
            if let Some(c) = self.ground_decision(ac) {
                calls.push((ac.id, c));
            }
        }
        for (id, clearance) in calls {
            self.issue(id, vec![clearance], Position::Ground);
        }
    }

    /// Give arrivals without a spot the nearest free one.
    pub(super) fn assign_parking(&mut self) {
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            if ac.flow != Flow::Arrival || !ac.on_ground || ac.parking.is_some() {
                continue;
            }
            let from = super::clearance::taxi_origin(ac, self.surface());
            if let Some(spot) = self.free_parking(from) {
                self.aircraft[index].parking = Some(spot);
            }
        }
    }

    /// The closest parking spot by taxi route that nobody is in or bound for.
    /// Each runway crossing counts as a long way round.
    pub(super) fn free_parking(&self, from: usize) -> Option<usize> {
        let map = self.surface();
        (0..map.parking.len())
            .filter(|&p| {
                let at = map.nodes[map.parking[p].node].pos;
                !self.aircraft.iter().any(|a| {
                    (a.flow == Flow::Arrival && a.parking == Some(p))
                        || (a.on_ground && a.pos.distance(at) < 0.02)
                })
            })
            .filter_map(|p| {
                let route = map.route(from, map.parking[p].node, &[])?;
                let length: f32 = route
                    .nodes
                    .windows(2)
                    .map(|w| map.nodes[w[0]].pos.distance(map.nodes[w[1]].pos))
                    .sum();
                Some((length + PARKING_CROSSING_NM * route.runways_entered.len() as f32, p))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }

    /// The automated Approach in tower-only modes: departures and go-arounds
    /// leave the board once they're well clear of the field.
    pub(super) fn run_automated_approach(&mut self) {
        if self.config.staffing.approach {
            return;
        }
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            let gone = !ac.on_ground
                && ac.phase == Phase::Controlled
                && ac.pos.length() > AUTOMATED_APPROACH_RANGE_NM;
            if gone {
                let ac = self.aircraft.remove(index);
                if ac.flow == Flow::Departure {
                    self.emit(SimEvent::Exited { callsign: ac.callsign });
                }
                continue;
            }
            index += 1;
        }
    }

    /// Handoffs between separate tower positions the player forgot.
    pub(super) fn check_handoffs(&mut self) {
        let staffing = self.config.staffing;
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            if !ac.on_ground || !staffing.works(ac.frequency()) {
                continue;
            }
            let freq = ac.frequency();
            let (waiting, to, next) = if freq == Position::Local && !staffing.ground {
                let w = self.clear_of_runways(ac) && !self.bound_for_departure_hold(ac);
                (w && staffing.tower(), "Ground", Position::Ground)
            } else if freq == Position::Ground && !staffing.local {
                let w = self.at_departure_hold(ac)
                    || ac.taxi.as_ref().is_some_and(|t| t.holding_short.is_some());
                (w, "Tower", Position::Local)
            } else {
                (false, "", freq)
            };
            let now = self.time;
            let ac = &mut self.aircraft[index];
            if !waiting {
                ac.waiting_since = None;
                continue;
            }
            let since = *ac.waiting_since.get_or_insert(now);
            if now - since >= HANDOFF_GRACE_S {
                ac.surface_freq = next;
                ac.freq_since = now;
                ac.waiting_since = None;
                let callsign = ac.callsign.clone();
                self.emit(SimEvent::MissedHandoff { callsign, to });
            }
        }
    }

    /// The position the player would speak as for this instruction, if any.
    pub fn issuer(&self, ac: &Aircraft, clearance: &Clearance) -> Option<Position> {
        issuer_for(clearance, ac.frequency(), self.config.staffing)
    }
}

/// Time to hold the next departure behind this one.
fn departure_interval(ac: &Aircraft) -> f32 {
    if ac.vfr.is_some() {
        VFR_DEPARTURE_INTERVAL
    } else if ac.ty.wake == WakeCategory::Heavy {
        DEPARTURE_INTERVAL_AFTER_HEAVY
    } else {
        DEPARTURE_INTERVAL
    }
}

/// Departure ready for release; position and phase are set when it's placed.
pub(super) fn queued_departure(mut ac: Aircraft, initial_altitude: f32) -> Aircraft {
    ac.target_altitude = initial_altitude;
    ac.target_speed = ac.ty.max_speed.min(250.0);
    ac
}

/// From a takeoff clearance to airborne, with a margin before anyone may land:
/// the pilot answering, lining up, and the roll, seconds.
fn takeoff_window(ac: &Aircraft) -> f32 {
    PILOT_DELAY.1 + LINE_UP_S + ac.takeoff_roll_seconds() + TAKEOFF_MARGIN_S
}
