//! The end-of-shift debrief, and the replay bar: a timeline of the shift with
//! its incidents marked, to click into the replay.

use super::{BACKGROUND, BAD, DIM, TEXT, TITLE};
use crate::sim::World;
use crate::sim::scoring::ShiftEnd;
use macroquad::prelude::*;
use super::gfx::{clear_background, draw_line, draw_rectangle, draw_rectangle_lines, draw_text, draw_triangle, measure_text, mouse_position, screen_height, screen_width};

const GOOD: Color = Color::new(0.35, 0.85, 0.45, 1.0);
const AMBER: Color = Color::new(1.00, 0.75, 0.25, 1.0);
const ROW: f32 = 22.0;
/// The replay starts this long before an incident clicked.
pub const LEAD_SECONDS: f32 = 45.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkKind {
    Penalty,
    Credit,
    Emergency,
}

/// Something to mark on the timeline.
#[derive(Debug, Clone)]
pub struct Mark {
    pub time: f32,
    pub kind: MarkKind,
    pub text: String,
}

impl Mark {
    fn color(&self) -> Color {
        match self.kind {
            MarkKind::Penalty => BAD,
            MarkKind::Credit => GOOD,
            MarkKind::Emergency => AMBER,
        }
    }
}

/// The shift's incidents and emergencies, in time order.
pub fn marks(world: &World) -> Vec<Mark> {
    let mut marks: Vec<Mark> = world
        .score
        .incidents
        .iter()
        .map(|i| Mark {
            time: i.time,
            kind: if i.points < 0 { MarkKind::Penalty } else { MarkKind::Credit },
            text: format!("{}  {:+}  {}", clock(i.time), i.points, i.text),
        })
        .chain(world.score.emergencies.iter().map(|e| Mark {
            time: e.declared_at,
            kind: MarkKind::Emergency,
            text: format!("{}  {}", clock(e.declared_at), e.summary()),
        }))
        .collect();
    marks.sort_by(|a, b| a.time.total_cmp(&b.time));
    marks
}

/// "12:34", minutes and seconds into the shift.
pub fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// What the mouse did on a timeline.
#[derive(Default)]
pub struct TimelineInput {
    /// The mark under the mouse.
    pub hovered: Option<usize>,
    /// The mark clicked.
    pub clicked: Option<usize>,
    /// The time under the mouse while the button is held on the bar.
    pub scrub: Option<f32>,
}

