//! Pilot mistakes: hearback errors, altitude busts, wrong turns on the
//! taxiway, and, very rarely, a runway incursion or a takeoff without a
//! clearance. Only pilots working the player make them; spotting them is
//! the player's job.

use super::ground_map::GroundMap;
use super::units::normalize_heading;
use crate::sim::clearance::{MAX_ASSIGNED_ALTITUDE, MIN_ASSIGNED_ALTITUDE};

/// How often pilots slip up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PilotMistakes {
    #[default]
    Off,
    /// About as often as real pilots: now and then in a long shift.
    Realistic,
    Occasional,
    Frequent,
}

impl PilotMistakes {
    pub const ALL: [PilotMistakes; 4] =
        [PilotMistakes::Off, PilotMistakes::Realistic, PilotMistakes::Occasional, PilotMistakes::Frequent];

    pub fn label(self) -> &'static str {
        match self {
            PilotMistakes::Off => "Off",
            PilotMistakes::Realistic => "Realistic",
            PilotMistakes::Occasional => "Occasional",
            PilotMistakes::Frequent => "Frequent",
        }
    }

    /// What to expect, for the menu.
    pub fn note(self) -> &'static str {
        match self {
            PilotMistakes::Off => "Pilots always get it right",
            PilotMistakes::Realistic => "A wrong readback or a busted altitude now and then",
            PilotMistakes::Occasional => "A few slips a shift to catch: listen to the readbacks",
            PilotMistakes::Frequent => "Watch every readback, Mode C and taxi route",
        }
    }

    /// Chance that a radar instruction is misheard, or an altitude busted.
    pub fn instruction_chance(self) -> f32 {
        match self {
            PilotMistakes::Off => 0.0,
            PilotMistakes::Realistic => 1.0 / 150.0,
            PilotMistakes::Occasional => 1.0 / 30.0,
            PilotMistakes::Frequent => 1.0 / 12.0,
        }
    }

    /// Chance that a taxi clearance ends with a wrong turn.
    pub fn taxi_chance(self) -> f32 {
        match self {
            PilotMistakes::Off => 0.0,
            PilotMistakes::Realistic => 1.0 / 80.0,
            PilotMistakes::Occasional => 1.0 / 15.0,
            PilotMistakes::Frequent => 1.0 / 6.0,
        }
    }

    /// Chance of a major error (an incursion or a takeoff without clearance)
    /// each time one is possible.
    pub fn major_chance(self) -> f32 {
        match self {
            PilotMistakes::Off => 0.0,
            PilotMistakes::Realistic => 1.0 / 4000.0,
            PilotMistakes::Occasional => 1.0 / 600.0,
            PilotMistakes::Frequent => 1.0 / 150.0,
        }
    }

    pub fn step(self, by: i32) -> PilotMistakes {
        step(&Self::ALL, self, by)
    }
}

/// One step along `all` from `value`, stopping at the ends.
pub fn step<T: Copy + PartialEq>(all: &[T], value: T, by: i32) -> T {
    let i = all.iter().position(|&v| v == value).unwrap_or(0) as i32;
    all[(i + by).clamp(0, all.len() as i32 - 1) as usize]
}

/// A misheard instruction is missed once the aircraft is this far past the
/// altitude it was given, feet, or this long after the readback, seconds.
pub const HEARBACK_MISS_FT: f32 = 300.0;
pub const HEARBACK_MISS_S: f32 = 60.0;
/// A bust shows on Mode C once it's this far through the altitude, feet.
pub const BUST_SHOWS_FT: f32 = 200.0;
/// How far through the altitude a bust goes before the pilot catches it, feet.
pub const BUST_FT: (f32, f32) = (400.0, 1000.0);
/// Only a climb or descent of at least this much can end in a bust, feet.
pub const BUST_MIN_CHANGE_FT: f32 = 1500.0;
/// Furthest a wrong turn takes an aircraft along the wrong taxiway, NM.
pub const WRONG_TURN_NM: f32 = 0.25;
/// And the least: long enough, at taxi speed, for Ground to see it and say
/// something before it's over, NM.
pub const MIN_WRONG_TURN_NM: f32 = 0.08;
/// A takeoff without clearance starts this long after line up and wait, s.
pub const EARLY_ROLL_S: (f32, f32) = (20.0, 60.0);
/// The player has this long to react to a major error, seconds.
pub const INCURSION_REACT_S: f32 = 15.0;
pub const EARLY_ROLL_REACT_S: f32 = 10.0;
/// Fastest a takeoff roll can still be stopped, knots.
pub const STOP_MAX_KT: f32 = 80.0;

