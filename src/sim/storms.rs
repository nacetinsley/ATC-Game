//! Thunderstorms: cells that build, drift with the steering wind and decay,
//! and that pilots won't fly into. They keep clear of the airport and its
//! published holds, so the finals and the holds stay usable.

use super::aircraft::{Aircraft, Lateral, Phase};
use super::airspace::Airport;
use super::scoring::SimEvent;
use super::units::{format_heading, heading_vec, normalize_heading, vec_heading};
use super::{Speaker, World};
use glam::Vec2;

/// Pilots won't fly into precipitation this strong or worse (of 6).
pub const SEVERE_LEVEL: f32 = 3.0;
/// Going round, pilots want nothing worse than this along their way, and
/// stay off course until the way back is this clear too.
pub const CLEAR_LEVEL: f32 = 2.0;
/// Pilots look this far ahead along their track for weather, NM.
pub const LOOKAHEAD_NM: f32 = 10.0;
/// Cells keep at least this far from the airport, edge to field, NM: clear of
/// a 10-mile final.
const AIRPORT_KEEP_OUT_NM: f32 = 12.0;
/// And this far from a published holding fix, NM.
const HOLD_KEEP_OUT_NM: f32 = 6.0;
/// Cells form no further out than this from the field, NM.
const FORM_WITHIN_NM: f32 = 48.0;
/// A cell's radius, NM.
const RADIUS_NM: (f32, f32) = (3.0, 8.0);
/// How long a cell lives, from first echo to gone, seconds.
const LIFE_S: (f32, f32) = (30.0 * 60.0, 60.0 * 60.0);
/// The steering wind cells drift with, knots.
const STEER_KT: (f32, f32) = (10.0, 30.0);
/// A Storms shift keeps between these many cells going.
const SCATTERED: (usize, usize) = (3, 6);
/// A new cell forms this long after the last, seconds.
const FORM_EVERY_S: (f32, f32) = (2.0 * 60.0, 7.0 * 60.0);
/// A Variable shift's line of storms comes this far upwind of the field, NM,
/// with this many cells.
const LINE_UPWIND_NM: f32 = 32.0;
const LINE_CELLS: (usize, usize) = (3, 5);
/// A cell drifting within this of a keep-out circle dies away, NM...
const FADE_MARGIN_NM: f32 = 2.5;
/// ...over this long, seconds: less time than it takes to drift that far.
const FADE_S: f32 = 180.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub pos: Vec2,
    /// NM per second.
    vel: Vec2,
    pub radius: f32,
    /// Strongest it gets, 1 to 6.
    peak: f32,
    born: f32,
    life: f32,
    /// How strong it is now.
    pub strength: f32,
    /// Dying away early since this time, too close to the field or a hold.
    fade_from: Option<f32>,
}

impl Cell {
    /// A cell that sits still at full strength: for tests and lessons.
    pub fn steady(pos: Vec2, radius: f32, strength: f32) -> Cell {
        Cell { pos, vel: Vec2::ZERO, radius, peak: strength, born: -5.0e8, life: 1.0e9, strength, fade_from: None }
    }

    fn strength_at(&self, t: f32) -> f32 {
        let age = (t - self.born) / self.life;
        let cycle = if !(0.0..1.0).contains(&age) {
            0.0
        } else if age < 0.25 {
            age / 0.25
        } else if age < 0.7 {
            1.0
        } else {
            (1.0 - age) / 0.3
        };
        let fade = self.fade_from.map_or(1.0, |f| (1.0 - (t - f) / FADE_S).max(0.0));
        self.peak * cycle * fade
    }

    /// Precipitation level at `p`: strongest at the core, nothing at the edge.
    pub fn level_at(&self, p: Vec2) -> f32 {
        let d = p.distance(self.pos) / self.radius;
        if d >= 1.0 { 0.0 } else { self.strength * (1.0 - d * d) }
    }

