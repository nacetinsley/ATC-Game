//! Emergencies: how often they happen, what goes wrong, how the aircraft flies
//! with it, and what the controllers do about it.

use super::aircraft::{Aircraft, Flow, Lateral, Leg, Phase, VfrIntent};
use super::clearance::Clearance;
use super::performance::{WakeCategory, wake_required_nm};
use super::scoring::SimEvent;
use super::tower::FINAL_SPAWN_NM;
use super::units::{heading_vec, vec_heading};
use super::{Speaker, World};
use glam::Vec2;

/// An engine-out aircraft climbs at this share of its normal rate.
const ENGINE_OUT_CLIMB: f32 = 0.35;
/// And flies no faster than its approach speed plus this, knots.
const ENGINE_OUT_SPEED_MARGIN: f32 = 70.0;
/// It levels off this high above the field, unless the MVA is higher.
const ENGINE_OUT_LEVEL_AGL: f32 = 3000.0;
/// And won't accept a climb above this height over the field.
const ENGINE_OUT_CEILING_AGL: f32 = 5000.0;
/// Emergency descent rate, feet per minute.
const EMERGENCY_DESCENT_FPM: f32 = 6000.0;
/// Where an emergency descent levels off, unless the MVA is higher.
const EMERGENCY_DESCENT_ALTITUDE: f32 = 10000.0;
/// Center hands a pressurization emergency over this high, feet.
const PRESSURIZATION_ENTRY: (f32, f32) = (16000.0, 17000.0);
/// It declares this long after the player accepts it, seconds.
const PRESSURIZATION_DELAY_S: (f32, f32) = (30.0, 90.0);
/// A departure's engine fails between these heights over the field.
const FAILURE_AGL: (f32, f32) = (400.0, 2000.0);
/// Slowest a low-fuel aircraft will fly away from the approach, knots.
const MIN_FUEL_SPEED: f32 = 210.0;
/// A low-fuel aircraft diverts after this many go-arounds.
const FUEL_GO_AROUNDS: u32 = 2;
/// The automated Approach brings a returning aircraft back on final this
/// long after taking it, seconds.
const RETURN_DELAY_S: (f32, f32) = (240.0, 360.0);
const FUEL_RETURN_DELAY_S: f32 = 150.0;
/// Try again this soon if final is busy, seconds.
const RETURN_RETRY_S: f32 = 20.0;
/// A disabled aircraft closes its runway for this long, seconds.
const CLOSURE_S: (f32, f32) = (360.0, 600.0);
/// The automated Tower holds departures while an emergency is this close to
/// landing on the same runway, NM.
pub const EMERGENCY_HOLD_NM: f32 = 10.0;
/// NORDO self-positioning: the base point it flies to, NM out on final and
/// to the side, and the centerline point it then aims at to intercept.
const NORDO_BASE_ALONG_NM: f32 = 14.0;
const NORDO_BASE_SIDE_NM: f32 = 4.0;
const NORDO_INTERCEPT_ALONG_NM: f32 = 7.0;
const NORDO_AT_BASE_NM: f32 = 1.5;
const NORDO_SPEED: f32 = 210.0;
/// A VFR aircraft on base or final keeps landing during a closure only if it
/// will be down and off the runway this long before the next IFR arrival, s.
const VFR_RUNWAY_S: f32 = 75.0;
/// With Approach automated, an emergency is put on this short a final when
/// there's a gap for it, NM.
pub const PRIORITY_FINAL_NM: f32 = 6.0;
/// The gap it needs: wake spacing (at least 3 NM) to whoever is ahead and
/// behind, plus this for the speeds closing up on final, NM.
const PRIORITY_MARGIN_NM: f32 = 1.5;
/// Medical arrivals parked this soon after landing earn Ground a bonus, seconds.
pub const MEDICAL_PARKING_S: f32 = 240.0;

/// How often emergencies happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmergencyRate {
    #[default]
    Off,
    /// About 1 in 3,500 movements, as at a busy real airport.
    Realistic,
    Occasional,
    Frequent,
    Intense,
}

