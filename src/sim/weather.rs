//! Wind that changes through a shift, the ATIS that reports it, and the
//! runway changes it forces.

use super::aircraft::{Flow, Lateral, Leg, Phase, VfrIntent};
use super::units::{Wind, heading_diff, heading_vec, normalize_heading};
use super::{Speaker, World, best_config};

/// The target wind wanders this often, seconds.
const DRIFT_EVERY_S: f32 = 60.0;
/// Most the target wind wanders each time, degrees and knots.
const DRIFT_DEG: f32 = 8.0;
const DRIFT_KT: f32 = 2.0;
const MIN_KT: f32 = 3.0;
const MAX_KT: f32 = 25.0;
/// How fast the wind follows its target, degrees and knots a second.
const DRIFT_DEG_PER_S: f32 = 0.5;
const FRONT_DEG_PER_S: f32 = 1.0;
const KT_PER_S: f32 = 0.1;
/// Share of shifts that see a front go through.
const FRONT_SHARE: f32 = 0.6;
/// When in the shift a front arrives, seconds.
const FRONT_FROM_S: f32 = 8.0 * 60.0;
const FRONT_TO_S: f32 = 18.0 * 60.0;
/// How far a front veers the wind, degrees, and how strong it leaves it, knots.
const FRONT_TURN_DEG: (f32, f32) = (140.0, 220.0);
const FRONT_KT: (f32, f32) = (8.0, 20.0);
/// A new ATIS goes out when the wind moves this far from the last one.
const ATIS_DEG: f32 = 30.0;
/// Direction changes in lighter wind than this don't count.
const ATIS_MIN_KT: f32 = 5.0;
const ATIS_KT: f32 = 5.0;
/// Wind changes alone don't bring a new ATIS more often than this, seconds.
const ATIS_MIN_INTERVAL_S: f32 = 120.0;
/// More tailwind than this on the landing runway forces a change, knots.
pub const TAILWIND_LIMIT_KT: f32 = 5.0;
/// A runway change is announced this long before it happens, seconds.
pub const CHANGE_NOTICE_S: f32 = 240.0;
/// No second runway change within this long of the last, seconds.
const CHANGE_REST_S: f32 = 600.0;

const PHONETIC: [&str; 26] = [
    "Alfa", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliett", "Kilo",
    "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango", "Uniform",
    "Victor", "Whiskey", "X-ray", "Yankee", "Zulu",
];

/// A runway change that's been announced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunwayChange {
    /// Sim time it takes effect.
    pub at: f32,
    /// The runway configuration it changes to.
    pub config: usize,
}

#[derive(Debug, Clone, Copy)]
enum FrontTo {
    /// Veer this many degrees.
    Veer(f32),
    /// Swing round the short way to blow from here.
    From(f32),
}

#[derive(Debug, Clone, Copy)]
struct Front {
    at: f32,
    to: FrontTo,
    speed: f32,
}

#[derive(Debug, Clone)]
pub struct Weather {
    rng: fastrand::Rng,
    /// The wind changes at all. Off when a test fixes the wind.
    changing: bool,
    /// Wind direction, unwrapped so a front can veer through north.
    dir: f32,
    target_dir: f32,
    target_speed: f32,
    turn_rate: f32,
    next_drift: f32,
    front: Option<Front>,
    /// Current ATIS letter, 0 for Alfa.
    letter: usize,
    /// The wind the current ATIS reports.
    broadcast: Wind,
    broadcast_at: f32,
    pub pending: Option<RunwayChange>,
    last_change: f32,
}

