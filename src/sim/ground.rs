//! Surface movement: leaving the runway after landing, taxiing along the ground
//! graph, holding short, following traffic, and runway incursions.

use super::aircraft::{Aircraft, EXIT_SPEED, Flow, Lateral, Phase, ROLLOUT_DECEL, Taxi};
use super::airspace::Airport;
use super::clearance::{AT_NODE_NM, start_roll};
use super::ground_map::GroundMap;
use super::positions::Position;
use super::ramp::Handling;
use super::scoring::{GoAroundReason, SimEvent};
use super::tower::{INCURSION_FINAL_NM, VFR_INCURSION_NM};
use super::units::{SECONDS_PER_HOUR, heading_diff, heading_vec, vec_heading};
use super::{Speaker, TICK, World};
use glam::Vec2;
use std::collections::BTreeSet;

/// Normal taxi speed, knots.
pub const TAXI_SPEED: f32 = 15.0;
/// Speed through a sharp turn, knots.
const TURN_SPEED: f32 = 8.0;
const SHARP_TURN_DEG: f32 = 35.0;
/// Braking pilots plan on when slowing for a turn or a stop, knots per second.
const PLAN_DECEL: f32 = 3.0;
/// How far ahead pilots look for a turn or a stop, NM.
const LOOKAHEAD_NM: f32 = 1.5;
const TAXI_ACCEL: f32 = 2.0;
const TAXI_BRAKE: f32 = 5.0;
/// Pilots stop for traffic ahead closer than this, NM (about 180 ft).
const FOLLOW_NM: f32 = 0.03;
/// "Ahead" is within this cosine of the direction of travel.
const AHEAD_COS: f32 = 0.7;
/// Closer than this on the ground is a conflict, NM (about 60 ft).
const CONFLICT_NM: f32 = 0.01;
/// A departure can't be placed at a hold line with traffic this close, NM.
const HOLD_CLEAR_NM: f32 = 0.04;
/// A runway exit turning back more than a right angle by more than this
/// (cosine of about 95 degrees) is only taken when there's nothing else.
const EXIT_BACK_COS: f32 = 0.09;

type Pair = (u32, u32);

fn pair_key(a: u32, b: u32) -> Pair {
    (a.min(b), a.max(b))
}

fn approach(value: f32, target: f32, max_step: f32) -> f32 {
    value + (target - value).clamp(-max_step, max_step)
}

/// Distance along a runway end's direction from its threshold.
fn along_runway(airport: &Airport, end: usize, p: Vec2) -> f32 {
    let r = &airport.runways[end];
    (p - r.threshold()).dot(heading_vec(r.heading))
}

fn path_length(map: &GroundMap, nodes: &[usize]) -> f32 {
    nodes.windows(2).map(|w| map.nodes[w[0]].pos.distance(map.nodes[w[1]].pos)).sum()
}

/// From a node beside the runway, the way off it: up to and including the first
/// node outside its zone, heading for `parking` if given.
fn route_off(map: &GroundMap, from: usize, runway: usize, parking: Option<usize>) -> Option<Vec<usize>> {
    let truncate = |nodes: Vec<usize>| {
        let cut = nodes.iter().position(|&n| !map.in_zone(n, runway))?;
        Some(nodes[..=cut].to_vec())
    };
    if let Some(spot) = parking
        && let Some(route) = map.route(from, spot, &[])
        && let Some(nodes) = truncate(route.nodes)
    {
        return Some(nodes);
    }
    // Nearest hold line for this runway.
    (0..map.nodes.len())
        .filter(|&n| map.nodes[n].hold == Some(runway))
        .filter_map(|h| map.route(from, h, &[]).and_then(|r| truncate(r.nodes)))
        .min_by(|a, b| path_length(map, a).total_cmp(&path_length(map, b)))
}

