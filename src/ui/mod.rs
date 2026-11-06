//! Macroquad front end: screens, the fixed-timestep loop, and drawing.

pub mod audio;
pub mod camera;
pub mod input;
pub mod panels;
pub mod scope;
pub mod surface;
pub mod tutorial;

use crate::sim::airspace::{Airport, CATALOG};
use crate::sim::positions::Staffing;
use crate::sim::scoring::ShiftEnd;
use crate::sim::traffic::Busyness;
use crate::sim::tutorial::{LessonKind, Tutorial};
use crate::sim::{TICK, World, WorldConfig};
use audio::Sounds;
use input::{UiState, View};
use macroquad::prelude::*;

/// Longest frame time to catch up on, so a stall doesn't fast-forward the sim.
const MAX_FRAME_SECONDS: f32 = 0.25;

const TITLE: Color = Color::new(0.35, 0.95, 0.45, 1.0);
const TEXT: Color = Color::new(0.85, 0.90, 0.88, 1.0);
const DIM: Color = Color::new(0.55, 0.62, 0.62, 1.0);
const BAD: Color = Color::new(1.00, 0.45, 0.40, 1.0);
const BACKGROUND: Color = Color::new(0.02, 0.05, 0.05, 1.0);

const APPROACH_BRIEFING: [&str; 6] = [
    "ARRIVALS  Accept from Center (T). Descend and vector them to the final approach course,",
    "          at 30 degrees or less, at or below the glideslope. Clear for the ILS (C).",
    "DEPARTURES  They check in climbing out. Climb them to the altitude on their strip,",
    "          and hand to Center (T) before they leave the sector near their exit fix.",
    "SEPARATE  3 NM or 1,000 ft. Wake spacing on final: Heavy>Heavy 4, Heavy>Large 5,",
    "          Heavy>Small 6, Large>Small 4, otherwise 3. Stay above the MVA.",
];
const LOCAL_BRIEFING: [&str; 4] = [
    "LANDING  Clear arrivals to land (L) before 1 NM final, with the runway clear.",
    "TAKEOFF  Line up and wait (W) or cleared for takeoff (K) from the hold line. 90 s between.",
    "CROSSING  Anyone holding short of a runway waits for you to cross them (X).",
    "INCURSION  Nobody onto a runway with traffic inside 2 NM final or rolling.",
];
const GROUND_BRIEFING: [&str; 3] = [
    "TAXI     Click an aircraft, then a hold line, gate or intersection. Ctrl-click taxiways",
    "         first to route via them. Ground never crosses a runway: Tower does.",
    "SPACING  Keep taxiing traffic apart: two aircraft nose to nose can't pass.",
];
const RADAR_CONTROLS: &str =
    "CONTROLS  Click/Tab select | Drag: heading, or onto a fix: direct | Arrows hdg, alt | +/- speed | C ILS | R route";
const TOWER_CONTROLS: &str =
    "CONTROLS  H hold/continue | X cross | W line up | K takeoff | L land | G go-around | T contact next";
const COMMON_CONTROLS: &str = "          T handoff | ENTER send | BKSP cancel";
const VFR_CONTROLS: &str =
    "VFR       E enter/extend downwind | B base | O option | U touch-and-go | L full stop | Shift-click follow";
const VFR_BRIEFING: &str =
    "VFR      Light aircraft fly the pattern on their own. Sequence them, and clear every landing.";

/// What the player is told before a shift, for the positions they work.
fn briefing(staffing: Staffing, vfr: bool) -> Vec<&'static str> {
    let mut lines = vec![match (staffing.approach, staffing.local, staffing.ground) {
        (true, false, false) => "You work Approach radar. Tower, Ground and Center are automated.",
        (false, true, false) => "You work Local Control (Tower): the runways. Ground and Approach are automated.",
        (false, false, true) => "You work Ground Control: the taxiways. Tower and Approach are automated.",
        (false, true, true) => "You work Tower and Ground combined. Approach is automated.",
        _ => "You work every position: Approach, Tower and Ground. F1 radar, F2 surface.",
    }];
    lines.push("");
    if staffing.approach {
        lines.extend(APPROACH_BRIEFING);
    }
    if staffing.local {
        lines.extend(LOCAL_BRIEFING);
        if !staffing.ground {
            lines.push("HANDOFF  Hand arrivals to Ground (T) once clear of the runway.");
        }
        if !staffing.approach {
            lines.push("HANDOFF  Hand departures to Departure (T) before 3 NM.");
        }
    }
    if staffing.ground {
        lines.extend(GROUND_BRIEFING);
        if !staffing.local {
            lines.push("HANDOFF  Hand departures to Tower (T) when they hold short.");
        }
    }
    if vfr && staffing.local {
        lines.push(VFR_BRIEFING);
    }
    lines
}