impl Weather {
    pub fn new(seed: u64, wind: Wind, changing: bool) -> Weather {
        let mut rng = fastrand::Rng::with_seed(seed ^ 0x5745_4154_4845_5221);
        let letter = rng.usize(0..PHONETIC.len());
        let front = (changing && rng.f32() < FRONT_SHARE).then(|| Front {
            at: FRONT_FROM_S + rng.f32() * (FRONT_TO_S - FRONT_FROM_S),
            to: FrontTo::Veer(FRONT_TURN_DEG.0 + rng.f32() * (FRONT_TURN_DEG.1 - FRONT_TURN_DEG.0)),
            speed: FRONT_KT.0 + rng.f32() * (FRONT_KT.1 - FRONT_KT.0),
        });
        Weather {
            rng,
            changing,
            dir: wind.from,
            target_dir: wind.from,
            target_speed: wind.speed,
            turn_rate: DRIFT_DEG_PER_S,
            next_drift: DRIFT_EVERY_S,
            front,
            letter,
            broadcast: wind,
            broadcast_at: 0.0,
            pending: None,
            last_change: f32::NEG_INFINITY,
        }
    }

    /// The current ATIS letter, spoken.
    pub fn information(&self) -> &'static str {
        PHONETIC[self.letter]
    }

    /// The current ATIS letter.
    pub fn letter(&self) -> char {
        (b'A' + self.letter as u8) as char
    }

    /// Whether a front is due later in the shift.
    pub fn front_due(&self) -> bool {
        self.front.is_some()
    }
}

impl World {
    pub(super) fn run_weather(&mut self) {
        self.step_wind();
        self.check_atis();
        self.check_runway_change();
    }

    /// Force a front: the wind swings round to `wind` from `at`. Also lets the
    /// wind change in a shift that started with a fixed one.
    pub fn schedule_wind_shift(&mut self, at: f32, wind: Wind) {
        let w = &mut self.weather;
        w.changing = true;
        w.front = Some(Front { at, to: FrontTo::From(wind.from), speed: wind.speed });
        // A test that fixes the wind wants only this change, not drift too.
        w.next_drift = f32::INFINITY;
    }

    /// "Information Bravo. Wind 270 at 12. Landing 27L, departing 27R."
    pub fn atis_text(&self) -> String {
        let mut text = format!(
            "Information {}. Wind {:03.0} at {:.0}. {}.",
            self.weather.information(),
            self.airport.to_magnetic(self.wind.from),
            self.wind.speed,
            self.runways_in_use()
        );
        if let Some(closed) = self.closed_runway_name() {
            text += &format!(" Runway {closed} closed.");
        }
        text
    }

    /// The announced runway change, if one is coming: seconds until it
    /// happens, and the landing and departure runways it changes to.
    pub fn runway_change_notice(&self) -> Option<(f32, String, String)> {
        let change = self.weather.pending?;
        let config = &self.airport.configs[change.config];
        // As `departure_runway`: IFR departures share the arrival runway when VFR has its own.
        let shared = self.vfr_enabled() && self.airport.vfr_runway == super::airspace::VfrRunway::Own;
        let departure = if shared { &config.arrival } else { &config.departure };
        Some(((change.at - self.time).max(0.0), config.arrival.clone(), departure.clone()))
    }

    /// "RWY CHG 3:12 ARR 09R DEP 09L", for the status lines.
    pub fn runway_change_line(&self) -> Option<String> {
        let (seconds, arrival, departure) = self.runway_change_notice()?;
        let s = seconds.ceil() as i32;
        Some(format!("RWY CHG {}:{:02} ARR {arrival} DEP {departure}", s / 60, s % 60))
    }

    fn step_wind(&mut self) {
        let time = self.time;
        let w = &mut self.weather;
        if !w.changing {
            return;
        }
        if let Some(front) = w.front.filter(|f| time >= f.at) {
            w.front = None;
            w.target_dir = match front.to {
                FrontTo::Veer(degrees) => w.dir + degrees,
                FrontTo::From(from) => w.dir + heading_diff(normalize_heading(w.dir), from),
            };
            w.target_speed = front.speed;
            w.turn_rate = FRONT_DEG_PER_S;
        }
        if time >= w.next_drift {
            w.next_drift += DRIFT_EVERY_S;
            // Not while a front is still swinging the wind round.
            if (w.target_dir - w.dir).abs() < 1.0 {
                w.target_dir += (w.rng.f32() * 2.0 - 1.0) * DRIFT_DEG;
                w.target_speed = (w.target_speed + (w.rng.f32() * 2.0 - 1.0) * DRIFT_KT).clamp(MIN_KT, MAX_KT);
                w.turn_rate = DRIFT_DEG_PER_S;
            }
        }
        let step = w.turn_rate * super::TICK;
        w.dir += (w.target_dir - w.dir).clamp(-step, step);
        let speed_step = KT_PER_S * super::TICK;
        let speed = self.wind.speed + (w.target_speed - self.wind.speed).clamp(-speed_step, speed_step);
        self.wind = Wind { from: normalize_heading(w.dir), speed };
    }

