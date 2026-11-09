//! Selection, drag vectors, hotkeys and the pending clearance.

use super::camera::{Camera, inset_area, mouse_in_scope, scope_area, scope_width};
use super::scope::{self, BLOCK_SIZE};
use super::surface;
use crate::sim::World;
use crate::sim::airspace::Airport;
use crate::sim::approach::ApproachKind;
use crate::sim::aircraft::{Flow, Landing, Leg, Phase};
use crate::sim::clearance::{Clearance, PendingClearance};
use crate::sim::positions::{Position, SOLO, Seat, Staffing};
use crate::sim::replay::Action;
use crate::sim::units::vec_heading;
use macroquad::prelude::*;
use super::gfx::{mouse_position};
use std::collections::{HashMap, HashSet};

const TARGET_HIT_PX: f32 = 12.0;
const FIX_SNAP_PX: f32 = 14.0;
const DRAG_START_PX: f32 = 8.0;
const ZOOM_STEP: f32 = 1.12;
/// Radar range shown in the inset, NM.
const INSET_RANGE_NM: f32 = 12.0;

/// The two pictures a controller can have up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Radar,
    Surface,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    None,
    /// Pulling a heading vector out of a target.
    Vector { id: u32, start: Vec2, active: bool },
    /// Moving a data block. `grab` is the cursor's offset within the block.
    Block { id: u32, grab: Vec2 },
    Pan { last: Vec2, surface: bool },
}

pub struct UiState {
    /// The radar scope's camera.
    pub camera: Camera,
    /// The surface map's camera.
    pub surface: Camera,
    /// Which view fills the screen; the other, if any, is an inset.
    pub main_view: View,
    /// The player at this screen: everything they say goes out from this
    /// seat. Always `SOLO` in single player.
    pub seat: Seat,
    /// The positions this player works. In co-op, only theirs: the world's
    /// staffing has everyone's.
    pub staffing: Staffing,
    /// Taxiways picked for the next taxi instruction, in order.
    pub via: Vec<String>,
    pub selected: Option<u32>,
    pub pending: PendingClearance,
    pub drag: Drag,
    /// Data block offsets from their targets, for blocks the player has moved.
    pub block_offsets: HashMap<u32, Vec2>,
    pub show_mva: bool,
    pub paused: bool,
    /// Sim seconds per real second.
    pub rate: f32,
    /// UTC seconds since the epoch when the shift started, for the scope clock.
    pub clock_start: f64,
    /// A tutorial lesson rather than a timed shift.
    pub lesson: bool,
    /// Something else used Enter this frame (turning a lesson page), so it doesn't transmit.
    pub enter_taken: bool,
    /// Emergencies the player has selected: their EM tag stops blinking.
    pub acknowledged: HashSet<u32>,
    /// Conflict alert pairs and MSAW tracks acknowledged by selecting one:
    /// their CA or LA stops blinking.
    pub acknowledged_pairs: HashSet<(u32, u32)>,
    pub acknowledged_low: HashSet<u32>,
    /// The flight strip bay.
    pub board: super::strips::Board,
    /// The aircraft whose strip the mouse is over, pointed out on the scope.
    pub hovered: Option<u32>,
    /// Show the direction of flight beside a flight plan (F4 hides it).
    pub course_hint: bool,
    /// Watching a replay: everything can be looked at, nothing can be said.
    pub replay: bool,
}

impl UiState {
    pub fn new(airport: &Airport, staffing: Staffing) -> UiState {
        let main_view = if staffing.approach { View::Radar } else { View::Surface };
        let mut ui = UiState {
            camera: Camera::fit_sector(airport),
            surface: Camera::fit_surface(airport),
            main_view,
            seat: SOLO,
            staffing,
            via: Vec::new(),
            selected: None,
            pending: PendingClearance::default(),
            drag: Drag::None,
            block_offsets: HashMap::new(),
            show_mva: true,
            paused: false,
            rate: 1.0,
            clock_start: macroquad::miniquad::date::now(),
            lesson: false,
            enter_taken: false,
            acknowledged: HashSet::new(),
            acknowledged_pairs: HashSet::new(),
            acknowledged_low: HashSet::new(),
            board: Default::default(),
            hovered: None,
            course_hint: true,
            replay: false,
        };
        ui.frame_views(airport);
        ui
    }

    /// The screen of the player in `seat`, with the world's `staffing` of
    /// every player.
    pub fn for_seat(airport: &Airport, staffing: Staffing, seat: Seat) -> UiState {
        let mut ui = UiState::new(airport, staffing.of_seat(seat));
        ui.seat = seat;
        ui
    }

