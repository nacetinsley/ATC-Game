//! Macroquad front end: screens, the fixed-timestep loop, and drawing.

pub mod audio;
pub mod camera;
pub mod coop;
pub mod debrief;
pub mod difficulty;
pub mod gfx;
pub mod input;
pub mod panels;
pub mod replays;
pub mod scope;
pub mod settings;
pub mod shift;
pub mod strips;
pub mod surface;
pub mod tutorial;
pub mod voice;

use crate::sim::airspace::{Airport, CATALOG};
use crate::sim::positions::{MAX_SEATS, Position, SOLO, Seat, Staffing};
use crate::net::session::{HOST, Notice, ShiftSetup, staffing_of};
use crate::sim::replay::{Action, Recording, ShiftCode};
use crate::sim::scoring::ShiftEnd;
use crate::sim::traffic::Busyness;
use crate::sim::tutorial::{LessonKind, Tutorial};
use crate::sim::{TICK, World, WorldConfig};
use audio::Sounds;
use difficulty::Difficulty;
use gfx::{Palette, SCALES};
use settings::Settings;
use std::path::PathBuf;
use input::{UiState, View};
use macroquad::prelude::*;
use gfx::{clear_background, draw_circle, draw_line, draw_rectangle, draw_rectangle_lines, draw_text, measure_text, mouse_position, screen_height, screen_width};

/// Longest frame time to catch up on, so a stall doesn't fast-forward the sim.
const MAX_FRAME_SECONDS: f32 = 0.25;
/// A shift quit before this many ticks (a minute) isn't worth a replay.
const LEAVE_KEEP_TICKS: u32 = crate::sim::replay::SNAPSHOT_TICKS;

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
    "INCURSION  Nobody onto a runway with traffic inside 2 NM final or rolling toward them.",
];
const GROUND_BRIEFING: [&str; 3] = [
    "TAXI     Click an aircraft, then a hold line, gate or intersection. Ctrl-click taxiways",
    "         first to route via them. Ground never crosses a runway: Tower does.",
    "SPACING  Keep taxiing traffic apart: two aircraft nose to nose can't pass.",
];
const DELIVERY_BRIEFING: [&str; 4] = [
    "CLEARANCE  Departures call from the gate. Check the flight plan, then clear it (I).",
    "          Cruising altitude by direction (magnetic course): 0-179 odd, 180-359 even;",
    "          above FL410, FL410/450/490 east, 430/470/510 west. SIDs must be in use.",
    "READBACK  Listen to it. If the code or altitude comes back wrong, press I again.",
];
const DELIVERY_CONTROLS: &str =
    "DELIVERY  I clearance (again: repeat code and altitude) | Up/Down amend altitude | S next SID | F4 course hint";
const RADAR_CONTROLS: &str =
    "CONTROLS  Click/Tab select | Drag: heading, or onto a fix: direct (Ctrl: hold) | Arrows hdg, alt | +/- speed | C ILS | R route | H hold";
const TOWER_CONTROLS: &str =
    "CONTROLS  H hold/continue | X cross | W line up | K takeoff | L land | G go-around | T contact next";
const COMMON_CONTROLS: &str = "          T handoff | ENTER send | BKSP cancel";
const VFR_CONTROLS: &str =
    "VFR       E enter/extend downwind | B base | O option | U touch-and-go | L full stop | Shift-click follow";
const VFR_BRIEFING: &str =
    "VFR      Light aircraft fly the pattern on their own. Sequence them, and clear every landing.";

