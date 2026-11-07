//! The flight strip bay: FAA terminal flight progress strips in the JO 7110.65
//! format (para 2-3-3, Forms 7230-7.1 departure and 7230-7.2 arrival).
//!
//! A strip has numbered blocks. Blocks 1 to 9c are the order's; blocks 10 to
//! 18 are set by each facility, and this game's "facility directive" is in
//! `marks`. What the flight data computer prints stays fixed once the
//! strip is printed; everything a controller writes afterwards is a pen mark,
//! struck through when it's superseded.
//!
//! ```text
//! +--------------+------+---------+--------------------------+----+----+----+
//! | 1 NVA101   2 | 6    | 8       | 9                        | 10 | 13 | 16 |
//! | 3 H/B744/L   | 7    | 8a      |                          | 11 | 14 | 17 |
//! | 4 512  5 2731| 2a   | 8b      |  9a       9b       9c    | 12 | 15 | 18 |
//! +--------------+------+---------+--------------------------+----+----+----+
//! ```

use super::camera::PANEL_WIDTH;
use super::input::UiState;
use crate::sim::World;
use crate::sim::aircraft::{Aircraft, Flow, Phase, VfrIntent};
use crate::sim::clearance::{Clearance, spoken_runway};
use crate::sim::delivery;
use crate::sim::emergency::EmergencyKind;
use crate::sim::positions::Position;
use crate::sim::units::vec_heading;
use macroquad::prelude::*;
use std::collections::HashMap;

const PAPER: Color = Color::new(0.87, 0.85, 0.77, 1.0);
/// Another position's strip: still readable, clearly not yours.
const PAPER_OTHER: Color = Color::new(0.52, 0.52, 0.49, 1.0);
const INK: Color = Color::new(0.08, 0.08, 0.10, 1.0);
/// Handwriting: a controller's blue pen.
const PEN: Color = Color::new(0.07, 0.20, 0.62, 1.0);
const RED_PEN: Color = Color::new(0.78, 0.07, 0.07, 1.0);
const RULE: Color = Color::new(0.40, 0.38, 0.33, 1.0);
const HOLDER_DEPARTURE: Color = Color::new(0.25, 0.48, 0.90, 1.0);
const HOLDER_ARRIVAL: Color = Color::new(0.92, 0.66, 0.20, 1.0);
const HOLDER_VFR: Color = Color::new(0.30, 0.72, 0.36, 1.0);
const HOLDER_EMERGENCY: Color = Color::new(0.95, 0.20, 0.18, 1.0);
const SELECTED: Color = Color::new(0.98, 0.95, 0.40, 1.0);
/// Frame round a strip about to cost points.
const URGENT: Color = Color::new(1.00, 0.55, 0.10, 1.0);
const BAY_TEXT: Color = Color::new(0.48, 0.56, 0.56, 1.0);
const BAY_RULE: Color = Color::new(0.22, 0.27, 0.28, 1.0);
const DIM: Color = Color::new(0.55, 0.62, 0.62, 1.0);

/// The strip and its holder fill the panel less this margin each side.
const MARGIN: f32 = 4.0;
const HOLDER: f32 = 6.0;
const ROW: f32 = 17.0;
pub const STRIP_HEIGHT: f32 = ROW * 3.0 + 2.0;
/// A strip folded to one row.
const COMPACT_HEIGHT: f32 = 20.0;
const GAP: f32 = 3.0;
const BAY_HEADER: f32 = 18.0;
const FONT: f32 = 13.0;
const SMALL: f32 = 12.0;
/// Column widths left to right, after the holder. Block 9 takes what's left.
const COL_ID: f32 = 96.0;
const COL_6: f32 = 46.0;
const COL_8: f32 = 44.0;
const GRID_CELL: f32 = 28.0;

/// Which form the strip is printed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// FAA Form 7230-7.1.
    Departure,
    /// FAA Form 7230-7.2.
    Arrival,
}

/// One entry in a block: printed, or written in pen, and maybe struck through.
#[derive(Debug, Clone, PartialEq)]
pub struct Pen {
    pub text: String,
    pub hand: bool,
    pub struck: bool,
    /// Written in red: emergencies.
    pub red: bool,
}

impl Pen {
    fn printed(text: impl Into<String>) -> Pen {
        Pen {
            text: text.into(),
            hand: false,
            struck: false,
            red: false,
        }
    }
    fn hand(text: impl Into<String>) -> Pen {
        Pen {
            text: text.into(),
            hand: true,
            struck: false,
            red: false,
        }
    }
    fn red(text: impl Into<String>) -> Pen {
        Pen {
            text: text.into(),
            hand: true,
            struck: false,
            red: true,
        }
    }
    fn struck(mut self, struck: bool) -> Pen {
        self.struck = struck;
        self
    }
}

/// What the flight data printer put on the strip. Fixed once printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Printed {
    pub form: Form,
    /// Block 2a: the position the strip was printed for.
    pub originator: &'static str,
    /// Block 3: heavy indicator, type and equipment suffix.
    pub ty: String,
    /// Block 4: computer identification number.
    pub cid: String,
    /// Block 5: assigned beacon code.
    pub beacon: String,
    /// Block 6: proposed departure time (departures) or previous fix (arrivals).
    pub b6: String,
    /// Block 7: requested altitude (departures) or coordination fix (arrivals).
    pub b7: String,
    /// Block 8: departure airport (departures) or ETA at the coordination fix (arrivals).
    pub b8: String,
    /// Block 9, printed part: route and destination (departures) or altitude (arrivals).
    pub b9: String,
    /// Block 9, printed remarks after the altitude (arrivals: the STAR).
    pub remarks: String,
    /// ATIS letter current when it was printed, for block 11.
    pub atis: String,
}

/// The strip bay's memory between frames: printed strips and runway history.
#[derive(Debug, Default)]
pub struct Board {
    printed: HashMap<u32, Printed>,
    /// Runways assigned, oldest first, for block 10.
    runways: HashMap<u32, Vec<String>>,
    /// The strip bay fills the whole panel (F3).
    pub expanded: bool,
    /// How far the bay is scrolled down, pixels.
    pub scroll: f32,
}

impl Board {
    /// The strip as printed, printing it now if it's new.
    fn strip(&mut self, world: &World, ac: &Aircraft, clock_start: f64) -> &Printed {
        self.printed
            .entry(ac.id)
            .or_insert_with(|| print(world, ac, clock_start))
    }

    /// Note the runway it has now, keeping earlier ones to strike through.
    fn runway_history(&mut self, ac: &Aircraft, runway: Option<String>) -> &[String] {
        let list = self.runways.entry(ac.id).or_default();
        if let Some(r) = runway
            && list.last() != Some(&r)
        {
            list.push(r);
        }
        list
    }

    /// Forget aircraft that have gone.
    fn prune(&mut self, world: &World) {
        if self.printed.len() > world.aircraft.len() + 16 {
            let live: std::collections::HashSet<u32> =
                world.aircraft.iter().map(|a| a.id).collect();
            self.printed.retain(|id, _| live.contains(id));
            self.runways.retain(|id, _| live.contains(id));
        }
    }
}

