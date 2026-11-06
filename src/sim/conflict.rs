//! Safety net: separation, conflict prediction, terrain and wake spacing.

use super::aircraft::{Aircraft, Phase};
use super::airspace::Airport;
use super::performance::wake_required_nm;
use super::scoring::{GoAroundReason, MAX_SEPARATION_LOSSES, ShiftEnd, SimEvent};
use super::units::SECONDS_PER_HOUR;
use super::{Speaker, World};
use glam::Vec2;
use std::collections::BTreeSet;

pub const SEPARATION_NM: f32 = 3.0;
pub const SEPARATION_FT: f32 = 1000.0;
pub const NEAR_COLLISION_NM: f32 = 0.5;
pub const NEAR_COLLISION_FT: f32 = 500.0;
/// Tower doesn't radar-separate VFR traffic, or IFR traffic on its own runways
/// (a departure off one parallel beside an arrival on the other is normal);
/// this close is a near miss.
pub const TOWER_NEAR_COLLISION_NM: f32 = 0.1;
pub const TOWER_NEAR_COLLISION_FT: f32 = 200.0;
pub const STCA_LOOKAHEAD_S: f32 = 60.0;
const STCA_STEP_S: f32 = 5.0;
/// An alert stays up until the pair is this far beyond the limit, to stop it flickering.
const CLEAR_MARGIN_NM: f32 = 0.2;
const CLEAR_MARGIN_FT: f32 = 100.0;
/// Below this height above the field, Tower is separating traffic visually.
const SEPARATION_FLOOR_AGL_FT: f32 = 1000.0;
/// Past this much too close, Tower sends the follower around.
const WAKE_GO_AROUND_MARGIN_NM: f32 = 1.5;

type Pair = (u32, u32);

pub fn is_loss_of_separation(lateral_nm: f32, vertical_ft: f32) -> bool {
    lateral_nm < SEPARATION_NM && vertical_ft < SEPARATION_FT
}

pub fn is_near_collision(lateral_nm: f32, vertical_ft: f32) -> bool {
    lateral_nm < NEAR_COLLISION_NM && vertical_ft < NEAR_COLLISION_FT
}

/// Altitude after `seconds`, levelling off at the assigned altitude.
fn projected_altitude(ac: &Aircraft, seconds: f32) -> f32 {
    let projected = ac.altitude + ac.vertical_speed / 60.0 * seconds;
    if ac.on_glideslope {
        return projected;
    }
    if ac.vertical_speed > 0.0 {
        projected.min(ac.target_altitude)
    } else if ac.vertical_speed < 0.0 {
        projected.max(ac.target_altitude)
    } else {
        projected
    }
}

fn projected_position(ac: &Aircraft, seconds: f32) -> Vec2 {
    ac.pos + ac.ground_velocity / SECONDS_PER_HOUR * seconds
}

/// Will these two lose separation within the look-ahead, on their current velocities?
pub fn predicts_conflict(a: &Aircraft, b: &Aircraft) -> bool {
    let mut t = STCA_STEP_S;
    while t <= STCA_LOOKAHEAD_S {
        let lateral = (projected_position(a, t) - projected_position(b, t)).length();
        let vertical = (projected_altitude(a, t) - projected_altitude(b, t)).abs();
        if is_loss_of_separation(lateral, vertical) {
            return true;
        }
        t += STCA_STEP_S;
    }
    false
}

/// Radar separation is the player's job for this aircraft.
fn separation_applies(ac: &Aircraft, elevation: f32) -> bool {
    !ac.on_ground
        && ac.vfr.is_none()
        && ac.altitude - elevation >= SEPARATION_FLOOR_AGL_FT
        && !matches!(ac.phase, Phase::TowerHandedOff | Phase::TowerRelease | Phase::Rollout)
}

fn same_final(a: &Aircraft, b: &Aircraft) -> bool {
    a.on_final() && b.on_final() && a.approach_runway == b.approach_runway
}

/// Is this aircraft low over terrain, on the player's watch?
pub fn is_below_mva(ac: &Aircraft, airport: &Airport) -> bool {
    !ac.on_ground
        && ac.vfr.is_none()
        && ac.msaw_armed
        && !ac.on_final()
        && !matches!(ac.phase, Phase::GoAround | Phase::TowerRelease | Phase::Rollout)
        // Without a radio it can't be given a safe altitude.
        && !ac.nordo()
        && ac.altitude < airport.mva_at(ac.pos)
}

impl World {
    pub(super) fn run_safety_net(&mut self) {
        self.check_pairs();
        self.check_terrain();
        self.check_wake();
    }

