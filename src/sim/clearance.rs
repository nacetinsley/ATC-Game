//! Clearances: what the controller says, who may say it, and how the pilot responds.

use super::aircraft::{Aircraft, Flow, Landing, Lateral, Leg, Phase, StripMark, Taxi, VfrIntent};
use super::airspace::Airport;
use super::approach::{ApproachFacts, ApproachKind, VISUAL_MIN_NM};

use super::ground_map::GroundMap;
use super::holding::{HoldSpec, HoldState};
use super::positions::{Position, SOLO, Seat, Staffing};
use super::scoring::{GoAroundReason, SimEvent};
use super::units::{format_heading, heading_diff, normalize_heading};
use super::vfr::{compass, landing_for, side_word};
use super::{Speaker, World};
use serde::{Deserialize, Serialize};

pub const MIN_ASSIGNED_ALTITUDE: f32 = 1000.0;
pub const MAX_ASSIGNED_ALTITUDE: f32 = 17000.0;
/// 7110.65 5-9-2, TBL 5-9-1: the steepest intercept of the final approach
/// course 2 miles or more outside the approach gate...
pub const MAX_INTERCEPT_DEG: f32 = 30.0;
/// ... and closer than that, degrees.
pub const MAX_INTERCEPT_NEAR_GATE_DEG: f32 = 20.0;
/// No speed adjustments inside this distance from the runway (7110.65 5-7-2), NM.
pub const NO_SPEED_INSIDE_NM: f32 = 5.0;
/// An approach clearance cancels any assigned speed (7110.65 5-7-2): the
/// pilot flies this, or slower, until the approach's own schedule takes
/// over, knots.
pub const OWN_APPROACH_SPEED: f32 = 210.0;
/// Pilot response time range, seconds.
pub(super) const PILOT_DELAY: (f32, f32) = (2.0, 6.0);
/// Below this distance there is no room left to establish and hand to Tower.
const MIN_APPROACH_CLEARANCE_NM: f32 = 6.0;
const MAX_APPROACH_CLEARANCE_NM: f32 = 35.0;
/// How far above the glideslope an aircraft can be and still capture it from below.
const ABOVE_GLIDESLOPE_TOLERANCE: f32 = 200.0;
/// Closer than this to a node, an aircraft is at it, NM.
pub(super) const AT_NODE_NM: f32 = 1e-4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Clearance {
    Heading(f32),
    Altitude(f32),
    Speed(f32),
    DirectFix(usize),
    ResumeRoute,
    ClearedIls(usize),
    /// The RNAV (GPS) approach to this runway end: from a published fix, or
    /// on vectors to final.
    ClearedRnav(usize),
    /// A visual approach to this runway end, following traffic if there's
    /// some: its id and the traffic call that names it.
    ClearedVisual { runway: usize, follow: Option<(u32, String)> },
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
    /// Hold at a fix.
    Hold(HoldSpec),
    /// An IFR clearance from Delivery: the departure route, the altitude to
    /// expect, and the beacon code.
    Ifr { route: Vec<usize>, altitude: f32, squawk: u16 },
    /// Reset the transponder to this code: radar's fix for a wrong squawk.
    Squawk(u16),
    /// Abandon a takeoff roll that's begun: "stop immediately".
    StopTakeoff,
    /// Approve the deviation round weather the pilot asked for.
    Deviation,
}

impl Clearance {
    /// Positions that can give this instruction.
    pub fn issuers(&self) -> &'static [Position] {
        use Position::*;
        match self {
            Clearance::Altitude(_)
            | Clearance::Speed(_)
            | Clearance::DirectFix(_)
            | Clearance::ResumeRoute
            | Clearance::ClearedIls(_)
            | Clearance::ClearedRnav(_)
            | Clearance::ClearedVisual { .. }
            | Clearance::HandoffTower
            | Clearance::HandoffCenter
            | Clearance::Deviation => &[Approach],
            // Tower assigns headings too: to a departure, or a go-around.
            Clearance::Heading(_) => &[Approach, Local],
            Clearance::GoAround => &[Local, Approach],
            Clearance::Taxi { .. } | Clearance::HoldPosition | Clearance::ContinueTaxi => {
                &[Ground, Local]
            }
            // Ground crosses runways too, having asked Local first.
            Clearance::CrossRunway(_) => &[Local, Ground],
            Clearance::LineUpAndWait(_)
            | Clearance::ClearedForTakeoff(_)
            | Clearance::ClearedToLand(_)
            | Clearance::ContactDeparture
            | Clearance::EnterDownwind(_)
            | Clearance::ExtendDownwind
            | Clearance::TurnBase
            | Clearance::Follow { .. }
            | Clearance::ClearedTouchAndGo(_)
            | Clearance::ClearedOption(_)
            | Clearance::StopTakeoff => &[Local],
            Clearance::ContactTower => &[Ground],
            // Local after landing; Delivery once it has its clearance.
            Clearance::ContactGround => &[Local, Delivery],
            Clearance::Hold(_) => &[Approach, Local],
            Clearance::Ifr { .. } => &[Delivery],
            Clearance::Squawk(_) => &[Approach],
        }
    }
}