/// A mistake to make on the next chance, whatever the dice say: for tests
/// and lessons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Force {
    Hearback,
    Bust,
    WrongTurn,
    EarlyRoll,
    Incursion,
}

/// Something a pilot got wrong that the player hasn't sorted out yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Slip {
    pub kind: SlipKind,
    /// When it happened.
    pub at: f32,
    /// Already scored as missed: it stays until corrected, shown in amber.
    pub missed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlipKind {
    /// Read back and flying the wrong altitude.
    Altitude { assigned: f32, heard: f32 },
    /// Read back and flying the wrong heading.
    Heading { assigned: f32, heard: f32 },
    /// Flying through the assigned altitude to `peak` before correcting.
    Bust { assigned: f32, peak: f32 },
    /// Turned off the cleared route onto the wrong taxiway; it stops at the
    /// end of `path` and asks for help.
    /// `turned` once it's past the junction, heading the wrong way.
    WrongTurn { intended: Vec<usize>, junction: usize, stop: usize, turned: bool },
    /// Crossed a hold line without a clearance onto this physical runway;
    /// `entered` once it's on it.
    Incursion { runway: usize, conflict: bool, entered: bool },
    /// Will roll from line up and wait without a takeoff clearance at `due`.
    EarlyRoll { due: f32, rolled: bool, conflict: bool },
}

impl Slip {
    pub fn new(kind: SlipKind, at: f32) -> Slip {
        Slip { kind, at, missed: false }
    }

    /// A misheard altitude or heading, or a bust.
    pub fn is_airborne(&self) -> bool {
        matches!(self.kind, SlipKind::Altitude { .. } | SlipKind::Heading { .. } | SlipKind::Bust { .. })
    }
}

/// The random stream for one aircraft's chance of a mistake: the same
/// every run of the seed, and apart from the traffic's.
pub fn rng(seed: u64, id: u32, salt: u64) -> fastrand::Rng {
    fastrand::Rng::with_seed(seed ^ 0x5011_7000_0000 ^ ((id as u64) << 20) ^ salt.wrapping_mul(0x9E37_79B9))
}

/// What a pilot might hear instead of `altitude`: a thousand feet off.
pub fn misheard_altitude(altitude: f32, roll: u32) -> f32 {
    let up = altitude + 1000.0;
    let down = altitude - 1000.0;
    let up_ok = up <= MAX_ASSIGNED_ALTITUDE;
    let down_ok = down >= MIN_ASSIGNED_ALTITUDE;
    match (up_ok, down_ok) {
        (true, true) if roll.is_multiple_of(2) => up,
        (true, true) | (false, true) => down,
        _ => up,
    }
}

/// What a pilot might hear instead of heading `heading` (whole degrees,
/// 1 to 360): ten or twenty degrees off, or two digits swapped.
pub fn misheard_heading(heading: f32, roll: u32) -> f32 {
    let h = heading.round() as i32;
    let h = if h <= 0 { h + 360 } else { h };
    let digits = [h / 100, (h / 10) % 10, h % 10];
    let swapped = digits[1] * 100 + digits[0] * 10 + digits[2];
    let options = [h + 10, h - 10, h + 20, h - 20, swapped];
    let pick = options[(roll % options.len() as u32) as usize];
    // A swap that isn't a heading, or changes nothing: ten degrees off instead.
    let pick = if pick == h || (pick == swapped && !(1..=360).contains(&pick)) { h + 10 } else { pick };
    normalize_heading(pick as f32)
}

/// Where a bust levels off: `over` feet through `assigned` in the direction
/// of travel from `from`.
pub fn bust_peak(from: f32, assigned: f32, over: f32) -> f32 {
    let peak = if assigned > from { assigned + over } else { assigned - over };
    peak.clamp(MIN_ASSIGNED_ALTITUDE - 1000.0, MAX_ASSIGNED_ALTITUDE + 1000.0)
}

