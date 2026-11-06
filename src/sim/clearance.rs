//! Clearances: what the controller says, who may say it, and how the pilot responds.

use super::aircraft::{Aircraft, Flow, Landing, Lateral, Leg, Phase, Taxi, VfrIntent};
use super::airspace::Airport;
use super::ground_map::GroundMap;
use super::positions::{Position, Staffing};
use super::scoring::{GoAroundReason, SimEvent};
use super::units::{format_heading, heading_diff, normalize_heading};
use super::vfr::{compass, landing_for, side_word};
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
/// Closer than this to a node, an aircraft is at it, NM.
pub(super) const AT_NODE_NM: f32 = 1e-4;

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
    /// Taxi to a node along these taxiways in order (empty for the shortest route).
    Taxi { to: usize, via: Vec<String> },
    HoldPosition,
    ContinueTaxi,
    /// Physical runway index in the ground map.
    CrossRunway(usize),
    /// Runway end index.
    LineUpAndWait(usize),
    ClearedForTakeoff(usize),
    ClearedToLand(usize),
    ContactGround,
    ContactTower,
    ContactDeparture,
    /// VFR: enter the downwind for this runway end.
    EnterDownwind(usize),
    ExtendDownwind,
    TurnBase,
    /// VFR: follow this aircraft, described as the traffic call names it.
    Follow { id: u32, traffic: String },
    ClearedTouchAndGo(usize),
    ClearedOption(usize),
}

impl Clearance {
    /// Positions that can give this instruction.
    pub fn issuers(&self) -> &'static [Position] {
        use Position::*;
        match self {
            Clearance::Heading(_)
            | Clearance::Altitude(_)
            | Clearance::Speed(_)
            | Clearance::DirectFix(_)
            | Clearance::ResumeRoute
            | Clearance::ClearedIls(_)
            | Clearance::HandoffTower
            | Clearance::HandoffCenter => &[Approach],
            Clearance::GoAround => &[Local, Approach],
            Clearance::Taxi { .. } | Clearance::HoldPosition | Clearance::ContinueTaxi => {
                &[Ground, Local]
            }
            Clearance::CrossRunway(_)
            | Clearance::LineUpAndWait(_)
            | Clearance::ClearedForTakeoff(_)
            | Clearance::ClearedToLand(_)
            | Clearance::ContactGround
            | Clearance::ContactDeparture
            | Clearance::EnterDownwind(_)
            | Clearance::ExtendDownwind
            | Clearance::TurnBase
            | Clearance::Follow { .. }
            | Clearance::ClearedTouchAndGo(_)
            | Clearance::ClearedOption(_) => &[Local],
            Clearance::ContactTower => &[Ground],
        }
    }
}

/// Can `speaker` talk to an aircraft on `frequency`? Its own frequency, or any
/// frequency the player also works: that's what a combined position is.
pub fn can_speak(speaker: Position, frequency: Position, staffing: Staffing) -> bool {
    speaker == frequency || (staffing.works(speaker) && staffing.works(frequency))
}

/// The position the player speaks as for this instruction, if any.
pub fn issuer_for(clearance: &Clearance, frequency: Position, staffing: Staffing) -> Option<Position> {
    clearance
        .issuers()
        .iter()
        .copied()
        .find(|&p| staffing.works(p) && can_speak(p, frequency, staffing))
}

/// A radio call waiting for the pilot to respond.
#[derive(Debug, Clone)]
pub(super) struct RadioCall {
    pub due: f32,
    pub aircraft: u32,
    /// Each instruction with the position giving it, `None` when nobody can.
    pub clearances: Vec<(Clearance, Option<Position>)>,
    /// Shown in the comms log.
    pub logged: bool,
}

pub(super) fn thousands(altitude: f32) -> String {
    let a = altitude.round() as i32;
    if a >= 1000 {
        format!("{},{:03}", a / 1000, a % 1000)
    } else {
        a.to_string()
    }
}

/// The node a new taxi route starts from: where the aircraft is, or the next
/// node if it's between two.
pub fn taxi_origin(ac: &Aircraft, map: &GroundMap) -> usize {
    match &ac.taxi {
        Some(t) => match t.next() {
            Some(next) if map.nodes[t.node].pos.distance(ac.pos) > AT_NODE_NM => next,
            _ => t.node,
        },
        None => map.nearest_node(ac.pos),
    }
}