    /// Radius of the part at `level` or worse, NM.
    pub fn radius_at(&self, level: f32) -> f32 {
        if self.strength <= level { 0.0 } else { self.radius * (1.0 - level / self.strength).sqrt() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Calm,
    /// Cells forming and dying all shift.
    Scattered { target: usize },
    /// One line of cells comes through at this time.
    Line { at: f32 },
}

#[derive(Debug, Clone)]
pub struct Storms {
    pub cells: Vec<Cell>,
    rng: fastrand::Rng,
    /// Where cells drift, NM per second.
    steer: Vec2,
    mode: Mode,
    next_form: f32,
    /// Circles cells stay out of: centre and radius, NM.
    keep_out: Vec<(Vec2, f32)>,
}

impl Storms {
    pub fn calm() -> Storms {
        Storms {
            cells: Vec::new(),
            rng: fastrand::Rng::with_seed(0),
            steer: Vec2::ZERO,
            mode: Mode::Calm,
            next_form: f32::INFINITY,
            keep_out: Vec::new(),
        }
    }

    /// `line` brings one line of storms through mid-shift; `scattered` keeps
    /// cells going all shift. Neither: calm.
    pub fn new(airport: &Airport, seed: u64, shift: f32, scattered: bool, line: bool) -> Storms {
        if !scattered && !line {
            return Storms::calm();
        }
        let mut rng = fastrand::Rng::with_seed(seed ^ 0x5354_4F52_4D53_0001);
        let toward = rng.f32() * 360.0;
        let speed = STEER_KT.0 + rng.f32() * (STEER_KT.1 - STEER_KT.0);
        let steer = heading_vec(toward) * speed / 3600.0;
        let mut keep_out = vec![(Vec2::ZERO, AIRPORT_KEEP_OUT_NM)];
        for hold in &airport.holds {
            if let Some(fix) = airport.fix_index(&hold.fix) {
                keep_out.push((airport.fixes[fix].pos(), HOLD_KEEP_OUT_NM));
            }
        }
        let mode = if scattered {
            Mode::Scattered { target: rng.usize(SCATTERED.0..=SCATTERED.1) }
        } else {
            Mode::Line { at: shift * (0.15 + rng.f32() * 0.25) }
        };
        let mut storms = Storms { cells: Vec::new(), rng, steer, mode, next_form: 0.0, keep_out };
        if let Mode::Scattered { target } = mode {
            // The shift starts with storms already about, at all ages.
            for _ in 0..target {
                let age = storms.rng.f32() * 0.6;
                storms.form(0.0, age, None);
            }
            storms.next_form = storms.between(FORM_EVERY_S);
        }
        storms.step(0.0, 0.0);
        storms
    }

    fn between(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + self.rng.f32() * (hi - lo)
    }

    /// Will a cell at `pos` of `radius` stay clear of every keep-out circle
    /// over its whole life?
    fn stays_clear(&self, pos: Vec2, radius: f32, life: f32) -> bool {
        let end = pos + self.steer * life;
        self.keep_out.iter().all(|&(c, r)| distance_to_segment(c, pos, end) > r + radius)
    }

    /// A new cell `age` (share of its life) old, somewhere it will stay clear
    /// of the field and holds; near `at` if given. False if there's no room.
    fn form(&mut self, now: f32, age: f32, at: Option<Vec2>) -> bool {
        for attempt in 0..30 {
            let radius = self.between(RADIUS_NM);
            let life = self.between(LIFE_S);
            let peak = 2.0 + self.rng.f32() * 4.0;
            let pos = match at {
                // Nudged along the line if its place is taken.
                Some(p) => p + heading_vec(self.rng.f32() * 360.0) * (attempt as f32 * 0.5).min(6.0),
                None => {
                    let bearing = self.rng.f32() * 360.0;
                    heading_vec(bearing) * FORM_WITHIN_NM * self.rng.f32().sqrt()
                }
            };
            let born = now - age * life;
            // A line comes through regardless, and the cells that would hit
            // the field die away before they get there.
            let clear = match at {
                Some(_) => self.stays_clear(pos, radius + FADE_MARGIN_NM, 0.0),
                None => self.stays_clear(pos, radius, life * (1.0 - age)),
            };
            if !clear {
                continue;
            }
            let vel = self.steer;
            self.cells.push(Cell { pos, vel, radius, peak, born, life, strength: 0.0, fade_from: None });
            return true;
        }
        false
    }

    /// The line of storms: cells side by side across the steering wind,
    /// upwind of the field, with a gap where one would hit it.
    fn form_line(&mut self, now: f32) {
        let along = self.steer.normalize_or_zero();
        let across = along.perp();
        let count = self.rng.usize(LINE_CELLS.0..=LINE_CELLS.1);
        let spacing = 9.0;
        let start = -along * LINE_UPWIND_NM - across * spacing * (count as f32 - 1.0) / 2.0;
        let offset = (self.rng.f32() - 0.5) * spacing;
        for i in 0..count {
            let at = start + across * (spacing * i as f32 + offset);
            self.form(now, 0.1, Some(at));
        }
    }

    /// Move the cells on by `dt` seconds to time `now`.
    pub fn step(&mut self, now: f32, dt: f32) {
        let keep_out = &self.keep_out;
        for cell in &mut self.cells {
            cell.pos += cell.vel * dt;
            let close = keep_out.iter().any(|&(c, r)| cell.pos.distance(c) - cell.radius < r + FADE_MARGIN_NM);
            if close && cell.fade_from.is_none() {
                cell.fade_from = Some(now);
            }
            cell.strength = cell.strength_at(now);
        }
        self.cells.retain(|c| now < c.born + c.life && c.fade_from.is_none_or(|f| now < f + FADE_S));
        match self.mode {
            Mode::Calm => {}
            Mode::Scattered { target } => {
                if self.cells.len() < target && now >= self.next_form {
                    self.form(now, 0.0, None);
                    self.next_form = now + self.between(FORM_EVERY_S);
                }
            }
            Mode::Line { at } => {
                if now >= at {
                    self.form_line(now);
                    self.mode = Mode::Calm;
                }
            }
        }
    }

    pub fn active(&self) -> bool {
        !self.cells.is_empty()
    }

    /// Precipitation level at `p`, 0 to 6.
    pub fn level_at(&self, p: Vec2) -> f32 {
        self.cells.iter().map(|c| c.level_at(p)).fold(0.0, f32::max)
    }

    /// The worst weather along a track from `from` on `bearing` for `nm`.
    pub fn worst_along(&self, from: Vec2, bearing: f32, nm: f32) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let dir = heading_vec(bearing);
        let steps = (nm / 0.5).ceil() as usize;
        (0..=steps).map(|i| self.level_at(from + dir * (i as f32 * 0.5))).fold(0.0, f32::max)
    }

    /// Does a track from `from` on `bearing` run into weather of `level` or
    /// worse within `nm`?
    pub fn blocked_at(&self, from: Vec2, bearing: f32, nm: f32, level: f32) -> bool {
        self.worst_along(from, bearing, nm) >= level
    }

    /// Does a track from `from` on `bearing` run into severe weather within `nm`?
    pub fn blocked(&self, from: Vec2, bearing: f32, nm: f32) -> bool {
        self.worst_along(from, bearing, nm) >= SEVERE_LEVEL
    }

    /// Distance from `p` to the nearest severe weather, NM.
    pub fn severe_distance(&self, p: Vec2) -> Option<f32> {
        self.cells
            .iter()
            .filter(|c| c.strength > SEVERE_LEVEL)
            .map(|c| (p.distance(c.pos) - c.radius_at(SEVERE_LEVEL)).max(0.0))
            .reduce(f32::min)
    }

    /// The strongest cell near the field, for the ATIS: how far, which way
    /// from the field, and which way it's moving.
    pub fn report(&self, within: f32) -> Option<(f32, f32, f32)> {
        let cell = self
            .cells
            .iter()
            .filter(|c| c.strength >= SEVERE_LEVEL && c.pos.length() - c.radius < within)
            .min_by(|a, b| a.pos.length().total_cmp(&b.pos.length()))?;
        let moving = normalize_heading(vec_heading(cell.vel));
        Some(((cell.pos.length() - cell.radius).max(0.0), vec_heading(cell.pos), moving))
    }
}

/// A pilot's way round the weather: degrees off the track it means to fly,
/// positive to the right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deviation {
    /// The track it meant to fly when it asked: the offset is from this, so
    /// it holds a steady heading round the weather.
    pub track: f32,
    pub wanted: f32,
    /// What it's flying now: nothing until the deviation is approved.
    pub offset: f32,
    pub asked_at: f32,
    pub approved: bool,
}