/// HHMM UTC for a sim time.
fn hhmm(clock_start: f64, time: f32) -> String {
    let utc = (clock_start + time as f64).rem_euclid(86_400.0) as u64;
    format!("{:02}{:02}", utc / 3600, utc / 60 % 60)
}

/// A discrete beacon code, octal digits, from the aircraft's id. Center
/// assigns arrivals codes from its range; the TRACON gives departures its own.
/// Never 1200 or a 7xxx special code.
fn beacon(ac: &Aircraft) -> String {
    if ac.vfr.is_some() {
        return "1200".into();
    }
    if let Some(plan) = &ac.flight_plan {
        return delivery::code_text(plan.squawk);
    }
    match ac.flow {
        Flow::Arrival => format!("{:04o}", 0o2000 + (ac.id.wrapping_mul(29) % 0o4000)),
        Flow::Departure => delivery::code_text(delivery::departure_code(ac.id)),
    }
}

fn star_name(world: &World, ac: &Aircraft) -> Option<String> {
    let first = world.airport.fixes.get(*ac.route.first()?)?.name.clone();
    world
        .airport
        .stars
        .iter()
        .find(|s| s.fixes.first() == Some(&first))
        .and_then(|s| s.name.clone())
}

/// The departure's route as filed: SID and exit fix, or the fixes if the SID
/// has no name.
fn departure_route(world: &World, ac: &Aircraft) -> String {
    let names: Vec<String> = ac
        .route
        .iter()
        .filter_map(|&f| world.airport.fixes.get(f))
        .map(|f| f.name.clone())
        .collect();
    let sid = world
        .airport
        .sids
        .iter()
        .find(|s| s.fixes == names)
        .and_then(|s| s.name.clone());
    match (sid, names.last()) {
        (Some(sid), Some(exit)) => format!("{sid} {exit}"),
        _ => names.join(" "),
    }
}

/// The eight compass points as a controller abbreviates them.
fn compass_short(bearing: f32) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[((crate::sim::units::normalize_heading(bearing) + 22.5) / 45.0) as usize % 8]
}

/// Print a strip: everything the flight data computer knows at this moment.
pub fn print(world: &World, ac: &Aircraft, clock_start: f64) -> Printed {
    let airport = &world.airport;
    let heavy = if ac.ty.wake == crate::sim::performance::WakeCategory::Heavy {
        "H/"
    } else {
        ""
    };
    let ty = format!("{heavy}{}/{}", ac.ty.designator, ac.ty.equipment_suffix());
    let cid = format!("{:03}", (ac.id.wrapping_mul(37) + 101) % 1000);
    let atis = world.weather.letter().to_string();
    let vfr = ac.vfr.as_ref();
    if ac.flow == Flow::Departure {
        let exit = ac.exit_fix.and_then(|f| airport.fixes.get(f));
        let bearing = match (vfr, exit) {
            (Some(v), _) => v.outbound,
            (None, Some(fix)) => vec_heading(fix.pos()),
            (None, None) => 0.0,
        };
        let destination = airport
            .city_pair(bearing, ac.id)
            .unwrap_or_default()
            .to_string();
        let plan = ac.flight_plan.as_ref();
        let destination = plan.map_or(destination, |p| p.destination.clone());
        let b9 = match (vfr.map(|v| v.intent), plan) {
            (Some(VfrIntent::Closed), _) => "CLOSED TFC".to_string(),
            (Some(_), _) => format!("{} {destination}", compass_short(bearing)),
            (None, Some(p)) => format!("{} {destination}", delivery::route_short(airport, &p.route))
                .trim()
                .to_string(),
            (None, None) => format!("{} {destination}", departure_route(world, ac))
                .trim()
                .to_string(),
        };
        Printed {
            form: Form::Departure,
            originator: if vfr.is_some() { "LC" } else { "CD" },
            ty,
            cid,
            beacon: beacon(ac),
            b6: format!("P{}", hhmm(clock_start, ac.spawn_time)),
            b7: match (vfr, plan) {
                (Some(_), _) => "VFR".into(),
                (None, Some(p)) => format!("{:03.0}", p.altitude / 100.0),
                (None, None) => format!("{:03}", ac.ty.filed_altitude()),
            },
            b8: airport.code.clone(),
            b9,
            remarks: String::new(),
            atis,
        }
    } else {
        let gate = ac.route.first().and_then(|&f| airport.fixes.get(f));
        let from = gate.map(|f| f.pos()).unwrap_or(ac.pos);
        // A flight plan straight to the coordination fix: the fix before it is
        // the departure airport.
        let origin = airport
            .city_pair(vec_heading(from), ac.id)
            .unwrap_or_default()
            .to_string();
        let eta = match gate {
            Some(fix) if ac.route_index == 0 && !ac.on_ground => {
                let gs = ac.ground_velocity.length().max(60.0);
                world.time + ac.pos.distance(fix.pos()) / gs * 3600.0
            }
            _ => world.time,
        };
        Printed {
            form: Form::Arrival,
            originator: if vfr.is_some() { "LC" } else { "AR" },
            ty,
            cid,
            beacon: beacon(ac),
            b6: origin,
            b7: gate.map(|f| f.name.clone()).unwrap_or_default(),
            b8: hhmm(clock_start, eta),
            b9: if vfr.is_some() {
                "VFR".into()
            } else {
                format!("{:03.0}", (ac.target_altitude / 100.0).round())
            },
            remarks: star_name(world, ac).unwrap_or_default(),
            atis,
        }
    }
}

/// Everything written on a strip, block by block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marks {
    /// Block 2: revision number, once the route has been amended.
    pub revision: Option<u32>,
    pub beacon: Vec<Pen>,
    /// Block 7: requested altitude, amended by Delivery.
    pub b7: Vec<Pen>,
    pub b9: Vec<Pen>,
    pub b9a: Vec<Pen>,
    pub b9b: Vec<Pen>,
    pub b9c: Vec<Pen>,
    /// Blocks 10 to 18, by the game's facility directive.
    pub grid: [Vec<Pen>; 9],
}

fn alt_mark(target: f32, before: f32) -> String {
    let arrow = if target > before { '↑' } else { '↓' };
    format!("{arrow}{:03.0}", target / 100.0)
}

fn runway_name(world: &World, end: usize) -> String {
    world
        .airport
        .runways
        .get(end)
        .map(|r| r.id.clone())
        .unwrap_or_default()
}

/// Mark a run of entries so only the newest stands.
fn strike_all_but_last(pens: &mut [Pen]) {
    let n = pens.len();
    for (i, p) in pens.iter_mut().enumerate() {
        p.struck = p.struck || i + 1 < n;
    }
}