/// A wrong turn on a taxi route: the first junction ahead (from `path[from]`)
/// with a taxiway off the route that isn't a hold line or in a runway zone.
/// Returns the new path, the junction's index in it, and where it stops.
pub fn wrong_turn(map: &GroundMap, came_from: usize, path: &[usize], from: usize) -> Option<(Vec<usize>, usize, usize)> {
    let safe = |n: usize| map.nodes[n].zones.is_empty() && map.nodes[n].hold.is_none() && map.nodes[n].runway.is_none();
    for i in from..path.len().saturating_sub(1) {
        let junction = path[i];
        let prev = if i == 0 { came_from } else { path[i - 1] };
        let next = path[i + 1];
        let wrong = map.adjacent[junction].iter().map(|&e| &map.edges[e]).find(|e| {
            let other = e.other(junction);
            e.taxiway().is_some_and(|n| !n.is_empty()) && other != prev && other != next && safe(other)
        });
        let Some(edge) = wrong else { continue };
        let name = edge.taxiway().expect("checked").to_string();
        let mut new_path = path[..=i].to_vec();
        let (mut at, mut here) = (junction, edge.other(junction));
        let mut length = edge.length;
        new_path.push(here);
        // On along the same taxiway while it's safe and short of the limit.
        while length < WRONG_TURN_NM {
            let on = map.adjacent[here].iter().map(|&e| &map.edges[e]).find(|e| {
                let other = e.other(here);
                e.taxiway() == Some(name.as_str()) && other != at && safe(other) && !new_path.contains(&other)
            });
            let Some(edge) = on else { break };
            length += edge.length;
            at = here;
            here = edge.other(here);
            new_path.push(here);
        }
        if length < MIN_WRONG_TURN_NM {
            continue;
        }
        return Some((new_path, i, here));
    }
    None
}

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::airspace::Airport;
use super::clearance::{Clearance, phrase};
use super::scoring::SimEvent;
use super::{Speaker, World};

/// What came of an instruction the player gave, for the slip: the readback
/// if the pilot heard it wrong, and anything to score.
#[derive(Debug, Default)]
pub(super) struct Heard {
    pub readback: Option<String>,
    pub events: Vec<SimEvent>,
}

fn thousands(feet: f32) -> String {
    super::clearance::thousands(feet)
}

