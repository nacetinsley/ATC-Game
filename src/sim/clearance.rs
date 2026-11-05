//! Clearances: what the controller says, and how the pilot responds.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::airspace::Airport;
use super::scoring::{GoAroundReason, SimEvent};
use super::units::{format_heading, heading_diff, normalize_heading};
use super::{Speaker, World};

pub const MIN_ASSIGNED_ALTITUDE: f32 = 1000.0;
pub const MAX_ASSIGNED_ALTITUDE: f32 = 17000.0;
pub const MAX_INTERCEPT_DEG: f32 = 30.0;
/// Pilot response time range, seconds.
const PILOT_DELAY: (f32, f32) = (2.0, 6.0);
/// Below this distance there is no room left to establish and hand to Tower.
const MIN_APPROACH_CLEARANCE_NM: f32 = 6.0;
const MAX_APPROACH_CLEARANCE_NM: f32 = 35.0;
/// How far above the glideslope an aircraft can be and still capture it from below.
const ABOVE_GLIDESLOPE_TOLERANCE: f32 = 200.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Clearance {
    Heading(f32),
    Altitude(f32),
    Speed(f32),
    DirectFix(usize),
    ResumeRoute,
    ClearedIls(usize),
    GoAround,
    HandoffTower,
    HandoffCenter,
}

/// A radio call waiting for the pilot to respond.
#[derive(Debug, Clone)]
pub(super) struct RadioCall {
    pub due: f32,
    pub aircraft: u32,
    pub clearances: Vec<Clearance>,
}

fn thousands(altitude: f32) -> String {
    let a = altitude.round() as i32;
    if a >= 1000 {
        format!("{},{:03}", a / 1000, a % 1000)
    } else {
        a.to_string()
    }
}

/// The controller's wording, as sent. Decided from the aircraft's state at transmission.
pub fn phrase(clearance: &Clearance, ac: &Aircraft, airport: &Airport) -> String {
    match clearance {
        Clearance::Heading(h) => {
            let dir = if heading_diff(ac.heading, *h) >= 0.0 { "right" } else { "left" };
            format!("turn {dir} heading {}", format_heading(airport.to_magnetic(*h)))
        }
        Clearance::Altitude(a) => {
            let verb = if *a > ac.altitude { "climb" } else { "descend" };
            format!("{verb} and maintain {}", thousands(*a))
        }
        Clearance::Speed(s) => format!("maintain {s:.0} knots"),
        Clearance::DirectFix(f) => format!("direct {}", airport.fixes[*f].name),
        Clearance::ResumeRoute => "resume own navigation".into(),
        Clearance::ClearedIls(r) => format!("cleared ILS runway {}", airport.runways[*r].id),
        Clearance::GoAround => "go around".into(),
        Clearance::HandoffTower => "contact tower".into(),
        Clearance::HandoffCenter => "contact center".into(),
    }
}

impl World {
    /// Accept an aircraft Center is offering. Immediate: it's a landline, not the radio.
    pub fn accept_handoff(&mut self, id: u32) -> bool {
        let Some(ac) = self.aircraft.iter_mut().find(|a| a.id == id) else {
            return false;
        };
        if ac.phase != Phase::Offered {
            return false;
        }
        ac.phase = Phase::Controlled;
        let line = format!("{}, radar contact.", ac.callsign);
        self.say(Speaker::Controller, line);
        true
    }

    /// Send a radio call. The pilot responds after a short random delay.
    pub fn transmit(&mut self, id: u32, clearances: Vec<Clearance>) -> bool {
        if clearances.is_empty() {
            return false;
        }
        let Some(ac) = self.aircraft.iter().find(|a| a.id == id) else {
            return false;
        };
        let words: Vec<String> =
            clearances.iter().map(|c| phrase(c, ac, &self.airport)).collect();
        let line = format!("{}, {}.", ac.callsign, words.join(", "));
        let delay = PILOT_DELAY.0 + self.rng.f32() * (PILOT_DELAY.1 - PILOT_DELAY.0);
        self.radio.push(RadioCall { due: self.time + delay, aircraft: id, clearances });
        self.say(Speaker::Controller, line);
        true
    }

    /// Answer any radio calls whose pilot delay has elapsed.
    pub(super) fn run_radio(&mut self) {
        let now = self.time;
        let (due, waiting): (Vec<RadioCall>, Vec<RadioCall>) =
            std::mem::take(&mut self.radio).into_iter().partition(|c| c.due <= now);
        self.radio = waiting;
        for call in due {
            self.answer(call);
        }
    }

