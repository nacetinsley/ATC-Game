//! Clearance Delivery: filed flight plans, IFR clearances and readbacks.
//!
//! An IFR departure files a flight plan before it calls. With Delivery staffed
//! it calls for its clearance from the gate, and the controller checks the plan
//! before clearing it: about one in six has something wrong with it. With
//! Delivery automated, every departure is cleared as it's placed.

use super::aircraft::{Aircraft, Flow, Phase};
use super::airspace::Airport;
use super::clearance::thousands;
use super::positions::Position;
use super::scoring::SimEvent;
use super::units::{normalize_heading, vec_heading};
use super::{Speaker, World};

/// One flight plan in this many is filed with a mistake, when Delivery is staffed.
pub const PLAN_ERROR_ONE_IN: u32 = 6;
/// One readback in this many gets the squawk or altitude wrong.
pub const READBACK_ERROR_ONE_IN: u32 = 10;
/// On a combined Ground and Delivery position, a cleared pilot calls for taxi
/// this long after the readback, seconds.
pub const READY_TO_TAXI_S: f32 = 30.0;
/// The automated Departure controller notices a wrong code at radar contact,
/// this long after the aircraft checks in, and has it reset, seconds.
pub const SQUAWK_RESET_S: f32 = 15.0;
/// Waiting longer than this for a clearance is delay, seconds.
pub const CLEARANCE_WAIT_S: f32 = 180.0;
pub const MIN_CLEARED_ALTITUDE: f32 = 3000.0;
pub const MAX_CLEARED_ALTITUDE: f32 = 51000.0;
/// Up to here cruising altitudes go in 1,000 ft steps; above it, in 4,000 ft
/// steps each way (14 CFR 91.179 outside RVSM).
const RVSM_CEILING: f32 = 41000.0;
/// From here up altitudes are flight levels.
const FLIGHT_LEVELS: f32 = 18000.0;

/// What a departure filed, and what Delivery has cleared it for.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightPlan {
    pub destination: String,
    /// The filed departure route: a SID's fixes.
    pub route: Vec<usize>,
    /// Requested cruising altitude, feet.
    pub altitude: f32,
    /// The initial altitude to maintain after takeoff, feet.
    pub initial: f32,
    /// Beacon code assigned with the clearance.
    pub squawk: u16,
    /// When it called for its clearance.
    pub called_at: Option<f32>,
    pub cleared: Option<Cleared>,
    /// A wrong value the pilot read back and set.
    pub readback: Option<Readback>,
    /// The code its transponder shows, when that isn't the one assigned: a
    /// squawk readback nobody caught, until radar has it reset.
    pub squawking: Option<u16>,
    /// Scored, once it has left Delivery.
    pub done: bool,
    /// Clearances it has been given, for the readback roll.
    pub issued: u32,
    /// For the lesson: `Some(true)` makes the next readback get the code
    /// wrong, `Some(false)` keeps every readback right.
    pub mishear: Option<bool>,
}

impl FlightPlan {
    pub fn is_cleared(&self) -> bool {
        self.cleared.is_some()
    }
}

/// The clearance as given.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleared {
    pub route: Vec<usize>,
    pub altitude: f32,
    pub squawk: u16,
    pub time: f32,
}

/// What the pilot got wrong in the readback, and set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Readback {
    Squawk(u16),
    /// The initial altitude.
    Altitude(f32),
}

/// A discrete departure code from the TRACON's range, by aircraft id. Never
/// 1200 or a 7xxx code. Octal digits, as a transponder takes.
pub fn departure_code(id: u32) -> u16 {
    (0o400 + id.wrapping_mul(13) % 0o400) as u16
}

/// A code as said and set: four octal digits.
pub fn code_text(code: u16) -> String {
    format!("{code:04o}")
}

/// Altitude as cleared: "FL350", or "7,000" below the flight levels.
pub fn altitude_text(altitude: f32) -> String {
    if altitude >= FLIGHT_LEVELS {
        format!("FL{:03.0}", altitude / 100.0)
    } else {
        thousands(altitude)
    }
}

/// Eastbound for 14 CFR 91.179: magnetic course 0 to 179.
pub fn eastbound(course: f32) -> bool {
    normalize_heading(course) < 180.0
}