/// An instruction from the player has been accepted: put right any slip it
/// answers, then perhaps make a new one.
pub(super) fn after_instruction(
    ac: &mut Aircraft,
    clearance: &Clearance,
    rate: PilotMistakes,
    seed: u64,
    now: f32,
    airport: &Airport,
) -> Heard {
    let mut heard = Heard::default();
    answer_slip(ac, clearance, &mut heard);
    if (rate == PilotMistakes::Off && ac.force_slip.is_none()) || ac.slip.is_some() || ac.emergency.is_some() {
        return heard;
    }
    let mut rng = rng(seed, ac.id, ac.strip.len() as u64);
    let roll = rng.f32();
    let force = ac.force_slip;
    let forced = |kinds: &[Force]| force.is_some_and(|f| kinds.contains(&f));
    let used = match clearance {
        Clearance::Altitude(_) => forced(&[Force::Hearback, Force::Bust]),
        Clearance::Heading(_) => forced(&[Force::Hearback]),
        Clearance::Taxi { .. } => forced(&[Force::WrongTurn]),
        Clearance::LineUpAndWait(_) => forced(&[Force::EarlyRoll]),
        _ => false,
    };
    if used {
        ac.force_slip = None;
    }
    match clearance {
        Clearance::Altitude(assigned) if used || roll < rate.instruction_chance() => {
            let change = (assigned - ac.altitude).abs();
            let bust = match force.filter(|_| used) {
                Some(f) => f == Force::Bust,
                None => change >= BUST_MIN_CHANGE_FT && rng.bool(),
            };
            if bust {
                let over = BUST_FT.0 + rng.f32() * (BUST_FT.1 - BUST_FT.0);
                let peak = bust_peak(ac.altitude, *assigned, over);
                ac.target_altitude = peak;
                ac.slip = Some(Slip::new(SlipKind::Bust { assigned: *assigned, peak }, now));
            } else {
                let wrong = misheard_altitude(*assigned, rng.u32(..));
                ac.target_altitude = wrong;
                ac.slip = Some(Slip::new(SlipKind::Altitude { assigned: *assigned, heard: wrong }, now));
                heard.readback = Some(phrase(&Clearance::Altitude(wrong), ac, airport, &[]));
            }
        }
        Clearance::Heading(assigned) if !ac.on_ground && (used || roll < rate.instruction_chance()) => {
            let wrong = misheard_heading(airport.to_magnetic(*assigned), rng.u32(..));
            let wrong = airport.to_true(wrong);
            heard.readback = Some(phrase(&Clearance::Heading(wrong), ac, airport, &[]));
            ac.lateral = Lateral::Heading(wrong);
            ac.slip = Some(Slip::new(SlipKind::Heading { assigned: *assigned, heard: wrong }, now));
        }
        Clearance::Taxi { .. } if used || roll < rate.taxi_chance() => {
            let Some(map) = airport.surface.as_ref() else { return heard };
            let Some(taxi) = ac.taxi.as_mut() else { return heard };
            if let Some((path, junction, stop)) = wrong_turn(map, taxi.node, &taxi.path, taxi.index) {
                let intended = std::mem::replace(&mut taxi.path, path);
                let kind = SlipKind::WrongTurn { intended, junction, stop, turned: false };
                ac.slip = Some(Slip::new(kind, now));
            }
        }
        Clearance::LineUpAndWait(_) if !ac.cleared_takeoff && (used || roll < rate.major_chance()) => {
            let due = now + EARLY_ROLL_S.0 + rng.f32() * (EARLY_ROLL_S.1 - EARLY_ROLL_S.0);
            ac.slip = Some(Slip::new(SlipKind::EarlyRoll { due, rolled: false, conflict: false }, now));
        }
        _ => {}
    }
    heard
}

/// Does this instruction put the slip right? Score it if it was caught in time.
fn answer_slip(ac: &mut Aircraft, clearance: &Clearance, heard: &mut Heard) {
    let Some(slip) = ac.slip.clone() else { return };
    let callsign = ac.callsign.clone();
    let caught = |what: String, major: bool| SimEvent::SlipCaught { callsign: callsign.clone(), what, major };
    let lateral = matches!(
        clearance,
        Clearance::Heading(_) | Clearance::DirectFix(_) | Clearance::ResumeRoute | Clearance::Hold(_)
    );
    match (&slip.kind, clearance) {
        (SlipKind::Altitude { assigned, heard: wrong }, Clearance::Altitude(_)) => {
            if !slip.missed {
                let what = format!("readback corrected: {} read back as {}", thousands(*assigned), thousands(*wrong));
                heard.events.push(caught(what, false));
            }
            ac.slip = None;
        }
        (SlipKind::Bust { assigned, peak }, Clearance::Altitude(_)) => {
            let through = (ac.altitude - assigned) * (peak - assigned).signum();
            if through > BUST_SHOWS_FT {
                let what = format!("altitude deviation caught: {:.0} ft through {}", through, thousands(*assigned));
                heard.events.push(caught(what, false));
            }
            ac.slip = None;
        }
        (SlipKind::Heading { assigned, heard: wrong }, _) if lateral => {
            if !slip.missed {
                let what = format!("readback corrected: heading {assigned:03.0} read back as {wrong:03.0}");
                heard.events.push(caught(what, false));
            }
            ac.slip = None;
        }
        (SlipKind::WrongTurn { turned, .. }, Clearance::Taxi { .. } | Clearance::HoldPosition) => {
            let taxi = ac.taxi.as_mut();
            if *turned {
                heard.events.push(caught("wrong turn caught".to_string(), false));
            }
            // Told to hold: it stops at the next node and waits for a new route.
            if let (Clearance::HoldPosition, Some(taxi)) = (clearance, taxi) {
                let end = (taxi.index + 1).min(taxi.path.len());
                taxi.path.truncate(end);
            }
            ac.slip = None;
        }
        (SlipKind::Incursion { conflict, .. }, Clearance::HoldPosition) => {
            if *conflict && !slip.missed {
                heard.events.push(caught("runway incursion handled".to_string(), true));
            }
            ac.slip = None;
        }
        (SlipKind::EarlyRoll { rolled: true, conflict, .. }, Clearance::StopTakeoff) => {
            if *conflict && !slip.missed {
                heard.events.push(caught("takeoff without clearance stopped".to_string(), true));
            }
            ac.slip = None;
        }
        (SlipKind::EarlyRoll { rolled: false, .. }, Clearance::ClearedForTakeoff(_)) => ac.slip = None,
        _ => {}
    }
}