/// A bar from 0 to `length` seconds, with ticks at round intervals, the marks,
/// and a playhead. The played part runs to `reached`.
pub fn draw_timeline(marks: &[Mark], length: f32, reached: f32, rect: Rect, playhead: Option<f32>) -> TimelineInput {
    let length = length.max(1.0);
    let x_of = |t: f32| rect.x + rect.w * (t / length).clamp(0.0, 1.0);
    let mid = rect.y + rect.h / 2.0;
    draw_rectangle(rect.x, mid - 3.0, rect.w, 6.0, Color::new(0.12, 0.16, 0.16, 1.0));
    draw_rectangle(rect.x, mid - 3.0, x_of(reached) - rect.x, 6.0, Color::new(0.22, 0.32, 0.30, 1.0));
    // A dozen ticks or fewer, at round intervals.
    let step = [30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0]
        .into_iter()
        .find(|s| length / s <= 12.0)
        .unwrap_or(7200.0);
    let mut t = 0.0;
    while t <= length + 0.5 {
        let x = x_of(t);
        draw_line(x, mid + 5.0, x, mid + 10.0, 1.0, DIM);
        let label = clock(t);
        let w = measure_text(&label, None, 13, 1.0).width;
        draw_text(&label, (x - w / 2.0).clamp(rect.x - 4.0, rect.x + rect.w - w + 4.0), mid + 22.0, 13.0, DIM);
        t += step;
    }

    let mouse = Vec2::from(mouse_position());
    let mut input = TimelineInput::default();
    let mut best = f32::MAX;
    for (index, mark) in marks.iter().enumerate() {
        let x = x_of(mark.time);
        let color = mark.color();
        match mark.kind {
            MarkKind::Emergency => {
                let (top, bottom) = (mid - 15.0, mid - 3.0);
                let c = (top + bottom) / 2.0;
                draw_triangle(vec2(x, top), vec2(x - 5.0, c), vec2(x + 5.0, c), color);
                draw_triangle(vec2(x, bottom), vec2(x - 5.0, c), vec2(x + 5.0, c), color);
            }
            _ => draw_rectangle(x - 1.5, mid - 14.0, 3.0, 12.0, color),
        }
        let d = (mouse.x - x).abs();
        if d < 6.0 && (mouse.y - mid).abs() < 18.0 && d < best {
            best = d;
            input.hovered = Some(index);
        }
    }
    if let Some(t) = playhead {
        let x = x_of(t);
        draw_line(x, mid - 16.0, x, mid + 8.0, 2.0, TEXT);
        draw_triangle(vec2(x - 5.0, mid - 20.0), vec2(x + 5.0, mid - 20.0), vec2(x, mid - 14.0), TEXT);
    }
    if let Some(index) = input.hovered {
        let text = &marks[index].text;
        let w = measure_text(text, None, 16, 1.0).width;
        let tx = (x_of(marks[index].time) - w / 2.0).clamp(4.0, screen_width() - w - 4.0);
        draw_rectangle(tx - 6.0, mid - 42.0, w + 12.0, 22.0, BACKGROUND);
        draw_rectangle_lines(tx - 6.0, mid - 42.0, w + 12.0, 22.0, 1.0, marks[index].color());
        draw_text(text, tx, mid - 26.0, 16.0, TEXT);
    }
    let on_bar = Rect::new(rect.x - 6.0, rect.y, rect.w + 12.0, rect.h).contains(mouse);
    if is_mouse_button_pressed(MouseButton::Left) && input.hovered.is_some() {
        input.clicked = input.hovered;
    } else if on_bar && is_mouse_button_down(MouseButton::Left) {
        input.scrub = Some(((mouse.x - rect.x) / rect.w).clamp(0.0, 1.0) * length);
    }
    input
}

/// What the player chose on the debrief.
pub enum Action {
    Stay,
    /// Replay from this many seconds into the shift.
    Replay(f32),
    NewShift,
    Menu,
    CopyCode,
}

/// What the debrief shows besides the world.
pub struct Info<'a> {
    pub code: Option<&'a str>,
    pub saved: Option<&'a str>,
    /// A note under the code: "copied", or a version warning.
    pub note: Option<&'a str>,
    /// Whether there's a replay to open.
    pub can_replay: bool,
}