/// A runway as a controller names it: the end in use if one is.
pub fn spoken_runway(airport: &Airport, map: &GroundMap, physical: usize) -> String {
    let ends = &map.runways[physical].ends;
    ends.iter()
        .map(|&e| airport.runways[e].id.clone())
        .find(|id| airport.configs.iter().any(|c| c.arrival == *id || c.departure == *id))
        .unwrap_or_else(|| airport.runways[ends[0]].id.clone())
}

fn taxi_phrase(to: usize, via: &[String], ac: &Aircraft, airport: &Airport) -> String {
    let Some(map) = airport.surface.as_ref() else { return "taxi".into() };
    let target = if let Some(spot) = map.parking.iter().find(|p| p.node == to) {
        format!("taxi to {}", spot.name)
    } else if let Some(end) = (0..airport.runways.len()).find(|&e| map.departure_hold(e) == Some(to))
    {
        format!("runway {}, taxi", airport.runways[end].id)
    } else {
        "taxi".into()
    };
    let Some(route) = map.route(taxi_origin(ac, map), to, via) else { return target };
    let mut words = target;
    if !route.taxiways.is_empty() {
        words += &format!(" via {}", route.taxiways.join(", "));
    }
    for &r in &route.runways_entered {
        words += &format!(", hold short of runway {}", spoken_runway(airport, map, r));
    }
    words
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
        Clearance::Taxi { to, via } => taxi_phrase(*to, via, ac, airport),
        Clearance::HoldPosition => "hold position".into(),
        Clearance::ContinueTaxi => "continue taxi".into(),
        Clearance::CrossRunway(r) => match airport.surface.as_ref() {
            Some(map) => format!("cross runway {}", spoken_runway(airport, map, *r)),
            None => "cross the runway".into(),
        },
        Clearance::LineUpAndWait(e) => {
            format!("runway {}, line up and wait", airport.runways[*e].id)
        }
        Clearance::ClearedForTakeoff(e) => {
            let runway = &airport.runways[*e];
            match ac.vfr.as_ref().map(|v| (v.intent, v.outbound)) {
                Some((VfrIntent::Closed, _)) => format!(
                    "runway {}, make {} closed traffic, cleared for takeoff",
                    runway.id,
                    side_word(runway)
                ),
                Some((VfrIntent::Departure, bearing)) => format!(
                    "runway {}, {} departure approved, cleared for takeoff",
                    runway.id,
                    compass(bearing)
                ),
                _ => format!("runway {}, cleared for takeoff", runway.id),
            }
        }
        Clearance::ClearedToLand(e) => {
            format!("runway {}, cleared to land", airport.runways[*e].id)
        }
        Clearance::ContactGround => "contact ground".into(),
        Clearance::ContactTower => "contact tower".into(),
        Clearance::ContactDeparture if ac.vfr.is_some() => "frequency change approved".into(),
        Clearance::ContactDeparture => "contact departure".into(),
        Clearance::EnterDownwind(e) => {
            let runway = &airport.runways[*e];
            format!("enter {} downwind runway {}", side_word(runway), runway.id)
        }
        Clearance::ExtendDownwind => "extend downwind, I'll call your base".into(),
        Clearance::TurnBase => "turn base".into(),
        Clearance::Follow { traffic, .. } => format!("follow the {traffic}"),
        Clearance::ClearedTouchAndGo(e) => {
            format!("runway {}, cleared touch-and-go", airport.runways[*e].id)
        }
        Clearance::ClearedOption(e) => {
            format!("runway {}, cleared for the option", airport.runways[*e].id)
        }
    }
}