/// The controls for the positions worked.
fn controls(staffing: Staffing) -> Vec<&'static str> {
    let mut lines = Vec::new();
    if staffing.approach {
        lines.push(RADAR_CONTROLS);
    }
    if staffing.tower() {
        lines.push(TOWER_CONTROLS);
    }
    if staffing.local {
        lines.push(VFR_CONTROLS);
    }
    lines.push(COMMON_CONTROLS);
    lines
}

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
    "          SPACE pause | ESC menu | 1 2 4 rate | M terrain | Wheel zoom | Right-drag pan | F1/F2 views";

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
    /// Index into `Staffing::CHOICES`.
    position: usize,
    /// VFR traffic, when a tower position is chosen.
    vfr: bool,
    /// The lesson being played, if this is a tutorial rather than a shift.
    tutorial: Option<Tutorial>,
}

fn new_world(airport: &Airport, busyness: Busyness, staffing: Staffing, vfr: bool) -> World {
    let seed = macroquad::miniquad::date::now() as u64;
    World::new(airport.clone(), WorldConfig { seed, busyness, staffing, vfr, ..Default::default() })
}

/// Both views, when the player has two: one full size, one inset.
fn draw_views(world: &World, ui: &mut UiState) {
    ui.layout();
    match (ui.has_inset(), ui.main_view) {
        (false, _) => scope::draw(world, ui, false),
        (true, View::Radar) => {
            scope::draw(world, ui, false);
            surface::draw(world, ui, true);
        }
        (true, View::Surface) => {
            surface::draw(world, ui, false);
            scope::draw(world, ui, true);
        }
    }
}

/// Every font size the UI draws text at.
const FONT_SIZES: [u16; 11] = [14, 16, 17, 18, 20, 22, 24, 28, 30, 36, 56];

/// Cache every printable ASCII glyph at every size before the first frame.
/// macroquad grows its glyph atlas on demand, and a resize partway through
/// drawing can leave every glyph sampling the wrong texture, so text draws as
/// solid blocks. Growing it once up front avoids that.
fn warm_font_cache() {
    let font = get_default_font();
    let chars: Vec<char> = (' '..='~').collect();
    let dpi = miniquad::window::dpi_scale();
    for size in FONT_SIZES {
        font.populate_font_cache(&chars, (size as f32 * dpi).ceil() as u16);
    }
}

impl App {
    pub async fn new() -> App {
        warm_font_cache();
        let airports: Vec<Airport> = CATALOG
            .iter()
            .map(|&(text, ground)| Airport::load(text, ground).expect("built-in airport data is valid"))
            .collect();
        App {
            screen: Screen::Menu,
            world: new_world(&airports[0], Busyness::default(), Staffing::default(), false),
            ui: UiState::new(&airports[0], Staffing::default()),
            sounds: Sounds::load().await,
            accumulator: 0.0,
            airports,
            choice: 0,
            busyness: Busyness::default(),
            position: 0,
            vfr: true,
            tutorial: None,
        }
    }

    fn staffing(&self) -> Staffing {
        Staffing::CHOICES[self.position]
    }

