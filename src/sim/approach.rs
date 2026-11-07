//! Approaches besides the ILS: RNAV (GPS) approaches from the published
//! fixes, and visual approaches, alone or following traffic. Which runways
//! offer which, the weather each needs, and ILS outages.

use super::aircraft::{Aircraft, Flow, Lateral, Phase};
use super::airspace::Airport;
use super::scoring::GoAroundReason;
use super::units::heading_diff;
use super::weather::WeatherKind;
use super::{Speaker, World};

/// Runway ends within this many degrees of the landing runway's heading land
/// the same way: the parallel, as well as the runway itself.
const SAME_FLOW_DEG: f32 = 30.0;
/// RNAV minimums, simplified from LPV and LNAV: ceiling feet above the field,
/// and visibility in statute miles.
pub const RNAV_CEILING_FT: f32 = 300.0;
pub const RNAV_VISIBILITY_SM: f32 = 1.0;
/// A visual approach needs this visibility, SM...
pub const VISUAL_VISIBILITY_SM: f32 = 3.0;
/// ...and a ceiling this far above the minimum vectoring altitude, feet.
pub const VISUAL_MVA_MARGIN_FT: f32 = 500.0;
/// Pilots pick out the field from no further than this, NM.
const VISUAL_RANGE_NM: f32 = 12.0;
/// Or traffic to follow, NM.
const TRAFFIC_RANGE_NM: f32 = 8.0;
/// Closer than this to the threshold, a visual approach is too tight, NM.
pub const VISUAL_MIN_NM: f32 = 3.0;
/// The least a visual follower keeps behind its leader when no wake minimum
/// applies, NM.
pub const VISUAL_FOLLOW_MIN_NM: f32 = 2.0;
/// A visual follower slows when it's within this much of its minimum, NM.
const FOLLOW_SLOW_MARGIN_NM: f32 = 1.0;
/// And slows to this much below the leader's ground speed, knots.
const FOLLOW_SLOW_KT: f32 = 10.0;
/// Visibility reported for a clear day, SM: the pilot sees as far as it matters.
const CLEAR_SM: f32 = 10.0;
/// Statute miles to nautical miles.
const SM_TO_NM: f32 = 0.869;
/// Parallel finals closer than this are one final for wake and spacing, feet.
pub const SAME_LANE_FT: f32 = 2500.0;
/// Parallel finals at least this far apart are independent: no separation
/// is needed between aircraft established on each, feet.
pub const INDEPENDENT_FT: f32 = 4300.0;
/// Between those: dependent finals, with this diagonal between aircraft
/// established on each, NM.
pub const DEPENDENT_NM: f32 = 1.5;
/// Share of shifts with Variable or Low IFR weather that see the ILS fail.
const OUTAGE_SHARE: f32 = 0.3;
/// When it fails and for how long, in a 30-minute shift, seconds.
const OUTAGE_FROM_S: (f32, f32) = (10.0 * 60.0, 40.0 * 60.0);
const OUTAGE_LENGTH_S: (f32, f32) = (20.0 * 60.0, 40.0 * 60.0);
const OUTAGE_SHIFT_S: f32 = 30.0 * 60.0;
/// Keeps the outage roll apart from everything else drawn from the seed.
const OUTAGE_SEED: u64 = 0x494c_535f_4f55_5421;

/// The kind of approach an arrival is cleared for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApproachKind {
    #[default]
    Ils,
    Rnav,
    Visual,
}

impl ApproachKind {
    pub const ALL: [ApproachKind; 3] = [ApproachKind::Ils, ApproachKind::Rnav, ApproachKind::Visual];

    /// "ILS", "RNAV", "visual".
    pub fn name(self) -> &'static str {
        match self {
            ApproachKind::Ils => "ILS",
            ApproachKind::Rnav => "RNAV",
            ApproachKind::Visual => "visual",
        }
    }

    /// The scratchpad letter: I26L, R26R, V26L.
    pub fn letter(self) -> char {
        match self {
            ApproachKind::Ils => 'I',
            ApproachKind::Rnav => 'R',
            ApproachKind::Visual => 'V',
        }
    }

    /// As the strip writes it.
    pub fn strip(self) -> &'static str {
        match self {
            ApproachKind::Ils => "ILS",
            ApproachKind::Rnav => "RNAV",
            ApproachKind::Visual => "VIS",
        }
    }
}

/// An ILS out of service for part of the shift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IlsOutage {
    pub from: f32,
    pub until: f32,
    /// The runway end whose ILS is out, once it has failed.
    pub end: Option<usize>,
}

