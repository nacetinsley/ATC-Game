//! Controller positions, and which of them the players are working.
//!
//! Each human at the facility has a seat. Single player is one seat, seat 0,
//! working every staffed position. In co-op each player has their own seat
//! and their own positions; a position nobody works is automated.

/// Who an aircraft is talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Position {
    /// Outside the facility: the en-route centre.
    Center,
    /// Approach and departure radar.
    Approach,
    /// Local control, "Tower": the runways and the airspace close to the field.
    Local,
    /// Ground control: the taxiways.
    Ground,
    /// Clearance Delivery: IFR clearances for departures at the gate.
    Delivery,
    /// An airline's ramp tower, or a pilot parking at an FBO: off ATC frequencies.
    Ramp,
}

impl Position {
    pub fn name(self) -> &'static str {
        match self {
            Position::Center => "Center",
            Position::Approach => "Approach",
            Position::Local => "Tower",
            Position::Ground => "Ground",
            Position::Delivery => "Clearance",
            Position::Ramp => "Ramp",
        }
    }
}

/// One human at the facility. Single player is seat 0.
pub type Seat = u8;

/// The seat a single player sits in.
pub const SOLO: Seat = 0;

/// The most players at one facility: one for each position.
pub const MAX_SEATS: usize = 4;

/// A set of seats: the players who hear a transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Seats(u8);

impl Seats {
    pub const NONE: Seats = Seats(0);

    pub fn of(seat: Seat) -> Seats {
        Seats(1 << seat)
    }

    pub fn has(self, seat: Seat) -> bool {
        self.0 & (1 << seat) != 0
    }

    pub fn any(self) -> bool {
        self.0 != 0
    }

    pub fn union(self, other: Seats) -> Seats {
        Seats(self.0 | other.0)
    }

    pub fn iter(self) -> impl Iterator<Item = Seat> {
        (0..MAX_SEATS as Seat).filter(move |&s| self.has(s))
    }
}

/// The positions players work, and whose seat each is in. The rest are
/// automated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Staffing {
    pub approach: bool,
    pub local: bool,
    pub ground: bool,
    pub delivery: bool,
    /// The seat working Approach, Local, Ground and Delivery, in that order,
    /// where they're worked. All `SOLO` in single player.
    pub seats: [Seat; 4],
}

impl Default for Staffing {
    fn default() -> Self {
        Staffing::APPROACH
    }
}

impl Staffing {
    /// One player working these positions.
    pub const fn solo(approach: bool, local: bool, ground: bool, delivery: bool) -> Staffing {
        Staffing { approach, local, ground, delivery, seats: [SOLO; 4] }
    }

    pub const APPROACH: Staffing = Staffing::solo(true, false, false, false);
    pub const LOCAL: Staffing = Staffing::solo(false, true, false, false);
    pub const GROUND: Staffing = Staffing::solo(false, false, true, false);
    pub const DELIVERY: Staffing = Staffing::solo(false, false, false, true);
    pub const GROUND_DELIVERY: Staffing = Staffing::solo(false, false, true, true);
    pub const LOCAL_GROUND: Staffing = Staffing::solo(false, true, true, false);
    pub const ALL: Staffing = Staffing::solo(true, true, true, true);
    /// Everything automated: for tests and demos, not offered on the menu.
    pub const NONE: Staffing = Staffing::solo(false, false, false, false);

    /// The combinations offered, in menu order. Approach with Local but without
    /// Ground isn't one: a tower cab combines Local with Ground first, and
    /// Ground with Delivery.
    pub const CHOICES: [Staffing; 7] = [
        Staffing::APPROACH,
        Staffing::LOCAL,
        Staffing::GROUND,
        Staffing::DELIVERY,
        Staffing::GROUND_DELIVERY,
        Staffing::LOCAL_GROUND,
        Staffing::ALL,
    ];

    /// A player works this position.
    pub fn works(self, position: Position) -> bool {
        match position {
            Position::Center => false,
            Position::Approach => self.approach,
            Position::Local => self.local,
            Position::Ground => self.ground,
            Position::Delivery => self.delivery,
            Position::Ramp => false,
        }
    }

    /// Where a position comes in `seats`: None for Center and the ramp,
    /// which nobody works.
    fn index(position: Position) -> Option<usize> {
        match position {
            Position::Approach => Some(0),
            Position::Local => Some(1),
            Position::Ground => Some(2),
            Position::Delivery => Some(3),
            Position::Center | Position::Ramp => None,
        }
    }

    /// The seat working this position, if a player works it.
    pub fn seat(self, position: Position) -> Option<Seat> {
        let index = Staffing::index(position)?;
        self.works(position).then_some(self.seats[index])
    }

    /// This seat works this position.
    pub fn seat_works(self, seat: Seat, position: Position) -> bool {
        self.seat(position) == Some(seat)
    }

