//! Turns sim events into a score, an incident list and a shift rating.

use super::emergency::EmergencyKind;
use super::positions::Staffing;

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
pub const POINTS_PARKED: i32 = 50;
pub const PENALTY_INCURSION: i32 = -400;
pub const PENALTY_RUNWAY_SEPARATION: i32 = -300;
pub const PENALTY_GROUND_CONFLICT: i32 = -200;
pub const PENALTY_DEADLOCK: i32 = -100;
pub const PENALTY_MISSED_HANDOFF: i32 = -25;
pub const POINTS_TOUCH_AND_GO: i32 = 20;
pub const POINTS_EMERGENCY_LANDED: i32 = 150;
/// Landed this soon after declaring.
pub const POINTS_EMERGENCY_PROMPT: i32 = 250;
pub const EMERGENCY_PROMPT_MINUTES: f32 = 8.0;
pub const POINTS_EMERGENCY_PARKED: i32 = 50;
pub const PENALTY_EMERGENCY_GO_AROUND: i32 = -150;
pub const PENALTY_EMERGENCY_DIVERT: i32 = -300;
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
    /// A VFR departure left without being told it could change frequency.
    NoFrequencyChange { callsign: String },
    Parked { callsign: String },
    TouchAndGo { callsign: String },
    EmergencyDeclared { callsign: String, kind: EmergencyKind },
    EmergencyLanded { callsign: String, kind: EmergencyKind, minutes: f32 },
    EmergencyGoAround { callsign: String },
    /// A low-fuel aircraft left for its alternate.
    EmergencyDiverted { callsign: String },
    /// A medical arrival reached its gate promptly.
    EmergencyParked { callsign: String },
    ShiftEnded { end: ShiftEnd },
    /// Center is offering an arrival to the player's Approach.
    HandoffOffered { callsign: String },
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

/// Which completed flights earn the player points: only the work their own
/// positions did, not what the automated controllers handled for them.
#[derive(Debug, Clone, Copy)]
pub struct Credit {
    /// Landings and touch-and-goes: Local, or Approach delivering them to final.
    pub landings: bool,
    pub touch_and_goes: bool,
    /// Arrivals taxied in to the gate.
    pub parking: bool,
}

impl Credit {
    pub fn for_staffing(staffing: Staffing) -> Credit {
        Credit {
            landings: staffing.approach || staffing.local,
            touch_and_goes: staffing.local,
            parking: staffing.ground,
        }
    }
}

