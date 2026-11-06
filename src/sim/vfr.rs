//! VFR traffic, with Tower: aircraft flying closed traffic in the pattern,
//! arrivals joining the downwind, and departures leaving the area.

use super::aircraft::{Aircraft, Flow, Landing, Lateral, Leg, Phase, STANDARD_TURN_RATE, Vfr, VfrIntent};
use super::airspace::Runway;
use super::performance::{AircraftType, aircraft_type};
use super::scoring::{GoAroundReason, SimEvent};
use super::units::{SECONDS_PER_HOUR, heading_diff, heading_for_track, heading_vec, normalize_heading, vec_heading};
use super::{Speaker, World};
use glam::Vec2;

/// Pattern altitude above the field, feet.
pub const PATTERN_AGL: f32 = 1000.0;
/// The downwind leg's distance from the runway, NM.
pub const DOWNWIND_NM: f32 = 1.0;
/// Pilots turn base this far beyond the threshold, NM.
const BASE_NM: f32 = 1.2;
/// Told to turn base early, pilots still leave this much final, NM.
const MIN_BASE_NM: f32 = 0.5;
/// Extended this far with no gap to turn into, pilots break off and re-enter, NM.
const MAX_EXTEND_NM: f32 = 4.0;
/// Turn crosswind past the runway end, once at least this high above the field.
const CROSSWIND_AGL: f32 = 600.0;
const CROSSWIND_PAST_END_NM: f32 = 0.3;
/// On a long runway, turn crosswind this far from the threshold at the latest, NM.
const CROSSWIND_MAX_ALONG_NM: f32 = 1.5;
/// VFR departures turn on course at this height above the field.
const DEPARTURE_TURN_AGL: f32 = 500.0;
/// A landing clearance is needed by this distance from the threshold, NM.
const LANDING_CHECK_NM: f32 = 0.3;
/// VFR arrivals hold outside this distance from the field until told how to enter.
const AIRSPACE_NM: f32 = 4.0;
/// VFR arrivals appear this far out, this high above the field.
const ARRIVAL_SPAWN_NM: f32 = 9.0;
const ARRIVAL_AGL: f32 = 2000.0;
/// VFR departures climb to this height above the field.
const DEPARTURE_AGL: f32 = 2500.0;
/// A departure should have its frequency change by here, and leaves the board
/// further out, NM from the field.
const RELEASE_NM: f32 = 3.0;
const EXIT_NM: f32 = 4.5;
/// The automated Tower lets departures go at this distance, NM.
const AUTO_RELEASE_NM: f32 = 1.5;
/// Pilots turn base no closer than this behind traffic landing ahead, NM.
const FOLLOW_GAP_NM: f32 = 1.0;
/// A pilot doesn't turn base while an IFR arrival on final for a parallel
/// runway will come abeam the turn within this long, seconds: a wide turn
/// onto final would swing into its path.
const PARALLEL_TURN_S: f32 = 90.0;
/// ... unless it has already passed the base turn point by this much, NM.
const PARALLEL_PASSED_NM: f32 = 0.5;
/// An arrival this far either side of a parallel's centerline is on its final, NM.
const PARALLEL_FINAL_NM: f32 = 1.5;
/// Pilots slow behind traffic on the same leg closer than this, NM.
const CATCH_UP_NM: f32 = 0.8;
/// Closer than this behind traffic on the same leg, pilots slow below its speed
/// to open the gap, NM.
const MIN_TRAIL_NM: f32 = 0.6;
const OPEN_GAP_KT: f32 = 15.0;
/// A pilot at the entry point won't start the join with downwind traffic this
/// close to where they'd merge, NM; they orbit there until it's clear.
const JOIN_CLEAR_NM: f32 = 0.7;
/// How far downwind traffic flies while a pilot joins from the entry point, NM.
const JOIN_TIME_NM: f32 = 1.4;
/// How hard pilots steer back onto a leg, per NM off it.
const TRACK_GAIN: f32 = 1.5;
/// Pilots outside the pattern look out for traffic this close, NM and feet,
/// and turn away from anyone who'd pass closer than `AVOID_CPA_NM` in the next
/// `AVOID_LOOKAHEAD_S` seconds.
const AVOID_NM: f32 = 1.5;
const AVOID_FT: f32 = 800.0;
const AVOID_CPA_NM: f32 = 0.4;
const AVOID_CPA_FT: f32 = 400.0;
const AVOID_LOOKAHEAD_S: f32 = 45.0;
/// VFR arrivals come from within this many degrees of straight out from the
/// pattern side.
const ARRIVAL_SPREAD_DEG: f32 = 45.0;
/// VFR departures turn out toward the pattern side, away from the IFR runway,
/// by between these many degrees off the runway heading.
const DEPARTURE_TURN_DEG: (f32, f32) = (20.0, 70.0);
/// Arrive at the 45 degree entry this far out from the downwind, NM.
const ENTRY_NM: f32 = 1.0;
/// Pilots slow to pattern speed this far from the entry point, so they can
/// turn onto the downwind without swinging wide, NM.
const ENTRY_SLOW_NM: f32 = 3.0;
/// Within this of the entry point it counts as reached, NM.
const ENTRY_CAPTURE_NM: f32 = 0.4;
/// An aircraft whose entry point is taken by an earlier one circles this far
/// out until it's free, NM.
const ENTRY_QUEUE_NM: f32 = 2.0;
/// Joining traffic aims this far ahead along the downwind, NM.
const JOIN_LEAD_NM: f32 = 0.8;
/// Within this of the downwind line, a joining aircraft is on the downwind, NM.
const JOINED_NM: f32 = 0.15;
/// VFR movements per hour at the Real-world traffic level, unless the airport
/// sets its own.
pub const VFR_RATE: f32 = 8.0;
/// Automated Tower extends downwind traffic due to land this long after a
/// waiting departure would need the runway, seconds.
const EXTEND_MARGIN_S: f32 = 20.0;
/// Most closed-traffic aircraft at once, on the ground or in the air.
const MAX_CLOSED: usize = 4;
/// VFR traffic grows with the traffic level only up to this multiple.
const MAX_VFR_MULTIPLIER: f32 = 2.0;
/// With more departures than this waiting, Tower turns down pattern work and
/// VFR departures until the queue clears.
const BUSY_DEPARTURES: usize = 4;
/// Retry a spawn this much later when there's no room, seconds.
const SPAWN_RETRY_S: f32 = 30.0;
/// Light aircraft that fly VFR.
pub const VFR_FLEET: [&str; 3] = ["C172", "PA28", "SR22"];
/// Shares of closed traffic and arrivals among VFR movements; the rest depart.
const CLOSED_SHARE: f32 = 0.45;
const ARRIVAL_SHARE: f32 = 0.30;