impl Deviation {
    /// The heading it flies while deviating.
    pub fn heading(&self) -> f32 {
        normalize_heading(self.track + self.offset)
    }

    pub fn spoken(&self) -> String {
        let side = if self.wanted >= 0.0 { "right" } else { "left" };
        format!("{:.0} {side}", self.wanted.abs())
    }
}

/// A pilot waits this long for a deviation before going anyway, seconds...
pub const DEVIATION_WAIT_S: f32 = 30.0;
/// ...or until severe weather is this close, NM.
const DEVIATION_LAST_NM: f32 = 3.0;
/// Pilots check the weather ahead this often, in ticks.
const CHECK_EVERY_TICKS: u64 = 20;
/// Offsets pilots try, smallest first.
const OFFSETS: [f32; 3] = [20.0, 40.0, 60.0];

/// Where the aircraft means to go: its bearing and how far, NM.
fn intended(ac: &Aircraft, airport: &Airport) -> Option<(f32, f32)> {
    let to_fix = |fix: usize| {
        let d = airport.fixes[fix].pos() - ac.pos;
        (vec_heading(d), d.length().min(LOOKAHEAD_NM))
    };
    match ac.lateral {
        Lateral::Heading(h) => Some((h, LOOKAHEAD_NM)),
        Lateral::Direct(fix) => Some(to_fix(fix)),
        Lateral::Route => ac.route.get(ac.route_index).map(|&f| to_fix(f)),
        _ => None,
    }
}