/// The controllers' marks on a strip, from the instructions the pilot has
/// accepted and what the aircraft is doing now.
///
/// The facility directive for blocks 10 to 18:
/// - 10 runway; 11 ATIS letter; 12 taxi route;
/// - 13 ground: hold position `HP`, hold short `HS27L`, crossing `X27L`;
/// - 14 local: `L` cleared to land and `MA` missed approach (JO 7110.65 TBL
///   2-3-11 and 2-3-12), `T→` cleared for takeoff, `LUAW`, `TG`, `OPT`,
///   and VFR pattern marks;
/// - 15 approach: `ILS` cleared for the approach;
/// - 16 time of the block 14 mark; 17 handoff, struck once taken;
/// - 18 what it's waiting for from you (`World::awaiting`).
pub fn marks(
    world: &World,
    ac: &Aircraft,
    printed: &Printed,
    runways: &[String],
    clock_start: f64,
) -> Marks {
    let mut m = Marks::default();

    // Block 5: a squawk change is written beside the printed code.
    let special = match &ac.emergency {
        Some(e) if e.nordo() => Some("7600"),
        Some(e) if e.declared() => Some("7700"),
        _ => None,
    };
    m.beacon
        .push(Pen::printed(&printed.beacon).struck(special.is_some()));
    if let Some(code) = special {
        m.beacon.push(Pen::red(code));
    }

    // Block 9: the printed altitude or route, then amendments in the order given.
    let mut altitudes: Vec<Pen> = Vec::new();
    // Climb or descend is relative to the altitude it had before each instruction.
    let mut before = match printed.form {
        Form::Arrival => printed
            .b9
            .parse::<f32>()
            .map(|h| h * 100.0)
            .unwrap_or(ac.altitude),
        Form::Departure => world.airport.departure_initial_altitude,
    };
    let mut vectors: Vec<Pen> = Vec::new();
    let mut speeds: Vec<Pen> = Vec::new();
    let mut directs: Vec<Pen> = Vec::new();
    let mut taxi: Vec<Pen> = Vec::new();
    let mut ground: Vec<Pen> = Vec::new();
    let mut local: Vec<(Pen, f32)> = Vec::new();
    let mut approach: Vec<Pen> = Vec::new();
    let mut handoff: Option<(Position, Pen)> = None;
    let mut revision = 0;
    // Delivery's amendments to the filed route and altitude.
    let mut route_amended: Option<String> = None;
    let mut altitude_amended: Vec<Pen> = Vec::new();
    for mark in &ac.strip {
        match &mark.clearance {
            Clearance::Altitude(a) => {
                altitudes.push(Pen::hand(alt_mark(*a, before)));
                before = *a;
            }
            Clearance::Heading(h) => {
                vectors.push(Pen::hand(format!(
                    "H{:03.0}",
                    world.airport.to_magnetic(*h).round()
                )));
                directs.iter_mut().for_each(|p| p.struck = true);
            }
            Clearance::Speed(s) => speeds.push(Pen::hand(format!("S{s:.0}"))),
            Clearance::DirectFix(f) => {
                let name = world
                    .airport
                    .fixes
                    .get(*f)
                    .map(|f| f.name.as_str())
                    .unwrap_or("");
                directs.push(Pen::hand(format!("D {name}")));
                vectors.iter_mut().for_each(|p| p.struck = true);
                revision += 1;
            }
            // The holding fix and the EFC's minutes, as controllers write it.
            Clearance::Hold(spec) => {
                let name = world
                    .airport
                    .fixes
                    .get(spec.fix)
                    .map(|f| f.name.as_str())
                    .unwrap_or("");
                directs.push(Pen::hand(format!("H {name} {:02}", spec.efc_hhmm % 100)));
                vectors.iter_mut().for_each(|p| p.struck = true);
            }
            Clearance::ResumeRoute => {
                directs.push(Pen::hand("RTE"));
                vectors.iter_mut().for_each(|p| p.struck = true);
                revision += 1;
            }
            Clearance::ClearedIls(_) => approach.push(Pen::hand("ILS")),
            Clearance::GoAround => local.push((Pen::hand("MA"), mark.time)),
            Clearance::HandoffTower => handoff = Some((Position::Local, Pen::hand("TWR"))),
            Clearance::HandoffCenter => handoff = Some((Position::Center, Pen::hand("CTR"))),
            Clearance::ContactGround => handoff = Some((Position::Ground, Pen::hand("GND"))),
            Clearance::ContactTower => handoff = Some((Position::Local, Pen::hand("TWR"))),
            Clearance::ContactDeparture => {
                let mark = if ac.vfr.is_some() { "FREQ" } else { "DEP" };
                handoff = Some((Position::Approach, Pen::hand(mark)));
            }
            Clearance::Taxi { to, via } => {
                let map = world.airport.surface.as_ref();
                let target = map.and_then(|m| {
                    m.parking
                        .iter()
                        .find(|p| p.node == *to)
                        .map(|p| p.name.clone())
                        .or_else(|| {
                            (0..world.airport.runways.len())
                                .find(|&e| m.departure_hold(e) == Some(*to))
                                .map(|e| runway_name(world, e))
                        })
                });
                let mut words: Vec<String> = via.clone();
                if let Some(t) = target {
                    words.push(t);
                }
                taxi.push(Pen::hand(if words.is_empty() {
                    "TAXI".into()
                } else {
                    words.join(" ")
                }));
                ground.retain(|p| p.text != "HP");
            }
            Clearance::HoldPosition => ground.push(Pen::hand("HP")),
            Clearance::ContinueTaxi => ground
                .iter_mut()
                .filter(|p| p.text == "HP")
                .for_each(|p| p.struck = true),
            Clearance::CrossRunway(r) => {
                let name = world
                    .airport
                    .surface
                    .as_ref()
                    .map(|m| spoken_runway(&world.airport, m, *r))
                    .unwrap_or_default();
                ground
                    .iter_mut()
                    .filter(|p| p.text == format!("HS{name}"))
                    .for_each(|p| p.struck = true);
                ground.push(Pen::hand(format!("X{name}")));
            }
            Clearance::LineUpAndWait(_) => local.push((Pen::hand("LUAW"), mark.time)),
            Clearance::ClearedForTakeoff(_) => local.push((Pen::hand("T→"), mark.time)),
            Clearance::ClearedToLand(_) => local.push((Pen::hand("L"), mark.time)),
            Clearance::ClearedTouchAndGo(_) => local.push((Pen::hand("TG"), mark.time)),
            Clearance::ClearedOption(_) => local.push((Pen::hand("OPT"), mark.time)),
            Clearance::EnterDownwind(_) => local.push((Pen::hand("DW"), mark.time)),
            Clearance::ExtendDownwind => local.push((Pen::hand("EXT"), mark.time)),
            Clearance::TurnBase => local.push((Pen::hand("B"), mark.time)),
            Clearance::Follow { .. } => local.push((Pen::hand("FOL"), mark.time)),
            Clearance::Ifr { route, altitude, .. } => {
                if let Some(plan) = &ac.flight_plan {
                    let new_route = delivery::route_short(&world.airport, route);
                    let current = route_amended.clone().unwrap_or_else(|| delivery::route_short(&world.airport, &plan.route));
                    if new_route != current {
                        route_amended = Some(new_route);
                        revision += 1;
                    }
                    let text = format!("{:03.0}", altitude / 100.0);
                    let last = altitude_amended.last().map_or(printed.b7.clone(), |p| p.text.clone());
                    if text != last {
                        altitude_amended.push(Pen::hand(text));
                    }
                }
                if !ground.iter().any(|p| p.text == "CLR") {
                    ground.push(Pen::hand("CLR"));
                }
            }
            // A reset to the code already printed: nothing new to write.
            Clearance::Squawk(_) => {}
        }
    }
    // Holding short now: written when it stopped at the line.
    if let (Some(r), Some(map)) = (
        ac.taxi.as_ref().and_then(|t| t.holding_short),
        world.airport.surface.as_ref(),
    ) {
        let mark = format!("HS{}", spoken_runway(&world.airport, map, r));
        if !ground.iter().any(|p| p.text == mark) {
            ground.push(Pen::hand(mark));
        }
    }
    strike_all_but_last(&mut altitudes);
    strike_all_but_last(&mut vectors);
    strike_all_but_last(&mut speeds);
    strike_all_but_last(&mut directs);
    strike_all_but_last(&mut taxi);
    m.revision = (revision > 0).then_some(revision);

    let emergency = ac.emergency.as_ref().filter(|e| e.declared());
    match printed.form {
        Form::Arrival => {
            m.b9.push(Pen::printed(&printed.b9).struck(!altitudes.is_empty()));
            m.b9.extend(altitudes);
            if !printed.remarks.is_empty() {
                m.b9.push(Pen::printed(&printed.remarks));
            }
        }
        Form::Departure => {
            m.b9.push(Pen::printed(&printed.b9).struck(route_amended.is_some()));
            if let Some(route) = route_amended {
                m.b9.push(Pen::hand(route));
            }
            m.b9.extend(altitudes);
        }
    }
    strike_all_but_last(&mut altitude_amended);
    m.b7.push(Pen::printed(&printed.b7).struck(!altitude_amended.is_empty()));
    m.b7.extend(altitude_amended);
    if let Some(e) = emergency {
        m.b9.push(Pen::red(e.strip()));
        if e.kind == EmergencyKind::LowFuel {
            m.b9a.push(Pen::red("MIN FUEL"));
        }
    }
    m.b9a.extend(vectors);
    m.b9a.extend(speeds);
    m.b9b = directs;

    // Block 10: runways assigned, the latest standing.
    let mut rwys: Vec<Pen> = runways.iter().map(Pen::hand).collect();
    strike_all_but_last(&mut rwys);
    m.grid[0] = rwys;
    m.grid[1] = vec![Pen::hand(&printed.atis)];
    m.grid[2] = taxi;
    m.grid[3] = ground;
    let time = local.last().map(|(_, t)| *t);
    let mut local: Vec<Pen> = local.into_iter().map(|(p, _)| p).collect();
    strike_all_but_last(&mut local);
    m.grid[4] = local;
    m.grid[5] = approach;
    m.grid[6] = time
        .map(|t| vec![Pen::hand(hhmm(clock_start, t))])
        .unwrap_or_default();
    if let Some((to, pen)) = handoff {
        let taken = ac.frequency() == to;
        m.grid[7] = vec![pen.struck(taken)];
    }
    if let Some(w) = world.awaiting(ac) {
        m.grid[8] = vec![if w.urgent() {
            Pen::red(w.word())
        } else {
            Pen::hand(w.word())
        }];
    }
    m
}

