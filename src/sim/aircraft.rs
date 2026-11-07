//! Aircraft state and flight dynamics.

use super::airspace::Airport;
use super::autopilot;
use super::performance::{AircraftType, WakeCategory};
use super::positions::Position;
use super::units::{SECONDS_PER_HOUR, Wind, heading_diff, heading_vec, normalize_heading, tas_from_ias};
use glam::Vec2;

/// Standard rate turn, degrees per second.
pub const STANDARD_TURN_RATE: f32 = 3.0;
/// Turn rate at 25 degrees of bank is this divided by TAS in knots.
pub(crate) const BANK_LIMIT_FACTOR: f32 = 508.7;
/// Departures lift off at Vref plus this.
const ROTATE_MARGIN: f32 = 15.0;
/// Braking after touchdown, knots per second.
pub const ROLLOUT_DECEL: f32 = 4.0;
/// Speed a landing aircraft slows to before turning off, knots.
pub const EXIT_SPEED: f32 = 20.0;
const SPEED_LIMIT_ALTITUDE: f32 = 10000.0;
const SPEED_LIMIT: f32 = 250.0;
/// Light aircraft get up to speed quicker than their cruise acceleration suggests.
const LIGHT_ROLL_ACCEL: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Arrival,
    Departure,
}

/// An instruction the pilot accepted, as a controller would mark it on the
/// flight progress strip.
#[derive(Debug, Clone, PartialEq)]
pub struct StripMark {
    /// Sim time the pilot read it back.
    pub time: f32,
    /// The position that gave it.
    pub by: Position,
    pub clearance: super::clearance::Clearance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Center is offering the aircraft; the player has not accepted it.
    Offered,
    /// Talking to the player.
    Controlled,
    /// Cleared for the ILS, not yet established on the localizer.
    ClearedApproach,
    Localizer,
    Glideslope,
    TowerHandedOff,
    /// On the runway after landing.
    Rollout,
    /// Climbing out; becomes `Controlled` once safely airborne.
    GoAround,
    /// Rolling for takeoff and climbing out, with Tower until handed to Departure.
    TowerRelease,
    OfferedToCenter,
    /// At a parking spot, ready to taxi.
    Parked,
    /// Moving or holding on the surface.
    Taxiing,
    /// On the runway, waiting for a takeoff clearance.
    LinedUp,
    /// VFR, flying with Tower: in or around the traffic pattern, or leaving.
    Pattern,
}

/// Why a VFR aircraft is flying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfrIntent {
    /// Touch-and-goes in the pattern, then a full stop.
    Closed,
    Arrival,
    Departure,
}

/// Where a VFR aircraft is in or around the pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// Inbound to the 45 degree entry.
    Entry,
    /// Orbiting outside the airspace, waiting to be told how to enter.
    Holding,
    /// On the 45 to the downwind.
    Join,
    Upwind,
    Crosswind,
    Downwind,
    Base,
    Final,
    /// Leaving the area.
    Departing,
}

/// The landing a VFR aircraft is cleared for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    FullStop,
    TouchAndGo,
    /// The pilot's choice.
    Option,
}

/// What a VFR aircraft is doing and what it has been told.
#[derive(Debug, Clone, PartialEq)]
pub struct Vfr {
    pub intent: VfrIntent,
    /// The runway end it flies the pattern for.
    pub end: usize,
    pub leg: Leg,
    /// Touch-and-goes still wanted before a full stop.
    pub circuits: u32,
    /// Told how to enter the pattern. Closed traffic starts out in it.
    pub cleared_pattern: bool,
    /// Told to extend downwind until called.
    pub extend: bool,
    /// Told to turn base now.
    pub turn_base: bool,
    /// The aircraft it has been told to follow.
    pub follow: Option<u32>,
    pub landing: Option<Landing>,
    /// Departures: true bearing it leaves on.
    pub outbound: f32,
    /// Departures: told it may leave the frequency.
    pub released: bool,
    /// Made its downwind call this circuit.
    pub reported: bool,
}

impl Vfr {
    pub fn new(intent: VfrIntent, end: usize) -> Vfr {
        Vfr {
            intent,
            end,
            leg: Leg::Entry,
            circuits: 0,
            cleared_pattern: false,
            extend: false,
            turn_base: false,
            follow: None,
            landing: None,
            outbound: 0.0,
            released: false,
            reported: false,
        }
    }

    /// What the pilot would choose, given the option.
    pub fn wants_full_stop(&self) -> bool {
        self.intent != VfrIntent::Closed || self.circuits == 0
    }