/// What the player is told before a shift, for the positions they work.
fn briefing(staffing: Staffing, vfr: bool) -> Vec<&'static str> {
    let mut lines = vec![match (staffing.approach, staffing.local, staffing.ground, staffing.delivery) {
        (true, false, false, false) => "You work Approach radar. Tower, Ground and Center are automated.",
        (false, true, false, false) => "You work Local Control (Tower): the runways. Ground and Approach are automated.",
        (false, false, true, false) => "You work Ground Control: the taxiways. Tower and Approach are automated.",
        (false, false, false, true) => "You work Clearance Delivery: IFR clearances at the gate. Everything else is automated.",
        (false, false, true, true) => "You work Ground and Clearance Delivery combined. Tower and Approach are automated.",
        (false, true, true, false) => "You work Tower and Ground combined. Approach is automated.",
        _ => "You work every position: Approach, Tower, Ground and Delivery. F1 radar, F2 surface.",
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
    if staffing.delivery {
        lines.extend(DELIVERY_BRIEFING);
        if !staffing.ground {
            lines.push("HANDOFF  Once it has read back its clearance, send it to Ground (T).");
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
    if staffing.delivery {
        lines.push(DELIVERY_CONTROLS);
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
    /// Sounds no real facility makes: the handoff ding and the like.
    GameCues,
    /// Pilots and controllers speaking their transmissions.
    Voices,
    /// The player's own transmissions spoken too.
    OwnVoice,
    UiScale,
    /// The colour-blind palettes.
    Colours,
    Controls,
    MainMenu,
    Quit,
    /// Back to the start menu, from the settings screen.
    Back,
}

const PAUSE_ITEMS: [PauseItem; 11] = [
    PauseItem::Resume,
    PauseItem::Restart,
    PauseItem::Sound,
    PauseItem::Voices,
    PauseItem::OwnVoice,
    PauseItem::GameCues,
    PauseItem::UiScale,
    PauseItem::Colours,
    PauseItem::Controls,
    PauseItem::MainMenu,
    PauseItem::Quit,
];
/// The settings screen, from the start menu.
const SETTINGS_ITEMS: [PauseItem; 7] = [
    PauseItem::Sound,
    PauseItem::Voices,
    PauseItem::OwnVoice,
    PauseItem::GameCues,
    PauseItem::UiScale,
    PauseItem::Colours,
    PauseItem::Back,
];
const PAUSE_ROW: f32 = 40.0;
const PAUSE_WIDTH: f32 = 340.0;
const BRIEFING_VIEW: &str =
    "          SPACE pause | ESC menu | 1 2 4 rate | M terrain | Wheel zoom | Right-drag pan | F1/F2 views";

enum Screen {
    Menu,
    /// The Difficulty screen, with the row that has the keyboard.
    Difficulty { row: usize },
    /// Sound, UI scale and colours, from the start menu.
    Settings { choice: usize },
    Playing,
    /// The sim is frozen behind the pause menu.
    Paused { choice: usize, show_controls: bool },
    /// The end of a shift, with the incident highlighted in the list and the
    /// list's first row shown.
    Debrief { end: ShiftEnd, choice: Option<usize>, scroll: usize },
    /// Watching the shift back, from the debrief.
    Replay { end: ShiftEnd },
    /// Running a saved replay's shift again, to watch it.
    Rebuilding,
    /// Saved replays, from the start menu.
    Replays { choice: usize },
    /// Typing a shift code, from the start menu.
    Code,
    /// Co-op before the shift: its menu, joining, and the lobby.
    Coop,
}

pub struct App {
    screen: Screen,
    world: World,
    ui: UiState,
    sounds: Sounds,
    accumulator: f32,
    airports: Vec<Airport>,
    choice: usize,
    difficulty: Difficulty,
    /// Index into `Staffing::CHOICES`.
    position: usize,
    /// VFR traffic, when a tower position is chosen.
    vfr: bool,
    /// The lesson being played, if this is a tutorial rather than a shift.
    tutorial: Option<Tutorial>,
    /// Shift length chosen on the menu, minutes.
    shift_minutes: u32,
    /// Minutes being typed on the menu's number keys.
    typed: Option<shift::Typed>,
    /// UI scale chosen, one of `gfx::SCALES`.
    ui_scale: f32,
    palette: Palette,
    /// As last saved to the settings file.
    saved: Settings,
    /// The shift as played, to replay. None for lessons.
    recording: Option<Recording>,
    /// The shift's code.
    code: Option<ShiftCode>,
    /// Where its replay was saved.
    replay_path: Option<String>,
    /// A note for the debrief and the replay: "Copied", or a version warning.
    note: Option<&'static str>,
    /// The code being typed on the code screen, and why it was refused.
    code_text: String,
    code_error: Option<String>,
    /// Saved replays, read when the list opens.
    replay_list: Vec<(PathBuf, replays::ReplayFile)>,
    /// The saved replay being rebuilt, to check against.
    rebuilding: Option<replays::ReplayFile>,
    /// Started with `--hot-seat`: a test of co-op on one machine. Shifts
    /// split the positions between two seats, and F9 swaps between them.
    hot_seat: bool,
    /// In hot-seat play, the other seats' screens, waiting their turn.
    other_seats: Vec<UiState>,
    /// Co-op before the shift: where the player is in it.
    coop_flow: Option<coop::Flow>,
    /// The co-op session behind the shift being played, if it's one.
    session: Option<coop::Session>,
    /// The name other players see, and the relay's address.
    coop_name: String,
    relay: String,
    /// Co-op notices on screen.
    banner: coop::Banner,
    /// Co-op: each seat's player's name, as last heard, for the debrief and
    /// the replay. Someone who dropped keeps theirs.
    names: [Option<String>; MAX_SEATS],
}

/// "Tower + Ground": the positions in a staffing, by name.
pub fn positions_label(staffing: Staffing) -> String {
    let names: Vec<&str> = crate::sim::scoring::TEAM.iter().filter(|&&p| staffing.works(p)).map(|p| p.name()).collect();
    if names.is_empty() { "nothing".to_string() } else { names.join(" + ") }
}

/// Hot-seat play's split: Approach in seat 0, the tower cab in seat 1.
fn hot_seat_staffing() -> Staffing {
    Staffing::NONE
        .with_seat(Position::Approach, 0)
        .with_seat(Position::Local, 1)
        .with_seat(Position::Ground, 1)
        .with_seat(Position::Delivery, 1)
}

/// What the menu chose for a new shift.
struct ShiftChoice {
    difficulty: Difficulty,
    staffing: Staffing,
    vfr: bool,
    minutes: u32,
}

/// A new shift's settings, with a fresh seed: 32 bits, so a shift code holds it.
fn shift_config(choice: ShiftChoice) -> WorldConfig {
    let seed = fastrand::u32(..) as u64;
    let ShiftChoice { difficulty, staffing, vfr, minutes } = choice;
    let Difficulty { busyness, emergencies, mistakes, weather } = difficulty;
    let shift_seconds = minutes as f32 * 60.0;
    WorldConfig { seed, busyness, staffing, vfr, emergencies, mistakes, weather, shift_seconds, ..Default::default() }
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
const FONT_SIZES: [u16; 14] = [12, 13, 14, 15, 16, 17, 18, 20, 22, 24, 28, 30, 36, 56];

/// Cache every printable ASCII glyph at every size, at the UI scale, before
/// anything is drawn: at startup, and again when the scale changes.
/// macroquad grows its glyph atlas on demand, and a resize partway through
/// drawing can leave every glyph sampling the wrong texture, so text draws as
/// solid blocks. Growing it once up front avoids that.
fn warm_font_cache() {
    let font = get_default_font();
    let chars: Vec<char> = (' '..='~').collect();
    let dpi = miniquad::window::dpi_scale();
    for size in FONT_SIZES {
        let raster = gfx::raster_size(size as f32);
        font.populate_font_cache(&chars, (raster as f32 * dpi).ceil() as u16);
    }
}

impl App {
    pub async fn new() -> App {
        warm_font_cache();
        let airports: Vec<Airport> = CATALOG
            .iter()
            .map(|&(text, ground)| Airport::load(text, ground).expect("built-in airport data is valid"))
            .collect();
        let mut runways: Vec<String> =
            airports.iter().flat_map(|a| a.runways.iter().map(|r| r.id.clone())).collect();
        runways.sort();
        runways.dedup();
        let settings = Settings::load();
        gfx::set_palette(settings.palette);
        let mut sounds = Sounds::load(&runways).await;
        sounds.volume = settings.volume;
        sounds.game_cues = settings.game_cues;
        sounds.voices.enabled = settings.voices;
        sounds.voices.own = settings.own_voice;
        let choice = airports.iter().position(|a| a.code == settings.airport).unwrap_or(0);
        let position = Staffing::CHOICES.iter().position(|&s| s == settings.staffing).unwrap_or(0);
        let args = coop::CoopArgs::from_args();
        let coop_name = args.name.clone().unwrap_or_else(|| settings.coop_name.clone());
        let relay = args.relay.clone().unwrap_or_else(|| settings.relay.clone());
        // The command line's shortcuts into co-op.
        let coop_flow = if let Some(what) = &args.join {
            Some(coop::Flow::join(what, &relay))
        } else if args.host_relay {
            Some(coop::Flow::host_relay(&relay))
        } else if args.host_lan {
            Some(coop::Flow::host_lan())
        } else {
            None
        };
        App {
            screen: if coop_flow.is_some() { Screen::Coop } else { Screen::Menu },
            world: World::new(
                airports[0].clone(),
                shift_config(ShiftChoice {
                    difficulty: Difficulty::STANDARD,
                    staffing: Staffing::default(),
                    vfr: false,
                    minutes: shift::DEFAULT_MINUTES,
                }),
            ),
            ui: UiState::new(&airports[0], Staffing::default()),
            hot_seat: std::env::args().any(|a| a == "--hot-seat"),
            other_seats: Vec::new(),
            coop_flow,
            session: None,
            coop_name,
            relay,
            banner: coop::Banner::default(),
            names: Default::default(),
            sounds,
            accumulator: 0.0,
            airports,
            choice,
            difficulty: settings.difficulty,
            position,
            vfr: settings.vfr,
            tutorial: None,
            shift_minutes: settings.shift_minutes,
            typed: None,
            ui_scale: settings.ui_scale,
            palette: settings.palette,
            saved: settings,
            recording: None,
            code: None,
            replay_path: None,
            note: None,
            code_text: String::new(),
            code_error: None,
            replay_list: Vec::new(),
            rebuilding: None,
        }
    }

    /// The settings as they stand now, saved if they've changed.
    fn save_settings(&mut self) {
        let now = Settings {
            volume: self.sounds.volume,
            game_cues: self.sounds.game_cues,
            voices: self.sounds.voices.enabled,
            own_voice: self.sounds.voices.own,
            ui_scale: self.ui_scale,
            palette: self.palette,
            airport: self.airports[self.choice].code.clone(),
            staffing: self.staffing(),
            vfr: self.vfr,
            shift_minutes: self.shift_minutes,
            difficulty: self.difficulty,
            coop_name: self.coop_name.clone(),
            relay: self.relay.clone(),
        };
        if now != self.saved {
            now.save();
            self.saved = now;
        }
    }

    /// Change a setting from the pause menu or the settings screen: `step`
    /// is +1 or -1 for Right or Left, and Enter steps forward.
    fn adjust(&mut self, item: PauseItem, step: i32) {
        match item {
            PauseItem::Sound => {
                self.sounds.volume = if step < 0 { self.sounds.volume.previous() } else { self.sounds.volume.next() };
            }
            PauseItem::GameCues => self.sounds.game_cues = !self.sounds.game_cues,
            PauseItem::OwnVoice => self.sounds.voices.own = !self.sounds.voices.own,
            PauseItem::Voices => {
                self.sounds.voices.enabled = !self.sounds.voices.enabled;
                if !self.sounds.voices.enabled {
                    self.sounds.voices.silence();
                }
            }
            PauseItem::UiScale => {
                let i = SCALES.iter().position(|&s| s == self.ui_scale).unwrap_or(2) as i32;
                // Enter goes round; the arrows stop at the ends.
                let next = match step {
                    0 => (i + 1) % SCALES.len() as i32,
                    _ => (i + step).clamp(0, SCALES.len() as i32 - 1),
                };
                self.ui_scale = SCALES[next as usize];
            }
            PauseItem::Colours => {
                self.palette = if step < 0 { self.palette.previous() } else { self.palette.next() };
                gfx::set_palette(self.palette);
            }
            _ => return,
        }
        self.save_settings();
    }

    fn staffing(&self) -> Staffing {
        Staffing::CHOICES[self.position]
    }

    fn start_shift(&mut self) {
        self.save_settings();
        let choice = ShiftChoice {
            difficulty: self.difficulty,
            staffing: self.staffing(),
            vfr: self.vfr,
            minutes: self.shift_minutes,
        };
        self.start_world(self.choice, shift_config(choice));
    }

    /// Start a shift at the airport at `index`, recording it.
    fn start_world(&mut self, index: usize, mut config: WorldConfig) {
        self.end_session("the host started another shift");
        let airport = &self.airports[index];
        if self.hot_seat {
            config.staffing = hot_seat_staffing();
        }
        self.world = World::new(airport.clone(), config);
        let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
        self.ui = gfx::laid_out_at(scale, || UiState::for_seat(airport, config.staffing, 0));
        self.other_seats = if self.hot_seat {
            config
                .staffing
                .players()
                .iter()
                .skip(1)
                .map(|seat| gfx::laid_out_at(scale, || UiState::for_seat(airport, config.staffing, seat)))
                .collect()
        } else {
            Vec::new()
        };
        for ui in &mut self.other_seats {
            ui.clock_start = self.ui.clock_start;
        }
        // The sim's clock, for EFC times, reads the same as the strips'.
        self.world.set_clock_start(self.ui.clock_start);
        self.banner.clear();
        self.names = Default::default();
        self.code = Some(ShiftCode::of(&config, &airport.code));
        self.recording = Some(Recording::new(&self.world));
        self.replay_path = None;
        self.note = None;
        self.sounds.reset(&self.world);
        self.tutorial = None;
        self.accumulator = 0.0;
        self.screen = Screen::Playing;
    }

    /// Leave the co-op session, if there is one.
    fn end_session(&mut self, why: &str) {
        if let Some(mut session) = self.session.take() {
            session.leave(why);
        }
    }

    /// Run the co-op session: send what this player said, and run the
    /// ticks that have come. If the session is lost, the shift ends here.
    fn run_session(&mut self) {
        let Some(session) = &mut self.session else { return };
        for (id, action) in self.ui.take_outbox() {
            session.command(id, action);
        }
        let mut lost = false;
        for notice in session.update(get_frame_time(), &mut self.world, &self.airports) {
            self.banner.push(coop::notice_text(&notice, &self.world));
            match notice {
                // The player carries on: the host sends the shift again.
                Notice::Desync { tick } => {
                    if let Some(path) = coop::save_desync_report(&self.world, session.setup(), tick) {
                        self.banner.push(format!("Saved {} for a bug report.", path.display()));
                    }
                }
                // The world is the host's again. Snapshots taken since the
                // drop or the desync may be wrong: the replay keeps only the
                // start, and takes them again from here.
                Notice::CaughtUp => {
                    if let Some(recording) = &self.recording {
                        self.recording = Some(Recording::from_start(recording.first().clone(), Vec::new()));
                    }
                    self.sounds.reset(&self.world);
                }
                Notice::HostGone { .. } => lost = true,
                _ => {}
            }
        }
        if lost {
            self.session = None;
            self.world.leave_shift();
        }
    }

    /// What a co-op shift will be, from the start menu's choices.
    fn coop_about(&self) -> String {
        let airport = &self.airports[self.choice];
        let real_data = airport.traffic.is_some();
        format!(
            "{} {}, {}, {}",
            airport.code,
            airport.name,
            shift::label(self.shift_minutes),
            self.difficulty.summary(real_data)
        )
    }

    /// Co-op before the shift: the menu, connecting, and the lobby.
    fn coop_frame(&mut self) {
        self.sounds.silence();
        let about = self.coop_about();
        let Some(flow) = &mut self.coop_flow else {
            self.screen = Screen::Menu;
            return;
        };
        let ctx = coop::Ctx { name: &mut self.coop_name, relay: &mut self.relay, airports: &self.airports, about };
        match coop::frame(flow, ctx) {
            coop::Outcome::Stay => {}
            coop::Outcome::Menu => {
                self.coop_flow = None;
                self.screen = Screen::Menu;
            }
            coop::Outcome::Save => self.save_settings(),
            coop::Outcome::Start(host, label, reopen) => self.start_hosted_shift(*host, label, reopen),
            coop::Outcome::Joined(client, world, seat, dial) => self.begin_joined_shift(*client, *world, seat, dial),
        }
    }

    /// The host starts the lobby's shift: the start menu's choices, with
    /// the positions the lobby gave out.
    fn start_hosted_shift(&mut self, mut host: coop::AnyHost, label: String, reopen: Option<coop::Reopen>) {
        self.save_settings();
        self.end_session("a new shift");
        let choice = ShiftChoice {
            difficulty: self.difficulty,
            staffing: staffing_of(&host.positions()),
            vfr: self.vfr,
            minutes: self.shift_minutes,
        };
        let config = shift_config(choice);
        let airport = self.airports[self.choice].clone();
        // Built from what the players are sent, so every world is the same.
        let setup = ShiftSetup::new(&config, &airport.code, config.staffing, miniquad::date::now());
        let config = setup.config().expect("a code just made reads back");
        self.world = World::new(airport.clone(), config);
        let code = ShiftCode::parse(&setup.code).ok();
        host.start(setup, &self.world);
        self.begin_coop_shift(&airport, config.staffing, config.clock_start, HOST);
        self.code = code;
        self.recording = Some(Recording::new(&self.world));
        self.session = Some(coop::Session::Host(coop::Hosting::new(host, label, reopen)));
    }

    /// Joined: play the host's shift, from where it has got to.
    fn begin_joined_shift(
        &mut self,
        client: coop::AnyClient,
        world: World,
        seat: crate::sim::positions::Seat,
        dial: coop::Dial,
    ) {
        self.end_session("a new shift");
        let setup = client.setup().expect("a joined client has the shift").clone();
        let airport = (*world.airport).clone();
        self.world = world;
        self.begin_coop_shift(&airport, setup.staffing, setup.clock_start, seat);
        // The replay starts where the shift did, however late this player came.
        self.code = ShiftCode::parse(&setup.code).ok();
        self.recording = setup.world(&self.airports).ok().map(|start| Recording::from_start(start, Vec::new()));
        self.session = Some(coop::Session::Client(coop::Player::new(client, dial)));
    }

    /// The screen for a co-op shift in `seat`.
    fn begin_coop_shift(&mut self, airport: &Airport, staffing: Staffing, clock_start: f64, seat: crate::sim::positions::Seat) {
        let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
        self.ui = gfx::laid_out_at(scale, || UiState::for_seat(airport, staffing, seat));
        self.ui.clock_start = clock_start;
        self.ui.outbox = Some(Default::default());
        self.other_seats.clear();
        self.code = None;
        self.recording = None;
        self.replay_path = None;
        self.note = None;
        self.sounds.reset(&self.world);
        self.tutorial = None;
        self.accumulator = 0.0;
        self.banner.clear();
        self.names = Default::default();
        self.coop_flow = None;
        self.screen = Screen::Playing;
    }

    /// The shift is over: keep its replay and show the debrief.
    fn finish_shift(&mut self, end: ShiftEnd) {
        self.end_session("the shift is over");
        self.save_replay();
        self.screen = Screen::Debrief { end, choice: None, scroll: 0 };
    }

    /// Quitting a shift to the main menu or the desktop: keep its replay, unless it was a
    /// tutorial or barely begun.
    fn save_left_shift(&mut self) {
        if self.tutorial.is_some() || self.world.ticks() < LEAVE_KEEP_TICKS {
            return;
        }
        self.world.leave_shift();
        self.save_replay();
    }

    /// Save the shift as played, if it's being recorded.
    fn save_replay(&mut self) {
        if let (Some(recording), Some(code)) = (&mut self.recording, &self.code) {
            recording.finish(&self.world);
            let file = replays::ReplayFile::new(code, recording.first(), &self.world, &self.names, self.ui.seat);
            self.replay_path = replays::save(&file).map(|p| p.display().to_string());
        }
    }

    /// The world at the end of the shift, for the debrief.
    fn final_world(&self) -> &World {
        self.recording.as_ref().map_or(&self.world, |r| r.last())
    }

    fn debrief_frame(&mut self, end: ShiftEnd, mut choice: Option<usize>, mut scroll: usize) {
        self.sounds.silence();
        let code = self.code.as_ref().map(|c| c.text());
        let info = debrief::Info {
            code: code.as_deref(),
            saved: self.replay_path.as_deref(),
            note: self.note,
            can_replay: self.recording.is_some(),
            names: &self.names,
        };
        let action = debrief::frame(self.final_world(), end, &mut choice, &mut scroll, info);
        self.screen = Screen::Debrief { end, choice, scroll };
        match action {
            debrief::Action::Stay => {}
            debrief::Action::Replay(t) => self.open_replay(end, t),
            debrief::Action::NewShift => self.restart(),
            debrief::Action::Menu => {
                self.tutorial = None;
                self.screen = Screen::Menu;
            }
            debrief::Action::CopyCode => {
                if let Some(code) = code {
                    macroquad::miniquad::window::clipboard_set(&code);
                    self.note = Some("Copied");
                }
            }
        }
    }

    /// Watch the shift from `t` seconds in, paused, with the aircraft an
    /// incident there names selected.
    fn open_replay(&mut self, end: ShiftEnd, t: f32) {
        let Some(recording) = &self.recording else { return };
        let incident = debrief::marks(recording.last())
            .into_iter()
            .find(|m| (m.time - debrief::LEAD_SECONDS - t).abs() < 0.5)
            .map(|m| m.text);
        self.seek(t);
        self.ui.replay = true;
        self.ui.paused = true;
        self.ui.drag = input::Drag::None;
        let named = incident.and_then(|text| debrief::callsign_in(&self.world, &text));
        self.ui.select(named);
        self.accumulator = 0.0;
        self.screen = Screen::Replay { end };
    }

    /// Put the replay at `t` seconds into the shift.
    fn seek(&mut self, t: f32) {
        let Some(recording) = &self.recording else { return };
        let tick = (t.clamp(0.0, recording.last().time) / TICK).round() as u32;
        self.world = recording.seek(tick);
        self.world.drain_events();
        self.sounds.reset(&self.world);
    }

    fn replay_frame(&mut self, end: ShiftEnd) {
        if is_key_pressed(KeyCode::Escape) {
            self.ui.replay = false;
            self.ui.paused = false;
            self.screen = Screen::Debrief { end, choice: None, scroll: 0 };
            return;
        }
        if is_key_pressed(KeyCode::T) && self.world.ended.is_none() {
            self.take_over();
            return;
        }
        if is_key_pressed(KeyCode::F9) {
            self.watch_next_seat();
        }
        let Some(recording) = &self.recording else { return };
        let now = self.world.time;
        let step = if is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift) { 60.0 } else { 10.0 };
        let mut seek = None;
        if is_key_pressed(KeyCode::Left) {
            seek = Some(now - step);
        }
        if is_key_pressed(KeyCode::Right) {
            seek = Some(now + step);
        }
        if is_key_pressed(KeyCode::N) {
            seek = debrief::next_mark(recording.last(), now, true).or(seek);
        }
        if is_key_pressed(KeyCode::P) {
            seek = debrief::next_mark(recording.last(), now, false).or(seek);
        }

        self.ui.layout();
        self.ui.update(&mut self.world);
        if !self.ui.paused && self.world.ended.is_none() {
            self.accumulator += get_frame_time().min(MAX_FRAME_SECONDS) * self.ui.rate;
            while self.accumulator >= TICK && self.world.ended.is_none() {
                self.accumulator -= TICK;
                self.world.replay_tick(&recording.commands);
            }
        }
        let events = self.world.drain_events();
        self.sounds.update(&self.world, &self.ui, &events);
        draw_views(&self.world, &mut self.ui);
        panels::draw(&mut self.world, &mut self.ui);
        let bar = debrief::draw_bar(&self.world, recording.last(), self.ui.rate, self.ui.paused, self.note);
        if self.world.seated().is_coop() {
            let who = self.names[self.ui.seat as usize].clone().unwrap_or_else(|| format!("seat {}", self.ui.seat + 1));
            let text = format!("WATCHING {who}: {}. F9 for the next player", positions_label(self.world.seated().of_seat(self.ui.seat)));
            draw_text(&text, 16.0, screen_height() - debrief::BAR_HEIGHT - 30.0, 18.0, YELLOW);
        }
        // Dragging along the bar seeks only when it moves the replay.
        if let Some(t) = seek.or(bar.seek.filter(|t| (t - now).abs() >= TICK)) {
            self.seek(t);
        }
    }

    /// In a co-op replay, watch from the next player's seat.
    fn watch_next_seat(&mut self) {
        let seated = self.world.seated();
        let seats: Vec<Seat> = seated.players().iter().collect();
        if seats.len() < 2 {
            return;
        }
        let next = seats.iter().copied().find(|&s| s > self.ui.seat).unwrap_or(seats[0]);
        let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
        let mut ui = gfx::laid_out_at(scale, || UiState::for_seat(&self.world.airport, seated, next));
        ui.replay = true;
        ui.paused = self.ui.paused;
        ui.rate = self.ui.rate;
        ui.clock_start = self.ui.clock_start;
        ui.selected = self.ui.selected;
        ui.names = self.names.clone();
        self.ui = ui;
    }

    /// Play on from here, live: the shift from now on is whatever happens next.
    /// A co-op shift goes on as single player, with every position the
    /// players had.
    fn take_over(&mut self) {
        if let Some(recording) = &mut self.recording {
            recording.take_over(&self.world);
        }
        if self.world.seated().is_coop() {
            self.world.act(SOLO, 0, Action::Solo);
            let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
            let mut ui = gfx::laid_out_at(scale, || UiState::for_seat(&self.world.airport, self.world.staffing(), SOLO));
            ui.clock_start = self.ui.clock_start;
            ui.selected = self.ui.selected;
            self.ui = ui;
            // Hot seat's other screens too: it's one player now.
            self.other_seats.clear();
        }
        self.world.note_take_over();
        self.ui.replay = false;
        self.ui.paused = false;
        self.ui.rate = 1.0;
        self.replay_path = None;
        self.note = None;
        self.accumulator = 0.0;
        self.sounds.reset(&self.world);
        self.screen = Screen::Playing;
    }

    /// Open a saved replay: rebuild its shift, then show its debrief.
    fn open_replay_file(&mut self, file: replays::ReplayFile) -> Result<(), String> {
        let code = ShiftCode::parse(&file.code).map_err(str::to_string)?;
        let index = self
            .airports
            .iter()
            .position(|a| a.code == code.airport)
            .ok_or_else(|| format!("This version has no airport {}", code.airport))?;
        let staffing = file.seats.unwrap_or(code.staffing);
        let mut world = World::new(self.airports[index].clone(), WorldConfig { staffing, ..code.config() });
        world.set_clock_start(file.clock_start);
        let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
        let seat = if staffing.players().has(file.seat) { file.seat } else { SOLO };
        self.ui = gfx::laid_out_at(scale, || UiState::for_seat(&self.airports[index], staffing, seat));
        self.other_seats.clear();
        self.names = file.names();
        self.ui.names = self.names.clone();
        self.ui.clock_start = file.clock_start;
        self.recording = Some(Recording::from_start(world, file.command.clone()));
        self.code = Some(code);
        self.replay_path = None;
        self.note = None;
        self.tutorial = None;
        self.rebuilding = Some(file);
        self.screen = Screen::Rebuilding;
        Ok(())
    }

    fn rebuild_frame(&mut self) {
        self.sounds.silence();
        clear_background(BACKGROUND);
        let (Some(recording), Some(code)) = (&mut self.recording, &self.code) else {
            self.screen = Screen::Menu;
            return;
        };
        let done = recording.rebuild_step(4000);
        let total = (code.minutes * 240).max(1) as f32;
        let fraction = (recording.end_tick() as f32 / total).min(1.0);
        let (w, h) = (screen_width(), screen_height());
        let bar = Rect::new(w / 2.0 - 200.0, h / 2.0, 400.0, 16.0);
        draw_text("Rebuilding replay...", bar.x, bar.y - 16.0, 28.0, TITLE);
        draw_rectangle_lines(bar.x, bar.y, bar.w, bar.h, 1.0, DIM);
        draw_rectangle(bar.x + 2.0, bar.y + 2.0, (bar.w - 4.0) * fraction, bar.h - 4.0, TITLE);
        if done {
            let world = recording.last().clone();
            if self.rebuilding.take().is_some_and(|file| file.summary != world.summary()) {
                self.note = Some("Saved by another version of the game: the replay may not match what happened");
            }
            let end = world.ended.unwrap_or(ShiftEnd::Completed);
            self.world = world;
            self.screen = Screen::Debrief { end, choice: None, scroll: 0 };
        }
    }

    fn replays_frame(&mut self, mut choice: usize) {
        self.sounds.silence();
        clear_background(BACKGROUND);
        let count = self.replay_list.len();
        if count > 0 {
            if is_key_pressed(KeyCode::Up) {
                choice = (choice + count - 1) % count;
            }
            if is_key_pressed(KeyCode::Down) {
                choice = (choice + 1) % count;
            }
            choice = choice.min(count - 1);
        }
        let x = 60.0;
        draw_text("REPLAYS", x, 80.0, 56.0, TITLE);
        draw_text("Up/Down choose | ENTER or click watch | DEL delete | ESC back", x, 112.0, 20.0, DIM);
        let top = 150.0;
        let row = 30.0;
        let rows = ((screen_height() - top - 70.0) / row).max(1.0) as usize;
        let first = choice.saturating_sub(rows.saturating_sub(1));
        let mouse = Vec2::from(mouse_position());
        let mut open = start_pressed() && count > 0;
        if count == 0 {
            draw_text("No replays yet. Every shift you finish or leave is saved here.", x, top + 20.0, 22.0, TEXT);
        }
        for (n, (_, file)) in self.replay_list.iter().enumerate().skip(first).take(rows) {
            let y = top + row * (n - first) as f32;
            let rect = Rect::new(x - 8.0, y, screen_width() - 2.0 * x + 16.0, row - 4.0);
            if rect.contains(mouse) {
                if mouse_delta_position() != Vec2::ZERO {
                    choice = n;
                }
                if is_mouse_button_pressed(MouseButton::Left) {
                    choice = n;
                    open = true;
                }
            }
            if n == choice {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, Color::new(0.10, 0.22, 0.12, 1.0));
            }
            draw_text(file.label(), x, y + 19.0, 20.0, if n == choice { TITLE } else { TEXT });
        }
        if let Some(error) = &self.code_error {
            draw_text(error, x, screen_height() - 50.0, 20.0, BAD);
        }
        if let Some(dir) = replays::dir() {
            draw_text(dir.display().to_string(), x, screen_height() - 22.0, 16.0, DIM);
        }
        self.screen = Screen::Replays { choice };
        if is_key_pressed(KeyCode::Escape) {
            self.code_error = None;
            self.screen = Screen::Menu;
        } else if is_key_pressed(KeyCode::Delete) && count > 0 {
            let (path, _) = self.replay_list.remove(choice);
            let _ = std::fs::remove_file(path);
        } else if open {
            let file = self.replay_list[choice].1.clone();
            if let Err(error) = self.open_replay_file(file) {
                self.code_error = Some(error);
            }
        }
    }

    fn code_frame(&mut self) {
        self.sounds.silence();
        clear_background(BACKGROUND);
        while let Some(c) = get_char_pressed() {
            if (c.is_ascii_alphanumeric() || c == '-') && self.code_text.len() < 32 {
                self.code_text.push(c.to_ascii_uppercase());
                self.code_error = None;
            }
        }
        let pasted = (input::ctrl_down() && is_key_pressed(KeyCode::V))
            .then(macroquad::miniquad::window::clipboard_get)
            .flatten();
        if let Some(text) = pasted {
            let text: String = text.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
            self.code_text = text.to_ascii_uppercase().chars().take(32).collect();
            self.code_error = None;
        }
        if is_key_pressed(KeyCode::Backspace) {
            self.code_text.pop();
            self.code_error = None;
        }
        let x = 60.0;
        draw_text("SHIFT CODE", x, 80.0, 56.0, TITLE);
        draw_text("Type or paste (Ctrl+V) a code from a debrief, to play the same traffic.", x, 112.0, 20.0, DIM);
        draw_text("ENTER play | ESC back", x, 136.0, 20.0, DIM);
        let shown = format!("{}_", self.code_text);
        draw_rectangle_lines(x - 10.0, 170.0, 560.0, 48.0, 1.5, DIM);
        draw_text(&shown, x, 204.0, 30.0, TEXT);
        let parsed = ShiftCode::parse(&self.code_text);
        if let Some(error) = &self.code_error {
            draw_text(error, x, 252.0, 22.0, BAD);
        } else if let Ok(code) = &parsed {
            let difficulty = Difficulty {
                busyness: code.busyness,
                emergencies: code.emergencies,
                mistakes: code.mistakes,
                weather: code.weather,
            };
            let real_data = self.airports.iter().find(|a| a.code == code.airport).is_some_and(|a| a.traffic.is_some());
            let line = format!(
                "{}  {}  {}{}",
                code.airport,
                code.staffing.label(),
                shift::label(code.minutes),
                if code.vfr && code.staffing.tower() { "  VFR" } else { "" },
            );
            draw_text(&line, x, 252.0, 22.0, TITLE);
            draw_text(difficulty.summary(real_data), x, 280.0, 20.0, DIM);
        }
        if is_key_pressed(KeyCode::Escape) {
            self.screen = Screen::Menu;
        } else if start_pressed() {
            match parsed {
                Ok(code) => match self.airports.iter().position(|a| a.code == code.airport) {
                    Some(index) => self.start_world(index, code.config()),
                    None => self.code_error = Some(format!("This version has no airport {}", code.airport)),
                },
                Err(error) => self.code_error = Some(error.to_string()),
            }
        }
    }

    fn start_lesson(&mut self, kind: LessonKind) {
        self.end_session("the host left");
        let tutorial = Tutorial::new(kind);
        self.world = tutorial.world();
        let scale = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
        self.ui = gfx::laid_out_at(scale, || UiState::new(&self.world.airport, kind.staffing()));
        self.other_seats.clear();
        self.ui.lesson = true;
        self.world.set_clock_start(self.ui.clock_start);
        self.recording = None;
        self.code = None;
        self.replay_path = None;
        self.note = None;
        self.sounds.reset(&self.world);
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
        let room = match self.screen {
            Screen::Menu => gfx::MENU_ROOM,
            _ => gfx::GAME_ROOM,
        };
        if gfx::begin_frame(self.ui_scale, room) {
            warm_font_cache();
        }
        match self.screen {
            Screen::Menu => {
                self.sounds.silence();
                let count = self.airports.len();
                if is_key_pressed(KeyCode::Up) {
                    self.choice = (self.choice + count - 1) % count;
                }
                if is_key_pressed(KeyCode::Down) {
                    self.choice = (self.choice + 1) % count;
                }
                if is_key_pressed(KeyCode::P) || is_key_pressed(KeyCode::Tab) {
                    self.position = (self.position + 1) % Staffing::CHOICES.len();
                }
                if is_key_pressed(KeyCode::V) && self.staffing().tower() {
                    self.vfr = !self.vfr;
                }
                self.shift_keys();
                let picked = draw_menu(
                    &self.airports,
                    self.choice,
                    MenuState {
                        difficulty: self.difficulty,
                        staffing: self.staffing(),
                        vfr: self.vfr,
                        shift_minutes: self.shift_minutes,
                    },
                );
                if picked.difficulty || is_key_pressed(KeyCode::D) {
                    self.screen = Screen::Difficulty { row: 0 };
                    return;
                }
                if picked.settings || is_key_pressed(KeyCode::O) {
                    self.screen = Screen::Settings { choice: 0 };
                    return;
                }
                if picked.code || is_key_pressed(KeyCode::C) {
                    self.code_text.clear();
                    self.code_error = None;
                    while get_char_pressed().is_some() {}
                    self.screen = Screen::Code;
                    return;
                }
                if picked.coop || is_key_pressed(KeyCode::K) {
                    self.save_settings();
                    self.coop_flow = Some(coop::Flow::Menu { choice: 0 });
                    while get_char_pressed().is_some() {}
                    self.screen = Screen::Coop;
                    return;
                }
                if picked.replays || is_key_pressed(KeyCode::R) {
                    self.replay_list = replays::list();
                    self.code_error = None;
                    self.screen = Screen::Replays { choice: 0 };
                    return;
                }
                if let Some(minutes) = picked.shift_minutes {
                    self.shift_minutes = minutes;
                    self.typed = None;
                }
                if picked.vfr {
                    self.vfr = !self.vfr;
                }
                if let Some(position) = picked.position {
                    self.position = position;
                }
                let lesson_keys = [KeyCode::F5, KeyCode::F6, KeyCode::F7, KeyCode::F8];
                let lesson = picked
                    .lesson
                    .or_else(|| lesson_keys.iter().position(|&k| is_key_pressed(k)).map(|i| LessonKind::ALL[i]));
                if let Some(kind) = lesson {
                    self.start_lesson(kind);
                } else if start_pressed() {
                    self.start_shift();
                }
            }
            Screen::Difficulty { mut row } => {
                self.sounds.silence();
                let real_data = self.airports[self.choice].traffic.is_some();
                match difficulty::frame(&mut self.difficulty, &mut row, real_data) {
                    difficulty::Action::Stay => self.screen = Screen::Difficulty { row },
                    difficulty::Action::Back => {
                        self.save_settings();
                        self.screen = Screen::Menu;
                    }
                    difficulty::Action::Start => self.start_shift(),
                }
            }
            Screen::Settings { choice } => self.settings_frame(choice),
            Screen::Playing => self.play_frame(),
            Screen::Paused { choice, show_controls } => self.pause_frame(choice, show_controls),
            Screen::Debrief { end, choice, scroll } => self.debrief_frame(end, choice, scroll),
            Screen::Replay { end } => self.replay_frame(end),
            Screen::Rebuilding => self.rebuild_frame(),
            Screen::Replays { choice } => self.replays_frame(choice),
            Screen::Code => self.code_frame(),
            Screen::Coop => self.coop_frame(),
        }
    }

    /// The shift length keys on the menu: minus and plus step it (a minute at
    /// a time with Shift), and number keys type it in minutes.
    fn shift_keys(&mut self) {
        // Digits first, so a step in the same frame applies to what was typed.
        while let Some(c) = get_char_pressed() {
            if let Some(digit) = c.to_digit(10) {
                let typed = shift::Typed::digit(self.typed, digit, get_time());
                self.shift_minutes = typed.minutes();
                self.typed = Some(typed);
            }
        }
        let fine = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) {
            self.shift_minutes = shift::step(self.shift_minutes, false, fine);
            self.typed = None;
        }
        if is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd) {
            self.shift_minutes = shift::step(self.shift_minutes, true, fine);
            self.typed = None;
        }
    }

    fn pause_frame(&mut self, mut choice: usize, mut show_controls: bool) {
        self.sounds.silence();
        // Co-op doesn't stop for one player's menu.
        if self.session.is_some() {
            self.run_session();
            if let Some(end) = self.world.ended {
                self.finish_shift(end);
                return;
            }
        }
        draw_views(&self.world, &mut self.ui);
        let items = self.pause_items();
        let count = items.len();
        if is_key_pressed(KeyCode::Up) {
            choice = (choice + count - 1) % count;
        }
        if is_key_pressed(KeyCode::Down) {
            choice = (choice + 1) % count;
        }
        let labels: Vec<String> = items.iter().map(|&item| self.item_label(item, show_controls)).collect();
        let (hovered, clicked) = draw_pause(choice, &labels, show_controls, self.ui.staffing);
        if let Some(code) = self.code.as_ref().filter(|_| !show_controls) {
            let text = format!("Shift code  {}", code.text());
            let width = measure_text(&text, None, 18, 1.0).width;
            draw_text(&text, (screen_width() - width) / 2.0, screen_height() - 24.0, 18.0, DIM);
        }
        let item = items[choice];
        if is_key_pressed(KeyCode::Left) {
            self.adjust(item, -1);
        }
        if is_key_pressed(KeyCode::Right) {
            self.adjust(item, 1);
        }
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
        match items[choice] {
            PauseItem::Resume => self.resume(),
            PauseItem::Restart => self.restart(),
            PauseItem::Sound
            | PauseItem::Voices
            | PauseItem::OwnVoice
            | PauseItem::GameCues
            | PauseItem::UiScale
            | PauseItem::Colours => {
                self.adjust(item, 0)
            }
            PauseItem::Controls => {
                show_controls = !show_controls;
                self.screen = Screen::Paused { choice, show_controls };
            }
            PauseItem::MainMenu => {
                self.save_left_shift();
                self.end_session("the host left");
                self.tutorial = None;
                self.screen = Screen::Menu;
            }
            PauseItem::Quit => {
                self.save_left_shift();
                self.end_session("the host left");
                macroquad::miniquad::window::order_quit();
            }
            PauseItem::Back => {}
        }
    }

    /// The pause menu's rows: a co-op shift is everyone's, so it isn't
    /// restarted from here.
    fn pause_items(&self) -> Vec<PauseItem> {
        PAUSE_ITEMS.iter().copied().filter(|&item| !(self.session.is_some() && item == PauseItem::Restart)).collect()
    }

    fn item_label(&self, item: PauseItem, show_controls: bool) -> String {
        match item {
            PauseItem::Resume if self.session.is_some() => "Back to the shift".into(),
            PauseItem::Resume => "Resume".into(),
            PauseItem::Restart => "Restart shift".into(),
            PauseItem::Sound => format!("Sound: {}", self.sounds.volume.label()),
            PauseItem::GameCues => format!("Game cues: {}", if self.sounds.game_cues { "On" } else { "Off" }),
            PauseItem::Voices => format!("Radio voices: {}", if self.sounds.voices.enabled { "On" } else { "Off" }),
            PauseItem::OwnVoice => {
                format!("Speak my transmissions: {}", if self.sounds.voices.own { "On" } else { "Off" })
            }
            PauseItem::UiScale => {
                let chosen = percent(self.ui_scale);
                let fits = gfx::effective_scale(self.ui_scale, gfx::GAME_ROOM);
                if fits < self.ui_scale {
                    format!("UI scale: {chosen} ({} fits)", percent(fits))
                } else {
                    format!("UI scale: {chosen}")
                }
            }
            PauseItem::Colours => format!("Colours: {}", self.palette.label()),
            PauseItem::Controls if show_controls => "Hide controls".into(),
            PauseItem::Controls => "Show controls".into(),
            PauseItem::MainMenu if self.session.is_some() => "Leave the shift".into(),
            PauseItem::MainMenu => "Quit to main menu".into(),
            PauseItem::Quit => "Quit to desktop".into(),
            PauseItem::Back => "Back".into(),
        }
    }

    /// The settings screen, from the start menu.
    fn settings_frame(&mut self, mut choice: usize) {
        self.sounds.silence();
        clear_background(BACKGROUND);
        let count = SETTINGS_ITEMS.len();
        if is_key_pressed(KeyCode::Up) {
            choice = (choice + count - 1) % count;
        }
        if is_key_pressed(KeyCode::Down) {
            choice = (choice + 1) % count;
        }
        let item = SETTINGS_ITEMS[choice];
        if is_key_pressed(KeyCode::Left) {
            self.adjust(item, -1);
        }
        if is_key_pressed(KeyCode::Right) {
            self.adjust(item, 1);
        }
        let labels: Vec<String> = SETTINGS_ITEMS.iter().map(|&item| self.item_label(item, false)).collect();
        let (hovered, clicked) = draw_menu_list("SETTINGS", choice, &labels, "Up/Down  Left/Right change  ESC back");
        if let Some(index) = hovered.filter(|_| mouse_delta_position() != Vec2::ZERO || clicked) {
            choice = index;
        }
        let path = Settings::path().map(|p| p.display().to_string()).unwrap_or_else(|| "not saved".into());
        let width = measure_text(&path, None, 16, 1.0).width;
        draw_text(&path, (screen_width() - width) / 2.0, screen_height() - 24.0, 16.0, DIM);
        self.screen = Screen::Settings { choice };
        let back = is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::O);
        if back || ((clicked || start_pressed()) && SETTINGS_ITEMS[choice] == PauseItem::Back) {
            self.screen = Screen::Menu;
        } else if clicked || start_pressed() {
            self.adjust(SETTINGS_ITEMS[choice], 0);
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
        // Hot seat: hand the screen to the next seat.
        if is_key_pressed(KeyCode::F9) && !self.other_seats.is_empty() {
            let next = self.other_seats.remove(0);
            let mut done = std::mem::replace(&mut self.ui, next);
            done.drag = input::Drag::None;
            self.ui.paused = done.paused;
            self.ui.rate = done.rate;
            self.other_seats.push(done);
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

        if let Some(session) = &self.session {
            // The session keeps the time: real time, for everyone.
            self.ui.paused = false;
            self.ui.rate = 1.0;
            for (slot, name) in self.names.iter_mut().zip(session.names()) {
                if name.is_some() {
                    *slot = name;
                }
            }
            self.ui.names = self.names.clone();
            self.run_session();
        } else if !self.ui.paused {
            self.accumulator += get_frame_time().min(MAX_FRAME_SECONDS) * self.ui.rate;
            while self.accumulator >= TICK {
                self.accumulator -= TICK;
                self.world.tick();
            }
        }
        let events = self.world.drain_events();
        self.sounds.update(&self.world, &self.ui, &events);

        if let Some(tutorial) = &mut self.tutorial {
            tutorial.update(&mut self.world);
        }

        draw_views(&self.world, &mut self.ui);
        panels::draw(&mut self.world, &mut self.ui);
        if let Some(tutorial) = &self.tutorial {
            tutorial::draw(tutorial, &self.world, &self.ui);
        }
        if !self.other_seats.is_empty() {
            let text = format!("HOT SEAT: seat {}, {}. F9 for the next seat", self.ui.seat, self.ui.staffing.label());
            draw_text(&text, 12.0, screen_height() - 12.0, 20.0, YELLOW);
        }
        let status = self.session.as_ref().map(|s| s.status(&self.world, self.ui.seat)).unwrap_or_default();
        self.banner.draw(&status, screen_height() - 12.0);

        if let Some(recording) = &mut self.recording {
            recording.update(&self.world);
        }
        if let Some(end) = self.world.ended {
            self.finish_shift(end);
        }
    }
}

fn start_pressed() -> bool {
    is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)
}

/// Five-notch slider for traffic level, under a title at `y`. Returns a
/// level if one was clicked.
fn draw_busyness(busyness: Busyness, real_data: bool, x: f32, y: f32) -> Option<Busyness> {
    const SPACING: f32 = 112.0;
    const HIT: f32 = 14.0;
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

/// Briefing line spacing on the menu, and the least it shrinks to on a short window.
const BRIEFING_LINE: f32 = 23.0;
const MIN_BRIEFING_LINE: f32 = 19.0;
/// Where a wrapped briefing line carries on, under the text after its label.
const BRIEFING_INDENT: &str = "          ";

/// Split a briefing line to fit `width` pixels, carrying on indented.
fn wrap_to_width(line: &str, width: f32, size: f32) -> Vec<String> {
    let fits = |text: &str| measure_text(text, None, size as u16, 1.0).width <= width;
    if fits(line) {
        return vec![line.to_string()];
    }
    let body = line.trim_start();
    let mut out = Vec::new();
    let mut current = line[..line.len() - body.len()].to_string();
    for word in body.split(' ') {
        let candidate = if current.trim().is_empty() { format!("{current}{word}") } else { format!("{current} {word}") };
        if fits(&candidate) || current.trim().is_empty() {
            current = candidate;
        } else {
            out.push(current);
            current = format!("{BRIEFING_INDENT}{word}");
        }
    }
    out.push(current);
    out
}

/// Where the position buttons wrap to a second row.
const POSITION_ROW_BREAK: usize = 4;

/// Position choices as two rows of buttons. Returns one if clicked.
fn draw_positions(staffing: Staffing, x: f32, y: f32) -> Option<usize> {
    const GAP: f32 = 10.0;
    draw_text("POSITION  P or click to choose", x, y, 20.0, DIM);
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut left = x;
    let mut top = y + 10.0;
    let mut picked = None;
    for (index, &choice) in Staffing::CHOICES.iter().enumerate() {
        // The surface positions on the first row, the combinations on the second.
        if index == POSITION_ROW_BREAK {
            left = x;
            top += 38.0;
        }
        let label = choice.label();
        let width = measure_text(label, None, 20, 1.0).width + 24.0;
        let rect = Rect::new(left, top, width, 30.0);
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
    /// The difficulty box was clicked.
    difficulty: bool,
    position: Option<usize>,
    /// The VFR toggle was clicked.
    vfr: bool,
    /// A lesson button was clicked.
    lesson: Option<LessonKind>,
    /// The settings button was clicked.
    settings: bool,
    /// The shift code button.
    code: bool,
    /// The replays button.
    replays: bool,
    /// The co-op button.
    coop: bool,
    shift_minutes: Option<u32>,
}

/// The menu's current choices, to draw.
struct MenuState {
    difficulty: Difficulty,
    staffing: Staffing,
    vfr: bool,
    shift_minutes: u32,
}

/// A menu button. Returns whether it was clicked, and its width.
fn menu_button(label: &str, left: f32, top: f32, selected: bool) -> (bool, f32) {
    let width = measure_text(label, None, 20, 1.0).width + 20.0;
    let r = Rect::new(left, top, width, 30.0);
    if selected {
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.10, 0.22, 0.12, 1.0));
    }
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, if selected { TITLE } else { DIM });
    draw_text(label, r.x + 10.0, r.y + 21.0, 20.0, if selected { TITLE } else { TEXT });
    let clicked = is_mouse_button_pressed(MouseButton::Left) && r.contains(Vec2::from(mouse_position()));
    (clicked, width)
}

