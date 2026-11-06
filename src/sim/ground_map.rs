//! The airport surface: taxiways, runway centerlines, hold lines and parking,
//! built into a graph for taxi routing and runway protection.
//!
//! Data lists taxiways as polylines. Vertices closer than [`MERGE_NM`] become one
//! node, so taxiways that share a vertex are connected. A vertex on a runway
//! centerline becomes a runway node, and runway nodes are joined along the runway.
//!
//! Each runway has a protected zone: its centerline nodes plus every taxiway node
//! between it and its hold lines. Entering the zone needs a clearance for that
//! runway, and anything in the zone occupies it.

use super::airspace::Runway;
use super::units::{FT_PER_NM, heading_vec, vec_heading};
use glam::Vec2;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Vertices closer than this are the same node, NM (about 12 ft).
const MERGE_NM: f32 = 0.002;
/// A vertex this close to a centerline is on the runway, NM (about 25 ft).
const ON_RUNWAY_NM: f32 = 0.004;
/// A taxiway must reach a hold line within this distance of a runway, NM.
const MAX_HOLD_DISTANCE_NM: f32 = 0.2;
/// Extra routing cost for taxiing along a runway instead of a parallel taxiway.
const RUNWAY_TAXI_FACTOR: f32 = 3.0;
/// Extra routing cost for each runway entered, NM.
const RUNWAY_ENTRY_COST_NM: f32 = 0.3;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GroundData {
    pub taxiways: Vec<TaxiwayData>,
    #[serde(default)]
    pub holds: Vec<HoldData>,
    #[serde(default)]
    pub parking: Vec<ParkingData>,
}

/// A taxiway centerline. An empty name is an apron taxilane, never named in clearances.
#[derive(Debug, Clone, Deserialize)]
pub struct TaxiwayData {
    #[serde(default)]
    pub name: String,
    pub points: Vec<[f32; 2]>,
}

/// A runway holding position. `runway` is either end's id.
#[derive(Debug, Clone, Deserialize)]
pub struct HoldData {
    pub at: [f32; 2],
    pub runway: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParkingData {
    pub name: String,
    pub at: [f32; 2],
    /// True heading when parked.
    pub heading: f32,
}

/// Both ends of one strip of pavement.
#[derive(Debug, Clone)]
pub struct PhysicalRunway {
    /// Such as "09R/27L".
    pub name: String,
    /// Indices into `Airport::runways`.
    pub ends: Vec<usize>,
    /// Pavement ends, in the direction of `ends[0]`.
    pub start: Vec2,
    pub end: Vec2,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgeKind {
    /// Taxiway name, empty for an apron taxilane.
    Taxiway(String),
    /// Along a runway's centerline. Index into `GroundMap::runways`.
    Runway(usize),
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    pub kind: EdgeKind,
    pub length: f32,
}

impl Edge {
    pub fn other(&self, node: usize) -> usize {
        if node == self.a { self.b } else { self.a }
    }