    /// Will come to a full stop on this landing.
    pub fn full_stop(&self) -> bool {
        match self.landing {
            Some(Landing::FullStop) => true,
            Some(Landing::TouchAndGo) => false,
            _ => self.wants_full_stop(),
        }
    }

    /// Back on upwind with this circuit's instructions forgotten.
    pub fn reset_circuit(&mut self) {
        self.landing = None;
        self.extend = false;
        self.turn_base = false;
        self.follow = None;
        self.reported = false;
        self.leg = Leg::Upwind;
    }
}

/// Movement along the taxi graph.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Taxi {
    /// Nodes to visit. `path[index]` is the next one.
    pub path: Vec<usize>,
    pub index: usize,
    /// The node most recently passed.
    pub node: usize,
    /// Physical runways the aircraft may enter.
    pub cleared: Vec<usize>,
    /// Told to hold position.
    pub holding: bool,
    /// Physical runway the aircraft is stopped short of, waiting for a clearance.
    pub holding_short: Option<usize>,
    /// Another aircraft it has stopped for.
    pub blocked_by: Option<u32>,
    /// The route ends lined up on the departure runway.
    pub lineup: bool,
}

impl Taxi {
    pub fn at(node: usize) -> Taxi {
        Taxi { path: Vec::new(), index: 0, node, ..Default::default() }
    }

    pub fn next(&self) -> Option<usize> {
        self.path.get(self.index).copied()
    }

    /// Reached the end of its route.
    pub fn done(&self) -> bool {
        self.index >= self.path.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lateral {
    Heading(f32),
    Direct(usize),
    Route,
    Localizer,
    /// Holding at a fix.
    Hold(super::holding::HoldState),
}

#[derive(Debug, Clone)]
pub struct Aircraft {
    pub id: u32,
    pub callsign: String,
    pub flow: Flow,
    pub ty: &'static AircraftType,
    pub phase: Phase,

    pub pos: Vec2,
    pub altitude: f32,
    pub heading: f32,
    pub ias: f32,
    /// Feet per minute, positive climbing.
    pub vertical_speed: f32,
    /// Knots over the ground.
    pub ground_velocity: Vec2,
    pub on_ground: bool,

    pub lateral: Lateral,
    pub target_altitude: f32,
    pub target_speed: f32,
    pub route: Vec<usize>,
    pub route_index: usize,
    /// True heading to fly at the end of the route. Otherwise it keeps its last heading.
    pub route_end_heading: Option<f32>,

    /// Runway this aircraft is cleared to, once cleared.
    pub approach_runway: Option<usize>,
    pub on_glideslope: bool,

    pub spawn_time: f32,
    pub entered_sector: bool,
    /// Terrain warnings only apply once the aircraft has climbed above the MVA.
    pub msaw_armed: bool,
    /// Departures only: where and how high Center wants them.
    pub exit_fix: Option<usize>,
    pub exit_altitude: f32,
    /// On the surface: who it's talking to, Ground or Tower.
    pub surface_freq: Position,
    pub taxi: Option<Taxi>,
    pub cleared_to_land: bool,
    /// Line up and roll without stopping, once lined up.
    pub cleared_takeoff: bool,
    /// Departures only: the runway end it will take off from.
    pub departure_end: Option<usize>,
    /// Arrivals only: where it's parking.
    pub parking: Option<usize>,
    /// Sim time it was last handed to a new controller, for missed-handoff checks.
    pub freq_since: f32,
    /// Penalised already for a missed handoff in its current state.
    pub handoff_flagged: bool,
    /// When it started waiting for a handoff the controller hasn't given yet.
    pub waiting_since: Option<f32>,
    /// VFR aircraft only.
    pub vfr: Option<Vfr>,
    /// Something gone wrong: an emergency, declared or still to come.
    pub emergency: Option<super::emergency::Emergency>,
    /// Every instruction it has accepted, oldest first: the strip's pen marks.
    pub strip: Vec<StripMark>,
    /// IFR departures only: what it filed and what it's cleared for.
    pub flight_plan: Option<super::delivery::FlightPlan>,
    /// A pilot mistake the player hasn't sorted out yet.
    pub slip: Option<super::mistakes::Slip>,
    /// Make this mistake at the next chance: for tests and lessons.
    pub force_slip: Option<super::mistakes::Force>,
    /// Go-arounds at minimums in fog it couldn't land in.
    pub weather_go_arounds: u32,
    /// Since when it has been unable to land for the weather, seconds.
    pub minimums_since: Option<f32>,
    /// Going round a thunderstorm, or asking to.
    pub deviation: Option<super::storms::Deviation>,
}

impl Aircraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u32,
        callsign: impl Into<String>,
        flow: Flow,
        ty: &'static AircraftType,
        pos: Vec2,
        heading: f32,
        altitude: f32,
        ias: f32,
    ) -> Aircraft {
        Aircraft {
            id,
            callsign: callsign.into(),
            flow,
            ty,
            phase: Phase::Controlled,
            pos,
            altitude,
            heading: normalize_heading(heading),
            ias,
            vertical_speed: 0.0,
            ground_velocity: heading_vec(heading) * tas_from_ias(ias, altitude),
            on_ground: false,
            lateral: Lateral::Heading(normalize_heading(heading)),
            target_altitude: altitude,
            target_speed: ias,
            route: Vec::new(),
            route_index: 0,
            route_end_heading: None,
            approach_runway: None,
            on_glideslope: false,
            spawn_time: 0.0,
            entered_sector: false,
            msaw_armed: true,
            exit_fix: None,
            exit_altitude: 0.0,
            surface_freq: Position::Ground,
            taxi: None,
            cleared_to_land: false,
            cleared_takeoff: false,
            departure_end: None,
            parking: None,
            freq_since: 0.0,
            handoff_flagged: false,
            waiting_since: None,
            vfr: None,
            emergency: None,
            strip: Vec::new(),
            flight_plan: None,
            slip: None,
            force_slip: None,
            weather_go_arounds: 0,
            minimums_since: None,
            deviation: None,
        }
    }