    fn check_atis(&mut self) {
        if self.time - self.weather.broadcast_at < ATIS_MIN_INTERVAL_S {
            return;
        }
        let (now, then) = (self.wind, self.weather.broadcast);
        let veered = heading_diff(then.from, now.from).abs() >= ATIS_DEG && now.speed >= ATIS_MIN_KT;
        if veered || (now.speed - then.speed).abs() >= ATIS_KT {
            self.new_atis();
        }
    }

    /// Put out the next ATIS letter with the current wind and runways.
    pub(super) fn new_atis(&mut self) {
        let w = &mut self.weather;
        w.letter = (w.letter + 1) % PHONETIC.len();
        w.broadcast = self.wind;
        w.broadcast_at = self.time;
        let text = self.atis_text();
        self.say(Speaker::System, text);
    }

    /// Tailwind on the landing runway in use, knots. Negative is a headwind.
    pub fn tailwind(&self) -> f32 {
        let runway = &self.airport.runways[self.arrival_runway()];
        -heading_vec(runway.heading).dot(heading_vec(self.wind.from)) * self.wind.speed
    }

    fn check_runway_change(&mut self) {
        if let Some(change) = self.weather.pending {
            if self.time >= change.at {
                self.weather.pending = None;
                self.change_runways(change.config);
            }
            return;
        }
        let w = &self.weather;
        let rested = self.time - w.last_change >= CHANGE_REST_S;
        let room = self.time + CHANGE_NOTICE_S <= self.config.shift_seconds;
        if !w.changing || !rested || !room || self.tailwind() <= TAILWIND_LIMIT_KT {
            return;
        }
        let best = best_config(&self.airport, self.wind);
        if best == self.config_index {
            return;
        }
        self.weather.pending = Some(RunwayChange { at: self.time + CHANGE_NOTICE_S, config: best });
        let config = &self.airport.configs[best];
        let line = format!(
            "Runway change in {:.0} minutes: landing {}, departing {}.",
            CHANGE_NOTICE_S / 60.0,
            config.arrival,
            config.departure
        );
        self.say(Speaker::System, line);
        self.new_atis();
    }

    /// Switch to another runway configuration now, and move the traffic that
    /// can still move over to it.
    pub fn change_runways(&mut self, config: usize) {
        let old_pattern = self.pattern_runway();
        self.config_index = config;
        self.weather.last_change = self.time;
        self.say(Speaker::System, format!("Runway change complete. {}.", self.runways_in_use()));
        if self.vfr_enabled() && old_pattern != self.pattern_runway() {
            let line = format!(
                "Tower: attention all aircraft, runway change, VFR traffic pattern now runway {}.",
                self.airport.runways[self.pattern_runway()].id
            );
            self.say(Speaker::System, line);
        }
        self.reassign_departures();
        self.reroute_arrivals();
        self.new_atis();
    }