/// Can `speaker` talk to an aircraft on `frequency`? Its own frequency, or any
/// frequency the same player also works: that's what a combined position is.
pub fn can_speak(speaker: Position, frequency: Position, staffing: Staffing) -> bool {
    speaker == frequency || staffing.same_seat(speaker, frequency)
}

/// The position the player in `seat` speaks as for this instruction, if any.
pub fn issuer_for(clearance: &Clearance, frequency: Position, staffing: Staffing, seat: Seat) -> Option<Position> {
    clearance
        .issuers()
        .iter()
        .copied()
        .find(|&p| staffing.seat_works(seat, p) && can_speak(p, frequency, staffing))
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

pub fn thousands(altitude: f32) -> String {
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

/// A runway as a controller names it: the end in use now (`in_use`, from
/// `World::ends_in_use`), else an end some configuration uses, else the first.
pub fn spoken_runway(airport: &Airport, map: &GroundMap, physical: usize, in_use: &[usize]) -> String {
    let ends = &map.runways[physical].ends;
    let end = ends.iter().copied().find(|e| in_use.contains(e)).or_else(|| {
        ends.iter().copied().find(|&e| {
            let id = &airport.runways[e].id;
            airport.configs.iter().any(|c| c.arrival == *id || c.departure == *id)
        })
    });
    airport.runways[end.unwrap_or(ends[0])].id.clone()
}

fn taxi_phrase(to: usize, via: &[String], ac: &Aircraft, airport: &Airport, in_use: &[usize]) -> String {
    let Some(map) = airport.surface.as_ref() else { return "taxi".into() };
    let target = if let Some(spot) = map.parking.iter().find(|p| p.node == to) {
        format!("taxi to {}", spot.name)
    } else if let Some(spot) = map.spots.iter().find(|s| s.node == to) {
        format!("taxi to {}", spot.spoken())
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
        words += &format!(", hold short of runway {}", spoken_runway(airport, map, r, in_use));
    }
    words
}

/// Tower's heading for a departure is at most this far off the runway, degrees.
pub const MAX_TOWER_TURN: f32 = 90.0;

/// The controller's wording, as sent. Decided from the aircraft's state at transmission.
/// `in_use` names runway ends the way `spoken_runway` does.
pub fn phrase(clearance: &Clearance, ac: &Aircraft, airport: &Airport, in_use: &[usize]) -> String {
    match clearance {
        // 7110.65 5-8-2: before departure, a turn off the runway heading.
        Clearance::Heading(h) if ac.on_ground => {
            let runway = ac.departure_end.map_or(ac.heading, |e| airport.runways[e].heading);
            let turn = heading_diff(runway, *h);
            if turn.abs() < 1.0 {
                "fly runway heading".into()
            } else {
                let dir = if turn > 0.0 { "right" } else { "left" };
                format!("turn {dir} heading {}", format_heading(airport.to_magnetic(*h)))
            }
        }
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
        // 7110.65 5-9-4: an altitude to maintain until established, then the clearance.
        Clearance::ClearedIls(r) if ac.lateral == Lateral::Localizer => {
            format!("cleared ILS runway {} approach", airport.runways[*r].id)
        }
        Clearance::ClearedIls(r) => format!(
            "maintain {} until established on the localizer, cleared ILS runway {} approach",
            thousands(ac.target_altitude),
            airport.runways[*r].id
        ),
        Clearance::ClearedRnav(r) => format!("cleared RNAV runway {} approach", airport.runways[*r].id),
        Clearance::ClearedVisual { runway, follow } => {
            let cleared = format!("cleared visual approach runway {}", airport.runways[*runway].id);
            match follow {
                Some((_, traffic)) => format!("follow the {traffic}, {cleared}"),
                None => cleared,
            }
        }
        Clearance::GoAround => "go around".into(),
        Clearance::HandoffTower => "contact tower".into(),
        Clearance::HandoffCenter => "contact center".into(),
        Clearance::Taxi { to, via } => taxi_phrase(*to, via, ac, airport, in_use),
        Clearance::HoldPosition => "hold position".into(),
        Clearance::ContinueTaxi => "continue taxi".into(),
        // 7110.65 3-7-2: the runway, and where to cross it.
        Clearance::CrossRunway(r) => match airport.surface.as_ref() {
            Some(map) => {
                let runway = spoken_runway(airport, map, *r, in_use);
                let at = ac.taxi.as_ref().and_then(|t| {
                    let ahead: Vec<usize> = std::iter::once(t.node).chain(t.path.iter().skip(t.index).copied()).collect();
                    map.crossing_taxiway(&ahead, *r)
                });
                match at {
                    Some(taxiway) => format!("cross runway {runway} at {taxiway}"),
                    None => format!("cross runway {runway}"),
                }
            }
            None => "cross the runway".into(),
        },
        // Already in position: told to stay there (7110.65 3-9-4).
        Clearance::LineUpAndWait(e) if ac.phase == Phase::LinedUp && ac.departure_end == Some(*e) => {
            format!("runway {}, continue holding", airport.runways[*e].id)
        }
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
        Clearance::Hold(spec) => hold_phrase(spec, airport),
        Clearance::Ifr { route, altitude, squawk } => {
            super::delivery::phrase(ac, route, *altitude, *squawk, airport)
        }
        Clearance::Squawk(code) => format!("reset transponder, squawk {}", super::delivery::code_text(*code)),
        Clearance::StopTakeoff => "stop immediately, I say again, stop immediately".into(),
        Clearance::Deviation => match ac.deviation {
            Some(d) => format!("deviation {} approved, advise clear of weather", d.spoken()),
            None => "deviation approved, advise clear of weather".into(),
        },
    }
}

/// 7110.65 4-6-4: the hold, then when to expect further clearance.
fn hold_phrase(spec: &HoldSpec, airport: &Airport) -> String {
    let fix = &airport.fixes[spec.fix].name;
    let side = compass(spec.direction());
    let efc = format!("expect further clearance {:04}", spec.efc_hhmm);
    if spec.published {
        format!("hold {side} of {fix} as published, {efc}")
    } else {
        format!(
            "hold {side} of {fix} on the {} course inbound, {} turns, {}, {efc}",
            format_heading(airport.to_magnetic(spec.inbound)),
            spec.turns.word(),
            spec.leg.words()
        )
    }
}

/// What `apply` needs to know about the world.
struct Context<'a> {
    airport: &'a Airport,
    now: f32,
    /// For the pilot's own slips, decided from the seed.
    seed: u64,
    mistakes: super::mistakes::PilotMistakes,
    storms: &'a super::storms::Storms,
    /// The approaches on offer, and what this pilot can see.
    approaches: ApproachFacts,
    /// The player works Approach, and takes over Tower's headings.
    player_approach: bool,
}

/// Who is sending a radio call.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sender {
    Player(Seat),
    Automated(Position),
}

