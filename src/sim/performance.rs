//! Aircraft performance and wake categories.
//!
//! Figures are rounded approximations for gameplay, not manufacturer data.
//! Type designators and CWT categories are from FAA Order JO 7360.1K.

/// The FAA weight class, for the "heavy" in a callsign and the strip prefix.
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

/// Consolidated Wake Turbulence category (FAA Order JO 7110.126): A, the
/// super, down to I, the lightest small aircraft. Wake separation in
/// JO 7110.65 5-5-4 and 3-9-6 is by these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cwt {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
}

impl Cwt {
    pub fn letter(self) -> char {
        (b'A' + self as u8) as char
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftType {
    pub designator: &'static str,
    pub wake: WakeCategory,
    pub cwt: Cwt,
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
    /// Equipped and crewed for Category III approaches: lands in fog below
    /// Category I minimums.
    pub cat3: bool,
}

/// The types an airport with no traffic section draws from. New types go after these.
pub const DEFAULT_FLEET_SIZE: usize = 6;

pub static TYPES: [AircraftType; 19] = [
    AircraftType {
        designator: "C208",
        wake: WakeCategory::Small,
        cwt: Cwt::I,
        vref: 85.0,
        min_speed: 90.0,
        max_speed: 170.0,
        climb_fpm: 900.0,
        descent_fpm: 1000.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 9000.0,
        min_runway_ft: 2500.0,
        cat3: false,
    },
    AircraftType {
        designator: "E75L",
        wake: WakeCategory::Large,
        cwt: Cwt::G,
        vref: 130.0,
        min_speed: 150.0,
        max_speed: 300.0,
        climb_fpm: 2600.0,
        descent_fpm: 2000.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 6000.0,
        cat3: true,
    },
    AircraftType {
        designator: "A320",
        wake: WakeCategory::Large,
        cwt: Cwt::F,
        vref: 135.0,
        min_speed: 160.0,
        max_speed: 320.0,
        climb_fpm: 2500.0,
        descent_fpm: 2200.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 7000.0,
        cat3: true,
    },
    AircraftType {
        designator: "B738",
        wake: WakeCategory::Large,
        cwt: Cwt::F,
        vref: 140.0,
        min_speed: 160.0,
        max_speed: 320.0,
        climb_fpm: 2500.0,
        descent_fpm: 2200.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 7500.0,
        cat3: true,
    },
    AircraftType {
        designator: "A359",
        wake: WakeCategory::Heavy,
        cwt: Cwt::B,
        vref: 140.0,
        min_speed: 170.0,
        max_speed: 320.0,
        climb_fpm: 2200.0,
        descent_fpm: 2000.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
        min_runway_ft: 8500.0,
        cat3: true,
    },
    AircraftType {
        designator: "B77W",
        wake: WakeCategory::Heavy,
        cwt: Cwt::B,
        vref: 145.0,
        min_speed: 170.0,
        max_speed: 320.0,
        climb_fpm: 2000.0,
        descent_fpm: 2000.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
        min_runway_ft: 9500.0,
        cat3: true,
    },
    AircraftType {
        designator: "C172",
        wake: WakeCategory::Small,
        cwt: Cwt::I,
        vref: 65.0,
        min_speed: 80.0,
        max_speed: 120.0,
        climb_fpm: 700.0,
        descent_fpm: 700.0,
        accel: 1.0,
        decel: 1.0,
        arrival_altitude: 6000.0,
        min_runway_ft: 1800.0,
        cat3: false,
    },
    AircraftType {
        designator: "PC12",
        wake: WakeCategory::Small,
        cwt: Cwt::I,
        vref: 85.0,
        min_speed: 110.0,
        max_speed: 240.0,
        climb_fpm: 1500.0,
        descent_fpm: 1500.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 10000.0,
        min_runway_ft: 2600.0,
        cat3: false,
    },
    AircraftType {
        designator: "C56X",
        wake: WakeCategory::Small,
        cwt: Cwt::H,
        vref: 115.0,
        min_speed: 130.0,
        max_speed: 300.0,
        climb_fpm: 3500.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 12000.0,
        min_runway_ft: 3800.0,
        cat3: false,
    },
    AircraftType {
        designator: "CL35",
        wake: WakeCategory::Small,
        cwt: Cwt::H,
        vref: 125.0,
        min_speed: 140.0,
        max_speed: 320.0,
        climb_fpm: 3500.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 5300.0,
        cat3: false,
    },
    AircraftType {
        designator: "GLF6",
        wake: WakeCategory::Large,
        cwt: Cwt::F,
        vref: 130.0,
        min_speed: 150.0,
        max_speed: 320.0,
        climb_fpm: 3000.0,
        descent_fpm: 2500.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 14000.0,
        min_runway_ft: 5900.0,
        cat3: true,
    },
    AircraftType {
        designator: "P28A",
        wake: WakeCategory::Small,
        cwt: Cwt::I,
        vref: 63.0,
        min_speed: 75.0,
        max_speed: 115.0,
        climb_fpm: 650.0,
        descent_fpm: 700.0,
        accel: 1.0,
        decel: 1.0,
        arrival_altitude: 6000.0,
        min_runway_ft: 1800.0,
        cat3: false,
    },
    AircraftType {
        designator: "SR22",
        wake: WakeCategory::Small,
        cwt: Cwt::I,
        vref: 78.0,
        min_speed: 90.0,
        max_speed: 170.0,
        climb_fpm: 1000.0,
        descent_fpm: 1000.0,
        accel: 1.5,
        decel: 1.5,
        arrival_altitude: 8000.0,
        min_runway_ft: 2000.0,
        cat3: false,
    },
    // Memphis: the FedEx hub fleet and regional jets.
    AircraftType {
        designator: "B77L",
        wake: WakeCategory::Heavy,
        cwt: Cwt::B,
        vref: 150.0,
        min_speed: 170.0,
        max_speed: 320.0,
        climb_fpm: 2000.0,
        descent_fpm: 2200.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
        min_runway_ft: 9000.0,
        cat3: true,
    },
    AircraftType {
        designator: "MD11",
        wake: WakeCategory::Heavy,
        cwt: Cwt::C,
        vref: 155.0,
        min_speed: 175.0,
        max_speed: 320.0,
        climb_fpm: 1800.0,
        descent_fpm: 2200.0,
        accel: 1.8,
        decel: 1.1,
        arrival_altitude: 14000.0,
        min_runway_ft: 9500.0,
        cat3: true,
    },
    AircraftType {
        designator: "B763",
        wake: WakeCategory::Heavy,
        cwt: Cwt::C,
        vref: 140.0,
        min_speed: 165.0,
        max_speed: 320.0,
        climb_fpm: 2200.0,
        descent_fpm: 2200.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
        min_runway_ft: 8000.0,
        cat3: true,
    },
    AircraftType {
        designator: "A306",
        wake: WakeCategory::Heavy,
        cwt: Cwt::C,
        vref: 140.0,
        min_speed: 165.0,
        max_speed: 310.0,
        climb_fpm: 2000.0,
        descent_fpm: 2000.0,
        accel: 2.0,
        decel: 1.2,
        arrival_altitude: 14000.0,
        min_runway_ft: 8000.0,
        cat3: true,
    },
    AircraftType {
        designator: "B752",
        wake: WakeCategory::Large,
        cwt: Cwt::E,
        vref: 135.0,
        min_speed: 160.0,
        max_speed: 320.0,
        climb_fpm: 3000.0,
        descent_fpm: 2400.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 6500.0,
        cat3: true,
    },
    AircraftType {
        designator: "CRJ9",
        wake: WakeCategory::Large,
        cwt: Cwt::G,
        vref: 135.0,
        min_speed: 150.0,
        max_speed: 300.0,
        climb_fpm: 2500.0,
        descent_fpm: 2000.0,
        accel: 2.5,
        decel: 1.5,
        arrival_altitude: 13000.0,
        min_runway_ft: 6500.0,
        cat3: false,
    },
];

impl AircraftType {
    /// FAA flight plan equipment suffix: `/L` for RVSM with GNSS (the jets),
    /// `/G` for GNSS without RVSM (turboprops and piston singles).
    pub fn equipment_suffix(&self) -> char {
        match self.designator {
            "C208" | "C172" | "PC12" | "P28A" | "SR22" => 'G',
            _ => 'L',
        }
    }

    /// The altitude it typically files for, hundreds of feet.
    pub fn filed_altitude(&self) -> u32 {
        match self.designator {
            "C172" | "P28A" => 65,
            "SR22" => 90,
            "C208" => 120,
            "PC12" => 260,
            "C56X" | "CL35" => 410,
            "GLF6" => 450,
            "A359" | "B77W" => 370,
            "B77L" | "MD11" | "B763" | "A306" => 330,
            _ => 350,
        }
    }
}

pub fn aircraft_type(designator: &str) -> Option<&'static AircraftType> {
    TYPES.iter().find(|t| t.designator == designator)
}

/// Miles the follower needs behind the leader on approach to the same
/// runway: JO 7110.65 5-5-4, TBL 5-5-4. Where the table has no wake
/// minimum, the 3-mile radar minimum.
pub fn wake_required_nm(leader: Cwt, follower: Cwt) -> f32 {
    use Cwt::*;
    match (leader, follower) {
        (A, A) => 3.0,
        (A, B) => 5.0,
        (A, C | D) => 6.0,
        (A, E | F | G) => 7.0,
        (A, H | I) => 8.0,
        (B | D, B) => 3.0,
        (B | D, C | D) => 4.0,
        (B, E | F | G | H) => 5.0,
        (B, I) => 6.0,
        (C, E | F | G) => 3.5,
        (C, H) => 5.0,
        (C, I) => 6.0,
        (D, E | F | G) => 5.0,
        (D, H | I) => 6.0,
        (E | F, I) => 4.0,
        _ => 3.0,
    }
}

/// The wait between takeoffs from the same runway, or parallels less than
/// 2,500 feet apart, for wake turbulence: JO 7110.65 3-9-6f-h, seconds from
/// the leader starting its roll. `None` when the pair needs none.
pub fn wake_interval_s(leader: Cwt, follower: Cwt) -> Option<f32> {
    use Cwt::*;
    match leader {
        A if follower >= B => Some(180.0),
        B | D if follower >= B => Some(120.0),
        C if follower >= E => Some(120.0),
        E if follower == I => Some(120.0),
        _ => None,
    }
}

/// Does the follower get a wake turbulence advisory behind the leader on
/// the same runway (JO 7110.65 3-10-3)? B-I behind A, B or D; E-I behind
/// C; I behind E.
pub fn wake_advisory(leader: Cwt, follower: Cwt) -> bool {
    use Cwt::*;
    match leader {
        A | B | D => follower >= B,
        C => follower >= E,
        E => follower == I,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwt_letters() {
        assert_eq!(Cwt::A.letter(), 'A');
        assert_eq!(Cwt::I.letter(), 'I');
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
        assert_eq!(names, ["C208", "E75L", "A320", "B738", "A359", "B77W"]);
    }
}