impl World {
    /// The player sent an arrival around: that answers an incursion or a
    /// takeoff without clearance on its runway.
    pub(super) fn go_around_answers_major(&mut self, end: usize) {
        let Some(map) = self.airport.surface.as_ref() else { return };
        let physical = map.runway_of_end[end];
        let mut events = Vec::new();
        for ac in &mut self.aircraft {
            let Some(slip) = &ac.slip else { continue };
            let on_runway = match slip.kind {
                SlipKind::Incursion { runway, conflict: true, .. } => runway == physical,
                SlipKind::EarlyRoll { rolled: true, conflict: true, .. } => {
                    ac.departure_end.is_some_and(|e| map.runway_of_end[e] == physical)
                }
                _ => false,
            };
            if on_runway && !slip.missed {
                let what = match slip.kind {
                    SlipKind::Incursion { .. } => "runway incursion handled",
                    _ => "takeoff without clearance handled",
                };
                events.push(SimEvent::SlipCaught { callsign: ac.callsign.clone(), what: what.into(), major: true });
                ac.slip = None;
            }
        }
        for event in events {
            self.emit(event);
        }
    }

    /// Mistakes play out: misheard values get missed, busts correct, wrong
    /// turns end in a call for help, and the major errors happen.
    pub(super) fn run_slips(&mut self) {
        let now = self.time;
        for index in 0..self.aircraft.len() {
            let Some(slip) = self.aircraft[index].slip.clone() else { continue };
            match slip.kind {
                SlipKind::Altitude { assigned, heard } => self.step_misheard_altitude(index, &slip, assigned, heard),
                SlipKind::Heading { assigned, heard } => {
                    let ac = &mut self.aircraft[index];
                    if ac.lateral != Lateral::Heading(heard) {
                        ac.slip = None;
                    } else if !slip.missed && now - slip.at > HEARBACK_MISS_S {
                        let what = format!("hearback error missed: flying heading {heard:03.0}, assigned {assigned:03.0}");
                        self.miss(index, what, false);
                    }
                }
                SlipKind::Bust { assigned, peak } => self.step_bust(index, assigned, peak),
                SlipKind::WrongTurn { stop, junction, .. } => self.step_wrong_turn(index, stop, junction),
                SlipKind::Incursion { runway, conflict, .. } => {
                    let ac = &self.aircraft[index];
                    // Still on it, or still to cross it.
                    let on_it = ac.on_ground && self.occupies(ac, runway);
                    let ahead = self.airport.surface.as_ref().zip(ac.taxi.as_ref()).is_some_and(|(map, t)| {
                        map.in_zone(t.node, runway) || t.path.iter().skip(t.index).any(|&n| map.in_zone(n, runway))
                    });
                    if conflict && !slip.missed && now - slip.at > INCURSION_REACT_S {
                        self.miss(index, "runway incursion not acted on".to_string(), true);
                    } else if !on_it && !ahead {
                        self.aircraft[index].slip = None;
                    }
                }
                SlipKind::EarlyRoll { due, rolled, conflict } => {
                    let ac = &self.aircraft[index];
                    if !rolled {
                        if !matches!(ac.phase, Phase::Taxiing | Phase::LinedUp) || ac.cleared_takeoff {
                            self.aircraft[index].slip = None;
                        } else if now >= due && ac.phase == Phase::LinedUp {
                            self.roll_without_clearance(index);
                        }
                    } else if !conflict || slip.missed || !ac.on_ground {
                        self.aircraft[index].slip = None;
                    } else if now - slip.at > EARLY_ROLL_REACT_S {
                        self.miss(index, "takeoff without clearance not acted on".to_string(), true);
                    }
                }
            }
        }
    }