    fn answer(&mut self, call: RadioCall) {
        let Some(index) = self.aircraft.iter().position(|a| a.id == call.aircraft) else {
            return;
        };
        let arrival_runway = self.arrival_runway();
        let mut readback = Vec::new();
        for clearance in &call.clearances {
            let ac = &mut self.aircraft[index];
            let words = phrase(clearance, ac, &self.airport);
            match apply(clearance, ac, &self.airport, arrival_runway) {
                Ok(()) => {
                    readback.push(words);
                    if *clearance == Clearance::GoAround {
                        let callsign = ac.callsign.clone();
                        self.emit(SimEvent::GoAround { callsign, reason: GoAroundReason::Ordered });
                    }
                }
                Err(reason) => readback.push(format!("unable {words}, {reason}")),
            }
        }
        let line = format!("{}, {}.", readback.join(", "), self.aircraft[index].callsign);
        self.say(Speaker::Pilot, line);
    }
}

fn break_approach(ac: &mut Aircraft) {
    if matches!(ac.phase, Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope) {
        ac.phase = Phase::Controlled;
        ac.approach_runway = None;
        // Hold the altitude reached: the pilot stops descending on the glideslope.
        if ac.on_glideslope {
            ac.target_altitude = ac.altitude;
        }
        ac.on_glideslope = false;
    }
}

/// Validate one clearance and carry it out, or explain why not.
fn apply(
    clearance: &Clearance,
    ac: &mut Aircraft,
    airport: &Airport,
    arrival_runway: usize,
) -> Result<(), String> {
    if !ac.on_player_frequency() {
        return Err("not on your frequency".into());
    }
    match clearance {
        Clearance::Heading(h) => {
            break_approach(ac);
            ac.lateral = Lateral::Heading(normalize_heading(*h));
        }
        Clearance::Altitude(a) => {
            if ac.on_glideslope {
                return Err("established on the glideslope".into());
            }
            if !(MIN_ASSIGNED_ALTITUDE..=MAX_ASSIGNED_ALTITUDE).contains(a) {
                return Err("altitude out of range".into());
            }
            ac.target_altitude = *a;
        }
        Clearance::Speed(s) => {
            if *s < ac.ty.min_speed || *s > ac.ty.max_speed {
                return Err(format!("{} performance", ac.ty.designator));
            }
            if *s > 250.0 && ac.altitude < 10000.0 {
                return Err("below 10,000 feet".into());
            }
            ac.target_speed = *s;
        }
        Clearance::DirectFix(f) => {
            break_approach(ac);
            ac.lateral = Lateral::Direct(*f);
        }
        Clearance::ResumeRoute => {
            if ac.route_index >= ac.route.len() {
                return Err("no route remaining".into());
            }
            break_approach(ac);
            ac.lateral = Lateral::Route;
        }
        Clearance::ClearedIls(r) => {
            check_ils(ac, *r, airport, arrival_runway)?;
            ac.phase = Phase::ClearedApproach;
            ac.approach_runway = Some(*r);
        }
        Clearance::GoAround => {
            if !matches!(
                ac.phase,
                Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope
            ) {
                return Err("not on approach".into());
            }
            ac.start_go_around(airport);
        }
        Clearance::HandoffTower => {
            if !matches!(ac.phase, Phase::Localizer | Phase::Glideslope) {
                return Err("not established".into());
            }
            ac.phase = Phase::TowerHandedOff;
        }
        Clearance::HandoffCenter => {
            if ac.flow != Flow::Departure || ac.phase != Phase::Controlled {
                return Err("not a departure".into());
            }
            ac.phase = Phase::OfferedToCenter;
        }
    }
    Ok(())
}

fn check_ils(
    ac: &Aircraft,
    rwy: usize,
    airport: &Airport,
    arrival_runway: usize,
) -> Result<(), String> {
    if ac.flow != Flow::Arrival {
        return Err("we're a departure".into());
    }
    if rwy != arrival_runway {
        return Err("that runway isn't landing".into());
    }
    let runway = &airport.runways[rwy];
    let along = runway.along(ac.pos);
    if !(MIN_APPROACH_CLEARANCE_NM..=MAX_APPROACH_CLEARANCE_NM).contains(&along) {
        return Err("position not suitable for the approach".into());
    }
    if ac.altitude > airport.glideslope_altitude(rwy, along) + ABOVE_GLIDESLOPE_TOLERANCE {
        return Err("above the glideslope".into());
    }
    let heading = match ac.lateral {
        Lateral::Heading(h) => h,
        _ => ac.heading,
    };
    let intercept = heading_diff(runway.heading, heading);
    if intercept.abs() > MAX_INTERCEPT_DEG {
        return Err("intercept angle too steep".into());
    }
    // Right of the course must be heading left of it, and vice versa.
    let cross = runway.cross(ac.pos);
    if cross.abs() > 0.3 && cross * intercept > 0.0 {
        return Err("heading won't intercept".into());
    }
    Ok(())
}