impl IlsOutage {
    /// Roll whether this shift's ILS fails, and when.
    pub fn roll(seed: u64, shift: f32, kind: WeatherKind) -> Option<IlsOutage> {
        if !matches!(kind, WeatherKind::Variable | WeatherKind::LowIfr) {
            return None;
        }
        let mut rng = fastrand::Rng::with_seed(seed ^ OUTAGE_SEED);
        if rng.f32() >= OUTAGE_SHARE {
            return None;
        }
        let scale = shift / OUTAGE_SHIFT_S;
        let from = (OUTAGE_FROM_S.0 + rng.f32() * (OUTAGE_FROM_S.1 - OUTAGE_FROM_S.0)) * scale;
        let length = (OUTAGE_LENGTH_S.0 + rng.f32() * (OUTAGE_LENGTH_S.1 - OUTAGE_LENGTH_S.0)) * scale;
        Some(IlsOutage { from, until: from + length, end: None })
    }
}

/// What a pilot can tell about the approaches on offer, worked out before
/// the clearance is read back.
#[derive(Debug, Clone, Default)]
pub(super) struct ApproachFacts {
    /// Every approach a landing runway offers now.
    pub options: Vec<(ApproachKind, usize)>,
    /// The runway end whose ILS is out, if one is.
    pub ils_out: Option<usize>,
    /// Below this aircraft's minimums, for each kind in `ApproachKind::ALL` order.
    pub below: [bool; 3],
    pub field_in_sight: bool,
    /// Traffic it's asked to follow: id, the runway it's landing, and in sight.
    pub leaders: Vec<(u32, Option<usize>, bool)>,
}

impl ApproachFacts {
    pub fn offers(&self, kind: ApproachKind, end: usize) -> bool {
        self.options.contains(&(kind, end))
    }

    pub fn below(&self, kind: ApproachKind) -> bool {
        self.below[kind as usize]
    }

    /// Why this runway can't be given for this kind of approach, if it can't.
    pub fn refusal(&self, kind: ApproachKind, end: usize, airport: &Airport) -> Option<String> {
        if self.offers(kind, end) {
            return None;
        }
        if !self.options.iter().any(|&(_, e)| e == end) {
            return Some("that runway isn't landing".into());
        }
        Some(match kind {
            ApproachKind::Ils if self.ils_out == Some(end) => "the ILS is out of service".into(),
            ApproachKind::Ils => format!("no ILS to runway {}", airport.runways[end].id),
            ApproachKind::Rnav => format!("no RNAV approach to runway {}", airport.runways[end].id),
            ApproachKind::Visual => "not in visual conditions".into(),
        })
    }
}

impl Airport {
    /// The vertical path an approach follows: the RNAV approach's glidepath,
    /// or the runway's glideslope.
    pub fn approach_altitude(&self, end: usize, along: f32, kind: ApproachKind) -> f32 {
        let deg = match kind {
            ApproachKind::Rnav => self.rnav(end).map(|a| a.glidepath_deg),
            _ => None,
        };
        self.glidepath_altitude(end, along, deg.unwrap_or(self.runways[end].glideslope_deg))
    }

    /// Are these two runway ends' finals one final, for spacing? The same
    /// end, or a parallel too close to fly side by side.
    pub fn same_lane(&self, a: usize, b: usize) -> bool {
        a == b || (self.parallel(a, b) && self.centreline_spacing_ft(a, b) < SAME_LANE_FT)
    }

    /// Landing the same way.
    pub(crate) fn parallel(&self, a: usize, b: usize) -> bool {
        heading_diff(self.runways[a].heading, self.runways[b].heading).abs() < SAME_FLOW_DEG
    }

    /// The diagonal needed between aircraft established on these two
    /// runways' finals, NM: `None` when they're independent, or not parallel.
    pub fn parallel_minimum(&self, a: usize, b: usize) -> Option<f32> {
        if !self.parallel(a, b) || self.same_lane(a, b) {
            return None;
        }
        (self.centreline_spacing_ft(a, b) < INDEPENDENT_FT).then_some(DEPENDENT_NM)
    }

    /// Where an approach's published fixes are, in order from `fix` to the
    /// last before the runway, if `fix` is the intermediate or final
    /// approach fix, or on a transition to them.
    pub fn rnav_route_from(&self, end: usize, fix: usize) -> Option<Vec<usize>> {
        let approach = self.rnav(end)?;
        let name = &self.fixes[fix].name;
        let index = |n: &String| self.fix_index(n).expect("validated at load");
        // The intermediate fix and the final approach fix, not step-downs inside it.
        if let Some(i) = approach.fixes.iter().take(2).position(|f| f == name) {
            return Some(approach.fixes[i..].iter().map(index).collect());
        }
        approach.transitions.iter().find_map(|t| {
            let i = t.iter().position(|f| f == name)?;
            Some(t[i..].iter().chain(&approach.fixes[1..]).map(index).collect())
        })
    }