impl World {
    /// Accept an aircraft Center is offering. Immediate: it's a landline, not the radio.
    pub fn accept_handoff(&mut self, id: u32) -> bool {
        self.accept_handoff_as(SOLO, id)
    }

    /// The player in `seat`, on Approach, accepts Center's offer.
    pub(super) fn accept_handoff_as(&mut self, seat: Seat, id: u32) -> bool {
        if !self.config.staffing.seat_works(seat, Position::Approach) {
            return false;
        }
        let Some(ac) = self.aircraft.iter_mut().find(|a| a.id == id) else {
            return false;
        };
        if ac.phase != Phase::Offered {
            return false;
        }
        ac.phase = Phase::Controlled;
        let line = format!("{}, radar contact.", ac.callsign);
        self.player_say(line, seat, Position::Approach);
        self.record(seat, id, super::replay::Action::AcceptHandoff);
        true
    }

    /// Is this aircraft talking to a position the player works?
    pub fn is_players(&self, ac: &Aircraft) -> bool {
        self.config.staffing.works(ac.frequency())
    }

    /// Send a radio call as the player. The pilot responds after a short random delay.
    pub fn transmit(&mut self, id: u32, clearances: Vec<Clearance>) -> bool {
        self.transmit_as(SOLO, id, clearances)
    }

    /// Send a radio call as the player in `seat`. An instruction only their
    /// positions could give it goes unanswered ("not on your frequency").
    pub(super) fn transmit_as(&mut self, seat: Seat, id: u32, clearances: Vec<Clearance>) -> bool {
        let staffing = self.config.staffing;
        let Some(ac) = self.aircraft.iter().find(|a| a.id == id) else {
            return false;
        };
        let frequency = ac.frequency();
        let calls = clearances
            .iter()
            .cloned()
            .map(|c| {
                let issuer = issuer_for(&c, frequency, staffing, seat);
                (c, issuer)
            })
            .collect();
        let sent = self.send(id, calls, Sender::Player(seat));
        if sent {
            self.record(seat, id, super::replay::Action::Transmit(clearances));
        }
        sent
    }

    /// Send a radio call from an automated position.
    pub(super) fn issue(&mut self, id: u32, clearances: Vec<Clearance>, from: Position) {
        let calls = clearances.into_iter().map(|c| (c, Some(from))).collect();
        self.send(id, calls, Sender::Automated(from));
    }

    /// True while an instruction to this aircraft is waiting for the pilot.
    pub(super) fn call_pending(&self, id: u32) -> bool {
        self.radio.iter().any(|c| c.aircraft == id)
    }