/// The runway the strip shows in block 10 right now.
fn current_runway(world: &World, ac: &Aircraft) -> Option<String> {
    let end = match (&ac.vfr, ac.flow) {
        (Some(v), _) => Some(v.end),
        (None, Flow::Departure) => ac.departure_end,
        (None, Flow::Arrival) => ac.approach_runway.or_else(|| Some(world.arrival_runway())),
    }?;
    Some(runway_name(world, end))
}

/// The bays, in the order a controller works them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bay {
    Emergency,
    Inbound,
    Arrivals,
    Final,
    Departures,
    Arriving,
    Runway,
    HoldShort,
    TaxiOut,
    TaxiIn,
    /// At the gate, calling Delivery or cleared and not yet with Ground.
    Clearance,
    Pending,
}

impl Bay {
    pub fn name(self) -> &'static str {
        match self {
            Bay::Emergency => "EMERGENCY",
            Bay::Inbound => "INBOUND",
            Bay::Arrivals => "ARRIVALS",
            Bay::Final => "FINAL",
            Bay::Departures => "DEPARTURES",
            Bay::Arriving => "ARRIVING",
            Bay::Runway => "RUNWAY",
            Bay::HoldShort => "HOLD SHORT",
            Bay::TaxiOut => "TAXI OUT",
            Bay::TaxiIn => "TAXI IN",
            Bay::Clearance => "CLEARANCE",
            Bay::Pending => "PENDING",
        }
    }
}

/// Where the strip sits for this position, if it has one at all.
pub fn bay(world: &World, ac: &Aircraft) -> Option<Bay> {
    let staffing = world.staffing();
    let tower = staffing.tower();
    let radar = staffing.approach;
    let vfr_inbound = ac.vfr.as_ref().is_some_and(|v| {
        v.intent == VfrIntent::Arrival
            && matches!(
                v.leg,
                crate::sim::aircraft::Leg::Entry | crate::sim::aircraft::Leg::Holding
            )
    });
    let tower_bay = match ac.phase {
        Phase::TowerHandedOff => Some(Bay::Arriving),
        Phase::Pattern if vfr_inbound => Some(Bay::Arriving),
        Phase::LinedUp
        | Phase::Rollout
        | Phase::TowerRelease
        | Phase::GoAround
        | Phase::Pattern => Some(Bay::Runway),
        Phase::Taxiing => {
            let waiting = ac.taxi.as_ref().is_some_and(|t| {
                t.holding_short.is_some() || (t.done() && ac.flow == Flow::Departure)
            });
            Some(if waiting {
                Bay::HoldShort
            } else if ac.flow == Flow::Departure {
                Bay::TaxiOut
            } else {
                Bay::TaxiIn
            })
        }
        Phase::Parked if ac.flow == Flow::Departure && ac.surface_freq == Position::Delivery => {
            Some(Bay::Clearance)
        }
        Phase::Parked if ac.flow == Flow::Departure => Some(Bay::Pending),
        _ => None,
    };
    let radar_bay = match ac.phase {
        _ if ac.on_ground && !ac.rolling() => None,
        Phase::Offered => Some(Bay::Inbound),
        Phase::Controlled | Phase::GoAround if ac.flow == Flow::Arrival => Some(Bay::Arrivals),
        Phase::ClearedApproach | Phase::Localizer | Phase::Glideslope | Phase::TowerHandedOff => {
            Some(Bay::Final)
        }
        Phase::Controlled | Phase::TowerRelease | Phase::OfferedToCenter
            if ac.flow == Flow::Departure =>
        {
            Some(Bay::Departures)
        }
        _ => None,
    };
    let bay = match (tower, radar) {
        // Combined: the tower bays for the airport, the radar ones beyond it.
        (true, true) => tower_bay.or(radar_bay),
        (true, false) => tower_bay,
        (false, _) => radar_bay,
    }?;
    // On the radar, a declared emergency gets its own bay at the top.
    if radar && !tower && ac.in_emergency() {
        return Some(Bay::Emergency);
    }
    Some(bay)
}