    /// Do something as this player. False if it can't be done.
    pub fn act(&self, world: &mut World, id: u32, action: Action) -> bool {
        !self.replay && world.act(self.seat, id, action)
    }

    /// Tower positions see both pictures: one full size, one inset.
    pub fn has_inset(&self) -> bool {
        self.staffing.tower()
    }

    /// Put each camera in its viewport for this frame.
    pub fn layout(&mut self) {
        let (main, inset) = (scope_area(), inset_area());
        let (radar, surface) = match (self.has_inset(), self.main_view) {
            (false, _) => (main, main),
            (true, View::Radar) => (main, inset),
            (true, View::Surface) => (inset, main),
        };
        self.camera.viewport = radar;
        self.surface.viewport = surface;
    }

    /// Frame both views for where they now are.
    fn frame_views(&mut self, airport: &Airport) {
        self.layout();
        let (radar, surface) = (self.camera.viewport, self.surface.viewport);
        self.camera = if self.has_inset() && self.main_view == View::Surface {
            Camera::range(glam::Vec2::ZERO, INSET_RANGE_NM, radar)
        } else {
            let mut c = Camera::fit_sector(airport);
            c.viewport = radar;
            c
        };
        let points: Vec<glam::Vec2> = match &airport.surface {
            Some(map) => map.nodes.iter().map(|n| n.pos).collect(),
            None => vec![glam::Vec2::ZERO],
        };
        // Full size with the radar inset over a corner: keep the runways clear of it.
        self.surface = if self.has_inset() && self.main_view == View::Surface {
            Camera::fit_clear_of(&points, surface, radar)
        } else {
            Camera::fit(points, surface)
        };
    }

    /// Swap which view is full size, when both are up.
    pub fn show(&mut self, view: View, airport: &Airport) {
        if self.has_inset() && self.staffing.approach && self.main_view != view {
            self.main_view = view;
            self.frame_views(airport);
        }
    }

    /// Select an aircraft. An unsent clearance never carries over to another aircraft.
    pub fn select(&mut self, id: Option<u32>) {
        if self.selected != id {
            self.selected = id;
            self.pending = PendingClearance::default();
            self.via.clear();
        }
    }

    pub fn transmit(&mut self, world: &mut World) {
        if self.replay {
            return;
        }
        if let Some(id) = self.selected
            && self.act(world, id, Action::Transmit(self.pending.to_clearances())) {
                self.pending = PendingClearance::default();
            }
    }

    /// One action for all handoffs: accept from Center, or pass the aircraft on
    /// to whoever is next: Tower, Ground, Departure or Center.
    pub fn handoff(&mut self, world: &mut World) {
        let Some(id) = self.selected.filter(|_| !self.replay) else { return };
        let Some(ac) = world.find(id) else { return };
        // An emergency: tell the next position on the landline first.
        if world.owed_emergency_call_for(self.seat, id).is_some() {
            self.act(world, id, Action::CallEmergency);
            return;
        }
        match (ac.phase, ac.flow) {
            (Phase::Offered, _) => {
                self.act(world, id, Action::AcceptHandoff);
            }
            _ if ac.on_ground => match ac.frequency() {
                Position::Ground => self.pending.add_tower(Clearance::ContactTower),
                Position::Local | Position::Delivery => self.pending.add_tower(Clearance::ContactGround),
                _ => {}
            },
            (Phase::TowerRelease | Phase::GoAround | Phase::Pattern, _) => {
                self.pending.add_tower(Clearance::ContactDeparture)
            }
            (_, Flow::Arrival) => self.pending.handoff_tower = true,
            (_, Flow::Departure) => self.pending.handoff_center = true,
        }
    }

