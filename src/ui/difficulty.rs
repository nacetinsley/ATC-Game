//! The Difficulty screen: how busy, how often things go wrong, and the weather.

use super::{BACKGROUND, DIM, TEXT, TITLE, menu_button};
use crate::sim::emergency::EmergencyRate;
use crate::sim::mistakes::{PilotMistakes, step};
use crate::sim::traffic::Busyness;
use crate::sim::weather::WeatherKind;
use macroquad::prelude::*;
use super::gfx::{clear_background, draw_rectangle_lines, draw_text, measure_text, mouse_position};

/// Everything on the Difficulty screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Difficulty {
    pub busyness: Busyness,
    pub emergencies: EmergencyRate,
    pub mistakes: PilotMistakes,
    pub weather: WeatherKind,
}

/// Named settings, one click away.
pub const PRESETS: [(&str, Difficulty); 4] = [
    (
        "Relaxed",
        Difficulty {
            busyness: Busyness::Quiet,
            emergencies: EmergencyRate::Off,
            mistakes: PilotMistakes::Off,
            weather: WeatherKind::Good,
        },
    ),
    // As the real job is: the airport's normal traffic, and emergencies and
    // pilot mistakes at real-world rates.
    (
        "Realistic",
        Difficulty {
            busyness: Busyness::RealWorld,
            emergencies: EmergencyRate::Realistic,
            mistakes: PilotMistakes::Realistic,
            weather: WeatherKind::Variable,
        },
    ),
    ("Standard", Difficulty::STANDARD),
    (
        "Demanding",
        Difficulty {
            busyness: Busyness::Busy,
            emergencies: EmergencyRate::Frequent,
            mistakes: PilotMistakes::Frequent,
            weather: WeatherKind::Storms,
        },
    ),
];

impl Difficulty {
    pub const STANDARD: Difficulty = Difficulty {
        busyness: Busyness::RealWorld,
        emergencies: EmergencyRate::Occasional,
        mistakes: PilotMistakes::Occasional,
        weather: WeatherKind::Variable,
    };

    /// The preset these settings match, or "Custom".
    pub fn preset_name(&self) -> &'static str {
        PRESETS.iter().find(|(_, d)| d == self).map_or("Custom", |(name, _)| name)
    }

    /// One line for the main menu.
    pub fn summary(&self, real_data: bool) -> String {
        format!(
            "{}: {} traffic, emergencies {}, pilot mistakes {}, {} weather",
            self.preset_name(),
            self.busyness.label(real_data),
            self.emergencies.label().to_lowercase(),
            self.mistakes.label().to_lowercase(),
            self.weather.label(),
        )
    }

    /// Change row `row` by one step left (`-1`) or right (`+1`).
    fn step_row(&mut self, row: usize, by: i32) {
        match row {
            0 => {
                let i = PRESETS.iter().position(|(_, d)| d == self);
                let next = match i {
                    Some(i) => (i as i32 + by).clamp(0, PRESETS.len() as i32 - 1) as usize,
                    None if by > 0 => 0,
                    None => PRESETS.len() - 1,
                };
                *self = PRESETS[next].1;
            }
            1 => self.busyness = self.busyness.step(by),
            2 => self.emergencies = step(&EmergencyRate::ALL, self.emergencies, by),
            3 => self.mistakes = self.mistakes.step(by),
            _ => self.weather = self.weather.step(by),
        }
    }
}

/// Rows on the screen: the presets, then the four settings.
pub const ROWS: usize = 5;

/// What the player did on the Difficulty screen this frame.
pub enum Action {
    Stay,
    Back,
    Start,
}

/// Keys and drawing for the Difficulty screen. `row` is the highlighted row.
pub fn frame(difficulty: &mut Difficulty, row: &mut usize, real_data: bool) -> Action {
    if is_key_pressed(KeyCode::Up) {
        *row = (*row + ROWS - 1) % ROWS;
    }
    if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::Tab) {
        *row = (*row + 1) % ROWS;
    }
    if is_key_pressed(KeyCode::Left) {
        difficulty.step_row(*row, -1);
    }
    if is_key_pressed(KeyCode::Right) {
        difficulty.step_row(*row, 1);
    }
    let action = if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::D) {
        Action::Back
    } else if super::start_pressed() {
        Action::Start
    } else {
        Action::Stay
    };
    if let Some((clicked_row, picked)) = draw(difficulty, *row, real_data) {
        *row = clicked_row;
        *difficulty = picked;
    }
    action
}