/// What `apply` needs to know about the world.
struct Context<'a> {
    airport: &'a Airport,
    arrival_runway: usize,
    now: f32,
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

    /// Is this aircraft talking to a position the player works?
    pub fn is_players(&self, ac: &Aircraft) -> bool {
        self.config.staffing.works(ac.frequency())
    }

    /// Send a radio call as the player. The pilot responds after a short random delay.
    pub fn transmit(&mut self, id: u32, clearances: Vec<Clearance>) -> bool {
        let staffing = self.config.staffing;
        let Some(ac) = self.aircraft.iter().find(|a| a.id == id) else {
            return false;
        };
        let frequency = ac.frequency();
        let calls = clearances
            .into_iter()
            .map(|c| {
                let issuer = issuer_for(&c, frequency, staffing);
                (c, issuer)
            })
            .collect();
        self.send(id, calls, None)
    }

    /// Send a radio call from an automated position.
    pub(super) fn issue(&mut self, id: u32, clearances: Vec<Clearance>, from: Position) {
        let calls = clearances.into_iter().map(|c| (c, Some(from))).collect();
        self.send(id, calls, Some(from));
    }

    /// True while an instruction to this aircraft is waiting for the pilot.
    pub(super) fn call_pending(&self, id: u32) -> bool {
        self.radio.iter().any(|c| c.aircraft == id)
    }

    fn send(
        &mut self,
        id: u32,
        clearances: Vec<(Clearance, Option<Position>)>,
        automated: Option<Position>,
    ) -> bool {
        if clearances.is_empty() {
            return false;
        }
        let Some(ac) = self.aircraft.iter().find(|a| a.id == id) else {
            return false;
        };
        let words: Vec<String> =
            clearances.iter().map(|(c, _)| phrase(c, ac, &self.airport)).collect();
        // Automated controllers are only heard when the player works in the tower.
        let logged = automated.is_none() || self.config.staffing.tower();
        let line = match automated {
            Some(from) => format!("{}: {}, {}.", from.name(), ac.callsign, words.join(", ")),
            None => format!("{}, {}.", ac.callsign, words.join(", ")),
        };
        let delay = PILOT_DELAY.0 + self.rng.f32() * (PILOT_DELAY.1 - PILOT_DELAY.0);
        self.radio.push(RadioCall { due: self.time + delay, aircraft: id, clearances, logged });
        if logged {
            self.say(Speaker::Controller, line);
        }
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
        let staffing = self.config.staffing;
        let ctx = Context {
            airport: &self.airport,
            arrival_runway: self.arrival_runway(),
            now: self.time,
        };
        let mut readback = Vec::new();
        let mut events = Vec::new();
        let mut rolled = false;
        let mut new_frequency = None;
        for (clearance, issuer) in &call.clearances {
            let ac = &mut self.aircraft[index];
            let words = phrase(clearance, ac, ctx.airport);
            let before = ac.frequency();
            let result = match issuer {
                Some(p) if can_speak(*p, before, staffing) => apply(clearance, ac, &ctx),
                _ => Err("not on your frequency".to_string()),
            };
            match result {
                Ok(()) => {
                    readback.push(words);
                    if *clearance == Clearance::GoAround {
                        let callsign = ac.callsign.clone();
                        events.push(SimEvent::GoAround { callsign, reason: GoAroundReason::Ordered });
                    }
                    if matches!(clearance, Clearance::ClearedForTakeoff(_)) && ac.rolling() {
                        rolled = true;
                    }
                    let after = ac.frequency();
                    // Checking in on a new frequency the player works, from one they don't.
                    if after != before
                        && staffing.works(after)
                        && !issuer.is_some_and(|p| staffing.works(p))
                    {
                        new_frequency = Some(after);
                    }
                }
                Err(reason) => readback.push(format!("unable {words}, {reason}")),
            }
        }
        let callsign = self.aircraft[index].callsign.clone();
        if call.logged {
            let line = format!("{}, {}.", readback.join(", "), callsign);
            self.say(Speaker::Pilot, line);
        }
        for event in events {
            self.emit(event);
        }
        if rolled {
            self.begin_takeoff(index);
        }
        if let Some(frequency) = new_frequency {
            self.check_in(index, frequency);
        }
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

/// Start the takeoff roll from where the aircraft is lined up.
pub(super) fn start_roll(ac: &mut Aircraft, airport: &Airport) {
    let Some(end) = ac.departure_end else { return };
    let heading = airport.runways[end].heading;
    ac.phase = Phase::TowerRelease;
    ac.taxi = None;
    ac.heading = heading;
    ac.lateral = Lateral::Heading(heading);
    ac.ias = 0.0;
    ac.cleared_takeoff = false;
}

fn surface<'a>(ac: &Aircraft, airport: &'a Airport) -> Result<&'a GroundMap, String> {
    if !ac.on_ground {
        return Err("we're airborne".into());
    }
    airport.surface.as_ref().ok_or_else(|| "no taxi chart".to_string())
}