/// The debrief, drawn from the world at the end of the shift. `choice` is the
/// highlighted row of the incident list and `scroll` its first row shown.
pub fn frame(world: &World, end: ShiftEnd, choice: &mut Option<usize>, scroll: &mut usize, info: Info) -> Action {
    clear_background(BACKGROUND);
    let x = 60.0;
    let right_edge = screen_width() - 60.0;
    let score = &world.score;
    let headline = match end {
        ShiftEnd::Completed => "SHIFT COMPLETE",
        ShiftEnd::Decertified => "DECERTIFIED",
        ShiftEnd::NearCollision => "NEAR COLLISION",
        ShiftEnd::Left => "LEFT EARLY",
    };
    let color = if matches!(end, ShiftEnd::Completed | ShiftEnd::Left) { TITLE } else { BAD };
    draw_text(headline, x, 80.0, 56.0, color);
    draw_text(format!("Rating: {}", score.rating(end)), x, 118.0, 28.0, TEXT);
    draw_text(
        format!(
            "Score {}   Landed {}   Parked {}   Handed off {}   Go-arounds {}   Separation losses {}",
            score.points, score.landed, score.parked, score.exited, score.go_arounds, score.separation_losses
        ),
        x,
        148.0,
        22.0,
        DIM,
    );

    let marks = marks(world);
    let mut action = Action::Stay;
    let timeline = Rect::new(x, 196.0, right_edge - x, 40.0);
    if info.can_replay {
        draw_text("TIMELINE  click a mark to replay it", x, 186.0, 16.0, DIM);
    }
    let input = draw_timeline(&marks, world.shift_seconds(), world.time, timeline, None);
    if info.can_replay {
        if let Some(index) = input.clicked {
            action = Action::Replay(marks[index].time - LEAD_SECONDS);
        } else if let Some(t) = input.scrub.filter(|_| is_mouse_button_pressed(MouseButton::Left)) {
            action = Action::Replay(t);
        }
    }

    // The list: incidents and emergencies in time order.
    let top = 282.0;
    let footer = 96.0;
    draw_text("Incidents", x, top - 12.0, 24.0, TEXT);
    let rows = ((screen_height() - top - footer) / ROW).max(1.0) as usize;
    if marks.is_empty() {
        draw_text("None. A clean shift.", x, top + 14.0, 20.0, DIM);
    }
    if !marks.is_empty() {
        if is_key_pressed(KeyCode::Down) {
            *choice = Some(choice.map_or(0, |c| (c + 1).min(marks.len() - 1)));
        }
        if is_key_pressed(KeyCode::Up) {
            *choice = Some(choice.map_or(0, |c| c.saturating_sub(1)));
        }
    }
    let wheel = mouse_wheel().1;
    let max_scroll = marks.len().saturating_sub(rows);
    if wheel < 0.0 {
        *scroll = (*scroll + 3).min(max_scroll);
    } else if wheel > 0.0 {
        *scroll = scroll.saturating_sub(3);
    }
    if let Some(c) = *choice {
        if c < *scroll {
            *scroll = c;
        } else if c >= *scroll + rows {
            *scroll = c + 1 - rows;
        }
    }
    *scroll = (*scroll).min(max_scroll);
    let mouse = Vec2::from(mouse_position());
    let moved = mouse_delta_position() != Vec2::ZERO;
    for (row, index) in (*scroll..marks.len()).take(rows).enumerate() {
        let mark = &marks[index];
        let y = top + ROW * row as f32;
        let rect = Rect::new(x - 8.0, y, right_edge - x + 16.0, ROW);
        let hovered = info.can_replay && rect.contains(mouse);
        if hovered && moved {
            *choice = Some(index);
        }
        if Some(index) == *choice || input.hovered == Some(index) {
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, Color::new(0.10, 0.22, 0.12, 1.0));
        }
        let color = match mark.kind {
            MarkKind::Penalty => BAD,
            MarkKind::Credit => DIM,
            MarkKind::Emergency => AMBER,
        };
        draw_text(&mark.text, x, y + 16.0, 20.0, color);
        if hovered && is_mouse_button_pressed(MouseButton::Left) {
            action = Action::Replay(mark.time - LEAD_SECONDS);
        }
    }
    if marks.len() > rows {
        let shown = format!("{}-{} of {}  wheel to scroll", *scroll + 1, (*scroll + rows).min(marks.len()), marks.len());
        draw_text(&shown, x, top + ROW * rows as f32 + 14.0, 16.0, DIM);
    }
    // The footer: the code, where the replay went, and the keys.
    let mut fy = screen_height() - 66.0;
    if let Some(code) = info.code {
        let text = format!("Shift code  {code}   C copies it, to play this traffic again from the menu");
        draw_text(&text, x, fy, 20.0, TEXT);
        if let Some(note) = info.note {
            let w = measure_text(&text, None, 20, 1.0).width;
            draw_text(note, x + w + 16.0, fy, 18.0, AMBER);
        }
        fy += 20.0;
    }
    if let Some(saved) = info.saved {
        draw_text(format!("Replay saved: {saved}"), x, fy, 16.0, DIM);
    }
    let keys = if info.can_replay {
        "ENTER another shift   R replay from the start   Up/Down + SPACE replay an incident   ESC menu"
    } else {
        "ENTER again   ESC menu"
    };
    draw_text(keys, x, screen_height() - 22.0, 22.0, TITLE);

    if matches!(action, Action::Stay) {
        if super::start_pressed() {
            action = Action::NewShift;
        } else if is_key_pressed(KeyCode::Escape) {
            action = Action::Menu;
        } else if info.can_replay && is_key_pressed(KeyCode::R) {
            action = Action::Replay(0.0);
        } else if info.can_replay && is_key_pressed(KeyCode::Space) {
            if let Some(c) = choice.filter(|&c| c < marks.len()) {
                action = Action::Replay(marks[c].time - LEAD_SECONDS);
            }
        } else if info.code.is_some() && is_key_pressed(KeyCode::C) {
            action = Action::CopyCode;
        }
    }
    action
}

