//! Controller positions, and which of them the player is working.

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
}

impl Position {
    pub fn name(self) -> &'static str {
        match self {
            Position::Center => "Center",
            Position::Approach => "Approach",
            Position::Local => "Tower",
            Position::Ground => "Ground",
        }
    }
}

/// The positions the player works. The rest are automated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Staffing {
    pub approach: bool,
    pub local: bool,
    pub ground: bool,
}

impl Default for Staffing {
    fn default() -> Self {
        Staffing::APPROACH
    }
}

impl Staffing {
    pub const APPROACH: Staffing = Staffing { approach: true, local: false, ground: false };
    pub const LOCAL: Staffing = Staffing { approach: false, local: true, ground: false };
    pub const GROUND: Staffing = Staffing { approach: false, local: false, ground: true };
    pub const LOCAL_GROUND: Staffing = Staffing { approach: false, local: true, ground: true };
    pub const ALL: Staffing = Staffing { approach: true, local: true, ground: true };
    /// Everything automated: for tests and demos, not offered on the menu.
    pub const NONE: Staffing = Staffing { approach: false, local: false, ground: false };

    /// The combinations offered, in menu order. Approach with Local but without
    /// Ground isn't one: a tower cab combines Local with Ground first.
    pub const CHOICES: [Staffing; 5] =
        [Staffing::APPROACH, Staffing::LOCAL, Staffing::GROUND, Staffing::LOCAL_GROUND, Staffing::ALL];

    pub fn works(self, position: Position) -> bool {
        match position {
            Position::Center => false,
            Position::Approach => self.approach,
            Position::Local => self.local,
            Position::Ground => self.ground,
        }
    }

    /// Any tower position: the surface matters to the player.
    pub fn tower(self) -> bool {
        self.local || self.ground
    }

    /// Traffic starts and ends at parking, rather than at the runway. Only the
    /// Approach position on its own skips the surface.
    pub fn surface_traffic(self) -> bool {
        self.tower() || !self.approach
    }

    pub fn label(self) -> &'static str {
        match (self.approach, self.local, self.ground) {
            (true, false, false) => "Approach",
            (false, true, false) => "Local (Tower)",
            (false, false, true) => "Ground",
            (false, true, true) => "Local + Ground",
            (true, true, true) => "All three combined",
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
        let odd = Staffing { approach: true, local: true, ground: false };
        assert!(!odd.is_offered());
        assert!(Staffing::CHOICES.iter().all(|s| s.is_offered()));
        assert_eq!(Staffing::default(), Staffing::APPROACH);
    }

    #[test]
    fn nobody_works_center() {
        assert!(!Staffing::ALL.works(Position::Center));
        assert!(Staffing::LOCAL_GROUND.works(Position::Ground));
        assert!(!Staffing::LOCAL.works(Position::Ground));
    }
}
