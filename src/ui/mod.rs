//! Macroquad front end: screens, the fixed-timestep loop, and drawing.

pub mod audio;
pub mod camera;
pub mod input;
pub mod panels;
pub mod scope;

use crate::sim::airspace::Airport;
use crate::sim::scoring::ShiftEnd;
use crate::sim::{TICK, World, WorldConfig};
use audio::Sounds;
use input::UiState;
use macroquad::prelude::*;

/// Longest frame time to catch up on, so a stall doesn't fast-forward the sim.
const MAX_FRAME_SECONDS: f32 = 0.25;

const TITLE: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const TEXT: Color = Color::new(0.85, 0.90, 0.88, 1.0);
const DIM: Color = Color::new(0.55, 0.62, 0.62, 1.0);
const BAD: Color = Color::new(1.00, 0.45, 0.40, 1.0);
const BACKGROUND: Color = Color::new(0.02, 0.05, 0.05, 1.0);

const BRIEFING: [&str; 13] = [
    "You are the approach radar controller. Tower and Center are automated.",
    "",
    "ARRIVALS  Accept from Center (T). Descend and vector them to the final approach course,",
    "          at 30 degrees or less, at or below the glideslope. Clear for the ILS (C).",
    "          Once on LOC or GS, hand to Tower (T) before 5 NM final.",
    "DEPARTURES  They check in climbing to 5,000. Climb them to the altitude on their strip,",
    "          and hand to Center (T) before they leave the sector near their exit fix.",
    "",
    "SEPARATE  3 NM or 1,000 ft. Wake spacing on final: Heavy>Heavy 4, Heavy>Large 5,",
    "          Heavy>Small 6, Large>Small 4, otherwise 3. Stay above the MVA.",
    "",
    "CONTROLS  Click/Tab select | Drag from target: heading, or onto a fix: direct | Arrows: hdg, alt",
    "          +/-: speed | C ILS | R route | T handoff | G go-around | ENTER send | ESC cancel",
];
const BRIEFING_VIEW: &str =
    "          SPACE pause | 1 2 4 rate | M terrain | Wheel zoom | Right-drag pan";

enum Screen {
    Menu,
    Playing,
    Ended(ShiftEnd),
}

pub struct App {
    screen: Screen,
    world: World,
    ui: UiState,
    sounds: Sounds,
    accumulator: f32,
}

fn new_world() -> World {
    let seed = macroquad::miniquad::date::now() as u64;
    World::new(Airport::default_airport(), WorldConfig { seed, ..Default::default() })
}

impl App {
    pub async fn new() -> App {
        App {
            screen: Screen::Menu,
            world: new_world(),
            ui: UiState::new(),
            sounds: Sounds::load().await,
            accumulator: 0.0,
        }
    }

    fn start_shift(&mut self) {
        self.world = new_world();
        self.ui = UiState::new();
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    pub fn frame(&mut self) {
        match self.screen {
            Screen::Menu => {
                draw_menu();
                if start_pressed() {
                    self.start_shift();
                }
            }
            Screen::Playing => self.play_frame(),
            Screen::Ended(end) => {
                draw_end(&self.world, end);
                if start_pressed() {
                    self.start_shift();
                }
            }
        }
    }

    fn play_frame(&mut self) {
        self.ui.update(&mut self.world);

        if !self.ui.paused {
            self.accumulator += get_frame_time().min(MAX_FRAME_SECONDS) * self.ui.rate;
            while self.accumulator >= TICK {
                self.accumulator -= TICK;
                self.world.tick();
            }
        }
        let events = self.world.drain_events();
        self.sounds.play_for(&events);

        scope::draw(&self.world, &self.ui);
        panels::draw(&mut self.world, &mut self.ui);

        if let Some(end) = self.world.ended {
            self.screen = Screen::Ended(end);
        }
    }
}

fn start_pressed() -> bool {
    is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)
}

fn draw_menu() {
    clear_background(BACKGROUND);
    let x = 60.0;
    draw_text("APPROACH CONTROL", x, 90.0, 56.0, TITLE);
    draw_text("Harrowgate International - 30 minute shift", x, 124.0, 24.0, DIM);

    let mut y = 180.0;
    for line in BRIEFING.iter().chain(std::iter::once(&BRIEFING_VIEW)) {
        draw_text(line, x, y, 20.0, TEXT);
        y += 24.0;
    }
    draw_text("Press ENTER to start your shift", x, y + 30.0, 30.0, TITLE);
}

fn draw_end(world: &World, end: ShiftEnd) {
    clear_background(BACKGROUND);
    let x = 60.0;
    let score = &world.score;
    let headline = match end {
        ShiftEnd::Completed => "SHIFT COMPLETE",
        ShiftEnd::Decertified => "DECERTIFIED",
        ShiftEnd::NearCollision => "NEAR COLLISION",
    };
    let color = if end == ShiftEnd::Completed { TITLE } else { BAD };
    draw_text(headline, x, 90.0, 56.0, color);
    draw_text(format!("Rating: {}", score.rating(end)), x, 130.0, 28.0, TEXT);
    draw_text(
        format!(
            "Score {}   Landed {}   Handed off {}   Go-arounds {}   Separation losses {}",
            score.points, score.landed, score.exited, score.go_arounds, score.separation_losses
        ),
        x,
        162.0,
        22.0,
        DIM,
    );

    draw_text("Incidents", x, 206.0, 24.0, TEXT);
    let mut y = 232.0;
    if score.incidents.is_empty() {
        draw_text("None. A clean shift.", x, y, 20.0, DIM);
    }
    // Most recent first, as many as fit above the restart prompt.
    let room = ((screen_height() - y - 70.0) / 22.0).max(0.0) as usize;
    for incident in score.incidents.iter().rev().take(room) {
        let line = format!(
            "{:02}:{:02}  {:+5}  {}",
            (incident.time / 60.0) as i32,
            (incident.time % 60.0) as i32,
            incident.points,
            incident.text
        );
        draw_text(&line, x, y, 20.0, if incident.points < 0 { BAD } else { DIM });
        y += 22.0;
    }
    draw_text("Press ENTER for another shift", x, screen_height() - 30.0, 28.0, TITLE);
}
