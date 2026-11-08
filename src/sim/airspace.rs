//! Airport and airspace data, loaded from TOML.

use super::ground_map::{GroundData, GroundMap};
use super::performance::aircraft_type;
use super::units::{FT_PER_NM, heading_vec, normalize_heading};
use glam::Vec2;
use serde::Deserialize;

/// The airport built into the game.
pub const DEFAULT_AIRPORT: &str = include_str!("../../assets/airports/fictional.toml");
/// Generated from FAA data by `tools/import_faa.py`.
pub const KSUS_AIRPORT: &str = include_str!("../../assets/airports/ksus.toml");
/// Generated from OpenStreetMap by `tools/import_osm.py`. ODbL, kept in its own file.
pub const KSUS_GROUND: &str = include_str!("../../assets/airports/ksus_ground.toml");
/// Memphis, generated the same way as KSUS.
pub const KMEM_AIRPORT: &str = include_str!("../../assets/airports/kmem.toml");
pub const KMEM_GROUND: &str = include_str!("../../assets/airports/kmem_ground.toml");
/// Every playable airport in menu order, with any separate ground layout.
pub const CATALOG: [(&str, Option<&str>); 3] = [
    (DEFAULT_AIRPORT, None),
    (KSUS_AIRPORT, Some(KSUS_GROUND)),
    (KMEM_AIRPORT, Some(KMEM_GROUND)),
];

/// A ground layout file on its own.
#[derive(Deserialize)]
struct GroundFile {
    ground: GroundData,
}

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
    /// Landing threshold to the far end, NM.
    pub length: f32,
    /// Where takeoffs start, when a displaced threshold puts it behind `x`/`y`.
    pub start_x: Option<f32>,
    pub start_y: Option<f32>,
    #[serde(default = "default_true")]
    pub ils: bool,
    #[serde(default = "default_glideslope")]
    pub glideslope_deg: f32,
    #[serde(default = "default_crossing_height")]
    pub threshold_crossing_height: f32,
    /// The traffic pattern is flown to the right of this end, not the usual left.
    #[serde(default)]
    pub right_traffic: bool,
}

fn default_true() -> bool {
    true
}

fn default_glideslope() -> f32 {
    GLIDESLOPE_DEG
}

fn default_crossing_height() -> f32 {
    THRESHOLD_CROSSING_HEIGHT
}

fn default_vfr_rate() -> f32 {
    super::vfr::VFR_RATE
}

fn default_initial_altitude() -> f32 {
    5000.0
}