    /// The altitude the controller assigned: what the data block shows,
    /// whatever the pilot is really climbing or descending to.
    pub fn assigned_altitude(&self) -> f32 {
        use super::mistakes::SlipKind;
        match self.slip.as_ref().map(|s| &s.kind) {
            Some(SlipKind::Altitude { assigned, .. } | SlipKind::Bust { assigned, .. }) => *assigned,
            _ => self.target_altitude,
        }
    }

    /// The heading the controller assigned, if it's flying one.
    pub fn assigned_heading(&self) -> Option<f32> {
        use super::mistakes::SlipKind;
        match (self.slip.as_ref().map(|s| &s.kind), self.lateral) {
            (Some(SlipKind::Heading { assigned, .. }), _) => Some(*assigned),
            (_, Lateral::Heading(h)) => Some(h),
            _ => None,
        }
    }

    /// A misheard altitude or heading the player let through: shown in amber
    /// until it's put right.
    pub fn missed_hearback(&self) -> Option<&super::mistakes::SlipKind> {
        self.slip.as_ref().filter(|s| s.missed && s.is_airborne()).map(|s| &s.kind)
    }

    pub fn tas(&self) -> f32 {
        tas_from_ias(self.ias, self.altitude)
    }

    pub fn ground_speed(&self) -> f32 {
        self.ground_velocity.length()
    }

    /// The controller this aircraft is talking to.
    pub fn frequency(&self) -> Position {
        match self.phase {
            Phase::Offered | Phase::OfferedToCenter => Position::Center,
            Phase::Controlled | Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope => {
                Position::Approach
            }
            Phase::TowerHandedOff
            | Phase::Rollout
            | Phase::TowerRelease
            | Phase::LinedUp
            | Phase::GoAround
            | Phase::Pattern => Position::Local,
            Phase::Parked | Phase::Taxiing => self.surface_freq,
        }
    }

    /// Acceleration on the takeoff roll, knots per second.
    pub fn roll_accel(&self) -> f32 {
        let rate = self.ty.accel.max(self.ty.decel);
        if self.ty.wake == WakeCategory::Small { rate.max(LIGHT_ROLL_ACCEL) } else { rate }
    }

    /// Seconds from starting the roll to lifting off.
    pub fn takeoff_roll_seconds(&self) -> f32 {
        (self.ty.vref + ROTATE_MARGIN) / self.roll_accel()
    }

    /// Rolling down a runway for takeoff.
    pub fn rolling(&self) -> bool {
        self.on_ground && self.phase == Phase::TowerRelease
    }

    /// Established on the final approach course.
    pub fn on_final(&self) -> bool {
        matches!(self.phase, Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff)
    }

    /// Fix the aircraft is navigating to, if any.
    pub fn active_fix(&self) -> Option<usize> {
        match self.lateral {
            Lateral::Direct(fix) => Some(fix),
            Lateral::Route => self.route.get(self.route_index).copied(),
            _ => None,
        }
    }

