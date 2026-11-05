//! Aircraft performance and wake categories.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WakeCategory {
    Small,
    Large,
    Heavy,
}

impl WakeCategory {
    pub fn letter(self) -> char {
        match self {
            WakeCategory::Small => 'S',
            WakeCategory::Large => 'L',
            WakeCategory::Heavy => 'H',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftType {
    pub designator: &'static str,
    pub wake: WakeCategory,
    /// Final approach speed, knots.
    pub vref: f32,
    /// Slowest and fastest speeds ATC can assign, knots IAS.
    pub min_speed: f32,
    pub max_speed: f32,
    /// Feet per minute, at sea level for climb.
    pub climb_fpm: f32,
    pub descent_fpm: f32,
    /// Knots per second.
    pub accel: f32,
    pub decel: f32,
    /// Altitude it arrives at when offered by Center, feet.
    pub arrival_altitude: f32,
}

pub static TYPES: [AircraftType; 6] = [
    AircraftType {
        designator: "C208",
        wake: WakeCategory::Small,
        vref: 85.0,
        min_speed: 90.0,
        max_speed: 170.0,
        climb_fpm: 900.0,
        descent_fpm: 1000.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 9000.0,
    },
    AircraftType {
        designator: "E175",
        wake: WakeCategory::Large,
        vref: 130.0,
        min_speed: 150.0,
        max_speed: 300.0,
        climb_fpm: 2600.0,
        descent_fpm: 2000.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
    },
    AircraftType {
        designator: "A320",
        wake: WakeCategory::Large,
        vref: 135.0,
        min_speed: 160.0,
        max_speed: 320.0,
        climb_fpm: 2500.0,
        descent_fpm: 2200.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
    },
    AircraftType {
        designator: "B738",
        wake: WakeCategory::Large,
        vref: 140.0,
        min_speed: 160.0,
        max_speed: 320.0,
        climb_fpm: 2500.0,
        descent_fpm: 2200.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
    },
    AircraftType {
        designator: "A359",
        wake: WakeCategory::Heavy,
        vref: 140.0,
        min_speed: 170.0,
        max_speed: 320.0,
        climb_fpm: 2200.0,
        descent_fpm: 2000.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
    },
    AircraftType {
        designator: "B77W",
        wake: WakeCategory::Heavy,
        vref: 145.0,
        min_speed: 170.0,
        max_speed: 320.0,
        climb_fpm: 2000.0,
        descent_fpm: 2000.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
    },
];

pub fn aircraft_type(designator: &str) -> Option<&'static AircraftType> {
    TYPES.iter().find(|t| t.designator == designator)
}

/// Miles the follower needs behind the leader on final.
pub fn wake_required_nm(leader: WakeCategory, follower: WakeCategory) -> f32 {
    use WakeCategory::*;
    match (leader, follower) {
        (Heavy, Heavy) => 4.0,
        (Heavy, Large) => 5.0,
        (Heavy, Small) => 6.0,
        (Large, Small) => 4.0,
        _ => 3.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WakeCategory::*;

    #[test]
    fn wake_table_matches_the_plan() {
        assert_eq!(wake_required_nm(Heavy, Heavy), 4.0);
        assert_eq!(wake_required_nm(Heavy, Large), 5.0);
        assert_eq!(wake_required_nm(Heavy, Small), 6.0);
        assert_eq!(wake_required_nm(Large, Small), 4.0);
        assert_eq!(wake_required_nm(Large, Large), 3.0);
        assert_eq!(wake_required_nm(Small, Heavy), 3.0);
    }

    #[test]
    fn every_type_can_fly_its_own_approach() {
        for t in &TYPES {
            assert!(t.vref <= t.min_speed, "{}", t.designator);
            assert!(t.min_speed < t.max_speed, "{}", t.designator);
        }
    }
}
