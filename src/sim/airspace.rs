//! Airport and airspace data, loaded from TOML.

use super::units::{FT_PER_NM, heading_vec};
use glam::Vec2;
use serde::Deserialize;

/// The airport built into the game.
pub const DEFAULT_AIRPORT: &str = include_str!("../../assets/airports/fictional.toml");

pub const GLIDESLOPE_DEG: f32 = 3.0;
/// Height the glideslope crosses the threshold, feet.
pub const THRESHOLD_CROSSING_HEIGHT: f32 = 50.0;

#[derive(Debug, Clone, Deserialize)]
pub struct Fix {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

impl Fix {
    pub fn pos(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

/// One landing direction of a runway. `x`/`y` is the landing threshold.
#[derive(Debug, Clone, Deserialize)]
pub struct Runway {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub length: f32,
}

impl Runway {
    pub fn threshold(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn far_end(&self) -> Vec2 {
        self.threshold() + heading_vec(self.heading) * self.length
    }

    /// Miles to run to the threshold along the course (negative once past it).
    pub fn along(&self, pos: Vec2) -> f32 {
        -(pos - self.threshold()).dot(heading_vec(self.heading))
    }

    /// Miles off the extended centerline, positive to the right of the inbound course.
    pub fn cross(&self, pos: Vec2) -> f32 {
        (pos - self.threshold()).dot(heading_vec(self.heading + 90.0))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub name: String,
    pub arrival: String,
    pub departure: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Star {
    pub config: String,
    pub fixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sid {
    pub config: String,
    pub fixes: Vec<String>,
    pub exit_altitude: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MvaArea {
    pub altitude: f32,
    pub polygon: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Airport {
    pub name: String,
    pub code: String,
    pub elevation: f32,
    pub go_around_altitude: f32,
    pub mva_default: f32,
    pub sector: Vec<[f32; 2]>,
    pub mva: Vec<MvaArea>,
    pub runways: Vec<Runway>,
    pub configs: Vec<Config>,
    pub fixes: Vec<Fix>,
    pub stars: Vec<Star>,
    pub sids: Vec<Sid>,
}

impl Airport {
    /// Parses and checks airport data, so a bad reference fails at load, not mid-shift.
    pub fn from_toml(text: &str) -> Result<Airport, String> {
        let airport: Airport = toml::from_str(text).map_err(|e| e.to_string())?;
        airport.validate()?;
        Ok(airport)
    }

    pub fn default_airport() -> Airport {
        Airport::from_toml(DEFAULT_AIRPORT).expect("built-in airport data is valid")
    }

    fn validate(&self) -> Result<(), String> {
        if self.sector.len() < 3 {
            return Err("sector needs at least three points".into());
        }
        if self.configs.is_empty() {
            return Err("airport needs at least one runway configuration".into());
        }
        for config in &self.configs {
            for id in [&config.arrival, &config.departure] {
                if self.runway_index(id).is_none() {
                    return Err(format!("config {}: unknown runway {id}", config.name));
                }
            }
            if !self.stars.iter().any(|s| s.config == config.name) {
                return Err(format!("config {} has no STAR", config.name));
            }
            if !self.sids.iter().any(|s| s.config == config.name) {
                return Err(format!("config {} has no SID", config.name));
            }
        }
        let routes = self
            .stars
            .iter()
            .map(|s| &s.fixes)
            .chain(self.sids.iter().map(|s| &s.fixes));
        for route in routes {
            if route.is_empty() {
                return Err("route with no fixes".into());
            }
            for name in route {
                if self.fix_index(name).is_none() {
                    return Err(format!("route references unknown fix {name}"));
                }
            }
        }
        Ok(())
    }

    pub fn fix_index(&self, name: &str) -> Option<usize> {
        self.fixes.iter().position(|f| f.name == name)
    }

    pub fn runway_index(&self, id: &str) -> Option<usize> {
        self.runways.iter().position(|r| r.id == id)
    }

    /// Fix indices for a validated route.
    pub fn route_indices(&self, names: &[String]) -> Vec<usize> {
        names.iter().filter_map(|n| self.fix_index(n)).collect()
    }

    pub fn in_sector(&self, pos: Vec2) -> bool {
        point_in_polygon(pos, &self.sector)
    }

    /// Minimum vectoring altitude at a position.
    pub fn mva_at(&self, pos: Vec2) -> f32 {
        self.mva
            .iter()
            .filter(|a| point_in_polygon(pos, &a.polygon))
            .map(|a| a.altitude)
            .fold(self.mva_default, f32::max)
    }

    /// Altitude of the glideslope at a distance from the threshold.
    pub fn glideslope_altitude(&self, along: f32) -> f32 {
        let slope = GLIDESLOPE_DEG.to_radians().tan();
        self.elevation + THRESHOLD_CROSSING_HEIGHT + along.max(0.0) * FT_PER_NM * slope
    }
}

pub fn point_in_polygon(p: Vec2, polygon: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[j]);
        if (a[1] > p.y) != (b[1] > p.y) {
            let x = (b[0] - a[0]) * (p.y - a[1]) / (b[1] - a[1]) + a[0];
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_airport_loads() {
        let a = Airport::default_airport();
        assert_eq!(a.configs.len(), 2);
        assert!(a.in_sector(Vec2::ZERO));
        assert!(!a.in_sector(Vec2::new(100.0, 0.0)));
    }

    #[test]
    fn unknown_fix_is_rejected() {
        let bad = DEFAULT_AIRPORT.replace("\"BOLTE\", \"ALDER\"", "\"NOPE\", \"ALDER\"");
        let err = Airport::from_toml(&bad).unwrap_err();
        assert!(err.contains("NOPE"), "{err}");
    }

    #[test]
    fn centerline_geometry_for_27l() {
        let a = Airport::default_airport();
        let r = &a.runways[a.runway_index("27L").unwrap()];
        // 10 NM east of the threshold, on the centerline.
        let on_final = r.threshold() + Vec2::new(10.0, 0.0);
        assert!((r.along(on_final) - 10.0).abs() < 1e-3);
        assert!(r.cross(on_final).abs() < 1e-3);
        // Flying west, north of the centerline is to the right.
        assert!(r.cross(on_final + Vec2::new(0.0, 2.0)) > 1.9);
    }

    #[test]
    fn glideslope_is_about_318_ft_per_mile() {
        let a = Airport::default_airport();
        let per_nm = a.glideslope_altitude(11.0) - a.glideslope_altitude(10.0);
        assert!((per_nm - 318.0).abs() < 2.0, "{per_nm}");
    }

    #[test]
    fn mva_uses_terrain_areas() {
        let a = Airport::default_airport();
        assert_eq!(a.mva_at(Vec2::new(0.0, 0.0)), 2000.0);
        assert_eq!(a.mva_at(Vec2::new(-30.0, 30.0)), 5000.0);
        assert_eq!(a.mva_at(Vec2::new(30.0, -30.0)), 4000.0);
    }

    #[test]
    fn star_fixes_used_for_vectoring_are_outside_terrain() {
        let a = Airport::default_airport();
        for name in ["ALDER", "BRICK", "ELMOR", "FINCH"] {
            let fix = &a.fixes[a.fix_index(name).unwrap()];
            assert_eq!(a.mva_at(fix.pos()), a.mva_default, "{name}");
        }
    }
}