/// The strips in bay order; within a bay, emergencies first. Arrivals then
/// go in landing sequence; elsewhere, strips waiting for you come next, then
/// first come first served.
pub fn ordered(world: &World) -> Vec<(Bay, &Aircraft)> {
    let mut rows: Vec<(Bay, &Aircraft)> = world
        .aircraft
        .iter()
        .filter_map(|a| bay(world, a).map(|b| (b, a)))
        .collect();
    let to_go = |a: &Aircraft| match a.approach_runway.or(a.vfr.as_ref().map(|v| v.end)) {
        Some(end) => world.airport.runways[end].threshold().distance(a.pos),
        None => 0.0,
    };
    let waiting = |a: &Aircraft| world.awaiting(a).is_some();
    rows.sort_by(|(ba, a), (bb, b)| {
        ba.cmp(bb)
            .then(b.in_emergency().cmp(&a.in_emergency()))
            .then_with(|| match ba {
                Bay::Final | Bay::Arriving => to_go(a).total_cmp(&to_go(b)),
                _ => waiting(b).cmp(&waiting(a)),
            })
    });
    rows
}

/// Draw text with arrows drawn as strokes, since the font has none.
/// Returns the width used.
fn write(text: &str, x: f32, y: f32, size: f32, color: Color) -> f32 {
    let mut cx = x;
    let mut run = String::new();
    let flush = |run: &mut String, cx: &mut f32| {
        if !run.is_empty() {
            draw_text(run.as_str(), *cx, y, size, color);
            *cx += measure_text(run.as_str(), None, size as u16, 1.0).width;
            run.clear();
        }
    };
    for ch in text.chars() {
        match ch {
            '↑' | '↓' | '→' => {
                flush(&mut run, &mut cx);
                let w = size * 0.5;
                let (mid, top, bottom) = (cx + w / 2.0, y - size * 0.7, y);
                match ch {
                    '↑' => {
                        draw_line(mid, bottom, mid, top, 1.3, color);
                        draw_line(mid, top, mid - w * 0.4, top + w * 0.5, 1.3, color);
                        draw_line(mid, top, mid + w * 0.4, top + w * 0.5, 1.3, color);
                    }
                    '↓' => {
                        draw_line(mid, top, mid, bottom, 1.3, color);
                        draw_line(mid, bottom, mid - w * 0.4, bottom - w * 0.5, 1.3, color);
                        draw_line(mid, bottom, mid + w * 0.4, bottom - w * 0.5, 1.3, color);
                    }
                    _ => {
                        let cy = y - size * 0.35;
                        draw_line(cx, cy, cx + w, cy, 1.3, color);
                        draw_line(cx + w, cy, cx + w * 0.55, cy - w * 0.4, 1.3, color);
                        draw_line(cx + w, cy, cx + w * 0.55, cy + w * 0.4, 1.3, color);
                    }
                }
                cx += w + 1.0;
            }
            _ => run.push(ch),
        }
    }
    flush(&mut run, &mut cx);
    cx - x
}

fn measure(text: &str, size: f32) -> f32 {
    let arrows = text
        .chars()
        .filter(|c| matches!(c, '↑' | '↓' | '→'))
        .count();
    let rest: String = text
        .chars()
        .filter(|c| !matches!(c, '↑' | '↓' | '→'))
        .collect();
    measure_text(&rest, None, size as u16, 1.0).width + arrows as f32 * (size * 0.5 + 1.0)
}

/// Write a block's entries left to right. If they don't fit, the oldest are
/// left off: the newest marks are the ones that matter.
fn write_block(pens: &[Pen], x: f32, y: f32, width: f32, size: f32, faded: bool) {
    let space = size * 0.4;
    let mut start = 0;
    while start < pens.len() {
        let total: f32 = pens[start..]
            .iter()
            .map(|p| measure(&p.text, size) + space)
            .sum::<f32>()
            - space;
        if total <= width {
            break;
        }
        start += 1;
    }
    // The newest mark always shows, written smaller if it's too long alone.
    let mut size = size;
    if start == pens.len() && !pens.is_empty() {
        start = pens.len() - 1;
        size *= width / measure(&pens[start].text, size);
    }
    let space = size * 0.4;
    let mut cx = x;
    for p in &pens[start..] {
        let mut color = if p.red {
            RED_PEN
        } else if p.hand {
            PEN
        } else {
            INK
        };
        if faded {
            color = Color::new(
                color.r * 0.7 + 0.1,
                color.g * 0.7 + 0.1,
                color.b * 0.7 + 0.1,
                1.0,
            );
        }
        let w = write(&p.text, cx, y, size, color);
        if p.struck {
            draw_line(
                cx - 1.0,
                y - size * 0.32,
                cx + w + 1.0,
                y - size * 0.32,
                1.2,
                color,
            );
        }
        cx += w + space;
    }
}

/// Write pens across up to two lines of block 9, wrapping between entries.
fn write_wrapped(pens: &[Pen], x: f32, y: f32, width: f32, faded: bool) {
    let space = FONT * 0.4;
    let mut split = pens.len();
    let mut used = 0.0;
    for (i, p) in pens.iter().enumerate() {
        used += measure(&p.text, FONT) + space;
        if used - space > width {
            split = i.max(1);
            break;
        }
    }
    write_block(&pens[..split], x, y, width, FONT, faded);
    if split < pens.len() {
        write_block(&pens[split..], x, y + ROW, width, FONT, faded);
    }
}

/// One strip's holder colour.
fn holder_color(ac: &Aircraft) -> Color {
    if ac.in_emergency() {
        HOLDER_EMERGENCY
    } else if ac.vfr.is_some() {
        HOLDER_VFR
    } else if ac.flow == Flow::Departure {
        HOLDER_DEPARTURE
    } else {
        HOLDER_ARRIVAL
    }
}

/// On for half of each blink.
fn blink_on() -> bool {
    (get_time() * 2.0).fract() < 0.5
}

/// The holder, blinking while the strip waits for you, and a frame round an
/// urgent one.
fn draw_holder(world: &World, ac: &Aircraft, x: f32, y: f32, w: f32, h: f32) {
    let waiting = world.awaiting(ac);
    let color = if waiting.is_some() && blink_on() {
        WHITE
    } else {
        holder_color(ac)
    };
    draw_rectangle(x, y, HOLDER, h, color);
    if waiting.is_some_and(|a| a.urgent()) && blink_on() {
        draw_rectangle_lines(x - 1.0, y - 1.0, w + 2.0, h + 2.0, 2.0, URGENT);
    }
}