/// Severe weather along a track a pilot is told to fly, unless it is
/// already in it: grounds to refuse the instruction.
pub fn weather_ahead(storms: &Storms, pos: Vec2, bearing: f32, nm: f32) -> bool {
    storms.level_at(pos) < SEVERE_LEVEL && storms.blocked(pos, bearing, nm)
}

impl World {
    pub(super) fn run_storms(&mut self) {
        self.storms.step(self.time, super::TICK);
        if !self.storms.active() && self.aircraft.iter().all(|a| a.deviation.is_none()) {
            return;
        }
        if !((self.time / super::TICK).round() as u64).is_multiple_of(CHECK_EVERY_TICKS) {
            return;
        }
        for index in 0..self.aircraft.len() {
            self.check_weather_ahead(index);
        }
    }

    fn check_weather_ahead(&mut self, index: usize) {
        self.skip_fix_in_weather(index);
        let ac = &self.aircraft[index];
        let flying = !ac.on_ground
            && ac.vfr.is_none()
            && matches!(ac.phase, Phase::Controlled | Phase::Offered | Phase::OfferedToCenter);
        let Some((bearing, nm)) = intended(ac, &self.airport).filter(|_| flying) else {
            // On an approach, in a hold, or landed: nothing to go round.
            self.aircraft[index].deviation = None;
            return;
        };
        let pos = ac.pos;
        let blocked = self.storms.blocked(pos, bearing, nm);
        let players = self.is_players(ac) && !ac.nordo();
        let callsign = ac.callsign.clone();
        let now = self.time;
        match ac.deviation {
            None if blocked => {
                let wanted = self.way_round(pos, bearing, nm);
                let mut deviation = Deviation { track: bearing, wanted, offset: 0.0, asked_at: now, approved: false };
                if players {
                    self.say(Speaker::Pilot, format!("{callsign}, request {} for weather.", deviation.spoken()));
                } else {
                    // Center, or an automated controller, approves it.
                    deviation.offset = wanted;
                    deviation.approved = true;
                }
                self.aircraft[index].deviation = Some(deviation);
            }
            None => {}
            // Asked, then given a heading or direct that misses it instead.
            Some(d) if !d.approved && !blocked => self.aircraft[index].deviation = None,
            Some(d) if !d.approved => {
                let close = self.storms.severe_distance(pos).is_some_and(|n| n < DEVIATION_LAST_NM);
                if now - d.asked_at >= DEVIATION_WAIT_S || close {
                    self.aircraft[index].deviation = Some(Deviation { offset: d.wanted, approved: true, ..d });
                    let line = format!("{callsign}, deviating {} for weather, unable to wait.", d.spoken());
                    self.say(Speaker::Pilot, line);
                    self.emit(SimEvent::DeviationIgnored { callsign });
                }
            }
            Some(_) if !self.storms.blocked_at(pos, bearing, nm, CLEAR_LEVEL) => self.end_deviation(index, players),
            Some(d) => {
                // Not enough: further round, the same way.
                let ahead = self.storms.blocked_at(pos, d.heading(), LOOKAHEAD_NM * 0.5, CLEAR_LEVEL);
                if d.offset.abs() < OFFSETS[OFFSETS.len() - 1] && ahead {
                    let offset = d.offset + 20.0 * d.offset.signum();
                    self.aircraft[index].deviation = Some(Deviation { offset, wanted: offset, ..d });
                }
            }
        }
    }