    fn send(&mut self, id: u32, clearances: Vec<(Clearance, Option<Position>)>, sender: Sender) -> bool {
        if clearances.is_empty() {
            return false;
        }
        let Some(ac) = self.aircraft.iter().find(|a| a.id == id) else {
            return false;
        };
        let in_use = self.ends_in_use();
        // Worded as the call goes out: an altitude in it is the one later
        // instructions refer to ("maintain 3,000 until established").
        let mut preview = ac.clone();
        for (c, _) in &clearances {
            if let Clearance::Altitude(a) = c {
                preview.target_altitude = *a;
            }
        }
        let words: Vec<String> = clearances
            .iter()
            .map(|(c, _)| {
                let mut words = phrase(c, &preview, &self.airport, &in_use);
                if let Some(traffic) = self.line_up_traffic(ac, c) {
                    words = format!("{words}, {traffic}");
                }
                if self.wake_caution(ac, c) { format!("{words}, caution wake turbulence") } else { words }
            })
            .collect();
        // Automated controllers are only heard when the player works in the tower.
        let logged = matches!(sender, Sender::Player(_)) || self.config.staffing.tower();
        let line = match sender {
            Sender::Automated(from) => format!("{}: {}, {}.", from.name(), ac.callsign, words.join(", ")),
            Sender::Player(_) => format!("{}, {}.", ac.callsign, words.join(", ")),
        };
        let frequency = ac.frequency();
        let delay = PILOT_DELAY.0 + self.rng.f32() * (PILOT_DELAY.1 - PILOT_DELAY.0);
        self.radio.push(RadioCall { due: self.time + delay, aircraft: id, clearances, logged });
        if logged {
            match sender {
                Sender::Player(seat) => self.player_say(line, seat, frequency),
                Sender::Automated(from) => self.automated_say(line, from),
            }
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
        if self.aircraft[index].nordo() {
            self.answer_nordo(index, &call);
            return;
        }
        let staffing = self.config.staffing;
        // Ground crossing a runway Local hasn't agreed to.
        let uncoordinated: Vec<bool> = call
            .clearances
            .iter()
            .map(|(c, issuer)| match c {
                Clearance::CrossRunway(r) => {
                    *issuer == Some(Position::Ground)
                        && staffing.ground
                        && !staffing.local
                        && !self.crossing_approved(call.aircraft, *r)
                }
                _ => false,
            })
            .collect();
        // Local launching a departure Approach hasn't released.
        let unreleased: Vec<bool> = call
            .clearances
            .iter()
            .map(|(c, issuer)| {
                matches!(c, Clearance::ClearedForTakeoff(_))
                    && *issuer == Some(Position::Local)
                    && self.needs_release(call.aircraft)
            })
            .collect();
        // The player breaking a line up and wait rule, judged as it's heard.
        let line_up: Vec<Option<String>> = call
            .clearances
            .iter()
            .map(|(c, issuer)| {
                let player = issuer.is_some_and(|p| staffing.works(p));
                if player { self.line_up_breach(&self.aircraft[index], c) } else { None }
            })
            .collect();
        let in_use = self.ends_in_use();
        let ctx = Context {
            approaches: self.approach_facts(index, &call.clearances),
            airport: &self.airport,
            now: self.time,
            seed: self.config.seed,
            mistakes: self.config.mistakes,
            storms: &self.storms,
            player_approach: staffing.approach,
        };
        let heard_on = self.aircraft[index].frequency();
        let mut readback = Vec::new();
        let mut events = Vec::new();
        let mut rolled = false;
        let mut new_frequency = None;
        let mut player_go_around = None;
        for (n, (clearance, issuer)) in call.clearances.iter().enumerate() {
            let ac = &mut self.aircraft[index];
            let words = phrase(clearance, ac, ctx.airport, &in_use);
            let repeat = match clearance {
                Clearance::Ifr { route, altitude, squawk } => {
                    super::delivery::is_repeat(ac, route, *altitude, *squawk)
                }
                _ => false,
            };
            let before = ac.frequency();
            let landing_on = ac.approach_runway;
            let result = match issuer {
                Some(p) if can_speak(*p, before, staffing) => apply(clearance, ac, &ctx),
                _ => Err("not on your frequency".to_string()),
            };
            match result {
                Ok(()) => {
                    // 7110.65 5-7-2: an approach clearance cancels an assigned
                    // speed, unless it restates one.
                    let restated = call.clearances.iter().any(|(c, _)| matches!(c, Clearance::Speed(_)));
                    if is_approach_clearance(clearance) && !restated {
                        ac.target_speed = ac.ty.max_speed.min(OWN_APPROACH_SPEED);
                    }
                    // The player's instructions can be misheard, or answer a slip.
                    let player = issuer.is_some_and(|p| staffing.works(p));
                    let heard = if player {
                        super::mistakes::after_instruction(ac, clearance, ctx.mistakes, ctx.seed, ctx.now, ctx.airport)
                    } else {
                        Default::default()
                    };
                    events.extend(heard.events);
                    if player && *clearance == Clearance::GoAround {
                        player_go_around = landing_on;
                    }
                    // An IFR clearance is read back as the pilot heard it.
                    let words = match clearance {
                        Clearance::Ifr { .. } => super::delivery::readback(ac, repeat, ctx.airport),
                        Clearance::Deviation => match ac.deviation {
                            Some(d) => format!("deviating {}, will advise clear of weather", d.spoken()),
                            None => words,
                        },
                        _ => heard.readback.unwrap_or(words),
                    };
                    readback.push(words);
                    if let Some(by) = *issuer {
                        ac.strip.push(StripMark { time: ctx.now, by, clearance: clearance.clone() });
                    }
                    if uncoordinated[n] {
                        events.push(SimEvent::UncoordinatedCrossing { callsign: ac.callsign.clone() });
                    }
                    if unreleased[n] {
                        events.push(SimEvent::UnreleasedDeparture { callsign: ac.callsign.clone() });
                    }
                    if let Some(what) = &line_up[n] {
                        events.push(SimEvent::LineUpAndWait { what: what.clone() });
                    }
                    if *clearance == Clearance::GoAround {
                        let callsign = ac.callsign.clone();
                        events.push(SimEvent::GoAround { callsign, reason: GoAroundReason::Ordered });
                    }
                    if matches!(clearance, Clearance::ClearedForTakeoff(_)) && ac.rolling() {
                        rolled = true;
                    }
                    let after = ac.frequency();
                    // Checking in on a new frequency a player works, from a
                    // controller who isn't them.
                    if after != before
                        && staffing.works(after)
                        && !issuer.is_some_and(|p| staffing.same_seat(p, after))
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
            self.pilot_say_on(Speaker::Pilot, callsign.clone(), line, heard_on);
        }
        for event in events {
            self.emit(event);
        }
        if let Some(end) = player_go_around {
            self.go_around_answers_major(end);
            self.go_around_started(call.aircraft, end, true);
        }
        if rolled {
            self.begin_takeoff(index);
        }
        if let Some(frequency) = new_frequency {
            self.check_in(index, frequency);
        }
    }
}

impl World {
    /// No radio. A landing clearance gets through as a steady green light from
    /// the tower; nothing else does.
    fn answer_nordo(&mut self, index: usize, call: &RadioCall) {
        let staffing = self.config.staffing;
        let ac = &mut self.aircraft[index];
        let callsign = ac.callsign.clone();
        let frequency = ac.frequency();
        let light = call.clearances.iter().any(|(c, issuer)| {
            matches!(c, Clearance::ClearedToLand(_))
                && issuer.is_some_and(|p| p == Position::Local && can_speak(p, frequency, staffing))
        });
        let line = if light && ac.phase == Phase::TowerHandedOff {
            ac.cleared_to_land = true;
            if let Some((c, _)) = call.clearances.iter().find(|(c, _)| matches!(c, Clearance::ClearedToLand(_))) {
                ac.strip.push(StripMark { time: self.time, by: Position::Local, clearance: c.clone() });
            }
            format!("Tower: steady green light to {callsign}. It rocks its wings.")
        } else {
            format!("No reply from {callsign}: radio failure.")
        };
        if call.logged {
            self.say(Speaker::System, line);
        }
    }
}

pub(super) fn break_approach(ac: &mut Aircraft) {
    if matches!(ac.phase, Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope) {
        ac.phase = Phase::Controlled;
        ac.approach_runway = None;
        // Hold the altitude reached: the pilot stops descending on the glideslope.
        if ac.on_glideslope || matches!(ac.lateral, Lateral::Visual(_)) {
            ac.target_altitude = ac.altitude;
        }
        if matches!(ac.lateral, Lateral::Visual(_) | Lateral::Localizer) {
            ac.lateral = Lateral::Heading(ac.heading.round());
        }
        ac.on_glideslope = false;
        ac.clear_approach();
    }
}

/// A departure leaving Tower: on course, unless it's flying a heading Tower
/// gave and the player, working Approach, takes over the vector.
pub(super) fn on_course(ac: &mut Aircraft, airport: &Airport, player_approach: bool) {
    if let Some(h) = ac.tower_heading.take() {
        ac.lateral = Lateral::Heading(h);
    }
    if ac.flow != Flow::Departure || ac.route_index >= ac.route.len() {
        return;
    }
    let runway = ac.departure_end.map(|e| airport.runways[e].heading);
    let vectored = matches!((ac.lateral, runway), (Lateral::Heading(h), Some(r)) if heading_diff(r, h).abs() > 1.0);
    if !(vectored && player_approach) {
        ac.lateral = Lateral::Route;
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
pub(super) fn set_route(ac: &mut Aircraft, map: &GroundMap, origin: usize, nodes: Vec<usize>) {
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
    taxi.lineup = false;
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
/// An ILS, RNAV or visual approach clearance.
pub fn is_approach_clearance(clearance: &Clearance) -> bool {
    matches!(clearance, Clearance::ClearedIls(_) | Clearance::ClearedRnav(_) | Clearance::ClearedVisual { .. })
}

fn apply(clearance: &Clearance, ac: &mut Aircraft, ctx: &Context) -> Result<(), String> {
    let airport = ctx.airport;
    if let Some(reason) = super::emergency::refusal(ac, clearance, airport.elevation) {
        return Err(reason);
    }
    // Radar vectors don't apply to VFR traffic flying its own pattern.
    if ac.vfr.is_some() && (clearance.issuers() == [Position::Approach] || matches!(clearance, Clearance::Heading(_))) {
        return Err("we're VFR".into());
    }
    match clearance {
        Clearance::Heading(h) => {
            if ac.on_ground {
                // Tower's heading for after takeoff.
                let end = ac.departure_end.filter(|_| ac.flow == Flow::Departure);
                let Some(end) = end else { return Err("we're on the ground".into()) };
                let runway = &airport.runways[end];
                if heading_diff(runway.heading, *h).abs() > MAX_TOWER_TURN {
                    return Err(format!("too far off runway {}", runway.id));
                }
                ac.tower_heading = Some(normalize_heading(*h));
                return Ok(());
            }
            if matches!(ac.phase, Phase::TowerHandedOff | Phase::Rollout) {
                return Err("on final".into());
            }
            if super::storms::weather_ahead(ctx.storms, ac.pos, *h, super::storms::LOOKAHEAD_NM) {
                return Err("weather ahead".into());
            }
            break_approach(ac);
            ac.lateral = Lateral::Heading(normalize_heading(*h));
            ac.deviation = None;
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
            // 7110.65 5-7-2: no speed adjustments inside the final approach
            // fix or 5 miles from the runway.
            if let Some(rwy) = ac.approach_runway
                && ac.on_final()
                && airport.runways[rwy].along(ac.pos) < NO_SPEED_INSIDE_NM
            {
                return Err("inside the final approach fix".into());
            }
            if *s < ac.ty.min_speed || *s > ac.ty.max_speed {
                return Err(format!("{} performance", ac.ty.designator));
            }
            if *s > 250.0 && ac.altitude < 10000.0 {
                return Err("below 10,000 feet".into());
            }
            ac.target_speed = *s;
        }
        Clearance::DirectFix(f) => {
            let to_fix = airport.fixes[*f].pos() - ac.pos;
            let nm = to_fix.length().min(super::storms::LOOKAHEAD_NM);
            if super::storms::weather_ahead(ctx.storms, ac.pos, super::units::vec_heading(to_fix), nm) {
                return Err("weather that way".into());
            }
            break_approach(ac);
            ac.lateral = Lateral::Direct(*f);
            ac.deviation = None;
        }
        Clearance::ResumeRoute => {
            // Out of a hold at a fix on the route: on to the next one.
            if let Lateral::Hold(state) = ac.lateral
                && ac.route.get(ac.route_index) == Some(&state.spec.fix)
            {
                ac.route_index += 1;
            }
            if ac.route_index >= ac.route.len() {
                return Err("no route remaining".into());
            }
            break_approach(ac);
            ac.lateral = Lateral::Route;
        }
        Clearance::Hold(spec) => {
            if ac.on_ground {
                return Err("we're on the ground".into());
            }
            if ac.vfr.is_some() {
                return Err("we're VFR".into());
            }
            if ac.on_glideslope || matches!(ac.phase, Phase::TowerHandedOff) {
                return Err("established on the approach".into());
            }
            break_approach(ac);
            ac.lateral = Lateral::Hold(HoldState::new(*spec));
        }
        Clearance::ClearedIls(_) | Clearance::ClearedRnav(_) | Clearance::ClearedVisual { .. }
            if matches!(ac.lateral, Lateral::Hold(_)) =>
        {
            return Err("in the hold, request vectors".into());
        }
        Clearance::ClearedIls(r) => {
            check_approach(ac, *r, ApproachKind::Ils, ctx)?;
            check_final(ac, *r, ApproachKind::Ils, airport)?;
            ac.phase = Phase::ClearedApproach;
            ac.approach_runway = Some(*r);
            ac.approach = ApproachKind::Ils;
            ac.visual_follow = None;
        }
        Clearance::ClearedRnav(r) => {
            check_approach(ac, *r, ApproachKind::Rnav, ctx)?;
            // Sent to one of the approach's fixes: it flies the rest of them.
            match ac.active_fix().and_then(|f| airport.rnav_route_from(*r, f)) {
                Some(route) => {
                    ac.route = route;
                    ac.route_index = 0;
                    ac.route_end_heading = Some(airport.runways[*r].heading);
                    ac.lateral = Lateral::Route;
                }
                None => check_final(ac, *r, ApproachKind::Rnav, airport)?,
            }
            ac.phase = Phase::ClearedApproach;
            ac.approach_runway = Some(*r);
            ac.approach = ApproachKind::Rnav;
            ac.visual_follow = None;
        }
        Clearance::ClearedVisual { runway, follow } => {
            check_approach(ac, *runway, ApproachKind::Visual, ctx)?;
            let facts = &ctx.approaches;
            let leader = match follow {
                Some((id, _)) => match facts.leaders.iter().find(|l| l.0 == *id) {
                    Some(&(_, landing, true)) if landing == Some(*runway) => Some(*id),
                    Some(&(_, Some(other), true)) => {
                        return Err(format!("that traffic's landing {}", airport.runways[other].id));
                    }
                    _ => return Err("traffic not in sight".into()),
                },
                None => None,
            };
            if !facts.field_in_sight {
                return Err("negative contact, field not in sight".into());
            }
            let rwy = &airport.runways[*runway];
            let established = ac.on_final() && ac.approach_runway == Some(*runway);
            if !established && (rwy.along(ac.pos) <= 0.0 || rwy.threshold().distance(ac.pos) < VISUAL_MIN_NM) {
                return Err("too close in for that".into());
            }
            if !established {
                ac.phase = Phase::ClearedApproach;
                ac.lateral = Lateral::Visual(*runway);
                ac.on_glideslope = false;
                ac.deviation = None;
            }
            ac.approach_runway = Some(*runway);
            ac.approach = ApproachKind::Visual;
            ac.visual_follow = leader;
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
            let visual = ac.phase == Phase::ClearedApproach && matches!(ac.lateral, Lateral::Visual(_));
            if !matches!(ac.phase, Phase::Localizer | Phase::Glideslope) && !visual {
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
        Clearance::Ifr { route, altitude, squawk } => {
            super::delivery::apply(ac, route, *altitude, *squawk, ctx.now, ctx.seed)?;
        }
        Clearance::Squawk(code) => super::delivery::reset_squawk(ac, *code)?,
        Clearance::Deviation => match ac.deviation.as_mut() {
            Some(d) if !d.approved => {
                d.approved = true;
                d.offset = d.wanted;
            }
            Some(_) => return Err("already deviating".into()),
            None => return Err("no deviation requested".into()),
        },
        Clearance::StopTakeoff => {
            if !(ac.on_ground && ac.phase == Phase::TowerRelease) {
                return Err("not rolling".into());
            }
            if ac.ias > super::mistakes::STOP_MAX_KT {
                return Err("too fast to stop".into());
            }
            // Stopped on the runway, lined up again.
            ac.phase = Phase::LinedUp;
            ac.ias = 0.0;
            ac.cleared_takeoff = false;
        }
        Clearance::Taxi { to, via } => {
            let map = surface(ac, airport)?;
            if !matches!(ac.phase, Phase::Parked | Phase::Taxiing) {
                return Err("not taxiing".into());
            }
            if ac.flight_plan.as_ref().is_some_and(|p| !p.is_cleared()) {
                return Err("we don't have our IFR clearance yet".into());
            }
            let origin = taxi_origin(ac, map);
            let route = map.route(origin, *to, via).ok_or_else(|| "no route that way".to_string())?;
            set_route(ac, map, origin, route.nodes);
            // Keep a clearance for a runway it's on, so it can get off it.
            let taxi = ac.taxi.as_mut().expect("just set");
            let here = map.nodes[taxi.node].zones.clone();
            taxi.cleared.retain(|r| here.contains(r));
            ac.phase = Phase::Taxiing;
            // Taxied straight from a combined Delivery: it's with Ground now.
            if ac.surface_freq == Position::Delivery {
                ac.surface_freq = Position::Ground;
                ac.freq_since = ctx.now;
            }
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
                } else {
                    // "Continue holding": the 90 seconds start again.
                    ac.in_position_since = Some(ctx.now);
                }
                return Ok(());
            }
            if ac.phase != Phase::Taxiing {
                return Err("not ready".into());
            }
            let origin = taxi_origin(ac, map);
            let nodes = map
                .lineup_route(&airport.runways, *end, origin, ac.ty.min_runway_ft)
                .ok_or_else(|| "can't get to that runway".to_string())?;
            let physical = map.runway_of_end[*end];
            set_route(ac, map, origin, nodes);
            let taxi = ac.taxi.as_mut().expect("just set");
            taxi.lineup = true;
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
            let at_gate = ac.phase == Phase::Parked && ac.surface_freq == Position::Delivery;
            if at_gate && ac.flight_plan.as_ref().is_some_and(|p| !p.is_cleared()) {
                return Err("we still need our clearance".into());
            }
            if ac.phase != Phase::Taxiing && !at_gate {
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
            on_course(ac, airport, ctx.player_approach);
            ac.freq_since = ctx.now;
        }
    }
    Ok(())
}

/// An arrival, to a runway offering this approach, in weather it can fly it in.
fn check_approach(ac: &Aircraft, rwy: usize, kind: ApproachKind, ctx: &Context) -> Result<(), String> {
    if ac.flow != Flow::Arrival {
        return Err("we're a departure".into());
    }
    if ac.on_ground {
        return Err("we're on the ground".into());
    }
    if let Some(reason) = ctx.approaches.refusal(kind, rwy, ctx.airport) {
        return Err(reason);
    }
    if ctx.approaches.below(kind) {
        return Err(match kind {
            ApproachKind::Visual => "not in visual conditions".into(),
            _ => "below our minimums, request holding".into(),
        });
    }
    Ok(())
}

/// Placed to join the final course on vectors: far enough out, below the
/// glidepath, and on a heading that will intercept.
fn check_final(ac: &Aircraft, rwy: usize, kind: ApproachKind, airport: &Airport) -> Result<(), String> {
    let runway = &airport.runways[rwy];
    let along = runway.along(ac.pos);
    if !(MIN_APPROACH_CLEARANCE_NM..=MAX_APPROACH_CLEARANCE_NM).contains(&along) {
        return Err("position not suitable for the approach".into());
    }
    if ac.altitude > airport.approach_altitude(rwy, along, kind) + ABOVE_GLIDESLOPE_TOLERANCE {
        return Err(if kind == ApproachKind::Ils { "above the glideslope" } else { "above the glidepath" }.into());
    }
    let heading = match ac.lateral {
        Lateral::Heading(h) => h,
        _ => ac.heading,
    };
    let intercept = heading_diff(runway.heading, heading);
    if intercept.abs() > MAX_INTERCEPT_DEG {
        return Err("intercept angle too steep".into());
    }
    // Joining the course less than 2 miles outside the approach gate: 20 degrees.
    let cross = runway.cross(ac.pos);
    let joins_at = along - cross.abs() / super::math::tan(intercept.abs().to_radians()).max(1e-3);
    if intercept.abs() > MAX_INTERCEPT_NEAR_GATE_DEG && joins_at - airport.approach_gate_nm(rwy) < 2.0 {
        return Err("intercept angle too steep".into());
    }
    // Right of the course must be heading left of it, and vice versa.
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
    pub hold: Option<HoldSpec>,
    /// The approach clearance: which kind, to which runway end.
    pub approach: Option<(ApproachKind, usize)>,
    /// Traffic to follow on a visual approach, with the call that names it.
    pub follow: Option<(u32, String)>,
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
        self.hold = None;
    }

    pub fn nudge_heading(&mut self, ac: &Aircraft, degrees: f32) {
        let base = self.heading.unwrap_or(ac.assigned_heading().unwrap_or(ac.heading.round()));
        self.set_heading(base + degrees);
    }

    pub fn nudge_altitude(&mut self, ac: &Aircraft, feet: f32) {
        let base = self.altitude.unwrap_or(ac.assigned_altitude());
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
        self.hold = None;
    }

    pub fn set_resume(&mut self) {
        self.resume = true;
        self.heading = None;
        self.direct = None;
        self.hold = None;
    }

    /// Hold at a fix. Replaces any heading, fix or route instruction, and
    /// pressing it again takes it back.
    pub fn set_hold(&mut self, spec: HoldSpec) {
        if self.hold.is_some_and(|h| h.fix == spec.fix) {
            self.hold = None;
            return;
        }
        self.hold = Some(spec);
        self.heading = None;
        self.direct = None;
        self.resume = false;
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
        if self.go_around {
            out.push(Clearance::GoAround);
        }
        if let Some(h) = self.heading {
            out.push(Clearance::Heading(h));
        }
        if let Some(f) = self.direct {
            out.push(Clearance::DirectFix(f));
        }
        if self.resume {
            out.push(Clearance::ResumeRoute);
        }
        if let Some(spec) = self.hold {
            out.push(Clearance::Hold(spec));
        }
        if let Some(a) = self.altitude {
            out.push(Clearance::Altitude(a));
        }
        if let Some(s) = self.speed {
            out.push(Clearance::Speed(s));
        }
        match self.approach {
            Some((ApproachKind::Ils, r)) => out.push(Clearance::ClearedIls(r)),
            Some((ApproachKind::Rnav, r)) => out.push(Clearance::ClearedRnav(r)),
            Some((ApproachKind::Visual, runway)) => {
                out.push(Clearance::ClearedVisual { runway, follow: self.follow.clone() })
            }
            None => {}
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
        let p = PendingClearance {
            heading: Some(240.0),
            approach: Some((ApproachKind::Ils, 0)),
            ..Default::default()
        };
        assert_eq!(p.to_clearances(), vec![Clearance::Heading(240.0), Clearance::ClearedIls(0)]);
    }

    #[test]
    fn phrasing_picks_turn_and_altitude_direction() {
        let airport = Airport::default_airport();
        let a = ac();
        assert_eq!(phrase(&Clearance::Heading(200.0), &a, &airport, &[]), "turn left heading 200");
        assert_eq!(phrase(&Clearance::Heading(300.0), &a, &airport, &[]), "turn right heading 300");
        assert_eq!(
            phrase(&Clearance::Altitude(3000.0), &a, &airport, &[]),
            "descend and maintain 3,000"
        );
    }

    #[test]
    fn combined_positions_speak_on_each_others_frequencies() {
        use Position::*;
        let land = Clearance::ClearedToLand(0);
        assert_eq!(issuer_for(&land, Local, Staffing::LOCAL, SOLO), Some(Local));
        assert_eq!(issuer_for(&land, Local, Staffing::APPROACH, SOLO), None);
        let taxi = Clearance::Taxi { to: 0, via: vec![] };
        // Local+Ground: Tower can taxi an aircraft still on Ground, and Ground one on Tower.
        assert!(issuer_for(&taxi, Ground, Staffing::LOCAL_GROUND, SOLO).is_some());
        assert_eq!(issuer_for(&Clearance::CrossRunway(0), Ground, Staffing::LOCAL_GROUND, SOLO), Some(Local));
        // Split: Tower can't clear an aircraft still talking to Ground.
        assert_eq!(issuer_for(&Clearance::CrossRunway(0), Ground, Staffing::LOCAL, SOLO), None);
        assert_eq!(issuer_for(&Clearance::Heading(90.0), Approach, Staffing::ALL, SOLO), Some(Approach));
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
