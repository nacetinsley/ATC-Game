//! Co-op players coming and going in the middle of a shift.
//!
//! A player who drops has their positions automated until they come back.
//! Both are commands (`Action::Vacate` and `Action::Take`), so every machine,
//! and a replay, changes over at the same tick.
//!
//! The automated tower positions pick up whatever is on their frequency,
//! tick by tick. The automated Approach doesn't vector: it holds arrivals and
//! sends them to a 10-mile final, and lets departures go. So when Approach
//! goes automated, its traffic is handed over to work that way.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::holding::HoldState;
use super::positions::{Position, Seat, Staffing};
use super::scoring::SimEvent;
use super::{Speaker, World};

const WORKED: [Position; 4] = [Position::Approach, Position::Local, Position::Ground, Position::Delivery];

impl World {
    /// Who works what as the shift was set up, before anyone dropped.
    pub fn seated(&self) -> Staffing {
        self.seated
    }

    /// This seat's player has gone for now: its positions are automated.
    /// False if the seat had no positions, or has already gone.
    pub(super) fn vacate(&mut self, by: Seat, seat: Seat) -> bool {
        if self.ended.is_some() {
            return false;
        }
        let gone: Vec<Position> =
            WORKED.into_iter().filter(|&p| self.config.staffing.seat_works(seat, p)).collect();
        if gone.is_empty() {
            return false;
        }
        self.record(by, 0, super::replay::Action::Vacate(seat));
        let mut staffing = self.config.staffing;
        for &p in &gone {
            set_works(&mut staffing, p, false);
        }
        self.config.staffing = staffing;
        // Calls the gone player owed can't be made now.
        self.coordination.retain(|c| !(c.state == super::coordination::CoordState::Owed && gone.contains(&c.from)));
        if gone.contains(&Position::Approach) {
            self.automate_approach_traffic();
        }
        let names = names(&gone);
        self.say(Speaker::System, format!("{names} automated: the controller has dropped."));
        true
    }

    /// This seat's player is back: it takes its positions again. False if it
    /// hadn't gone.
    pub(super) fn take(&mut self, by: Seat, seat: Seat) -> bool {
        if self.ended.is_some() {
            return false;
        }
        let back: Vec<Position> = WORKED
            .into_iter()
            .filter(|&p| self.seated.seat_works(seat, p) && !self.config.staffing.works(p))
            .collect();
        if back.is_empty() {
            return false;
        }
        self.record(by, 0, super::replay::Action::Take(seat));
        let mut staffing = self.config.staffing;
        for &p in &back {
            staffing = staffing.with_seat(p, seat);
        }
        self.config.staffing = staffing;
        if back.contains(&Position::Approach) {
            self.staff_approach_holds();
        }
        let names = names(&back);
        self.say(Speaker::System, format!("{names} staffed again: the controller is back."));
        true
    }

    /// Taking over a co-op replay: from here one player, in `by`'s seat,
    /// works every position the players were given. False if it wasn't
    /// co-op.
    pub(super) fn play_solo(&mut self, by: Seat) -> bool {
        if self.ended.is_some() || !self.seated.is_coop() {
            return false;
        }
        self.record(by, 0, super::replay::Action::Solo);
        let back: Vec<Position> =
            WORKED.into_iter().filter(|&p| self.seated.works(p) && !self.config.staffing.works(p)).collect();
        let solo = self.seated.as_solo();
        let solo = Staffing { seats: [by; 4], ..solo };
        self.config.staffing = solo;
        self.seated = solo;
        if back.contains(&Position::Approach) {
            self.staff_approach_holds();
        }
        true
    }

    /// Approach has just gone automated. Arrivals established on the final
    /// go to Tower; the rest join the automated holds (an emergency goes
    /// straight to the final). Departures it was working are let go.
    fn automate_approach_traffic(&mut self) {
        // Decided for everyone first: an arrival put in a hold goes on the
        // end of the list, and mustn't be come to again.
        let ids: Vec<u32> = self.aircraft.iter().map(|a| a.id).collect();
        for id in ids {
            let Some(index) = self.aircraft.iter().position(|a| a.id == id) else { continue };
            let ac = &self.aircraft[index];
            let ifr_airborne = !ac.on_ground && ac.vfr.is_none();
            // Holds the player gave are the automated Approach's holds now.
            let held = matches!(ac.lateral, Lateral::Hold(_));
            match (ac.flow, ac.phase) {
                (_, Phase::Localizer | Phase::Glideslope) if ifr_airborne => {
                    let now = self.time;
                    let ac = &mut self.aircraft[index];
                    ac.phase = Phase::TowerHandedOff;
                    ac.freq_since = now;
                }
                (Flow::Arrival, Phase::Offered | Phase::Controlled | Phase::ClearedApproach)
                    if ifr_airborne && !held =>
                {
                    let ac = self.aircraft.remove(index);
                    self.automate_arrival(ac);
                }
                (Flow::Departure, Phase::Controlled) if ifr_airborne => {
                    let ac = self.aircraft.remove(index);
                    self.emit(SimEvent::Exited { callsign: ac.callsign });
                }
                _ => {}
            }
        }
    }

    /// An arrival the automated Approach takes over: into a hold, or onto
    /// the final if it's an emergency or the holds are full. It goes to the
    /// back of the aircraft list, as the automated Approach's own do.
    fn automate_arrival(&mut self, ac: Aircraft) {
        let slot = if ac.emergency.is_none() { self.automated_hold_slot() } else { None };
        match slot {
            Some((fix, altitude)) => self.place_in_hold(ac.id, ac.callsign, ac.ty, ac.spawn_time, fix, altitude),
            None => {
                let rwy = self.arrival_runway();
                self.place_on_final(ac.id, ac.callsign, ac.ty, ac.spawn_time, false);
                if let Some(emergency) = ac.emergency {
                    let mut placed = self.aircraft.pop().expect("just placed");
                    placed.emergency = Some(emergency);
                    self.priority_final(&mut placed, rwy);
                    self.aircraft.push(placed);
                }
            }
        }
    }

    /// Approach is a player's again: the automated holds become theirs, with
    /// a fresh EFC.
    fn staff_approach_holds(&mut self) {
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let Lateral::Hold(state) = &ac.lateral else { continue };
            if ac.phase != Phase::Controlled || ac.on_ground {
                continue;
            }
            let spec = self.hold_for(ac, state.spec.fix);
            let now = self.time;
            let ac = &mut self.aircraft[index];
            ac.lateral = Lateral::Hold(HoldState::new(spec));
            ac.freq_since = now;
        }
    }
}

fn set_works(staffing: &mut Staffing, position: Position, works: bool) {
    match position {
        Position::Approach => staffing.approach = works,
        Position::Local => staffing.local = works,
        Position::Ground => staffing.ground = works,
        Position::Delivery => staffing.delivery = works,
        Position::Center | Position::Ramp => {}
    }
}

/// "Ground and Delivery are", "Approach is".
fn names(positions: &[Position]) -> String {
    let names: Vec<&str> = positions.iter().map(|p| p.name()).collect();
    match names.as_slice() {
        [one] => format!("{one} is"),
        [rest @ .., last] => format!("{} and {last} are", rest.join(", ")),
        [] => String::new(),
    }
}