impl EmergencyRate {
    pub const ALL: [EmergencyRate; 5] = [
        EmergencyRate::Off,
        EmergencyRate::Realistic,
        EmergencyRate::Occasional,
        EmergencyRate::Frequent,
        EmergencyRate::Intense,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EmergencyRate::Off => "Off",
            EmergencyRate::Realistic => "Realistic",
            EmergencyRate::Occasional => "Occasional",
            EmergencyRate::Frequent => "Frequent",
            EmergencyRate::Intense => "Intense",
        }
    }

    /// What to expect, for the menu.
    pub fn note(self) -> &'static str {
        match self {
            EmergencyRate::Off => "No emergencies",
            EmergencyRate::Realistic => "As often as real life: about 1 in 3,500 flights",
            EmergencyRate::Occasional => "About one every few shifts",
            EmergencyRate::Frequent => "About one a shift",
            EmergencyRate::Intense => "Several a shift",
        }
    }

    /// Chance that an IFR movement has an emergency.
    pub fn chance(self) -> f32 {
        match self {
            EmergencyRate::Off => 0.0,
            EmergencyRate::Realistic => 1.0 / 3500.0,
            EmergencyRate::Occasional => 1.0 / 48.0,
            EmergencyRate::Frequent => 1.0 / 16.0,
            EmergencyRate::Intense => 1.0 / 5.0,
        }
    }

    /// Most emergencies going on at once.
    fn max_active(self) -> usize {
        if self == EmergencyRate::Intense { 2 } else { 1 }
    }

    pub fn next(self) -> EmergencyRate {
        let i = Self::ALL.iter().position(|&r| r == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Why an aircraft will stop on the runway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledCause {
    Smoke,
    Hydraulics,
    Gear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyKind {
    Medical,
    LowFuel,
    EngineFailure,
    BirdStrike,
    Pressurization,
    /// Lands and stops on the runway, closing it.
    Disabled(DisabledCause),
    RadioFailure,
}

impl EmergencyKind {
    /// Short form for strips.
    pub fn label(self) -> &'static str {
        match self {
            EmergencyKind::Medical => "MED",
            EmergencyKind::LowFuel => "FUEL",
            EmergencyKind::EngineFailure => "ENG",
            EmergencyKind::BirdStrike => "BIRD",
            EmergencyKind::Pressurization => "PRESS",
            EmergencyKind::Disabled(DisabledCause::Smoke) => "SMOKE",
            EmergencyKind::Disabled(DisabledCause::Hydraulics) => "HYD",
            EmergencyKind::Disabled(DisabledCause::Gear) => "GEAR",
            EmergencyKind::RadioFailure => "RDOF",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            EmergencyKind::Medical => "medical emergency",
            EmergencyKind::LowFuel => "low fuel",
            EmergencyKind::EngineFailure => "engine failure",
            EmergencyKind::BirdStrike => "bird strike",
            EmergencyKind::Pressurization => "pressurization failure",
            EmergencyKind::Disabled(DisabledCause::Smoke) => "smoke in the cabin",
            EmergencyKind::Disabled(DisabledCause::Hydraulics) => "hydraulic failure",
            EmergencyKind::Disabled(DisabledCause::Gear) => "gear problem",
            EmergencyKind::RadioFailure => "radio failure",
        }
    }

    fn engine_out(self) -> bool {
        matches!(
            self,
            EmergencyKind::EngineFailure | EmergencyKind::BirdStrike
        )
    }
}

/// When the emergency happens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger {
    /// As soon as the aircraft is talking to someone other than Center.
    OnCheckIn,
    /// This many seconds after the player accepts it from Center.
    AfterAccept(f32),
    /// Climbing through this height over the field.
    AtHeight(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Emergency {
    pub kind: EmergencyKind,
    pub trigger: Trigger,
    /// Sim time it was declared, once it has been.
    pub declared_at: Option<f32>,
    pub souls: u32,
    /// Fuel remaining at the time of declaring, minutes.
    pub fuel_minutes: u32,
    /// Sim time it touched down.
    pub landed_at: Option<f32>,
    /// In an emergency descent.
    pub descending: bool,
    pub go_arounds: u32,
    /// Leaving for its alternate.
    pub divert: bool,
    accepted_at: Option<f32>,
}

impl Emergency {
    pub fn new(kind: EmergencyKind, trigger: Trigger, souls: u32, fuel_minutes: u32) -> Emergency {
        Emergency {
            kind,
            trigger,
            declared_at: None,
            souls,
            fuel_minutes,
            landed_at: None,
            descending: false,
            go_arounds: 0,
            divert: false,
            accepted_at: None,
        }
    }

    pub fn declared(&self) -> bool {
        self.declared_at.is_some()
    }

    pub fn nordo(&self) -> bool {
        self.declared() && self.kind == EmergencyKind::RadioFailure
    }

    pub fn engine_out(&self) -> bool {
        self.declared() && self.kind.engine_out()
    }

    /// Will stop on the runway after landing.
    pub fn stops_on_runway(&self) -> bool {
        self.declared() && matches!(self.kind, EmergencyKind::Disabled(_))
    }

    /// What the data block shows: `EM` for 7700, `RDOF` for 7600.
    /// The special purpose code's ID, as STARS shows it: RF for 7600 (radio
    /// failure), EM for 7700.
    pub fn tag(&self) -> &'static str {
        if self.kind == EmergencyKind::RadioFailure {
            "RF"
        } else {
            "EM"
        }
    }

    /// What the strip shows.
    pub fn strip(&self) -> String {
        if self.kind == EmergencyKind::RadioFailure {
            "RDOF".into()
        } else {
            format!("EMRG {}", self.kind.label())
        }
    }
}

impl Aircraft {
    /// Emergency speed limit, knots.
    pub(super) fn emergency_speed_cap(&self) -> Option<f32> {
        self.emergency
            .as_ref()
            .filter(|e| e.engine_out())
            .map(|_| self.ty.vref + ENGINE_OUT_SPEED_MARGIN)
    }

    /// Climb rate after any emergency, feet per minute.
    pub(super) fn emergency_climb(&self, rate: f32) -> f32 {
        if self.emergency.as_ref().is_some_and(|e| e.engine_out()) {
            rate * ENGINE_OUT_CLIMB
        } else {
            rate
        }
    }

    /// Descent rate after any emergency, feet per minute.
    pub(super) fn emergency_descent(&self, rate: f32) -> f32 {
        if self.emergency.as_ref().is_some_and(|e| e.descending) {
            rate.max(EMERGENCY_DESCENT_FPM)
        } else {
            rate
        }
    }

    /// No radio: only a light-gun landing clearance gets through.
    pub fn nordo(&self) -> bool {
        self.emergency.as_ref().is_some_and(|e| e.nordo())
    }

    /// A declared emergency, of any kind.
    pub fn in_emergency(&self) -> bool {
        self.emergency.as_ref().is_some_and(|e| e.declared())
    }
}

/// Why the pilot of an aircraft in an emergency won't do this, if they won't.
pub(super) fn refusal(ac: &Aircraft, clearance: &Clearance, elevation: f32) -> Option<String> {
    let e = ac.emergency.as_ref().filter(|e| e.declared())?;
    match clearance {
        Clearance::Altitude(_) if e.descending => Some("emergency descent".into()),
        Clearance::Altitude(a) if e.engine_out() && *a > elevation + ENGINE_OUT_CEILING_AGL => {
            Some("single engine".into())
        }
        Clearance::Speed(s) if e.engine_out() && *s > ac.ty.vref + ENGINE_OUT_SPEED_MARGIN => {
            Some("single engine".into())
        }
        Clearance::Speed(s)
            if e.kind == EmergencyKind::LowFuel
                && *s < MIN_FUEL_SPEED
                && !matches!(
                    ac.phase,
                    Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff
                ) =>
        {
            Some("minimum fuel".into())
        }
        _ => None,
    }
}

/// A runway closed by a disabled aircraft.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Closure {
    /// Physical runway in the ground map.
    pub physical: usize,
    /// Sim time the tow clears it.
    pub until: f32,
    /// The disabled aircraft.
    pub aircraft: u32,
}

/// Emergency state for the whole world.
#[derive(Debug, Clone)]
pub struct Emergencies {
    rng: fastrand::Rng,
    pub closure: Option<Closure>,
    /// Aircraft the automated Approach is bringing back to final, and when.
    returns: Vec<(f32, Aircraft)>,
}

impl Emergencies {
    pub fn new(seed: u64) -> Emergencies {
        Emergencies {
            rng: fastrand::Rng::with_seed(seed ^ 0x454D_4552_4745_4E43),
            closure: None,
            returns: Vec::new(),
        }
    }

    fn between(&mut self, range: (f32, f32)) -> f32 {
        range.0 + self.rng.f32() * (range.1 - range.0)
    }

    /// Aircraft away with the automated Approach, coming back to final.
    pub fn returning(&self) -> impl Iterator<Item = &Aircraft> {
        self.returns.iter().map(|(_, ac)| ac)
    }
}

/// People on board, by size.
fn souls(rng: &mut fastrand::Rng, wake: WakeCategory) -> u32 {
    match wake {
        WakeCategory::Small => rng.u32(1..6),
        WakeCategory::Large => rng.u32(60..190),
        WakeCategory::Heavy => rng.u32(180..360),
    }
}

/// "25 minutes", "2 hours 10 minutes".
fn fuel_words(minutes: u32) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m} minutes"),
        (1, 0) => "1 hour".into(),
        (h, 0) => format!("{h} hours"),
        (1, m) => format!("1 hour {m} minutes"),
        (h, m) => format!("{h} hours {m} minutes"),
    }
}