    fn check_pairs(&mut self) {
        let mut los = BTreeSet::new();
        let mut stca = BTreeSet::new();
        let mut events = Vec::new();

        for i in 0..self.aircraft.len() {
            for j in (i + 1)..self.aircraft.len() {
                let (a, b) = (&self.aircraft[i], &self.aircraft[j]);
                if a.on_ground || b.on_ground {
                    continue;
                }
                let lateral = (a.pos - b.pos).length();
                let vertical = (a.altitude - b.altitude).abs();
                let pair = pair_key(a.id, b.id);

                let elevation = self.airport.elevation;
                let with_tower = !separation_applies(a, elevation) && !separation_applies(b, elevation);
                let near = if a.vfr.is_some() || b.vfr.is_some() || with_tower {
                    lateral < TOWER_NEAR_COLLISION_NM && vertical < TOWER_NEAR_COLLISION_FT
                } else {
                    is_near_collision(lateral, vertical)
                };
                if near {
                    events.push(SimEvent::NearCollision {
                        a: a.callsign.clone(),
                        b: b.callsign.clone(),
                    });
                }

                // Pairs on the same final are spaced by the wake rules instead.
                // Two aircraft neither of which is talking to the player are
                // someone else's problem: Center's, or an automated position's.
                if !separation_applies(a, self.airport.elevation)
                    || !separation_applies(b, self.airport.elevation)
                    || same_final(a, b)
                    || (!self.is_players(a) && !self.is_players(b))
                {
                    continue;
                }

                let was_lost = self.alerts.separation.contains(&pair);
                let lost = is_loss_of_separation(lateral, vertical)
                    || (was_lost
                        && lateral < SEPARATION_NM + CLEAR_MARGIN_NM
                        && vertical < SEPARATION_FT + CLEAR_MARGIN_FT);
                if lost {
                    los.insert(pair);
                    // An emergency descent is the pilot's deviation, not the controller's.
                    let deviating = [a, b].iter().any(|x| x.emergency.as_ref().is_some_and(|e| e.descending));
                    if !was_lost && !deviating {
                        events.push(SimEvent::LossOfSeparation {
                            a: a.callsign.clone(),
                            b: b.callsign.clone(),
                        });
                    }
                } else if predicts_conflict(a, b) {
                    stca.insert(pair);
                    if !self.alerts.conflict.contains(&pair) {
                        events.push(SimEvent::ConflictAlert {
                            a: a.callsign.clone(),
                            b: b.callsign.clone(),
                        });
                    }
                }
            }
        }

        self.alerts.separation = los;
        self.alerts.conflict = stca;
        let near_collision = events.iter().any(|e| matches!(e, SimEvent::NearCollision { .. }));
        // Keep repeated near-collision reports out once the shift is over.
        for event in events {
            if self.ended.is_none() {
                self.emit(event);
            }
        }
        if near_collision {
            self.end_shift(ShiftEnd::NearCollision);
        } else if self.score.separation_losses >= MAX_SEPARATION_LOSSES {
            self.end_shift(ShiftEnd::Decertified);
        }
    }

    fn check_terrain(&mut self) {
        // Arm once an aircraft has reached safe altitude after a climb-out.
        let airport = &self.airport;
        for ac in &mut self.aircraft {
            if !ac.msaw_armed && ac.altitude >= airport.mva_at(ac.pos) {
                ac.msaw_armed = true;
            }
        }

        let low: Vec<(u32, String)> = self
            .aircraft
            .iter()
            .filter(|a| is_below_mva(a, &self.airport) && self.is_players(a))
            .map(|a| (a.id, a.callsign.clone()))
            .collect();
        let active: BTreeSet<u32> = low.iter().map(|(id, _)| *id).collect();
        for (id, callsign) in low {
            if !self.alerts.terrain.contains(&id) {
                self.emit(SimEvent::Msaw { callsign });
            }
        }
        self.alerts.terrain = active;
    }

