//! Turns sim events into a score, an incident list and a shift rating.

pub const POINTS_LANDED: i32 = 100;
pub const POINTS_EXITED: i32 = 80;
pub const PENALTY_SEPARATION: i32 = -300;
pub const PENALTY_NEAR_COLLISION: i32 = -1000;
pub const PENALTY_WAKE: i32 = -100;
pub const PENALTY_MSAW: i32 = -75;
pub const PENALTY_SECTOR_BUST: i32 = -150;
pub const PENALTY_WRONG_EXIT: i32 = -50;
pub const PENALTY_LOW_EXIT: i32 = -50;
pub const PENALTY_LATE_HANDOFF: i32 = -75;
pub const PENALTY_GO_AROUND: i32 = -25;
pub const PENALTY_LATE_ACCEPT: i32 = -25;
pub const POINTS_PARKED: i32 = 20;
pub const PENALTY_INCURSION: i32 = -400;
pub const PENALTY_RUNWAY_SEPARATION: i32 = -300;
pub const PENALTY_GROUND_CONFLICT: i32 = -200;
pub const PENALTY_DEADLOCK: i32 = -100;
pub const PENALTY_MISSED_HANDOFF: i32 = -25;
/// Separation losses a controller can have before being decertified.
pub const MAX_SEPARATION_LOSSES: u32 = 5;