fn draw_strip(world: &World, ui: &mut UiState, ac: &Aircraft, x: f32, y: f32) {
    let clock = ui.clock_start;
    let printed = ui.board.strip(world, ac, clock).clone();
    let runways = ui
        .board
        .runway_history(ac, current_runway(world, ac))
        .to_vec();
    let m = marks(world, ac, &printed, &runways, clock);
    let mine = world.is_players(ac) || ac.phase == Phase::Offered;
    let faded = !mine;
    let width = PANEL_WIDTH - MARGIN * 2.0;

    let px = x + HOLDER;
    let pw = width - HOLDER;
    draw_rectangle(
        px,
        y,
        pw,
        STRIP_HEIGHT,
        if mine { PAPER } else { PAPER_OTHER },
    );
    draw_holder(world, ac, x, y, width, STRIP_HEIGHT);

    // Column edges.
    let c1 = px;
    let c2 = c1 + COL_ID;
    let c3 = c2 + COL_6;
    let c4 = c3 + COL_8;
    let grid = px + pw - GRID_CELL * 3.0;
    let c9w = grid - c4;
    for edge in [c2, c3, c4, grid, grid + GRID_CELL, grid + GRID_CELL * 2.0] {
        draw_line(edge, y, edge, y + STRIP_HEIGHT, 1.0, RULE);
    }
    for row in 1..3 {
        let ry = y + 1.0 + ROW * row as f32;
        draw_line(c2, ry, c4, ry, 0.6, RULE);
        draw_line(grid, ry, px + pw, ry, 0.6, RULE);
    }
    // Block 9a/9b/9c sit along the bottom row of block 9.
    let ry = y + 1.0 + ROW * 2.0;
    draw_line(c4, ry, grid, ry, 0.6, RULE);
    draw_line(
        c4 + c9w / 3.0,
        ry,
        c4 + c9w / 3.0,
        y + STRIP_HEIGHT,
        0.6,
        RULE,
    );
    draw_line(
        c4 + c9w * 2.0 / 3.0,
        ry,
        c4 + c9w * 2.0 / 3.0,
        y + STRIP_HEIGHT,
        0.6,
        RULE,
    );

    let base = |row: usize| y + 1.0 + ROW * row as f32 + 13.0;
    let ink = if faded {
        Color::new(0.16, 0.16, 0.18, 1.0)
    } else {
        INK
    };
    let text = |s: &str, tx: f32, row: usize, size: f32| {
        draw_text(s, tx, base(row), size, ink);
    };

    // Blocks 1-5.
    text(&ac.callsign, c1 + 3.0, 0, 15.0);
    if let Some(rev) = m.revision {
        write_block(
            &[Pen::hand(rev.to_string())],
            c2 - 12.0,
            base(0),
            10.0,
            SMALL,
            faded,
        );
    }
    text(&printed.ty, c1 + 3.0, 1, FONT);
    text(&printed.cid, c1 + 3.0, 2, SMALL);
    write_block(&m.beacon, c1 + 30.0, base(2), COL_ID - 32.0, FONT, faded);

    // Blocks 6, 7, 2a and 8, 8a, 8b.
    text(&printed.b6, c2 + 2.0, 0, SMALL);
    write_block(&m.b7, c2 + 2.0, base(1), COL_6 - 4.0, SMALL, faded);
    text(printed.originator, c2 + 2.0, 2, SMALL);
    text(&printed.b8, c3 + 2.0, 0, SMALL);

    // Block 9 and 9a-9c.
    write_wrapped(&m.b9, c4 + 3.0, base(0), c9w - 6.0, faded);
    let third = c9w / 3.0;
    write_block(&m.b9a, c4 + 3.0, base(2), third - 5.0, SMALL, faded);
    write_block(&m.b9b, c4 + third + 3.0, base(2), third - 5.0, SMALL, faded);
    write_block(
        &m.b9c,
        c4 + third * 2.0 + 3.0,
        base(2),
        third - 5.0,
        SMALL,
        faded,
    );

    // Blocks 10-18, down each column.
    for (i, pens) in m.grid.iter().enumerate() {
        let (col, row) = (i / 3, i % 3);
        write_block(
            pens,
            grid + GRID_CELL * col as f32 + 2.0,
            base(row),
            GRID_CELL - 3.0,
            SMALL,
            faded,
        );
    }

    if ui.selected == Some(ac.id) {
        draw_rectangle_lines(
            x - 1.0,
            y - 1.0,
            width + 2.0,
            STRIP_HEIGHT + 2.0,
            2.0,
            SELECTED,
        );
    }
}

/// A strip folded to one row for a crowded bay: blocks 1, 3 and 5, the start
/// of block 9, and blocks 14 and 18.
fn draw_compact(world: &World, ui: &mut UiState, ac: &Aircraft, x: f32, y: f32) {
    let clock = ui.clock_start;
    let printed = ui.board.strip(world, ac, clock).clone();
    let runways = ui
        .board
        .runway_history(ac, current_runway(world, ac))
        .to_vec();
    let m = marks(world, ac, &printed, &runways, clock);
    let mine = world.is_players(ac) || ac.phase == Phase::Offered;
    let faded = !mine;
    let width = PANEL_WIDTH - MARGIN * 2.0;
    let px = x + HOLDER;
    let pw = width - HOLDER;
    draw_rectangle(
        px,
        y,
        pw,
        COMPACT_HEIGHT,
        if mine { PAPER } else { PAPER_OTHER },
    );
    draw_holder(world, ac, x, y, width, COMPACT_HEIGHT);

    let base = y + 14.0;
    let ink = if faded {
        Color::new(0.16, 0.16, 0.18, 1.0)
    } else {
        INK
    };
    draw_text(&ac.callsign, px + 3.0, base, FONT, ink);
    draw_text(&printed.ty, px + 66.0, base, SMALL, ink);
    write_block(&m.beacon, px + 136.0, base, 52.0, SMALL, faded);
    let grid = px + pw - GRID_CELL * 3.0;
    write_block(&m.b9, px + 192.0, base, grid - px - 196.0, SMALL, faded);
    draw_line(grid, y, grid, y + COMPACT_HEIGHT, 1.0, RULE);
    write_block(
        &m.grid[4],
        grid + 2.0,
        base,
        GRID_CELL * 2.0 - 4.0,
        SMALL,
        faded,
    );
    write_block(
        &m.grid[8],
        grid + GRID_CELL * 2.0 + 2.0,
        base,
        GRID_CELL - 3.0,
        SMALL,
        faded,
    );

    if ui.selected == Some(ac.id) {
        draw_rectangle_lines(
            x - 1.0,
            y - 1.0,
            width + 2.0,
            COMPACT_HEIGHT + 2.0,
            2.0,
            SELECTED,
        );
    }
}

/// What the strip doesn't say: where the aircraft is and what it's doing now.
fn live_status(world: &World, ac: &Aircraft) -> String {
    if ac.on_ground {
        return super::surface::status(world, ac);
    }
    let trend = if ac.vertical_speed > 300.0 {
        "climbing"
    } else if ac.vertical_speed < -300.0 {
        "descending"
    } else {
        "level"
    };
    format!(
        "{} ft {trend}, {:.0} kt, heading {:03.0}",
        crate::sim::clearance::thousands(ac.altitude),
        ac.ias,
        world.airport.to_magnetic(ac.heading).round()
    )
}