/// Is this cruising altitude right for the direction of flight? Odd thousands
/// eastbound, even westbound; above FL410, FL410, 450, 490 east and 430, 470,
/// 510 west.
pub fn direction_ok(altitude: f32, course: f32) -> bool {
    let a = altitude.round() as i32;
    if a % 1000 != 0 {
        return false;
    }
    let east = eastbound(course);
    if altitude <= RVSM_CEILING {
        (a / 1000 % 2 == 1) == east
    } else {
        let base = if east { 41000 } else { 43000 };
        a >= base && (a - base) % 4000 == 0
    }
}

/// The highest correct altitude at or below `altitude` for the course.
pub fn level_for(altitude: f32, course: f32) -> f32 {
    let east = eastbound(course);
    if altitude > RVSM_CEILING {
        let base = if east { 41000.0 } else { 43000.0 };
        if altitude >= base {
            return base + ((altitude - base) / 4000.0).floor() * 4000.0;
        }
    }
    let mut t = (altitude.min(RVSM_CEILING) / 1000.0).floor() as i32;
    while (t % 2 == 1) != east {
        t -= 1;
    }
    (t.max(if east { 3 } else { 4 }) * 1000) as f32
}

/// The next altitude a step up (`+1`) or down (`-1`) from this one, as the
/// controller amends it: thousands up to FL410, 2,000 ft steps above.
pub fn step_altitude(altitude: f32, by: i32) -> f32 {
    let step = if altitude + by as f32 * 1000.0 > RVSM_CEILING { 2000.0 } else { 1000.0 };
    let stepped = ((altitude + by as f32 * step) / 1000.0).round() * 1000.0;
    stepped.clamp(MIN_CLEARED_ALTITUDE, MAX_CLEARED_ALTITUDE)
}

/// Magnetic course to the destination: out by the route's exit fix.
pub fn course(airport: &Airport, route: &[usize]) -> f32 {
    match route.last().and_then(|&f| airport.fixes.get(f)) {
        Some(fix) => airport.to_magnetic(vec_heading(fix.pos())),
        None => 0.0,
    }
}

fn names(airport: &Airport, route: &[usize]) -> Vec<String> {
    route.iter().filter_map(|&f| airport.fixes.get(f)).map(|f| f.name.clone()).collect()
}

/// SIDs published for a runway configuration, as fix lists.
fn sids<'a>(airport: &'a Airport, config: &'a str) -> impl Iterator<Item = Vec<usize>> + 'a {
    airport.sids.iter().filter(move |s| s.config == config).map(|s| airport.route_indices(&s.fixes))
}

/// Is this route a SID in use with the configuration?
pub fn sid_in_use(airport: &Airport, config: &str, route: &[usize]) -> bool {
    sids(airport, config).any(|r| r == route)
}

/// The SIDs in use the controller can amend to, in order: those leaving by the
/// same exit fix, or all of them if none do.
pub fn sid_choices(airport: &Airport, config: &str, route: &[usize]) -> Vec<Vec<usize>> {
    let all: Vec<Vec<usize>> = sids(airport, config).collect();
    let same: Vec<Vec<usize>> = all.iter().filter(|r| r.last() == route.last()).cloned().collect();
    if same.is_empty() { all } else { same }
}

/// "the LINDY9 departure, MYERZ transition", or the fixes for a SID with no name.
pub fn route_words(airport: &Airport, route: &[usize]) -> String {
    let fixes = names(airport, route);
    let sid = airport.sids.iter().find(|s| s.fixes == fixes).and_then(|s| s.name.clone());
    match (sid, fixes.last()) {
        (Some(sid), Some(exit)) if fixes.len() > 1 => format!("the {sid} departure, {exit} transition"),
        (Some(sid), _) => format!("the {sid} departure"),
        _ => fixes.join(", "),
    }
}

/// The route as a strip shows it: "LINDY9 MYERZ", or the fixes.
pub fn route_short(airport: &Airport, route: &[usize]) -> String {
    let fixes = names(airport, route);
    let sid = airport.sids.iter().find(|s| s.fixes == fixes).and_then(|s| s.name.clone());
    match (sid, fixes.last()) {
        (Some(sid), Some(exit)) => format!("{sid} {exit}"),
        _ => fixes.join(" "),
    }
}