/// A runway end's pattern, in its own frame: along the runway from the
/// threshold, and out to the pattern side.
struct Frame {
    threshold: Vec2,
    along: Vec2,
    out: Vec2,
    length: f32,
}

impl Frame {
    fn of(runway: &Runway) -> Frame {
        let side = if runway.right_traffic { 90.0 } else { -90.0 };
        Frame {
            threshold: runway.threshold(),
            along: heading_vec(runway.heading),
            out: heading_vec(runway.heading + side),
            length: runway.length,
        }
    }

    /// Distance past the threshold toward the far end, NM.
    fn a(&self, p: Vec2) -> f32 {
        (p - self.threshold).dot(self.along)
    }

    /// Distance out to the pattern side of the centerline, NM.
    fn c(&self, p: Vec2) -> f32 {
        (p - self.threshold).dot(self.out)
    }

    fn point(&self, a: f32, c: f32) -> Vec2 {
        self.threshold + self.along * a + self.out * c
    }

    /// The start of a 45 degree entry to the downwind.
    fn entry(&self) -> Vec2 {
        self.point(self.length / 2.0 + ENTRY_NM, DOWNWIND_NM + ENTRY_NM)
    }
}

/// Speed flown around the pattern.
pub fn pattern_speed(ty: &AircraftType) -> f32 {
    (ty.vref + 25.0).clamp(ty.min_speed, ty.max_speed)
}

fn turn_radius(ac: &Aircraft) -> f32 {
    ac.tas() / SECONDS_PER_HOUR / STANDARD_TURN_RATE.to_radians()
}

/// The eight compass points, for pilots' position reports.
pub fn compass(bearing: f32) -> &'static str {
    const POINTS: [&str; 8] =
        ["north", "northeast", "east", "southeast", "south", "southwest", "west", "northwest"];
    POINTS[((normalize_heading(bearing) + 22.5) / 45.0) as usize % 8]
}

/// "left" or "right" traffic for a runway end.
pub fn side_word(runway: &Runway) -> &'static str {
    if runway.right_traffic { "right" } else { "left" }
}

/// Short label for where a VFR aircraft is, for strips and data blocks.
pub fn leg_label(vfr: &Vfr) -> &'static str {
    match vfr.leg {
        Leg::Entry => "INBND",
        Leg::Holding => "HOLD",
        Leg::Join => "ENTRY",
        Leg::Upwind => "UPWND",
        Leg::Crosswind => "XWIND",
        Leg::Downwind => "DNWND",
        Leg::Base => "BASE",
        Leg::Final => "FINAL",
        Leg::Departing => "DEP",
    }
}

/// How a traffic call names an aircraft: its type and where it is.
pub fn traffic_description(ac: &Aircraft) -> String {
    let where_ = match &ac.vfr {
        Some(v) => match v.leg {
            Leg::Upwind => "on upwind",
            Leg::Crosswind => "on crosswind",
            Leg::Downwind | Leg::Join => "on downwind",
            Leg::Base => "on base",
            Leg::Final => "on final",
            _ => "inbound",
        },
        None if ac.on_final() => "on final",
        None => "",
    };
    format!("{} {where_}", ac.ty.designator).trim_end().to_string()
}

/// What a pilot on downwind does about the base turn.
enum BaseTurn {
    Wait,
    Turn,
    /// Extended as far as they'll go with no gap: leave and come back in.
    Reenter,
}

impl World {
    /// VFR traffic is on for this shift. Approach on its own never sees it.
    pub fn vfr_enabled(&self) -> bool {
        self.config.vfr && self.config.staffing.surface_traffic()
    }

    /// The runway end VFR traffic uses: the departure runway.
    /// The runway VFR traffic has to itself: the configured departure runway,
    /// with IFR departures moved over to the arrival runway.
    pub fn pattern_runway(&self) -> usize {
        self.configured_departure_runway()
    }

