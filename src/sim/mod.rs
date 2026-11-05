//! Headless simulation. Nothing in here may depend on macroquad.

pub mod aircraft;
pub mod airspace;
pub mod autopilot;
pub mod clearance;
pub mod conflict;
pub mod performance;
pub mod scoring;
pub mod tower;
pub mod traffic;
pub mod units;

use aircraft::{Aircraft, Flow, Phase};
use airspace::{Airport, Config};
use clearance::RadioCall;
use glam::Vec2;
use scoring::{Score, ShiftEnd, SimEvent};
use std::collections::{BTreeSet, VecDeque};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Controller,
    Pilot,
    System,
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
}

impl Alerts {
    fn pair_has(pairs: &BTreeSet<(u32, u32)>, id: u32) -> bool {
        pairs.iter().any(|&(a, b)| a == id || b == id)
    }

    /// Red: separation already lost or below terrain.
    pub fn critical(&self, id: u32) -> bool {
        Self::pair_has(&self.separation, id) || self.terrain.contains(&id)
    }

    /// Orange: predicted conflict or wake spacing.
    pub fn warning(&self, id: u32) -> bool {
        Self::pair_has(&self.conflict, id) || Self::pair_has(&self.wake, id)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorldConfig {
    pub seed: u64,
    /// Generate traffic. Tests turn this off and place their own aircraft.
    pub traffic: bool,
    /// Fixed wind, or `None` to roll one from the seed.
    pub wind: Option<Wind>,
    /// End the shift after this many sim-seconds.
    pub shift_seconds: f32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        WorldConfig { seed: 1, traffic: true, wind: None, shift_seconds: traffic::SHIFT_SECONDS }
    }
}

pub struct World {
    pub airport: Airport,
    pub wind: Wind,
    pub time: f32,
    pub aircraft: Vec<Aircraft>,
    pub radar: Vec<RadarTrack>,
    pub log: Vec<LogEntry>,
    pub score: Score,
    pub alerts: Alerts,
    pub ended: Option<ShiftEnd>,

    config: WorldConfig,
    config_index: usize,
    rng: fastrand::Rng,
    events: Vec<SimEvent>,
    radio: Vec<RadioCall>,
    id_counter: u32,
    next_sweep: f32,
    next_spawn_time: f32,
    next_departure_time: f32,
    departure_queue: VecDeque<Aircraft>,
    runway_occupied_until: Vec<f32>,
}

impl World {
    pub fn new(airport: Airport, config: WorldConfig) -> World {
        let mut rng = fastrand::Rng::with_seed(config.seed);
        let wind = config.wind.unwrap_or_else(|| Wind {
            from: rng.f32() * 360.0,
            speed: 5.0 + rng.f32() * 20.0,
        });
        let config_index = best_config(&airport, wind);
        let runway_occupied_until = vec![0.0; airport.runways.len()];
        let mut world = World {
            airport,
            wind,
            time: 0.0,
            aircraft: Vec::new(),
            radar: Vec::new(),
            log: Vec::new(),
            score: Score::default(),
            alerts: Alerts::default(),
            ended: None,
            config,
            config_index,
            rng,
            events: Vec::new(),
            radio: Vec::new(),
            id_counter: 0,
            next_sweep: 0.0,
            next_spawn_time: 0.0,
            next_departure_time: 0.0,
            departure_queue: VecDeque::new(),
            runway_occupied_until,
        };
        let opening = format!(
            "Shift start. Wind {:03.0} at {:.0}. Landing {}, departing {}.",
            world.wind.from,
            world.wind.speed,
            world.config().arrival,
            world.config().departure,
        );
        world.say(Speaker::System, opening);
        world
    }

    pub fn config(&self) -> &Config {
        &self.airport.configs[self.config_index]
    }

    pub fn arrival_runway(&self) -> usize {
        self.airport.runway_index(&self.config().arrival).expect("validated at load")
    }

    pub fn departure_runway(&self) -> usize {
        self.airport.runway_index(&self.config().departure).expect("validated at load")
    }

    pub fn shift_seconds(&self) -> f32 {
        self.config.shift_seconds
    }

    pub fn departures_waiting(&self) -> usize {
        self.departure_queue.len()
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

        self.run_radio();
        if self.config.traffic {
            self.run_traffic();
        }
        self.run_departure_release();

        let wind = self.wind;
        for ac in &mut self.aircraft {
            ac.step(TICK, wind, &self.airport);
        }

        self.run_approaches();
        self.check_sector();
        self.clear_finished_rollouts();
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
                .map(|f| (self.airport.fixes[f].pos() - ac.pos).length() <= EXIT_TOLERANCE_NM)
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
            "{:.3} {:?} {:?} {} {:?} {:?}",
            self.time,
            self.aircraft,
            self.departure_queue,
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