    /// Score a missed slip and keep it, marked, until it's put right.
    fn miss(&mut self, index: usize, what: String, major: bool) {
        let ac = &mut self.aircraft[index];
        if let Some(slip) = ac.slip.as_mut() {
            slip.missed = true;
        }
        let callsign = ac.callsign.clone();
        self.emit(SimEvent::SlipMissed { callsign, what, major });
    }

    fn step_misheard_altitude(&mut self, index: usize, slip: &Slip, assigned: f32, heard: f32) {
        let ac = &mut self.aircraft[index];
        // Someone else changed it: it's not a slip any more.
        if ac.target_altitude != heard {
            ac.slip = None;
            return;
        }
        if slip.missed {
            return;
        }
        let past = (ac.altitude - assigned) * (heard - assigned).signum();
        if past > HEARBACK_MISS_FT || self.time - slip.at > HEARBACK_MISS_S {
            let doing = if heard > assigned { "climbing to" } else { "descending to" };
            let what = format!("hearback error missed: {doing} {}, assigned {}", thousands(heard), thousands(assigned));
            self.miss(index, what, false);
        }
    }

    fn step_bust(&mut self, index: usize, assigned: f32, peak: f32) {
        let ac = &mut self.aircraft[index];
        if ac.target_altitude != peak {
            ac.slip = None;
            return;
        }
        if (ac.altitude - peak).abs() > 30.0 {
            return;
        }
        // The pilot sees it and goes back down, or up.
        ac.target_altitude = assigned;
        ac.slip = None;
        let callsign = ac.callsign.clone();
        let over = (peak - assigned).abs();
        let back = if peak > assigned { "descending" } else { "climbing" };
        if self.config.staffing.works(ac.frequency()) {
            self.pilot_say(Speaker::Pilot, callsign.clone(), format!("{callsign}, sorry, {back} back to {}.", thousands(assigned)));
        }
        let what = format!("altitude bust missed: {over:.0} ft through {}", thousands(assigned));
        self.emit(SimEvent::SlipMissed { callsign, what, major: false });
    }

    fn step_wrong_turn(&mut self, index: usize, stop: usize, junction: usize) {
        let ac = &mut self.aircraft[index];
        let Some(taxi) = ac.taxi.as_ref() else {
            ac.slip = None;
            return;
        };
        if taxi.index > junction
            && let Some(Slip { kind: SlipKind::WrongTurn { turned, .. }, .. }) = ac.slip.as_mut()
        {
            *turned = true;
        }
        // Given a new route some other way: forget it.
        if taxi.path.last() != Some(&stop) || ac.phase != Phase::Taxiing {
            ac.slip = None;
            return;
        }
        let Some(map) = self.airport.surface.as_ref() else { return };
        let there = taxi.done() && ac.pos.distance(map.nodes[stop].pos) < super::clearance::AT_NODE_NM;
        if !there {
            return;
        }
        ac.slip = None;
        let callsign = ac.callsign.clone();
        let to = match ac.frequency() {
            super::positions::Position::Local => "Tower",
            _ => "Ground",
        };
        self.pilot_say(Speaker::Pilot, callsign.clone(), format!("{to}, {callsign}, I think we missed our turn, request progressive taxi."));
        self.emit(SimEvent::SlipMissed { callsign, what: "wrong turn missed".to_string(), major: false });
    }

    /// The pilot rolls from line up and wait without a takeoff clearance.
    fn roll_without_clearance(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let (Some(end), Some(map)) = (ac.departure_end, self.airport.surface.as_ref()) else { return };
        let physical = map.runway_of_end[end];
        let id = ac.id;
        let close = self.arrivals_within(physical, super::tower::INCURSION_FINAL_NM, super::tower::VFR_INCURSION_NM);
        let conflict = self.runway_occupied(physical, id)
            || !close.is_empty()
            || self.time < self.next_departure_time
            || self.wake_wait(ac).is_some();
        let callsign = ac.callsign.clone();
        let now = self.time;
        let ac = &mut self.aircraft[index];
        super::clearance::start_roll(ac, &self.airport);
        if let Some(slip) = ac.slip.as_mut() {
            slip.kind = SlipKind::EarlyRoll { due: now, rolled: true, conflict };
            slip.at = now;
        }
        let interval = super::tower::departure_interval(&self.aircraft[index]);
        self.next_departure_time = self.next_departure_time.max(now + interval);
        self.record_takeoff(index);
        if self.config.staffing.works(self.aircraft[index].frequency()) {
            self.pilot_say(Speaker::Pilot, callsign.clone(), format!("{callsign}, rolling."));
        }
        let what = "took off without a takeoff clearance".to_string();
        self.emit(SimEvent::PilotDeviation { callsign, what });
    }

