//! Low cloud and fog at the field. In IMC, VFR traffic stops; under
//! low-visibility procedures the ILS critical area is protected; and below
//! Category I minimums only Category III aircraft can land, while the rest
//! hold, go around at minimums, and in the end divert.

use super::aircraft::{Aircraft, Flow, Landing, Phase, VfrIntent};
use super::approach::ApproachKind;

use super::mistakes::SlipKind;
use super::performance::AircraftType;
use super::scoring::{GoAroundReason, SimEvent};
use super::weather::WeatherKind;
use super::{Speaker, World};

/// Under low-visibility procedures nothing may be on the arrival runway
/// while an arrival on the ILS is closer than this, NM.
pub const ILS_CRITICAL_NM: f32 = 5.0;
/// Closer in, something on the runway is a runway incursion, scored as one, NM.
const ILS_CRITICAL_INNER_NM: f32 = super::tower::INCURSION_FINAL_NM;
/// The automated Tower's release limit under low-visibility procedures: the
/// critical area plus the time it takes to line up and get rolling, NM.
pub(super) const LVP_RELEASE_NM: f32 = 6.5;
/// The automated Approach's gap between arrivals on final under
/// low-visibility procedures, NM, against 8 normally.
pub(super) const LVP_FINAL_CLEAR_NM: f32 = 9.5;
/// Decision altitude, 200 ft above the threshold on a 3° glidepath, is this
/// far from the runway, NM.
pub const DECISION_NM: f32 = 0.6;
/// An arrival that can't land diverts after this many go-arounds at minimums...
pub const DIVERT_GO_AROUNDS: u32 = 2;
/// ...or after this long waiting for the weather, seconds.
pub const DIVERT_AFTER_S: f32 = 20.0 * 60.0;

/// Can this aircraft land in fog below Category I minimums? Category III
/// aircraft autoland; one in an emergency lands whatever the weather.
pub fn lands_below_cat1(ac: &Aircraft) -> bool {
    ac.ty.cat3 || ac.in_emergency()
}

impl World {
    /// Can an arrival of this type land on the landing runway in the weather
    /// now, by the approach the automated Approach would give it?
    pub(super) fn can_land(&self, ty: &AircraftType) -> bool {
        let sky = self.weather.sky;
        match self.default_approach(self.arrival_runway()) {
            Some(ApproachKind::Ils) => ty.cat3 || !sky.below_cat1(),
            Some(ApproachKind::Rnav) => !sky.below(super::approach::RNAV_CEILING_FT, super::approach::RNAV_VISIBILITY_SM),
            Some(ApproachKind::Visual) => true,
            None => false,
        }
    }

    pub(super) fn run_visibility(&mut self) {
        if self.config.weather == WeatherKind::Good {
            return;
        }
        let sky = self.weather.sky;
        if sky.imc() {
            self.ground_vfr();
        }
        if sky.lvp() && self.config.staffing.local {
            self.protect_critical_area();
        }
        self.run_weather_diversions();
    }