    /// H: on the ground, hold position, or continue if already holding. In
    /// the air, hold at the fix it's flying to, or the nearest one.
    pub fn hold(&mut self, world: &World) {
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return };
        if !ac.on_ground {
            let fix = self.pending.direct.or_else(|| world.hold_fix_for(ac));
            if let Some(fix) = fix {
                self.pending.set_hold(world.hold_for(ac, fix));
            }
            return;
        }
        let holding = ac.taxi.as_ref().is_some_and(|t| t.holding);
        self.pending.add_tower(if holding { Clearance::ContinueTaxi } else { Clearance::HoldPosition });
    }

    /// The selected aircraft, if it's at the gate on Delivery's frequency.
    fn at_delivery<'a>(&self, world: &'a World) -> Option<&'a crate::sim::aircraft::Aircraft> {
        self.selected
            .and_then(|id| world.find(id))
            .filter(|a| a.on_ground && a.phase == Phase::Parked && a.frequency() == Position::Delivery)
            .filter(|a| a.flight_plan.is_some())
    }

    /// The IFR clearance being put together: the pending one, or a fresh one
    /// from what it's cleared for already or what it filed.
    fn ifr_draft(&self, world: &World) -> Option<(Vec<usize>, f32, u16)> {
        let ac = self.at_delivery(world)?;
        for c in &self.pending.tower {
            if let Clearance::Ifr { route, altitude, squawk } = c {
                return Some((route.clone(), *altitude, *squawk));
            }
        }
        let plan = ac.flight_plan.as_ref()?;
        Some(match &plan.cleared {
            Some(c) => (c.route.clone(), c.altitude, c.squawk),
            None => (plan.route.clone(), plan.altitude, plan.squawk),
        })
    }

    /// I: the IFR clearance, as filed or amended. Again once cleared, it
    /// repeats the altitude and code, to put a wrong readback right.
    /// I: the IFR clearance at Delivery; in the air, reset a wrong code.
    pub fn ifr(&mut self, world: &World) {
        if let Some((route, altitude, squawk)) = self.ifr_draft(world) {
            self.pending.add_tower(Clearance::Ifr { route, altitude, squawk });
            return;
        }
        let Some(ac) = self.selected.and_then(|id| world.find(id)).filter(|a| !a.on_ground) else { return };
        // Airborne: say again what it was given, to put a wrong readback right.
        if ac.vfr.is_none() && world.is_seats(self.seat, ac) && ac.phase == Phase::Controlled {
            self.pending.altitude = Some(ac.assigned_altitude());
            if let Some(h) = ac.assigned_heading() {
                self.pending.set_heading(h);
            }
        }
        if let Some(plan) = &ac.flight_plan {
            self.pending.add_tower(Clearance::Squawk(plan.squawk));
        }
    }

    /// Up/Down at Delivery: amend the altitude to expect.
    pub fn amend_altitude(&mut self, world: &World, by: i32) {
        if let Some((route, altitude, squawk)) = self.ifr_draft(world) {
            let altitude = crate::sim::delivery::step_altitude(altitude, by);
            self.pending.add_tower(Clearance::Ifr { route, altitude, squawk });
        }
    }

    /// S at Delivery: the next SID in use, leaving by the same exit fix if one does.
    pub fn cycle_sid(&mut self, world: &World) {
        let Some((route, altitude, squawk)) = self.ifr_draft(world) else { return };
        let choices = crate::sim::delivery::sid_choices(&world.airport, world.config_name(), &route);
        if choices.is_empty() {
            return;
        }
        let next = match choices.iter().position(|r| *r == route) {
            Some(i) => choices[(i + 1) % choices.len()].clone(),
            None => choices[0].clone(),
        };
        self.pending.add_tower(Clearance::Ifr { route: next, altitude, squawk });
    }

    /// Cross the runway it's holding short of, or the next one on its route.
    /// X: cross the next runway ahead. On Ground without Local, the first
    /// press asks Tower on the landline; with Tower's approval (or pressed
    /// again regardless) it's the crossing clearance.
    pub fn cross(&mut self, world: &mut World) {
        let Some(id) = self.selected else { return };
        let Some(runway) = world.find(id).and_then(|ac| crossing_ahead(world, ac)) else { return };
        if self.needs_request(world, id, runway) {
            self.act(world, id, Action::RequestCrossing(runway));
            return;
        }
        self.pending.add_tower(Clearance::CrossRunway(runway));
    }

    /// X would ask Tower rather than cross.
    fn needs_request(&self, world: &World, id: u32, runway: usize) -> bool {
        use crate::sim::coordination::{CoordKind, CoordState};
        let asked = world.coordination_for(id).is_some_and(|c| {
            c.kind == CoordKind::Crossing { runway }
                && matches!(c.state, CoordState::Pending | CoordState::Unable { callback: true })
        });
        world.needs_crossing_approval(id, runway) && !asked
    }

    /// The X button's label: a request on Ground before Tower has agreed.
    pub fn cross_label(&self, world: &World) -> &'static str {
        let request = self.selected.is_some_and(|id| {
            world.find(id).and_then(|ac| crossing_ahead(world, ac)).is_some_and(|r| self.needs_request(world, id, r))
        });
        if request { "Req cross X" } else { "Cross X" }
    }

    /// Y or N: answer a landline request, about the selected aircraft if
    /// there's one, else the oldest.
    pub fn coordinate(&mut self, world: &mut World, approve: bool) {
        if self.replay {
            return;
        }
        let selected = self.selected.filter(|&id| world.incoming_for(self.seat).any(|c| c.aircraft == id));
        if let Some(id) = selected.or_else(|| world.incoming_for(self.seat).next().map(|c| c.aircraft)) {
            self.act(world, id, Action::Coordinate { approve });
        }
    }

    /// K on Local while departures are on call for release, with no release
    /// asked for yet: the first press asks Approach on the landline.
    fn needs_release_request(&self, world: &World) -> bool {
        use crate::sim::coordination::{CoordKind, CoordState};
        self.selected.is_some_and(|id| {
            let asked = world.coordination_for(id).is_some_and(|c| {
                matches!(c.kind, CoordKind::Release { .. })
                    && matches!(c.state, CoordState::Pending | CoordState::Unable { callback: true })
            });
            self.staffing.local && world.needs_release(id) && !asked
        })
    }

    /// The K button's label: a release request while one is needed.
    pub fn takeoff_label(&self, world: &World) -> &'static str {
        if self.needs_release_request(world) { "Req rel K" } else { "Takeoff K" }
    }

    /// G: go around; or, for an aircraft already going around that Approach
    /// hasn't been told about, the landline call to Approach. With nothing
    /// selected, the oldest call owed.
    pub fn go_around(&mut self, world: &mut World) {
        let owed =
            world.coordination.iter().find(|c| world.owes_go_around_call_for(self.seat, c.aircraft)).map(|c| c.aircraft);
        let target = match self.selected {
            Some(id) if world.owes_go_around_call_for(self.seat, id) => Some(id),
            Some(_) => None,
            None => owed,
        };
        match target {
            Some(id) if !self.replay => {
                self.act(world, id, Action::CallGoAround);
            }
            Some(_) => {}
            None if self.selected.is_some() => self.pending.go_around = true,
            None => {}
        }
    }

    /// The G button's label: the call to Approach while one is owed.
    pub fn go_around_label(&self, world: &World) -> &'static str {
        if self.selected.is_some_and(|id| world.owes_go_around_call_for(self.seat, id)) { "Call G/A G" } else { "G/A G" }
    }

    /// F on Approach with Tower automated: stop departures, or release them.
    pub fn toggle_call_for_release(&mut self, world: &mut World) {
        let on = !world.call_for_release();
        self.act(world, 0, Action::CallForRelease { on });
    }

    /// Line up and wait, or cleared for takeoff, on the aircraft's departure runway.
    pub fn departure(&mut self, world: &mut World, takeoff: bool) {
        if takeoff && self.needs_release_request(world) {
            if let Some(id) = self.selected {
                self.act(world, id, Action::RequestRelease);
            }
            return;
        }
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return };
        let Some(end) = ac.departure_end else { return };
        let clearance = if takeoff { Clearance::ClearedForTakeoff(end) } else { Clearance::LineUpAndWait(end) };
        // One or the other, never both.
        self.pending.tower.retain(|c| {
            !matches!(c, Clearance::ClearedForTakeoff(_) | Clearance::LineUpAndWait(_))
        });
        self.pending.add_tower(clearance);
    }

    pub fn land(&mut self, world: &World) {
        if self.selected.and_then(|id| world.find(id)).is_some_and(|a| a.vfr.is_some()) {
            self.vfr_landing(world, Landing::FullStop);
            return;
        }
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return };
        let end = ac.approach_runway.unwrap_or_else(|| world.arrival_runway());
        self.pending.add_tower(Clearance::ClearedToLand(end));
    }

    /// Clear a VFR aircraft to land, touch and go, or for the option. One of them only.
    pub fn vfr_landing(&mut self, world: &World, kind: Landing) {
        let Some(vfr) = self.selected.and_then(|id| world.find(id)).and_then(|a| a.vfr.as_ref()) else { return };
        let end = vfr.end;
        self.pending.tower.retain(|c| {
            !matches!(c, Clearance::ClearedToLand(_) | Clearance::ClearedTouchAndGo(_) | Clearance::ClearedOption(_))
        });
        self.pending.add_tower(match kind {
            Landing::FullStop => Clearance::ClearedToLand(end),
            Landing::TouchAndGo => Clearance::ClearedTouchAndGo(end),
            Landing::Option => Clearance::ClearedOption(end),
        });
    }

    /// Enter the downwind, for a VFR arrival outside the pattern; otherwise extend it.
    pub fn enter_or_extend(&mut self, world: &World) {
        let Some(vfr) = self.selected.and_then(|id| world.find(id)).and_then(|a| a.vfr.as_ref()) else { return };
        let clearance = if matches!(vfr.leg, Leg::Entry | Leg::Holding) && !vfr.cleared_pattern {
            Clearance::EnterDownwind(vfr.end)
        } else {
            Clearance::ExtendDownwind
        };
        self.pending.tower.retain(|c| !matches!(c, Clearance::TurnBase));
        self.pending.add_tower(clearance);
    }

    pub fn turn_base(&mut self, world: &World) {
        if self.selected.and_then(|id| world.find(id)).is_some_and(|a| a.vfr.is_some()) {
            self.pending.tower.retain(|c| !matches!(c, Clearance::ExtendDownwind));
            self.pending.add_tower(Clearance::TurnBase);
        }
    }

    /// Shift-click: tell the selected VFR aircraft to follow another. True if it did.
    fn follow(&mut self, world: &World, leader: u32) -> bool {
        let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return false };
        let Some(lead) = world.find(leader) else { return false };
        if !shift || ac.on_ground || ac.id == leader {
            return false;
        }
        let traffic = crate::sim::vfr::traffic_description(lead);
        if ac.vfr.is_none() {
            // An IFR arrival: a visual approach behind it, to its runway.
            if ac.flow != Flow::Arrival || lead.vfr.is_some() {
                return false;
            }
            let runway = lead.approach_runway.unwrap_or_else(|| world.arrival_runway());
            self.pending.approach = Some((ApproachKind::Visual, runway));
            self.pending.follow = Some((leader, traffic));
            return true;
        }
        self.pending.add_tower(Clearance::Follow { id: leader, traffic });
        true
    }

    /// Add a taxiway to the via list, or take it back off if it was the last one.
    fn toggle_via(&mut self, name: String) {
        if self.via.last() == Some(&name) {
            self.via.pop();
        } else {
            self.via.push(name);
        }
        for c in &mut self.pending.tower {
            if let Clearance::Taxi { via, .. } = c {
                *via = self.via.clone();
            }
        }
    }

    /// Step through the approaches on offer: ILS, RNAV and visual to the
    /// landing runway, then the parallel's, then none. With the aircraft
    /// sent to one of an RNAV approach's fixes, that approach comes first.
    pub fn cycle_approach(&mut self, world: &World) {
        let mut options = world.approach_options();
        let fix = self.pending.direct.or_else(|| {
            let ac = self.selected.and_then(|id| world.find(id))?;
            if self.pending.heading.is_some() { None } else { ac.active_fix() }
        });
        if let Some(fix) = fix
            && let Some(i) = options
                .iter()
                .position(|&(k, e)| k == ApproachKind::Rnav && world.airport.rnav_route_from(e, fix).is_some())
        {
            let first = options.remove(i);
            options.insert(0, first);
        }
        let next = match self.pending.approach.and_then(|p| options.iter().position(|&o| o == p)) {
            Some(i) => options.get(i + 1).copied(),
            None => options.first().copied(),
        };
        self.pending.approach = next;
        self.pending.follow = None;
    }

    pub fn nudge_heading(&mut self, world: &World, degrees: f32) {
        if let Some(ac) = self.selected.and_then(|id| world.find(id)) {
            self.pending.nudge_heading(ac, degrees);
        }
    }

    pub fn nudge_altitude(&mut self, world: &World, feet: f32) {
        if self.at_delivery(world).is_some() {
            self.amend_altitude(world, feet.signum() as i32);
            return;
        }
        if let Some(ac) = self.selected.and_then(|id| world.find(id)) {
            self.pending.nudge_altitude(ac, feet);
        }
    }

    pub fn nudge_speed(&mut self, world: &World, knots: f32) {
        if let Some(ac) = self.selected.and_then(|id| world.find(id)) {
            self.pending.nudge_speed(ac, knots);
        }
    }

    pub fn cancel(&mut self) {
        if self.pending.is_empty() && self.via.is_empty() {
            self.select(None);
        } else {
            self.pending = PendingClearance::default();
            self.via.clear();
        }
    }

    /// Next aircraft in strip order that isn't Tower's or already gone.
    fn cycle_selection(&mut self, world: &World) {
        let ids: Vec<u32> = world
            .aircraft
            .iter()
            .filter(|a| world.is_seats(self.seat, a) || a.phase == Phase::Offered)
            .map(|a| a.id)
            .collect();
        if ids.is_empty() {
            self.select(None);
            return;
        }
        let next = match self.selected.and_then(|s| ids.iter().position(|&i| i == s)) {
            Some(i) => ids[(i + 1) % ids.len()],
            None => ids[0],
        };
        self.select(Some(next));
    }

    pub fn update(&mut self, world: &mut World) {
        if self.selected.is_some_and(|id| world.find(id).is_none()) {
            self.select(None);
        }
        self.handle_keys(world);
        self.handle_mouse(world);
        if self.replay {
            // Nothing is said in a replay, so nothing is drafted either.
            self.pending = PendingClearance::default();
            self.via.clear();
        }
        if let Some(id) = self.selected.filter(|&id| world.find(id).is_some_and(|a| a.in_emergency())) {
            self.acknowledged.insert(id);
        }
        // A pair that clears and comes back blinks again.
        let alerts = &world.alerts;
        self.acknowledged_pairs.retain(|p| alerts.separation.contains(p) || alerts.conflict.contains(p));
        self.acknowledged_low.retain(|id| alerts.terrain.contains(id));
        if let Some(id) = self.selected {
            for &p in alerts.separation.iter().chain(&alerts.conflict) {
                if p.0 == id || p.1 == id {
                    self.acknowledged_pairs.insert(p);
                }
            }
            if alerts.terrain.contains(&id) {
                self.acknowledged_low.insert(id);
            }
        }
    }

    fn handle_keys(&mut self, world: &mut World) {
        let fine = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        let heading_step = if fine { 1.0 } else { 5.0 };

        if is_key_pressed(KeyCode::Tab) {
            self.cycle_selection(world);
        }
        if self.replay {
            self.view_keys(world);
            return;
        }
        if is_key_pressed(KeyCode::Left) {
            self.nudge_heading(world, -heading_step);
        }
        if is_key_pressed(KeyCode::Right) {
            self.nudge_heading(world, heading_step);
        }
        if is_key_pressed(KeyCode::Up) {
            self.nudge_altitude(world, 1000.0);
        }
        if is_key_pressed(KeyCode::Down) {
            self.nudge_altitude(world, -1000.0);
        }
        if is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd) {
            self.nudge_speed(world, 10.0);
        }
        if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) {
            self.nudge_speed(world, -10.0);
        }
        if self.selected.is_some() {
            if is_key_pressed(KeyCode::C) {
                self.cycle_approach(world);
            }

            if is_key_pressed(KeyCode::R) {
                self.pending.set_resume();
            }
            if is_key_pressed(KeyCode::G) {
                // On a takeoff roll: stop. Otherwise: go around.
                let rolling = self
                    .selected
                    .and_then(|id| world.find(id))
                    .is_some_and(|a| a.on_ground && a.phase == Phase::TowerRelease);
                if rolling {
                    self.pending.add_tower(Clearance::StopTakeoff);
                } else {
                    self.go_around(world);
                }
            }
            if is_key_pressed(KeyCode::H) {
                self.hold(world);
            }
            // D: approve the deviation round weather it asked for.
            if is_key_pressed(KeyCode::D)
                && self.selected.and_then(|id| world.find(id)).is_some_and(|a| a.deviation.is_some_and(|d| !d.approved))
            {
                self.pending.add_tower(Clearance::Deviation);
            }
            if is_key_pressed(KeyCode::X) {
                self.cross(world);
            }
            if is_key_pressed(KeyCode::W) {
                self.departure(world, false);
            }
            if is_key_pressed(KeyCode::K) {
                self.departure(world, true);
            }
            if is_key_pressed(KeyCode::L) {
                self.land(world);
            }
            if is_key_pressed(KeyCode::E) {
                self.enter_or_extend(world);
            }
            if is_key_pressed(KeyCode::B) {
                self.turn_base(world);
            }
            if is_key_pressed(KeyCode::O) {
                self.vfr_landing(world, Landing::Option);
            }
            if is_key_pressed(KeyCode::U) {
                self.vfr_landing(world, Landing::TouchAndGo);
            }
            if is_key_pressed(KeyCode::I) {
                self.ifr(world);
            }
            if is_key_pressed(KeyCode::S) {
                self.cycle_sid(world);
            }
        }
        // G with nothing selected: the oldest go-around call owed to Approach.
        if self.selected.is_none() && is_key_pressed(KeyCode::G) {
            self.go_around(world);
        }
        if is_key_pressed(KeyCode::T) {
            self.handoff(world);
        }
        if is_key_pressed(KeyCode::Y) {
            self.coordinate(world, true);
        }
        if is_key_pressed(KeyCode::F) {
            self.toggle_call_for_release(world);
        }
        if is_key_pressed(KeyCode::N) {
            self.coordinate(world, false);
        }
        if (is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)) && !self.enter_taken {
            self.transmit(world);
        }
        // Escape opens the pause menu, handled by the app.
        if is_key_pressed(KeyCode::Backspace) || is_key_pressed(KeyCode::Delete) {
            self.cancel();
        }
        self.view_keys(world);
    }

    /// Keys that change what's shown, not what's said: these work in a replay too.
    fn view_keys(&mut self, world: &World) {
        if is_key_pressed(KeyCode::F4) {
            self.course_hint = !self.course_hint;
        }
        if is_key_pressed(KeyCode::F1) {
            self.show(View::Radar, &world.airport);
        }
        if is_key_pressed(KeyCode::F2) {
            self.show(View::Surface, &world.airport);
        }
        if is_key_pressed(KeyCode::F3) {
            self.board.expanded = !self.board.expanded;
        }
        if is_key_pressed(KeyCode::Space) {
            self.paused = !self.paused;
        }
        if is_key_pressed(KeyCode::Key1) {
            self.rate = 1.0;
        }
        if is_key_pressed(KeyCode::Key2) {
            self.rate = 2.0;
        }
        if is_key_pressed(KeyCode::Key4) {
            self.rate = 4.0;
        }
        if self.replay && is_key_pressed(KeyCode::Key8) {
            self.rate = 8.0;
        }
        if is_key_pressed(KeyCode::M) {
            self.show_mva = !self.show_mva;
        }
    }

    fn handle_mouse(&mut self, world: &World) {
        let mouse = Vec2::from(mouse_position());
        if self.replay && mouse.y > screen_height() - super::debrief::BAR_HEIGHT && mouse.x < scope_width() {
            // The replay bar has the mouse.
            return;
        }
        let in_inset = self.has_inset() && inset_area().contains(mouse);
        let in_main = mouse_in_scope() && !in_inset;
        let (_, wheel) = mouse_wheel();
        let factor = if wheel > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
        let surface_main = self.main_view == View::Surface;

        if in_inset && wheel != 0.0 {
            // The inset zooms about its own centre.
            let inset = if surface_main { &mut self.camera } else { &mut self.surface };
            let centre = inset.viewport.center();
            inset.zoom_at(centre, factor);
        }
        if in_inset && is_mouse_button_pressed(MouseButton::Left) {
            // The inset is a working view too: pick aircraft from it.
            if surface_main {
                self.press_left(world, mouse);
            } else {
                self.surface_click(world, mouse);
            }
        }
        if in_main {
            if wheel != 0.0 {
                let main = if surface_main { &mut self.surface } else { &mut self.camera };
                main.zoom_at(mouse, factor);
            }
            if is_mouse_button_pressed(MouseButton::Right) {
                self.drag = Drag::Pan { last: mouse, surface: surface_main };
            }
            if is_mouse_button_pressed(MouseButton::Left) {
                if surface_main {
                    self.surface_click(world, mouse);
                } else {
                    self.press_left(world, mouse);
                }
            }
        }

        match self.drag {
            Drag::Pan { last, surface } => {
                let camera = if surface { &mut self.surface } else { &mut self.camera };
                camera.pan_pixels(mouse - last);
                self.drag = Drag::Pan { last: mouse, surface };
                if !is_mouse_button_down(MouseButton::Right) {
                    self.drag = Drag::None;
                }
            }
            Drag::Block { id, grab } => {
                if let Some(track) = world.track(id) {
                    let target = self.camera.to_screen(track.pos);
                    self.block_offsets.insert(id, mouse - grab - target);
                }
                if !is_mouse_button_down(MouseButton::Left) {
                    self.drag = Drag::None;
                }
            }
            Drag::Vector { id, start, active } => {
                let active = active || mouse.distance(start) >= DRAG_START_PX;
                self.drag = Drag::Vector { id, start, active };
                if !is_mouse_button_down(MouseButton::Left) {
                    if active {
                        self.finish_vector(world, id, mouse);
                    }
                    self.drag = Drag::None;
                }
            }
            Drag::None => {}
        }
    }

    /// A click on the surface map: pick an aircraft, or tell the selected one
    /// where to taxi. Ctrl-click a taxiway to route via it.
    fn surface_click(&mut self, world: &World, mouse: Vec2) {
        let Some(map) = world.airport.surface.as_ref() else { return };
        if let Some(id) = surface::aircraft_at(world, &self.surface, mouse) {
            if !self.follow(world, id) {
                self.select(Some(id));
            }
            return;
        }
        let selected = self.selected.and_then(|id| world.find(id)).filter(|a| a.on_ground);
        if selected.is_some() {
            if ctrl_down() {
                if let Some(name) = surface::taxiway_at(map, &self.surface, mouse) {
                    self.toggle_via(name);
                }
                return;
            }
            if let Some(node) = surface::node_at(map, &self.surface, mouse) {
                self.pending.add_tower(Clearance::Taxi { to: node, via: self.via.clone() });
                return;
            }
        }
        if self.pending.is_empty() {
            self.select(None);
        }
    }

    fn press_left(&mut self, world: &World, mouse: Vec2) {
        // Data blocks first: they're what the player grabs to untangle a busy scope.
        for track in world.radar.iter().rev() {
            let origin = scope::block_origin(self, track.id, track.pos);
            if Rect::new(origin.x, origin.y, BLOCK_SIZE.x, BLOCK_SIZE.y).contains(mouse) {
                if self.follow(world, track.id) {
                    return;
                }
                self.select(Some(track.id));
                self.drag = Drag::Block { id: track.id, grab: mouse - origin };
                return;
            }
        }
        for track in world.radar.iter().rev() {
            if self.camera.to_screen(track.pos).distance(mouse) <= TARGET_HIT_PX {
                if self.follow(world, track.id) {
                    return;
                }
                self.select(Some(track.id));
                // Only Approach vectors; a tower controller just picks the target.
                if self.staffing.approach {
                    self.drag = Drag::Vector { id: track.id, start: mouse, active: false };
                }
                return;
            }
        }
        if self.pending.is_empty() {
            self.select(None);
        }
    }

    /// Fix the cursor is close enough to snap a drag onto.
    pub fn fix_under(&self, world: &World, mouse: Vec2) -> Option<usize> {
        world
            .airport
            .fixes
            .iter()
            .position(|f| self.camera.to_screen(f.pos()).distance(mouse) <= FIX_SNAP_PX)
    }

    fn finish_vector(&mut self, world: &World, id: u32, mouse: Vec2) {
        if let Some(fix) = self.fix_under(world, mouse) {
            match world.find(id).filter(|_| ctrl_down()) {
                Some(ac) => self.pending.set_hold(world.hold_for(ac, fix)),
                None => self.pending.set_direct(fix),
            }
        } else if let Some(heading) = self.drag_heading(world, id, mouse) {
            self.pending.set_heading(heading);
        }
    }

    /// Heading a drag is pointing at, rounded to the nearest five degrees.
    pub fn drag_heading(&self, world: &World, id: u32, mouse: Vec2) -> Option<f32> {
        let track = world.track(id)?;
        let to_mouse = self.camera.to_world(mouse) - track.pos;
        if to_mouse.length() < 1e-3 {
            return None;
        }
        // Round in magnetic, the headings a controller actually assigns.
        let magnetic = world.airport.to_magnetic(vec_heading(to_mouse));
        Some(world.airport.to_true((magnetic / 5.0).round() * 5.0))
    }
}

/// Either Control key, or Command on a Mac: Ctrl-drag holds, Ctrl-click
/// routes via a taxiway.
pub fn ctrl_down() -> bool {
    [KeyCode::LeftControl, KeyCode::RightControl, KeyCode::LeftSuper, KeyCode::RightSuper]
        .into_iter()
        .any(is_key_down)
}

/// The next runway an aircraft has to cross that it isn't cleared across.
fn crossing_ahead(world: &World, ac: &crate::sim::aircraft::Aircraft) -> Option<usize> {
    let (Some(taxi), Some(map)) = (&ac.taxi, world.airport.surface.as_ref()) else { return None };
    taxi.holding_short.or_else(|| {
        taxi.path[taxi.index.min(taxi.path.len())..]
            .iter()
            .flat_map(|&n| map.nodes[n].zones.iter().copied())
            .find(|z| !taxi.cleared.contains(z))
    })
}