    pub(super) fn run_vfr_traffic(&mut self) {
        // No new VFR traffic while a closed runway leaves one for everyone.
        if !self.vfr_enabled() || self.time < self.next_vfr_time || self.emergencies.closure.is_some() {
            return;
        }
        let roll = self.rng.f32();
        let intent = if roll < CLOSED_SHARE {
            VfrIntent::Closed
        } else if roll < CLOSED_SHARE + ARRIVAL_SHARE {
            VfrIntent::Arrival
        } else {
            VfrIntent::Departure
        };
        let busy = self.departures_waiting() > BUSY_DEPARTURES;
        let spawned = !(busy && intent != VfrIntent::Arrival) && self.spawn_vfr(intent);
        self.next_vfr_time = if spawned {
            let rate = self.airport.vfr_rate * self.config.busyness.multiplier().min(MAX_VFR_MULTIPLIER);
            self.time + 3600.0 / rate * (0.75 + self.rng.f32() * 0.5)
        } else {
            self.time + SPAWN_RETRY_S
        };
    }

    /// Add one VFR aircraft: a departure or closed-traffic aircraft at a parking
    /// spot, or an arrival out to the pattern side. False if there's no room.
    pub fn spawn_vfr(&mut self, intent: VfrIntent) -> bool {
        let ty = aircraft_type(VFR_FLEET[self.rng.usize(0..VFR_FLEET.len())]).expect("listed");
        let end = self.pattern_runway();
        let mut vfr = Vfr::new(intent, end);
        match intent {
            VfrIntent::Closed => {
                let closed = self.aircraft.iter().chain(&self.departure_queue);
                let count = closed.filter(|a| a.vfr.as_ref().is_some_and(|v| v.intent == VfrIntent::Closed)).count();
                if count >= MAX_CLOSED {
                    return false;
                }
                vfr.circuits = self.rng.u32(2..5);
                vfr.cleared_pattern = true;
            }
            VfrIntent::Departure => {
                let runway = &self.airport.runways[end];
                let side = if runway.right_traffic { 1.0 } else { -1.0 };
                let (least, most) = DEPARTURE_TURN_DEG;
                let turn = least + self.rng.f32() * (most - least);
                vfr.outbound = normalize_heading(runway.heading + side * turn);
            }
            VfrIntent::Arrival => return self.spawn_vfr_arrival(ty, vfr),
        }
        let callsign = self.vfr_callsign();
        let id = self.next_id();
        let mut ac = Aircraft::new(id, callsign, Flow::Departure, ty, Vec2::ZERO, 0.0, 0.0, 0.0);
        ac.target_altitude = self.airport.elevation + PATTERN_AGL;
        ac.target_speed = pattern_speed(ty);
        ac.vfr = Some(vfr);
        self.departure_queue.push_back(ac);
        true
    }

    /// A bearing from the field out over the pattern side, where VFR arrivals
    /// come from, clear of the instrument traffic and of VFR departures.
    fn pattern_side_bearing(&mut self, end: usize) -> f32 {
        let runway = &self.airport.runways[end];
        let out = runway.heading + if runway.right_traffic { 90.0 } else { -90.0 };
        normalize_heading(out + (self.rng.f32() * 2.0 - 1.0) * ARRIVAL_SPREAD_DEG)
    }

    fn vfr_callsign(&mut self) -> String {
        loop {
            let callsign = super::registration::n_number(&mut self.rng);
            let taken = self.aircraft.iter().chain(&self.departure_queue).any(|a| a.callsign == callsign);
            if !taken {
                return callsign;
            }
        }
    }

    fn spawn_vfr_arrival(&mut self, ty: &'static AircraftType, vfr: Vfr) -> bool {
        let bearing = self.pattern_side_bearing(vfr.end);
        let pos = heading_vec(bearing) * ARRIVAL_SPAWN_NM;
        let altitude = self.airport.elevation + ARRIVAL_AGL;
        if self.aircraft.iter().any(|a| a.pos.distance(pos) < 2.0 && (a.altitude - altitude).abs() < 1000.0) {
            return false;
        }
        let callsign = self.vfr_callsign();
        let id = self.next_id();
        let heading = vec_heading(-pos);
        let speed = ty.max_speed;
        let mut ac = Aircraft::new(id, callsign, Flow::Arrival, ty, pos, heading, altitude, speed);
        ac.phase = Phase::Pattern;
        ac.vfr = Some(vfr);
        ac.spawn_time = self.time;
        ac.msaw_armed = false;
        ac.entered_sector = self.airport.in_sector(pos);
        ac.freq_since = self.time;
        if self.config.staffing.local {
            let line = format!(
                "Tower, {}, {ARRIVAL_SPAWN_NM:.0} miles {} at {}, inbound for landing.",
                ac.callsign,
                compass(bearing),
                super::clearance::thousands((altitude / 100.0).round() * 100.0)
            );
            self.say(Speaker::Pilot, line);
        }
        self.add_aircraft(ac);
        true
    }