/// The next place along the route the aircraft must slow down: the distance to
/// it and the speed to be down to there. Sharp turns, hold lines it isn't
/// cleared past, and the end of the route.
fn next_slowdown(map: &GroundMap, taxi: &Taxi, pos: Vec2, rollout: bool) -> (f32, f32) {
    let mut distance = 0.0;
    let mut from = pos;
    let mut previous: Option<usize> = None;
    for i in taxi.index..taxi.path.len() {
        let node = taxi.path[i];
        let at = map.nodes[node].pos;
        distance += from.distance(at);
        if distance > LOOKAHEAD_NM {
            break;
        }
        let Some(&after) = taxi.path.get(i + 1) else { return (distance, 0.0) };
        let here = &map.nodes[node].zones;
        if map.nodes[after].zones.iter().any(|z| !here.contains(z) && !taxi.cleared.contains(z)) {
            return (distance, 0.0);
        }
        let incoming = match previous {
            Some(p) => map.heading_between(p, node),
            None => vec_heading(at - from),
        };
        if from.distance(at) > 1e-6 || previous.is_some() {
            let turn = heading_diff(incoming, map.heading_between(node, after));
            if turn.abs() > SHARP_TURN_DEG {
                let speed = if rollout && map.nodes[node].runway.is_some() { EXIT_SPEED } else { TURN_SPEED };
                return (distance, speed);
            }
        }
        previous = Some(node);
        from = at;
    }
    (LOOKAHEAD_NM, f32::MAX)
}

/// The turn-off for an aircraft rolling out: the first exit it can stop for,
/// on the side closest to `goal` (its stand or ramp spot), if it has one.
/// Returns the node behind it and the path.
fn plan_exit(map: &GroundMap, airport: &Airport, ac: &Aircraft, goal: Option<usize>) -> Option<(usize, Vec<usize>)> {
    let end = ac.approach_runway?;
    let runway = map.runway_of_end[end];
    let s = |n: usize| along_runway(airport, end, map.nodes[n].pos);
    let here = along_runway(airport, end, ac.pos);
    let mut on_runway: Vec<usize> =
        (0..map.nodes.len()).filter(|&n| map.nodes[n].runway == Some(runway)).collect();
    on_runway.sort_by(|&a, &b| s(a).total_cmp(&s(b)));
    let behind = on_runway.iter().copied().rev().find(|&n| s(n) <= here).unwrap_or(on_runway[0]);

    let stopping = (ac.ias.powi(2) - EXIT_SPEED.powi(2)).max(0.0) / (2.0 * ROLLOUT_DECEL * SECONDS_PER_HOUR);
    let all = map.exits(runway);
    // A taxiway leaving at a right angle or angled ahead; not one that doubles back.
    let direction = heading_vec(airport.runways[end].heading);
    let forward = |&(rn, first): &(usize, usize)| {
        (map.nodes[first].pos - map.nodes[rn].pos).normalize_or_zero().dot(direction) > -EXIT_BACK_COS
    };
    // The first exit it can slow down for.
    let mut exits: Vec<(usize, usize)> =
        all.iter().copied().filter(|e| s(e.0) > here + stopping && forward(e)).collect();
    exits.sort_by(|a, b| s(a.0).total_cmp(&s(b.0)));
    if exits.is_empty() {
        // Too fast for any of them: the last one ahead, braking harder.
        exits = all.iter().copied().filter(|e| s(e.0) > here && forward(e)).collect();
        exits.sort_by(|a, b| s(b.0).total_cmp(&s(a.0)));
    }
    if exits.is_empty() {
        // Rolled past the last exit: take the nearest one, even behind.
        exits = all;
        exits.sort_by(|a, b| (s(a.0) - here).abs().total_cmp(&(s(b.0) - here).abs()));
    }
    // Group by turn-off point, nearest first; pick the best side at the first that works.
    let mut index = 0;
    while index < exits.len() {
        let rn = exits[index].0;
        let group: Vec<usize> = exits.iter().filter(|e| e.0 == rn).map(|e| e.1).collect();
        index += exits[index..].iter().take_while(|e| e.0 == rn).count();
        let best = group
            .iter()
            .filter_map(|&first| {
                let off = route_off(map, first, runway, goal)?;
                let onward = goal.and_then(|g| map.route(first, g, &[])).map(|r| path_length(map, &r.nodes));
                Some((onward.unwrap_or(0.0) + path_length(map, &off), off))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, off)) = best else { continue };
        let (lo, hi) = (here.min(s(rn)), here.max(s(rn)));
        let mut along: Vec<usize> = on_runway.iter().copied().filter(|&n| s(n) > lo && s(n) <= hi).collect();
        if s(rn) < here {
            along.reverse();
        }
        if along.last() != Some(&rn) {
            along.push(rn);
        }
        along.extend(off);
        return Some((behind, along));
    }
    None
}

