//! Headless simulation. Nothing in here may depend on macroquad.

pub mod aircraft;
pub mod approach;
pub mod airspace;
pub mod autopilot;
pub mod awaiting;
pub mod clearance;
pub mod conflict;
pub mod coordination;
pub mod delivery;
pub mod emergency;
pub mod ground;
pub mod holding;
pub mod mistakes;
pub mod performance;
pub mod positions;
pub mod ramp;
pub mod scoring;
pub mod sky;
pub mod storms;
pub mod visibility;
pub mod tower;
pub mod ground_map;
pub mod registration;
pub mod replay;
pub mod traffic;
pub mod tutorial;
pub mod units;
pub mod vfr;
pub mod weather;

use aircraft::{Aircraft, Flow, Phase};
use airspace::{Airport, Config};
use clearance::RadioCall;
use glam::Vec2;
use scoring::{Credit, Score, ShiftEnd, SimEvent};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use units::{Wind, heading_vec, vec_heading};

/// Fixed simulation step, seconds.
pub const TICK: f32 = 0.25;
/// Time for one radar antenna rotation, seconds.
pub const RADAR_SWEEP: f32 = 4.8;
const HISTORY_DOTS: usize = 6;
const MAX_LOG: usize = 200;
/// A departure has to leave within this distance of its exit fix.
const EXIT_TOLERANCE_NM: f32 = 12.0;
const EXIT_ALTITUDE_TOLERANCE: f32 = 500.0;
/// The first VFR aircraft turns up this far into a shift, seconds.
const FIRST_VFR_S: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Controller,
    Pilot,
    System,
    /// Mayday and pan-pan calls, and radio-failure notices.
    Emergency,
    /// Controllers talking to each other on the landline.
    Landline,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: f32,
    pub speaker: Speaker,
    pub text: String,
}

/// What the radar saw on the last sweep. The scope only ever draws these.
#[derive(Debug, Clone)]
pub struct RadarTrack {
    pub id: u32,
    pub pos: Vec2,
    pub altitude: f32,
    pub ground_speed: f32,
    pub velocity: Vec2,
    pub vertical_speed: f32,
    /// Previous returns, newest first.
    pub history: VecDeque<Vec2>,
}

#[derive(Debug, Clone, Default)]
pub struct Alerts {
    pub separation: BTreeSet<(u32, u32)>,
    pub conflict: BTreeSet<(u32, u32)>,
    pub wake: BTreeSet<(u32, u32)>,
    pub terrain: BTreeSet<u32>,
    /// Aircraft too close together on the ground.
    pub ground: BTreeSet<(u32, u32)>,
    /// Taxiing aircraft nose to nose, each waiting for the other.
    pub deadlock: BTreeSet<(u32, u32)>,
}

impl Alerts {
    fn pair_has(pairs: &BTreeSet<(u32, u32)>, id: u32) -> bool {
        pairs.iter().any(|&(a, b)| a == id || b == id)
    }

    /// Red: separation already lost or below terrain.
    pub fn critical(&self, id: u32) -> bool {
        Self::pair_has(&self.separation, id)
            || self.terrain.contains(&id)
            || Self::pair_has(&self.ground, id)
    }

