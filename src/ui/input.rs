//! Selection, drag vectors, hotkeys and the pending clearance.

use super::camera::{Camera, inset_area, mouse_in_scope, scope_area};
use super::scope::{self, BLOCK_SIZE};
use super::surface;
use crate::sim::World;
use crate::sim::airspace::Airport;
use crate::sim::aircraft::{Flow, Landing, Leg, Phase};
use crate::sim::clearance::{Clearance, PendingClearance};
use crate::sim::positions::{Position, Staffing};
use crate::sim::units::vec_heading;
use macroquad::prelude::*;
use std::collections::HashMap;

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
}

impl UiState {
    pub fn new(airport: &Airport, staffing: Staffing) -> UiState {
        let main_view = if staffing.approach { View::Radar } else { View::Surface };
        let mut ui = UiState {
            camera: Camera::fit_sector(airport),
            surface: Camera::fit_surface(airport),
            main_view,
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
        };
        ui.frame_views(airport);
        ui
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
        if let Some(id) = self.selected
            && world.transmit(id, self.pending.to_clearances()) {
                self.pending = PendingClearance::default();
            }
    }

    /// One action for all handoffs: accept from Center, or pass the aircraft on
    /// to whoever is next: Tower, Ground, Departure or Center.
    pub fn handoff(&mut self, world: &mut World) {
        let Some(id) = self.selected else { return };
        let Some(ac) = world.find(id) else { return };
        match (ac.phase, ac.flow) {
            (Phase::Offered, _) => {
                world.accept_handoff(id);
            }
            _ if ac.on_ground => match ac.frequency() {
                Position::Ground => self.pending.add_tower(Clearance::ContactTower),
                Position::Local => self.pending.add_tower(Clearance::ContactGround),
                _ => {}
            },
            (Phase::TowerRelease | Phase::GoAround | Phase::Pattern, _) => {
                self.pending.add_tower(Clearance::ContactDeparture)
            }
            (_, Flow::Arrival) => self.pending.handoff_tower = true,
            (_, Flow::Departure) => self.pending.handoff_center = true,
        }
    }

    /// Hold position, or continue if already holding.
    pub fn hold(&mut self, world: &World) {
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return };
        let holding = ac.taxi.as_ref().is_some_and(|t| t.holding);
        self.pending.add_tower(if holding { Clearance::ContinueTaxi } else { Clearance::HoldPosition });
    }

    /// Cross the runway it's holding short of, or the next one on its route.
    pub fn cross(&mut self, world: &World) {
        let Some(ac) = self.selected.and_then(|id| world.find(id)) else { return };
        let (Some(taxi), Some(map)) = (&ac.taxi, world.airport.surface.as_ref()) else { return };
        let ahead = taxi.holding_short.or_else(|| {
            taxi.path[taxi.index.min(taxi.path.len())..]
                .iter()
                .flat_map(|&n| map.nodes[n].zones.iter().copied())
                .find(|z| !taxi.cleared.contains(z))
        });
        if let Some(runway) = ahead {
            self.pending.add_tower(Clearance::CrossRunway(runway));
        }
    }

    /// Line up and wait, or cleared for takeoff, on the aircraft's departure runway.
    pub fn departure(&mut self, world: &World, takeoff: bool) {
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
        if !shift || ac.vfr.is_none() || ac.on_ground || ac.id == leader {
            return false;
        }
        let traffic = crate::sim::vfr::traffic_description(lead);
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

    pub fn clear_for_ils(&mut self, world: &World) {
        self.pending.ils = Some(world.arrival_runway());
    }

    pub fn nudge_heading(&mut self, world: &World, degrees: f32) {
        if let Some(ac) = self.selected.and_then(|id| world.find(id)) {
            self.pending.nudge_heading(ac, degrees);
        }
    }

    pub fn nudge_altitude(&mut self, world: &World, feet: f32) {
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
            .filter(|a| world.is_players(a) || a.phase == Phase::Offered)
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
    }

    fn handle_keys(&mut self, world: &mut World) {
        let fine = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        let heading_step = if fine { 1.0 } else { 5.0 };

        if is_key_pressed(KeyCode::Tab) {
            self.cycle_selection(world);
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
                self.clear_for_ils(world);
            }
            if is_key_pressed(KeyCode::R) {
                self.pending.set_resume();
            }
            if is_key_pressed(KeyCode::G) {
                self.pending.go_around = true;
            }
            if is_key_pressed(KeyCode::H) {
                self.hold(world);
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
        }
        if is_key_pressed(KeyCode::F1) {
            self.show(View::Radar, &world.airport);
        }
        if is_key_pressed(KeyCode::F2) {
            self.show(View::Surface, &world.airport);
        }
        if is_key_pressed(KeyCode::T) {
            self.handoff(world);
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
            self.transmit(world);
        }
        // Escape opens the pause menu, handled by the app.
        if is_key_pressed(KeyCode::Backspace) || is_key_pressed(KeyCode::Delete) {
            self.cancel();
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
        if is_key_pressed(KeyCode::M) {
            self.show_mva = !self.show_mva;
        }
    }

    fn handle_mouse(&mut self, world: &World) {
        let mouse = Vec2::from(mouse_position());
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
            if is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl) {
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
            self.pending.set_direct(fix);
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