impl World {
    /// Emergencies still to happen or still being worked: once one has
    /// landed it no longer counts (a closed runway is limited separately).
    fn emergencies_active(&self) -> usize {
        self.aircraft
            .iter()
            .chain(self.departure_queue.iter())
            .chain(self.emergencies.returning())
            .filter(|a| a.emergency.as_ref().is_some_and(|e| e.landed_at.is_none()))
            .count()
    }

    /// Roll for an emergency on a new movement.
    fn roll(&mut self) -> bool {
        let rate = self.config.emergencies;
        rate != EmergencyRate::Off
            && self.emergencies_active() < rate.max_active()
            && self.emergencies.rng.f32() < rate.chance()
    }

    /// The open parallel landing the same way as this runway end, if any.
    pub(super) fn open_parallel(&self, end: usize) -> Option<usize> {
        let map = self.airport.surface.as_ref()?;
        let runway = &self.airport.runways[end];
        (0..self.airport.runways.len()).find(|&other| {
            map.runway_of_end[other] != map.runway_of_end[end]
                && (self.airport.runways[other].heading - runway.heading).abs() < 1.0
                && !self.closed(other)
        })
    }

    /// Is this runway end closed?
    pub fn closed(&self, end: usize) -> bool {
        let (Some(c), Some(map)) = (self.emergencies.closure, self.airport.surface.as_ref()) else {
            return false;
        };
        map.runway_of_end[end] == c.physical
    }