/// Replace an aircraft's taxi route, starting from `origin`.
fn set_route(ac: &mut Aircraft, map: &GroundMap, origin: usize, nodes: Vec<usize>) {
    let at_origin = map.nodes[origin].pos.distance(ac.pos) < AT_NODE_NM;
    let taxi = ac.taxi.get_or_insert_with(|| Taxi::at(origin));
    if at_origin {
        taxi.node = origin;
    }
    // The first node is where it is; otherwise it's still heading there.
    taxi.index = usize::from(at_origin);
    taxi.path = nodes;
    taxi.holding = false;
    taxi.holding_short = None;
}

/// A VFR aircraft in the pattern, and the leg it's on.
fn pattern_leg(ac: &Aircraft) -> Result<Leg, String> {
    match &ac.vfr {
        Some(v) if ac.phase == Phase::Pattern && !ac.on_ground => Ok(v.leg),
        Some(_) => Err("not in the pattern".into()),
        None => Err("we're IFR".into()),
    }
}

/// Clear a VFR aircraft in the pattern to land, touch and go, or for the option.
fn clear_vfr_landing(ac: &mut Aircraft, end: usize, kind: Landing, airport: &Airport) -> Result<(), String> {
    let leg = pattern_leg(ac)?;
    if matches!(leg, Leg::Entry | Leg::Holding | Leg::Departing) {
        return Err("not in the pattern".into());
    }
    let vfr = ac.vfr.as_mut().expect("checked");
    if vfr.end != end {
        return Err(format!("we're in the pattern for {}", airport.runways[vfr.end].id));
    }
    vfr.landing = Some(landing_for(kind, vfr)?);
    Ok(())
}