    /// Fly every VFR aircraft around its pattern.
    pub(super) fn run_pattern(&mut self) {
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            if ac.phase == Phase::Pattern && !ac.on_ground && ac.vfr.is_some() && self.fly_pattern(index) {
                continue;
            }
            index += 1;
        }
    }

    /// How far an airborne aircraft has to go to land on this physical runway,
    /// when it's committed to it: on final, or a VFR aircraft on base.
    pub(super) fn landing_distance(&self, ac: &Aircraft, physical: usize) -> Option<f32> {
        if ac.on_ground {
            return None;
        }
        let map = self.surface();
        if let Some(v) = &ac.vfr {
            let committed = ac.phase == Phase::Pattern && matches!(v.leg, Leg::Base | Leg::Final);
            return (committed && map.runway_of_end[v.end] == physical)
                .then(|| self.airport.runways[v.end].threshold().distance(ac.pos));
        }
        let end = ac.approach_runway?;
        (ac.on_final() && map.runway_of_end[end] == physical).then(|| self.airport.runways[end].along(ac.pos))
    }

    /// How far the aircraft it's following has to go to land: zero once it's down,
    /// `None` when it has passed and there's nobody left to follow.
    fn leader_distance(&self, id: u32) -> Option<f32> {
        let leader = self.find(id)?;
        if leader.on_ground {
            return Some(0.0);
        }
        match &leader.vfr {
            Some(v) => match v.leg {
                Leg::Base | Leg::Final => Some(self.airport.runways[v.end].threshold().distance(leader.pos)),
                Leg::Entry | Leg::Holding | Leg::Join | Leg::Downwind => Some(f32::MAX),
                _ => None,
            },
            None if leader.on_final() => {
                leader.approach_runway.map(|e| self.airport.runways[e].along(leader.pos))
            }
            None => Some(f32::MAX),
        }
    }

    /// What an aircraft on downwind does about its base turn now.
    fn base_turn(&self, index: usize, a: f32) -> BaseTurn {
        let ac = &self.aircraft[index];
        let vfr = ac.vfr.as_ref().expect("VFR");
        if vfr.turn_base {
            return if a <= -MIN_BASE_NM { BaseTurn::Turn } else { BaseTurn::Wait };
        }
        let limit = a <= -MAX_EXTEND_NM;
        if !limit && (a > -BASE_NM || vfr.extend) {
            return BaseTurn::Wait;
        }
        // Pilots follow traffic they're told to, and never cut in front of
        // anyone already on base or final.
        let room = -a - FOLLOW_GAP_NM;
        let behind_leader = vfr.follow.and_then(|id| self.leader_distance(id)).is_some_and(|d| d > room);
        let physical = self.surface().runway_of_end[vfr.end];
        let cut_in = self
            .aircraft
            .iter()
            .filter(|b| b.id != ac.id)
            .any(|b| self.landing_distance(b, physical).is_some_and(|d| d > room));
        let parallel = self.parallel_arrival_near(vfr.end, -a);
        match (behind_leader || cut_in || parallel, limit) {
            (false, _) => BaseTurn::Turn,
            (true, false) => BaseTurn::Wait,
            (true, true) => BaseTurn::Reenter,
        }
    }

    /// An IFR arrival on final for a runway parallel to `end` that will come
    /// abeam a base turn `turn_nm` out within `PARALLEL_TURN_S`, or is abeam it now.
    fn parallel_arrival_near(&self, end: usize, turn_nm: f32) -> bool {
        let map = self.surface();
        let physical = map.runway_of_end[end];
        let heading = self.airport.runways[end].heading;
        self.aircraft.iter().any(|b| {
            if b.vfr.is_some() || b.on_ground || b.flow != Flow::Arrival {
                return false;
            }
            let Some(e) = b.approach_runway else { return false };
            let runway = &self.airport.runways[e];
            if map.runway_of_end[e] == physical || heading_diff(runway.heading, heading).abs() > 5.0 {
                return false;
            }
            let side = heading_vec(runway.heading).perp();
            if (b.pos - runway.threshold()).dot(side).abs() > PARALLEL_FINAL_NM {
                return false;
            }
            let d = runway.along(b.pos);
            let speed = b.ground_speed().max(60.0) / SECONDS_PER_HOUR;
            d > turn_nm - PARALLEL_PASSED_NM && d < turn_nm + speed * PARALLEL_TURN_S
        })
    }

    /// One tick of flying the pattern. True if the aircraft left the board.
    fn fly_pattern(&mut self, index: usize) -> bool {
        self.follow_runway_change(index);
        let elevation = self.airport.elevation;
        let pattern_alt = elevation + PATTERN_AGL;
        let ac = &self.aircraft[index];
        let mut vfr = ac.vfr.clone().expect("VFR");
        let runway = self.airport.runways[vfr.end].clone();
        let f = Frame::of(&runway);
        let (a, c) = (f.a(ac.pos), f.c(ac.pos));
        let radius = turn_radius(ac);
        let air = self.wind.vector_at(ac.altitude);
        let local = self.config.staffing.local;
        let callsign = ac.callsign.clone();
        let mut lines = Vec::new();
        // Traffic at, or about to reach, the point on the downwind a joining
        // aircraft would merge at. Pilots wait at the entry point for it to pass.
        let merge = f.length / 2.0;
        let crosswind_at = (f.length + CROSSWIND_PAST_END_NM).min(CROSSWIND_MAX_ALONG_NM);
        let near_merge = |along: f32| (-JOIN_CLEAR_NM..JOIN_TIME_NM + JOIN_CLEAR_NM).contains(&(along - merge));
        let entry_blocked = vfr.leg == Leg::Entry
            && vfr.cleared_pattern
            && self.aircraft.iter().any(|b| {
                b.id != ac.id
                    && !b.on_ground
                    && b.vfr.as_ref().is_some_and(|v| {
                        v.end == vfr.end
                            && match v.leg {
                                Leg::Downwind => near_merge(f.a(b.pos)),
                                Leg::Upwind | Leg::Crosswind => near_merge(crosswind_at),
                                // One aircraft joins at a time.
                                Leg::Join => true,
                                _ => false,
                            }
                    })
            });

        // One aircraft at a time at the entry point: a later one closer in
        // than this one, or already there, keeps it circling further out.
        let to_entry = ac.pos.distance(f.entry());
        let queued = vfr.leg == Leg::Entry
            && to_entry < ENTRY_QUEUE_NM
            && to_entry > ENTRY_CAPTURE_NM * 2.0
            && self.aircraft.iter().any(|b| {
                b.id != ac.id
                    && !b.on_ground
                    && b.pos.distance(f.entry()) < to_entry
                    && b.vfr.as_ref().is_some_and(|v| v.leg == Leg::Entry && v.end == vfr.end)
            });
        let entry_alt = pattern_alt + 500.0;

        // Leg changes.
        let leg = vfr.leg;
        match leg {
            Leg::Entry => {
                let middle = f.point(f.length / 2.0, 0.0);
                if !vfr.cleared_pattern && ac.pos.distance(middle) < AIRSPACE_NM {
                    vfr.leg = Leg::Holding;
                    if local {
                        lines.push(format!("{callsign}, holding outside your airspace, request entry."));
                    }
                } else if vfr.cleared_pattern && !entry_blocked && ac.pos.distance(f.entry()) < ENTRY_CAPTURE_NM {
                    vfr.leg = Leg::Join;
                }
            }
            Leg::Holding if vfr.cleared_pattern => vfr.leg = Leg::Entry,
            // On the downwind once it's actually on the line, not just abeam midfield.
            Leg::Join if (c - DOWNWIND_NM).abs() < JOINED_NM && a < f.length / 2.0 + radius => {
                vfr.leg = Leg::Downwind;
            }
            Leg::Upwind => {
                let agl = ac.altitude - elevation;
                if vfr.intent == VfrIntent::Departure && agl >= DEPARTURE_TURN_AGL {
                    vfr.leg = Leg::Departing;
                } else if a >= (f.length + CROSSWIND_PAST_END_NM).min(CROSSWIND_MAX_ALONG_NM) && agl >= CROSSWIND_AGL {
                    vfr.leg = Leg::Crosswind;
                }
            }
            Leg::Crosswind if c >= DOWNWIND_NM - radius => vfr.leg = Leg::Downwind,
            Leg::Downwind => {
                let midfield = a <= f.length / 2.0;
                if local && midfield && !vfr.reported && vfr.landing.is_none() {
                    vfr.reported = true;
                    let intent = if vfr.wants_full_stop() { "full stop" } else { "touch-and-go" };
                    lines.push(format!(
                        "{callsign}, midfield {} downwind runway {}, {intent}.",
                        side_word(&runway),
                        runway.id
                    ));
                }
                match self.base_turn(index, a) {
                    BaseTurn::Wait => {}
                    BaseTurn::Turn => {
                        if a <= -MAX_EXTEND_NM && local {
                            lines.push(format!("{callsign}, turning base."));
                        }
                        vfr.leg = Leg::Base;
                        vfr.extend = false;
                        vfr.turn_base = false;
                        vfr.follow = None;
                    }
                    BaseTurn::Reenter => {
                        if local {
                            lines.push(format!("{callsign}, breaking off to re-enter the downwind."));
                        }
                        vfr.reset_circuit();
                        vfr.leg = Leg::Entry;
                    }
                }
            }
            Leg::Base if c <= radius + 0.05 => vfr.leg = Leg::Final,
            _ => {}
        }

        // Steering, height and speed for the leg.
        let ac = &mut self.aircraft[index];
        let steer = |dir: Vec2, ac: &Aircraft| Lateral::Heading(heading_for_track(vec_heading(dir), ac.tas(), air));
        let on_line = |offset: f32| (offset * TRACK_GAIN).clamp(-0.6, 0.6);
        let speed = pattern_speed(ac.ty);
        match vfr.leg {
            // Orbit at the entry point until the downwind is clear to join.
            Leg::Entry if queued || (entry_blocked && ac.pos.distance(f.entry()) < ENTRY_CAPTURE_NM * 2.0) => {
                ac.lateral = Lateral::Heading(normalize_heading(ac.heading + 30.0));
                ac.target_altitude = entry_alt;
                ac.target_speed = speed;
            }
            Leg::Entry => {
                ac.lateral = steer(f.entry() - ac.pos, ac);
                ac.target_altitude = entry_alt;
                let near = ac.pos.distance(f.entry()) < ENTRY_SLOW_NM;
                ac.target_speed = if near { speed } else { ac.ty.max_speed };
            }
            Leg::Holding => {
                ac.lateral = Lateral::Heading(normalize_heading(ac.heading + 30.0));
                ac.target_speed = speed;
            }
            Leg::Join => {
                // Aim at the downwind ahead, never back at a midfield point already passed.
                let target = f.point((a - JOIN_LEAD_NM).min(f.length / 2.0), DOWNWIND_NM);
                ac.lateral = steer(target - ac.pos, ac);
                ac.target_altitude = pattern_alt;
                ac.target_speed = speed;
            }
            Leg::Upwind => {
                ac.lateral = steer(f.along - f.out * on_line(c), ac);
                ac.target_altitude = pattern_alt;
                ac.target_speed = speed;
            }
            Leg::Crosswind => {
                ac.lateral = steer(f.out, ac);
                ac.target_altitude = pattern_alt;
            }
            Leg::Downwind => {
                ac.lateral = steer(-f.along - f.out * on_line(c - DOWNWIND_NM), ac);
                ac.target_altitude = pattern_alt;
                ac.target_speed = speed;
            }
            Leg::Base => {
                ac.lateral = steer(-f.out, ac);
                let glide = self.airport.glideslope_altitude(vfr.end, f.threshold.distance(ac.pos));
                ac.target_altitude = glide.min(pattern_alt);
                ac.target_speed = (ac.ty.vref + 15.0).min(speed);
            }
            Leg::Final => {
                ac.lateral = Lateral::Localizer;
                ac.approach_runway = Some(vfr.end);
                ac.on_glideslope = true;
                ac.target_speed = ac.ty.vref + 5.0;
            }
            Leg::Departing => {
                ac.lateral = steer(heading_vec(vfr.outbound), ac);
                ac.target_altitude = elevation + DEPARTURE_AGL;
                ac.target_speed = ac.ty.max_speed;
            }
        }
        let id = ac.id;
        let leg = vfr.leg;
        ac.vfr = Some(vfr);
        for line in lines {
            self.say(Speaker::Pilot, line);
        }
        if matches!(leg, Leg::Upwind | Leg::Crosswind | Leg::Downwind | Leg::Base | Leg::Final) {
            self.keep_spacing(index);
        }
        // Outside the pattern, pilots give way to everyone.
        if matches!(leg, Leg::Entry | Leg::Holding | Leg::Join | Leg::Departing) {
            self.see_and_avoid(index);
        }

        match leg {
            Leg::Final => self.step_vfr_final(index),
            Leg::Departing => return self.step_vfr_departure(index, id),
            _ => {}
        }
        false
    }

    /// Turn away from, and stop climbing or descending into, traffic about to
    /// pass too close.
    fn see_and_avoid(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let threat = self
            .aircraft
            .iter()
            .filter(|b| b.id != ac.id && !b.on_ground && (b.altitude - ac.altitude).abs() < AVOID_FT)
            .filter_map(|b| {
                let r = b.pos - ac.pos;
                let d = r.length();
                if !(1e-6..AVOID_NM).contains(&d) {
                    return None;
                }
                let v = (b.ground_velocity - ac.ground_velocity) / SECONDS_PER_HOUR;
                let t = (-r.dot(v) / v.length_squared().max(1e-12)).clamp(0.0, AVOID_LOOKAHEAD_S);
                let dz = (b.altitude - ac.altitude) + (b.vertical_speed - ac.vertical_speed) / 60.0 * t;
                ((r + v * t).length() < AVOID_CPA_NM && dz.abs() < AVOID_CPA_FT).then_some((d, r, b.altitude))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0));
        let Some((_, r, other_altitude)) = threat else { return };
        // Away from the traffic, and to the right if it's head on.
        let own = heading_vec(ac.heading);
        let dir = own - r.normalize() * 1.2 + heading_vec(ac.heading + 90.0) * 0.8;
        let air = self.wind.vector_at(ac.altitude);
        let ac = &mut self.aircraft[index];
        ac.lateral = Lateral::Heading(heading_for_track(vec_heading(dir), ac.tas(), air));
        let toward = (other_altitude - ac.altitude) * (ac.target_altitude - ac.altitude);
        if toward > 0.0 {
            ac.target_altitude = ac.altitude;
        }
    }

    /// Seconds until an airborne aircraft needs this physical runway clear,
    /// if it's on its way to land there: on final or base, an IFR arrival
    /// headed for it, or a VFR aircraft past midfield downwind. Pilots check the
    /// runway short of the threshold, so that's where this counts to.
    pub(super) fn time_to_land(&self, ac: &Aircraft, physical: usize) -> Option<f32> {
        let speed = ac.ground_speed().max(40.0) / SECONDS_PER_HOUR;
        let check = if ac.vfr.is_some() { LANDING_CHECK_NM } else { super::tower::RUNWAY_CHECK_NM };
        if let Some(d) = self.landing_distance(ac, physical) {
            let base = ac.vfr.as_ref().is_some_and(|v| v.leg == Leg::Base);
            let d = if base { d * 1.3 } else { d };
            return Some((d - check).max(0.0) / speed);
        }
        // An IFR arrival still joining the localizer: straight to the threshold,
        // which no real path is shorter than.
        if ac.vfr.is_none() && !ac.on_ground && ac.flow == super::aircraft::Flow::Arrival {
            let end = ac.approach_runway?;
            return (self.surface().runway_of_end[end] == physical)
                .then(|| (self.airport.runways[end].threshold().distance(ac.pos) - check).max(0.0) / speed);
        }
        let vfr = ac.vfr.as_ref()?;
        let downwind = !ac.on_ground && ac.phase == Phase::Pattern && vfr.leg == Leg::Downwind;
        if !downwind || self.surface().runway_of_end[vfr.end] != physical {
            return None;
        }
        let f = Frame::of(&self.airport.runways[vfr.end]);
        let a = f.a(ac.pos);
        if a > f.length / 2.0 {
            return None;
        }
        // Down to the base turn, across, and in on final.
        let path = (a + BASE_NM).max(0.0) + DOWNWIND_NM + BASE_NM.max(-a);
        Some((path - LANDING_CHECK_NM).max(0.0) / speed)
    }

    /// Something will land on this runway within `seconds`: IFR traffic on
    /// final, or VFR traffic on base, final or late downwind. With
    /// `held_extended`, traffic Tower is holding on an extended downwind
    /// doesn't count: it won't turn in until Tower says so.
    pub(super) fn landing_within(&self, physical: usize, seconds: f32, held_extended: bool) -> bool {
        self.aircraft.iter().any(|a| {
            // Not if Tower's just told it to turn in and the pilot hasn't yet.
            let held = held_extended
                && !self.call_pending(a.id)
                && a.vfr.as_ref().is_some_and(|v| v.extend && !v.turn_base && v.leg == Leg::Downwind);
            !held && self.time_to_land(a, physical).is_some_and(|t| t < seconds)
        })
    }

    /// Pilots slow down rather than overtake traffic ahead on the same leg.
    fn keep_spacing(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let Some(leg) = ac.vfr.as_ref().map(|v| v.leg) else { return };
        let dir = heading_vec(ac.heading);
        let slower = self
            .aircraft
            .iter()
            .filter(|b| b.id != ac.id && !b.on_ground && b.vfr.as_ref().is_some_and(|v| v.leg == leg))
            .filter_map(|b| {
                let rel = b.pos - ac.pos;
                let d = rel.length();
                (d < CATCH_UP_NM && d > 1e-6 && rel.dot(dir) / d > 0.7)
                    .then_some(if d < MIN_TRAIL_NM { b.ias - OPEN_GAP_KT } else { b.ias })
            })
            .reduce(f32::min);
        if let Some(lead_speed) = slower {
            let ac = &mut self.aircraft[index];
            ac.target_speed = ac.target_speed.min(lead_speed.max(ac.ty.min_speed));
        }
    }

    fn step_vfr_final(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        let vfr = ac.vfr.as_ref().expect("VFR");
        let end = vfr.end;
        let to_go = self.airport.runways[end].along(ac.pos);
        if to_go < LANDING_CHECK_NM {
            if vfr.landing.is_none() {
                self.go_around(index, GoAroundReason::NoLandingClearance);
                return;
            }
            let physical = self.surface().runway_of_end[end];
            // A departure already rolling ahead will be airborne by the time it lands.
            let id = ac.id;
            let occupied = self.aircraft.iter().any(|b| b.id != id && !b.rolling() && self.occupies(b, physical));
            if occupied {
                self.go_around(index, GoAroundReason::RunwayOccupied);
                return;
            }
        }
        if to_go > 0.0 {
            return;
        }
        if vfr.full_stop() {
            let ac = &mut self.aircraft[index];
            ac.flow = Flow::Arrival;
            self.touch_down(index);
        } else {
            self.touch_and_go(index);
        }
    }

    /// Down on the runway and straight back up again.
    fn touch_and_go(&mut self, index: usize) {
        let elevation = self.airport.elevation;
        let ac = &mut self.aircraft[index];
        let mut vfr = ac.vfr.take().expect("VFR");
        let heading = self.airport.runways[vfr.end].heading;
        ac.on_ground = true;
        ac.altitude = elevation;
        ac.vertical_speed = 0.0;
        ac.on_glideslope = false;
        ac.approach_runway = None;
        ac.phase = Phase::TowerRelease;
        ac.departure_end = Some(vfr.end);
        ac.heading = heading;
        ac.lateral = Lateral::Heading(heading);
        ac.target_altitude = elevation + PATTERN_AGL;
        ac.target_speed = pattern_speed(ac.ty);
        vfr.circuits = vfr.circuits.saturating_sub(1);
        vfr.reset_circuit();
        ac.vfr = Some(vfr);
        let callsign = ac.callsign.clone();
        self.emit(SimEvent::TouchAndGo { callsign });
    }

    /// Climbing away: gone once it's clear of the field.
    fn step_vfr_departure(&mut self, index: usize, id: u32) -> bool {
        let distance = self.aircraft[index].pos.length();
        let released = self.aircraft[index].vfr.as_ref().is_some_and(|v| v.released);
        if !released && distance > RELEASE_NM {
            if let Some(v) = self.aircraft[index].vfr.as_mut() {
                v.released = true;
            }
            if self.config.staffing.local {
                let callsign = self.aircraft[index].callsign.clone();
                self.emit(SimEvent::NoFrequencyChange { callsign });
            }
        }
        if distance > EXIT_NM {
            if let Some(i) = self.aircraft.iter().position(|a| a.id == id) {
                let ac = self.aircraft.remove(i);
                self.emit(SimEvent::Exited { callsign: ac.callsign });
            }
            return true;
        }
        false
    }

    /// Off the runway after a takeoff or touch-and-go: into the pattern.
    pub(super) fn vfr_airborne(&mut self, index: usize) {
        let ac = &mut self.aircraft[index];
        if let Some(v) = ac.vfr.as_mut() {
            v.leg = Leg::Upwind;
            ac.phase = Phase::Pattern;
            ac.freq_since = self.time;
        }
    }

    /// Instructions the automated Tower gives a VFR aircraft in the air.
    pub(super) fn local_vfr_decision(&self, ac: &Aircraft, vfr: &Vfr) -> Option<super::clearance::Clearance> {
        use super::clearance::Clearance;
        match vfr.leg {
            Leg::Entry | Leg::Holding if !vfr.cleared_pattern => Some(Clearance::EnterDownwind(vfr.end)),
            Leg::Upwind | Leg::Departing
                if vfr.intent == VfrIntent::Departure && !vfr.released && ac.pos.length() > AUTO_RELEASE_NM =>
            {
                Some(Clearance::ContactDeparture)
            }
            // Departures get priority: downwind traffic extends to make the gap,
            // and turns in again, one at a time, once the departure is airborne.
            Leg::Downwind if !ac.on_ground => {
                let physical = self.surface().runway_of_end[vfr.end];
                match self.departure_window(physical) {
                    // Only once it's due a release: no point holding everyone
                    // out while the departure waits on its interval anyway.
                    Some(window) if !vfr.extend && !vfr.turn_base && self.time >= self.next_departure_time => self
                        .time_to_land(ac, physical)
                        .is_some_and(|t| t < window + EXTEND_MARGIN_S)
                        .then_some(Clearance::ExtendDownwind),
                    None if vfr.extend && !self.departure_under_way(physical) => {
                        self.first_off_extension(ac, vfr, physical).then_some(Clearance::TurnBase)
                    }
                    _ => None,
                }
            }
            Leg::Base | Leg::Final if vfr.landing.is_none() => {
                let physical = self.surface().runway_of_end[vfr.end];
                let mine = self.landing_distance(ac, physical)?;
                let number_one = !self
                    .aircraft
                    .iter()
                    .any(|b| b.id != ac.id && self.landing_distance(b, physical).is_some_and(|d| d < mine));
                // A departure already rolling will be gone by the time it gets
                // there, and so will an arrival rolling out ahead: the pilot still
                // checks the runway is clear before landing.
                let blocked = self.aircraft.iter().any(|b| {
                    b.id != ac.id && !b.rolling() && b.phase != Phase::Rollout && self.occupies(b, physical)
                });
                (number_one && !blocked).then_some(if vfr.intent == VfrIntent::Closed {
                    Clearance::ClearedOption(vfr.end)
                } else {
                    Clearance::ClearedToLand(vfr.end)
                })
            }
            _ => None,
        }
    }

    /// This extended aircraft is the furthest out of those extending, and
    /// turning base now puts it a safe gap behind anyone already on base or final.
    fn first_off_extension(&self, ac: &Aircraft, vfr: &Vfr, physical: usize) -> bool {
        let map = self.surface();
        let f = Frame::of(&self.airport.runways[vfr.end]);
        let a = f.a(ac.pos);
        let room = -a - FOLLOW_GAP_NM;
        !self.aircraft.iter().filter(|b| b.id != ac.id).any(|b| {
            let ahead_extending = !b.on_ground
                && b.vfr.as_ref().is_some_and(|v| {
                    v.extend && v.leg == Leg::Downwind && map.runway_of_end[v.end] == physical && f.a(b.pos) < a
                });
            ahead_extending || self.landing_distance(b, physical).is_some_and(|d| d > room)
        })
    }

    /// VFR traffic on upwind from this runway, which a faster departure would run into.
    pub(super) fn upwind_traffic(&self, physical: usize) -> bool {
        let map = self.surface();
        self.aircraft.iter().any(|a| {
            !a.on_ground
                && a.phase == Phase::Pattern
                && a.vfr.as_ref().is_some_and(|v| v.leg == Leg::Upwind && map.runway_of_end[v.end] == physical)
        })
    }

    /// The pilot's first call to Ground, from a parking spot.
    pub(super) fn vfr_ready_call(&self, ac: &Aircraft, spot: &str) -> String {
        let request = match ac.vfr.as_ref().map(|v| (v.intent, v.outbound)) {
            Some((VfrIntent::Closed, _)) => "closed traffic".to_string(),
            Some((_, bearing)) => format!("departure to the {}", compass(bearing)),
            None => String::new(),
        };
        format!("Ground, {}, at {spot}, VFR {request}, ready to taxi.", ac.callsign)
    }
}

/// What a pilot does with a landing clearance.
pub(super) fn landing_for(clearance_kind: Landing, vfr: &Vfr) -> Result<Landing, String> {
    match clearance_kind {
        Landing::TouchAndGo if vfr.intent != VfrIntent::Closed => Err("we need a full stop".into()),
        other => Ok(other),
    }
}