/// The shift length: minus and plus buttons around the length, then the
/// presets. Returns a new length if one was clicked.
fn draw_shift_length(minutes: u32, x: f32, y: f32) -> Option<u32> {
    const GAP: f32 = 8.0;
    draw_text("SHIFT LENGTH  - or +, type minutes, or click", x, y, 20.0, DIM);
    let fine = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
    let mut picked = None;

    let top = y + 10.0;
    let (less, width) = menu_button("-", x, top, false);
    let value = shift::label(minutes);
    let value_x = x + width + GAP;
    let value_w = measure_text("12 h 59 min", None, 24, 1.0).width + 16.0;
    let text_x = value_x + (value_w - measure_text(&value, None, 24, 1.0).width) / 2.0;
    draw_text(&value, text_x, top + 22.0, 24.0, TITLE);
    let (more, _) = menu_button("+", value_x + value_w + GAP, top, false);
    if less {
        picked = Some(shift::step(minutes, false, fine));
    }
    if more {
        picked = Some(shift::step(minutes, true, fine));
    }

    let mut left = x;
    for preset in shift::PRESETS {
        let (hit, width) = menu_button(&shift::label(preset), left, top + 40.0, preset == minutes);
        if hit {
            picked = Some(preset);
        }
        left += width + GAP;
    }
    draw_text("Hold Shift for one-minute steps. 1 min to 12 h.", x, top + 92.0, 18.0, DIM);
    picked
}