/// What's wrong with clearing this route and altitude now, if anything.
pub fn problem(airport: &Airport, config: &str, route: &[usize], altitude: f32) -> Option<String> {
    if !sid_in_use(airport, config, route) {
        return Some(format!(
            "via {}, not in use with the {} runways",
            route_short(airport, route),
            config.to_lowercase()
        ));
    }
    let course = course(airport, route);
    if !direction_ok(altitude, course) {
        let (way, rule) = match (eastbound(course), altitude > RVSM_CEILING) {
            (true, false) => ("eastbound", "odd altitudes only"),
            (false, false) => ("westbound", "even altitudes only"),
            (true, true) => ("eastbound", "FL410, 450, 490 only"),
            (false, true) => ("westbound", "FL430, 470, 510 only"),
        };
        return Some(format!("at {} {way}: {rule}", altitude_text(altitude)));
    }
    None
}

/// The controller's words for an IFR clearance (7110.65 4-3-2): limit, route,
/// altitude, transponder. Re-issued with nothing changed, it's the
/// altitude and code again, said whether or not the readback was wrong.
pub fn phrase(ac: &Aircraft, route: &[usize], altitude: f32, squawk: u16, airport: &Airport) -> String {
    let Some(plan) = &ac.flight_plan else { return "cleared as filed".into() };
    if is_repeat(ac, route, altitude, squawk) {
        return format!("I say again, maintain {}, squawk {}", thousands(plan.initial), code_text(squawk));
    }
    let amended = if plan.is_cleared() { "amended clearance, " } else { "" };
    let limit = if plan.destination.is_empty() {
        "cleared as filed".to_string()
    } else {
        format!("cleared to {} airport", plan.destination)
    };
    format!(
        "{amended}{limit} via {}, then as filed, maintain {}, expect {} one zero minutes after departure, squawk {}",
        route_words(airport, route),
        thousands(plan.initial),
        altitude_text(altitude),
        code_text(squawk)
    )
}

/// The same clearance it already has: a repeat of the key items.
pub fn is_repeat(ac: &Aircraft, route: &[usize], altitude: f32, squawk: u16) -> bool {
    ac.flight_plan.as_ref().and_then(|p| p.cleared.as_ref()).is_some_and(|c| {
        c.route == route && c.altitude == altitude && c.squawk == squawk
    })
}

/// The pilot's readback, with whatever it got wrong.
pub fn readback(ac: &Aircraft, repeat: bool, airport: &Airport) -> String {
    let Some(plan) = &ac.flight_plan else { return String::new() };
    let Some(cleared) = &plan.cleared else { return String::new() };
    let (initial, squawk) = match plan.readback {
        Some(Readback::Squawk(code)) => (plan.initial, code),
        Some(Readback::Altitude(a)) => (a, cleared.squawk),
        None => (plan.initial, cleared.squawk),
    };
    let squawk = code_text(squawk);
    let initial = thousands(initial);
    if repeat {
        return format!("maintain {initial}, squawk {squawk}");
    }
    let to = if plan.destination.is_empty() { String::new() } else { format!(" to {}", plan.destination) };
    format!(
        "cleared{to} via {}, maintain {initial}, expect {}, squawk {squawk}",
        route_words(airport, &cleared.route),
        altitude_text(cleared.altitude)
    )
}

/// A plausible slip: two digits swapped, or one misheard.
fn wrong_code(code: u16, roll: u32) -> u16 {
    let d = [(code >> 9) & 7, (code >> 6) & 7, (code >> 3) & 7, code & 7];
    let mut w = d;
    if roll.is_multiple_of(2) && d[2] != d[3] {
        w.swap(2, 3);
    } else if d[1] != d[2] {
        w.swap(1, 2);
    } else {
        w[3] = (d[3] + 1 + (roll % 6) as u16) % 8;
    }
    (w[0] << 9) | (w[1] << 6) | (w[2] << 3) | w[3]
}

