//! Macroquad front end: screens, the fixed-timestep loop, and drawing.

pub mod audio;
pub mod camera;
pub mod input;
pub mod panels;
pub mod scope;

use crate::sim::airspace::{Airport, CATALOG};
use crate::sim::scoring::ShiftEnd;
use crate::sim::traffic::Busyness;
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
    "DEPARTURES  They check in climbing out. Climb them to the altitude on their strip,",
    "          and hand to Center (T) before they leave the sector near their exit fix.",
    "",
    "SEPARATE  3 NM or 1,000 ft. Wake spacing on final: Heavy>Heavy 4, Heavy>Large 5,",
    "          Heavy>Small 6, Large>Small 4, otherwise 3. Stay above the MVA.",
    "",
    "CONTROLS  Click/Tab select | Drag from target: heading, or onto a fix: direct | Arrows: hdg, alt",
    "          +/-: speed | C ILS | R route | T handoff | G go-around | ENTER send | BKSP cancel",
];

/// Pause menu entries, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PauseItem {
    Resume,
    Restart,
    Sound,
    Controls,
    MainMenu,
    Quit,
}

const PAUSE_ITEMS: [PauseItem; 6] = [
    PauseItem::Resume,
    PauseItem::Restart,
    PauseItem::Sound,
    PauseItem::Controls,
    PauseItem::MainMenu,
    PauseItem::Quit,
];
const PAUSE_ROW: f32 = 40.0;
const PAUSE_WIDTH: f32 = 340.0;
const BRIEFING_VIEW: &str =
    "          SPACE pause | ESC menu | 1 2 4 rate | M terrain | Wheel zoom | Right-drag pan";

enum Screen {
    Menu,
    Playing,
    /// The sim is frozen behind the pause menu.
    Paused { choice: usize, show_controls: bool },
    Ended(ShiftEnd),
}

pub struct App {
    screen: Screen,
    world: World,
    ui: UiState,
    sounds: Sounds,
    accumulator: f32,
    airports: Vec<Airport>,
    choice: usize,
    busyness: Busyness,
}

fn new_world(airport: &Airport, busyness: Busyness) -> World {
    let seed = macroquad::miniquad::date::now() as u64;
    World::new(airport.clone(), WorldConfig { seed, busyness, ..Default::default() })
}

impl App {
    pub async fn new() -> App {
        let airports: Vec<Airport> = CATALOG
            .iter()
            .map(|text| Airport::from_toml(text).expect("built-in airport data is valid"))
            .collect();
        App {
            screen: Screen::Menu,
            world: new_world(&airports[0], Busyness::default()),
            ui: UiState::new(&airports[0]),
            sounds: Sounds::load().await,
            accumulator: 0.0,
            airports,
            choice: 0,
            busyness: Busyness::default(),
        }
    }