    /// Departures that haven't been cleared onto a runway move to the new
    /// one. Pilots get a new SID leaving by the same exit where there is one.
    pub(super) fn reassign_departures(&mut self) {
        let surface = self.config.staffing.surface_traffic();
        let mut queue = std::mem::take(&mut self.departure_queue);
        for ac in &mut queue {
            if ac.vfr.is_none() {
                let (route, _) = self.sid_for_exit(ac.exit_fix);
                ac.exit_fix = route.last().copied();
                ac.route = route;
            }
        }
        let mut requeue = Vec::new();
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            let movable = ac.flow == Flow::Departure
                && ac.on_ground
                && matches!(ac.phase, Phase::Parked | Phase::Taxiing)
                && !ac.cleared_takeoff;
            if !movable {
                index += 1;
                continue;
            }
            let end = match &ac.vfr {
                Some(_) => self.pattern_runway(),
                None => self.departure_runway_for(ac.ty),
            };
            if ac.departure_end == Some(end) {
                index += 1;
                continue;
            }
            // With nobody working the surface, departures wait out of sight at
            // the hold line: just bring them back at the new one.
            if !surface {
                requeue.push(self.aircraft.remove(index));
                continue;
            }
            let exit = ac.exit_fix;
            let sid = ac.vfr.is_none().then(|| self.sid_for_exit(exit));
            let ac = &mut self.aircraft[index];
            ac.departure_end = Some(end);
            if let Some(vfr) = &mut ac.vfr {
                vfr.end = end;
            }
            if let Some((route, _)) = sid {
                ac.exit_fix = route.last().copied();
                ac.route = route;
            }
            index += 1;
        }
        for mut ac in requeue.into_iter().rev() {
            let (route, _) = self.sid_for_exit(ac.exit_fix);
            ac.exit_fix = route.last().copied();
            ac.route = route;
            ac.taxi = None;
            ac.departure_end = None;
            queue.push_front(ac);
        }
        self.departure_queue = queue;
    }

    /// Arrivals still heading for their STAR's first fix switch to the new
    /// configuration's STAR from that fix. The rest need vectors.
    fn reroute_arrivals(&mut self) {
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let on_star = ac.flow == Flow::Arrival
                && !ac.on_ground
                && ac.vfr.is_none()
                && ac.approach_runway.is_none()
                && ac.lateral == Lateral::Route
                && ac.route_index == 0;
            let Some(&gate) = ac.route.first().filter(|_| on_star) else { continue };
            if let Some((route, final_heading)) = self.star_from_gate(gate) {
                let ac = &mut self.aircraft[index];
                ac.route = route;
                ac.route_end_heading = final_heading;
            }
        }
    }

    /// After a runway change: an aircraft still coming in to land the old
    /// way, against the new flow. Nothing departs until it's down.
    pub(super) fn landing_against_flow(&self) -> bool {
        let flow = self.airport.runways[self.arrival_runway()].heading;
        self.aircraft.iter().any(|a| {
            let vfr_landing = a.vfr.as_ref().filter(|v| matches!(v.leg, Leg::Base | Leg::Final)).map(|v| v.end);
            let end = if a.vfr.is_some() { vfr_landing } else { a.approach_runway };
            !a.on_ground && end.is_some_and(|e| heading_diff(self.airport.runways[e].heading, flow).abs() > 90.0)
        })
    }

    /// A VFR aircraft flying the pattern for a runway end no longer in use
    /// moves to the new one: re-entering if it's in the circuit, or making a
    /// full stop if it's already on base or final.
    pub(super) fn follow_runway_change(&mut self, index: usize) {
        let pattern = self.pattern_runway();
        let ac = &self.aircraft[index];
        let Some(vfr) = ac.vfr.as_ref().filter(|v| v.end != pattern && v.intent != VfrIntent::Departure) else {
            return;
        };
        let leg = vfr.leg;
        let callsign = ac.callsign.clone();
        let local = self.config.staffing.local;
        let runway = self.airport.runways[pattern].id.clone();
        let vfr = self.aircraft[index].vfr.as_mut().expect("checked");
        match leg {
            Leg::Entry | Leg::Holding | Leg::Join => {
                vfr.end = pattern;
                vfr.leg = if leg == Leg::Join { Leg::Entry } else { leg };
            }
            Leg::Upwind | Leg::Crosswind | Leg::Downwind => {
                vfr.reset_circuit();
                vfr.leg = Leg::Entry;
                vfr.end = pattern;
                vfr.cleared_pattern = true;
                if local {
                    self.say(Speaker::Pilot, format!("{callsign}, re-entering the downwind for runway {runway}."));
                }
            }
            // Committed to the old runway: land there and stop.
            Leg::Base | Leg::Final => {
                vfr.circuits = 0;
                if vfr.landing == Some(super::aircraft::Landing::TouchAndGo) {
                    vfr.landing = Some(super::aircraft::Landing::FullStop);
                }
            }
            Leg::Departing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::airspace::Airport;
    use crate::sim::{TICK, WorldConfig};

    fn world(wind: Option<Wind>) -> World {
        World::new(Airport::default_airport(), WorldConfig { seed: 5, traffic: false, wind, ..Default::default() })
    }

    fn run(world: &mut World, seconds: f32) {
        let end = world.time + seconds;
        while world.time < end && world.ended.is_none() {
            world.tick();
        }
    }

    #[test]
    fn a_fixed_wind_never_changes() {
        let wind = Wind { from: 270.0, speed: 10.0 };
        let mut w = world(Some(wind));
        run(&mut w, 1500.0);
        assert_eq!(w.wind, wind);
    }

    #[test]
    fn drift_wanders_within_limits() {
        for seed in 0..20 {
            let config = WorldConfig { seed, traffic: false, ..Default::default() };
            let mut w = World::new(Airport::default_airport(), config);
            let start = w.wind;
            w.weather.front = None;
            run(&mut w, 600.0);
            assert!((MIN_KT.min(start.speed)..=MAX_KT.max(start.speed)).contains(&w.wind.speed), "{:?}", w.wind);
            // Ten minutes of drift moves it 80 degrees at the very most.
            assert!(heading_diff(start.from, w.wind.from).abs() <= 10.0 * DRIFT_DEG + 1.0, "{start:?} {:?}", w.wind);
        }
    }

    #[test]
    fn the_atis_letter_moves_on_for_a_big_change_but_not_a_small_one() {
        let mut w = world(Some(Wind { from: 270.0, speed: 10.0 }));
        let first = w.weather.letter();
        run(&mut w, ATIS_MIN_INTERVAL_S);
        w.wind.from = 280.0;
        run(&mut w, TICK);
        assert_eq!(w.weather.letter(), first);
        w.wind.from = 310.0;
        run(&mut w, TICK);
        assert_ne!(w.weather.letter(), first);
        assert!(w.log.last().unwrap().text.starts_with("Information "), "{:?}", w.log.last());
    }

    #[test]
    fn a_tailwind_brings_a_runway_change_four_minutes_later_and_no_second_one_soon() {
        let mut w = world(Some(Wind { from: 270.0, speed: 10.0 }));
        assert_eq!(w.config().arrival, "27L");
        w.schedule_wind_shift(30.0, Wind { from: 90.0, speed: 12.0 });
        run(&mut w, 600.0);
        assert_eq!(w.config().arrival, "09R");
        let announced = w.log.iter().find(|l| l.text.starts_with("Runway change in")).expect("announced").time;
        let done = w.log.iter().find(|l| l.text.starts_with("Runway change complete")).expect("done").time;
        assert!((done - announced - CHANGE_NOTICE_S).abs() <= TICK, "{announced} {done}");

        // Straight back again: it has to wait out the rest period.
        w.schedule_wind_shift(w.time, Wind { from: 270.0, speed: 12.0 });
        run(&mut w, 300.0);
        assert_eq!(w.config().arrival, "09R");
        assert!(w.runway_change_notice().is_none());
        run(&mut w, CHANGE_REST_S);
        assert_eq!(w.config().arrival, "27L");
    }

    #[test]
    fn no_change_is_announced_too_late_in_the_shift() {
        let mut w = world(Some(Wind { from: 270.0, speed: 10.0 }));
        let late = w.config.shift_seconds - CHANGE_NOTICE_S + 30.0;
        w.schedule_wind_shift(late, Wind { from: 90.0, speed: 15.0 });
        let shift = w.config.shift_seconds;
        run(&mut w, shift);
        assert_eq!(w.config().arrival, "27L");
        assert!(!w.log.iter().any(|l| l.text.starts_with("Runway change")));
    }
}
