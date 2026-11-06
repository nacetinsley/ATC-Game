//! Holding patterns: published holds, the hold instruction, and flying the
//! racetrack with a proper entry.
//!
//! The geometry works in a frame at the holding fix: `u` along the inbound
//! course (negative before the fix), `v` across it, positive on the holding
//! side. The inbound leg runs along `v = 0` into the fix; the outbound leg is a
//! turn's width out on the holding side.

use super::scoring::SimEvent;
use super::{Speaker, World};
use super::aircraft::{Aircraft, Lateral, Phase, STANDARD_TURN_RATE};
use super::units::{SECONDS_PER_HOUR, heading_diff, heading_for_track, heading_vec, normalize_heading, vec_heading};
use glam::Vec2;
use serde::Deserialize;

/// The fix counts as reached within this distance, NM.
const ARRIVE_NM: f32 = 0.4;
/// Degrees of correction per mile off a leg.
const LEG_GAIN: f32 = 40.0;
const LEG_MAX_CUT: f32 = 30.0;
/// A turn onto a leg ends this close to its heading, degrees; the leg's
/// tracking takes over from there.
const TURN_DONE_DEG: f32 = 15.0;
/// The teardrop entry's outbound offset from the reciprocal, degrees.
const TEARDROP_DEG: f32 = 30.0;
/// Expect further clearance this long after the hold is given, at least, seconds.
/// Holding with no fix ahead uses the nearest one within this distance, NM.
const PRESENT_POSITION_NM: f32 = 5.0;
const EFC_S: f32 = 600.0;
/// Past the EFC by this much with nothing new, the pilot's been forgotten, seconds.
pub const EFC_GRACE_S: f32 = 120.0;
/// The automated Approach holds arrivals at fixes at least this far out, NM.
const AUTO_HOLD_MIN_NM: f32 = 10.0;
/// The bottom of an automated stack, and the step up it, feet.
const AUTO_STACK_BASE_FT: f32 = 5000.0;
const AUTO_STACK_STEP_FT: f32 = 1000.0;
const AUTO_STACK_LEVELS: usize = 4;
/// The most arrivals it holds at once; past that, demand waits outside the sector.
const AUTO_HOLD_MAX: usize = 8;
/// Above this the standard leg is a minute and a half.
const HIGH_HOLD_FT: f32 = 14_000.0;
/// Pilots fly a held turn as a steady bank: a heading this far round keeps
/// the turn going in the right direction.
const FORCE_TURN_DEG: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Turns {
    #[default]
    Right,
    Left,
}

impl Turns {
    /// +1 for right turns, -1 for left.
    pub fn sign(self) -> f32 {
        match self {
            Turns::Right => 1.0,
            Turns::Left => -1.0,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Turns::Right => "right",
            Turns::Left => "left",
        }
    }
}

/// A published hold, as in the airport file.
#[derive(Debug, Clone, Deserialize)]
pub struct HoldData {
    pub fix: String,
    /// True course inbound to the fix, degrees.
    pub inbound: f32,
    #[serde(default)]
    pub turns: Turns,
    pub leg_minutes: Option<f32>,
    pub leg_nm: Option<f32>,
    /// Lowest published holding altitude, feet.
    pub altitude: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoldLeg {
    /// A minute below 14,000 ft, a minute and a half above.
    Standard,
    Minutes(f32),
    Miles(f32),
}

impl HoldLeg {
    fn of(data: &HoldData) -> HoldLeg {
        match (data.leg_nm, data.leg_minutes) {
            (Some(nm), _) => HoldLeg::Miles(nm),
            (None, Some(m)) => HoldLeg::Minutes(m),
            (None, None) => HoldLeg::Standard,
        }
    }