    /// Orange: predicted conflict or wake spacing.
    pub fn warning(&self, id: u32) -> bool {
        Self::pair_has(&self.conflict, id)
            || Self::pair_has(&self.wake, id)
            || Self::pair_has(&self.deadlock, id)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorldConfig {
    pub seed: u64,
    /// Generate traffic. Tests turn this off and place their own aircraft.
    pub traffic: bool,
    /// Fixed wind, or `None` to roll one from the seed and let it change
    /// through the shift.
    pub wind: Option<Wind>,
    /// End the shift after this many sim-seconds.
    pub shift_seconds: f32,
    /// Scales the airport's traffic rate.
    pub busyness: traffic::Busyness,
    /// The positions the player works. The rest are automated.
    pub staffing: positions::Staffing,
    /// VFR traffic, on tower positions only.
    pub vfr: bool,
    /// How often emergencies happen.
    pub emergencies: emergency::EmergencyRate,
    /// How often pilots slip up.
    pub mistakes: mistakes::PilotMistakes,
    /// Ceiling, visibility and storms.
    pub weather: weather::WeatherKind,
    /// UTC at the start of the shift, seconds (only the time of day counts):
    /// for clock times said on the radio.
    pub clock_start: f64,
}

impl Default for WorldConfig {
    fn default() -> Self {
        WorldConfig {
            seed: 1,
            traffic: true,
            wind: None,
            shift_seconds: traffic::SHIFT_SECONDS,
            busyness: traffic::Busyness::default(),
            staffing: positions::Staffing::default(),
            vfr: false,
            emergencies: emergency::EmergencyRate::Off,
            mistakes: mistakes::PilotMistakes::Off,
            weather: weather::WeatherKind::Good,
            clock_start: 0.0,
        }
    }
}

#[derive(Clone)]
pub struct World {
    /// Shared, so a snapshot of the world doesn't copy the map.
    pub airport: Arc<Airport>,
    pub wind: Wind,
    pub time: f32,
    pub aircraft: Vec<Aircraft>,
    pub radar: Vec<RadarTrack>,
    pub log: Vec<LogEntry>,
    /// Lines ever said, including ones the log has since dropped: how the UI
    /// spots new transmissions.
    pub said: u64,
    pub score: Score,
    pub alerts: Alerts,
    pub ended: Option<ShiftEnd>,
    /// Wind changes, the ATIS and runway changes.
    pub weather: weather::Weather,
    /// Thunderstorm cells, if the weather has any.
    pub storms: storms::Storms,
    /// Emergency state: closed runways, aircraft being brought back.
    pub emergencies: emergency::Emergencies,
    /// An ILS failure this shift, if there is one.
    pub ils_outage: Option<approach::IlsOutage>,
    /// Open landline coordination between controllers.
    pub coordination: Vec<coordination::Coordination>,
    /// Whether departures need Approach's release.
    pub releases: coordination::Releases,

    config: WorldConfig,
    config_index: usize,
    rng: fastrand::Rng,
    events: Vec<SimEvent>,
    radio: Vec<RadioCall>,
    id_counter: u32,
    next_sweep: f32,
    next_spawn_time: f32,
    next_departure_time: f32,
    /// Recent takeoff rolls, for wake turbulence intervals.
    takeoffs: Vec<tower::Takeoff>,
    next_vfr_time: f32,
    departure_queue: VecDeque<Aircraft>,
    /// Ticks run so far.
    ticks: u32,
    /// What the player has said, in order: a replay is the seed and these.
    commands: Vec<replay::Command>,
}

impl World {
    pub fn new(airport: Airport, config: WorldConfig) -> World {
        let mut rng = fastrand::Rng::with_seed(config.seed);
        let wind = config.wind.unwrap_or_else(|| Wind {
            from: rng.f32() * 360.0,
            speed: 5.0 + rng.f32() * 20.0,
        });
        let config_index = best_config(&airport, wind);
        let weather = weather::Weather::new(config.seed, wind, config.wind.is_none(), config.shift_seconds, config.weather);
        let storms = weather.storms_for(&airport, config.seed, config.shift_seconds, config.weather);
        let mut world = World {
            storms,
            airport: Arc::new(airport),
            wind,
            time: 0.0,
            aircraft: Vec::new(),
            radar: Vec::new(),
            log: Vec::new(),
            said: 0,
            score: Score::new(Credit::for_staffing(config.staffing)),
            alerts: Alerts::default(),
            ended: None,
            weather,
            emergencies: emergency::Emergencies::new(config.seed),
            ils_outage: approach::IlsOutage::roll(config.seed, config.shift_seconds, config.weather),
            coordination: Vec::new(),
            releases: coordination::Releases::default(),
            config,
            config_index,
            rng,
            events: Vec::new(),
            radio: Vec::new(),
            id_counter: 0,
            next_sweep: 0.0,
            next_spawn_time: 0.0,
            next_departure_time: 0.0,
            takeoffs: Vec::new(),
            next_vfr_time: FIRST_VFR_S,
            departure_queue: VecDeque::new(),
            ticks: 0,
            commands: Vec::new(),
        };
        let opening = format!("Shift start. {}", world.atis_text());
        world.say(Speaker::System, opening);
        world
    }

    /// Runways in use: arrival, IFR departure, and the VFR runway if VFR is on.
    pub fn runway_ids(&self) -> (&str, &str, Option<&str>) {
        let id = |end: usize| self.airport.runways[end].id.as_str();
        let vfr = self.vfr_enabled().then(|| id(self.pattern_runway()));
        (id(self.arrival_runway()), id(self.departure_runway()), vfr)
    }

    /// "Landing 26L, departing 26L, VFR 26R", for the shift opening.
    pub fn runways_in_use(&self) -> String {
        let (arrival, departure, vfr) = self.runway_ids();
        let mut text = format!("Landing {arrival}, departing {departure}");
        if let Some(vfr) = vfr {
            text += &format!(", VFR {vfr}");
        }
        text
    }

    /// Runway ends in use now (landing, departing, VFR and the long-runway
    /// departure end), so a physical runway is named by the end that's active.
    pub fn ends_in_use(&self) -> Vec<usize> {
        let mut ends = vec![self.arrival_runway(), self.departure_runway()];
        if self.vfr_enabled() {
            ends.push(self.pattern_runway());
        }
        if let Some(id) = &self.config().departure_long {
            ends.extend(self.airport.runway_index(id).map(|e| self.avoiding_closure(e)));
        }
        ends
    }

    pub fn config(&self) -> &Config {
        &self.airport.configs[self.config_index]
    }

    /// The landing runway: the configuration's, or its parallel while it's closed.
    pub fn arrival_runway(&self) -> usize {
        let end = self.airport.runway_index(&self.config().arrival).expect("validated at load");
        self.avoiding_closure(end)
    }

    /// The runway IFR departures use. With VFR traffic on at an airport that
    /// gives VFR its own runway, the configured departure runway goes to VFR,
    /// and IFR departures share the arrival runway instead.
    pub fn departure_runway(&self) -> usize {
        if self.vfr_enabled() && self.airport.vfr_runway == airspace::VfrRunway::Own {
            self.arrival_runway()
        } else {
            self.configured_departure_runway()
        }
    }

    pub(crate) fn configured_departure_runway(&self) -> usize {
        let end = self.airport.runway_index(&self.config().departure).expect("validated at load");
        self.avoiding_closure(end)
    }

    /// Runway for a departure: the normal one unless the aircraft needs a longer one.
    pub fn departure_runway_for(&self, ty: &performance::AircraftType) -> usize {
        let normal = self.departure_runway();
        match &self.config().departure_long {
            Some(id) if self.airport.runways[normal].takeoff_length_ft() < ty.min_runway_ft => {
                self.avoiding_closure(self.airport.runway_index(id).expect("validated at load"))
            }
            _ => normal,
        }
    }

    /// HHMM UTC at a sim time, as said on the radio and written on strips.
    pub fn utc(&self, time: f32) -> String {
        let utc = (self.config.clock_start + time as f64).rem_euclid(86_400.0) as u64;
        format!("{:02}{:02}", utc / 3600, utc / 60 % 60)
    }

    pub fn clock_start(&self) -> f64 {
        self.config.clock_start
    }

    /// Set the UTC clock, seconds into the day at sim time zero.
    pub fn set_clock_start(&mut self, utc: f64) {
        self.config.clock_start = utc;
    }

    pub fn shift_seconds(&self) -> f32 {
        self.config.shift_seconds
    }

    pub fn staffing(&self) -> positions::Staffing {
        self.config.staffing
    }

    /// Departures that haven't started their takeoff roll.
    pub fn departures_waiting(&self) -> usize {
        let on_ground = self
            .aircraft
            .iter()
            .filter(|a| a.flow == Flow::Departure && a.on_ground && !a.rolling())
            .count();
        self.departure_queue.len() + on_ground
    }

    pub fn find(&self, id: u32) -> Option<&Aircraft> {
        self.aircraft.iter().find(|a| a.id == id)
    }

    pub fn find_mut(&mut self, id: u32) -> Option<&mut Aircraft> {
        self.aircraft.iter_mut().find(|a| a.id == id)
    }

    pub fn track(&self, id: u32) -> Option<&RadarTrack> {
        self.radar.iter().find(|t| t.id == id)
    }

    fn next_id(&mut self) -> u32 {
        self.id_counter += 1;
        self.id_counter
    }

    /// Add an aircraft directly. Used by traffic generation and test scenarios.
    pub fn add_aircraft(&mut self, ac: Aircraft) {
        self.id_counter = self.id_counter.max(ac.id);
        self.aircraft.push(ac);
    }

    /// Events since the last call, for sounds and test assertions.
    pub fn drain_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.events)
    }