/// Draw the screen. Returns the row clicked and the settings it gives, if any.
fn draw(difficulty: &Difficulty, row: usize, real_data: bool) -> Option<(usize, Difficulty)> {
    clear_background(BACKGROUND);
    let x = 60.0;
    draw_text("DIFFICULTY", x, 80.0, 56.0, TITLE);
    draw_text(
        "Up/Down choose a row | Left/Right or click to change | ESC or D back | ENTER start",
        x,
        112.0,
        20.0,
        DIM,
    );
    let mut picked = None;
    let mut y = 170.0;
    let mut pick = |r: usize, d: Option<Difficulty>| {
        if let Some(d) = d {
            picked = Some((r, d));
        }
    };

    // Presets.
    let names: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).chain(std::iter::once("Custom")).collect();
    let current = PRESETS.iter().position(|(_, d)| d == difficulty).unwrap_or(PRESETS.len());
    let clicked = button_row("PRESET", &names, current, row == 0, x, y);
    pick(0, clicked.filter(|&i| i < PRESETS.len()).map(|i| PRESETS[i].1));
    draw_text("Or set each one yourself below.", x, y + 62.0, 18.0, DIM);
    y += 110.0;

    // Traffic: the five-notch slider.
    heading("TRAFFIC", row == 1, x, y);
    let level = super::draw_busyness(difficulty.busyness, real_data, x, y);
    pick(1, level.map(|busyness| Difficulty { busyness, ..*difficulty }));
    y += 124.0;

    let labels: Vec<&str> = EmergencyRate::ALL.iter().map(|r| r.label()).collect();
    let i = EmergencyRate::ALL.iter().position(|&r| r == difficulty.emergencies).unwrap_or(0);
    let clicked = button_row("EMERGENCIES", &labels, i, row == 2, x, y);
    draw_text(difficulty.emergencies.note(), x, y + 62.0, 18.0, DIM);
    pick(2, clicked.map(|i| Difficulty { emergencies: EmergencyRate::ALL[i], ..*difficulty }));
    y += 100.0;

    let labels: Vec<&str> = PilotMistakes::ALL.iter().map(|m| m.label()).collect();
    let i = PilotMistakes::ALL.iter().position(|&m| m == difficulty.mistakes).unwrap_or(0);
    let clicked = button_row("PILOT MISTAKES", &labels, i, row == 3, x, y);
    draw_text(difficulty.mistakes.note(), x, y + 62.0, 18.0, DIM);
    pick(3, clicked.map(|i| Difficulty { mistakes: PilotMistakes::ALL[i], ..*difficulty }));
    y += 100.0;

    let labels: Vec<&str> = WeatherKind::ALL.iter().map(|w| w.label()).collect();
    let i = WeatherKind::ALL.iter().position(|&w| w == difficulty.weather).unwrap_or(0);
    let clicked = button_row("WEATHER", &labels, i, row == 4, x, y);
    draw_text(difficulty.weather.note(), x, y + 62.0, 18.0, DIM);
    pick(4, clicked.map(|i| Difficulty { weather: WeatherKind::ALL[i], ..*difficulty }));
    picked
}

/// A row's title, with a marker when it has the keyboard.
fn heading(title: &str, focused: bool, x: f32, y: f32) {
    if focused {
        draw_text(">", x - 24.0, y, 22.0, TITLE);
    }
    draw_text(title, x, y, 20.0, if focused { TITLE } else { DIM });
}

/// A titled row of buttons, one lit. Returns the index clicked, if any.
fn button_row(title: &str, labels: &[&str], selected: usize, focused: bool, x: f32, y: f32) -> Option<usize> {
    const GAP: f32 = 8.0;
    heading(title, focused, x, y);
    let mut left = x;
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let (hit, width) = menu_button(label, left, y + 10.0, i == selected);
        if hit {
            clicked = Some(i);
        }
        left += width + GAP;
    }
    clicked
}

/// The main menu's box: the summary, and a way in. True if clicked.
pub fn draw_summary(difficulty: &Difficulty, real_data: bool, x: f32, y: f32) -> bool {
    draw_text("DIFFICULTY  D or click to change", x, y, 20.0, DIM);
    let text = difficulty.summary(real_data);
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().expect("one line");
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if measure_text(&candidate, None, 20, 1.0).width > 470.0 && !line.is_empty() {
            lines.push(word.to_string());
        } else {
            *line = candidate;
        }
    }
    let height = 12.0 + 24.0 * lines.len() as f32;
    let rect = Rect::new(x, y + 10.0, 500.0, height);
    let hover = rect.contains(Vec2::from(mouse_position()));
    draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, if hover { TITLE } else { DIM });
    for (i, line) in lines.iter().enumerate() {
        draw_text(line, rect.x + 12.0, rect.y + 24.0 + 24.0 * i as f32, 20.0, TEXT);
    }
    hover && is_mouse_button_pressed(MouseButton::Left)
}