    /// This runway end, or the open parallel if it's closed.
    pub(super) fn avoiding_closure(&self, end: usize) -> usize {
        if self.closed(end) {
            self.open_parallel(end).unwrap_or(end)
        } else {
            end
        }
    }

    /// Maybe give a new arrival an emergency. `on_final` for arrivals the
    /// automated Approach delivers to Tower.
    pub(super) fn roll_arrival_emergency(&mut self, ac: &mut Aircraft, on_final: bool) {
        if ac.vfr.is_some() || !self.roll() {
            return;
        }
        let staffing = self.config.staffing;
        let mut kinds: Vec<(EmergencyKind, f32)> = vec![
            (EmergencyKind::Medical, 3.0),
            (EmergencyKind::LowFuel, 2.0),
            (EmergencyKind::EngineFailure, 1.0),
        ];
        if self.emergencies.closure.is_none() && self.open_parallel(self.arrival_runway()).is_some()
        {
            let causes = [
                DisabledCause::Smoke,
                DisabledCause::Hydraulics,
                DisabledCause::Gear,
            ];
            let cause = causes[self.emergencies.rng.usize(0..causes.len())];
            kinds.push((EmergencyKind::Disabled(cause), 1.5));
        }
        if staffing.approach || staffing.local {
            kinds.push((EmergencyKind::RadioFailure, 1.5));
        }
        if staffing.approach && !on_final {
            kinds.push((EmergencyKind::Pressurization, 1.5));
        }
        let total: f32 = kinds.iter().map(|k| k.1).sum();
        let mut pick = self.emergencies.rng.f32() * total;
        let kind = kinds
            .iter()
            .find(|(_, w)| {
                pick -= w;
                pick <= 0.0
            })
            .map_or(kinds[0].0, |k| k.0);

        let trigger = if kind == EmergencyKind::Pressurization {
            let altitude = if self.emergencies.rng.bool() {
                PRESSURIZATION_ENTRY.0
            } else {
                PRESSURIZATION_ENTRY.1
            };
            ac.altitude = altitude;
            ac.target_altitude = altitude;
            Trigger::AfterAccept(self.emergencies.between(PRESSURIZATION_DELAY_S))
        } else {
            Trigger::OnCheckIn
        };
        let souls = souls(&mut self.emergencies.rng, ac.ty.wake);
        let fuel = if kind == EmergencyKind::LowFuel {
            self.emergencies.rng.u32(15..26)
        } else {
            self.emergencies.rng.u32(60..241)
        };
        ac.emergency = Some(Emergency::new(kind, trigger, souls, fuel));
    }

    /// Maybe give the departure just queued an engine failure after takeoff.
    pub(super) fn roll_departure_emergency(&mut self) {
        if self.departure_queue.back().is_none_or(|a| a.vfr.is_some()) || !self.roll() {
            return;
        }
        let kind = if self.emergencies.rng.bool() {
            EmergencyKind::EngineFailure
        } else {
            EmergencyKind::BirdStrike
        };
        let height = self.emergencies.between(FAILURE_AGL);
        let fuel = self.emergencies.rng.u32(90..300);
        let wake = self
            .departure_queue
            .back()
            .map_or(WakeCategory::Large, |a| a.ty.wake);
        let souls = souls(&mut self.emergencies.rng, wake);
        if let Some(ac) = self.departure_queue.back_mut() {
            ac.emergency = Some(Emergency::new(kind, Trigger::AtHeight(height), souls, fuel));
        }
    }

    /// Once a tick: declare emergencies whose time has come, fly the NORDO
    /// procedure, bring back returning aircraft, and clear disabled runways.
    pub(super) fn run_emergencies(&mut self) {
        let now = self.time;
        let elevation = self.airport.elevation;
        for index in 0..self.aircraft.len() {
            let ac = &mut self.aircraft[index];
            let Some(e) = ac.emergency.as_mut() else {
                continue;
            };
            if e.declared_at.is_none() {
                if ac.phase != Phase::Offered && e.accepted_at.is_none() {
                    e.accepted_at = Some(now);
                }
                let due = match e.trigger {
                    Trigger::OnCheckIn => {
                        ac.phase != Phase::Offered || e.kind == EmergencyKind::RadioFailure
                    }
                    Trigger::AfterAccept(after) => e.accepted_at.is_some_and(|t| now - t >= after),
                    Trigger::AtHeight(height) => !ac.on_ground && ac.altitude - elevation >= height,
                };
                if due {
                    self.declare(index);
                }
                continue;
            }
            if e.descending && ac.altitude <= ac.target_altitude + 50.0 {
                e.descending = false;
                let line = format!(
                    "{}, level {:.0}, request vectors to land.",
                    ac.callsign, ac.target_altitude
                );
                self.say(Speaker::Emergency, line);
            }
        }
        for index in 0..self.aircraft.len() {
            if self.aircraft[index].nordo() {
                self.fly_nordo(index);
            }
        }
        self.run_diversions();
        self.run_returns();
        self.run_closure();
    }