    fn start_shift(&mut self) {
        let airport = &self.airports[self.choice];
        self.world = new_world(airport, self.busyness, self.staffing(), self.vfr);
        self.ui = UiState::new(airport, self.staffing());
        self.tutorial = None;
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    fn start_lesson(&mut self, kind: LessonKind) {
        let tutorial = Tutorial::new(kind);
        self.world = tutorial.world();
        self.ui = UiState::new(&self.world.airport, kind.staffing());
        self.ui.lesson = true;
        self.tutorial = Some(tutorial);
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    /// Start again: the same lesson, or a new shift.
    fn restart(&mut self) {
        match self.tutorial.as_ref().map(|t| t.kind) {
            Some(kind) => self.start_lesson(kind),
            None => self.start_shift(),
        }
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
                if is_key_pressed(KeyCode::P) || is_key_pressed(KeyCode::Tab) {
                    self.position = (self.position + 1) % Staffing::CHOICES.len();
                }
                if is_key_pressed(KeyCode::V) && self.staffing().tower() {
                    self.vfr = !self.vfr;
                }
                let picked = draw_menu(&self.airports, self.choice, self.busyness, self.staffing(), self.vfr);
                if picked.vfr {
                    self.vfr = !self.vfr;
                }
                if let Some(level) = picked.busyness {
                    self.busyness = level;
                }
                if let Some(position) = picked.position {
                    self.position = position;
                }
                let lesson_keys = [KeyCode::F5, KeyCode::F6, KeyCode::F7];
                let lesson = picked
                    .lesson
                    .or_else(|| lesson_keys.iter().position(|&k| is_key_pressed(k)).map(|i| LessonKind::ALL[i]));
                if let Some(kind) = lesson {
                    self.start_lesson(kind);
                } else if start_pressed() {
                    self.start_shift();
                }
            }
            Screen::Playing => self.play_frame(),
            Screen::Paused { choice, show_controls } => self.pause_frame(choice, show_controls),
            Screen::Ended(end) => {
                draw_end(&self.world, end);
                if start_pressed() {
                    self.restart();
                } else if is_key_pressed(KeyCode::Escape) {
                    self.screen = Screen::Menu;
                }
            }
        }
    }

    fn pause_frame(&mut self, mut choice: usize, mut show_controls: bool) {
        draw_views(&self.world, &mut self.ui);
        let count = PAUSE_ITEMS.len();
        if is_key_pressed(KeyCode::Up) {
            choice = (choice + count - 1) % count;
        }
        if is_key_pressed(KeyCode::Down) {
            choice = (choice + 1) % count;
        }
        let (hovered, clicked) =
            draw_pause(choice, self.sounds.muted, show_controls, self.world.staffing());
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
            PauseItem::Restart => self.restart(),
            PauseItem::Sound => self.sounds.muted = !self.sounds.muted,
            PauseItem::Controls => {
                show_controls = !show_controls;
                self.screen = Screen::Paused { choice, show_controls };
            }
            PauseItem::MainMenu => {
                self.tutorial = None;
                self.screen = Screen::Menu;
            }
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
            draw_views(&self.world, &mut self.ui);
            return;
        }
        self.ui.layout();
        if let Some(tutorial) = &mut self.tutorial {
            if tutorial.finished() {
                // The lesson is over: Enter goes back to the menu.
                if start_pressed() {
                    self.tutorial = None;
                    self.screen = Screen::Menu;
                    return;
                }
            } else if tutorial.current().is_some_and(|s| s.read) {
                if start_pressed() {
                    tutorial.acknowledge();
                }
            } else if is_key_pressed(KeyCode::S) {
                tutorial.show_me(&mut self.world);
            }
        }
        // Enter on a lesson page turns the page; it doesn't also send a command.
        self.ui.enter_taken =
            self.tutorial.as_ref().is_some_and(|t| t.finished() || t.current().is_some_and(|s| s.read));
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

        if let Some(tutorial) = &mut self.tutorial {
            tutorial.update(&mut self.world);
        }

        draw_views(&self.world, &mut self.ui);
        panels::draw(&mut self.world, &mut self.ui);
        if let Some(tutorial) = &self.tutorial {
            tutorial::draw(tutorial, &self.world, &self.ui);
        }

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

/// Position choices as a row of buttons. Returns one if clicked.
fn draw_positions(staffing: Staffing, x: f32, y: f32) -> Option<usize> {
    const GAP: f32 = 10.0;
    draw_text("POSITION  P or click to choose", x, y, 20.0, DIM);
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut left = x;
    let mut picked = None;
    for (index, &choice) in Staffing::CHOICES.iter().enumerate() {
        let label = choice.label();
        let width = measure_text(label, None, 20, 1.0).width + 24.0;
        let rect = Rect::new(left, y + 10.0, width, 30.0);
        let selected = choice == staffing;
        if selected {
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, Color::new(0.10, 0.22, 0.12, 1.0));
        }
        draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, if selected { TITLE } else { DIM });
        draw_text(label, rect.x + 12.0, rect.y + 21.0, 20.0, if selected { TITLE } else { TEXT });
        if clicked && rect.contains(mouse) {
            picked = Some(index);
        }
        left += width + GAP;
    }
    picked
}