    /// Outbound leg time at this altitude, seconds. Distance legs have none.
    fn seconds(self, altitude: f32) -> Option<f32> {
        match self {
            HoldLeg::Standard if altitude > HIGH_HOLD_FT => Some(90.0),
            HoldLeg::Standard => Some(60.0),
            HoldLeg::Minutes(m) => Some(m * 60.0),
            HoldLeg::Miles(_) => None,
        }
    }

    /// "1-minute legs", "4-mile legs".
    pub fn words(self) -> String {
        match self {
            HoldLeg::Standard | HoldLeg::Minutes(_) => {
                let minutes = match self {
                    HoldLeg::Minutes(m) => m,
                    _ => 1.0,
                };
                if (minutes - minutes.round()).abs() < 0.01 {
                    format!("{}-minute legs", minutes.round() as i32)
                } else {
                    format!("{minutes:.1}-minute legs")
                }
            }
            HoldLeg::Miles(nm) => format!("{}-mile legs", nm.round() as i32),
        }
    }
}

/// What the controller told the aircraft.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoldSpec {
    pub fix: usize,
    /// True course inbound to the fix.
    pub inbound: f32,
    pub turns: Turns,
    pub leg: HoldLeg,
    /// It's the published hold at this fix.
    pub published: bool,
    /// Expect further clearance, sim seconds.
    pub efc: f32,
    /// The same as a UTC clock time, HHMM, as said on the radio.
    pub efc_hhmm: u16,
}

impl HoldSpec {
    /// The published hold at `fix`.
    pub fn published(data: &HoldData, fix: usize, efc: f32) -> HoldSpec {
        HoldSpec {
            fix,
            inbound: normalize_heading(data.inbound),
            turns: data.turns,
            leg: HoldLeg::of(data),
            published: true,
            efc,
            efc_hhmm: 0,
        }
    }

    /// No published hold: the pilot's default, on the course it's flying to
    /// the fix, with right turns.
    pub fn unpublished(fix: usize, inbound: f32, efc: f32) -> HoldSpec {
        HoldSpec {
            fix,
            inbound: normalize_heading(inbound),
            turns: Turns::Right,
            leg: HoldLeg::Standard,
            published: false,
            efc,
            efc_hhmm: 0,
        }
    }

    /// Which way from the fix the hold lies: along the outbound course.
    pub fn direction(&self) -> f32 {
        normalize_heading(self.inbound + 180.0)
    }

    /// The racetrack for drawing, at a given true airspeed: points around it,
    /// starting at the fix.
    pub fn outline(&self, fix: Vec2, tas: f32, altitude: f32) -> Vec<Vec2> {
        let u = heading_vec(self.inbound);
        let v = heading_vec(self.inbound + 90.0 * self.turns.sign());
        let radius = turn_radius(tas);
        let leg = match self.leg.seconds(altitude) {
            Some(s) => tas * s / SECONDS_PER_HOUR,
            None => match self.leg {
                HoldLeg::Miles(nm) => nm,
                _ => unreachable!("only distance legs have no time"),
            },
        };
        let mut points = Vec::new();
        let arc = |centre: Vec2, start: f32, out: &mut Vec<Vec2>| {
            for i in 0..=12 {
                let a = (start + 180.0 * i as f32 / 12.0).to_radians();
                // Angle measured in the (u, v) frame.
                out.push(centre + (u * a.cos() + v * a.sin()) * radius);
            }
        };
        // Turn at the fix onto the outbound side, then the far turn back.
        arc(fix + v * radius, -90.0, &mut points);
        arc(fix - u * leg + v * radius, 90.0, &mut points);
        points.push(fix);
        points
    }
}

/// Where in the pattern an aircraft is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// On the way to the fix, entry not yet chosen.
    ToFix,
    /// Teardrop entry: outbound 30 degrees off the reciprocal, on the holding side.
    Teardrop,
    /// Parallel entry: outbound on the non-holding side.
    Parallel,
    /// Parallel entry: the turn back towards the holding side.
    ParallelTurn,
    /// Parallel entry: back to the fix.
    Return,
    OutboundTurn,
    Outbound,
    InboundTurn,
    Inbound,
}