    /// Advance the flight model by one tick.
    pub fn step(&mut self, dt: f32, wind: Wind, airport: &Airport) {
        if self.on_ground {
            // Taxiing aircraft are moved by the world, which knows the taxi graph.
            if self.taxi.is_none() && self.phase != Phase::Parked && self.phase != Phase::LinedUp {
                self.step_ground(dt);
            }
            return;
        }

        let air = wind.vector_at(self.altitude);
        let mut desired = autopilot::desired_heading(self, air, airport, dt);
        // Going round weather: off to one side of where it means to go.
        if let Some(d) = self.deviation
            && d.approved
            && matches!(self.lateral, Lateral::Heading(_) | Lateral::Direct(_) | Lateral::Route)
        {
            desired = d.heading();
        }
        self.turn_toward(desired, dt);
        self.step_speed(dt, airport);
        self.step_vertical(dt, airport);

        self.ground_velocity = heading_vec(self.heading) * self.tas() + air;
        self.pos += self.ground_velocity / SECONDS_PER_HOUR * dt;
    }

    fn step_ground(&mut self, dt: f32) {
        let (target, rate) = if self.phase == Phase::TowerRelease {
            (self.target_speed, self.roll_accel())
        } else if self.emergency.as_ref().is_some_and(|e| e.stops_on_runway()) {
            // Disabled: brakes to a stop on the runway.
            (0.0, ROLLOUT_DECEL)
        } else {
            (EXIT_SPEED, ROLLOUT_DECEL)
        };
        self.ias = approach(self.ias, target, rate * dt);
        self.vertical_speed = 0.0;
        self.ground_velocity = heading_vec(self.heading) * self.ias;
        self.pos += self.ground_velocity / SECONDS_PER_HOUR * dt;
        if self.phase == Phase::TowerRelease && self.ias >= self.ty.vref + ROTATE_MARGIN {
            self.on_ground = false;
        }
    }

    fn turn_toward(&mut self, desired: f32, dt: f32) {
        let rate = STANDARD_TURN_RATE.min(BANK_LIMIT_FACTOR / self.tas().max(1.0));
        let turn = heading_diff(self.heading, desired).clamp(-rate * dt, rate * dt);
        self.heading = normalize_heading(self.heading + turn);
    }

    fn step_speed(&mut self, dt: f32, airport: &Airport) {
        let target = self.commanded_speed(airport);
        let rate = if target > self.ias { self.ty.accel } else { self.ty.decel };
        self.ias = approach(self.ias, target, rate * dt);
    }

    /// Assigned speed after the speed limit and the approach speed schedule.
    pub fn commanded_speed(&self, airport: &Airport) -> f32 {
        let mut speed = self.target_speed.min(self.ty.max_speed);
        if let Some(cap) = self.emergency_speed_cap() {
            speed = speed.min(cap);
        }
        if self.altitude < SPEED_LIMIT_ALTITUDE {
            speed = speed.min(SPEED_LIMIT);
        }
        if let Lateral::Hold(_) = self.lateral {
            speed = speed.min(super::holding::max_speed(self.altitude));
        }
        if let (Lateral::Localizer, Some(rwy)) = (self.lateral, self.approach_runway) {
            let along = airport.runways[rwy].along(self.pos);
            let schedule = if along > 10.0 {
                180.0
            } else if along > 5.0 {
                self.ty.vref + 25.0
            } else {
                self.ty.vref + 5.0
            };
            speed = speed.min(schedule);
        }
        speed
    }

    fn step_vertical(&mut self, dt: f32, airport: &Airport) {
        let minutes = dt / 60.0;
        if self.on_glideslope
            && let Some(rwy) = self.approach_runway {
                let along = airport.runways[rwy].along(self.pos);
                let desired = airport.glideslope_altitude(rwy, along);
                let limit = self.ty.descent_fpm * minutes;
                let change = (desired - self.altitude).clamp(-limit, limit);
                self.altitude += change;
                self.vertical_speed = change / minutes;
                return;
            }

        let diff = self.target_altitude - self.altitude;
        // Climb performance falls away with altitude.
        let rate = if diff > 0.0 {
            self.emergency_climb(self.ty.climb_fpm * (1.0 - self.altitude.max(0.0) / 60000.0))
        } else {
            self.emergency_descent(self.ty.descent_fpm)
        };
        let limit = rate * minutes;
        let change = diff.clamp(-limit, limit);
        self.altitude += change;
        self.vertical_speed = change / minutes;
    }