    /// The at-or-above altitude for a fix on an approach: its own, or the
    /// intermediate fix's for a transition fix.
    pub fn rnav_altitude(&self, end: usize, fix: usize) -> Option<f32> {
        let approach = self.rnav(end)?;
        let name = &self.fixes[fix].name;
        match approach.fixes.iter().position(|f| f == name) {
            Some(i) => Some(approach.altitudes[i]),
            None => Some(approach.altitudes[0]),
        }
    }
}

impl World {
    /// Runway ends landing the same way as the landing runway, closed ones
    /// left out: the landing runway first.
    pub fn landing_runways(&self) -> Vec<usize> {
        let arrival = self.arrival_runway();
        let mut ends = vec![arrival];
        ends.extend((0..self.airport.runways.len()).filter(|&e| {
            e != arrival && self.airport.parallel(e, arrival) && !self.closed(e)
        }));
        ends
    }

    /// Is this runway end's ILS out of service?
    pub fn ils_out(&self, end: usize) -> bool {
        self.ils_outage.is_some_and(|o| o.end == Some(end))
    }

    /// Can an approach of this kind be flown to this runway end now? Visual
    /// approaches need visual conditions: whether this aircraft can see the
    /// field is up to the pilot.
    pub fn offers(&self, end: usize, kind: ApproachKind) -> bool {
        match kind {
            ApproachKind::Ils => self.airport.runways[end].ils && !self.ils_out(end),
            ApproachKind::Rnav => self.airport.rnav(end).is_some(),
            ApproachKind::Visual => self.visual_weather(),
        }
    }

    /// Every approach the landing runways offer: the landing runway's first,
    /// ILS, then RNAV, then visual.
    pub fn approach_options(&self) -> Vec<(ApproachKind, usize)> {
        self.landing_runways()
            .into_iter()
            .flat_map(|end| ApproachKind::ALL.into_iter().map(move |kind| (kind, end)))
            .filter(|&(kind, end)| self.offers(end, kind))
            .collect()
    }

    /// The approach to send an arrival onto a runway's final by: the ILS,
    /// the RNAV while it's out, or a visual if there's neither.
    pub fn default_approach(&self, end: usize) -> Option<ApproachKind> {
        ApproachKind::ALL.into_iter().find(|&k| self.offers(end, k))
    }

    /// Visibility good enough for visual approaches, and a ceiling at least
    /// VFR: whether a particular aircraft is clear of cloud is checked apart.
    pub fn visual_weather(&self) -> bool {
        !self.sky().imc()
    }

    /// The ceiling and visibility allow a visual approach from here.
    pub fn visual_conditions(&self, pos: glam::Vec2) -> bool {
        let sky = self.sky();
        let ceiling_ok = sky
            .ceiling
            .is_none_or(|c| self.airport.elevation + c >= self.airport.mva_at(pos) + VISUAL_MVA_MARGIN_FT);
        sky.visibility >= VISUAL_VISIBILITY_SM && ceiling_ok
    }

    /// How far a pilot can see, NM, capped at `cap`. Ten miles is reported
    /// for anything from there up.
    fn sight_nm(&self, cap: f32) -> f32 {
        let visibility = self.sky().visibility;
        if visibility >= CLEAR_SM { cap } else { (visibility * SM_TO_NM).min(cap) }
    }

    /// Below the cloud and close enough to see the airport.
    pub fn field_in_sight(&self, ac: &Aircraft) -> bool {
        let below_cloud = self.sky().ceiling.is_none_or(|c| ac.altitude < self.airport.elevation + c);
        below_cloud && ac.pos.length() <= self.sight_nm(VISUAL_RANGE_NM)
    }

    /// Can this pilot see the traffic it's asked to follow? An airborne
    /// arrival on an approach, ahead, and close enough.
    pub fn traffic_in_sight(&self, ac: &Aircraft, leader: &Aircraft) -> bool {
        let on_approach = matches!(
            leader.phase,
            Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff
        );
        let to = leader.pos - ac.pos;
        let ahead = heading_diff(ac.heading, super::units::vec_heading(to)).abs() <= 90.0;
        leader.id != ac.id
            && leader.flow == Flow::Arrival
            && !leader.on_ground
            && on_approach
            && ahead
            && to.length() <= self.sight_nm(TRAFFIC_RANGE_NM)
    }

    /// Is the weather below this aircraft's minimums for an approach?
    pub fn below_approach_minimums(&self, kind: ApproachKind, ac: &Aircraft) -> bool {
        let sky = self.sky();
        match kind {
            ApproachKind::Ils => sky.below_cat1() && !super::visibility::lands_below_cat1(ac),
            ApproachKind::Rnav => sky.below(RNAV_CEILING_FT, RNAV_VISIBILITY_SM) && !ac.in_emergency(),
            ApproachKind::Visual => !self.visual_conditions(ac.pos),
        }
    }