/// What the player changed on the menu this frame.
#[derive(Default)]
struct MenuPick {
    busyness: Option<Busyness>,
    position: Option<usize>,
    /// The VFR toggle was clicked.
    vfr: bool,
    /// A lesson button was clicked.
    lesson: Option<LessonKind>,
}

/// The lesson buttons. Returns the one clicked, if any.
fn draw_lessons(x: f32, y: f32) -> Option<LessonKind> {
    const GAP: f32 = 10.0;
    draw_text("TUTORIAL  F5 F6 F7 or click", x, y, 20.0, DIM);
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut left = x;
    let mut picked = None;
    for (kind, key) in LessonKind::ALL.into_iter().zip(["F5", "F6", "F7"]) {
        let label = format!("{key} {}", kind.label());
        let width = measure_text(&label, None, 20, 1.0).width + 24.0;
        let rect = Rect::new(left, y + 10.0, width, 30.0);
        draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, DIM);
        draw_text(&label, rect.x + 12.0, rect.y + 21.0, 20.0, TEXT);
        if clicked && rect.contains(mouse) {
            picked = Some(kind);
        }
        left += width + GAP;
    }
    picked
}

/// The VFR traffic toggle, for tower positions. True if clicked.
fn draw_vfr_toggle(vfr: bool, x: f32, y: f32) -> bool {
    draw_text("VFR TRAFFIC  V or click to toggle", x, y, 20.0, DIM);
    let label = if vfr { "On: pattern traffic, arrivals and departures" } else { "Off" };
    let rect = Rect::new(x + 330.0, y - 18.0, measure_text(label, None, 20, 1.0).width + 24.0, 26.0);
    draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, if vfr { TITLE } else { DIM });
    draw_text(label, rect.x + 12.0, rect.y + 19.0, 20.0, if vfr { TITLE } else { TEXT });
    is_mouse_button_pressed(MouseButton::Left) && rect.contains(Vec2::from(mouse_position()))
}

fn draw_menu(airports: &[Airport], choice: usize, busyness: Busyness, staffing: Staffing, vfr: bool) -> MenuPick {
    clear_background(BACKGROUND);
    let x = 60.0;
    draw_text("AIR TRAFFIC CONTROL", x, 80.0, 56.0, TITLE);
    draw_text(format!("30 minute shift   {}", staffing.label()), x, 112.0, 24.0, DIM);

    let mut y = 156.0;
    let mut pick = MenuPick {
        position: draw_positions(staffing, x, y),
        lesson: draw_lessons((screen_width() - 560.0).max(820.0), 70.0),
        ..Default::default()
    };
    if staffing.tower() {
        pick.vfr = draw_vfr_toggle(vfr, x, y + 68.0);
    }
    y += 100.0;
    let vfr = vfr && staffing.tower();
    for line in briefing(staffing, vfr).iter().chain(&controls(staffing)).chain(std::iter::once(&BRIEFING_VIEW)) {
        draw_text(line, x, y, 20.0, TEXT);
        y += 23.0;
    }
    y += 16.0;
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
    y += 16.0;
    pick.busyness = draw_busyness(busyness, airport.traffic.is_some(), x, y);
    draw_text("Press ENTER to start your shift", x, y + 120.0, 30.0, TITLE);
    pick
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
fn draw_pause(choice: usize, muted: bool, show_controls: bool, staffing: Staffing) -> (Option<usize>, bool) {
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
            controls(staffing).into_iter().chain(std::iter::once(BRIEFING_VIEW)).collect();
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
            "Score {}   Landed {}   Parked {}   Handed off {}   Go-arounds {}   Separation losses {}",
            score.points, score.landed, score.parked, score.exited, score.go_arounds, score.separation_losses
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