    /// The pilot declares: the mayday call, and what changes in how it flies.
    fn declare(&mut self, index: usize) {
        let now = self.time;
        let elevation = self.airport.elevation;
        let mva = self.airport.mva_at(self.aircraft[index].pos);
        let arrival = self.airport.runways[self.arrival_runway()].id.clone();
        let ac = &mut self.aircraft[index];
        let Some(e) = ac.emergency.as_mut() else {
            return;
        };
        e.declared_at = Some(now);
        let kind = e.kind;
        let departing = ac.flow == Flow::Departure;
        let tail = format!(
            ", {} souls, {} of fuel",
            e.souls,
            fuel_words(e.fuel_minutes)
        );
        let request = match kind {
            EmergencyKind::Medical => "medical emergency on board, request priority and paramedics at the gate".to_string(),
            EmergencyKind::LowFuel => "emergency fuel, unable to hold, request straight in".to_string(),
            EmergencyKind::EngineFailure if departing => format!("engine failure, request return, runway {arrival}"),
            EmergencyKind::EngineFailure => "engine shut down, single engine approach, request longest runway".to_string(),
            EmergencyKind::BirdStrike => format!("bird strike, engine damage, request return, runway {arrival}"),
            EmergencyKind::Pressurization => "pressurization failure, emergency descent to one zero thousand".to_string(),
            EmergencyKind::Disabled(DisabledCause::Smoke) => {
                "smoke in the cabin, request immediate landing, we'll stop on the runway and evacuate".to_string()
            }
            EmergencyKind::Disabled(DisabledCause::Hydraulics) => {
                "hydraulic failure, no nosewheel steering, we'll need a tow off the runway".to_string()
            }
            EmergencyKind::Disabled(DisabledCause::Gear) => {
                "gear unsafe indication, we'll stop on the runway for inspection".to_string()
            }
            EmergencyKind::RadioFailure => String::new(),
        };
        match kind {
            EmergencyKind::EngineFailure | EmergencyKind::BirdStrike if departing => {
                // Back to land: an arrival now.
                ac.flow = Flow::Arrival;
                ac.exit_fix = None;
                ac.route.clear();
                ac.route_index = 0;
                ac.departure_end = None;
                ac.spawn_time = now;
                ac.lateral = Lateral::Heading(ac.heading);
                ac.target_altitude = round_up_100((elevation + ENGINE_OUT_LEVEL_AGL).max(mva));
            }
            EmergencyKind::Pressurization => {
                e.descending = true;
                ac.target_altitude = round_up_100(EMERGENCY_DESCENT_ALTITUDE.max(mva));
            }
            _ => {}
        }
        if let Some(cap) = ac.emergency_speed_cap() {
            ac.target_speed = ac.target_speed.min(cap);
        }
        let callsign = ac.callsign.clone();
        let line = if kind == EmergencyKind::RadioFailure {
            let unit = if ac.phase == Phase::Offered {
                "Center"
            } else {
                "Approach"
            };
            format!(
                "{unit}: {callsign} is squawking 7600, radio failure. Last assigned {:.0}, expect the published arrival and approach.",
                ac.target_altitude
            )
        } else {
            let station = match ac.frequency() {
                super::positions::Position::Approach => "Approach",
                super::positions::Position::Local => "Tower",
                super::positions::Position::Ground => "Ground",
                super::positions::Position::Delivery => "Clearance",
                super::positions::Position::Center => "Center",
                super::positions::Position::Ramp => "Ramp",
            };
            let call = if kind == EmergencyKind::Medical {
                "Pan-pan, pan-pan, pan-pan"
            } else {
                "Mayday, mayday, mayday"
            };
            format!("{call}, {station}, {callsign}, {request}{tail}.")
        };
        self.say(Speaker::Emergency, line);
        self.emit(SimEvent::EmergencyDeclared { callsign, kind });
    }

    /// Lost-comms procedure: fly the arrival, then position for the ILS on
    /// its own and land on a light-gun signal.
    fn fly_nordo(&mut self, index: usize) {
        let rwy = self.arrival_runway();
        let runway = self.airport.runways[rwy].clone();
        let ac = &self.aircraft[index];
        match ac.phase {
            // Coordinated by landline: Tower takes it once it's established.
            Phase::Localizer | Phase::Glideslope => {
                let ac = &mut self.aircraft[index];
                ac.phase = Phase::TowerHandedOff;
                ac.freq_since = self.time;
            }
            Phase::Controlled | Phase::Offered => {
                let on_route = ac.lateral == Lateral::Route && ac.route_index < ac.route.len();
                if on_route {
                    return;
                }
                let out = -heading_vec(runway.heading);
                let threshold = runway.threshold();
                let perp = Vec2::new(out.y, -out.x);
                let side = if (ac.pos - threshold).dot(perp) < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                let base = threshold + out * NORDO_BASE_ALONG_NM + perp * side * NORDO_BASE_SIDE_NM;
                let altitude = round_up_100(
                    self.airport
                        .glideslope_altitude(rwy, NORDO_BASE_ALONG_NM)
                        .max(self.airport.mva_at(base)),
                );
                let at_base = ac.pos.distance(base) < NORDO_AT_BASE_NM;
                let ac = &mut self.aircraft[index];
                ac.target_altitude = altitude;
                ac.target_speed = ac.target_speed.min(NORDO_SPEED);
                if at_base {
                    let aim = threshold + out * NORDO_INTERCEPT_ALONG_NM;
                    ac.lateral = Lateral::Heading(vec_heading(aim - ac.pos));
                    ac.approach_runway = Some(rwy);
                    ac.phase = Phase::ClearedApproach;
                } else {
                    ac.lateral = Lateral::Heading(vec_heading(base - ac.pos));
                }
            }
            _ => {}
        }
    }