impl Default for Credit {
    fn default() -> Self {
        Credit { landings: true, touch_and_goes: true, parking: true }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Score {
    /// Departures always count: every position has a hand in getting them out.
    pub credit: Credit,
    pub points: i32,
    pub landed: u32,
    pub exited: u32,
    pub go_arounds: u32,
    pub separation_losses: u32,
    pub parked: u32,
    pub touch_and_goes: u32,
    pub incidents: Vec<Incident>,
    /// Every emergency declared this shift and how it ended, whoever worked it.
    pub emergencies: Vec<EmergencyRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmergencyOutcome {
    /// Still flying when the shift ended.
    Ongoing,
    Landed { minutes: f32 },
    Diverted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmergencyRecord {
    pub callsign: String,
    pub kind: EmergencyKind,
    pub declared_at: f32,
    pub go_arounds: u32,
    pub outcome: EmergencyOutcome,
}

impl EmergencyRecord {
    /// "MRD722  medical emergency  landed after 6 min, 1 go-around".
    pub fn summary(&self) -> String {
        let outcome = match self.outcome {
            EmergencyOutcome::Ongoing => "still airborne".to_string(),
            EmergencyOutcome::Landed { minutes } => format!("landed after {minutes:.0} min"),
            EmergencyOutcome::Diverted => "diverted".to_string(),
        };
        let go_arounds = match self.go_arounds {
            0 => String::new(),
            1 => ", 1 go-around".to_string(),
            n => format!(", {n} go-arounds"),
        };
        format!("{}  {}  {outcome}{go_arounds}", self.callsign, self.kind.describe())
    }
}

fn delay_penalty(minutes: f32) -> i32 {
    let over = (minutes - NOMINAL_ARRIVAL_MINUTES).max(0.0);
    -((over * DELAY_POINTS_PER_MINUTE).min(MAX_DELAY_PENALTY).round() as i32)
}

impl Score {
    pub fn new(credit: Credit) -> Score {
        Score { credit, ..Default::default() }
    }

    /// Keep the emergency list up to date, whatever the credit.
    fn record_emergency(&mut self, time: f32, event: &SimEvent) {
        let record = |list: &[EmergencyRecord], name: &str| -> Option<usize> {
            list.iter().rposition(|r| r.callsign == name && r.outcome == EmergencyOutcome::Ongoing)
        };
        match event {
            SimEvent::EmergencyDeclared { callsign, kind } => self.emergencies.push(EmergencyRecord {
                callsign: callsign.clone(),
                kind: *kind,
                declared_at: time,
                go_arounds: 0,
                outcome: EmergencyOutcome::Ongoing,
            }),
            SimEvent::EmergencyLanded { callsign, minutes, .. } => {
                if let Some(i) = record(&self.emergencies, callsign) {
                    self.emergencies[i].outcome = EmergencyOutcome::Landed { minutes: *minutes };
                }
            }
            SimEvent::EmergencyDiverted { callsign } => {
                if let Some(i) = record(&self.emergencies, callsign) {
                    self.emergencies[i].outcome = EmergencyOutcome::Diverted;
                }
            }
            SimEvent::EmergencyGoAround { callsign } => {
                if let Some(i) = record(&self.emergencies, callsign) {
                    self.emergencies[i].go_arounds += 1;
                }
            }
            _ => {}
        }
    }

    pub fn apply(&mut self, time: f32, event: &SimEvent) {
        self.record_emergency(time, event);
        match event {
            SimEvent::Landed { minutes, .. } => {
                self.landed += 1;
                if self.credit.landings {
                    self.points += POINTS_LANDED + delay_penalty(*minutes);
                }
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
            SimEvent::NoFrequencyChange { callsign } => {
                self.incident(
                    time,
                    format!("{callsign} left without a frequency change"),
                    PENALTY_MISSED_HANDOFF,
                );
            }
            SimEvent::Parked { .. } => {
                self.parked += 1;
                if self.credit.parking {
                    self.points += POINTS_PARKED;
                }
            }
            SimEvent::TouchAndGo { .. } => {
                self.touch_and_goes += 1;
                if self.credit.touch_and_goes {
                    self.points += POINTS_TOUCH_AND_GO;
                }
            }
            SimEvent::EmergencyLanded { callsign, kind, minutes } => {
                if self.credit.landings {
                    let points = if *minutes <= EMERGENCY_PROMPT_MINUTES {
                        POINTS_EMERGENCY_PROMPT
                    } else {
                        POINTS_EMERGENCY_LANDED
                    };
                    let text = format!("{callsign} landed safely with a {} after {minutes:.0} min", kind.describe());
                    self.incident(time, text, points);
                }
            }
            SimEvent::EmergencyGoAround { callsign } => {
                if self.credit.landings {
                    self.incident(time, format!("{callsign} went around during an emergency"), PENALTY_EMERGENCY_GO_AROUND);
                }
            }
            SimEvent::EmergencyDiverted { callsign } => {
                if self.credit.landings {
                    self.incident(time, format!("{callsign} diverted, short of fuel"), PENALTY_EMERGENCY_DIVERT);
                }
            }
            SimEvent::EmergencyParked { callsign } => {
                if self.credit.parking {
                    self.incident(time, format!("{callsign} at the gate for the paramedics"), POINTS_EMERGENCY_PARKED);
                }
            }
            SimEvent::ConflictAlert { .. }
            | SimEvent::EmergencyDeclared { .. }
            | SimEvent::ShiftEnded { .. }
            | SimEvent::HandoffOffered { .. } => {}
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
        let counted = |credited: bool, n: u32| if credited { n as i32 } else { 0 };
        let clean = counted(self.credit.landings, self.landed) * POINTS_LANDED
            + self.exited as i32 * POINTS_EXITED
            + counted(self.credit.parking, self.parked) * POINTS_PARKED
            + counted(self.credit.touch_and_goes, self.touch_and_goes) * POINTS_TOUCH_AND_GO;
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

    #[test]
    fn every_emergency_is_recorded_whoever_gets_the_credit() {
        let mut s = Score::new(Credit { landings: false, touch_and_goes: false, parking: true });
        let declared = |name: &str, kind| SimEvent::EmergencyDeclared { callsign: name.into(), kind };
        s.apply(60.0, &declared("A", EmergencyKind::Medical));
        s.apply(90.0, &declared("B", EmergencyKind::LowFuel));
        s.apply(120.0, &declared("C", EmergencyKind::RadioFailure));
        s.apply(200.0, &SimEvent::EmergencyGoAround { callsign: "A".into() });
        s.apply(400.0, &SimEvent::EmergencyLanded { callsign: "A".into(), kind: EmergencyKind::Medical, minutes: 5.7 });
        s.apply(500.0, &SimEvent::EmergencyDiverted { callsign: "B".into() });
        assert_eq!(s.points, 0);
        let lines: Vec<String> = s.emergencies.iter().map(|r| r.summary()).collect();
        assert_eq!(
            lines,
            [
                "A  medical emergency  landed after 6 min, 1 go-around",
                "B  low fuel  diverted",
                "C  radio failure  still airborne"
            ]
        );
    }

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

    #[test]
    fn only_the_players_own_work_earns_points() {
        let mut s = Score::new(Credit::for_staffing(Staffing::GROUND));
        s.apply(0.0, &landed(5.0));
        s.apply(0.0, &SimEvent::TouchAndGo { callsign: "B".into() });
        assert_eq!((s.points, s.landed, s.touch_and_goes), (0, 1, 1));
        assert!(s.rating(ShiftEnd::Completed).starts_with("No rating"));
        s.apply(0.0, &SimEvent::Parked { callsign: "A".into() });
        assert_eq!(s.points, POINTS_PARKED);
        let mut s = Score::new(Credit::for_staffing(Staffing::LOCAL));
        s.apply(0.0, &SimEvent::Parked { callsign: "A".into() });
        assert_eq!(s.points, 0);
    }
}