    /// A fix on its route under a storm: the pilot goes on to the next one.
    fn skip_fix_in_weather(&mut self, index: usize) {
        let ac = &self.aircraft[index];
        if ac.lateral != Lateral::Route || ac.route_index + 1 >= ac.route.len() || ac.on_ground {
            return;
        }
        let fix = ac.route[ac.route_index];
        if self.storms.level_at(self.airport.fixes[fix].pos()) < SEVERE_LEVEL {
            return;
        }
        let players = self.is_players(ac) && !ac.nordo();
        let ac = &mut self.aircraft[index];
        ac.route_index += 1;
        if players {
            let next = ac.route[ac.route_index];
            let line = format!(
                "{}, {} is under the weather, proceeding direct {}.",
                ac.callsign, self.airport.fixes[fix].name, self.airport.fixes[next].name
            );
            self.say(Speaker::Pilot, line);
        }
    }

    /// The smallest offset either side that clears the weather, the side
    /// with less of it first.
    fn way_round(&self, pos: Vec2, bearing: f32, nm: f32) -> f32 {
        let worst = |offset: f32| self.storms.worst_along(pos, normalize_heading(bearing + offset), nm);
        let mut best = OFFSETS[OFFSETS.len() - 1];
        let mut best_worst = f32::INFINITY;
        for &size in &OFFSETS {
            let (right, left) = (worst(size), worst(-size));
            if right.min(left) < CLEAR_LEVEL {
                return if left < right { -size } else { size };
            }
            for (offset, w) in [(size, right), (-size, left)] {
                if w < best_worst {
                    best_worst = w;
                    best = offset;
                }
            }
        }
        best
    }