    /// Abandon the approach: fly runway heading and climb to the published altitude.
    pub fn start_go_around(&mut self, airport: &Airport) {
        // VFR: climb straight ahead back into the pattern.
        if let Some(vfr) = self.vfr.as_mut() {
            let heading = airport.runways[vfr.end].heading;
            vfr.reset_circuit();
            self.phase = Phase::Pattern;
            self.lateral = Lateral::Heading(heading);
            self.target_altitude = airport.elevation + super::vfr::PATTERN_AGL;
            self.target_speed = super::vfr::pattern_speed(self.ty);
            self.approach_runway = None;
            self.on_glideslope = false;
            return;
        }
        let course = self
            .approach_runway
            .map(|r| airport.runways[r].heading)
            .unwrap_or(self.heading);
        self.phase = Phase::GoAround;
        self.lateral = Lateral::Heading(course);
        self.target_altitude = airport.go_around_altitude;
        self.target_speed = self.ty.max_speed.min(210.0);
        self.approach_runway = None;
        self.on_glideslope = false;
        self.msaw_armed = false;
    }
}

fn approach(value: f32, target: f32, max_step: f32) -> f32 {
    value + (target - value).clamp(-max_step, max_step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::performance::aircraft_type;

    fn jet(heading: f32, altitude: f32, ias: f32) -> Aircraft {
        Aircraft::new(
            1,
            "TST1",
            Flow::Arrival,
            aircraft_type("B738").unwrap(),
            Vec2::ZERO,
            heading,
            altitude,
            ias,
        )
    }

    fn run(ac: &mut Aircraft, seconds: f32, wind: Wind) {
        let airport = Airport::default_airport();
        for _ in 0..(seconds / 0.25) as usize {
            ac.step(0.25, wind, &airport);
        }
    }

    #[test]
    fn turns_at_standard_rate_the_short_way() {
        // Slow enough that the bank limit doesn't reduce the rate.
        let mut ac = jet(350.0, 3000.0, 150.0);
        ac.target_speed = 150.0;
        ac.lateral = Lateral::Heading(50.0);
        run(&mut ac, 10.0, Wind::CALM);
        // 10 s at 3 deg/s, turning right through north.
        assert!((ac.heading - 20.0).abs() < 0.01, "{}", ac.heading);
        run(&mut ac, 20.0, Wind::CALM);
        assert!((ac.heading - 50.0).abs() < 0.01, "{}", ac.heading);
    }

    #[test]
    fn fast_aircraft_turn_slower_than_standard_rate() {
        let mut ac = jet(0.0, 14000.0, 300.0);
        ac.lateral = Lateral::Heading(90.0);
        run(&mut ac, 10.0, Wind::CALM);
        assert!(ac.heading > 10.0 && ac.heading < 25.0, "{}", ac.heading);
    }

    #[test]
    fn descends_at_type_rate_and_levels_off() {
        let mut ac = jet(0.0, 10000.0, 240.0);
        ac.target_altitude = 8000.0;
        run(&mut ac, 30.0, Wind::CALM);
        assert!((ac.altitude - 8900.0).abs() < 1.0, "{}", ac.altitude);
        assert!(ac.vertical_speed < 0.0);
        run(&mut ac, 60.0, Wind::CALM);
        assert_eq!(ac.altitude, 8000.0);
        assert_eq!(ac.vertical_speed, 0.0);
    }

    #[test]
    fn climb_rate_drops_with_altitude() {
        let mut low = jet(0.0, 1000.0, 240.0);
        low.target_altitude = 30000.0;
        let mut high = jet(0.0, 15000.0, 240.0);
        high.target_altitude = 30000.0;
        run(&mut low, 4.0, Wind::CALM);
        run(&mut high, 4.0, Wind::CALM);
        assert!(low.vertical_speed > high.vertical_speed);
    }

    #[test]
    fn speed_limit_below_10000_overrides_assignment() {
        let mut ac = jet(0.0, 9000.0, 300.0);
        ac.target_speed = 300.0;
        run(&mut ac, 60.0, Wind::CALM);
        assert_eq!(ac.ias, 250.0);
    }

    #[test]
    fn wind_drifts_an_aircraft_on_a_fixed_heading() {
        let mut ac = jet(0.0, 3000.0, 200.0);
        run(&mut ac, 360.0, Wind { from: 270.0, speed: 30.0 });
        // Heading north with wind from the west: 6 minutes at 36 kt drifts about 3.6 NM east.
        assert!(ac.pos.x > 3.0 && ac.pos.x < 4.0, "{}", ac.pos.x);
        assert!(ac.pos.y > 20.0);
    }

    #[test]
    fn true_airspeed_exceeds_indicated_at_altitude() {
        let ac = jet(0.0, 10000.0, 250.0);
        assert_eq!(ac.tas(), 300.0);
    }
}