/// One row of the bay: a header or a strip.
enum Item<'a> {
    Header(Bay, usize),
    Strip(&'a Aircraft, bool),
}

impl Item<'_> {
    fn height(&self) -> f32 {
        match self {
            Item::Header(..) => BAY_HEADER,
            Item::Strip(_, true) => COMPACT_HEIGHT + GAP,
            Item::Strip(_, false) => STRIP_HEIGHT + GAP,
        }
    }
}

/// Lay the bay out: other positions' strips fold to one row when everything
/// won't fit.
fn layout<'a>(world: &'a World, available: f32) -> Vec<Item<'a>> {
    let rows = ordered(world);
    let mut counts: HashMap<Bay, usize> = HashMap::new();
    for (b, _) in &rows {
        *counts.entry(*b).or_default() += 1;
    }
    let build = |fold: bool| {
        let mut items = Vec::new();
        let mut current = None;
        for &(bay, ac) in &rows {
            if current != Some(bay) {
                current = Some(bay);
                items.push(Item::Header(bay, counts[&bay]));
            }
            let mine = world.is_players(ac) || ac.phase == Phase::Offered;
            // A clearance given folds up until it goes to Ground.
            let cleared = bay == Bay::Clearance && ac.flight_plan.as_ref().is_some_and(|p| p.is_cleared());
            items.push(Item::Strip(ac, (fold && !mine) || cleared));
        }
        items
    };
    let full = build(false);
    if full.iter().map(Item::height).sum::<f32>() <= available {
        full
    } else {
        build(true)
    }
}