    /// Can this aircraft get in by any approach on offer now?
    pub(super) fn can_get_in(&self, ac: &Aircraft) -> bool {
        self.approach_options().iter().any(|&(kind, _)| !self.below_approach_minimums(kind, ac))
    }

    /// What the pilot of `index` knows about the approaches it might be
    /// cleared for in `clearances`.
    pub(super) fn approach_facts(
        &self,
        index: usize,
        clearances: &[(super::clearance::Clearance, Option<super::positions::Position>)],
    ) -> ApproachFacts {
        use super::clearance::Clearance;
        let ac = &self.aircraft[index];
        let leaders = clearances
            .iter()
            .filter_map(|(c, _)| match c {
                Clearance::ClearedVisual { follow: Some((id, _)), .. } => Some(*id),
                _ => None,
            })
            .map(|id| match self.find(id) {
                Some(leader) => (id, leader.approach_runway, self.traffic_in_sight(ac, leader)),
                None => (id, None, false),
            })
            .collect();
        ApproachFacts {
            options: self.approach_options(),
            ils_out: self.ils_outage.and_then(|o| o.end),
            below: ApproachKind::ALL.map(|k| self.below_approach_minimums(k, ac)),
            field_in_sight: self.field_in_sight(ac),
            leaders,
        }
    }

    /// Fail the landing runway's ILS from `from` until `until`: for tests
    /// and lessons.
    pub fn schedule_ils_outage(&mut self, from: f32, until: f32) {
        self.ils_outage = Some(IlsOutage { from, until, end: None });
    }

    /// "ILS 26L OTS" while an ILS is out.
    pub fn ils_line(&self) -> Option<String> {
        let end = self.ils_outage?.end?;
        Some(format!("ILS {} OTS", self.airport.runways[end].id))
    }

    /// The ILS fails, and comes back.
    pub(super) fn run_ils_outage(&mut self) {
        let Some(outage) = self.ils_outage else { return };
        match outage.end {
            None if self.time >= outage.from && self.time < outage.until => {
                let end = self.arrival_runway();
                if !self.airport.runways[end].ils {
                    return;
                }
                self.ils_outage = Some(IlsOutage { end: Some(end), ..outage });
                self.new_atis();
                self.lose_localizer(end);
            }
            Some(_) if self.time >= outage.until => {
                self.ils_outage = None;
                self.new_atis();
            }
            _ => {}
        }
    }

    /// Arrivals flying the ILS that's just failed: those still with
    /// Approach ask for vectors; those with Tower go around.
    fn lose_localizer(&mut self, end: usize) {
        let mut go_arounds = Vec::new();
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let flying = ac.approach == ApproachKind::Ils && ac.approach_runway == Some(end) && !ac.on_ground;
            if !flying {
                continue;
            }
            if ac.phase == Phase::TowerHandedOff {
                go_arounds.push(index);
                continue;
            }
            let players = self.is_players(ac);
            let ac = &mut self.aircraft[index];
            super::clearance::break_approach(ac);
            ac.lateral = Lateral::Heading(ac.heading.round());
            if players {
                let line = format!("{}, localizer's gone, request vectors.", ac.callsign);
                self.say(Speaker::Pilot, line);
            }
        }
        for index in go_arounds {
            self.go_around(index, GoAroundReason::IlsOutage);
        }
    }

    /// The minimum a visual follower keeps behind its leader: the wake
    /// minimum where one applies, otherwise a little room, NM.
    pub fn visual_minimum(&self, leader: &Aircraft, follower: &Aircraft) -> f32 {
        let wake = super::performance::wake_required_nm(leader.ty.wake, follower.ty.wake);
        if wake > super::conflict::SEPARATION_NM { wake } else { VISUAL_FOLLOW_MIN_NM }
    }

    /// Visual followers closing on their leader slow down to keep their room.
    pub(super) fn space_visual_followers(&mut self) {
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let Some(leader) = ac.visual_follow.and_then(|id| self.find(id)) else { continue };
            if ac.on_ground || leader.on_ground {
                continue;
            }
            let gap = ac.pos.distance(leader.pos);
            if gap >= self.visual_minimum(leader, ac) + FOLLOW_SLOW_MARGIN_NM {
                continue;
            }
            let slow = (leader.ground_speed() - FOLLOW_SLOW_KT).max(ac.ty.min_speed);
            let ac = &mut self.aircraft[index];
            ac.target_speed = ac.target_speed.min(slow);
        }
    }
}