    /// Cloud and visibility below VFR minimums: VFR departures that haven't
    /// reached the runway cancel, and pattern work ends with a full stop.
    fn ground_vfr(&mut self) {
        self.departure_queue.retain(|a| a.vfr.is_none());
        let local = self.config.staffing.local;
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            let Some(vfr) = &ac.vfr else {
                index += 1;
                continue;
            };
            let near_runway = ac.taxi.as_ref().is_some_and(|t| {
                let map = self.surface();
                !t.cleared.is_empty() || !map.nodes[t.node].zones.is_empty() || t.holding_short.is_some()
            });
            let cancels = ac.on_ground
                && ac.flow == Flow::Departure
                && matches!(ac.phase, Phase::Parked | Phase::Taxiing)
                && !near_runway
                && !self.at_departure_hold(ac);
            if cancels {
                let players = self.is_players(ac);
                let ac = self.aircraft.remove(index);
                if players {
                    let line = format!("{}, with the weather going down we'll cancel, back to the ramp.", ac.callsign);
                    self.pilot_say(Speaker::Pilot, ac.callsign.clone(), line);
                }
                continue;
            }
            let more_circuits = !ac.on_ground && vfr.intent == VfrIntent::Closed && vfr.circuits > 0;
            if more_circuits {
                let callsign = ac.callsign.clone();
                let vfr = self.aircraft[index].vfr.as_mut().expect("checked");
                vfr.circuits = 0;
                if vfr.landing == Some(Landing::TouchAndGo) {
                    vfr.landing = Some(Landing::FullStop);
                }
                if local {
                    self.pilot_say(Speaker::Pilot, callsign.clone(), format!("{callsign}, weather's coming down, we'll make this one a full stop."));
                }
            }
            index += 1;
        }
    }

    /// Low-visibility procedures, with the player in the tower: an aircraft
    /// the player let onto the runway while an arrival is inside the critical
    /// area bends the localizer, and the arrival goes around.
    fn protect_critical_area(&mut self) {
        let Some(runways) = self.airport.surface.as_ref().map(|m| m.runways.len()) else { return };
        let mut go_arounds = Vec::new();
        for physical in 0..runways {
            let blocked = self.aircraft.iter().any(|b| {
                let deviating = matches!(b.slip.as_ref().map(|s| &s.kind), Some(SlipKind::Incursion { .. }));
                let entered = match b.phase {
                    Phase::LinedUp => true,
                    Phase::Taxiing => b.taxi.as_ref().is_some_and(|t| t.cleared.contains(&physical)),
                    _ => false,
                };
                entered && !deviating && self.occupies(b, physical)
            });
            if !blocked {
                continue;
            }
            for (index, a) in self.aircraft.iter().enumerate() {
                let inside = a.vfr.is_none()
                    && self
                        .landing_distance(a, physical)
                        .is_some_and(|d| (ILS_CRITICAL_INNER_NM..ILS_CRITICAL_NM).contains(&d));
                if inside {
                    go_arounds.push(index);
                }
            }
        }
        for index in go_arounds {
            self.go_around(index, GoAroundReason::IlsInterference);
        }
    }

    /// At decision altitude in weather below the minimums for its approach:
    /// go around.
    pub(super) fn below_minimums(&self, index: usize) -> bool {
        let ac = &self.aircraft[index];
        self.config.weather != WeatherKind::Good && self.below_approach_minimums(ac.approach, ac)
    }

    pub(super) fn minimums_go_around(&mut self, index: usize) {
        self.aircraft[index].weather_go_arounds += 1;
        if self.is_players(&self.aircraft[index]) {
            let line = format!("{}, minimums, no runway in sight.", self.aircraft[index].callsign);
            self.pilot_say(Speaker::Pilot, self.aircraft[index].callsign.clone(), line);
        }
        self.go_around(index, GoAroundReason::Weather);
    }

    /// Arrivals that can't land in the weather give up in the end and go to
    /// their alternate.
    fn run_weather_diversions(&mut self) {
        let now = self.time;
        let mut index = 0;
        while index < self.aircraft.len() {
            let stuck = {
                let ac = &self.aircraft[index];
                ac.flow == Flow::Arrival && !ac.on_ground && ac.vfr.is_none() && !self.can_get_in(ac)
            };
            let ac = &mut self.aircraft[index];
            if !stuck {
                ac.minimums_since = None;
                index += 1;
                continue;
            }
            let since = *ac.minimums_since.get_or_insert(now);
            let divert = ac.weather_go_arounds >= DIVERT_GO_AROUNDS || now - since >= DIVERT_AFTER_S;
            // Not off the final until it has gone around.
            if !divert || ac.on_final() {
                index += 1;
                continue;
            }
            let players = self.is_players(&self.aircraft[index]);
            let ac = self.aircraft.remove(index);
            if players {
                self.pilot_say(Speaker::Pilot, ac.callsign.clone(), format!("{}, we'll divert to our alternate.", ac.callsign));
            }
            self.emit(SimEvent::Diverted { callsign: ac.callsign });
        }
    }
}
