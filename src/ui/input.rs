//! Selection, drag vectors, hotkeys and the pending clearance.

use super::camera::{Camera, mouse_in_scope};
use super::scope::{self, BLOCK_SIZE};
use crate::sim::World;
use crate::sim::aircraft::{Flow, Phase};
use crate::sim::clearance::PendingClearance;
use crate::sim::units::{normalize_heading, vec_heading};
use macroquad::prelude::*;
use std::collections::HashMap;

const TARGET_HIT_PX: f32 = 12.0;
const FIX_SNAP_PX: f32 = 14.0;
const DRAG_START_PX: f32 = 8.0;
const ZOOM_STEP: f32 = 1.12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    None,
    /// Pulling a heading vector out of a target.
    Vector { id: u32, start: Vec2, active: bool },
    /// Moving a data block. `grab` is the cursor's offset within the block.
    Block { id: u32, grab: Vec2 },
    Pan { last: Vec2 },
}

pub struct UiState {
    pub camera: Camera,
    pub selected: Option<u32>,
    pub pending: PendingClearance,
    pub drag: Drag,
    /// Data block offsets from their targets, for blocks the player has moved.
    pub block_offsets: HashMap<u32, Vec2>,
    pub show_mva: bool,
    pub paused: bool,
    /// Sim seconds per real second.
    pub rate: f32,
}

impl UiState {
    pub fn new() -> UiState {
        UiState {
            camera: Camera::fit_sector(),
            selected: None,
            pending: PendingClearance::default(),
            drag: Drag::None,
            block_offsets: HashMap::new(),
            show_mva: true,
            paused: false,
            rate: 1.0,
        }
    }

    /// Select an aircraft. An unsent clearance never carries over to another aircraft.
    pub fn select(&mut self, id: Option<u32>) {
        if self.selected != id {
            self.selected = id;
            self.pending = PendingClearance::default();
        }
    }

    pub fn transmit(&mut self, world: &mut World) {
        if let Some(id) = self.selected
            && world.transmit(id, self.pending.to_clearances()) {
                self.pending = PendingClearance::default();
            }
    }

    /// One action for all handoffs: accept from Center, or pass on to Tower or Center.
    pub fn handoff(&mut self, world: &mut World) {
        let Some(id) = self.selected else { return };
        let Some(ac) = world.find(id) else { return };
        match (ac.phase, ac.flow) {
            (Phase::Offered, _) => {
                world.accept_handoff(id);
            }
            (_, Flow::Arrival) => self.pending.handoff_tower = true,
            (_, Flow::Departure) => self.pending.handoff_center = true,
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
        if self.pending.is_empty() {
            self.select(None);
        } else {
            self.pending = PendingClearance::default();
        }
    }

    /// Next aircraft in strip order that isn't Tower's or already gone.
    fn cycle_selection(&mut self, world: &World) {
        let ids: Vec<u32> = world
            .aircraft
            .iter()
            .filter(|a| a.on_player_frequency() || a.phase == Phase::Offered)
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
        }
        if is_key_pressed(KeyCode::T) {
            self.handoff(world);
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
            self.transmit(world);
        }
        if is_key_pressed(KeyCode::Escape) {
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

        if mouse_in_scope() {
            let (_, wheel) = mouse_wheel();
            if wheel != 0.0 {
                let factor = if wheel > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                self.camera.zoom_at(mouse, factor);
            }
            if is_mouse_button_pressed(MouseButton::Right) {
                self.drag = Drag::Pan { last: mouse };
            }
            if is_mouse_button_pressed(MouseButton::Left) {
                self.press_left(world, mouse);
            }
        }

        match self.drag {
            Drag::Pan { last } => {
                self.camera.pan_pixels(mouse - last);
                self.drag = Drag::Pan { last: mouse };
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

    fn press_left(&mut self, world: &World, mouse: Vec2) {
        // Data blocks first: they're what the player grabs to untangle a busy scope.
        for track in world.radar.iter().rev() {
            let origin = scope::block_origin(self, track.id, track.pos);
            if Rect::new(origin.x, origin.y, BLOCK_SIZE.x, BLOCK_SIZE.y).contains(mouse) {
                self.select(Some(track.id));
                self.drag = Drag::Block { id: track.id, grab: mouse - origin };
                return;
            }
        }
        for track in world.radar.iter().rev() {
            if self.camera.to_screen(track.pos).distance(mouse) <= TARGET_HIT_PX {
                self.select(Some(track.id));
                self.drag = Drag::Vector { id: track.id, start: mouse, active: false };
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
        Some(normalize_heading((vec_heading(to_mouse) / 5.0).round() * 5.0))
    }
}

impl Default for UiState {
    fn default() -> Self {
        UiState::new()
    }
}