    /// A low-fuel aircraft that has gone around too often leaves for its alternate.
    fn run_diversions(&mut self) {
        let mut index = 0;
        while index < self.aircraft.len() {
            if self.aircraft[index]
                .emergency
                .as_ref()
                .is_some_and(|e| e.divert)
            {
                let ac = self.aircraft.remove(index);
                let line = format!("{}, minimum fuel, diverting to our alternate.", ac.callsign);
                self.say(Speaker::Emergency, line);
                self.emit(SimEvent::EmergencyDiverted {
                    callsign: ac.callsign,
                });
                continue;
            }
            index += 1;
        }
    }

    /// Tower-only modes: the automated Approach takes an emergency aircraft
    /// that's left the final, and brings it back a few minutes later.
    fn run_returns(&mut self) {
        if self.config.staffing.approach {
            return;
        }
        let now = self.time;
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            let away = ac.in_emergency() && !ac.on_ground && ac.phase == Phase::Controlled;
            if away {
                let ac = self.aircraft.remove(index);
                let fuel = ac
                    .emergency
                    .as_ref()
                    .is_some_and(|e| e.kind == EmergencyKind::LowFuel);
                let delay = if fuel {
                    FUEL_RETURN_DELAY_S
                } else {
                    self.emergencies.between(RETURN_DELAY_S)
                };
                self.emergencies.returns.push((now + delay, ac));
                continue;
            }
            index += 1;
        }
        let returns = std::mem::take(&mut self.emergencies.returns);
        for (due, ac) in returns {
            if due > now {
                self.emergencies.returns.push((due, ac));
            } else if self.room_on_final() {
                self.spawn_return(ac);
            } else {
                self.emergencies.returns.push((now + RETURN_RETRY_S, ac));
            }
        }
    }

    /// Move an emergency just put on final in to the short final, if the gap
    /// is there. True if it moved.
    pub(super) fn priority_final(&self, ac: &mut Aircraft, rwy: usize) -> bool {
        let runway = &self.airport.runways[rwy];
        let crowded = self.aircraft.iter().any(|a| {
            if a.id == ac.id || a.on_ground || a.approach_runway != Some(rwy) {
                return false;
            }
            let d = runway.along(a.pos);
            // Not behind one that's going to stop on the runway.
            if a.emergency.as_ref().is_some_and(|e| e.stops_on_runway()) {
                return true;
            }
            let (gap, required) = if d < PRIORITY_FINAL_NM {
                (
                    PRIORITY_FINAL_NM - d,
                    wake_required_nm(a.ty.wake, ac.ty.wake),
                )
            } else {
                (
                    d - PRIORITY_FINAL_NM,
                    wake_required_nm(ac.ty.wake, a.ty.wake),
                )
            };
            gap < required + PRIORITY_MARGIN_NM
        });
        if crowded {
            return false;
        }
        ac.pos = runway.threshold() - heading_vec(runway.heading) * PRIORITY_FINAL_NM;
        ac.altitude = self.airport.glideslope_altitude(rwy, PRIORITY_FINAL_NM);
        ac.target_altitude = ac.altitude;
        ac.entered_sector = self.airport.in_sector(ac.pos);
        true
    }

    /// Put a returning aircraft back on final.
    fn spawn_return(&mut self, mut ac: Aircraft) {
        let rwy = self.arrival_runway();
        let runway = self.airport.runways[rwy].clone();
        ac.pos = runway.threshold() - heading_vec(runway.heading) * FINAL_SPAWN_NM;
        ac.heading = runway.heading;
        ac.altitude = self.airport.glideslope_altitude(rwy, FINAL_SPAWN_NM);
        ac.target_altitude = ac.altitude;
        let miles = if self.priority_final(&mut ac, rwy) {
            PRIORITY_FINAL_NM
        } else {
            FINAL_SPAWN_NM
        };
        ac.ias = ac.ias.min(180.0);
        ac.target_speed = ac.ias;
        ac.phase = Phase::TowerHandedOff;
        ac.lateral = Lateral::Localizer;
        ac.approach_runway = Some(rwy);
        ac.on_glideslope = true;
        ac.cleared_to_land = false;
        ac.msaw_armed = false;
        ac.freq_since = self.time;
        let callsign = ac.callsign.clone();
        if self.config.staffing.local {
            let line = if ac.nordo() {
                format!(
                    "Approach: {callsign}, radio failure, {miles:.0} mile final runway {}.",
                    runway.id
                )
            } else {
                let what = ac.emergency.as_ref().map_or("", |e| e.kind.describe());
                format!(
                    "Tower, {callsign}, {miles:.0} mile final runway {}, {what}.",
                    runway.id
                )
            };
            let speaker = if ac.nordo() {
                Speaker::System
            } else {
                Speaker::Emergency
            };
            self.say(speaker, line);
        }
        self.add_aircraft(ac);
    }

    /// Close the runway under a disabled aircraft, and open it once it's towed.
    fn run_closure(&mut self) {
        if let Some(c) = self.emergencies.closure {
            self.suspend_pattern();
            if self.time >= c.until {
                self.aircraft.retain(|a| a.id != c.aircraft);
                self.emergencies.closure = None;
                let name = self.physical_name(c.physical);
                self.say(
                    Speaker::System,
                    format!("Tower: disabled aircraft towed clear, runway {name} open."),
                );
                self.after_closure_change();
            }
            return;
        }
        let stopped = self.aircraft.iter().find(|a| {
            a.on_ground
                && a.phase == Phase::Rollout
                && a.ias < 1.0
                && a.emergency.as_ref().is_some_and(|e| e.stops_on_runway())
        });
        let Some((id, end)) = stopped.and_then(|a| a.approach_runway.map(|e| (a.id, e))) else {
            return;
        };
        let Some(physical) = self.airport.surface.as_ref().map(|m| m.runway_of_end[end]) else {
            return;
        };
        let until = self.time + self.emergencies.between(CLOSURE_S);
        self.emergencies.closure = Some(Closure {
            physical,
            until,
            aircraft: id,
        });
        let name = self.physical_name(physical);
        self.say(
            Speaker::System,
            format!("Tower: runway {name} closed, disabled aircraft on the runway."),
        );
        if self.aircraft.iter().any(|a| a.vfr.is_some()) {
            self.say(
                Speaker::System,
                "Tower: VFR traffic, the pattern is closed. Remain clear of the airspace.".into(),
            );
        }
        self.suspend_pattern();
        self.sidestep_arrivals();
        self.after_closure_change();
    }

    /// For the status lines: "RWY 27L/09R CLSD 6:40".
    pub fn closure_line(&self) -> Option<String> {
        let c = self.emergencies.closure?;
        let left = (c.until - self.time).max(0.0);
        Some(format!(
            "RWY {} CLSD {}:{:02}",
            self.physical_name(c.physical),
            (left / 60.0) as i32,
            (left % 60.0) as i32
        ))
    }

    /// The physical runway closed, if one is.
    pub fn closed_physical(&self) -> Option<usize> {
        self.emergencies.closure.map(|c| c.physical)
    }

    /// The closed runway, "27L/09R", if one is.
    pub fn closed_runway_name(&self) -> Option<String> {
        self.emergencies
            .closure
            .map(|c| self.physical_name(c.physical))
    }

    /// "27L/09R".
    fn physical_name(&self, physical: usize) -> String {
        let Some(map) = self.airport.surface.as_ref() else {
            return String::new();
        };
        let ids: Vec<&str> = (0..self.airport.runways.len())
            .filter(|&e| map.runway_of_end[e] == physical)
            .map(|e| self.airport.runways[e].id.as_str())
            .collect();
        ids.join("/")
    }

    /// With one runway left for everyone, VFR pattern work stops: closed
    /// traffic leaves instead of flying circuits, and anything in the air not
    /// yet on base or final breaks off and leaves the area.
    fn suspend_pattern(&mut self) {
        let Some(map) = self.airport.surface.as_ref() else {
            return;
        };
        let mut break_off = Vec::new();
        for (index, ac) in self.aircraft.iter().enumerate() {
            let Some(vfr) = ac.vfr.as_ref() else { continue };
            if ac.on_ground || ac.phase != Phase::Pattern || vfr.leg == Leg::Departing {
                continue;
            }
            if matches!(vfr.leg, Leg::Base | Leg::Final) {
                // Lands first only if it'll be clear before the IFR traffic gets there.
                let physical = map.runway_of_end[vfr.end];
                let ifr = self
                    .aircraft
                    .iter()
                    .filter(|b| b.vfr.is_none())
                    .filter_map(|b| self.time_to_land(b, physical))
                    .fold(f32::INFINITY, f32::min);
                if self
                    .time_to_land(ac, physical)
                    .is_some_and(|t| t + VFR_RUNWAY_S < ifr)
                {
                    continue;
                }
            }
            break_off.push(index);
        }
        let local = self.config.staffing.local;
        for ac in &mut self.aircraft {
            let Some(vfr) = ac.vfr.as_mut() else { continue };
            if vfr.intent == VfrIntent::Closed {
                vfr.intent = VfrIntent::Departure;
                vfr.outbound = self.airport.runways[vfr.end].heading;
            }
        }
        for index in break_off {
            let elevation = self.airport.elevation;
            let ac = &mut self.aircraft[index];
            let Some(vfr) = ac.vfr.as_mut() else { continue };
            let committed = matches!(vfr.leg, Leg::Base | Leg::Final);
            vfr.intent = VfrIntent::Departure;
            vfr.outbound = vec_heading(ac.pos);
            vfr.leg = Leg::Departing;
            vfr.landing = None;
            vfr.extend = false;
            vfr.turn_base = false;
            vfr.follow = None;
            ac.approach_runway = None;
            ac.on_glideslope = false;
            ac.target_altitude = ac.target_altitude.max(elevation + 1000.0);
            if committed && local {
                let line = format!("{}, breaking off, leaving the pattern.", ac.callsign);
                self.say(Speaker::Pilot, line);
            }
        }
    }

    /// Arrivals on final for the closed runway that the player can't re-clear
    /// sidestep to the parallel. Those close in go around as usual.
    fn sidestep_arrivals(&mut self) {
        let player_approach = self.config.staffing.approach;
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let Some(end) = ac.approach_runway.filter(|&e| self.closed(e)) else {
                continue;
            };
            let far = self.airport.runways[end].along(ac.pos) > 4.0;
            let theirs = player_approach && self.is_players(ac);
            if ac.on_ground || !far || theirs || ac.vfr.is_some() {
                continue;
            }
            let Some(to) = self.open_parallel(end) else {
                continue;
            };
            let line = format!(
                "Tower: {}, sidestep to runway {}.",
                ac.callsign, self.airport.runways[to].id
            );
            self.aircraft[index].approach_runway = Some(to);
            self.say(Speaker::System, line);
        }
    }

    /// Runways in use have changed: tell everyone and move the departures.
    fn after_closure_change(&mut self) {
        self.new_atis();
        self.reassign_departures();
    }

    /// The automated Tower holds departures from this runway while an
    /// emergency is close to landing on it.
    pub(super) fn emergency_inbound(&self, physical: usize) -> bool {
        let Some(map) = self.airport.surface.as_ref() else {
            return false;
        };
        self.aircraft.iter().any(|a| {
            !a.on_ground
                && a.in_emergency()
                && a.approach_runway.is_some_and(|e| {
                    map.runway_of_end[e] == physical
                        && self.airport.runways[e].along(a.pos) < EMERGENCY_HOLD_NM
                })
        })
    }

    /// Bookkeeping when an emergency aircraft goes around.
    pub(super) fn emergency_go_around(&mut self, index: usize) {
        let ac = &mut self.aircraft[index];
        let Some(e) = ac.emergency.as_mut().filter(|e| e.declared()) else {
            return;
        };
        e.go_arounds += 1;
        if e.kind == EmergencyKind::LowFuel && e.go_arounds >= FUEL_GO_AROUNDS {
            e.divert = true;
        }
        let callsign = ac.callsign.clone();
        self.emit(SimEvent::EmergencyGoAround { callsign });
    }

    /// Bookkeeping when an emergency aircraft touches down.
    pub(super) fn emergency_landed(&mut self, index: usize) {
        let now = self.time;
        let ac = &mut self.aircraft[index];
        let Some(e) = ac.emergency.as_mut().filter(|e| e.declared()) else {
            return;
        };
        e.landed_at = Some(now);
        let minutes = (now - e.declared_at.unwrap_or(now)) / 60.0;
        let kind = e.kind;
        let callsign = ac.callsign.clone();
        self.emit(SimEvent::EmergencyLanded {
            callsign,
            kind,
            minutes,
        });
    }

    /// Bookkeeping when an emergency aircraft parks.
    pub(super) fn emergency_parked(&mut self, ac: &Aircraft) {
        let Some(e) = ac.emergency.as_ref() else {
            return;
        };
        let prompt = e
            .landed_at
            .is_some_and(|t| self.time - t <= MEDICAL_PARKING_S);
        if e.kind == EmergencyKind::Medical && prompt {
            self.emit(SimEvent::EmergencyParked {
                callsign: ac.callsign.clone(),
            });
        }
    }
}

fn round_up_100(feet: f32) -> f32 {
    (feet / 100.0).ceil() * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_rise_from_off_to_intense() {
        let chances: Vec<f32> = EmergencyRate::ALL.iter().map(|r| r.chance()).collect();
        assert_eq!(chances[0], 0.0);
        assert!(chances.windows(2).all(|w| w[0] < w[1]), "{chances:?}");
        assert!((EmergencyRate::Realistic.chance() * 3500.0 - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_menu_key_cycles_through_every_level() {
        let mut rate = EmergencyRate::Off;
        for expected in EmergencyRate::ALL.iter().cycle().skip(1).take(5) {
            rate = rate.next();
            assert_eq!(rate, *expected);
        }
    }

    #[test]
    fn fuel_is_spoken_in_hours_and_minutes() {
        assert_eq!(fuel_words(25), "25 minutes");
        assert_eq!(fuel_words(60), "1 hour");
        assert_eq!(fuel_words(130), "2 hours 10 minutes");
    }
}