/// How a pilot joins the hold, by heading on reaching the fix (AIM 5-3-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Direct,
    Teardrop,
    Parallel,
}

/// The entry for an aircraft reaching the fix on `heading`: the 180 degrees
/// either side of the inbound course is direct, then 70 degrees of teardrop
/// and 110 of parallel on the far side.
pub fn entry(heading: f32, inbound: f32, turns: Turns) -> Entry {
    let relative = normalize_heading((heading - inbound) * turns.sign());
    if (110.0..180.0).contains(&relative) {
        Entry::Teardrop
    } else if (180.0..290.0).contains(&relative) {
        Entry::Parallel
    } else {
        Entry::Direct
    }
}

/// Turn radius at standard rate, NM.
fn turn_radius(tas: f32) -> f32 {
    // The same rate the aircraft turns at: standard, or less when bank-limited.
    let rate = STANDARD_TURN_RATE.min(super::aircraft::BANK_LIMIT_FACTOR / tas.max(1.0));
    tas / SECONDS_PER_HOUR / rate.to_radians()
}

/// Maximum holding airspeed (AIM 5-3-8), knots.
pub fn max_speed(altitude: f32) -> f32 {
    if altitude <= 6_000.0 {
        200.0
    } else if altitude <= HIGH_HOLD_FT {
        230.0
    } else {
        265.0
    }
}

/// Where the aircraft is and how it's flying, for steering the hold.
#[derive(Debug, Clone, Copy)]
pub struct Flight {
    pub pos: Vec2,
    pub heading: f32,
    pub altitude: f32,
    /// True airspeed, knots.
    pub tas: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoldState {
    pub spec: HoldSpec,
    pub stage: Stage,
    /// Seconds into the current timed leg.
    pub elapsed: f32,
    /// Circuits completed: passes over the fix inbound.
    pub circuits: u32,
    /// The pilot has asked for further clearance at the EFC.
    pub efc_called: bool,
    /// The EFC went by with nothing new, and it cost points.
    pub efc_missed: bool,
}

impl HoldState {
    pub fn new(spec: HoldSpec) -> HoldState {
        HoldState { spec, stage: Stage::ToFix, elapsed: 0.0, circuits: 0, efc_called: false, efc_missed: false }
    }

    /// Established: past the entry and flying the racetrack itself.
    pub fn established(&self) -> bool {
        self.circuits > 0
    }