    fn check_wake(&mut self) {
        // Each runway's own final: after a runway change, the last arrivals
        // to the old runway still need spacing.
        let mut on_final: Vec<(usize, f32, usize)> = self
            .aircraft
            .iter()
            .enumerate()
            // VFR traffic self-spaces in the pattern, closer than IFR wake minima.
            .filter(|(_, a)| a.on_final() && a.vfr.is_none())
            .filter_map(|(i, a)| a.approach_runway.map(|r| (r, self.airport.runways[r].along(a.pos), i)))
            .collect();
        on_final.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));

        let mut active = BTreeSet::new();
        let mut events = Vec::new();
        let mut go_arounds = Vec::new();
        for window in on_final.windows(2) {
            let (lead_runway, lead_along, lead) = window[0];
            let (follow_runway, follow_along, follow) = window[1];
            if lead_runway != follow_runway {
                continue;
            }
            let (leader, follower) = (&self.aircraft[lead], &self.aircraft[follow]);
            let required = wake_required_nm(leader.ty.wake, follower.ty.wake);
            let gap = follow_along - lead_along;
            if gap < required {
                let pair = pair_key(leader.id, follower.id);
                active.insert(pair);
                if !self.alerts.wake.contains(&pair) {
                    events.push(SimEvent::WakeViolation {
                        leader: leader.callsign.clone(),
                        follower: follower.callsign.clone(),
                    });
                }
                if gap < required - WAKE_GO_AROUND_MARGIN_NM {
                    go_arounds.push(follow);
                }
            }
        }
        self.alerts.wake = active;
        for event in events {
            self.emit(event);
        }
        for index in go_arounds {
            self.go_around(index, GoAroundReason::WakeSpacing);
        }
    }

    pub(super) fn go_around(&mut self, index: usize, reason: GoAroundReason) {
        let ac = &mut self.aircraft[index];
        ac.start_go_around(&self.airport);
        let callsign = ac.callsign.clone();
        let line = format!("{callsign} going around, climbing to {:.0}.", ac.target_altitude);
        // No radio: Tower sees it go around, but nobody hears it.
        let speaker = if ac.nordo() { Speaker::System } else { Speaker::Pilot };
        self.say(speaker, line);
        self.emit(SimEvent::GoAround { callsign, reason });
        self.emergency_go_around(index);
    }
}

fn pair_key(a: u32, b: u32) -> Pair {
    (a.min(b), a.max(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::aircraft::Flow;
    use crate::sim::performance::aircraft_type;

    fn ac(pos: Vec2, heading: f32, altitude: f32) -> Aircraft {
        Aircraft::new(
            1,
            "TST1",
            Flow::Arrival,
            aircraft_type("B738").unwrap(),
            pos,
            heading,
            altitude,
            200.0,
        )
    }

    #[test]
    fn separation_limit_is_exclusive_on_both_axes() {
        assert!(is_loss_of_separation(2.99, 999.0));
        assert!(!is_loss_of_separation(3.0, 999.0));
        assert!(!is_loss_of_separation(2.99, 1000.0));
    }

    #[test]
    fn near_collision_limit_is_exclusive_on_both_axes() {
        assert!(is_near_collision(0.49, 499.0));
        assert!(!is_near_collision(0.5, 499.0));
        assert!(!is_near_collision(0.49, 500.0));
    }

    #[test]
    fn stca_sees_head_on_traffic_at_the_same_level() {
        let a = ac(Vec2::new(-5.0, 0.0), 90.0, 5000.0);
        let b = ac(Vec2::new(5.0, 0.0), 270.0, 5000.0);
        assert!(predicts_conflict(&a, &b));
    }

    #[test]
    fn stca_stays_quiet_for_diverging_or_level_separated_traffic() {
        let a = ac(Vec2::new(-5.0, 0.0), 270.0, 5000.0);
        let b = ac(Vec2::new(5.0, 0.0), 90.0, 5000.0);
        assert!(!predicts_conflict(&a, &b));
        let above = ac(Vec2::new(5.0, 0.0), 270.0, 6000.0);
        let head_on = ac(Vec2::new(-5.0, 0.0), 90.0, 5000.0);
        assert!(!predicts_conflict(&head_on, &above));
    }

    #[test]
    fn stca_honours_the_assigned_level_off() {
        let mut descending = ac(Vec2::new(5.0, 0.0), 270.0, 7000.0);
        descending.vertical_speed = -2000.0;
        descending.target_altitude = 6000.0;
        let level = ac(Vec2::new(-5.0, 0.0), 90.0, 5000.0);
        assert!(!predicts_conflict(&descending, &level));
    }

    #[test]
    fn msaw_boundary_at_the_mva() {
        let airport = Airport::default_airport();
        let pos = Vec2::new(30.0, -30.0); // 4,000 ft area
        assert!(is_below_mva(&ac(pos, 0.0, 3999.0), &airport));
        assert!(!is_below_mva(&ac(pos, 0.0, 4000.0), &airport));
    }

    #[test]
    fn msaw_ignores_established_and_unarmed_aircraft() {
        let airport = Airport::default_airport();
        let pos = Vec2::new(30.0, -30.0);
        let mut established = ac(pos, 0.0, 1000.0);
        established.phase = Phase::Glideslope;
        assert!(!is_below_mva(&established, &airport));
        let mut climbing = ac(pos, 0.0, 1000.0);
        climbing.msaw_armed = false;
        assert!(!is_below_mva(&climbing, &airport));
    }
}