/// The lesson buttons. Returns the one clicked, if any.
fn draw_lessons(x: f32, y: f32) -> Option<LessonKind> {
    const GAP: f32 = 10.0;
    draw_text("TUTORIAL  F5 to F8 or click", x, y, 20.0, DIM);
    let mouse = Vec2::from(mouse_position());
    let clicked = is_mouse_button_pressed(MouseButton::Left);
    let mut left = x;
    let mut picked = None;
    for (kind, key) in LessonKind::ALL.into_iter().zip(["F5", "F6", "F7", "F8"]) {
        let label = format!("{key} {}", kind.short_label());
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

fn draw_menu(airports: &[Airport], choice: usize, state: MenuState) -> MenuPick {
    let MenuState { difficulty, staffing, vfr, shift_minutes } = state;
    clear_background(BACKGROUND);
    let x = 60.0;
    draw_text("AIR TRAFFIC CONTROL", x, 80.0, 56.0, TITLE);
    draw_text(format!("{} shift   {}", shift::label(shift_minutes), staffing.label()), x, 112.0, 24.0, DIM);
    let right = (screen_width() - 560.0).max(820.0);

    let mut y = 156.0;
    let mut pick = MenuPick {
        position: draw_positions(staffing, x, y),
        lesson: draw_lessons(right, 70.0),
        shift_minutes: draw_shift_length(shift_minutes, right, 150.0),
        ..Default::default()
    };
    if staffing.tower() {
        pick.vfr = draw_vfr_toggle(vfr, x, y + 106.0);
    }

    // The right-hand column: where, how busy, and go.
    let mut ry = 320.0;
    draw_text("AIRPORT  Up/Down to choose", right, ry, 20.0, DIM);
    ry += 28.0;
    for (index, airport) in airports.iter().enumerate() {
        let selected = index == choice;
        let marker = if selected { ">" } else { " " };
        let line = format!("{marker} {}  {}", airport.code, airport.name);
        draw_text(&line, right, ry, 24.0, if selected { TITLE } else { TEXT });
        ry += 28.0;
    }
    let airport = &airports[choice];
    if let Some(source) = &airport.data_source {
        draw_text(source, right, ry, 16.0, DIM);
        draw_text("Provided as is, with no warranty. See DATA_NOTICE.md.", right, ry + 19.0, 16.0, DIM);
        ry += 38.0;
    }
    ry += 26.0;
    pick.difficulty = difficulty::draw_summary(&difficulty, airport.traffic.is_some(), right, ry);
    draw_text("Press ENTER to start your shift", right, ry + 110.0, 30.0, TITLE);
    let (settings, width) = menu_button("O  Settings", right, ry + 132.0, false);
    let (code, code_width) = menu_button("C  Shift code", right + width + 10.0, ry + 132.0, false);
    let replays_left = right + width + code_width + 20.0;
    let (replays, replays_width) = menu_button("R  Replays", replays_left, ry + 132.0, false);
    let (coop, _) = menu_button("K  Co-op", replays_left + replays_width + 10.0, ry + 132.0, false);
    (pick.settings, pick.code, pick.replays, pick.coop) = (settings, code, replays, coop);

    // The left: the briefing, wrapped clear of the right column, closer
    // together if it's long.
    y += 146.0;
    let vfr = vfr && staffing.tower();
    let width = right - x - 30.0;
    let mut lines = Vec::new();
    for line in briefing(staffing, vfr).into_iter().chain(controls(staffing)).chain(std::iter::once(BRIEFING_VIEW)) {
        lines.extend(wrap_to_width(line, width, 20.0));
    }
    let spacing = ((screen_height() - y - 8.0) / lines.len() as f32).clamp(MIN_BRIEFING_LINE, BRIEFING_LINE);
    let size = (20.0 * spacing / BRIEFING_LINE).max(18.0);
    for line in lines {
        draw_text(&line, x, y, size, TEXT);
        y += spacing;
    }
    pick
}

fn percent(scale: f32) -> String {
    format!("{:.0}%", scale * 100.0)
}

/// A centred menu of rows under a title. Returns the row under the mouse and
/// whether it was clicked.
fn draw_menu_list(title: &str, choice: usize, labels: &[String], hint: &str) -> (Option<usize>, bool) {
    let height = 70.0 + PAUSE_ROW * labels.len() as f32 + 40.0;
    let width = labels.iter().map(|l| measure_text(l, None, 24, 1.0).width + 64.0).fold(PAUSE_WIDTH, f32::max);
    let left = (screen_width() - width) / 2.0;
    let top = (screen_height() - height) / 2.0;
    draw_rectangle(left, top, width, height, BACKGROUND);
    draw_rectangle_lines(left, top, width, height, 1.5, DIM);
    let size = measure_text(title, None, 36, 1.0);
    draw_text(title, left + (width - size.width) / 2.0, top + 46.0, 36.0, TITLE);

    let mouse = Vec2::from(mouse_position());
    let mut hovered = None;
    for (index, label) in labels.iter().enumerate() {
        let y = top + 70.0 + PAUSE_ROW * index as f32;
        let row = Rect::new(left + 16.0, y, width - 32.0, PAUSE_ROW - 6.0);
        if row.contains(mouse) {
            hovered = Some(index);
        }
        let selected = index == choice;
        if selected {
            draw_rectangle(row.x, row.y, row.w, row.h, Color::new(0.10, 0.22, 0.12, 1.0));
        }
        let label_width = measure_text(label, None, 24, 1.0).width;
        let color = if selected { TITLE } else { TEXT };
        draw_text(label, left + (width - label_width) / 2.0, y + 24.0, 24.0, color);
    }
    let hint_width = measure_text(hint, None, 18, 1.0).width;
    draw_text(hint, left + (width - hint_width) / 2.0, top + height - 16.0, 18.0, DIM);
    let clicked = hovered.is_some() && is_mouse_button_pressed(MouseButton::Left);
    (hovered, clicked)
}

/// Pause menu over the frozen scope. Returns the row under the mouse and whether it was clicked.
fn draw_pause(choice: usize, labels: &[String], show_controls: bool, staffing: Staffing) -> (Option<usize>, bool) {
    draw_rectangle(0.0, 0.0, screen_width(), screen_height(), Color::new(0.0, 0.0, 0.0, 0.65));
    let (hovered, clicked) = draw_menu_list("PAUSED", choice, labels, "Up/Down  Left/Right change  ESC resume");
    let height = 70.0 + PAUSE_ROW * labels.len() as f32 + 40.0;
    let top = (screen_height() - height) / 2.0;

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

    (hovered, clicked)
}