    /// One tick: the heading to steer now. `wind` is the wind vector at the
    /// aircraft's altitude.
    pub fn steer(&mut self, flight: Flight, wind: Vec2, fix: Vec2, dt: f32) -> f32 {
        let Flight { pos, heading, altitude, tas } = flight;
        let spec = self.spec;
        let sign = spec.turns.sign();
        let u_dir = heading_vec(spec.inbound);
        let v_dir = heading_vec(spec.inbound + 90.0 * sign);
        let rel = pos - fix;
        let (u, v) = (rel.dot(u_dir), rel.dot(v_dir));
        let outbound = normalize_heading(spec.inbound + 180.0);
        let to_fix = vec_heading(fix - pos);
        let forced = |dir: f32| normalize_heading(heading + FORCE_TURN_DEG * dir);
        let leg_done = |elapsed: f32| match spec.leg.seconds(altitude) {
            Some(s) => elapsed >= s,
            None => match spec.leg {
                HoldLeg::Miles(nm) => u <= -nm,
                _ => true,
            },
        };

        match self.stage {
            Stage::ToFix => {
                if rel.length() < ARRIVE_NM {
                    self.elapsed = 0.0;
                    self.stage = match entry(heading, spec.inbound, spec.turns) {
                        Entry::Direct => Stage::OutboundTurn,
                        Entry::Teardrop => Stage::Teardrop,
                        Entry::Parallel => Stage::Parallel,
                    };
                    return self.steer(flight, wind, fix, 0.0);
                }
                heading_for_track(to_fix, tas, wind)
            }
            Stage::Teardrop => {
                self.elapsed += dt;
                if self.elapsed >= spec.leg.seconds(altitude).unwrap_or(60.0) {
                    self.stage = Stage::InboundTurn;
                    return forced(sign);
                }
                normalize_heading(outbound - TEARDROP_DEG * sign)
            }
            Stage::Parallel => {
                self.elapsed += dt;
                if self.elapsed >= spec.leg.seconds(altitude).unwrap_or(60.0) {
                    self.stage = Stage::ParallelTurn;
                    return forced(-sign);
                }
                heading_for_track(outbound, tas, wind)
            }
            Stage::ParallelTurn => {
                // Round towards the holding side until pointing at the fix.
                if heading_diff(heading, to_fix).abs() < TURN_DONE_DEG {
                    self.stage = Stage::Return;
                    return heading_for_track(to_fix, tas, wind);
                }
                forced(-sign)
            }
            Stage::Return => {
                if rel.length() < ARRIVE_NM || u > 0.0 && v.abs() < 1.0 {
                    self.stage = Stage::OutboundTurn;
                    return forced(sign);
                }
                heading_for_track(to_fix, tas, wind)
            }
            Stage::OutboundTurn => {
                if heading_diff(heading, outbound).abs() < TURN_DONE_DEG {
                    self.stage = Stage::Outbound;
                    self.elapsed = 0.0;
                } else {
                    return forced(sign);
                }
                self.steer(flight, wind, fix, 0.0)
            }
            Stage::Outbound => {
                // Timing starts abeam the fix.
                if u <= 0.0 {
                    self.elapsed += dt;
                }
                if leg_done(self.elapsed) {
                    self.stage = Stage::InboundTurn;
                    return forced(sign);
                }
                let width = 2.0 * turn_radius(tas);
                let cut = ((width - v) * LEG_GAIN).clamp(-LEG_MAX_CUT, LEG_MAX_CUT);
                heading_for_track(normalize_heading(outbound - cut * sign), tas, wind)
            }
            Stage::InboundTurn => {
                if heading_diff(heading, spec.inbound).abs() < TURN_DONE_DEG * 2.0 {
                    self.stage = Stage::Inbound;
                    return self.steer(flight, wind, fix, 0.0);
                }
                forced(sign)
            }
            Stage::Inbound => {
                if u >= 0.0 {
                    self.circuits += 1;
                    self.stage = Stage::OutboundTurn;
                    return forced(sign);
                }
                let cut = (v * LEG_GAIN).clamp(-LEG_MAX_CUT, LEG_MAX_CUT);
                heading_for_track(normalize_heading(spec.inbound - cut * sign), tas, wind)
            }
        }
    }
}

impl World {
    /// The hold to give an aircraft at a fix: the published one, or the
    /// pilot's default on the course it's flying there. The EFC is ten
    /// minutes on, at the next whole minute.
    pub fn hold_for(&self, ac: &Aircraft, fix: usize) -> HoldSpec {
        let after = self.time + EFC_S;
        let utc = self.config.clock_start + after as f64;
        let efc = after + ((60.0 - utc.rem_euclid(60.0)) % 60.0) as f32;
        let mut spec = match self.airport.published_hold(fix) {
            Some(data) => HoldSpec::published(data, fix, efc),
            None => {
                let to = self.airport.fixes[fix].pos() - ac.pos;
                let inbound = if to.length() > ARRIVE_NM { vec_heading(to) } else { ac.heading };
                HoldSpec::unpublished(fix, inbound, efc)
            }
        };
        spec.efc_hhmm = self.utc(efc).parse().unwrap_or(0);
        spec
    }

    /// The arrival runway can't take anyone: closed with no parallel, or
    /// the old flow still landing after a runway change.
    pub(super) fn arrivals_blocked(&self) -> bool {
        self.closed(self.arrival_runway()) || self.landing_against_flow()
    }