    fn start_shift(&mut self) {
        let airport = &self.airports[self.choice];
        self.world = new_world(airport, self.busyness);
        self.ui = UiState::new(airport);
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    pub fn frame(&mut self) {
        match self.screen {
            Screen::Menu => {
                let count = self.airports.len();
                if is_key_pressed(KeyCode::Up) {
                    self.choice = (self.choice + count - 1) % count;
                }
                if is_key_pressed(KeyCode::Down) {
                    self.choice = (self.choice + 1) % count;
                }
                if is_key_pressed(KeyCode::Left) {
                    self.busyness = self.busyness.step(-1);
                }
                if is_key_pressed(KeyCode::Right) {
                    self.busyness = self.busyness.step(1);
                }
                if let Some(picked) = draw_menu(&self.airports, self.choice, self.busyness) {
                    self.busyness = picked;
                }
                if start_pressed() {
                    self.start_shift();
                }
            }
            Screen::Playing => self.play_frame(),
            Screen::Paused { choice, show_controls } => self.pause_frame(choice, show_controls),
            Screen::Ended(end) => {
                draw_end(&self.world, end);
                if start_pressed() {
                    self.start_shift();
                } else if is_key_pressed(KeyCode::Escape) {
                    self.screen = Screen::Menu;
                }
            }
        }
    }

    fn pause_frame(&mut self, mut choice: usize, mut show_controls: bool) {
        scope::draw(&self.world, &self.ui);
        let count = PAUSE_ITEMS.len();
        if is_key_pressed(KeyCode::Up) {
            choice = (choice + count - 1) % count;
        }
        if is_key_pressed(KeyCode::Down) {
            choice = (choice + 1) % count;
        }
        let (hovered, clicked) = draw_pause(choice, self.sounds.muted, show_controls);
        // The mouse only takes over the highlight when it moves, so arrow keys still work.
        let mouse_moved = mouse_delta_position() != Vec2::ZERO;
        if let Some(index) = hovered.filter(|_| mouse_moved || clicked) {
            choice = index;
        }
        let activate = clicked || start_pressed();

        self.screen = Screen::Paused { choice, show_controls };
        if is_key_pressed(KeyCode::Escape) {
            self.resume();
            return;
        }
        if !activate {
            return;
        }
        match PAUSE_ITEMS[choice] {
            PauseItem::Resume => self.resume(),
            PauseItem::Restart => self.start_shift(),
            PauseItem::Sound => self.sounds.muted = !self.sounds.muted,
            PauseItem::Controls => {
                show_controls = !show_controls;
                self.screen = Screen::Paused { choice, show_controls };
            }
            PauseItem::MainMenu => self.screen = Screen::Menu,
            PauseItem::Quit => macroquad::miniquad::window::order_quit(),
        }
    }

    fn resume(&mut self) {
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    fn play_frame(&mut self) {
        if is_key_pressed(KeyCode::Escape) {
            self.ui.drag = input::Drag::None;
            self.screen = Screen::Paused { choice: 0, show_controls: false };
            scope::draw(&self.world, &self.ui);
            return;
        }
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

/// Five-notch slider for traffic level. Returns a level if one was clicked.
fn draw_busyness(busyness: Busyness, real_data: bool, x: f32, y: f32) -> Option<Busyness> {
    const SPACING: f32 = 130.0;
    const HIT: f32 = 14.0;
    draw_text("TRAFFIC  Left/Right or click to choose", x, y, 20.0, DIM);
    let track = y + 30.0;
    let first = x + 40.0;
    let last = first + SPACING * (Busyness::ALL.len() - 1) as f32;
    draw_line(first, track, last, track, 3.0, DIM);

    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut picked = None;
    for (index, &level) in Busyness::ALL.iter().enumerate() {
        let cx = first + SPACING * index as f32;
        let selected = level == busyness;
        if selected {
            draw_circle(cx, track, 10.0, TITLE);
        } else {
            draw_circle(cx, track, 6.0, TEXT);
        }
        let label = level.label(real_data);
        let width = measure_text(label, None, 18, 1.0).width;
        draw_text(label, cx - width / 2.0, track + 30.0, 18.0, if selected { TITLE } else { TEXT });
        if clicked && mouse.distance(Vec2::new(cx, track)) < HIT {
            picked = Some(level);
        }
    }
    let note = format!("x{} the airport's normal traffic", busyness.multiplier());
    draw_text(&note, x, track + 56.0, 18.0, DIM);
    picked
}

fn draw_menu(airports: &[Airport], choice: usize, busyness: Busyness) -> Option<Busyness> {
    clear_background(BACKGROUND);
    let x = 60.0;
    draw_text("APPROACH CONTROL", x, 90.0, 56.0, TITLE);
    draw_text("30 minute shift", x, 124.0, 24.0, DIM);

    let mut y = 180.0;
    for line in BRIEFING.iter().chain(std::iter::once(&BRIEFING_VIEW)) {
        draw_text(line, x, y, 20.0, TEXT);
        y += 24.0;
    }
    y += 20.0;
    draw_text("AIRPORT  Up/Down to choose", x, y, 20.0, DIM);
    y += 28.0;
    for (index, airport) in airports.iter().enumerate() {
        let selected = index == choice;
        let marker = if selected { ">" } else { " " };
        let line = format!("{marker} {}  {}", airport.code, airport.name);
        draw_text(&line, x, y, 24.0, if selected { TITLE } else { TEXT });
        y += 28.0;
    }
    let airport = &airports[choice];
    if let Some(source) = &airport.data_source {
        draw_text(source, x, y, 18.0, DIM);
        y += 22.0;
        draw_text("Provided as is, with no warranty. See DATA_NOTICE.md.", x, y, 18.0, DIM);
        y += 22.0;
    }
    y += 24.0;
    let picked = draw_busyness(busyness, airport.traffic.is_some(), x, y);
    draw_text("Press ENTER to start your shift", x, y + 130.0, 30.0, TITLE);
    picked
}

fn pause_label(item: PauseItem, muted: bool, show_controls: bool) -> &'static str {
    match item {
        PauseItem::Resume => "Resume",
        PauseItem::Restart => "Restart shift",
        PauseItem::Sound if muted => "Sound: Off",
        PauseItem::Sound => "Sound: On",
        PauseItem::Controls if show_controls => "Hide controls",
        PauseItem::Controls => "Show controls",
        PauseItem::MainMenu => "Quit to main menu",
        PauseItem::Quit => "Quit to desktop",
    }
}

/// Pause menu over the frozen scope. Returns the row under the mouse and whether it was clicked.
fn draw_pause(choice: usize, muted: bool, show_controls: bool) -> (Option<usize>, bool) {
    draw_rectangle(0.0, 0.0, screen_width(), screen_height(), Color::new(0.0, 0.0, 0.0, 0.65));

    let height = 70.0 + PAUSE_ROW * PAUSE_ITEMS.len() as f32 + 40.0;
    let left = (screen_width() - PAUSE_WIDTH) / 2.0;
    let top = (screen_height() - height) / 2.0;
    draw_rectangle(left, top, PAUSE_WIDTH, height, BACKGROUND);
    draw_rectangle_lines(left, top, PAUSE_WIDTH, height, 1.5, DIM);
    let title = "PAUSED";
    let size = measure_text(title, None, 36, 1.0);
    draw_text(title, left + (PAUSE_WIDTH - size.width) / 2.0, top + 46.0, 36.0, TITLE);

    let mouse = Vec2::from(mouse_position());
    let mut hovered = None;
    for (index, &item) in PAUSE_ITEMS.iter().enumerate() {
        let y = top + 70.0 + PAUSE_ROW * index as f32;
        let row = Rect::new(left + 16.0, y, PAUSE_WIDTH - 32.0, PAUSE_ROW - 6.0);
        if row.contains(mouse) {
            hovered = Some(index);
        }
        let selected = index == choice;
        if selected {
            draw_rectangle(row.x, row.y, row.w, row.h, Color::new(0.10, 0.22, 0.12, 1.0));
        }
        let label = pause_label(item, muted, show_controls);
        let width = measure_text(label, None, 24, 1.0).width;
        let color = if selected { TITLE } else { TEXT };
        draw_text(label, left + (PAUSE_WIDTH - width) / 2.0, y + 24.0, 24.0, color);
    }
    draw_text(
        "Up/Down  ENTER select  ESC resume",
        left + 34.0,
        top + height - 16.0,
        18.0,
        DIM,
    );

    if show_controls {
        let lines: Vec<&str> =
            BRIEFING[11..].iter().copied().chain(std::iter::once(BRIEFING_VIEW)).collect();
        let box_top = top + height + 16.0;
        let box_left = 40.0;
        let box_width = screen_width() - 80.0;
        draw_rectangle(box_left, box_top, box_width, 24.0 * lines.len() as f32 + 20.0, BACKGROUND);
        for (i, line) in lines.iter().enumerate() {
            draw_text(line, box_left + 16.0, box_top + 28.0 + 24.0 * i as f32, 20.0, TEXT);
        }
    }

    let clicked = hovered.is_some() && is_mouse_button_pressed(MouseButton::Left);
    (hovered, clicked)
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