/// Draw the strip bay between `top` and `bottom`. Clicking a strip selects
/// it, and the mouse wheel scrolls a bay too full to show at once.
pub fn draw(world: &World, ui: &mut UiState, panel_x: f32, top: f32, bottom: f32) {
    ui.board.prune(world);
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let x = panel_x + MARGIN;
    let width = PANEL_WIDTH - MARGIN * 2.0;
    let area = Rect::new(panel_x, top, PANEL_WIDTH, bottom - top);
    let available = bottom - top - 8.0;
    let items = layout(world, available);
    let total: f32 = items.iter().map(Item::height).sum();

    let max_scroll = (total - available).max(0.0);
    if area.contains(mouse) {
        let (_, wheel) = mouse_wheel();
        if wheel != 0.0 {
            ui.board.scroll -= wheel.signum() * (STRIP_HEIGHT + GAP);
        }
    }
    ui.board.scroll = ui.board.scroll.clamp(0.0, max_scroll);

    ui.hovered = None;
    let mut y = top + 4.0 - ui.board.scroll;
    let mut hovered = None;
    for item in &items {
        let h = item.height();
        let visible = y >= top && y + h - GAP <= bottom;
        if visible {
            match item {
                Item::Header(bay, count) => {
                    let title = format!("{} {count}", bay.name());
                    draw_text(&title, x + 2.0, y + 13.0, 14.0, BAY_TEXT);
                    let tw = measure_text(&title, None, 14, 1.0).width;
                    draw_line(x + tw + 10.0, y + 8.0, x + width, y + 8.0, 1.0, BAY_RULE);
                }
                Item::Strip(ac, compact) => {
                    if *compact {
                        draw_compact(world, ui, ac, x, y);
                    } else {
                        draw_strip(world, ui, ac, x, y);
                    }
                    let rect = Rect::new(x, y, width, h - GAP);
                    if rect.contains(mouse) {
                        hovered = Some(*ac);
                        if clicked {
                            ui.select(Some(ac.id));
                        }
                    }
                }
            }
        }
        y += h;
    }
    if ui.board.scroll > 0.0 {
        draw_text("more", x + width - 40.0, top + 14.0, 14.0, DIM);
        draw_triangle(
            vec2(x + width - 62.0, top + 13.0),
            vec2(x + width - 56.0, top + 13.0),
            vec2(x + width - 59.0, top + 7.0),
            DIM,
        );
    }
    if ui.board.scroll < max_scroll {
        draw_text("more", x + width - 40.0, bottom - 5.0, 14.0, DIM);
        draw_triangle(
            vec2(x + width - 50.0, bottom - 12.0),
            vec2(x + width - 44.0, bottom - 12.0),
            vec2(x + width - 47.0, bottom - 6.0),
            DIM,
        );
    }

    // Hover: point the aircraft out on the scope, and show what the strip doesn't.
    if let Some(ac) = hovered {
        ui.hovered = Some(ac.id);
        let tip = format!("{}  {}", ac.callsign, live_status(world, ac));
        let tw = measure_text(&tip, None, 16, 1.0).width;
        let (tx, ty) = (panel_x - tw - 18.0, mouse.y - 10.0);
        draw_rectangle(tx, ty, tw + 12.0, 22.0, Color::new(0.05, 0.07, 0.08, 0.92));
        draw_rectangle_lines(tx, ty, tw + 12.0, 22.0, 1.0, BAY_RULE);
        draw_text(
            &tip,
            tx + 6.0,
            ty + 16.0,
            16.0,
            Color::new(0.85, 0.90, 0.88, 1.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::aircraft::StripMark;
    use crate::sim::airspace::{Airport, DEFAULT_AIRPORT};
    use crate::sim::performance::aircraft_type;
    use crate::sim::{World, WorldConfig};
    use glam::Vec2;

    fn world() -> World {
        World::new(
            Airport::from_toml(DEFAULT_AIRPORT).unwrap(),
            WorldConfig::default(),
        )
    }

    fn fixes(world: &World, names: &[&str]) -> Vec<usize> {
        names
            .iter()
            .map(|n| {
                world
                    .airport
                    .fixes
                    .iter()
                    .position(|f| f.name == *n)
                    .unwrap()
            })
            .collect()
    }

    fn departure(world: &World) -> Aircraft {
        let mut ac = Aircraft::new(
            7,
            "BAW217",
            Flow::Departure,
            aircraft_type("B77W").unwrap(),
            Vec2::ZERO,
            270.0,
            0.0,
            0.0,
        );
        ac.route = fixes(world, &["IVORY", "TOPAZ"]);
        ac.exit_fix = ac.route.last().copied();
        ac.spawn_time = 90.0;
        ac
    }

    fn arrival(world: &World) -> Aircraft {
        let mut ac = Aircraft::new(
            12,
            "NVA101",
            Flow::Arrival,
            aircraft_type("B738").unwrap(),
            Vec2::new(40.0, 40.0),
            225.0,
            13000.0,
            280.0,
        );
        ac.route = fixes(world, &["BOLTE", "ALDER"]);
        ac
    }

    fn mark(ac: &mut Aircraft, by: Position, clearance: Clearance) {
        let time = ac.strip.len() as f32 * 60.0;
        ac.strip.push(StripMark {
            time,
            by,
            clearance,
        });
    }

    #[test]
    fn departure_strip_prints_the_terminal_departure_blocks() {
        let w = world();
        let p = print(&w, &departure(&w), 0.0);
        assert_eq!(p.form, Form::Departure);
        assert_eq!(
            p.ty, "H/B77W/L",
            "block 3: heavy indicator, type, equipment suffix"
        );
        assert_eq!(p.b6, "P0001", "block 6: proposed departure time");
        assert_eq!(p.b7, "370", "block 7: requested altitude");
        assert_eq!(p.b8, "XHGT", "block 8: departure airport");
        assert!(
            p.b9.starts_with("IVORY TOPAZ X"),
            "block 9: route then destination, got {}",
            p.b9
        );
        assert_eq!(p.beacon.len(), 4);
        assert!(
            p.beacon.chars().all(|c| ('0'..='7').contains(&c)),
            "octal code {}",
            p.beacon
        );
        assert!(!p.beacon.starts_with('7') && p.beacon != "1200");
    }

    #[test]
    fn arrival_strip_prints_the_terminal_arrival_blocks() {
        let w = world();
        let p = print(&w, &arrival(&w), 0.0);
        assert_eq!(p.form, Form::Arrival);
        assert_eq!(p.ty, "B738/L");
        assert!(
            p.b6.starts_with('X') && p.b6.len() == 4,
            "block 6: the fix before, here the origin: {}",
            p.b6
        );
        assert_eq!(p.b7, "BOLTE", "block 7: coordination fix");
        assert_eq!(p.b8.len(), 4, "block 8: ETA at the coordination fix");
        assert_eq!(p.b9, "130", "block 9: altitude in hundreds");
    }

    #[test]
    fn the_same_flight_always_prints_the_same_strip() {
        let w = world();
        assert_eq!(print(&w, &arrival(&w), 0.0), print(&w, &arrival(&w), 0.0));
    }

    #[test]
    fn a_new_altitude_strikes_out_the_old_one() {
        let w = world();
        let mut ac = arrival(&w);
        mark(&mut ac, Position::Approach, Clearance::Altitude(5000.0));
        mark(&mut ac, Position::Approach, Clearance::Altitude(4000.0));
        let p = print(&w, &ac, 0.0);
        let m = marks(&w, &ac, &p, &[], 0.0);
        let b9: Vec<(&str, bool)> = m.b9.iter().map(|p| (p.text.as_str(), p.struck)).collect();
        assert_eq!(b9[..3], [("130", true), ("↓050", true), ("↓040", false)]);
        assert_eq!(m.revision, None, "an altitude isn't a route amendment");
    }

    #[test]
    fn a_hold_is_written_with_its_fix_and_efc_minutes() {
        let w = world();
        let mut ac = arrival(&w);
        mark(&mut ac, Position::Approach, Clearance::Heading(180.0));
        let alder = w.airport.fix_index("ALDER").unwrap();
        let mut spec = w.hold_for(&ac, alder);
        spec.efc_hhmm = 1435;
        mark(&mut ac, Position::Approach, Clearance::Hold(spec));
        let p = print(&w, &ac, 0.0);
        let m = marks(&w, &ac, &p, &[], 0.0);
        assert_eq!(m.b9b[0].text, "H ALDER 35");
        assert!(m.b9a[0].struck, "the vector is superseded");
    }

    #[test]
    fn delivery_amendments_strike_the_filed_altitude_and_route() {
        let w = world();
        let mut ac = departure(&w);
        let filed = fixes(&w, &["JUNIP", "SEGUN"]);
        let mut plan_world = World::new(
            Airport::from_toml(DEFAULT_AIRPORT).unwrap(),
            WorldConfig { traffic: false, ..WorldConfig::default() },
        );
        plan_world.request_departure(aircraft_type("B77W").unwrap(), filed.clone(), 13000.0);
        plan_world.tick();
        let mut plan = plan_world.aircraft[0].flight_plan.clone().unwrap();
        plan.route = filed;
        plan.altitude = 36000.0;
        ac.flight_plan = Some(plan);
        let p = print(&w, &ac, 0.0);
        assert_eq!(p.b7, "360");
        assert!(p.b9.starts_with("JUNIP SEGUN"), "{}", p.b9);
        let route = ac.route.clone();
        mark(&mut ac, Position::Delivery, Clearance::Ifr { route, altitude: 37000.0, squawk: 0o452 });
        let m = marks(&w, &ac, &p, &[], 0.0);
        let texts = |pens: &[Pen]| pens.iter().map(|p| (p.text.clone(), p.struck)).collect::<Vec<_>>();
        assert_eq!(texts(&m.b7), [("360".to_string(), true), ("370".to_string(), false)]);
        assert!(m.b9[0].struck && m.b9[1].text == "IVORY TOPAZ", "{:?}", m.b9);
        assert_eq!(m.revision, Some(1));
        assert!(m.grid[3].iter().any(|p| p.text == "CLR"));
        assert_eq!(p.beacon, delivery::code_text(ac.flight_plan.as_ref().unwrap().squawk));
    }

    #[test]
    fn a_route_amendment_bumps_the_revision_and_strikes_vectors() {
        let w = world();
        let mut ac = arrival(&w);
        mark(&mut ac, Position::Approach, Clearance::Heading(180.0));
        let alder = ac.route[1];
        mark(&mut ac, Position::Approach, Clearance::DirectFix(alder));
        let p = print(&w, &ac, 0.0);
        let m = marks(&w, &ac, &p, &[], 0.0);
        assert_eq!(m.revision, Some(1));
        assert!(m.b9a[0].struck, "the vector is superseded");
        assert_eq!(m.b9b[0].text, "D ALDER");
    }

    #[test]
    fn local_marks_keep_the_newest_and_time_it() {
        let w = world();
        let mut ac = departure(&w);
        mark(&mut ac, Position::Local, Clearance::LineUpAndWait(0));
        mark(&mut ac, Position::Local, Clearance::ClearedForTakeoff(0));
        let p = print(&w, &ac, 0.0);
        let m = marks(&w, &ac, &p, &["27R".into()], 0.0);
        let local: Vec<(&str, bool)> = m.grid[4]
            .iter()
            .map(|p| (p.text.as_str(), p.struck))
            .collect();
        assert_eq!(local, [("LUAW", true), ("T→", false)], "block 14");
        assert_eq!(m.grid[6][0].text, "0001", "block 16: when it was given");
        assert_eq!(m.grid[0][0].text, "27R", "block 10: runway");
    }

    #[test]
    fn a_handoff_is_struck_once_taken() {
        let w = world();
        let mut ac = arrival(&w);
        mark(&mut ac, Position::Approach, Clearance::HandoffTower);
        ac.phase = Phase::TowerHandedOff;
        let p = print(&w, &ac, 0.0);
        let m = marks(&w, &ac, &p, &[], 0.0);
        assert_eq!(m.grid[7][0].text, "TWR");
        assert!(m.grid[7][0].struck, "Tower has it");
    }
}
