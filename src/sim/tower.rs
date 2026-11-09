//! Local control: approach gates, landings, runway occupancy and takeoffs, plus
//! the automated Tower, Ground and Approach positions for whatever the player
//! isn't working.

use super::aircraft::{Aircraft, Flow, Lateral, Leg, Phase};
use super::performance::{Cwt, WakeCategory};
use super::approach::ApproachKind;
use super::autopilot::should_capture_localizer;
use super::clearance::{AT_NODE_NM, Clearance, PILOT_DELAY, issuer_for};
use super::ground_map::{GroundMap, Parking, StandUse};
use super::positions::{Position, SOLO, Seat};
use super::scoring::{GoAroundReason, SimEvent};
use super::traffic::airline_of;
use super::{Speaker, World};

/// Must be with Tower by this distance on final.
pub const TOWER_HANDOFF_GATE_NM: f32 = 5.0;
/// Tower checks the runway and landing clearance at this distance.
pub(super) const RUNWAY_CHECK_NM: f32 = 1.0;
/// A visual approach joins final closer in, so it can be with Tower later, NM.
const VISUAL_HANDOFF_GATE_NM: f32 = 2.0;
/// At the runway check, a visual approach this far above the glidepath or
/// off the centreline is unstable.
const VISUAL_TOO_HIGH_FT: f32 = 300.0;
const VISUAL_OFF_CENTRELINE_NM: f32 = 0.2;
/// Departures and go-arounds go to Approach above this height over the field.
const CHECK_IN_AGL: f32 = 1500.0;
/// A departure turns to Tower's heading above this height, feet.
const TOWER_TURN_AGL: f32 = 400.0;
/// The automated Tower clears arrivals to land inside this distance.
const LANDING_CLEARANCE_NM: f32 = 8.0;
/// Tower never launches a departure in front of an arrival closer than this,
/// whatever its speed.
const SHORT_FINAL_HOLD_NM: f32 = 2.5;
/// Or send an aircraft across a runway with an arrival closer than this.
pub(super) const CROSSING_HOLD_NM: f32 = 5.0;
/// Minimum time between departures, seconds.
/// The automated Tower's pace between IFR departures, seconds. Wake
/// turbulence intervals come on top (`wake_wait`).
pub const DEPARTURE_INTERVAL: f32 = 90.0;
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
pub(super) const VFR_CROSSING_S: f32 = 75.0;
/// A departure must be airborne this long before the next arrival lands, seconds.
const TAKEOFF_MARGIN_S: f32 = 15.0;
/// Time from the hold line to starting the roll, seconds.
const LINE_UP_S: f32 = 30.0;
/// Minimum time between a VFR departure and the next, seconds.
const VFR_DEPARTURE_INTERVAL: f32 = 45.0;
/// 7110.65 3-9-4 NOTE: no more than this long in position without further
/// instructions, seconds.
pub const LINE_UP_MAX_S: f32 = 90.0;
/// 7110.65 3-9-4: an aircraft told to line up and wait hears about the
/// closest arrival to its runway within this many flying miles.
pub const LINE_UP_TRAFFIC_NM: f32 = 6.0;
/// 7110.65 3-9-4g, 3-10-5: even with safety logic, no landing clearance with
/// traffic in position below this ceiling (feet) or visibility (SM).
const SAFETY_LOGIC_CEILING_FT: f32 = 800.0;
const SAFETY_LOGIC_VISIBILITY_SM: f32 = 2.0;

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
        self.space_visual_followers();
    }


    fn step_cleared(&mut self, index: usize) {
        let ac = &mut self.aircraft[index];
        let Some(rwy) = ac.approach_runway else { return };
        let runway = &self.airport.runways[rwy];
        // On the RNAV's fixes, or just past the last of them: the autopilot
        // drops out of route mode at the end of the route.
        let faf = self.airport.rnav(rwy).and_then(|a| self.airport.fix_index(a.faf()));
        let past_faf = faf.and_then(|f| ac.route.iter().position(|&r| r == f)).is_some_and(|i| ac.route_index > i);
        if ac.approach == ApproachKind::Rnav && (ac.lateral == Lateral::Route || past_faf) {
            self.step_rnav_fixes(index, rwy);
            return;
        }
        if matches!(ac.lateral, Lateral::Visual(_)) {
            if should_capture_localizer(ac, runway) {
                ac.lateral = Lateral::Localizer;
                ac.phase = Phase::Localizer;
            } else if runway.along(ac.pos) < RUNWAY_CHECK_NM {
                self.go_around(index, GoAroundReason::Unstable);
            }
            return;
        }
        if should_capture_localizer(ac, runway) {
            ac.lateral = Lateral::Localizer;
            ac.phase = Phase::Localizer;
        } else if runway.along(ac.pos) < RUNWAY_CHECK_NM {
            // Flew past without ever establishing: the clearance is void.
            ac.phase = Phase::Controlled;
            ac.approach_runway = None;
            let line = format!("{}, missed the localizer, request vectors.", ac.callsign);
            self.pilot_say(Speaker::Pilot, self.aircraft[index].callsign.clone(), line);
        }
    }

    /// On an RNAV approach from a fix: down to each fix's altitude in turn,
    /// never climbing, and onto the final course once past the final
    /// approach fix.
    fn step_rnav_fixes(&mut self, index: usize, rwy: usize) {
        let airport = &self.airport;
        let ac = &mut self.aircraft[index];
        let faf = airport.rnav(rwy).and_then(|a| airport.fix_index(a.faf()));
        let past_faf = faf
            .and_then(|f| ac.route.iter().position(|&r| r == f))
            .is_none_or(|i| ac.route_index > i);
        if past_faf {
            ac.lateral = Lateral::Localizer;
            ac.phase = Phase::Localizer;
            return;
        }
        let Some(floor) = ac.active_fix().and_then(|f| airport.rnav_altitude(rwy, f)) else { return };
        ac.target_altitude = ac.target_altitude.min(floor);
    }

    fn step_final(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(rwy) = ac.approach_runway else { return };
        let runway = &self.airport.runways[rwy];
        let along = runway.along(ac.pos);
        // Making its own way onto final, after the handoff to Tower.
        if let Lateral::Visual(_) = ac.lateral {
            if should_capture_localizer(ac, runway) {
                self.aircraft[index].lateral = Lateral::Localizer;
            } else if along < RUNWAY_CHECK_NM {
                self.go_around(index, GoAroundReason::Unstable);
                return;
            }
        }
        let ac = &self.aircraft[index];
        let gs_altitude = self.airport.approach_altitude(rwy, along, ac.approach);
        let phase = ac.phase;
        let on_glideslope = ac.on_glideslope;
        let joined = ac.lateral == Lateral::Localizer;
        let visual = ac.approach == ApproachKind::Visual;
        let captures_glideslope = (joined || !visual) && !on_glideslope && ac.altitude + 20.0 >= gs_altitude;
        // Too high, or not lined up, on a visual: unstable.
        let unstable_visual = visual
            && (!joined
                || ac.altitude > gs_altitude + VISUAL_TOO_HIGH_FT
                || runway.cross(ac.pos).abs() > VISUAL_OFF_CENTRELINE_NM);

        if captures_glideslope {
            let ac = &mut self.aircraft[index];
            ac.on_glideslope = true;
            if ac.phase == Phase::Localizer {
                ac.phase = Phase::Glideslope;
            }
        }

        if phase != Phase::TowerHandedOff {
            let gate = if visual { VISUAL_HANDOFF_GATE_NM } else { TOWER_HANDOFF_GATE_NM };
            if along >= gate {
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

        if along < super::visibility::DECISION_NM && self.below_minimums(index) {
            self.minimums_go_around(index);
            return;
        }
        if along < RUNWAY_CHECK_NM {
            if !(on_glideslope || captures_glideslope) || unstable_visual {
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
        // Tower's heading, flown once clear of the ground.
        if ac.tower_heading.is_some() && ac.altitude >= self.airport.elevation + TOWER_TURN_AGL {
            let ac = &mut self.aircraft[index];
            ac.lateral = Lateral::Heading(ac.tower_heading.take().expect("checked"));
        }
        let ac = &self.aircraft[index];
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
        super::clearance::on_course(ac, &self.airport, self.config.staffing.approach);
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
            Position::Ground => match (self.holding_short_of(ac), self.taxi_in_name(ac)) {
                (Some(runway), Some(spot)) => {
                    format!("Ground, {}, holding short runway {runway}, for {spot}.", ac.callsign)
                }
                (Some(runway), None) => format!("Ground, {}, holding short runway {runway}.", ac.callsign),
                (None, Some(spot)) => format!("Ground, {}, clear of the runway, for {spot}.", ac.callsign),
                (None, None) => format!("Ground, {}, with you.", ac.callsign),
            },
            Position::Center | Position::Delivery | Position::Ramp => return,
        };
        self.pilot_say(Speaker::Pilot, self.aircraft[index].callsign.clone(), line);
    }

    fn holding_short_of(&self, ac: &Aircraft) -> Option<String> {
        let map = self.surface();
        let taxi = ac.taxi.as_ref()?;
        if let Some(r) = taxi.holding_short {
            return Some(super::clearance::spoken_runway(&self.airport, map, r, &self.ends_in_use()));
        }
        let end = ac.departure_end?;
        (taxi.done() && map.departure_hold(end) == Some(taxi.node))
            .then(|| self.airport.runways[end].id.clone())
    }

    /// A takeoff roll has begun: space the next one, and check the runway
    /// was clear and the wake turbulence interval had passed.
    pub(super) fn begin_takeoff(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(end) = ac.departure_end else { return };
        let physical = self.surface().runway_of_end[end];
        let interval = departure_interval(ac);
        self.next_departure_time = self.next_departure_time.max(self.time + interval);
        let (id, callsign) = (ac.id, ac.callsign.clone());
        let wake = self.wake_wait(ac);
        if self.runway_occupied(physical, id) {
            self.emit(SimEvent::RunwaySeparation { callsign: callsign.clone() });
        }
        if let Some((_, leader)) = wake {
            self.emit(SimEvent::WakeDeparture { leader, follower: callsign });
        }
        self.record_takeoff(index);
    }

    /// Remember a takeoff roll for the wake intervals behind it.
    pub(super) fn record_takeoff(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(end) = ac.departure_end else { return };
        let takeoff = Takeoff { end, time: self.time, cwt: ac.ty.cwt, callsign: ac.callsign.clone() };
        let now = self.time;
        self.takeoffs.retain(|t| now - t.time < TAKEOFF_MEMORY_S);
        self.takeoffs.push(takeoff);
    }

    /// A departure still waiting out a wake turbulence interval behind an
    /// earlier takeoff from its runway, or one less than 2,500 feet away
    /// (JO 7110.65 3-9-6): the seconds left, and who it's behind.
    pub fn wake_wait(&self, ac: &Aircraft) -> Option<(f32, String)> {
        let end = ac.departure_end?;
        self.takeoffs
            .iter()
            .filter(|t| t.callsign != ac.callsign && self.airport.same_lane(t.end, end))
            .filter_map(|t| {
                let interval = super::performance::wake_interval_s(t.cwt, ac.ty.cwt)?;
                let left = t.time + interval - self.time;
                (left > 0.0).then(|| (left, t.callsign.clone()))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The caution a takeoff or landing clearance carries behind an aircraft
    /// that leaves wake on the same runway (JO 7110.65 3-10-3).
    pub(super) fn wake_caution(&self, ac: &Aircraft, clearance: &Clearance) -> bool {
        use super::performance::wake_advisory;
        let now = self.time;
        let recent = |end: usize| {
            self.takeoffs.iter().any(|t| {
                t.callsign != ac.callsign && self.airport.same_lane(t.end, end) && now - t.time < 120.0 && wake_advisory(t.cwt, ac.ty.cwt)
            })
        };
        match *clearance {
            Clearance::ClearedForTakeoff(end) => recent(end),
            Clearance::ClearedToLand(end) => {
                let along = self.airport.runways[end].along(ac.pos);
                // The arrival ahead on the same final, or one just landed.
                let ahead = self.aircraft.iter().any(|a| {
                    a.id != ac.id
                        && a.flow == Flow::Arrival
                        && a.approach_runway.is_some_and(|r| self.airport.same_lane(r, end))
                        && self.airport.runways[end].along(a.pos) < along
                        && wake_advisory(a.ty.cwt, ac.ty.cwt)
                });
                ahead || recent(end)
            }
            _ => false,
        }
    }

    /// A departure holding in position on this physical runway, or cleared
    /// to line up and wait and taxiing there, other than `except`.
    pub fn holding_in_position(&self, physical: usize, except: u32) -> Option<&Aircraft> {
        let map = self.surface();
        self.aircraft.iter().find(|a| {
            let mine = a.departure_end.is_some_and(|e| map.runway_of_end[e] == physical);
            let waiting = match a.phase {
                Phase::LinedUp => true,
                Phase::Taxiing => a.taxi.as_ref().is_some_and(|t| t.lineup),
                _ => false,
            };
            a.id != except && a.flow == Flow::Departure && a.on_ground && mine && waiting && !a.cleared_takeoff
        })
    }

    /// An airborne aircraft cleared to land, touch and go or for the option
    /// on this physical runway, other than `except`.
    pub fn cleared_to_land_on(&self, physical: usize, except: u32) -> Option<&Aircraft> {
        let map = self.surface();
        self.aircraft.iter().find(|a| {
            let end = match &a.vfr {
                Some(v) => v.landing.is_some().then_some(v.end),
                None => a.approach_runway.filter(|_| a.cleared_to_land),
            };
            a.id != except && !a.on_ground && end.is_some_and(|e| map.runway_of_end[e] == physical)
        })
    }

    /// Safety logic in full core alert, in good enough weather, lets a
    /// landing clearance and line up and wait share a runway (3-9-4g).
    fn safety_logic_on(&self) -> bool {
        self.airport.safety_logic && !self.sky().below(SAFETY_LOGIC_CEILING_FT, SAFETY_LOGIC_VISIBILITY_SM)
    }

    /// The line up and wait rule (7110.65 3-9-4, 3-9-6, 3-10-5) this
    /// clearance would break, said as the score shows it.
    pub(super) fn line_up_breach(&self, ac: &Aircraft, clearance: &Clearance) -> Option<String> {
        let map = self.surface();
        let cs = &ac.callsign;
        match *clearance {
            Clearance::LineUpAndWait(end) if !(ac.phase == Phase::LinedUp && ac.departure_end == Some(end)) => {
                let physical = map.runway_of_end[end];
                let rwy = &self.airport.runways[end].id;
                if !self.safety_logic_on()
                    && let Some(other) = self.cleared_to_land_on(physical, ac.id)
                {
                    return Some(format!("{cs} onto runway {rwy} with {} cleared to land", other.callsign));
                }
                // 3-9-6: a small aircraft doesn't wait out a super's or a
                // heavy's wake interval on the runway.
                if ac.ty.wake == WakeCategory::Small
                    && let Some(leader) = self.wake_leader(ac, end, |c| c <= Cwt::D)
                {
                    return Some(format!("{cs} lined up behind departing heavy {leader}"));
                }
                None
            }
            Clearance::ClearedToLand(end) | Clearance::ClearedTouchAndGo(end) | Clearance::ClearedOption(end) => {
                let physical = map.runway_of_end[end];
                let other = self.holding_in_position(physical, ac.id).filter(|_| !self.safety_logic_on())?;
                let rwy = &self.airport.runways[end].id;
                Some(format!("{cs} cleared to land on {rwy} with {} in position", other.callsign))
            }
            _ => None,
        }
    }

    /// A takeoff from this runway whose wake interval `ac` is still inside,
    /// by a leader whose category passes `which`.
    fn wake_leader(&self, ac: &Aircraft, end: usize, which: impl Fn(Cwt) -> bool) -> Option<String> {
        self.takeoffs
            .iter()
            .filter(|t| t.callsign != ac.callsign && self.airport.same_lane(t.end, end) && which(t.cwt))
            .find(|t| {
                super::performance::wake_interval_s(t.cwt, ac.ty.cwt).is_some_and(|i| t.time + i > self.time)
            })
            .map(|t| t.callsign.clone())
    }

    /// The traffic a line up and wait or landing clearance carries
    /// (7110.65 3-9-4, 3-10-5).
    pub(super) fn line_up_traffic(&self, ac: &Aircraft, clearance: &Clearance) -> Option<String> {
        let map = self.surface();
        match *clearance {
            Clearance::LineUpAndWait(end) if ac.phase != Phase::LinedUp => {
                let physical = map.runway_of_end[end];
                // The closest arrival to the runway within 6 flying miles.
                let (miles, arrival) = self
                    .aircraft
                    .iter()
                    .filter(|a| !a.on_ground && a.id != ac.id)
                    .filter_map(|a| {
                        let landing = match &a.vfr {
                            Some(v) => matches!(v.leg, Leg::Base | Leg::Final).then_some(v.end),
                            None => a.approach_runway.filter(|_| a.flow == Flow::Arrival),
                        }?;
                        let runway = &self.airport.runways[landing];
                        let miles = runway.along(a.pos);
                        let on_final = runway.cross(a.pos).abs() < 2.0;
                        (map.runway_of_end[landing] == physical && on_final && miles > 0.0 && miles <= LINE_UP_TRAFFIC_NM)
                            .then_some((miles, a))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))?;
                Some(format!("traffic, {}, {} mile final", arrival.ty.designator, miles.round().max(1.0)))
            }
            Clearance::ClearedToLand(end) | Clearance::ClearedTouchAndGo(end) | Clearance::ClearedOption(end) => {
                self.holding_in_position(map.runway_of_end[end], ac.id).map(|_| "traffic holding in position".to_string())
            }
            _ => None,
        }
    }

    /// An aircraft in position 90 seconds with nothing more said calls the
    /// tower about it (7110.65 3-9-4 NOTE).
    pub(super) fn check_line_up_time(&mut self) {
        if !self.config.staffing.local {
            return;
        }
        let now = self.time;
        let mut calls = Vec::new();
        for ac in &mut self.aircraft {
            let waiting = ac.phase == Phase::LinedUp && !ac.cleared_takeoff;
            if let Some(since) = ac.in_position_since
                && waiting
                && now - since > LINE_UP_MAX_S
            {
                ac.in_position_since = None;
                let end = ac.departure_end.map(|e| self.airport.runways[e].id.clone()).unwrap_or_default();
                calls.push((ac.callsign.clone(), end));
            }
        }
        for (callsign, end) in calls {
            self.pilot_say(Speaker::Pilot, callsign.clone(), format!("Tower, {callsign}, still holding in position runway {end}."));
            self.emit(SimEvent::LongLineUp { callsign });
        }
    }

    pub fn at_departure_hold(&self, ac: &Aircraft) -> bool {
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
            if self.crossing_blocker(r, ac.id).is_none() {
                return Some(Clearance::CrossRunway(r));
            }
            // An arrival stuck between runways goes over to the player on
            // Ground, and Tower calls back with the crossing.
            let staffing = self.config.staffing;
            let to_player = staffing.ground && ac.flow == Flow::Arrival;
            return to_player.then_some(Clearance::ContactGround);
        }
        if self.at_departure_hold(ac) {
            let end = ac.departure_end?;
            let r = map.runway_of_end[end];
            // Low visibility: the arrival's ILS critical area stays clear.
            let short_final =
                if self.sky().lvp() { super::visibility::LVP_RELEASE_NM } else { SHORT_FINAL_HOLD_NM };
            let safe = !launched
                && self.time >= self.next_departure_time
                && self.wake_wait(ac).is_none()
                && !self.runway_occupied(r, ac.id)
                // Ground has been promised a crossing.
                && !self.crossing_reserved(r)
                && self.arrivals_within(r, short_final, VFR_RELEASE_NM).is_empty()
                // Airborne with a margin before anyone lands.
                && !self.landing_within(r, takeoff_window(ac), true)
                // A jet would run up the back of a light aircraft still on upwind.
                && (ac.vfr.is_some() || !self.upwind_traffic(r))
                // After a runway change, the last arrival the old way lands first.
                && !self.landing_against_flow()
                // An emergency about to land gets the runway to itself.
                && !self.emergency_inbound(r)
                // On call for release: not until Approach has released it.
                && !self.needs_release(ac.id);
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
        let mut releases = Vec::new();
        let mut launched = false;
        for ac in &self.aircraft {
            if ac.frequency() != Position::Local || self.call_pending(ac.id) {
                continue;
            }
            // Departures on call for release: ask Approach at the hold line.
            if self.at_departure_hold(ac) && self.needs_release(ac.id) && self.coordination_for(ac.id).is_none() {
                releases.extend(ac.departure_end.map(|end| (ac.id, end)));
            }
            if let Some(c) = self.local_decision(ac, launched) {
                launched |= matches!(c, Clearance::ClearedForTakeoff(_));
                calls.push((ac.id, c, departure_interval(ac)));
            }
        }
        for (id, end) in releases {
            let kind = super::coordination::CoordKind::Release { end };
            self.ask(kind, id, Position::Local, Position::Approach);
        }
        for (id, clearance, interval) in calls {
            if matches!(clearance, Clearance::ClearedForTakeoff(_)) {
                // Hold the next release until this one is rolling.
                self.next_departure_time = self.time + interval;
            }
            let holding_short = self.find(id).and_then(|a| a.taxi.as_ref()).and_then(|t| t.holding_short);
            if let (Clearance::ContactGround, Some(r)) = (&clearance, holding_short) {
                self.hand_over_holding_short(id, r);
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
                    Flow::Arrival => self.taxi_in_goal(ac)?,
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
        let mut asks = Vec::new();
        for ac in &self.aircraft {
            if !ac.on_ground || ac.frequency() != Position::Ground || self.call_pending(ac.id) {
                continue;
            }
            // The player works Local: Ground asks before crossing, rather
            // than handing the aircraft over.
            let short = ac.taxi.as_ref().filter(|_| ac.phase == Phase::Taxiing).and_then(|t| t.holding_short);
            if let Some(r) = short.filter(|_| self.config.staffing.local) {
                if self.crossing_approved(ac.id, r) {
                    calls.push((ac.id, Clearance::CrossRunway(r)));
                } else if self.coordination_for(ac.id).is_none() {
                    asks.push((ac.id, r));
                }
                continue;
            }
            if let Some(c) = self.ground_decision(ac) {
                calls.push((ac.id, c));
            }
        }
        for (id, runway) in asks {
            self.ask(super::coordination::CoordKind::Crossing { runway }, id, Position::Ground, Position::Local);
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
            if let Some(spot) = self.pick_parking(ac, from) {
                self.aircraft[index].parking = Some(spot);
            }
        }
    }

    /// The kind of stand this aircraft parks at: general aviation at an FBO,
    /// cargo airlines on the ramp, the rest at a gate.
    pub(super) fn stand_for(&self, ac: &Aircraft) -> StandUse {
        let Some(airline) = airline_of(&ac.callsign).filter(|_| ac.vfr.is_none()) else {
            return StandUse::Ga;
        };
        let cargo = self.airport.traffic.as_ref().is_some_and(|t| t.cargo_airlines.iter().any(|c| c == airline));
        if cargo { StandUse::Ramp } else { StandUse::Gate }
    }

    /// The nearest free parking spot to `from` meant for `stand`; failing
    /// that, a general aviation aircraft takes a ramp spot, then anyone takes
    /// whatever is free.
    pub(super) fn free_parking(&self, from: usize, stand: StandUse) -> Option<usize> {
        let mut wanted = vec![Some(stand)];
        if stand == StandUse::Ga {
            wanted.push(Some(StandUse::Ramp));
        }
        wanted.push(None);
        wanted.into_iter().find_map(|kind| {
            self.free_parking_where(from, |p| kind.is_none() || p.stand == kind).first().copied()
        })
    }

    /// Free parking spots that pass `wanted`, closest first by taxi route from
    /// `from`: nobody in them or bound for them. Each runway crossing counts as
    /// a long way round.
    pub(super) fn free_parking_where(&self, from: usize, wanted: impl Fn(&Parking) -> bool) -> Vec<usize> {
        let map = self.surface();
        let mut free: Vec<(f32, usize)> = (0..map.parking.len())
            .filter(|&p| wanted(&map.parking[p]))
            .filter(|&p| {
                let at = map.nodes[map.parking[p].node].pos;
                !self.aircraft.iter().any(|a| a.parking == Some(p) || (a.on_ground && a.pos.distance(at) < 0.02))
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
            .collect();
        free.sort_by(|a, b| a.0.total_cmp(&b.0));
        free.into_iter().map(|(_, p)| p).collect()
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
            // Arrivals it's holding stay; anything else this far out is gone.
            let gone = !ac.on_ground
                && ac.phase == Phase::Controlled
                && !matches!(ac.lateral, Lateral::Hold(_))
                // Not until a wrong code Delivery let through has been reset.
                && super::delivery::wrong_code_showing(ac).is_none()
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
            } else if freq == Position::Delivery && !staffing.ground {
                // Cleared, and still on the player's Delivery frequency.
                let cleared = ac.flight_plan.as_ref().is_some_and(|p| p.is_cleared());
                (cleared && !self.call_pending(ac.id), "Ground", Position::Ground)
            } else if freq == Position::Ground && !staffing.local {
                // Holding short while Ground works the crossing out with
                // Tower on the landline isn't a forgotten handoff.
                let w = self.at_departure_hold(ac)
                    || (ac.taxi.as_ref().is_some_and(|t| t.holding_short.is_some())
                        && self.coordination_for(ac.id).is_none());
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
        self.issuer_as(SOLO, ac, clearance)
    }

    /// The position the player in `seat` would speak as, if any.
    pub fn issuer_as(&self, seat: Seat, ac: &Aircraft, clearance: &Clearance) -> Option<Position> {
        issuer_for(clearance, ac.frequency(), self.config.staffing, seat)
    }
}

/// Time to hold the next departure behind this one.
pub(super) fn departure_interval(ac: &Aircraft) -> f32 {
    if ac.vfr.is_some() { VFR_DEPARTURE_INTERVAL } else { DEPARTURE_INTERVAL }
}

/// A takeoff roll that began, for wake turbulence intervals behind it.
#[derive(Debug, Clone)]
pub(super) struct Takeoff {
    pub end: usize,
    pub time: f32,
    pub cwt: super::performance::Cwt,
    pub callsign: String,
}

/// Takeoffs are remembered this long, seconds: the longest wake interval.
const TAKEOFF_MEMORY_S: f32 = 180.0;

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