    pub fn taxiway(&self) -> Option<&str> {
        match &self.kind {
            EdgeKind::Taxiway(name) => Some(name),
            EdgeKind::Runway(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub pos: Vec2,
    /// Physical runway whose centerline this is on.
    pub runway: Option<usize>,
    /// Physical runway this is a hold line for.
    pub hold: Option<usize>,
    /// Physical runways whose protected zone includes this node.
    pub zones: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Parking {
    pub name: String,
    pub node: usize,
    pub heading: f32,
}

#[derive(Debug, Clone)]
pub struct GroundMap {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Edge indices touching each node.
    pub adjacent: Vec<Vec<usize>>,
    pub runways: Vec<PhysicalRunway>,
    /// Physical runway for each `Airport::runways` index.
    pub runway_of_end: Vec<usize>,
    /// Node where each runway end's takeoff roll starts.
    pub lineup_node: Vec<usize>,
    pub parking: Vec<Parking>,
}

/// A taxi route and what a controller would say about it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaxiRoute {
    pub nodes: Vec<usize>,
    /// Named taxiways in order, without repeats.
    pub taxiways: Vec<String>,
    /// Physical runways entered on the way, in order.
    pub runways_entered: Vec<usize>,
}

fn pos(p: [f32; 2]) -> Vec2 {
    Vec2::new(p[0], p[1])
}

/// Group runway ends that share a centerline and point opposite ways.
fn pair_runways(runways: &[Runway]) -> (Vec<PhysicalRunway>, Vec<usize>) {
    let mut physical: Vec<PhysicalRunway> = Vec::new();
    let mut of_end = vec![usize::MAX; runways.len()];
    for (i, r) in runways.iter().enumerate() {
        if of_end[i] != usize::MAX {
            continue;
        }
        let mut ends = vec![i];
        for (j, other) in runways.iter().enumerate().skip(i + 1) {
            let opposite = (super::units::heading_diff(r.heading, other.heading).abs() - 180.0).abs() < 2.0;
            if opposite && of_end[j] == usize::MAX && r.cross(other.threshold()).abs() < 0.01 {
                ends.push(j);
            }
        }
        let dir = heading_vec(r.heading);
        let points: Vec<Vec2> = ends
            .iter()
            .flat_map(|&e| [runways[e].threshold(), runways[e].takeoff_point(), runways[e].far_end()])
            .collect();
        let origin = r.threshold();
        let along = |p: Vec2| (p - origin).dot(dir);
        let min = points.iter().map(|&p| along(p)).fold(f32::INFINITY, f32::min);
        let max = points.iter().map(|&p| along(p)).fold(f32::NEG_INFINITY, f32::max);
        let name = ends.iter().map(|&e| runways[e].id.as_str()).collect::<Vec<_>>().join("/");
        for &e in &ends {
            of_end[e] = physical.len();
        }
        physical.push(PhysicalRunway {
            name,
            ends,
            start: origin + dir * min,
            end: origin + dir * max,
        });
    }
    (physical, of_end)
}

impl PhysicalRunway {
    fn direction(&self) -> Vec2 {
        (self.end - self.start).normalize()
    }

    /// Distance along from `start`, and distance off the centerline.
    fn locate(&self, p: Vec2) -> (f32, f32) {
        let dir = self.direction();
        let rel = p - self.start;
        (rel.dot(dir), rel.perp_dot(dir).abs())
    }

    fn length(&self) -> f32 {
        (self.end - self.start).length()
    }
}

#[derive(Copy, Clone, PartialEq)]
struct Visit {
    cost: f32,
    state: usize,
}

impl Eq for Visit {}

impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl GroundMap {
    pub fn build(data: &GroundData, runways: &[Runway]) -> Result<GroundMap, String> {
        let (physical, runway_of_end) = pair_runways(runways);
        let mut map = GroundMap {
            nodes: Vec::new(),
            edges: Vec::new(),
            adjacent: Vec::new(),
            runways: physical,
            runway_of_end,
            lineup_node: Vec::new(),
            parking: Vec::new(),
        };

        // Taxiway vertices and edges.
        for taxiway in &data.taxiways {
            if taxiway.points.len() < 2 {
                return Err(format!("taxiway {:?} needs at least two points", taxiway.name));
            }
            let ids: Vec<usize> = taxiway.points.iter().map(|&p| map.node_at(pos(p))).collect();
            for pair in ids.windows(2) {
                if pair[0] != pair[1] {
                    map.add_edge(pair[0], pair[1], EdgeKind::Taxiway(taxiway.name.clone()));
                }
            }
        }

        // Runway ends become nodes so every takeoff has a place to line up.
        for runway in runways {
            let node = map.node_at(runway.takeoff_point());
            map.lineup_node.push(node);
        }

        // Nodes on a centerline belong to that runway, joined in order along it.
        for (index, runway) in map.runways.clone().iter().enumerate() {
            let mut on: Vec<(f32, usize)> = Vec::new();
            for (n, node) in map.nodes.iter().enumerate() {
                let (along, off) = runway.locate(node.pos);
                if off <= ON_RUNWAY_NM && along >= -MERGE_NM && along <= runway.length() + MERGE_NM {
                    on.push((along, n));
                }
            }
            on.sort_by(|a, b| a.0.total_cmp(&b.0));
            for &(_, n) in &on {
                map.nodes[n].runway = Some(index);
            }
            for pair in on.windows(2) {
                map.add_edge(pair[0].1, pair[1].1, EdgeKind::Runway(index));
            }
        }

        for hold in &data.holds {
            let node = map
                .node_for_marker(pos(hold.at))
                .ok_or_else(|| format!("hold for {} at {:?} is not on a taxiway", hold.runway, hold.at))?;
            let end = runways
                .iter()
                .position(|r| r.id == hold.runway)
                .ok_or_else(|| format!("hold references unknown runway {}", hold.runway))?;
            map.nodes[node].hold = Some(map.runway_of_end[end]);
        }

        for spot in &data.parking {
            let node = map
                .node_for_marker(pos(spot.at))
                .ok_or_else(|| format!("parking {} is not on a taxiway", spot.name))?;
            map.parking.push(Parking { name: spot.name.clone(), node, heading: spot.heading });
        }

        map.build_zones()?;
        map.validate()?;
        Ok(map)
    }

    fn find_node(&self, p: Vec2) -> Option<usize> {
        self.nodes.iter().position(|n| n.pos.distance(p) < MERGE_NM)
    }

    /// Node a hold or parking spot sits on. Merging can move a node a little from
    /// any one vertex it absorbed, so this allows twice the merge distance.
    fn node_for_marker(&self, p: Vec2) -> Option<usize> {
        let nearest = self.nearest_node(p);
        (self.nodes[nearest].pos.distance(p) < MERGE_NM * 2.0).then_some(nearest)
    }

    fn node_at(&mut self, p: Vec2) -> usize {
        if let Some(n) = self.find_node(p) {
            return n;
        }
        self.nodes.push(Node { pos: p, runway: None, hold: None, zones: Vec::new() });
        self.adjacent.push(Vec::new());
        self.nodes.len() - 1
    }

    fn add_edge(&mut self, a: usize, b: usize, kind: EdgeKind) {
        let exists = self.adjacent[a].iter().any(|&e| self.edges[e].other(a) == b);
        if exists {
            return;
        }
        let length = self.nodes[a].pos.distance(self.nodes[b].pos);
        self.edges.push(Edge { a, b, kind, length });
        let id = self.edges.len() - 1;
        self.adjacent[a].push(id);
        self.adjacent[b].push(id);
    }

    /// Mark each runway's zone: out from the centerline until its hold lines.
    fn build_zones(&mut self) -> Result<(), String> {
        for runway in 0..self.runways.len() {
            let mut distance = vec![f32::INFINITY; self.nodes.len()];
            let mut queue: Vec<usize> = Vec::new();
            for (n, node) in self.nodes.iter().enumerate() {
                if node.runway == Some(runway) {
                    distance[n] = 0.0;
                    queue.push(n);
                }
            }
            while let Some(n) = queue.pop() {
                self.nodes[n].zones.push(runway);
                for &e in &self.adjacent[n] {
                    let next = self.edges[e].other(n);
                    let node = &self.nodes[next];
                    // Stop at the hold line, and at other runways (they have their own zones).
                    let stops = node.hold == Some(runway) || node.runway.is_some_and(|r| r != runway);
                    if node.runway == Some(runway) || stops {
                        continue;
                    }
                    let d = distance[n] + self.edges[e].length;
                    if d >= distance[next] {
                        continue;
                    }
                    if d > MAX_HOLD_DISTANCE_NM {
                        return Err(format!(
                            "a taxiway leaves runway {} at ({:.3}, {:.3}) without a hold line",
                            self.runways[runway].name, node.pos.x, node.pos.y
                        ));
                    }
                    distance[next] = d;
                    queue.push(next);
                }
            }
            for node in &mut self.nodes {
                node.zones.dedup();
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        if self.parking.is_empty() {
            return Err("ground layout needs at least one parking spot".into());
        }
        for (end, &node) in self.lineup_node.iter().enumerate() {
            let hold = self
                .departure_hold(end)
                .ok_or_else(|| format!("runway end {end} has no hold line to depart from"))?;
            for spot in &self.parking {
                if self.route(spot.node, hold, &[]).is_none() {
                    return Err(format!("parking {} can't reach the hold for runway end {end}", spot.name));
                }
            }
            if self.route(hold, node, &[]).is_none() {
                return Err(format!("hold for runway end {end} can't reach the runway"));
            }
        }
        for runway in 0..self.runways.len() {
            let exits = self.exits(runway);
            if exits.is_empty() {
                return Err(format!("runway {} has no exits", self.runways[runway].name));
            }
        }
        Ok(())
    }

    fn near_runway(&self, node: usize) -> bool {
        let n = &self.nodes[node];
        n.hold.is_some() || !n.zones.is_empty()
    }

    pub fn in_zone(&self, node: usize, runway: usize) -> bool {
        self.nodes[node].zones.contains(&runway)
    }

    /// Hold line to wait at before departing from a runway end: the one nearest its takeoff point.
    pub fn departure_hold(&self, end: usize) -> Option<usize> {
        let runway = self.runway_of_end[end];
        let start = self.nodes[self.lineup_node[end]].pos;
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.hold == Some(runway))
            .min_by(|a, b| a.1.pos.distance(start).total_cmp(&b.1.pos.distance(start)))
            .map(|(i, _)| i)
    }

    /// The way onto a runway end for takeoff, from `origin`. Pilots line up
    /// where their taxi route first meets the runway, the nearest point that
    /// leaves at least `min_runway_ft` ahead, rather than taxiing down the
    /// runway to use all of it. A jet at an intersection too short for it goes
    /// on to the end's full-length line-up point.
    pub fn lineup_route(&self, runways: &[Runway], end: usize, origin: usize, min_runway_ft: f32) -> Option<Vec<usize>> {
        let physical = self.runway_of_end[end];
        let runway = &runways[end];
        let ahead_ft = |n: usize| (runway.far_end() - self.nodes[n].pos).dot(heading_vec(runway.heading)) * FT_PER_NM;
        let length = |nodes: &[usize]| -> f32 {
            nodes.windows(2).map(|w| self.nodes[w[0]].pos.distance(self.nodes[w[1]].pos)).sum()
        };
        let full = self.lineup_node[end];
        (0..self.nodes.len())
            .filter(|&n| n == full || (self.nodes[n].runway == Some(physical) && ahead_ft(n) >= min_runway_ft))
            .filter_map(|n| {
                let nodes = self.route(origin, n, &[])?.nodes;
                // Straight onto the runway there, not along it from somewhere else.
                let first_on = nodes.iter().position(|&m| self.nodes[m].runway == Some(physical))?;
                (n == full || first_on + 1 == nodes.len()).then_some(nodes)
            })
            .min_by(|a, b| length(a).total_cmp(&length(b)))
    }

    /// Runway nodes with a taxiway leading off them, as (runway node, first taxiway node).
    pub fn exits(&self, runway: usize) -> Vec<(usize, usize)> {
        let mut exits = Vec::new();
        for (n, node) in self.nodes.iter().enumerate() {
            if node.runway != Some(runway) {
                continue;
            }
            for &e in &self.adjacent[n] {
                let other = self.edges[e].other(n);
                // Some taxiway data runs along the centerline for a stretch;
                // that's still the runway, not a way off it.
                if let EdgeKind::Taxiway(_) = self.edges[e].kind
                    && self.nodes[other].runway != Some(runway)
                {
                    exits.push((n, other));
                }
            }
        }
        exits
    }

    /// Cheapest route from one node to another. `via` names taxiways that must be
    /// used in that order; apron taxilanes and runway crossings are always allowed.
    pub fn route(&self, from: usize, to: usize, via: &[String]) -> Option<TaxiRoute> {
        let stages = via.len().max(1);
        let state = |node: usize, stage: usize| node * stages + stage;
        let mut cost = vec![f32::INFINITY; self.nodes.len() * stages];
        let mut previous: Vec<Option<usize>> = vec![None; self.nodes.len() * stages];
        let mut heap = BinaryHeap::new();
        let start_stage = 0;
        cost[state(from, start_stage)] = 0.0;
        heap.push(Visit { cost: 0.0, state: state(from, start_stage) });

        let goal_stage = if via.is_empty() { 0 } else { via.len() - 1 };
        let mut found = None;
        while let Some(Visit { cost: c, state: s }) = heap.pop() {
            if c > cost[s] {
                continue;
            }
            let (node, stage) = (s / stages, s % stages);
            if node == to && stage == goal_stage {
                found = Some(s);
                break;
            }
            for &e in &self.adjacent[node] {
                let edge = &self.edges[e];
                let next = edge.other(node);
                let next_stage = match (&edge.kind, via.is_empty()) {
                    (_, true) => 0,
                    (EdgeKind::Taxiway(name), false) if name.is_empty() => stage,
                    (EdgeKind::Taxiway(name), false) if *name == via[stage] => stage,
                    (EdgeKind::Taxiway(name), false)
                        if stage + 1 < via.len() && *name == via[stage + 1] =>
                    {
                        stage + 1
                    }
                    // Connectors to and across a runway are implied by the clearance.
                    (EdgeKind::Taxiway(_), false)
                        if self.near_runway(node) || self.near_runway(next) =>
                    {
                        stage
                    }
                    (EdgeKind::Taxiway(_), false) => continue,
                    (EdgeKind::Runway(_), false) => stage,
                };
                let mut step = edge.length;
                if matches!(edge.kind, EdgeKind::Runway(_)) {
                    step *= RUNWAY_TAXI_FACTOR;
                }
                let entering = self.nodes[next]
                    .zones
                    .iter()
                    .any(|z| !self.nodes[node].zones.contains(z));
                if entering {
                    step += RUNWAY_ENTRY_COST_NM;
                }
                let ns = state(next, next_stage);
                let nc = c + step;
                if nc < cost[ns] {
                    cost[ns] = nc;
                    previous[ns] = Some(s);
                    heap.push(Visit { cost: nc, state: ns });
                }
            }
        }

        let mut s = found?;
        let mut nodes = vec![s / stages];
        while let Some(p) = previous[s] {
            nodes.push(p / stages);
            s = p;
        }
        nodes.reverse();
        Some(self.describe(nodes))
    }

    fn edge_between(&self, a: usize, b: usize) -> Option<&Edge> {
        self.adjacent[a].iter().map(|&e| &self.edges[e]).find(|e| e.other(a) == b)
    }

    fn describe(&self, nodes: Vec<usize>) -> TaxiRoute {
        let mut taxiways: Vec<String> = Vec::new();
        let mut runways_entered = Vec::new();
        for pair in nodes.windows(2) {
            if let Some(name) = self.edge_between(pair[0], pair[1]).and_then(|e| e.taxiway())
                && !name.is_empty()
                && taxiways.last().is_none_or(|last| last != name)
            {
                taxiways.push(name.to_string());
            }
            for &z in &self.nodes[pair[1]].zones {
                if !self.nodes[pair[0]].zones.contains(&z) {
                    runways_entered.push(z);
                }
            }
        }
        TaxiRoute { nodes, taxiways, runways_entered }
    }

    /// Nearest node to a point, for clicks and spawning.
    pub fn nearest_node(&self, p: Vec2) -> usize {
        self.nodes
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.pos.distance(p).total_cmp(&b.1.pos.distance(p)))
            .map(|(i, _)| i)
            .expect("ground map has nodes")
    }

    /// True heading from one node to the next.
    pub fn heading_between(&self, a: usize, b: usize) -> f32 {
        vec_heading(self.nodes[b].pos - self.nodes[a].pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::airspace::Airport;

    fn harrowgate() -> Airport {
        Airport::default_airport()
    }

    fn map() -> GroundMap {
        harrowgate().surface.expect("Harrowgate has a ground layout")
    }

    fn node(map: &GroundMap, x: f32, y: f32) -> usize {
        map.find_node(Vec2::new(x, y)).unwrap_or_else(|| panic!("no node at {x}, {y}"))
    }

    fn parking(map: &GroundMap, name: &str) -> usize {
        map.parking.iter().find(|p| p.name == name).unwrap().node
    }

    fn end(airport: &Airport, id: &str) -> usize {
        airport.runway_index(id).unwrap()
    }

    #[test]
    fn runway_ends_pair_into_physical_runways() {
        let a = harrowgate();
        let m = map();
        assert_eq!(m.runways.len(), 2);
        assert_eq!(m.runway_of_end[end(&a, "27L")], m.runway_of_end[end(&a, "09R")]);
        assert_ne!(m.runway_of_end[end(&a, "27L")], m.runway_of_end[end(&a, "27R")]);
        assert!(m.runways.iter().any(|r| r.name == "27L/09R"), "{:?}", m.runways);
    }

    #[test]
    fn hold_lines_bound_the_runway_zone() {
        let a = harrowgate();
        let m = map();
        let south = m.runway_of_end[end(&a, "27L")];
        let hold = node(&m, 0.0, -0.5);
        assert_eq!(m.nodes[hold].hold, Some(south));
        assert!(!m.in_zone(hold, south), "holding short is outside the zone");
        assert!(m.in_zone(node(&m, 0.0, -0.6), south));
        assert!(!m.in_zone(node(&m, 0.0, -0.4), south));
    }

    #[test]
    fn gate_to_departure_runway_uses_the_parallel_taxiway_and_crosses_nothing() {
        let a = harrowgate();
        let m = map();
        let hold = m.departure_hold(end(&a, "27R")).unwrap();
        assert!(m.nodes[hold].pos.distance(Vec2::new(1.5, 0.5)) < 1e-3, "{:?}", m.nodes[hold].pos);
        let route = m.route(parking(&m, "G1"), hold, &[]).unwrap();
        assert_eq!(route.taxiways.last().map(String::as_str), Some("B5"), "{route:?}");
        assert!(route.taxiways.contains(&"B".to_string()), "{route:?}");
        assert!(route.runways_entered.is_empty(), "{route:?}");
    }

    #[test]
    fn cargo_ramp_route_crosses_the_south_runway() {
        let a = harrowgate();
        let m = map();
        let hold = m.departure_hold(end(&a, "27R")).unwrap();
        let route = m.route(parking(&m, "C1"), hold, &[]).unwrap();
        assert_eq!(route.runways_entered, vec![m.runway_of_end[end(&a, "27L")]], "{route:?}");
        assert_eq!(route.taxiways.first().map(String::as_str), Some("E"), "{route:?}");
    }

    #[test]
    fn via_forces_the_named_taxiways_in_order() {
        let a = harrowgate();
        let m = map();
        let hold = m.departure_hold(end(&a, "27R")).unwrap();
        let free = m.route(parking(&m, "G1"), hold, &[]).unwrap();
        assert!(!free.taxiways.contains(&"C".to_string()), "{free:?}");
        let via: Vec<String> = ["C", "B"].iter().map(|s| s.to_string()).collect();
        let forced = m.route(parking(&m, "G1"), hold, &via).unwrap();
        let c = forced.taxiways.iter().position(|t| t == "C").expect("uses C");
        let b = forced.taxiways.iter().position(|t| t == "B").expect("uses B");
        assert!(c < b, "{forced:?}");
        // A taxiway that doesn't exist can't be followed.
        assert!(m.route(parking(&m, "G1"), hold, &["Q".to_string()]).is_none());
    }

    #[test]
    fn taxiing_along_a_runway_is_a_last_resort() {
        let m = map();
        let from = node(&m, -1.5, -0.4);
        let to = node(&m, 1.5, -0.4);
        let route = m.route(from, to, &[]).unwrap();
        assert!(route.runways_entered.is_empty(), "{route:?}");
        assert_eq!(route.taxiways, vec!["A".to_string()]);
    }

    #[test]
    fn every_runway_has_exits_and_lineup_points() {
        let a = harrowgate();
        let m = map();
        for runway in 0..m.runways.len() {
            assert!(m.exits(runway).len() >= 5, "{}", m.runways[runway].name);
        }
        for (e, &n) in m.lineup_node.iter().enumerate() {
            assert!(m.nodes[n].pos.distance(a.runways[e].takeoff_point()) < 1e-3);
            assert_eq!(m.nodes[n].runway, Some(m.runway_of_end[e]));
        }
    }

    #[test]
    fn a_taxiway_onto_a_runway_without_a_hold_line_is_rejected() {
        let a = harrowgate();
        let mut data = a.ground.clone().unwrap();
        data.holds.retain(|h| h.at != [0.0, -0.5]);
        let err = GroundMap::build(&data, &a.runways).unwrap_err();
        assert!(err.contains("without a hold line"), "{err}");
    }

    #[test]
    fn unreachable_parking_is_rejected() {
        let a = harrowgate();
        let mut data = a.ground.clone().unwrap();
        data.taxiways.push(TaxiwayData { name: String::new(), points: vec![[5.0, 5.0], [5.1, 5.0]] });
        data.parking.push(ParkingData { name: "X".into(), at: [5.1, 5.0], heading: 0.0 });
        let err = GroundMap::build(&data, &a.runways).unwrap_err();
        assert!(err.contains("parking X"), "{err}");
    }
}
