//! Lateral navigation: what heading should the aircraft be turning to?

use super::aircraft::{Aircraft, Lateral};
use super::airspace::{Airport, Runway};
use super::holding::Flight;
use super::units::{heading_diff, heading_for_track, vec_heading};
use glam::Vec2;

/// Distance at which a fix counts as reached, NM.
const FIX_CAPTURE_NM: f32 = 1.5;
/// Degrees of correction per mile off the localizer.
const LOCALIZER_GAIN: f32 = 25.0;
const LOCALIZER_MAX_CUT: f32 = 30.0;
/// A visual approach joins final this share of its distance from the runway out.
const VISUAL_JOIN_SHARE: f32 = 0.6;


pub fn desired_heading(ac: &mut Aircraft, wind: Vec2, airport: &Airport, dt: f32) -> f32 {
    let tas = ac.tas();
    match ac.lateral {
        Lateral::Hold(mut state) => {
            let fix = airport.fixes[state.spec.fix].pos();
            let flight = Flight { pos: ac.pos, heading: ac.heading, altitude: ac.altitude, tas };
            let heading = state.steer(flight, wind, fix, dt);
            ac.lateral = Lateral::Hold(state);
            heading
        }
        Lateral::Heading(heading) => heading,
        Lateral::Direct(fix) => {
            let to_fix = airport.fixes[fix].pos() - ac.pos;
            if to_fix.length() < FIX_CAPTURE_NM {
                ac.lateral = Lateral::Heading(ac.heading);
                ac.heading
            } else {
                heading_for_track(vec_heading(to_fix), tas, wind)
            }
        }
        Lateral::Route => {
            while let Some(&fix) = ac.route.get(ac.route_index) {
                if (airport.fixes[fix].pos() - ac.pos).length() >= FIX_CAPTURE_NM {
                    break;
                }
                ac.route_index += 1;
            }
            match ac.route.get(ac.route_index) {
                Some(&fix) => {
                    heading_for_track(vec_heading(airport.fixes[fix].pos() - ac.pos), tas, wind)
                }
                None => {
                    // End of the route: hold the last heading until vectored.
                    let heading = ac.route_end_heading.unwrap_or(ac.heading);
                    ac.lateral = Lateral::Heading(heading);
                    heading
                }
            }
        }
        Lateral::Localizer => match ac.approach_runway {
            Some(rwy) => {
                let runway = &airport.runways[rwy];
                let cut = (runway.cross(ac.pos) * LOCALIZER_GAIN)
                    .clamp(-LOCALIZER_MAX_CUT, LOCALIZER_MAX_CUT);
                heading_for_track(runway.heading - cut, tas, wind)
            }
            None => ac.heading,
        },
        Lateral::Visual(rwy) => {
            let target = visual_join_point(ac.pos, &airport.runways[rwy]);
            heading_for_track(vec_heading(target - ac.pos), tas, wind)
        }
    }
}

/// Where a visual approach aims to join the final: some way out along the
/// extended centreline, closer in the closer the aircraft already is.
pub fn visual_join_point(pos: Vec2, runway: &Runway) -> Vec2 {
    let out = (runway.threshold().distance(pos) * VISUAL_JOIN_SHARE).max(super::approach::VISUAL_MIN_NM);
    runway.threshold() - super::units::heading_vec(runway.heading) * out
}

/// Should an aircraft cleared for the approach start turning onto the localizer now?
/// Leads the turn so it rolls out on the centerline instead of overshooting.
pub fn should_capture_localizer(ac: &Aircraft, runway: &Runway) -> bool {
    let along = runway.along(ac.pos);
    if !(0.5..=40.0).contains(&along) {
        return false;
    }
    let turn_radius = ac.tas() / 3600.0 / super::aircraft::STANDARD_TURN_RATE.to_radians();
    let angle = heading_diff(ac.heading, runway.heading).abs().to_radians();
    let lead = turn_radius * (1.0 - angle.cos()) + 0.1;
    runway.cross(ac.pos).abs() <= lead
}