    /// One player works both: a combined position, so nothing needs
    /// coordinating between them.
    pub fn same_seat(self, a: Position, b: Position) -> bool {
        self.seat(a).is_some() && self.seat(a) == self.seat(b)
    }

    /// One controller has both: one player, or both automated. Between
    /// different controllers, things are coordinated on the landline.
    pub fn same_controller(self, a: Position, b: Position) -> bool {
        self.seat(a) == self.seat(b)
    }

    /// The players who hear a frequency or a landline at this position.
    pub fn ears(self, position: Position) -> Seats {
        self.seat(position).map_or(Seats::NONE, Seats::of)
    }

    /// Every seat with a position.
    pub fn players(self) -> Seats {
        [Position::Approach, Position::Local, Position::Ground, Position::Delivery]
            .into_iter()
            .fold(Seats::NONE, |all, p| all.union(self.ears(p)))
    }

    /// This position given to this seat.
    pub fn with_seat(mut self, position: Position, seat: Seat) -> Staffing {
        assert!((seat as usize) < MAX_SEATS, "seat {seat}");
        if let Some(index) = Staffing::index(position) {
            match position {
                Position::Approach => self.approach = true,
                Position::Local => self.local = true,
                Position::Ground => self.ground = true,
                Position::Delivery => self.delivery = true,
                Position::Center | Position::Ramp => {}
            }
            self.seats[index] = seat;
        }
        self
    }

    /// Only the positions this seat works: what a player at it sees of the
    /// facility, as if they worked alone.
    pub fn of_seat(self, seat: Seat) -> Staffing {
        let mut mine = Staffing::NONE;
        for p in [Position::Approach, Position::Local, Position::Ground, Position::Delivery] {
            if self.seat_works(seat, p) {
                mine = mine.with_seat(p, SOLO);
            }
        }
        mine
    }

    /// Any tower position: the surface matters to the player.
    pub fn tower(self) -> bool {
        self.local || self.ground || self.delivery
    }

    /// Traffic starts and ends at parking, rather than at the runway. Only the
    /// Approach position on its own skips the surface.
    pub fn surface_traffic(self) -> bool {
        self.tower() || !self.approach
    }

    pub fn label(self) -> &'static str {
        match (self.approach, self.local, self.ground, self.delivery) {
            (true, false, false, false) => "Approach",
            (false, true, false, false) => "Local (Tower)",
            (false, false, true, false) => "Ground",
            (false, false, false, true) => "Clearance Delivery",
            (false, false, true, true) => "Ground + Delivery",
            (false, true, true, false) => "Local + Ground",
            (true, true, true, true) => "All combined",
            _ => "Custom",
        }
    }

    pub fn is_offered(self) -> bool {
        Staffing::CHOICES.contains(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approach_with_local_but_no_ground_is_not_offered() {
        let odd = Staffing::solo(true, true, false, false);
        assert!(!odd.is_offered());
        assert!(Staffing::CHOICES.iter().all(|s| s.is_offered()));
        assert_eq!(Staffing::default(), Staffing::APPROACH);
    }

    #[test]
    fn one_player_works_everything_staffed_from_one_seat() {
        let s = Staffing::LOCAL_GROUND;
        assert_eq!(s.seat(Position::Local), Some(SOLO));
        assert_eq!(s.seat(Position::Approach), None);
        assert!(s.same_seat(Position::Local, Position::Ground));
        assert!(!s.same_seat(Position::Local, Position::Approach));
        assert!(s.same_controller(Position::Approach, Position::Center), "both automated");
        assert_eq!(s.players(), Seats::of(SOLO));
        assert_eq!(s.of_seat(SOLO), s);
    }

    #[test]
    fn players_in_two_seats_each_have_their_own() {
        let s = Staffing::NONE
            .with_seat(Position::Approach, 0)
            .with_seat(Position::Local, 1)
            .with_seat(Position::Ground, 1);
        assert!(s.works(Position::Approach) && s.works(Position::Ground));
        assert!(!s.same_seat(Position::Approach, Position::Local));
        assert!(!s.same_controller(Position::Approach, Position::Local));
        assert!(s.same_seat(Position::Local, Position::Ground));
        assert!(s.seat_works(1, Position::Ground) && !s.seat_works(0, Position::Ground));
        assert_eq!(s.players().iter().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(s.of_seat(1), Staffing::LOCAL_GROUND);
        assert_eq!(s.of_seat(0), Staffing::APPROACH);
        assert_eq!(s.ears(Position::Delivery), Seats::NONE);
        assert!(s.ears(Position::Local).union(s.ears(Position::Approach)).has(0));
    }

    #[test]
    fn nobody_works_center() {
        assert!(!Staffing::ALL.works(Position::Center));
        assert!(Staffing::LOCAL_GROUND.works(Position::Ground));
        assert!(!Staffing::LOCAL.works(Position::Ground));
        assert!(Staffing::GROUND_DELIVERY.works(Position::Delivery));
        assert!(Staffing::DELIVERY.tower() && Staffing::DELIVERY.surface_traffic());
    }
}
