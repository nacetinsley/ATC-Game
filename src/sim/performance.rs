//! Aircraft performance and wake categories.
//!
//! Figures are rounded approximations for gameplay, not manufacturer data.

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
    /// Altitude it arrives at when offered by Center, feet. Also the highest a
    /// departure of this type is expected to reach before leaving the sector.
    pub arrival_altitude: f32,
    /// Shortest runway it will take off from, feet.
    pub min_runway_ft: f32,
}

/// The types an airport with no traffic section draws from. New types go after these.
pub const DEFAULT_FLEET_SIZE: usize = 6;

pub static TYPES: [AircraftType; 13] = [
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
        min_runway_ft: 2500.0,
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
        min_runway_ft: 6000.0,
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
        min_runway_ft: 7000.0,
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
        min_runway_ft: 7500.0,
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
        min_runway_ft: 8500.0,
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
        min_runway_ft: 9500.0,
    },
    AircraftType {
        designator: "C172",
        wake: WakeCategory::Small,
        vref: 65.0,
        min_speed: 80.0,
        max_speed: 120.0,
        climb_fpm: 700.0,
        descent_fpm: 700.0,
        accel: 1.0,
        decel: 1.0,
        arrival_altitude: 6000.0,
        min_runway_ft: 1800.0,
    },
    AircraftType {
        designator: "PC12",
        wake: WakeCategory::Small,
        vref: 85.0,
        min_speed: 110.0,
        max_speed: 240.0,
        climb_fpm: 1500.0,
        descent_fpm: 1500.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 10000.0,
        min_runway_ft: 2600.0,
    },
    AircraftType {
        designator: "C56X",
        wake: WakeCategory::Small,
        vref: 115.0,
        min_speed: 130.0,
        max_speed: 300.0,
        climb_fpm: 3500.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 12000.0,
        min_runway_ft: 3800.0,
    },
    AircraftType {
        designator: "CL35",
        wake: WakeCategory::Small,
        vref: 125.0,
        min_speed: 140.0,
        max_speed: 320.0,
        climb_fpm: 3500.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 5300.0,
    },
    AircraftType {
        designator: "GLF6",
        wake: WakeCategory::Large,
        vref: 130.0,
        min_speed: 150.0,
        max_speed: 320.0,
        climb_fpm: 3000.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 14000.0,
        min_runway_ft: 5900.0,
    },
    AircraftType {
        designator: "PA28",
        wake: WakeCategory::Small,
        vref: 63.0,
        min_speed: 75.0,
        max_speed: 115.0,
        climb_fpm: 650.0,
        descent_fpm: 700.0,
        accel: 1.0,
        decel: 1.0,
        arrival_altitude: 6000.0,
        min_runway_ft: 1800.0,
    },
    AircraftType {
        designator: "SR22",
        wake: WakeCategory::Small,
        vref: 78.0,
        min_speed: 90.0,
        max_speed: 170.0,
        climb_fpm: 1000.0,
        descent_fpm: 1000.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 8000.0,
        min_runway_ft: 2000.0,
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

    #[test]
    fn default_fleet_is_unchanged_by_later_additions() {
        let names: Vec<&str> = TYPES[..DEFAULT_FLEET_SIZE].iter().map(|t| t.designator).collect();
        assert_eq!(names, ["C208", "E175", "A320", "B738", "A359", "B77W"]);
    }
}