    fn say(&mut self, speaker: Speaker, text: String) {
        self.log.push(LogEntry { time: self.time, speaker, text });
        self.said += 1;
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn emit(&mut self, event: SimEvent) {
        self.score.apply(self.time, &event);
        self.events.push(event);
    }

    fn end_shift(&mut self, end: ShiftEnd) {
        if self.ended.is_none() {
            self.ended = Some(end);
            self.emit(SimEvent::ShiftEnded { end });
        }
    }

    /// Advance the world one fixed step. Does nothing once the shift has ended.
    pub fn tick(&mut self) {
        if self.ended.is_some() {
            return;
        }
        self.time += TICK;
        self.ticks += 1;

        self.run_weather();
        self.run_ils_outage();
        self.run_visibility();

        self.run_radio();
        if self.config.traffic {
            self.run_traffic();
            self.run_vfr_traffic();
        }
        self.place_departures();

        let wind = self.wind;
        for ac in &mut self.aircraft {
            ac.step(TICK, wind, &self.airport);
        }

        self.run_surface();
        self.run_approaches();
        self.run_emergencies();
        self.run_pattern();
        self.run_coordination();
        self.run_local();
        self.run_ground();
        self.run_automated_approach();
        self.run_holding();
        self.run_delivery();
        self.run_transponders();
        self.run_slips();
        self.run_storms();
        self.check_handoffs();
        self.check_sector();
        self.run_safety_net();
        self.run_sweep();

        if self.time >= self.config.shift_seconds {
            self.end_shift(ShiftEnd::Completed);
        }
    }

    /// Track entries to and exits from the sector.
    fn check_sector(&mut self) {
        let mut index = 0;
        while index < self.aircraft.len() {
            let ac = &self.aircraft[index];
            if ac.on_ground {
                index += 1;
                continue;
            }
            let inside = self.airport.in_sector(ac.pos);
            if inside && !ac.entered_sector {
                let late = ac.phase == Phase::Offered;
                let callsign = ac.callsign.clone();
                let ac = &mut self.aircraft[index];
                ac.entered_sector = true;
                if late {
                    ac.phase = Phase::Controlled;
                    self.emit(SimEvent::LateAccept { callsign });
                }
            } else if !inside && ac.entered_sector {
                let ac = self.aircraft.remove(index);
                self.leave_sector(&ac);
                continue;
            }
            index += 1;
        }
    }

    fn leave_sector(&mut self, ac: &Aircraft) {
        let callsign = ac.callsign.clone();
        if ac.flow == Flow::Departure && ac.phase == Phase::OfferedToCenter {
            let near_exit = ac
                .exit_fix
                .map(|f| self.distance_from_exit(ac, f) <= EXIT_TOLERANCE_NM)
                .unwrap_or(true);
            if !near_exit {
                self.emit(SimEvent::WrongExit { callsign: callsign.clone() });
            }
            if ac.altitude < ac.exit_altitude - EXIT_ALTITUDE_TOLERANCE {
                self.emit(SimEvent::LowExit { callsign: callsign.clone() });
            }
            self.emit(SimEvent::Exited { callsign });
        } else {
            self.emit(SimEvent::SectorBust { callsign });
        }
    }

    /// How far off its exit an aircraft left. An exit fix beyond the boundary can't be
    /// reached, so there it's the distance from the route's last leg.
    fn distance_from_exit(&self, ac: &Aircraft, exit: usize) -> f32 {
        let fix = self.airport.fixes[exit].pos();
        if self.airport.in_sector(fix) {
            return (fix - ac.pos).length();
        }
        let start = ac
            .route
            .iter()
            .rev()
            .nth(1)
            .map(|&f| self.airport.fixes[f].pos())
            .unwrap_or(Vec2::ZERO);
        let leg = fix - start;
        let t = ((ac.pos - start).dot(leg) / leg.length_squared().max(1e-6)).clamp(0.0, 1.0);
        (start + leg * t - ac.pos).length()
    }

    fn run_sweep(&mut self) {
        if self.time < self.next_sweep {
            return;
        }
        self.next_sweep += RADAR_SWEEP;
        // Radar doesn't paint aircraft on the ground. Keep history for the rest.
        let previous = std::mem::take(&mut self.radar);
        for ac in self.aircraft.iter().filter(|a| !a.on_ground) {
            let mut history = previous
                .iter()
                .find(|t| t.id == ac.id)
                .map(|t| {
                    let mut h = t.history.clone();
                    h.push_front(t.pos);
                    h.truncate(HISTORY_DOTS);
                    h
                })
                .unwrap_or_default();
            history.truncate(HISTORY_DOTS);
            self.radar.push(RadarTrack {
                id: ac.id,
                pos: ac.pos,
                altitude: ac.altitude,
                ground_speed: ac.ground_speed(),
                velocity: ac.ground_velocity,
                vertical_speed: ac.vertical_speed,
                history,
            });
        }
    }

    /// Everything that should be identical for two runs with the same seed and commands.
    pub fn state_digest(&self) -> String {
        format!(
            "{:.3} {:?} {:?} {:?} {:?} {} {:?} {:?}",
            self.time,
            self.aircraft,
            self.departure_queue,
            self.coordination,
            self.releases,
            self.score.points,
            self.score.incidents,
            self.log.iter().map(|l| &l.text).collect::<Vec<_>>(),
        )
    }
}

/// The configuration with the strongest headwind on its landing runway.
fn best_config(airport: &Airport, wind: Wind) -> usize {
    let into_wind = heading_vec(wind.from);
    let headwind = |c: &Config| {
        let rwy = airport.runway_index(&c.arrival).expect("validated at load");
        heading_vec(airport.runways[rwy].heading).dot(into_wind)
    };
    let mut best = 0;
    for (index, config) in airport.configs.iter().enumerate() {
        if headwind(config) > headwind(&airport.configs[best]) {
            best = index;
        }
    }
    best
}

/// Track over the ground, from a radar return.
pub fn track_heading(track: &RadarTrack) -> f32 {
    vec_heading(track.velocity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_selects_the_runway_direction() {
        let airport = Airport::default_airport();
        let config = |from| WorldConfig {
            wind: Some(Wind { from, speed: 10.0 }),
            traffic: false,
            ..Default::default()
        };
        let world = World::new(airport.clone(), config(270.0));
        assert_eq!(world.config().arrival, "27L");
        let world = World::new(airport, config(80.0));
        assert_eq!(world.config().arrival, "09R");
    }
}