impl World {
    /// Departures waiting to enter the game: at a free parking spot in tower
    /// modes, otherwise straight to the hold line with Tower.
    pub(super) fn place_departures(&mut self) {
        let Some(ac) = self.departure_queue.front() else { return };
        let end = match &ac.vfr {
            Some(v) => v.end,
            None => self.departure_runway_for(ac.ty),
        };
        let map = self.surface();
        let tower = self.config.staffing.surface_traffic();
        let (node, heading, parking) = if tower {
            let from = map.departure_hold(end).unwrap_or(map.lineup_node[end]);
            let Some(spot) = self.pick_parking(ac, from) else { return };
            let p = &map.parking[spot];
            (p.node, p.heading, Some(spot))
        } else {
            let Some(hold) = map.departure_hold(end) else { return };
            let at = map.nodes[hold].pos;
            if self.aircraft.iter().any(|a| a.on_ground && a.pos.distance(at) < HOLD_CLEAR_NM) {
                return;
            }
            let toward = map
                .route(hold, map.lineup_node[end], &[])
                .and_then(|r| r.nodes.get(1).copied())
                .map_or(self.airport.runways[end].heading, |n| map.heading_between(hold, n));
            (hold, toward, None)
        };
        let pos = map.nodes[node].pos;
        let spot_name = parking.map(|p| map.stand_name(p));
        // The ramp tower brings it out to a spot once it has its clearance.
        let ramp = self.handling(ac) == Handling::Ramp && self.ramp_spot_for(ac, parking).is_some();
        let Some(mut ac) = self.departure_queue.pop_front() else { return };
        ac.pos = pos;
        ac.heading = heading;
        ac.lateral = Lateral::Heading(self.airport.runways[end].heading);
        ac.flow = Flow::Departure;
        ac.on_ground = true;
        ac.ias = 0.0;
        ac.ground_velocity = Vec2::ZERO;
        ac.altitude = self.airport.elevation;
        ac.spawn_time = self.time;
        ac.msaw_armed = false;
        ac.entered_sector = true;
        ac.departure_end = Some(end);
        ac.taxi = Some(Taxi::at(node));
        ac.freq_since = self.time;
        // IFR with a Delivery controller: it calls for its clearance first.
        let delivery = tower && self.config.staffing.delivery && ac.flight_plan.is_some();
        if tower {
            ac.phase = Phase::Parked;
            ac.surface_freq = if delivery { Position::Delivery } else { Position::Ground };
        } else {
            ac.phase = Phase::Taxiing;
            ac.surface_freq = Position::Local;
        }
        if delivery {
            if let Some(plan) = ac.flight_plan.as_mut() {
                plan.called_at = Some(self.time);
            }
        } else {
            self.clear_automatically(&mut ac);
        }
        if ramp {
            ac.parking = parking;
            if !delivery {
                ac.surface_freq = Position::Ramp;
            }
        }
        let spot = spot_name.unwrap_or_default();
        let call = match (&ac.vfr, &ac.flight_plan) {
            (Some(_), _) => self.vfr_ready_call(&ac, &spot),
            (None, Some(plan)) if delivery => {
                let to = if plan.destination.is_empty() { String::new() } else { format!(" to {}", plan.destination) };
                format!(
                    "Clearance, {}, at {spot}, IFR{to}, ready to copy, with information {}.",
                    ac.callsign,
                    self.weather.letter()
                )
            }
            _ => format!("Ground, {}, at {spot}, ready to taxi.", ac.callsign),
        };
        let callsign = ac.callsign.clone();
        self.add_aircraft(ac);
        if delivery || (tower && self.config.staffing.ground && !ramp) {
            self.pilot_say(Speaker::Pilot, callsign, call);
        }
    }