    /// Whether a taxiing aircraft the player works crosses the hold line it
    /// should stop at: a major error, decided once per hold line.
    pub(super) fn deviates_at_hold_line(&self, ac: &Aircraft, node: usize) -> bool {
        if ac.force_slip == Some(Force::Incursion) {
            return true;
        }
        let rate = self.config.mistakes;
        if rate == PilotMistakes::Off || ac.slip.is_some() || ac.emergency.is_some() || !self.is_players(ac) {
            return false;
        }
        rng(self.config.seed, ac.id, 1_000_000 + node as u64).f32() < rate.major_chance()
    }
}

/// A departure stopped where a wrong turn left it, waiting for a new route.
pub fn lost(ac: &Aircraft) -> bool {
    ac.flow == Flow::Departure && ac.phase == Phase::Taxiing && ac.taxi.as_ref().is_some_and(|t| t.done())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::airspace::{Airport, KSUS_AIRPORT, KSUS_GROUND};

    #[test]
    fn a_misheard_heading_is_always_another_real_heading() {
        for h in 1..=360 {
            for roll in 0..10 {
                let wrong = misheard_heading(h as f32, roll);
                assert!((0.0..360.0).contains(&wrong), "{h} {roll} {wrong}");
                assert_ne!(normalize_heading(h as f32), wrong, "{h} {roll}");
            }
        }
        // Digits swapped: 240 heard as 420 isn't a heading, 305 as 035 is.
        assert_eq!(misheard_heading(305.0, 4), 35.0);
        assert_eq!(misheard_heading(240.0, 4), 250.0);
    }

    #[test]
    fn a_misheard_altitude_is_a_thousand_feet_off_and_assignable() {
        for thousand in 1..=17 {
            let a = thousand as f32 * 1000.0;
            for roll in 0..4 {
                let wrong = misheard_altitude(a, roll);
                assert_eq!((wrong - a).abs(), 1000.0);
                assert!((MIN_ASSIGNED_ALTITUDE..=MAX_ASSIGNED_ALTITUDE).contains(&wrong), "{a} {wrong}");
            }
        }
    }

    #[test]
    fn a_bust_goes_through_the_altitude_the_way_it_was_heading() {
        assert_eq!(bust_peak(10000.0, 7000.0, 500.0), 6500.0);
        assert_eq!(bust_peak(3000.0, 7000.0, 500.0), 7500.0);
    }

    #[test]
    fn wrong_turns_never_lead_onto_a_runway_or_a_hold_line() {
        let airports = [Airport::default_airport(), Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()];
        let mut found = 0;
        for airport in &airports {
            let map = airport.surface.as_ref().unwrap();
            for spot in &map.parking {
                for end in 0..airport.runways.len() {
                    let Some(hold) = map.departure_hold(end) else { continue };
                    let Some(route) = map.route(spot.node, hold, &[]) else { continue };
                    let Some((path, junction, stop)) = wrong_turn(map, spot.node, &route.nodes, 0) else { continue };
                    found += 1;
                    assert_eq!(path[..=junction], route.nodes[..=junction]);
                    assert_eq!(path.last(), Some(&stop));
                    for &n in &path[junction + 1..] {
                        let node = &map.nodes[n];
                        assert!(node.zones.is_empty() && node.hold.is_none() && node.runway.is_none());
                    }
                    let length: f32 = path[junction..].windows(2).map(|w| map.nodes[w[0]].pos.distance(map.nodes[w[1]].pos)).sum();
                    assert!(length < WRONG_TURN_NM + 0.3, "{length}");
                }
            }
        }
        assert!(found > 20, "only {found} routes had somewhere to go wrong");
    }
}