/// The pilot takes an IFR clearance and reads it back, maybe wrongly.
pub(super) fn apply(
    ac: &mut Aircraft,
    route: &[usize],
    altitude: f32,
    squawk: u16,
    now: f32,
    seed: u64,
) -> Result<(), String> {
    if ac.vfr.is_some() {
        return Err("we're VFR".into());
    }
    if ac.flow != Flow::Departure || !ac.on_ground {
        return Err("we're not departing".into());
    }
    if ac.phase != Phase::Parked {
        return Err("we already have our clearance".into());
    }
    if route.is_empty() {
        return Err("no route".into());
    }
    if !(MIN_CLEARED_ALTITUDE..=MAX_CLEARED_ALTITUDE).contains(&altitude) {
        return Err("altitude out of range".into());
    }
    let repeat = is_repeat(ac, route, altitude, squawk);
    let id = ac.id;
    let Some(plan) = ac.flight_plan.as_mut() else { return Err("no flight plan on file".into()) };
    if repeat {
        // Heard right this time.
        if let Some(Readback::Altitude(_)) = plan.readback {
            ac.target_altitude = plan.initial;
        }
        plan.readback = None;
        return Ok(());
    }
    plan.issued += 1;
    plan.cleared = Some(Cleared { route: route.to_vec(), altitude, squawk, time: now });
    plan.readback = None;
    let mut rng = fastrand::Rng::with_seed(seed ^ 0xC1EA_0000 ^ ((id as u64) << 24) ^ plan.issued as u64);
    let scripted = plan.mishear;
    let wrong = match scripted {
        Some(wrong) => wrong,
        None => rng.u32(0..READBACK_ERROR_ONE_IN) == 0,
    };
    if scripted == Some(true) {
        plan.mishear = Some(false);
    }
    if wrong {
        plan.readback = Some(if scripted.is_some() || rng.bool() {
            Readback::Squawk(wrong_code(squawk, rng.u32(..)))
        } else {
            Readback::Altitude(plan.initial + if rng.bool() { 1000.0 } else { -1000.0 })
        });
    }
    let initial = match plan.readback {
        Some(Readback::Altitude(a)) => a,
        _ => plan.initial,
    };
    ac.route = route.to_vec();
    ac.route_index = 0;
    ac.exit_fix = route.last().copied();
    ac.target_altitude = initial;
    Ok(())
}

impl World {
    /// File a flight plan for an IFR departure: as the pilot wants it, and
    /// now and then wrong, when there's a Delivery controller to catch it.
    pub(super) fn file_flight_plan(&self, ac: &mut Aircraft) {
        if ac.vfr.is_some() {
            return;
        }
        let airport = &self.airport;
        let bearing = ac.exit_fix.and_then(|f| airport.fixes.get(f)).map_or(0.0, |f| vec_heading(f.pos()));
        let destination = airport.city_pair(bearing, ac.id).unwrap_or_default().to_string();
        let mut route = ac.route.clone();
        let course = course(airport, &route);
        let mut altitude = level_for(ac.ty.filed_altitude() as f32 * 100.0, course);
        let mut rng = fastrand::Rng::with_seed(self.config.seed ^ 0xF11E_D000 ^ ((ac.id as u64) << 20));
        if self.config.staffing.delivery && rng.u32(0..PLAN_ERROR_ONE_IN) == 0 {
            let config = &self.config().name;
            let exit = route.last().copied();
            let wrong_sids: Vec<Vec<usize>> = airport
                .sids
                .iter()
                .filter(|s| &s.config != config)
                .map(|s| airport.route_indices(&s.fixes))
                .filter(|r| r.last().copied() == exit && !sid_in_use(airport, config, r))
                .collect();
            if !wrong_sids.is_empty() && rng.bool() {
                route = wrong_sids[rng.usize(0..wrong_sids.len())].clone();
            } else {
                let step = if altitude > RVSM_CEILING { 2000.0 } else { 1000.0 };
                altitude = if altitude - step >= MIN_CLEARED_ALTITUDE { altitude - step } else { altitude + step };
            }
        }
        ac.flight_plan = Some(FlightPlan {
            destination,
            route,
            altitude,
            initial: ac.target_altitude,
            squawk: departure_code(ac.id),
            called_at: None,
            cleared: None,
            readback: None,
            squawking: None,
            done: false,
            issued: 0,
            mishear: None,
        });
    }

    /// The automated Delivery: cleared as it should have filed, with no
    /// mistakes and nothing to score.
    pub(super) fn clear_automatically(&self, ac: &mut Aircraft) {
        let Some(plan) = ac.flight_plan.as_mut() else { return };
        let course = course(&self.airport, &ac.route);
        let altitude = if direction_ok(plan.altitude, course) {
            plan.altitude
        } else {
            level_for(plan.altitude, course)
        };
        plan.cleared = Some(Cleared { route: ac.route.clone(), altitude, squawk: plan.squawk, time: self.time });
        plan.done = true;
    }