    /// Everything that moves on the surface, once per tick.
    pub(super) fn run_surface(&mut self) {
        self.start_exits();
        self.find_blockers();
        let mut parked = Vec::new();
        let mut takeoffs = Vec::new();
        let mut entered = Vec::new();
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            let rolling_out = ac.phase == Phase::Rollout && ac.taxi.is_some();
            if ac.phase != Phase::Taxiing && !rolling_out {
                continue;
            }
            match self.move_taxi(index) {
                Arrival::Moving(Some(zone)) => entered.push((index, zone)),
                Arrival::Moving(None) => {}
                Arrival::Parked => parked.push(index),
                Arrival::LinedUp { roll } => {
                    if roll {
                        takeoffs.push(index);
                    }
                }
            }
        }
        for (index, zone) in entered {
            self.check_incursion(index, zone);
        }
        for index in takeoffs {
            self.begin_takeoff(index);
        }
        for &index in parked.iter().rev() {
            let ac = self.aircraft.remove(index);
            // Handed to the ramp at its spot: already counted there.
            if ac.surface_freq == Position::Ramp {
                continue;
            }
            self.emergency_parked(&ac);
            self.emit(SimEvent::Parked { callsign: ac.callsign });
        }
        self.run_ramp();
        self.check_ground_conflicts();
        self.remove_cleared_arrivals();
    }

    /// On touchdown a pilot picks a turn-off and brakes for it.
    fn start_exits(&mut self) {
        for index in 0..self.aircraft.len() {
            let ac = &self.aircraft[index];
            if ac.phase != Phase::Rollout || !ac.on_ground || ac.taxi.is_some() {
                continue;
            }
            // Disabled: it stops on the runway and waits for the tow.
            if ac.emergency.as_ref().is_some_and(|e| e.stops_on_runway()) {
                continue;
            }
            let parking = if self.config.staffing.surface_traffic() {
                self.pick_parking(ac, self.surface().nearest_node(ac.pos))
            } else {
                None
            };
            let goal = self.taxi_in_goal_for(ac, parking);
            let map = self.surface();
            let Some((behind, path)) = plan_exit(map, &self.airport, ac, goal) else { continue };
            let runway = map.runway_of_end[ac.approach_runway.expect("planned an exit")];
            let now = self.time;
            let ac = &mut self.aircraft[index];
            ac.surface_freq = Position::Local;
            ac.freq_since = now;
            ac.parking = parking;
            ac.taxi = Some(Taxi { path, index: 0, node: behind, cleared: vec![runway], ..Taxi::default() });
        }
    }

    fn automated(&self, ac: &Aircraft) -> bool {
        !self.config.staffing.works(ac.frequency())
    }

    /// Direction an aircraft is taxiing, if it's going anywhere.
    fn travel(&self, ac: &Aircraft) -> Option<Vec2> {
        let taxi = ac.taxi.as_ref().filter(|_| ac.phase == Phase::Taxiing)?;
        let next = self.surface().nodes[taxi.next()?].pos;
        (next - ac.pos).try_normalize()
    }

    /// Who each taxiing aircraft has stopped for.
    fn find_blockers(&mut self) {
        let mut blockers = Vec::with_capacity(self.aircraft.len());
        for a in &self.aircraft {
            let blocker = self.travel(a).and_then(|dir| {
                self.aircraft
                    .iter()
                    .filter(|b| b.id != a.id && b.on_ground && !matches!(b.phase, Phase::Rollout | Phase::TowerRelease))
                    .filter_map(|b| {
                        let rel = b.pos - a.pos;
                        let d = rel.length();
                        if !(1e-6..FOLLOW_NM).contains(&d) || rel.dot(dir) / d < AHEAD_COS {
                            return None;
                        }
                        // Automated pilots work head-on meetings out between themselves.
                        let head_on = self.travel(b).is_some_and(|bd| bd.dot(dir) < -0.5);
                        if head_on && self.automated(a) && self.automated(b) {
                            return None;
                        }
                        Some((d, b.id))
                    })
                    .min_by(|x, y| x.0.total_cmp(&y.0))
                    .map(|(_, id)| id)
            });
            blockers.push(blocker);
        }
        // Two automated pilots waiting for each other at an angle sort it out
        // between them too: the one that has been out longest goes first.
        for i in 0..blockers.len() {
            let Some(other) = blockers[i] else { continue };
            let Some(j) = self.aircraft.iter().position(|b| b.id == other) else { continue };
            let mutual = blockers[j] == Some(self.aircraft[i].id);
            let both_automated = self.automated(&self.aircraft[i]) && self.automated(&self.aircraft[j]);
            if mutual && both_automated && self.aircraft[i].id < self.aircraft[j].id {
                blockers[i] = None;
            }
        }
        for (ac, blocker) in self.aircraft.iter_mut().zip(blockers) {
            if let Some(taxi) = ac.taxi.as_mut() {
                taxi.blocked_by = blocker;
            }
        }

        // Two aircraft each waiting for the other.
        let mut deadlocks = BTreeSet::new();
        let mut events = Vec::new();
        for a in &self.aircraft {
            let Some(b_id) = a.taxi.as_ref().and_then(|t| t.blocked_by) else { continue };
            let Some(b) = self.find(b_id) else { continue };
            if b.taxi.as_ref().and_then(|t| t.blocked_by) == Some(a.id) {
                let pair = pair_key(a.id, b.id);
                if deadlocks.insert(pair) && !self.alerts.deadlock.contains(&pair) {
                    events.push(SimEvent::TaxiDeadlock { a: a.callsign.clone(), b: b.callsign.clone() });
                }
            }
        }
        self.alerts.deadlock = deadlocks;
        for event in events {
            self.emit(event);
        }
    }

    /// Move one taxiing aircraft along its route.
    fn move_taxi(&mut self, index: usize) -> Arrival {
        // A pilot about to stop at a hold line might, very rarely, not.
        let deviates = |world: &World, node: usize| world.deviates_at_hold_line(&world.aircraft[index], node);
        let deviate_at: Option<usize> = {
            let ac = &self.aircraft[index];
            ac.taxi.as_ref().and_then(|t| t.next()).filter(|&n| deviates(self, n))
        };
        let now = self.time;
        let map = self.airport.surface.as_ref().expect("airport has a ground layout");
        let ac = &mut self.aircraft[index];
        let Some(mut taxi) = ac.taxi.take() else { return Arrival::Moving(None) };
        let at_node = ac.pos.distance(map.nodes[taxi.node].pos) < AT_NODE_NM;
        let mut entered = None;

        // At a node: may it go on to the next?
        if at_node && let Some(next) = taxi.next() {
            let here = &map.nodes[taxi.node].zones;
            taxi.holding_short = map.nodes[next]
                .zones
                .iter()
                .copied()
                .find(|z| !here.contains(z) && !taxi.cleared.contains(z));
        }

        let rollout = ac.phase == Phase::Rollout;
        let target = match taxi.next() {
            None => 0.0,
            Some(_) if taxi.holding || taxi.holding_short.is_some() || taxi.blocked_by.is_some() => 0.0,
            Some(_) => {
                // Rolling out it only slows down; taxiing it keeps to taxi speed.
                let cruise = if rollout { ac.ias.max(EXIT_SPEED) } else { TAXI_SPEED };
                let decel = if rollout { ROLLOUT_DECEL } else { PLAN_DECEL };
                let (distance, slow) = next_slowdown(map, &taxi, ac.pos, rollout);
                let allowed = (slow.min(1000.0).powi(2) + 2.0 * decel * distance * SECONDS_PER_HOUR).sqrt();
                cruise.min(allowed)
            }
        };
        if taxi.holding_short.is_some() && at_node {
            ac.ias = 0.0;
        }
        let rate = if target > ac.ias { TAXI_ACCEL } else { TAXI_BRAKE.max(ROLLOUT_DECEL) };
        ac.ias = approach(ac.ias, target, rate * TICK);

        if let Some(next) = taxi.next()
            && ac.ias > 0.0
        {
            if at_node {
                let here = &map.nodes[taxi.node].zones;
                entered = map.nodes[next].zones.iter().copied().find(|z| !here.contains(z));
            }
            let goal = map.nodes[next].pos;
            let to = goal - ac.pos;
            let distance = to.length();
            let step = ac.ias / SECONDS_PER_HOUR * TICK;
            if distance > 1e-6 {
                ac.heading = vec_heading(to);
            }
            if step >= distance {
                ac.pos = goal;
                taxi.node = next;
                taxi.index += 1;
                // Off the runway it landed on: now it's taxiing.
                let landed_on = ac.approach_runway.map(|e| map.runway_of_end[e]);
                if rollout && map.nodes[next].runway != landed_on {
                    ac.phase = Phase::Taxiing;
                }
                // Stop dead at a hold line it isn't cleared past.
                if let Some(after) = taxi.next() {
                    let here = &map.nodes[next].zones;
                    let short = map.nodes[after]
                        .zones
                        .iter()
                        .copied()
                        .find(|z| !here.contains(z) && !taxi.cleared.contains(z));
                    if let Some(runway) = short.filter(|_| deviate_at == Some(next)) {
                        // On across without a clearance.
                        taxi.cleared.push(runway);
                        let kind = super::mistakes::SlipKind::Incursion { runway, conflict: false, entered: false };
                        ac.slip = Some(super::mistakes::Slip::new(kind, now));
                        ac.force_slip = None;
                    } else if short.is_some() {
                        taxi.holding_short = short;
                        ac.ias = 0.0;
                    }
                } else {
                    ac.ias = 0.0;
                }
            } else {
                ac.pos += to / distance * step;
            }
        }
        ac.vertical_speed = 0.0;
        ac.ground_velocity = heading_vec(ac.heading) * ac.ias;

        let result = if taxi.done() && ac.pos.distance(map.nodes[taxi.node].pos) < AT_NODE_NM {
            let lined_up = ac.flow == Flow::Departure
                && ac.departure_end.is_some_and(|e| taxi.lineup || map.lineup_node[e] == taxi.node);
            let parked = ac.flow == Flow::Arrival && map.parking.iter().any(|p| p.node == taxi.node);
            if lined_up {
                let end = ac.departure_end.expect("checked");
                ac.phase = Phase::LinedUp;
                ac.heading = self.airport.runways[end].heading;
                ac.ias = 0.0;
                let roll = ac.cleared_takeoff;
                ac.in_position_since = (!roll).then_some(now);
                ac.taxi = Some(taxi);
                if roll {
                    start_roll(ac, &self.airport);
                }
                return Arrival::LinedUp { roll };
            }
            if parked { Arrival::Parked } else { Arrival::Moving(None) }
        } else {
            Arrival::Moving(entered)
        };
        ac.taxi = Some(taxi);
        result
    }

    /// An aircraft has just crossed a hold line onto a runway.
    fn check_incursion(&mut self, index: usize, zone: usize) {
        let ac = &self.aircraft[index];
        let id = ac.id;
        let callsign = ac.callsign.clone();
        let close = self.arrivals_within(zone, INCURSION_FINAL_NM, VFR_INCURSION_NM);
        // On a runway it wasn't cleared onto: (Some(already on it), conflict so far).
        let deviation = match ac.slip.as_ref().map(|s| &s.kind) {
            Some(&super::mistakes::SlipKind::Incursion { runway, entered, conflict }) if runway == zone => {
                Some((entered, conflict))
            }
            _ => None,
        };
        let map = self.surface();
        let rolling = self.aircraft.iter().any(|a| {
            a.id != id
                && a.on_ground
                && match a.phase {
                    Phase::TowerRelease => a.departure_end.is_some_and(|e| map.runway_of_end[e] == zone),
                    Phase::Rollout => a.approach_runway.is_some_and(|e| map.runway_of_end[e] == zone),
                    _ => false,
                }
        });
        if let Some((entered, was_conflict)) = deviation {
            // The pilot's doing, not the controller's: logged, and left to
            // the controller to sort out if anyone's coming.
            let conflict = was_conflict || !close.is_empty() || rolling;
            let name = map.runways[zone].name.clone();
            let now = self.time;
            if let Some(slip) = self.aircraft[index].slip.as_mut() {
                slip.kind = super::mistakes::SlipKind::Incursion { runway: zone, conflict, entered: true };
                if !entered {
                    slip.at = now;
                }
            }
            if !entered {
                let what = format!("crossed the runway {name} hold line without a clearance");
                self.emit(SimEvent::PilotDeviation { callsign, what });
            }
            return;
        }
        if close.is_empty() && !rolling {
            return;
        }
        self.emit(SimEvent::RunwayIncursion { callsign });
        for i in close {
            self.go_around(i, GoAroundReason::RunwayOccupied);
        }
    }

    fn check_ground_conflicts(&mut self) {
        let mut pairs = BTreeSet::new();
        let mut events = Vec::new();
        let on_surface = |a: &Aircraft| {
            a.on_ground && matches!(a.phase, Phase::Taxiing | Phase::Parked | Phase::LinedUp)
        };
        for i in 0..self.aircraft.len() {
            for j in (i + 1)..self.aircraft.len() {
                let (a, b) = (&self.aircraft[i], &self.aircraft[j]);
                if !on_surface(a) || !on_surface(b) || a.pos.distance(b.pos) >= CONFLICT_NM {
                    continue;
                }
                if (a.phase == Phase::Parked && b.phase == Phase::Parked)
                    || (self.automated(a) && self.automated(b))
                {
                    continue;
                }
                let pair = pair_key(a.id, b.id);
                pairs.insert(pair);
                if !self.alerts.ground.contains(&pair) {
                    events.push(SimEvent::GroundConflict { a: a.callsign.clone(), b: b.callsign.clone() });
                }
            }
        }
        self.alerts.ground = pairs;
        for event in events {
            self.emit(event);
        }
    }

    /// With nobody working the tower, arrivals leave once they're off the runway.
    fn remove_cleared_arrivals(&mut self) {
        if self.config.staffing.surface_traffic() {
            return;
        }
        let map = self.airport.surface.as_ref().expect("airport has a ground layout");
        self.aircraft.retain(|a| {
            let clear = a.flow == Flow::Arrival
                && a.phase == Phase::Taxiing
                && a.taxi.as_ref().is_some_and(|t| map.nodes[t.node].zones.is_empty());
            !clear
        });
    }
}

/// What happened to a taxiing aircraft this tick.
enum Arrival {
    /// Still going; entered this runway's zone if `Some`.
    Moving(Option<usize>),
    Parked,
    LinedUp { roll: bool },
}