impl Runway {
    pub fn threshold(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn far_end(&self) -> Vec2 {
        self.threshold() + heading_vec(self.heading) * self.length
    }

    /// Where a takeoff roll begins.
    pub fn takeoff_point(&self) -> Vec2 {
        match (self.start_x, self.start_y) {
            (Some(x), Some(y)) => Vec2::new(x, y),
            _ => self.threshold(),
        }
    }

    pub fn takeoff_length_ft(&self) -> f32 {
        (self.far_end() - self.takeoff_point()).length() * FT_PER_NM
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
    /// Runway for aircraft that can't use `departure` because it's too short.
    pub departure_long: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Star {
    pub name: Option<String>,
    pub config: String,
    pub fixes: Vec<String>,
    /// True heading flown after the last fix, while waiting for vectors.
    pub final_heading: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sid {
    pub name: Option<String>,
    pub config: String,
    pub fixes: Vec<String>,
    pub exit_altitude: f32,
}

/// A published RNAV (GPS) approach to one runway end. The ILS is implied by
/// `Runway::ils`; this is the other instrument approach.
#[derive(Debug, Clone, Deserialize)]
pub struct Approach {
    /// The runway end, such as "26L".
    pub runway: String,
    /// "RNAV (GPS) RWY 26L".
    pub name: String,
    /// The intermediate fix, the final approach fix, then any step-down fixes,
    /// in order. The runway threshold follows the last.
    pub fixes: Vec<String>,
    /// At or above each fix, feet MSL.
    pub altitudes: Vec<f32>,
    /// The final approach's vertical path, degrees.
    #[serde(default = "default_glideslope")]
    pub glidepath_deg: f32,
    /// Routes from each initial approach fix to the intermediate fix,
    /// both ends included.
    #[serde(default)]
    pub transitions: Vec<Vec<String>>,
}

impl Approach {
    /// The final approach fix: the second fix, after the intermediate fix.
    pub fn faf(&self) -> &str {
        self.fixes.get(1).unwrap_or(&self.fixes[0])
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MvaArea {
    pub altitude: f32,
    pub polygon: Vec<[f32; 2]>,
    /// Areas inside the polygon that belong to a different sector.
    #[serde(default)]
    pub holes: Vec<Vec<[f32; 2]>>,
}

impl MvaArea {
    pub fn contains(&self, pos: Vec2) -> bool {
        point_in_polygon(pos, &self.polygon)
            && !self.holes.iter().any(|hole| point_in_polygon(pos, hole))
    }
}

/// An airport's own traffic. Without one, the default fleet and busy ramp apply.
#[derive(Debug, Clone, Deserialize)]
pub struct TrafficProfile {
    /// Movements per hour at the start and end of a shift.
    pub start_rate: f32,
    pub end_rate: f32,
    pub arrival_share: f32,
    pub callsign_prefixes: Vec<String>,
    pub fleet: Vec<String>,
    pub fleet_weights: Vec<f32>,
    /// Who flies each fleet type, by callsign prefix. Empty, or one list per
    /// type; a type with an empty list uses `callsign_prefixes`.
    #[serde(default)]
    pub fleet_airlines: Vec<Vec<String>>,
    /// Callsign prefixes that park on the cargo ramp rather than at a gate.
    #[serde(default)]
    pub cargo_airlines: Vec<String>,
}

/// An airline's ramp tower: Ground hands its aircraft over at a ramp spot,
/// and it picks their stands and brings departures out.
#[derive(Debug, Clone, Deserialize)]
pub struct RampControl {
    /// As spoken: "over to FedEx Ramp".
    pub name: String,
    /// Callsign prefixes it handles.
    pub airlines: Vec<String>,
}

/// Airports traffic flies to or from in one direction, for flight plans. The
/// sim doesn't fly beyond the sector, so these only fill in the strips.
#[derive(Debug, Clone, Deserialize)]
pub struct CityPair {
    /// True bearing from the field, degrees.
    pub bearing: f32,
    pub airports: Vec<String>,
}

/// A map landmark with no simulated traffic.
#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

/// Which runway VFR traffic gets. Either way it flies from the configured
/// departure runway.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VfrRunway {
    /// VFR has the departure runway to itself, and IFR departures move over
    /// to share the arrival runway, as at a general aviation field.
    #[default]
    Own,
    /// VFR shares the departure runway with IFR departures, which keep it,
    /// as at an airline airport that can't give up a runway.
    Shared,
}

/// The class of the airspace around the airport's surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum AirspaceClass {
    B,
    C,
    #[default]
    D,
}

impl AirspaceClass {
    /// 14 CFR 91.117(b): 200 knots at or below 2,500 feet above the surface
    /// within 4 NM of the primary airport of a Class C or D area. Class B
    /// airports come under 91.117(a) instead.
    pub fn surface_speed_limit(self) -> bool {
        matches!(self, AirspaceClass::C | AirspaceClass::D)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Airport {
    pub name: String,
    pub code: String,
    pub elevation: f32,
    /// Degrees, east positive. ATC speaks magnetic; the sim works in true.
    #[serde(default)]
    pub magnetic_variation: f32,
    /// Credit line for airports built from real data.
    pub data_source: Option<String>,
    #[serde(default = "default_initial_altitude")]
    pub departure_initial_altitude: f32,
    pub traffic: Option<TrafficProfile>,
    /// An airline ramp tower running part of the apron, if there is one.
    pub ramp_control: Option<RampControl>,
    #[serde(default)]
    pub references: Vec<Reference>,
    /// Where flights come from and go to, by direction.
    #[serde(default)]
    pub city_pairs: Vec<CityPair>,
    pub go_around_altitude: f32,
    pub mva_default: f32,
    /// Where VFR traffic flies when it's switched on.
    #[serde(default)]
    pub vfr_runway: VfrRunway,
    /// VFR movements per hour at the Normal traffic level.
    #[serde(default = "default_vfr_rate")]
    pub vfr_rate: f32,
    /// Class B, C or D surface airspace.
    #[serde(default)]
    pub airspace: AirspaceClass,
    pub sector: Vec<[f32; 2]>,
    pub mva: Vec<MvaArea>,
    pub runways: Vec<Runway>,
    pub configs: Vec<Config>,
    pub fixes: Vec<Fix>,
    pub stars: Vec<Star>,
    pub sids: Vec<Sid>,
    /// Published holding patterns.
    #[serde(default)]
    pub holds: Vec<super::holding::HoldData>,
    /// Published RNAV approaches.
    #[serde(default)]
    pub approaches: Vec<Approach>,
    /// Taxiways, hold lines and parking, for the tower positions.
    pub ground: Option<GroundData>,
    /// `ground` built into a routing graph at load.
    #[serde(skip)]
    pub surface: Option<GroundMap>,
}

impl Airport {
    /// An airport in the direction of `bearing` (true), chosen by `pick` so
    /// the same flight always gets the same one.
    pub fn city_pair(&self, bearing: f32, pick: u32) -> Option<&str> {
        let nearest = self.city_pairs.iter().min_by(|a, b| {
            let off = |p: &CityPair| super::units::heading_diff(bearing, p.bearing).abs();
            off(a).total_cmp(&off(b))
        })?;
        let n = nearest.airports.len();
        (n > 0).then(|| nearest.airports[pick as usize % n].as_str())
    }

    /// Parses and checks airport data, so a bad reference fails at load, not mid-shift.
    pub fn from_toml(text: &str) -> Result<Airport, String> {
        Airport::load(text, None)
    }

    /// Airport data plus an optional ground layout from a separate file.
    pub fn load(text: &str, ground: Option<&str>) -> Result<Airport, String> {
        let mut airport: Airport = toml::from_str(text).map_err(|e| e.to_string())?;
        if let Some(ground) = ground {
            let file: GroundFile = toml::from_str(ground).map_err(|e| format!("ground: {e}"))?;
            airport.ground = Some(file.ground);
        }
        airport.validate()?;
        if let Some(ground) = &airport.ground {
            let map = GroundMap::build(ground, &airport.runways)
                .map_err(|e| format!("{} ground layout: {e}", airport.code))?;
            airport.surface = Some(map);
        }
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
            if let Some(id) = &config.departure_long
                && self.runway_index(id).is_none()
            {
                return Err(format!("config {}: unknown runway {id}", config.name));
            }
            if let Some(rwy) = self.runway_index(&config.arrival)
                && !self.runways[rwy].ils
            {
                return Err(format!("landing runway {} has no ILS", config.arrival));
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
        for approach in &self.approaches {
            if self.runway_index(&approach.runway).is_none() {
                return Err(format!("{}: unknown runway {}", approach.name, approach.runway));
            }
            if approach.fixes.is_empty() || approach.fixes.len() != approach.altitudes.len() {
                return Err(format!("{}: fixes and altitudes must match and not be empty", approach.name));
            }
            if self.approaches.iter().filter(|a| a.runway == approach.runway).count() > 1 {
                return Err(format!("two RNAV approaches to runway {}", approach.runway));
            }
            for transition in &approach.transitions {
                if transition.last() != approach.fixes.first() {
                    return Err(format!("{}: a transition must end at the intermediate fix", approach.name));
                }
            }
            for name in approach.fixes.iter().chain(approach.transitions.iter().flatten()) {
                if self.fix_index(name).is_none() {
                    return Err(format!("{}: unknown fix {name}", approach.name));
                }
            }
        }
        for hold in &self.holds {
            if self.fix_index(&hold.fix).is_none() {
                return Err(format!("hold at unknown fix {}", hold.fix));
            }
            if self.holds.iter().filter(|h| h.fix == hold.fix).count() > 1 {
                return Err(format!("two holds at {}", hold.fix));
            }
        }
        if let Some(traffic) = &self.traffic {
            if traffic.fleet.is_empty() || traffic.fleet.len() != traffic.fleet_weights.len() {
                return Err("traffic fleet and weights must match and not be empty".into());
            }
            if !traffic.fleet_airlines.is_empty() && traffic.fleet_airlines.len() != traffic.fleet.len() {
                return Err("traffic fleet_airlines needs one list per fleet type".into());
            }
            if traffic.callsign_prefixes.is_empty() {
                return Err("traffic needs at least one callsign prefix".into());
            }
            for name in &traffic.fleet {
                if aircraft_type(name).is_none() {
                    return Err(format!("traffic references unknown type {name}"));
                }
            }
        }
        Ok(())
    }

    /// The published hold at a fix, if there is one.
    pub fn published_hold(&self, fix: usize) -> Option<&super::holding::HoldData> {
        let name = &self.fixes.get(fix)?.name;
        self.holds.iter().find(|h| &h.fix == name)
    }

    pub fn fix_index(&self, name: &str) -> Option<usize> {
        self.fixes.iter().position(|f| f.name == name)
    }

    pub fn runway_index(&self, id: &str) -> Option<usize> {
        self.runways.iter().position(|r| r.id == id)
    }

    /// True heading as a controller would say it.
    pub fn to_magnetic(&self, true_heading: f32) -> f32 {
        normalize_heading(true_heading - self.magnetic_variation)
    }

    pub fn to_true(&self, magnetic_heading: f32) -> f32 {
        normalize_heading(magnetic_heading + self.magnetic_variation)
    }

    /// Fix indices for a validated route.
    pub fn route_indices(&self, names: &[String]) -> Vec<usize> {
        names.iter().filter_map(|n| self.fix_index(n)).collect()
    }

    pub fn in_sector(&self, pos: Vec2) -> bool {
        point_in_polygon(pos, &self.sector)
    }

    /// Minimum vectoring altitude at a position. The default only covers places the
    /// chart doesn't: it isn't a floor under the chart's own values.
    pub fn mva_at(&self, pos: Vec2) -> f32 {
        self.mva
            .iter()
            .filter(|a| a.contains(pos))
            .map(|a| a.altitude)
            .reduce(f32::max)
            .unwrap_or(self.mva_default)
    }

    /// Altitude of a runway's glideslope at a distance from its threshold.
    pub fn glideslope_altitude(&self, rwy: usize, along: f32) -> f32 {
        self.glidepath_altitude(rwy, along, self.runways[rwy].glideslope_deg)
    }

    /// Altitude of a path at `deg` to a runway's threshold crossing height.
    pub fn glidepath_altitude(&self, rwy: usize, along: f32, deg: f32) -> f32 {
        let runway = &self.runways[rwy];
        let slope = deg.to_radians().tan();
        self.elevation + runway.threshold_crossing_height + along.max(0.0) * FT_PER_NM * slope
    }

    /// The RNAV approach to a runway end, if one is published.
    /// The approach gate on this runway end's final: a mile outside the
    /// final approach fix or 5 NM from the threshold, whichever is farther
    /// (Pilot/Controller Glossary), NM from the threshold.
    pub fn approach_gate_nm(&self, end: usize) -> f32 {
        let faf = self.rnav(end).and_then(|a| self.fix_index(a.faf())).map(|f| self.runways[end].along(self.fixes[f].pos()));
        faf.map_or(5.0, |d| (d + 1.0).max(5.0))
    }

    pub fn rnav(&self, end: usize) -> Option<&Approach> {
        let id = &self.runways.get(end)?.id;
        self.approaches.iter().find(|a| &a.runway == id)
    }

    /// Distance between two runway ends' centrelines, feet: how far apart
    /// parallel finals are.
    pub fn centreline_spacing_ft(&self, a: usize, b: usize) -> f32 {
        let (ra, rb) = (&self.runways[a], &self.runways[b]);
        ra.cross(rb.threshold()).abs() * FT_PER_NM
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
        let per_nm = a.glideslope_altitude(0, 11.0) - a.glideslope_altitude(0, 10.0);
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