/// Validate one clearance and carry it out, or explain why not.
fn apply(clearance: &Clearance, ac: &mut Aircraft, ctx: &Context) -> Result<(), String> {
    let airport = ctx.airport;
    // Radar vectors don't apply to VFR traffic flying its own pattern.
    if ac.vfr.is_some() && clearance.issuers() == [Position::Approach] {
        return Err("we're VFR".into());
    }
    match clearance {
        Clearance::Heading(h) => {
            if ac.on_ground {
                return Err("we're on the ground".into());
            }
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
            check_ils(ac, *r, airport, ctx.arrival_runway)?;
            ac.phase = Phase::ClearedApproach;
            ac.approach_runway = Some(*r);
        }
        Clearance::GoAround => {
            let on_approach = matches!(
                ac.phase,
                Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff
            );
            let vfr_landing = pattern_leg(ac).is_ok_and(|l| matches!(l, Leg::Base | Leg::Final));
            if !(on_approach || vfr_landing) || ac.on_ground {
                return Err("not on approach".into());
            }
            ac.start_go_around(airport);
        }
        Clearance::HandoffTower => {
            if !matches!(ac.phase, Phase::Localizer | Phase::Glideslope) {
                return Err("not established".into());
            }
            ac.phase = Phase::TowerHandedOff;
            ac.freq_since = ctx.now;
        }
        Clearance::HandoffCenter => {
            if ac.flow != Flow::Departure || ac.phase != Phase::Controlled {
                return Err("not a departure".into());
            }
            ac.phase = Phase::OfferedToCenter;
        }
        Clearance::Taxi { to, via } => {
            let map = surface(ac, airport)?;
            if !matches!(ac.phase, Phase::Parked | Phase::Taxiing) {
                return Err("not taxiing".into());
            }
            let origin = taxi_origin(ac, map);
            let route = map.route(origin, *to, via).ok_or_else(|| "no route that way".to_string())?;
            set_route(ac, map, origin, route.nodes);
            // Keep a clearance for a runway it's on, so it can get off it.
            let taxi = ac.taxi.as_mut().expect("just set");
            let here = map.nodes[taxi.node].zones.clone();
            taxi.cleared.retain(|r| here.contains(r));
            ac.phase = Phase::Taxiing;
        }
        Clearance::HoldPosition => {
            surface(ac, airport)?;
            let taxi = ac.taxi.as_mut().filter(|_| ac.phase == Phase::Taxiing);
            taxi.ok_or_else(|| "not taxiing".to_string())?.holding = true;
        }
        Clearance::ContinueTaxi => {
            surface(ac, airport)?;
            let taxi = ac.taxi.as_mut().filter(|_| ac.phase == Phase::Taxiing);
            taxi.ok_or_else(|| "not taxiing".to_string())?.holding = false;
        }
        Clearance::CrossRunway(r) => {
            let map = surface(ac, airport)?;
            let taxi = ac.taxi.as_mut().filter(|_| ac.phase == Phase::Taxiing);
            let taxi = taxi.ok_or_else(|| "not taxiing".to_string())?;
            let ahead = taxi.path[taxi.index.min(taxi.path.len())..]
                .iter()
                .any(|&n| map.in_zone(n, *r));
            if !ahead {
                return Err("we don't cross that runway".into());
            }
            if !taxi.cleared.contains(r) {
                taxi.cleared.push(*r);
            }
            if taxi.holding_short == Some(*r) {
                taxi.holding_short = None;
            }
        }
        Clearance::LineUpAndWait(end) | Clearance::ClearedForTakeoff(end) => {
            let map = surface(ac, airport)?;
            let takeoff = matches!(clearance, Clearance::ClearedForTakeoff(_));
            if ac.flow != Flow::Departure {
                return Err("we're not departing".into());
            }
            if ac.phase == Phase::LinedUp {
                if ac.departure_end != Some(*end) {
                    let ours = ac.departure_end.unwrap_or(*end);
                    return Err(format!("we're lined up on {}", airport.runways[ours].id));
                }
                if takeoff {
                    start_roll(ac, airport);
                }
                return Ok(());
            }
            if ac.phase != Phase::Taxiing {
                return Err("not ready".into());
            }
            let origin = taxi_origin(ac, map);
            let route = map
                .route(origin, map.lineup_node[*end], &[])
                .ok_or_else(|| "can't get to that runway".to_string())?;
            let physical = map.runway_of_end[*end];
            set_route(ac, map, origin, route.nodes);
            let taxi = ac.taxi.as_mut().expect("just set");
            if !taxi.cleared.contains(&physical) {
                taxi.cleared.push(physical);
            }
            ac.departure_end = Some(*end);
            ac.cleared_takeoff = takeoff;
        }
        Clearance::ClearedToLand(end) if ac.vfr.is_some() => {
            clear_vfr_landing(ac, *end, Landing::FullStop, airport)?;
        }
        Clearance::ClearedTouchAndGo(end) => clear_vfr_landing(ac, *end, Landing::TouchAndGo, airport)?,
        Clearance::ClearedOption(end) => clear_vfr_landing(ac, *end, Landing::Option, airport)?,
        Clearance::EnterDownwind(end) => {
            let leg = pattern_leg(ac)?;
            if !matches!(leg, Leg::Entry | Leg::Holding) {
                return Err("we're already in the pattern".into());
            }
            let vfr = ac.vfr.as_mut().expect("checked");
            if vfr.end != *end {
                return Err(format!("we'd like runway {}", airport.runways[vfr.end].id));
            }
            vfr.cleared_pattern = true;
        }
        Clearance::ExtendDownwind => {
            let leg = pattern_leg(ac)?;
            if !matches!(leg, Leg::Entry | Leg::Join | Leg::Upwind | Leg::Crosswind | Leg::Downwind) {
                return Err("past the downwind".into());
            }
            let vfr = ac.vfr.as_mut().expect("checked");
            vfr.extend = true;
            vfr.turn_base = false;
        }
        Clearance::TurnBase => {
            let leg = pattern_leg(ac)?;
            if !matches!(leg, Leg::Join | Leg::Downwind) {
                return Err("not on downwind".into());
            }
            let vfr = ac.vfr.as_mut().expect("checked");
            vfr.turn_base = true;
            vfr.extend = false;
        }
        Clearance::Follow { id, .. } => {
            let leg = pattern_leg(ac)?;
            if *id == ac.id || matches!(leg, Leg::Base | Leg::Final | Leg::Departing) {
                return Err("unable to follow".into());
            }
            ac.vfr.as_mut().expect("checked").follow = Some(*id);
        }
        Clearance::ContactDeparture if ac.vfr.is_some() => {
            let leg = pattern_leg(ac)?;
            let vfr = ac.vfr.as_mut().expect("checked");
            if vfr.intent != VfrIntent::Departure || !matches!(leg, Leg::Upwind | Leg::Departing) {
                return Err("we're staying with you".into());
            }
            vfr.released = true;
        }
        Clearance::ClearedToLand(end) => {
            if ac.on_ground || ac.flow != Flow::Arrival {
                return Err("not landing".into());
            }
            if !matches!(
                ac.phase,
                Phase::TowerHandedOff | Phase::Localizer | Phase::Glideslope | Phase::ClearedApproach
            ) {
                return Err("not on approach".into());
            }
            if ac.approach_runway != Some(*end) {
                return Err("that's not our runway".into());
            }
            ac.cleared_to_land = true;
            if matches!(ac.phase, Phase::Localizer | Phase::Glideslope) {
                ac.phase = Phase::TowerHandedOff;
            }
        }
        Clearance::ContactGround => {
            surface(ac, airport)?;
            if ac.phase != Phase::Taxiing {
                return Err("not taxiing".into());
            }
            ac.surface_freq = Position::Ground;
            ac.freq_since = ctx.now;
            ac.waiting_since = None;
        }
        Clearance::ContactTower => {
            surface(ac, airport)?;
            if !matches!(ac.phase, Phase::Parked | Phase::Taxiing) {
                return Err("not taxiing".into());
            }
            ac.surface_freq = Position::Local;
            ac.freq_since = ctx.now;
            ac.waiting_since = None;
        }
        Clearance::ContactDeparture => {
            if ac.on_ground || !matches!(ac.phase, Phase::TowerRelease | Phase::GoAround) {
                return Err("not departing".into());
            }
            ac.phase = Phase::Controlled;
            if ac.flow == Flow::Departure && ac.route_index < ac.route.len() {
                ac.lateral = Lateral::Route;
            }
            ac.freq_since = ctx.now;
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
    /// Surface and runway instructions, in the order given.
    pub tower: Vec<Clearance>,
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

    /// Add a surface or runway instruction, replacing one of the same kind.
    pub fn add_tower(&mut self, clearance: Clearance) {
        let kind = std::mem::discriminant(&clearance);
        self.tower.retain(|c| std::mem::discriminant(c) != kind);
        self.tower.push(clearance);
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
        out.extend(self.tower.iter().cloned());
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

    #[test]
    fn combined_positions_speak_on_each_others_frequencies() {
        use Position::*;
        let land = Clearance::ClearedToLand(0);
        assert_eq!(issuer_for(&land, Local, Staffing::LOCAL), Some(Local));
        assert_eq!(issuer_for(&land, Local, Staffing::APPROACH), None);
        let taxi = Clearance::Taxi { to: 0, via: vec![] };
        // Local+Ground: Tower can taxi an aircraft still on Ground, and Ground one on Tower.
        assert!(issuer_for(&taxi, Ground, Staffing::LOCAL_GROUND).is_some());
        assert_eq!(issuer_for(&Clearance::CrossRunway(0), Ground, Staffing::LOCAL_GROUND), Some(Local));
        // Split: Tower can't clear an aircraft still talking to Ground.
        assert_eq!(issuer_for(&Clearance::CrossRunway(0), Ground, Staffing::LOCAL), None);
        assert_eq!(issuer_for(&Clearance::Heading(90.0), Approach, Staffing::ALL), Some(Approach));
    }

    #[test]
    fn a_new_tower_instruction_replaces_one_of_the_same_kind() {
        let mut p = PendingClearance::default();
        p.add_tower(Clearance::Taxi { to: 1, via: vec![] });
        p.add_tower(Clearance::HoldPosition);
        p.add_tower(Clearance::Taxi { to: 2, via: vec![] });
        assert_eq!(
            p.to_clearances(),
            vec![Clearance::HoldPosition, Clearance::Taxi { to: 2, via: vec![] }]
        );
    }
}