    /// Delivery's side of the departure flow each tick: on a combined Ground
    /// position a cleared pilot calls for taxi, and every clearance is scored
    /// once the aircraft has left Delivery.
    pub(super) fn run_delivery(&mut self) {
        let staffing = self.config.staffing;
        if !staffing.delivery {
            return;
        }
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let Some(plan) = &ac.flight_plan else { continue };
            if plan.done {
                continue;
            }
            let Some(cleared) = &plan.cleared else { continue };
            let on_delivery = ac.on_ground && ac.surface_freq == Position::Delivery && ac.phase == Phase::Parked;
            if on_delivery {
                // Combined with Ground: the same controller, so the pilot just
                // calls for taxi once it has the clearance.
                if staffing.ground && self.time - cleared.time >= READY_TO_TAXI_S && !self.call_pending(ac.id) {
                    let spot = self.gate_name(ac);
                    let line = format!("Ground, {}, at {spot}, ready to taxi.", ac.callsign);
                    let now = self.time;
                    let ac = &mut self.aircraft[index];
                    ac.surface_freq = Position::Ground;
                    ac.freq_since = now;
                    self.say(Speaker::Pilot, line);
                    self.score_clearance(index);
                }
                continue;
            }
            self.score_clearance(index);
        }
    }

    /// The clearance is settled: it has left Delivery.
    fn score_clearance(&mut self, index: usize) {
        let config = self.config().name.clone();
        let ac = &self.aircraft[index];
        let Some(plan) = &ac.flight_plan else { return };
        let Some(cleared) = &plan.cleared else { return };
        let callsign = ac.callsign.clone();
        let waited = plan.called_at.map_or(0.0, |t| (cleared.time - t).max(0.0));
        let filed = problem(&self.airport, &config, &plan.route, plan.altitude);
        let given = problem(&self.airport, &config, &cleared.route, cleared.altitude);
        let mut events = vec![SimEvent::Cleared { callsign: callsign.clone(), waited_minutes: waited / 60.0 }];
        match (&filed, &given) {
            (_, Some(p)) => events.push(SimEvent::PlanMissed { callsign: callsign.clone(), problem: p.clone() }),
            (Some(p), None) => events.push(SimEvent::PlanCaught { callsign: callsign.clone(), problem: p.clone() }),
            (None, None) => {}
        }
        if let Some(wrong) = plan.readback {
            let what = match wrong {
                Readback::Squawk(code) => {
                    format!("squawking {}, assigned {}", code_text(code), code_text(cleared.squawk))
                }
                Readback::Altitude(a) => format!("climbing to {}, assigned {}", thousands(a), thousands(plan.initial)),
            };
            events.push(SimEvent::ReadbackMissed { callsign, what });
        }
        // Whatever slipped through, the next controller sorts out: a SID in
        // use from the same exit, the altitude it should have.
        let fix_route = !sid_in_use(&self.airport, &config, &cleared.route);
        let choice = sid_choices(&self.airport, &config, &cleared.route).into_iter().next();
        let initial = plan.initial;
        let had_altitude_slip = matches!(plan.readback, Some(Readback::Altitude(_)));
        let squawking = match plan.readback {
            Some(Readback::Squawk(code)) => Some(code),
            _ => None,
        };
        let ac = &mut self.aircraft[index];
        if fix_route && let Some(route) = choice {
            ac.exit_fix = route.last().copied();
            ac.route = route;
            ac.route_index = 0;
        }
        if had_altitude_slip {
            ac.target_altitude = initial;
        }
        let plan = ac.flight_plan.as_mut().expect("checked");
        plan.readback = None;
        plan.squawking = squawking;
        plan.done = true;
        for event in events {
            self.emit(event);
        }
    }

    /// The automated Departure controller has a wrong code reset once the
    /// aircraft has been on its frequency a little while. A player working
    /// Approach does that themselves.
    pub(super) fn run_transponders(&mut self) {
        if self.config.staffing.approach {
            return;
        }
        let due: Vec<(u32, u16)> = self
            .aircraft
            .iter()
            .filter(|a| !a.on_ground && a.frequency() == Position::Approach)
            .filter(|a| self.time - a.freq_since >= SQUAWK_RESET_S)
            .filter_map(|a| {
                let plan = a.flight_plan.as_ref()?;
                plan.squawking.map(|_| (a.id, plan.squawk))
            })
            .collect();
        for (id, code) in due {
            if !self.call_pending(id) {
                self.issue(id, vec![super::clearance::Clearance::Squawk(code)], Position::Approach);
            }
        }
    }

    /// The parking spot an aircraft is at.
    pub fn gate_name(&self, ac: &Aircraft) -> String {
        let Some(map) = self.airport.surface.as_ref() else { return String::new() };
        map.parking
            .iter()
            .min_by(|a, b| {
                let d = |p: &super::ground_map::Parking| map.nodes[p.node].pos.distance(ac.pos);
                d(a).total_cmp(&d(b))
            })
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    /// Waiting for a clearance from the player: at the gate, on Delivery, not yet cleared.
    pub fn wants_clearance(&self, ac: &Aircraft) -> bool {
        ac.on_ground
            && ac.phase == Phase::Parked
            && ac.surface_freq == Position::Delivery
            && ac.flight_plan.as_ref().is_some_and(|p| !p.is_cleared())
    }

    /// The configuration in use's name, for checking SIDs against.
    pub fn config_name(&self) -> &str {
        &self.config().name
    }
}

