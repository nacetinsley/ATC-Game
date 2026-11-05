//! Unit helpers. The sim works in nautical miles, feet, knots and degrees.
//! Positions use x = east, y = north; headings are clockwise from north.

use glam::Vec2;

pub const FT_PER_NM: f32 = 6076.12;
pub const SECONDS_PER_HOUR: f32 = 3600.0;

/// Unit vector pointing along a heading.
pub fn heading_vec(heading: f32) -> Vec2 {
    let r = heading.to_radians();
    Vec2::new(r.sin(), r.cos())
}

/// Heading of a vector, in `[0, 360)`.
pub fn vec_heading(v: Vec2) -> f32 {
    normalize_heading(v.x.atan2(v.y).to_degrees())
}

pub fn normalize_heading(heading: f32) -> f32 {
    let h = heading.rem_euclid(360.0);
    if h >= 360.0 { 0.0 } else { h }
}

/// Shortest signed turn from one heading to another, in `[-180, 180)`. Positive is right.
pub fn heading_diff(from: f32, to: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

/// Heading as a controller says it: rounded, three digits, north is 360.
pub fn format_heading(heading: f32) -> String {
    let h = normalize_heading(heading).round() as i32 % 360;
    format!("{:03}", if h == 0 { 360 } else { h })
}

/// True airspeed from indicated airspeed, using the 2% per 1,000 ft rule of thumb.
pub fn tas_from_ias(ias: f32, altitude: f32) -> f32 {
    ias * (1.0 + 0.02 * altitude.max(0.0) / 1000.0)
}

/// Wind, given the way controllers report it: the direction it blows from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wind {
    pub from: f32,
    pub speed: f32,
}

impl Wind {
    pub const CALM: Wind = Wind { from: 0.0, speed: 0.0 };

    /// Velocity of the air mass in knots. Wind strengthens with altitude.
    pub fn vector_at(&self, altitude: f32) -> Vec2 {
        let speed = self.speed * (1.0 + altitude.max(0.0) / 15000.0);
        -heading_vec(self.from) * speed
    }
}

/// Heading to fly so the ground track follows `track`, correcting for wind.
pub fn heading_for_track(track: f32, tas: f32, wind: Vec2) -> f32 {
    if tas < 1.0 {
        return track;
    }
    // Crosswind component, positive when blowing to the right of the track.
    let right = heading_vec(track + 90.0);
    let ratio = (wind.dot(right) / tas).clamp(-0.9, 0.9);
    normalize_heading(track - ratio.asin().to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_diff_takes_the_shortest_way_across_north() {
        assert_eq!(heading_diff(350.0, 10.0), 20.0);
        assert_eq!(heading_diff(10.0, 350.0), -20.0);
        assert_eq!(heading_diff(90.0, 90.0), 0.0);
        assert_eq!(heading_diff(90.0, 270.0), -180.0);
    }

    #[test]
    fn normalize_wraps_both_ways() {
        assert_eq!(normalize_heading(-10.0), 350.0);
        assert_eq!(normalize_heading(370.0), 10.0);
        assert_eq!(normalize_heading(360.0), 0.0);
    }

    #[test]
    fn heading_vectors_match_compass_directions() {
        assert!((heading_vec(0.0) - Vec2::new(0.0, 1.0)).length() < 1e-5);
        assert!((heading_vec(90.0) - Vec2::new(1.0, 0.0)).length() < 1e-5);
        assert!((vec_heading(Vec2::new(-1.0, 0.0)) - 270.0).abs() < 1e-3);
    }

    #[test]
    fn north_is_spoken_as_360() {
        assert_eq!(format_heading(0.0), "360");
        assert_eq!(format_heading(359.8), "360");
        assert_eq!(format_heading(5.0), "005");
    }

    #[test]
    fn wind_from_the_west_pushes_east() {
        let v = Wind { from: 270.0, speed: 20.0 }.vector_at(0.0);
        assert!((v.x - 20.0).abs() < 1e-3 && v.y.abs() < 1e-3);
    }

    #[test]
    fn crab_angle_points_into_the_wind() {
        // Tracking north with wind from the west: nose must point left of north.
        let wind = Wind { from: 270.0, speed: 30.0 }.vector_at(0.0);
        let h = heading_for_track(0.0, 200.0, wind);
        assert!(heading_diff(0.0, h) < -5.0);
    }
}