    /// Clear of the weather: back on course.
    fn end_deviation(&mut self, index: usize, players: bool) {
        let airport = &self.airport;
        let ac = &mut self.aircraft[index];
        ac.deviation = None;
        // Fixes it passed while going round are behind it now.
        if ac.lateral == Lateral::Route {
            while ac.route_index + 1 < ac.route.len() {
                let here = airport.fixes[ac.route[ac.route_index]].pos();
                let next = airport.fixes[ac.route[ac.route_index + 1]].pos();
                if (here - ac.pos).dot(next - here) >= 0.0 {
                    break;
                }
                ac.route_index += 1;
            }
        }
        if !players {
            return;
        }
        let back = match ac.lateral {
            Lateral::Heading(h) => format!("resuming heading {}", format_heading(airport.to_magnetic(h))),
            Lateral::Direct(f) => format!("proceeding direct {}", airport.fixes[f].name),
            _ => match ac.route.get(ac.route_index) {
                Some(&f) => format!("proceeding direct {}", airport.fixes[f].name),
                None => "back on course".to_string(),
            },
        };
        let line = format!("{}, clear of the weather, {back}.", ac.callsign);
        self.say(Speaker::Pilot, line);
    }
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::airspace::{KSUS_AIRPORT, KSUS_GROUND};

    fn airports() -> [Airport; 2] {
        [Airport::default_airport(), Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap()]
    }

    #[test]
    fn a_cell_is_strongest_at_its_core_and_nothing_past_its_edge() {
        let cell = Cell {
            pos: Vec2::new(10.0, 0.0),
            vel: Vec2::ZERO,
            radius: 5.0,
            peak: 5.0,
            born: 0.0,
            life: 100.0,
            strength: 5.0,
            fade_from: None,
        };
        assert_eq!(cell.level_at(cell.pos), 5.0);
        assert!(cell.level_at(Vec2::new(12.0, 0.0)) > cell.level_at(Vec2::new(14.0, 0.0)));
        assert_eq!(cell.level_at(Vec2::new(15.0, 0.0)), 0.0);
        let r = cell.radius_at(3.0);
        assert!((cell.level_at(Vec2::new(10.0 + r, 0.0)) - 3.0).abs() < 1e-4);
        // Builds, matures, decays.
        assert!(cell.strength_at(10.0) < cell.strength_at(50.0));
        assert!(cell.strength_at(90.0) < cell.strength_at(50.0));
        assert_eq!(cell.strength_at(100.0), 0.0);
    }

    #[test]
    fn cells_never_come_near_the_field_or_the_holds() {
        for airport in airports() {
            for seed in 0..40 {
                let shift = 60.0 * 60.0;
                let mut storms = Storms::new(&airport, seed, shift, seed % 3 != 0, seed % 3 == 0);
                let mut seen = 0;
                let mut t = 0.0;
                while t < shift {
                    storms.step(t, 10.0);
                    t += 10.0;
                    for cell in &storms.cells {
                        seen += 1;
                        assert!(cell.pos.length() - cell.radius > AIRPORT_KEEP_OUT_NM - 0.1, "{} seed {seed}", airport.code);
                        for hold in &airport.holds {
                            let fix = airport.fixes[airport.fix_index(&hold.fix).unwrap()].pos();
                            assert!(cell.pos.distance(fix) - cell.radius > HOLD_KEEP_OUT_NM - 0.1);
                        }
                    }
                }
                assert!(seen > 0, "{} seed {seed}: no storms", airport.code);
            }
        }
    }

    #[test]
    fn a_track_through_a_cell_is_blocked_and_one_beside_it_is_not() {
        let mut storms = Storms::calm();
        storms.cells.push(Cell {
            pos: Vec2::new(0.0, 8.0),
            vel: Vec2::ZERO,
            radius: 4.0,
            peak: 5.0,
            born: 0.0,
            life: 1000.0,
            strength: 5.0,
            fade_from: None,
        });
        assert!(storms.blocked(Vec2::ZERO, 0.0, 10.0));
        assert!(!storms.blocked(Vec2::ZERO, 0.0, 5.0));
        assert!(!storms.blocked(Vec2::ZERO, 40.0, 10.0));
        assert!(storms.severe_distance(Vec2::ZERO).unwrap() > 4.0);
    }
}