/// The code shown on the scope when it isn't the one assigned.
pub fn wrong_code_showing(ac: &Aircraft) -> Option<u16> {
    if ac.on_ground {
        return None;
    }
    ac.flight_plan.as_ref().and_then(|p| p.squawking)
}

/// "Reset transponder, squawk 0415": the pilot sets the code given.
pub(super) fn reset_squawk(ac: &mut Aircraft, code: u16) -> Result<(), String> {
    if ac.on_ground {
        return Err("we're on the ground".into());
    }
    let Some(plan) = ac.flight_plan.as_mut() else { return Err("no flight plan".into()) };
    plan.squawking = (code != plan.squawk).then_some(code);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_of_flight_follows_91_179() {
        assert!(direction_ok(35000.0, 90.0));
        assert!(!direction_ok(34000.0, 90.0));
        assert!(direction_ok(34000.0, 270.0));
        // The boundaries: 179 is east, 180 west, 359 west, 0 east.
        assert!(direction_ok(7000.0, 179.0));
        assert!(direction_ok(6000.0, 180.0));
        assert!(direction_ok(6000.0, 359.9));
        assert!(direction_ok(7000.0, 0.0));
        assert!(direction_ok(7000.0, 360.0));
        // FL410 is the top odd level; above it, 4,000 ft steps each way.
        assert!(direction_ok(41000.0, 90.0));
        assert!(direction_ok(40000.0, 270.0));
        assert!(direction_ok(45000.0, 90.0));
        assert!(!direction_ok(43000.0, 90.0));
        assert!(direction_ok(43000.0, 270.0));
        assert!(direction_ok(47000.0, 270.0));
        assert!(!direction_ok(45000.0, 270.0));
        assert!(!direction_ok(6500.0, 90.0));
    }

    #[test]
    fn filed_levels_round_down_to_the_right_parity() {
        assert_eq!(level_for(35000.0, 90.0), 35000.0);
        assert_eq!(level_for(35000.0, 270.0), 34000.0);
        assert_eq!(level_for(6500.0, 90.0), 5000.0);
        assert_eq!(level_for(6500.0, 270.0), 6000.0);
        assert_eq!(level_for(45000.0, 270.0), 43000.0);
        assert_eq!(level_for(41000.0, 270.0), 40000.0);
        assert_eq!(level_for(45000.0, 90.0), 45000.0);
        for alt in [6500.0, 12000.0, 26000.0, 35000.0, 37000.0, 41000.0, 45000.0] {
            for course in [10.0, 179.0, 180.0, 300.0] {
                assert!(direction_ok(level_for(alt, course), course), "{alt} {course}");
            }
        }
    }

    #[test]
    fn amendments_step_by_thousands_then_two_thousands() {
        assert_eq!(step_altitude(35000.0, 1), 36000.0);
        assert_eq!(step_altitude(40000.0, 1), 41000.0);
        assert_eq!(step_altitude(41000.0, 1), 43000.0);
        assert_eq!(step_altitude(43000.0, -1), 41000.0);
        assert_eq!(step_altitude(3000.0, -1), 3000.0);
    }

    #[test]
    fn a_misheard_code_is_still_a_transponder_code_and_different() {
        for code in [0o452, 0o777, 0o400, 0o444, 0o512] {
            for roll in 0..12 {
                let wrong = wrong_code(code, roll);
                assert_ne!(wrong, code);
                assert!(wrong < 0o10000);
            }
        }
        assert_eq!(code_text(wrong_code(0o452, 0)), "0425");
        assert_eq!(code_text(departure_code(7)), code_text(departure_code(7)));
        assert!(code_text(departure_code(7)).starts_with('0'));
    }
}
