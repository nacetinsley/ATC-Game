//! Coordination between controllers over the landline.
//!
//! Positions don't only talk to pilots: Ground asks Local before crossing a
//! runway, Local tells Ground about an arrival it has left holding short
//! between runways. The landline only matters between a position the player
//! works and one that's automated. On a combined position the player is both
//! ends, and two automated positions agree silently, as they always have.

use super::clearance::spoken_runway;
use super::positions::Position;
use super::replay::Action;
use super::scoring::SimEvent;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordKind {
    /// Ground asking Local to let an aircraft across a physical runway.
    Crossing { runway: usize },
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

    fn landline(&mut self, from: Position, to: Position, text: String) {
        self.say(Speaker::Landline, format!("{} to {}: {text}", from.name(), to.name()));
    }

    fn runway_name(&self, runway: usize) -> String {
        spoken_runway(&self.airport, self.surface(), runway, &self.ends_in_use())
    }

    fn callsign(&self, id: u32) -> String {
        self.find(id).map(|a| a.callsign.clone()).unwrap_or_default()
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
        };
        self.landline(from, to, text);
        if self.config.staffing.works(to) {
            self.emit(SimEvent::CoordinationRequest { callsign });
        }
    }

    /// The player, on Ground, asks Local to let this aircraft cross. Immediate:
    /// it's a landline, not the radio.
    pub fn request_crossing(&mut self, id: u32, runway: usize) -> bool {
        if !self.needs_crossing_approval(id, runway) {
            return false;
        }
        let asked = self.coordination.iter().any(|c| {
            c.aircraft == id
                && c.kind == CoordKind::Crossing { runway }
                && matches!(c.state, CoordState::Pending | CoordState::Unable { callback: true })
        });
        if asked {
            return false;
        }
        self.ask(CoordKind::Crossing { runway }, id, Position::Ground, Position::Local);
        self.record(id, Action::RequestCrossing(runway));
        true
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
        if !approve && was != CoordState::Pending {
            return false;
        }
        let now = self.time;
        let c = &mut self.coordination[index];
        c.state = if approve {
            CoordState::Approved { until: now + APPROVAL_S }
        } else {
            CoordState::Unable { callback: false }
        };
        let (kind, from, to) = (c.kind, c.from, c.to);
        if !approve {
            // Asked again later, from now.
            c.asked = now;
        }
        let callsign = self.callsign(id);
        let text = match kind {
            CoordKind::Crossing { runway } => {
                let name = self.runway_name(runway);
                if approve {
                    format!("{callsign} cross runway {name}, approved.")
                } else {
                    format!("{callsign} hold short runway {name}, unable.")
                }
            }
        };
        self.landline(to, from, text);
        self.record(id, Action::Coordinate { approve });
        true
    }

    /// Automated answers and callbacks, lapsed approvals, late answers.
    pub(super) fn run_coordination(&mut self) {
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
            let CoordKind::Crossing { runway } = c.kind;
            let map = self.surface();
            let taxi = ac.taxi.as_ref();
            let ahead = ac.on_ground
                && taxi.is_some_and(|t| t.path[t.index.min(t.path.len())..].iter().any(|&n| map.in_zone(n, runway)));
            let on_it = self.occupies(ac, runway);
            let cleared = taxi.is_some_and(|t| t.cleared.contains(&runway));
            let callsign = ac.callsign.clone();
            // Across, or no longer going that way: nothing left to coordinate.
            if !ahead && !on_it {
                self.coordination.remove(index);
                continue;
            }
            let automated_to = !staffing.works(c.to);
            match c.state {
                CoordState::Approved { .. } | CoordState::Unable { .. } | CoordState::Pending if cleared => {
                    self.coordination[index].state = CoordState::InUse;
                }
                CoordState::Pending if automated_to && now >= c.due => {
                    let name = self.runway_name(runway);
                    match self.crossing_blocker(runway, c.aircraft) {
                        None => {
                            self.coordination[index].state = CoordState::Approved { until: now + APPROVAL_S };
                            self.landline(c.to, c.from, format!("{callsign} cross runway {name}, approved."));
                        }
                        Some(why) => {
                            self.coordination[index].state = CoordState::Unable { callback: true };
                            self.landline(
                                c.to,
                                c.from,
                                format!("{callsign} hold short runway {name}, {why}. I'll call you."),
                            );
                        }
                    }
                }
                CoordState::Unable { callback: true } if automated_to => {
                    if self.crossing_blocker(runway, c.aircraft).is_none() {
                        let name = self.runway_name(runway);
                        self.coordination[index].state = CoordState::Approved { until: now + APPROVAL_S };
                        self.landline(c.to, c.from, format!("{callsign} cross runway {name}, approved."));
                    }
                }
                // The player turned it down: ask again in a while.
                CoordState::Unable { callback: false } if !staffing.works(c.from) && now - c.asked >= REASK_S => {
                    let name = self.runway_name(runway);
                    let item = &mut self.coordination[index];
                    item.state = CoordState::Pending;
                    item.asked = now;
                    item.late = false;
                    self.landline(c.from, c.to, format!("{callsign} still holding short runway {name}."));
                    self.emit(SimEvent::CoordinationRequest { callsign });
                }
                CoordState::Approved { until } if automated_to => {
                    let close = !self.arrivals_within(runway, CANCEL_CROSSING_NM, 0.0).is_empty();
                    if close || now >= until {
                        let name = self.runway_name(runway);
                        let why = if close { ", traffic short final. I'll call you" } else { "" };
                        self.landline(c.to, c.from, format!("cancel crossing runway {name} for {callsign}{why}."));
                        if !close {
                            // Lapsed: Ground has to ask again.
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