    /// Arrivals held by the automated Approach.
    fn automated_holders(&self) -> impl Iterator<Item = (usize, &Aircraft)> {
        let player_approach = self.config.staffing.approach;
        self.aircraft.iter().enumerate().filter(move |(_, a)| {
            !player_approach && a.phase == Phase::Controlled && matches!(a.lateral, Lateral::Hold(_))
        })
    }

    /// New arrivals join the hold: the runway is blocked, or others are
    /// already waiting and go first.
    pub(super) fn arrivals_held(&self) -> bool {
        self.arrivals_blocked() || self.automated_holders().next().is_some()
    }

    /// The fix and altitude for the next arrival the automated Approach
    /// holds: the least busy published hold that's well out and whose
    /// racetrack stays inside the sector, at its lowest free level.
    pub(super) fn automated_hold_slot(&self) -> Option<(usize, f32)> {
        if self.automated_holders().count() >= AUTO_HOLD_MAX {
            return None;
        }
        let mut best: Option<(usize, f32, usize)> = None;
        for data in &self.airport.holds {
            let Some(fix) = self.airport.fix_index(&data.fix) else { continue };
            let pos = self.airport.fixes[fix].pos();
            if pos.length() < AUTO_HOLD_MIN_NM {
                continue;
            }
            let base = data.altitude.unwrap_or(0.0).max(AUTO_STACK_BASE_FT);
            let top = base + AUTO_STACK_STEP_FT * (AUTO_STACK_LEVELS - 1) as f32;
            let spec = HoldSpec::published(data, fix, 0.0);
            let tas = super::units::tas_from_ias(max_speed(top), top);
            if !spec.outline(pos, tas, top).iter().all(|&p| self.airport.in_sector(p)) {
                continue;
            }
            let taken: Vec<f32> = self
                .automated_holders()
                .filter_map(|(_, a)| match a.lateral {
                    Lateral::Hold(h) if h.spec.fix == fix => Some(a.target_altitude),
                    _ => None,
                })
                .collect();
            let Some(level) = (0..AUTO_STACK_LEVELS)
                .find(|&i| !taken.iter().any(|&t| (t - (base + AUTO_STACK_STEP_FT * i as f32)).abs() < 1.0))
            else {
                continue;
            };
            if best.is_none_or(|(_, _, n)| taken.len() < n) {
                best = Some((fix, base + AUTO_STACK_STEP_FT * level as f32, taken.len()));
            }
        }
        best.map(|(fix, altitude, _)| (fix, altitude))
    }

    /// Each tick: pilots holding for the player ask for further clearance at
    /// their EFC, and a missed one costs points. The automated Approach lets
    /// its held arrivals go, lowest first, once the runway is open.
    pub(super) fn run_holding(&mut self) {
        let now = self.time;
        let mut calls = Vec::new();
        let mut missed = Vec::new();
        for index in 0..self.aircraft.len() {
            let players = self.is_players(&self.aircraft[index]);
            let ac = &mut self.aircraft[index];
            let Lateral::Hold(state) = &mut ac.lateral else { continue };
            if !players {
                continue;
            }
            if now >= state.spec.efc && !state.efc_called {
                state.efc_called = true;
                calls.push((ac.callsign.clone(), state.spec.fix));
            }
            if now >= state.spec.efc + EFC_GRACE_S && !state.efc_missed {
                state.efc_missed = true;
                missed.push(ac.callsign.clone());
            }
        }
        for (callsign, fix) in calls {
            let name = &self.airport.fixes[fix].name;
            let line = format!("{callsign}, holding at {name}, it's our EFC time, request further clearance.");
            self.say(Speaker::Pilot, line);
        }
        for callsign in missed {
            self.emit(SimEvent::MissedEfc { callsign });
        }
        self.release_holders();
    }

