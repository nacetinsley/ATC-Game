//! Coordination between controllers over the landline.
//!
//! Positions don't only talk to pilots: Ground asks Local before crossing a
//! runway, Local tells Ground about an arrival it has left holding short
//! between runways, Local asks Approach to release a departure when
//! Approach has put departures on call for release, and Local tells Approach
//! about every IFR go-around. The landline only
//! matters between a position the player works and one that's automated. On
//! a combined position the player is both ends, and two automated positions
//! agree silently, as they always have.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::clearance::spoken_runway;
use super::positions::Position;
use super::replay::Action;
use super::scoring::SimEvent;
use super::units::{format_heading, heading_diff, vec_heading};
use super::{Speaker, World};

/// An automated controller answers the landline after this long, seconds.
const LANDLINE_DELAY: (f32, f32) = (3.0, 6.0);
/// An approval not acted on lapses after this long, seconds.
pub const APPROVAL_S: f32 = 90.0;
/// A request to the player unanswered this long costs points, seconds.
pub const ANSWER_S: f32 = 45.0;
/// Turned down by the player, an automated controller asks again after this long, seconds.
const REASK_S: f32 = 60.0;
/// Local withdraws an unused crossing approval once an arrival is this close, NM.
const CANCEL_CROSSING_NM: f32 = 3.0;
/// A release is void if the departure isn't off within this long, seconds.
pub const RELEASE_VOID_S: f32 = 120.0;
/// Once Approach puts departures on call for release, it keeps them there at
/// least this long after the reason has gone, seconds.
pub const CALL_FOR_RELEASE_MIN_S: f32 = 300.0;
/// Tower has this long after a go-around to tell Approach, seconds.
pub const GO_AROUND_CALL_S: f32 = 60.0;
/// Severe weather this far along a departure's first leg holds it, NM.
const RELEASE_STORM_NM: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordKind {
    /// Ground asking Local to let an aircraft across a physical runway.
    Crossing { runway: usize },
    /// Local asking Approach to release an IFR departure off this runway end.
    Release { end: usize },
    /// Local telling Approach about an IFR go-around off this runway end.
    GoAround { end: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoordState {
    /// Waiting for an answer: an automated controller gives it at `due`.
    Pending,
    /// Approved, until this sim time unless used.
    Approved { until: f32 },
    /// Turned down. With `callback`, the other side calls back when it can.
    Unable { callback: bool },
    /// The crossing clearance has gone out: the runway is held for it until
    /// the aircraft is across.
    InUse,
    /// Not yet made: the player, on Local, owes Approach a call about this
    /// go-around.
    Owed,
}

#[derive(Debug, Clone)]
pub struct Coordination {
    pub kind: CoordKind,
    /// Who asked, and who is asked.
    pub from: Position,
    pub to: Position,
    pub aircraft: u32,
    pub asked: f32,
    /// When an automated controller answers.
    pub due: f32,
    /// Already scored as answered late.
    pub late: bool,
    pub state: CoordState,
}

impl Coordination {
    /// Approved and not yet over: the runway is promised to this aircraft.
    pub fn live(&self) -> bool {
        matches!(self.state, CoordState::Approved { .. } | CoordState::InUse)
    }
}

/// Whether IFR departures go when Tower is ready, or only once Approach has
/// released each one.
#[derive(Debug, Clone, Default)]
pub struct Releases {
    /// Departures are on call for release, and why. `None`: released automatically.
    pub call_for_release: Option<String>,
    /// When the automated Approach last saw a reason to hold departures.
    pub reason_seen: f32,
}

impl World {
    /// Open coordination on an aircraft.
    pub fn coordination_for(&self, id: u32) -> Option<&Coordination> {
        self.coordination.iter().find(|c| c.aircraft == id)
    }

    /// Requests the player has to answer: asked of a position they work, by
    /// one they don't, and not yet approved.
    pub fn incoming(&self) -> impl Iterator<Item = &Coordination> {
        let staffing = self.config.staffing;
        self.coordination.iter().filter(move |c| {
            staffing.works(c.to)
                && !staffing.works(c.from)
                && matches!(c.state, CoordState::Pending | CoordState::Unable { .. })
        })
    }

    /// Local has agreed to this aircraft crossing this runway.
    pub fn crossing_approved(&self, id: u32, runway: usize) -> bool {
        self.coordination
            .iter()
            .any(|c| c.aircraft == id && c.kind == CoordKind::Crossing { runway } && c.live())
    }

    /// A crossing has been promised on this runway: Local keeps it clear.
    pub(super) fn crossing_reserved(&self, runway: usize) -> bool {
        self.coordination.iter().any(|c| c.kind == CoordKind::Crossing { runway } && c.live())
    }

    /// Does the player, working Ground but not Local, need Local's approval
    /// before crossing this aircraft over this runway?
    pub fn needs_crossing_approval(&self, id: u32, runway: usize) -> bool {
        let staffing = self.config.staffing;
        staffing.ground
            && !staffing.local
            && self.find(id).is_some_and(|a| a.frequency() == Position::Ground)
            && !self.crossing_approved(id, runway)
    }

    /// Why Local won't let anyone across this runway right now, if it won't.
    pub(super) fn crossing_blocker(&self, runway: usize, id: u32) -> Option<String> {
        if let Some(d) = self
            .arrivals_within(runway, super::tower::CROSSING_HOLD_NM, 0.0)
            .iter()
            .filter_map(|&i| self.landing_distance(&self.aircraft[i], runway))
            .reduce(f32::min)
        {
            return Some(format!("traffic {:.0} mile final", d.max(1.0)));
        }
        if self.landing_within(runway, super::tower::VFR_CROSSING_S, false) {
            return Some("traffic landing".into());
        }
        if self.departure_under_way(runway) {
            return Some("departure rolling".into());
        }
        if self.runway_occupied(runway, id) {
            return Some("runway occupied".into());
        }
        None
    }

    /// Releases are coordinated only when the player works exactly one of
    /// Local and Approach.
    fn releases_coordinated(&self) -> bool {
        let staffing = self.config.staffing;
        staffing.local != staffing.approach
    }

    /// Departures are on call for release.
    pub fn call_for_release(&self) -> bool {
        self.releases.call_for_release.is_some()
    }

    /// Approach has released this departure, and the release isn't void.
    pub fn release_approved(&self, id: u32) -> bool {
        self.coordination
            .iter()
            .any(|c| c.aircraft == id && matches!(c.kind, CoordKind::Release { .. }) && c.live())
    }

    /// An IFR departure that needs Approach's release before it may take off.
    pub fn needs_release(&self, id: u32) -> bool {
        self.releases_coordinated()
            && self.call_for_release()
            && self.find(id).is_some_and(|a| {
                a.flow == Flow::Departure && a.vfr.is_none() && a.on_ground && a.phase != Phase::TowerRelease
            })
            && !self.release_approved(id)
    }

    /// Why Approach would hold departures off this runway end, flying out on
    /// `bearing`: a missed approach to sort out, a runway change, an
    /// emergency about to land, or weather on the way out.
    fn release_reason(&self, end: usize, bearing: f32) -> Option<String> {
        if self.aircraft.iter().any(|a| a.phase == Phase::GoAround && a.vfr.is_none()) {
            return Some("missed approach in progress".into());
        }
        if self.landing_against_flow() {
            return Some("runway change".into());
        }
        let physical = self.surface().runway_of_end[end];
        if self.emergency_inbound(physical) {
            return Some("emergency inbound".into());
        }
        if self.storms.blocked(self.airport.runways[end].far_end(), bearing, RELEASE_STORM_NM) {
            return Some("weather".into());
        }
        None
    }

    /// Why Approach won't release this departure yet, if it won't: as for
    /// the field, but with weather checked along its own first leg.
    fn release_blocker(&self, ac: &Aircraft) -> Option<String> {
        let end = ac.departure_end?;
        let runway = &self.airport.runways[end];
        let bearing = ac
            .route
            .get(ac.route_index)
            .map_or(runway.heading, |&f| vec_heading(self.airport.fixes[f].pos() - runway.far_end()));
        self.release_reason(end, bearing)
    }

    /// Tower's call to Approach about a go-around.
    fn go_around_text(&self, end: usize, id: u32) -> String {
        let callsign = self.callsign(id);
        let feet = self.airport.go_around_altitude as i32;
        let altitude = if feet >= 1000 { format!("{},{:03}", feet / 1000, feet % 1000) } else { feet.to_string() };
        let runway = &self.airport.runways[end];
        // A heading Tower gave it, or straight out.
        let heading = match self.find(id).map(|a| a.lateral) {
            Some(Lateral::Heading(h)) if heading_diff(runway.heading, h).abs() > 1.0 => {
                format!("heading {}", format_heading(self.airport.to_magnetic(h)))
            }
            _ => "runway heading".to_string(),
        };
        format!("go-around {callsign}, runway {}, {heading}, {altitude}.", runway.id)
    }

    /// The player, on Local, still has to tell Approach about this go-around.
    pub fn owes_go_around_call(&self, id: u32) -> bool {
        self.coordination
            .iter()
            .any(|c| c.aircraft == id && matches!(c.kind, CoordKind::GoAround { .. }) && c.state == CoordState::Owed)
    }

    /// An IFR aircraft has gone around off `end`. With the player on Local
    /// and Approach automated, the player owes Approach a call (made at once
    /// when the player `ordered` it). With the player on Approach and Local
    /// automated, Tower calls the player.
    pub(super) fn go_around_started(&mut self, id: u32, end: usize, ordered: bool) {
        if self.find(id).is_none_or(|a| a.vfr.is_some()) {
            return;
        }
        let staffing = self.config.staffing;
        let kind = CoordKind::GoAround { end };
        if staffing.local && !staffing.approach {
            if ordered {
                self.ask(kind, id, Position::Local, Position::Approach);
            } else {
                let now = self.time;
                self.coordination.retain(|c| !(c.aircraft == id && c.kind == kind));
                self.coordination.push(Coordination {
                    kind,
                    from: Position::Local,
                    to: Position::Approach,
                    aircraft: id,
                    asked: now,
                    due: now,
                    late: false,
                    state: CoordState::Owed,
                });
            }
        } else if staffing.approach && !staffing.local && !ordered {
            self.ask(kind, id, Position::Local, Position::Approach);
        }
    }

    /// The player, on Local, tells Approach about a go-around.
    pub fn call_go_around(&mut self, id: u32) -> bool {
        let Some(kind) =
            self.coordination.iter().find(|c| c.aircraft == id && c.state == CoordState::Owed).map(|c| c.kind)
        else {
            return false;
        };
        self.ask(kind, id, Position::Local, Position::Approach);
        self.record(id, Action::CallGoAround);
        true
    }

    fn landline(&mut self, from: Position, to: Position, text: String) {
        self.say(Speaker::Landline, format!("{} to {}: {text}", from.name(), to.name()));
    }

    fn runway_name(&self, runway: usize) -> String {
        spoken_runway(&self.airport, self.surface(), runway, &self.ends_in_use())
    }

    fn callsign(&self, id: u32) -> String {
        self.find(id).map(|a| a.callsign.clone()).unwrap_or_default()
    }

    /// The asked controller agreeing, as said on the landline.
    fn approval_text(&self, kind: CoordKind, callsign: &str, until: f32, automated: bool) -> String {
        match kind {
            CoordKind::Crossing { runway } => format!("{callsign} cross runway {}, approved.", self.runway_name(runway)),
            CoordKind::Release { .. } if automated => {
                format!("{callsign} released, void if not off by {}.", self.utc(until))
            }
            CoordKind::Release { .. } => format!("{callsign} released."),
            CoordKind::GoAround { .. } => format!("{callsign}, approved."),
        }
    }

    /// The asked controller saying no, with why and a promise to call back
    /// if there's a reason.
    fn unable_text(&self, kind: CoordKind, callsign: &str, why: Option<&str>) -> String {
        let what = match kind {
            CoordKind::Crossing { runway } => format!("{callsign} hold short runway {}", self.runway_name(runway)),
            CoordKind::Release { .. } => format!("{callsign} hold for release"),
            CoordKind::GoAround { .. } => callsign.to_string(),
        };
        match why {
            Some(why) => format!("{what}, {why}. I'll call you."),
            None => format!("{what}, unable."),
        }
    }

    /// Open a request. An automated controller answers after a short delay.
    pub(super) fn ask(&mut self, kind: CoordKind, id: u32, from: Position, to: Position) {
        let delay = LANDLINE_DELAY.0 + self.rng.f32() * (LANDLINE_DELAY.1 - LANDLINE_DELAY.0);
        let now = self.time;
        self.coordination.retain(|c| !(c.aircraft == id && c.kind == kind));
        self.coordination.push(Coordination {
            kind,
            from,
            to,
            aircraft: id,
            asked: now,
            due: now + delay,
            late: false,
            state: CoordState::Pending,
        });
        let callsign = self.callsign(id);
        let text = match kind {
            CoordKind::Crossing { runway } => {
                format!("request crossing runway {} for {callsign}.", self.runway_name(runway))
            }
            CoordKind::Release { end } => {
                format!("request release {callsign}, runway {}.", self.airport.runways[end].id)
            }
            CoordKind::GoAround { end } => self.go_around_text(end, id),
        };
        self.landline(from, to, text);
        if self.config.staffing.works(to) {
            self.emit(SimEvent::CoordinationRequest { callsign });
        }
    }

    /// A request of this kind is already waiting on the other controller.
    fn already_asked(&self, id: u32, kind: CoordKind) -> bool {
        self.coordination.iter().any(|c| {
            c.aircraft == id
                && c.kind == kind
                && matches!(c.state, CoordState::Pending | CoordState::Unable { callback: true })
        })
    }

    /// The player, on Ground, asks Local to let this aircraft cross. Immediate:
    /// it's a landline, not the radio.
    pub fn request_crossing(&mut self, id: u32, runway: usize) -> bool {
        let kind = CoordKind::Crossing { runway };
        if !self.needs_crossing_approval(id, runway) || self.already_asked(id, kind) {
            return false;
        }
        self.ask(kind, id, Position::Ground, Position::Local);
        self.record(id, Action::RequestCrossing(runway));
        true
    }

    /// The player, on Local, asks Approach to release this departure.
    pub fn request_release(&mut self, id: u32) -> bool {
        let Some(end) = self.find(id).and_then(|a| a.departure_end) else { return false };
        let kind = CoordKind::Release { end };
        if !self.config.staffing.local || !self.needs_release(id) || self.already_asked(id, kind) {
            return false;
        }
        self.ask(kind, id, Position::Local, Position::Approach);
        self.record(id, Action::RequestRelease);
        true
    }

    /// The player, on Approach with Tower automated, stops departures (each
    /// then needs a release) or lets them go again.
    pub fn set_call_for_release(&mut self, on: bool) -> bool {
        let staffing = self.config.staffing;
        if !staffing.approach || staffing.local || on == self.call_for_release() {
            return false;
        }
        if on {
            self.start_call_for_release(None);
        } else {
            self.end_call_for_release();
        }
        self.record(0, Action::CallForRelease { on });
        true
    }

    fn start_call_for_release(&mut self, why: Option<String>) {
        let runway = self.airport.runways[self.departure_runway()].id.clone();
        let reason = why.as_deref().map(|w| format!(", {w}")).unwrap_or_default();
        self.releases.call_for_release = Some(why.unwrap_or_default());
        self.releases.reason_seen = self.time;
        self.landline(
            Position::Approach,
            Position::Local,
            format!("departures off {runway} call for release{reason}."),
        );
    }

    fn end_call_for_release(&mut self) {
        self.releases.call_for_release = None;
        self.coordination.retain(|c| !matches!(c.kind, CoordKind::Release { .. }));
        self.landline(Position::Approach, Position::Local, "departures released, resume normal.".into());
    }

    /// The automated Approach, with the player on Local, puts departures on
    /// call for release while it has a reason, and lets them go a while after.
    fn run_releases(&mut self) {
        let staffing = self.config.staffing;
        if staffing.approach || !staffing.local {
            return;
        }
        let end = self.departure_runway();
        let reason = self.release_reason(end, self.airport.runways[end].heading);
        let now = self.time;
        match (reason, self.call_for_release()) {
            (Some(why), false) => self.start_call_for_release(Some(why)),
            (Some(_), true) => self.releases.reason_seen = now,
            (None, true) if now - self.releases.reason_seen >= CALL_FOR_RELEASE_MIN_S => {
                self.end_call_for_release()
            }
            _ => {}
        }
    }

    /// The player answers a request about this aircraft.
    pub fn answer_coordination(&mut self, id: u32, approve: bool) -> bool {
        let staffing = self.config.staffing;
        let Some(index) = self.coordination.iter().position(|c| {
            c.aircraft == id
                && staffing.works(c.to)
                && !staffing.works(c.from)
                && matches!(c.state, CoordState::Pending | CoordState::Unable { .. })
        }) else {
            return false;
        };
        let was = self.coordination[index].state;
        let go_around = matches!(self.coordination[index].kind, CoordKind::GoAround { .. });
        // A go-around is only acknowledged: there's nothing to refuse.
        if !approve && (was != CoordState::Pending || go_around) {
            return false;
        }
        let now = self.time;
        let c = &mut self.coordination[index];
        let lasts = if matches!(c.kind, CoordKind::Release { .. }) { RELEASE_VOID_S } else { APPROVAL_S };
        c.state = if approve {
            CoordState::Approved { until: now + lasts }
        } else {
            CoordState::Unable { callback: false }
        };
        let (kind, from, to) = (c.kind, c.from, c.to);
        if !approve {
            // Asked again later, from now.
            c.asked = now;
        }
        let callsign = self.callsign(id);
        let text = if approve {
            self.approval_text(kind, &callsign, now + lasts, false)
        } else {
            self.unable_text(kind, &callsign, None)
        };
        self.landline(to, from, text);
        self.record(id, Action::Coordinate { approve });
        if go_around {
            self.coordination.remove(index);
        }
        true
    }

    /// Automated answers and callbacks, lapsed approvals, late answers.
    pub(super) fn run_coordination(&mut self) {
        self.run_releases();
        if self.coordination.is_empty() {
            return;
        }
        let staffing = self.config.staffing;
        let now = self.time;
        let mut index = 0;
        while index < self.coordination.len() {
            let c = self.coordination[index].clone();
            let Some(ac) = self.find(c.aircraft) else {
                self.coordination.remove(index);
                continue;
            };
            let callsign = ac.callsign.clone();
            let (done, cleared, blocker) = match c.kind {
                CoordKind::Crossing { runway } => {
                    let map = self.surface();
                    let taxi = ac.taxi.as_ref();
                    let ahead = ac.on_ground
                        && taxi.is_some_and(|t| {
                            t.path[t.index.min(t.path.len())..].iter().any(|&n| map.in_zone(n, runway))
                        });
                    // Across, or no longer going that way: nothing left to coordinate.
                    let done = !ahead && !self.occupies(ac, runway);
                    let cleared = taxi.is_some_and(|t| t.cleared.contains(&runway));
                    (done, cleared, self.crossing_blocker(runway, c.aircraft))
                }
                CoordKind::Release { .. } => {
                    // Rolling or gone, or departures no longer need releasing.
                    let done = !ac.on_ground || ac.phase == Phase::TowerRelease || !self.call_for_release();
                    (done, false, self.release_blocker(ac))
                }
                CoordKind::GoAround { .. } => {
                    let done = match c.state {
                        // Never made in time.
                        CoordState::Owed if now - c.asked >= GO_AROUND_CALL_S => {
                            self.emit(SimEvent::UncoordinatedGoAround { callsign: callsign.clone() });
                            true
                        }
                        CoordState::Owed => ac.on_ground,
                        // Made: over once the aircraft is talking to Approach.
                        _ if staffing.works(c.from) => ac.on_ground || ac.frequency() != Position::Local,
                        _ => ac.on_ground,
                    };
                    (done, false, None)
                }
            };
            if done {
                self.coordination.remove(index);
                continue;
            }
            let automated_to = !staffing.works(c.to);
            match c.state {
                CoordState::Approved { .. } | CoordState::Unable { .. } | CoordState::Pending if cleared => {
                    self.coordination[index].state = CoordState::InUse;
                }
                CoordState::Pending if automated_to && now >= c.due => {
                    let text = match blocker {
                        None => {
                            let until = now + self.approval_lasts(c.kind);
                            self.coordination[index].state = CoordState::Approved { until };
                            self.approval_text(c.kind, &callsign, until, true)
                        }
                        Some(why) => {
                            self.coordination[index].state = CoordState::Unable { callback: true };
                            self.unable_text(c.kind, &callsign, Some(&why))
                        }
                    };
                    self.landline(c.to, c.from, text);
                }
                CoordState::Unable { callback: true } if automated_to => {
                    if blocker.is_none() {
                        let until = now + self.approval_lasts(c.kind);
                        self.coordination[index].state = CoordState::Approved { until };
                        let text = self.approval_text(c.kind, &callsign, until, true);
                        self.landline(c.to, c.from, text);
                    }
                }
                // The player turned it down: ask again in a while.
                CoordState::Unable { callback: false } if !staffing.works(c.from) && now - c.asked >= REASK_S => {
                    let text = match c.kind {
                        CoordKind::Crossing { runway } => {
                            format!("{callsign} still holding short runway {}.", self.runway_name(runway))
                        }
                        CoordKind::Release { .. } => format!("{callsign} still waiting for release."),
                        CoordKind::GoAround { end } => self.go_around_text(end, c.aircraft),
                    };
                    let item = &mut self.coordination[index];
                    item.state = CoordState::Pending;
                    item.asked = now;
                    item.late = false;
                    self.landline(c.from, c.to, text);
                    self.emit(SimEvent::CoordinationRequest { callsign });
                }
                CoordState::Approved { until } if automated_to => {
                    if let Some((text, callback)) = self.withdraw(c.kind, &callsign, until, blocker.as_deref()) {
                        self.landline(c.to, c.from, text);
                        if !callback {
                            // Lapsed: it has to be asked for again.
                            self.coordination.remove(index);
                            continue;
                        }
                        self.coordination[index].state = CoordState::Unable { callback: true };
                    }
                }
                CoordState::Approved { until } if now >= until => {
                    self.coordination.remove(index);
                    continue;
                }
                CoordState::Pending if !automated_to && !c.late && now - c.asked >= ANSWER_S => {
                    self.coordination[index].late = true;
                    self.emit(SimEvent::CoordinationLate { callsign });
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn approval_lasts(&self, kind: CoordKind) -> f32 {
        match kind {
            CoordKind::Crossing { .. } => APPROVAL_S,
            CoordKind::Release { .. } => RELEASE_VOID_S,
            // Approach has it in hand until the aircraft reaches it.
            CoordKind::GoAround { .. } => f32::MAX,
        }
    }

    /// An automated controller taking back an approval it gave, if it should:
    /// it has lapsed, or (with traffic now in the way) it's cancelled until
    /// it can call back. The landline line, and whether it will call back.
    fn withdraw(&self, kind: CoordKind, callsign: &str, until: f32, blocker: Option<&str>) -> Option<(String, bool)> {
        let lapsed = self.time >= until;
        match kind {
            CoordKind::Crossing { runway } => {
                let close = !self.arrivals_within(runway, CANCEL_CROSSING_NM, 0.0).is_empty();
                let why = if close { ", traffic short final. I'll call you" } else { "" };
                (close || lapsed).then(|| {
                    (format!("cancel crossing runway {} for {callsign}{why}.", self.runway_name(runway)), close)
                })
            }
            CoordKind::Release { .. } if lapsed => Some((format!("{callsign} release void."), false)),
            CoordKind::Release { .. } => {
                blocker.map(|why| (format!("cancel release {callsign}, {why}. I'll call you."), true))
            }
            CoordKind::GoAround { .. } => None,
        }
    }

    /// Automated Local leaves an arrival holding short between runways and
    /// hands it to the player on Ground: it tells Ground, and calls back
    /// with the crossing when it can.
    pub(super) fn hand_over_holding_short(&mut self, id: u32, runway: usize) {
        let Some(ac) = self.find(id) else { return };
        let callsign = ac.callsign.clone();
        let off = ac.approach_runway.map(|e| format!(" off {}", self.airport.runways[e].id)).unwrap_or_default();
        let name = self.runway_name(runway);
        let now = self.time;
        self.coordination.retain(|c| c.aircraft != id);
        self.coordination.push(Coordination {
            kind: CoordKind::Crossing { runway },
            from: Position::Ground,
            to: Position::Local,
            aircraft: id,
            asked: now,
            due: now,
            late: false,
            state: CoordState::Unable { callback: true },
        });
        self.landline(
            Position::Local,
            Position::Ground,
            format!("{callsign}{off}, holding short runway {name}. I'll call you for the crossing."),
        );
    }
}