/// Minutes an arrival can take before delay starts costing points.
const NOMINAL_ARRIVAL_MINUTES: f32 = 15.0;
const DELAY_POINTS_PER_MINUTE: f32 = 2.0;
const MAX_DELAY_PENALTY: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoAroundReason {
    Ordered,
    LateHandoff,
    RunwayOccupied,
    WakeSpacing,
    Unstable,
    NoLandingClearance,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimEvent {
    Landed { callsign: String, minutes: f32 },
    Exited { callsign: String },
    GoAround { callsign: String, reason: GoAroundReason },
    LossOfSeparation { a: String, b: String },
    NearCollision { a: String, b: String },
    ConflictAlert { a: String, b: String },
    WakeViolation { leader: String, follower: String },
    Msaw { callsign: String },
    SectorBust { callsign: String },
    WrongExit { callsign: String },
    LowExit { callsign: String },
    LateAccept { callsign: String },
    LateHandoff { callsign: String },
    /// An aircraft entered a runway with landing traffic close in or a departure rolling.
    RunwayIncursion { callsign: String },
    /// Takeoff begun with the runway still occupied.
    RunwaySeparation { callsign: String },
    GroundConflict { a: String, b: String },
    /// Two aircraft nose to nose with nowhere to go.
    TaxiDeadlock { a: String, b: String },
    MissedHandoff { callsign: String, to: &'static str },
    Parked { callsign: String },
    ShiftEnded { end: ShiftEnd },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftEnd {
    Completed,
    Decertified,
    NearCollision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub time: f32,
    pub text: String,
    pub points: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Score {
    pub points: i32,
    pub landed: u32,
    pub exited: u32,
    pub go_arounds: u32,
    pub separation_losses: u32,
    pub parked: u32,
    pub incidents: Vec<Incident>,
}

fn delay_penalty(minutes: f32) -> i32 {
    let over = (minutes - NOMINAL_ARRIVAL_MINUTES).max(0.0);
    -((over * DELAY_POINTS_PER_MINUTE).min(MAX_DELAY_PENALTY).round() as i32)
}

impl Score {
    pub fn apply(&mut self, time: f32, event: &SimEvent) {
        match event {
            SimEvent::Landed { minutes, .. } => {
                self.landed += 1;
                self.points += POINTS_LANDED + delay_penalty(*minutes);
            }
            SimEvent::Exited { .. } => {
                self.exited += 1;
                self.points += POINTS_EXITED;
            }
            SimEvent::GoAround { callsign, reason } => {
                self.go_arounds += 1;
                // A late handoff is scored by its own event, so don't charge twice.
                let points = if *reason == GoAroundReason::LateHandoff {
                    0
                } else {
                    PENALTY_GO_AROUND
                };
                let why = match reason {
                    GoAroundReason::Ordered => "ordered by the controller",
                    GoAroundReason::LateHandoff => "late handoff to Tower",
                    GoAroundReason::RunwayOccupied => "runway occupied",
                    GoAroundReason::WakeSpacing => "too close behind traffic",
                    GoAroundReason::Unstable => "unstable approach",
                    GoAroundReason::NoLandingClearance => "no landing clearance",
                };
                self.incident(time, format!("{callsign} went around: {why}"), points);
            }
            SimEvent::LossOfSeparation { a, b } => {
                self.separation_losses += 1;
                self.incident(
                    time,
                    format!("Loss of separation: {a} and {b}"),
                    PENALTY_SEPARATION,
                );
            }
            SimEvent::NearCollision { a, b } => {
                self.incident(
                    time,
                    format!("Near collision: {a} and {b}"),
                    PENALTY_NEAR_COLLISION,
                );
            }
            SimEvent::WakeViolation { leader, follower } => {
                self.incident(
                    time,
                    format!("Wake spacing: {follower} too close behind {leader}"),
                    PENALTY_WAKE,
                );
            }
            SimEvent::Msaw { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} below minimum vectoring altitude"),
                    PENALTY_MSAW,
                );
            }
            SimEvent::SectorBust { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} left the sector without a handoff"),
                    PENALTY_SECTOR_BUST,
                );
            }
            SimEvent::WrongExit { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} left the sector away from its exit fix"),
                    PENALTY_WRONG_EXIT,
                );
            }
            SimEvent::LowExit { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} left the sector below the required altitude"),
                    PENALTY_LOW_EXIT,
                );
            }
            SimEvent::LateAccept { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} entered the sector before it was accepted"),
                    PENALTY_LATE_ACCEPT,
                );
            }
            SimEvent::LateHandoff { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} not handed to Tower by 5 NM final"),
                    PENALTY_LATE_HANDOFF,
                );
            }
            SimEvent::RunwayIncursion { callsign } => {
                self.separation_losses += 1;
                self.incident(time, format!("Runway incursion: {callsign}"), PENALTY_INCURSION);
            }
            SimEvent::RunwaySeparation { callsign } => {
                self.separation_losses += 1;
                self.incident(
                    time,
                    format!("{callsign} began its takeoff with the runway occupied"),
                    PENALTY_RUNWAY_SEPARATION,
                );
            }
            SimEvent::GroundConflict { a, b } => {
                self.incident(
                    time,
                    format!("Ground conflict: {a} and {b}"),
                    PENALTY_GROUND_CONFLICT,
                );
            }
            SimEvent::TaxiDeadlock { a, b } => {
                self.incident(
                    time,
                    format!("{a} and {b} are nose to nose on a taxiway"),
                    PENALTY_DEADLOCK,
                );
            }
            SimEvent::MissedHandoff { callsign, to } => {
                self.incident(
                    time,
                    format!("{callsign} not handed to {to}"),
                    PENALTY_MISSED_HANDOFF,
                );
            }
            SimEvent::Parked { .. } => {
                self.parked += 1;
                self.points += POINTS_PARKED;
            }
            SimEvent::ConflictAlert { .. } | SimEvent::ShiftEnded { .. } => {}
        }
    }

    fn incident(&mut self, time: f32, text: String, points: i32) {
        self.points += points;
        self.incidents.push(Incident { time, text, points });
    }

    pub fn completed(&self) -> u32 {
        self.landed + self.exited + self.parked
    }

    /// Letter rating: share of the points a clean shift with this traffic would have scored.
    pub fn rating(&self, end: ShiftEnd) -> &'static str {
        if end == ShiftEnd::NearCollision {
            return "F - near collision";
        }
        if end == ShiftEnd::Decertified {
            return "F - decertified";
        }
        let clean = self.landed as i32 * POINTS_LANDED
            + self.exited as i32 * POINTS_EXITED
            + self.parked as i32 * POINTS_PARKED;
        if clean == 0 {
            return "No rating - no flights completed";
        }
        match self.points as f32 / clean as f32 {
            r if r >= 0.95 => "A - excellent",
            r if r >= 0.80 => "B - good",
            r if r >= 0.60 => "C - adequate",
            r if r >= 0.30 => "D - needs supervision",
            _ => "E - remedial training",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landed(minutes: f32) -> SimEvent {
        SimEvent::Landed { callsign: "A".into(), minutes }
    }

    #[test]
    fn delay_only_costs_points_past_nominal_and_is_capped() {
        let mut s = Score::default();
        s.apply(0.0, &landed(10.0));
        assert_eq!(s.points, POINTS_LANDED);
        let mut s = Score::default();
        s.apply(0.0, &landed(20.0));
        assert_eq!(s.points, POINTS_LANDED - 10);
        let mut s = Score::default();
        s.apply(0.0, &landed(200.0));
        assert_eq!(s.points, POINTS_LANDED - 40);
    }

    #[test]
    fn late_handoff_go_around_is_not_charged_twice() {
        let mut s = Score::default();
        s.apply(
            0.0,
            &SimEvent::GoAround { callsign: "A".into(), reason: GoAroundReason::LateHandoff },
        );
        s.apply(0.0, &SimEvent::LateHandoff { callsign: "A".into() });
        assert_eq!(s.points, PENALTY_LATE_HANDOFF);
        assert_eq!(s.go_arounds, 1);
    }

    #[test]
    fn rating_reflects_end_reason_and_cleanliness() {
        let mut s = Score::default();
        assert!(s.rating(ShiftEnd::Completed).starts_with("No rating"));
        s.apply(0.0, &landed(5.0));
        assert!(s.rating(ShiftEnd::Completed).starts_with('A'));
        assert!(s.rating(ShiftEnd::Decertified).starts_with('F'));
        s.apply(0.0, &SimEvent::Msaw { callsign: "A".into() });
        assert!(s.rating(ShiftEnd::Completed).starts_with('E'));
    }
}