    /// The automated Approach sends its lowest held arrival to the final
    /// when the runway is open and there's room.
    fn release_holders(&mut self) {
        if self.arrivals_blocked() || !self.room_on_final() {
            return;
        }
        let next = self
            .automated_holders()
            .min_by(|a, b| {
                a.1.target_altitude.total_cmp(&b.1.target_altitude).then(a.1.spawn_time.total_cmp(&b.1.spawn_time))
            })
            .map(|(i, _)| i);
        if let Some(index) = next {
            let ac = self.aircraft.remove(index);
            self.place_on_final(ac.id, ac.callsign, ac.ty, ac.spawn_time, false);
        }
    }

    /// Where "hold" sends an aircraft without a fix being named: the fix it's
    /// flying to, or the next one along its route, or failing those the
    /// nearest fix within a few miles. Fixes whose hold would leave the
    /// sector, like the arrival gates on its edge, are passed over.
    pub fn hold_fix_for(&self, ac: &Aircraft) -> Option<usize> {
        if let Lateral::Hold(state) = ac.lateral {
            return Some(state.spec.fix);
        }
        let ahead = ac.active_fix().into_iter().chain(match ac.lateral {
            Lateral::Route => ac.route.get(ac.route_index + 1..).unwrap_or(&[]).to_vec(),
            _ => Vec::new(),
        });
        let mut near: Vec<(usize, f32)> = self
            .airport
            .fixes
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.pos().distance(ac.pos)))
            .filter(|&(_, d)| d <= PRESENT_POSITION_NM)
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut candidates = ahead.chain(near.into_iter().map(|(i, _)| i));
        candidates.find(|&fix| self.hold_fits(ac, fix)).or_else(|| ac.active_fix())
    }

    /// The hold this aircraft would get at `fix` stays inside the sector.
    pub fn hold_fits(&self, ac: &Aircraft, fix: usize) -> bool {
        let spec = self.hold_for(ac, fix);
        let altitude = ac.target_altitude;
        let tas = super::units::tas_from_ias(max_speed(altitude).min(ac.ias.max(1.0)), altitude);
        spec.outline(self.airport.fixes[fix].pos(), tas, altitude).iter().all(|&p| self.airport.in_sector(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_follow_the_aim_sectors_for_right_turns() {
        // Inbound 360, right turns: the hold lies east of the inbound course, south of the fix.
        assert_eq!(entry(0.0, 0.0, Turns::Right), Entry::Direct);
        assert_eq!(entry(100.0, 0.0, Turns::Right), Entry::Direct);
        assert_eq!(entry(300.0, 0.0, Turns::Right), Entry::Direct);
        assert_eq!(entry(120.0, 0.0, Turns::Right), Entry::Teardrop);
        assert_eq!(entry(179.0, 0.0, Turns::Right), Entry::Teardrop);
        assert_eq!(entry(181.0, 0.0, Turns::Right), Entry::Parallel);
        assert_eq!(entry(280.0, 0.0, Turns::Right), Entry::Parallel);
    }

    #[test]
    fn left_turns_mirror_the_sectors() {
        assert_eq!(entry(240.0, 0.0, Turns::Left), Entry::Teardrop);
        assert_eq!(entry(100.0, 0.0, Turns::Left), Entry::Parallel);
        assert_eq!(entry(60.0, 0.0, Turns::Left), Entry::Direct);
    }

    #[test]
    fn holding_speeds_step_with_altitude() {
        assert_eq!(max_speed(5000.0), 200.0);
        assert_eq!(max_speed(10000.0), 230.0);
        assert_eq!(max_speed(20000.0), 265.0);
    }

    #[test]
    fn leg_wording() {
        assert_eq!(HoldLeg::Standard.words(), "1-minute legs");
        assert_eq!(HoldLeg::Minutes(1.5).words(), "1.5-minute legs");
        assert_eq!(HoldLeg::Miles(4.0).words(), "4-mile legs");
    }

    /// Fly a hold with a simple point-mass model: the same turn rate and
    /// wind drift the aircraft model uses.
    fn fly(spec: HoldSpec, start: Vec2, heading: f32, wind: Vec2, seconds: f32) -> (HoldState, Vec<Vec2>) {
        let (tas, altitude, dt) = (200.0, 6000.0, 0.25);
        let mut state = HoldState::new(spec);
        let (mut pos, mut hdg) = (start, heading);
        let mut track = Vec::new();
        for _ in 0..(seconds / dt) as usize {
            let want = state.steer(Flight { pos, heading: hdg, altitude, tas }, wind, Vec2::ZERO, dt);
            let turn = heading_diff(hdg, want).clamp(-STANDARD_TURN_RATE * dt, STANDARD_TURN_RATE * dt);
            hdg = normalize_heading(hdg + turn);
            pos += (heading_vec(hdg) * tas + wind) / SECONDS_PER_HOUR * dt;
            track.push(pos);
        }
        (state, track)
    }

    fn spec() -> HoldSpec {
        HoldSpec::unpublished(0, 0.0, 0.0)
    }

    #[test]
    fn a_direct_entry_settles_into_the_racetrack() {
        let (state, track) = fly(spec(), Vec2::new(0.0, -5.0), 0.0, Vec2::ZERO, 900.0);
        assert!(state.circuits >= 3, "{} circuits", state.circuits);
        // Right turns, inbound north: the pattern is south of the fix and east of
        // the course; the turn at the fix swings a turn radius (about 1 NM) past it.
        let late = &track[track.len() / 2..];
        assert!(late.iter().all(|p| p.x > -0.6 && p.x < 3.0 && p.y < 1.5 && p.y > -6.0), "strayed");
    }

    #[test]
    fn every_entry_ends_up_established_in_a_crosswind() {
        let wind = Vec2::new(-30.0, 0.0);
        for heading in [0.0, 90.0, 140.0, 200.0, 250.0, 320.0] {
            let start = -heading_vec(heading) * 4.0;
            let (state, track) = fly(spec(), start, heading, wind, 1200.0);
            assert!(state.established() && state.circuits >= 3, "heading {heading}: {} circuits", state.circuits);
            let late = &track[track.len() * 2 / 3..];
            assert!(
                late.iter().all(|p| p.length() < 8.0),
                "heading {heading}: wandered off"
            );
        }
    }

    #[test]
    fn the_inbound_leg_takes_about_a_minute_in_calm_air() {
        let (mut state, mut pos, mut hdg) = (HoldState::new(spec()), Vec2::new(0.0, -5.0), 0.0f32);
        let (tas, dt) = (200.0, 0.25);
        let mut inbound_since = None;
        let mut legs = Vec::new();
        for _ in 0..(1500.0 / dt) as usize {
            let before = state.stage;
            let want = state.steer(Flight { pos, heading: hdg, altitude: 6000.0, tas }, Vec2::ZERO, Vec2::ZERO, dt);
            if state.established() {
                if before != Stage::Inbound && state.stage == Stage::Inbound {
                    inbound_since = Some(0.0);
                }
                if before == Stage::Inbound
                    && state.stage != Stage::Inbound
                    && let Some(t) = inbound_since.take()
                {
                    legs.push(t);
                }
            }
            if let Some(t) = inbound_since.as_mut() {
                *t += dt;
            }
            let turn = heading_diff(hdg, want).clamp(-STANDARD_TURN_RATE * dt, STANDARD_TURN_RATE * dt);
            hdg = normalize_heading(hdg + turn);
            pos += heading_vec(hdg) * tas / SECONDS_PER_HOUR * dt;
        }
        assert!(!legs.is_empty());
        for t in legs {
            assert!((40.0..80.0).contains(&t), "inbound leg {t:.0} s");
        }
    }
}