/// Clearance being built by the player, not yet sent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingClearance {
    pub heading: Option<f32>,
    pub altitude: Option<f32>,
    pub speed: Option<f32>,
    pub direct: Option<usize>,
    pub resume: bool,
    pub ils: Option<usize>,
    pub go_around: bool,
    pub handoff_tower: bool,
    pub handoff_center: bool,
}

impl PendingClearance {
    pub fn is_empty(&self) -> bool {
        *self == PendingClearance::default()
    }

    /// Set a heading. Replaces any fix or route instruction.
    pub fn set_heading(&mut self, heading: f32) {
        self.heading = Some(normalize_heading(heading));
        self.direct = None;
        self.resume = false;
    }

    pub fn nudge_heading(&mut self, ac: &Aircraft, degrees: f32) {
        let base = self.heading.unwrap_or(match ac.lateral {
            Lateral::Heading(h) => h,
            _ => ac.heading.round(),
        });
        self.set_heading(base + degrees);
    }

    pub fn nudge_altitude(&mut self, ac: &Aircraft, feet: f32) {
        let base = self.altitude.unwrap_or(ac.target_altitude);
        let stepped = ((base + feet) / 1000.0).round() * 1000.0;
        self.altitude = Some(stepped.clamp(MIN_ASSIGNED_ALTITUDE, MAX_ASSIGNED_ALTITUDE));
    }

    pub fn nudge_speed(&mut self, ac: &Aircraft, knots: f32) {
        let base = self.speed.unwrap_or(ac.target_speed.min(ac.ty.max_speed));
        let stepped = ((base + knots) / 10.0).round() * 10.0;
        self.speed = Some(stepped.clamp(ac.ty.min_speed, ac.ty.max_speed));
    }

    pub fn set_direct(&mut self, fix: usize) {
        self.direct = Some(fix);
        self.heading = None;
        self.resume = false;
    }

    pub fn set_resume(&mut self) {
        self.resume = true;
        self.heading = None;
        self.direct = None;
    }

    /// The radio call, ordered so a vector and an approach clearance can go in one call.
    pub fn to_clearances(&self) -> Vec<Clearance> {
        let mut out = Vec::new();
        if let Some(h) = self.heading {
            out.push(Clearance::Heading(h));
        }
        if let Some(f) = self.direct {
            out.push(Clearance::DirectFix(f));
        }
        if self.resume {
            out.push(Clearance::ResumeRoute);
        }
        if let Some(a) = self.altitude {
            out.push(Clearance::Altitude(a));
        }
        if let Some(s) = self.speed {
            out.push(Clearance::Speed(s));
        }
        if let Some(r) = self.ils {
            out.push(Clearance::ClearedIls(r));
        }
        if self.go_around {
            out.push(Clearance::GoAround);
        }
        if self.handoff_tower {
            out.push(Clearance::HandoffTower);
        }
        if self.handoff_center {
            out.push(Clearance::HandoffCenter);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::performance::aircraft_type;
    use glam::Vec2;

    fn ac() -> Aircraft {
        Aircraft::new(
            1,
            "TST1",
            Flow::Arrival,
            aircraft_type("B738").unwrap(),
            Vec2::new(20.0, 5.0),
            240.0,
            4000.0,
            200.0,
        )
    }

    #[test]
    fn heading_replaces_direct_and_resume() {
        let mut p = PendingClearance::default();
        p.set_direct(3);
        p.set_heading(90.0);
        assert_eq!(p.direct, None);
        p.set_resume();
        assert_eq!(p.heading, None);
        assert_eq!(p.to_clearances(), vec![Clearance::ResumeRoute]);
    }

    #[test]
    fn nudges_start_from_the_aircraft_and_clamp() {
        let a = ac();
        let mut p = PendingClearance::default();
        p.nudge_heading(&a, 5.0);
        assert_eq!(p.heading, Some(245.0));
        p.nudge_altitude(&a, 1000.0);
        assert_eq!(p.altitude, Some(5000.0));
        p.nudge_speed(&a, -100.0);
        assert_eq!(p.speed, Some(a.ty.min_speed));
    }

    #[test]
    fn heading_is_ordered_before_the_approach_clearance() {
        let p = PendingClearance { heading: Some(240.0), ils: Some(0), ..Default::default() };
        assert_eq!(p.to_clearances(), vec![Clearance::Heading(240.0), Clearance::ClearedIls(0)]);
    }

    #[test]
    fn phrasing_picks_turn_and_altitude_direction() {
        let airport = Airport::default_airport();
        let a = ac();
        assert_eq!(phrase(&Clearance::Heading(200.0), &a, &airport), "turn left heading 200");
        assert_eq!(phrase(&Clearance::Heading(300.0), &a, &airport), "turn right heading 300");
        assert_eq!(
            phrase(&Clearance::Altitude(3000.0), &a, &airport),
            "descend and maintain 3,000"
        );
    }
}