/// The callsign an incident names, of those in the world.
pub fn callsign_in(world: &World, text: &str) -> Option<u32> {
    world
        .aircraft
        .iter()
        .filter(|a| text.contains(a.callsign.as_str()))
        .max_by_key(|a| a.callsign.len())
        .map(|a| a.id)
}

/// The height of the replay bar along the bottom.
pub const BAR_HEIGHT: f32 = 66.0;

/// What the player did on the replay bar.
pub struct BarInput {
    /// Seek here, seconds.
    pub seek: Option<f32>,
}

/// The replay bar: where the replay is, the keys, and the timeline.
pub fn draw_bar(world: &World, final_world: &World, rate: f32, paused: bool, note: Option<&str>) -> BarInput {
    // Under the scope only, so the radio log beside it stays in view.
    let top = screen_height() - BAR_HEIGHT;
    let w = super::camera::scope_width();
    draw_rectangle(0.0, top, w, BAR_HEIGHT, Color::new(0.02, 0.05, 0.05, 0.94));
    draw_line(0.0, top, w, top, 1.0, DIM);
    let state = if world.ended.is_some() {
        "END".to_string()
    } else if paused {
        "PAUSED".to_string()
    } else {
        format!("x{rate:.0}")
    };
    let head = format!("REPLAY  {} / {}  {state}", clock(world.time), clock(final_world.time));
    draw_text(&head, 16.0, top + 22.0, 20.0, TITLE);
    let keys = "SPACE play | 1 2 4 8 speed | Left/Right seek | N/P incident | T take over | ESC back";
    let kw = measure_text(keys, None, 14, 1.0).width;
    draw_text(keys, (w - kw - 16.0).max(16.0 + measure_text(&head, None, 20, 1.0).width + 20.0), top + 21.0, 14.0, DIM);
    if let Some(note) = note {
        draw_text(note, 16.0, top - 8.0, 18.0, AMBER);
    }
    let marks = marks(final_world);
    let rect = Rect::new(16.0, top + 30.0, w - 32.0, 26.0);
    let input = draw_timeline(&marks, final_world.shift_seconds(), final_world.time, rect, Some(world.time));
    let seek = match (input.clicked, input.scrub) {
        (Some(index), _) => Some(marks[index].time - LEAD_SECONDS),
        (None, Some(t)) => Some(t.min(final_world.time)),
        _ => None,
    };
    BarInput { seek }
}

/// The next or previous mark from `now`, for N and P.
pub fn next_mark(final_world: &World, now: f32, forward: bool) -> Option<f32> {
    let times: Vec<f32> = marks(final_world).iter().map(|m| m.time - LEAD_SECONDS / 3.0).collect();
    if forward {
        times.into_iter().find(|&t| t > now + 1.0)
    } else {
        times.into_iter().rev().find(|&t| t < now - 1.0)
    }
}
