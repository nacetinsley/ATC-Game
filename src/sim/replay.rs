//! Replays and shift codes.
//!
//! The sim is deterministic: the same seed and the same commands at the same
//! ticks give the same shift. So a replay is the shift's settings and what the
//! player said, and the world is rebuilt from those. Snapshots taken along the
//! way make seeking quick.
//!
//! A shift code packs the settings and the seed into a short string, so a
//! shift's traffic can be played again, or shared.

use super::clearance::Clearance;
use super::emergency::EmergencyRate;
use super::mistakes::PilotMistakes;
use super::positions::Staffing;
use super::scoring::ShiftEnd;
use super::traffic::Busyness;
use super::weather::WeatherKind;
use super::{World, WorldConfig};
use serde::{Deserialize, Serialize};

/// Ticks between snapshots: a minute of sim time.
pub const SNAPSHOT_TICKS: u32 = 240;

/// Something the player said, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// Applied before this tick runs: the world's tick count when it was said.
    pub tick: u32,
    pub id: u32,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Transmit(Vec<Clearance>),
    AcceptHandoff,
    /// On Ground, asking Local on the landline to cross this physical runway.
    RequestCrossing(usize),
    /// Answering a landline request about the aircraft.
    Coordinate { approve: bool },
    /// On Local, asking Approach on the landline to release the departure.
    RequestRelease,
    /// On Approach, putting departures on call for release, or letting them go.
    CallForRelease { on: bool },
    /// On Local, telling Approach about a go-around.
    CallGoAround,
    /// Telling the next position about an emergency before the handoff.
    CallEmergency,
    /// Quitting to the main menu: the shift ends here.
    Leave,
}

impl World {
    pub(super) fn record(&mut self, id: u32, action: Action) {
        self.commands.push(Command { tick: self.ticks, id, action });
    }

    /// Everything the player has said this shift.
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Ticks run so far.
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// Say a recorded command again.
    pub fn apply(&mut self, command: &Command) -> bool {
        match &command.action {
            Action::Transmit(clearances) => self.transmit(command.id, clearances.clone()),
            Action::AcceptHandoff => self.accept_handoff(command.id),
            Action::RequestCrossing(runway) => self.request_crossing(command.id, *runway),
            Action::Coordinate { approve } => self.answer_coordination(command.id, *approve),
            Action::RequestRelease => self.request_release(command.id),
            Action::CallForRelease { on } => self.set_call_for_release(*on),
            Action::CallGoAround => self.call_go_around(command.id),
            Action::CallEmergency => self.call_emergency(command.id),
            Action::Leave => {
                self.end_shift(ShiftEnd::Left);
                true
            }
        }
    }

    /// The player quits before the shift is over: it ends here, and its
    /// replay stops here too.
    pub fn leave_shift(&mut self) {
        if self.ended.is_none() {
            self.record(0, Action::Leave);
            self.end_shift(ShiftEnd::Left);
        }
    }

    /// Apply the commands due now, then run one tick.
    pub fn replay_tick(&mut self, commands: &[Command]) {
        let start = commands.partition_point(|c| c.tick < self.ticks);
        let end = commands.partition_point(|c| c.tick <= self.ticks);
        for command in &commands[start..end] {
            self.apply(command);
        }
        self.tick();
    }

    /// Run forward to `tick` with these commands, or until the shift ends.
    pub fn run_to(&mut self, tick: u32, commands: &[Command]) {
        // A world whose shift has ended doesn't tick any more.
        while self.ticks < tick && self.ended.is_none() {
            self.replay_tick(commands);
        }
    }

    /// Say on the log that the player took over a replay here.
    pub fn note_take_over(&mut self) {
        let s = self.time.max(0.0) as u32;
        self.say(super::Speaker::System, format!("Taking over at {:02}:{:02}.", s / 60, s % 60));
    }

    /// How the shift came out, to check a rebuilt replay against.
    pub fn summary(&self) -> Summary {
        Summary { ticks: self.ticks, points: self.score.points, incidents: self.score.incidents.len() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub ticks: u32,
    pub points: i32,
    pub incidents: usize,
}

/// A shift as played: its commands, and snapshots of the world to seek from.
#[derive(Clone)]
pub struct Recording {
    pub commands: Vec<Command>,
    /// Oldest first; the first is the world at tick 0.
    snapshots: Vec<World>,
}

impl Recording {
    /// Start recording a world that hasn't run yet.
    pub fn new(world: &World) -> Recording {
        Recording { commands: world.commands.clone(), snapshots: vec![world.clone()] }
    }

    /// Keep a snapshot if a minute has passed since the last, and the
    /// commands so far.
    pub fn update(&mut self, world: &World) {
        let last = self.snapshots.last().map_or(0, |s| s.ticks);
        if world.ticks >= last + SNAPSHOT_TICKS {
            self.snapshots.push(world.clone());
        }
        if world.commands.len() != self.commands.len() {
            self.commands = world.commands.clone();
        }
    }

    /// The world at `tick`, or at the end of the shift if that comes first.
    pub fn seek(&self, tick: u32) -> World {
        let index = self.snapshots.partition_point(|s| s.ticks <= tick).max(1) - 1;
        let mut world = self.snapshots[index].clone();
        world.run_to(tick, &self.commands);
        world
    }

    /// The last tick the replay reaches.
    pub fn end_tick(&self) -> u32 {
        self.snapshots.last().map_or(0, |s| s.ticks)
    }

    /// Rebuild snapshots from the start, a chunk of ticks at a time. Returns
    /// true once the whole shift has been run.
    pub fn rebuild_step(&mut self, ticks: u32) -> bool {
        let mut world = self.snapshots.last().expect("a recording starts with a world").clone();
        if world.ended.is_some() {
            return true;
        }
        let stop = world.ticks + ticks;
        while world.ticks < stop && world.ended.is_none() {
            world.replay_tick(&self.commands);
            if world.ticks.is_multiple_of(SNAPSHOT_TICKS) || world.ended.is_some() {
                self.snapshots.push(world.clone());
            }
        }
        world.ended.is_some()
    }

    /// A recording to rebuild from its start: the world at tick 0 and the commands.
    pub fn from_start(world: World, commands: Vec<Command>) -> Recording {
        Recording { commands, snapshots: vec![world] }
    }

    /// Keep the shift as it was up to `world`, to play on from there.
    pub fn take_over(&mut self, world: &World) {
        self.snapshots.retain(|s| s.ticks <= world.ticks);
        self.commands.truncate(self.commands.partition_point(|c| c.tick < world.ticks));
    }

    /// The final world, once the shift has ended.
    pub fn last(&self) -> &World {
        self.snapshots.last().expect("a recording starts with a world")
    }

    /// Add the world at the end of the shift.
    pub fn finish(&mut self, world: &World) {
        self.commands = world.commands.clone();
        if self.snapshots.last().is_none_or(|s| s.ticks < world.ticks) {
            self.snapshots.push(world.clone());
        }
    }
}

/// The settings and seed a shift is generated from, short enough to type.
#[derive(Debug, Clone, PartialEq)]
pub struct ShiftCode {
    pub airport: String,
    pub seed: u32,
    pub staffing: Staffing,
    pub busyness: Busyness,
    pub emergencies: EmergencyRate,
    pub mistakes: PilotMistakes,
    pub weather: WeatherKind,
    pub vfr: bool,
    pub minutes: u32,
}

/// Crockford's base 32: no I, L, O or U, so nothing reads as something else.
const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 57 bits of settings and 8 of check: 13 characters.
const CHARS: usize = 13;

fn index_of<T: PartialEq>(all: &[T], value: &T) -> u64 {
    all.iter().position(|v| v == value).unwrap_or(0) as u64
}

/// Eight bits from the settings and the airport, so a typo is caught.
fn check(bits: u64, airport: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bits.to_le_bytes().iter().chain(airport.as_bytes()) {
        hash = (hash ^ *byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    (hash ^ (hash >> 32) ^ (hash >> 16) ^ (hash >> 8)) & 0xff
}

impl ShiftCode {
    /// The code for a world's settings.
    pub fn of(config: &WorldConfig, airport: &str) -> ShiftCode {
        ShiftCode {
            airport: airport.to_string(),
            seed: config.seed as u32,
            staffing: config.staffing,
            busyness: config.busyness,
            emergencies: config.emergencies,
            mistakes: config.mistakes,
            weather: config.weather,
            vfr: config.vfr,
            minutes: (config.shift_seconds / 60.0).round() as u32,
        }
    }

    /// The settings for a shift from this code. The caller sets the clock.
    pub fn config(&self) -> WorldConfig {
        WorldConfig {
            seed: self.seed as u64,
            staffing: self.staffing,
            busyness: self.busyness,
            emergencies: self.emergencies,
            mistakes: self.mistakes,
            weather: self.weather,
            vfr: self.vfr,
            shift_seconds: self.minutes as f32 * 60.0,
            ..Default::default()
        }
    }

    fn bits(&self) -> u64 {
        let mut bits = self.seed as u64;
        let mut push = |value: u64, width: u32| bits = (bits << width) | (value & ((1 << width) - 1));
        push(index_of(&Staffing::CHOICES, &self.staffing), 4);
        push(index_of(&Busyness::ALL, &self.busyness), 3);
        push(index_of(&EmergencyRate::ALL, &self.emergencies), 3);
        push(index_of(&PilotMistakes::ALL, &self.mistakes), 2);
        push(index_of(&WeatherKind::ALL, &self.weather), 2);
        push(self.vfr as u64, 1);
        push(self.minutes.min(1023) as u64, 10);
        bits
    }

    /// "KSUS-7KQ4-XM2P-AB3CD".
    pub fn text(&self) -> String {
        let bits = self.bits();
        let mut value = ((bits as u128) << 8) | check(bits, &self.airport) as u128;
        let mut chars = [0u8; CHARS];
        for c in chars.iter_mut().rev() {
            *c = DIGITS[(value & 31) as usize];
            value >>= 5;
        }
        let s = String::from_utf8_lossy(&chars).into_owned();
        format!("{}-{}-{}-{}", self.airport, &s[..4], &s[4..8], &s[8..])
    }

    /// Read a code, as typed: case, spaces and dashes don't matter.
    pub fn parse(text: &str) -> Result<ShiftCode, &'static str> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
        let (airport, rest) = text.split_once('-').ok_or("A code starts with the airport, then a dash")?;
        if airport.is_empty() || !airport.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("A code starts with the airport, then a dash");
        }
        let body: Vec<char> = rest.chars().filter(|&c| c != '-').collect();
        if body.len() != CHARS {
            return Err("That code is the wrong length");
        }
        let mut value: u128 = 0;
        for c in body {
            let c = match c {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let digit = DIGITS.iter().position(|&d| d as char == c).ok_or("That code has a letter codes don't use")?;
            value = (value << 5) | digit as u128;
        }
        let bits = (value >> 8) as u64;
        if (value & 0xff) as u64 != check(bits, airport) {
            return Err("That code doesn't check out: is there a typo?");
        }
        let mut rest = bits;
        let mut take = |width: u32| {
            let value = rest & ((1 << width) - 1);
            rest >>= width;
            value as usize
        };
        let minutes = take(10) as u32;
        let vfr = take(1) == 1;
        let weather = *WeatherKind::ALL.get(take(2)).ok_or("That code is from a different version")?;
        let mistakes = *PilotMistakes::ALL.get(take(2)).ok_or("That code is from a different version")?;
        let emergencies = *EmergencyRate::ALL.get(take(3)).ok_or("That code is from a different version")?;
        let busyness = *Busyness::ALL.get(take(3)).ok_or("That code is from a different version")?;
        let staffing = *Staffing::CHOICES.get(take(4)).ok_or("That code is from a different version")?;
        let seed = rest as u32;
        if minutes == 0 {
            return Err("That code is from a different version");
        }
        Ok(ShiftCode { airport: airport.to_string(), seed, staffing, busyness, emergencies, mistakes, weather, vfr, minutes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> ShiftCode {
        ShiftCode {
            airport: "KSUS".into(),
            seed: 0xdead_beef,
            staffing: Staffing::LOCAL_GROUND,
            busyness: Busyness::Busy,
            emergencies: EmergencyRate::Realistic,
            mistakes: PilotMistakes::Frequent,
            weather: WeatherKind::Storms,
            vfr: true,
            minutes: 720,
        }
    }

    #[test]
    fn codes_round_trip() {
        let c = code();
        let text = c.text();
        assert_eq!(text.len(), "KSUS-".len() + CHARS + 2, "{text}");
        assert_eq!(ShiftCode::parse(&text), Ok(c.clone()));
        assert_eq!(ShiftCode::parse(&text.to_lowercase().replace('-', " - ")), Ok(c.clone()));
        for staffing in Staffing::CHOICES {
            for weather in WeatherKind::ALL {
                for emergencies in EmergencyRate::ALL {
                    let c = ShiftCode { staffing, weather, emergencies, minutes: 1, vfr: false, ..code() };
                    assert_eq!(ShiftCode::parse(&c.text()), Ok(c));
                }
            }
        }
        for (busyness, mistakes) in Busyness::ALL.into_iter().zip(PilotMistakes::ALL) {
            let c = ShiftCode { busyness, mistakes, seed: 0, ..code() };
            assert_eq!(ShiftCode::parse(&c.text()), Ok(c));
        }
    }

    #[test]
    fn typos_are_caught() {
        let text = code().text();
        let mut caught = 0;
        let mut tried = 0;
        for (i, ch) in text.char_indices().skip(5).filter(|(_, c)| *c != '-') {
            for replacement in ['0', '7', 'K', 'Z'] {
                if replacement == ch {
                    continue;
                }
                let mut typo = text.clone();
                typo.replace_range(i..i + 1, &replacement.to_string());
                tried += 1;
                if ShiftCode::parse(&typo).is_err() {
                    caught += 1;
                }
            }
        }
        // Eight check bits miss about one typo in 256.
        assert!(caught * 100 >= tried * 97, "{caught} of {tried}");
        assert!(ShiftCode::parse("KSUS-123").is_err());
        assert!(ShiftCode::parse("KSUS7KQ4XM2PAB3CD").is_err());
        assert!(ShiftCode::parse(&text.replace("KSUS", "XHGT")).is_err(), "the airport is checked too");
    }

    #[test]
    fn commands_survive_toml() {
        use super::super::holding::{HoldLeg, HoldSpec, Turns};
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct File {
            command: Vec<Command>,
        }
        let hold = HoldSpec {
            fix: 3,
            inbound: 271.5,
            turns: Turns::Left,
            leg: HoldLeg::Miles(10.0),
            published: true,
            efc: 1234.25,
            efc_hhmm: 1420,
        };
        let file = File {
            command: vec![
                Command { tick: 5, id: 1, action: Action::AcceptHandoff },
                Command {
                    tick: 9,
                    id: 2,
                    action: Action::Transmit(vec![
                        Clearance::Heading(240.0),
                        Clearance::Altitude(5000.0),
                        Clearance::GoAround,
                        Clearance::Taxi { to: 12, via: vec!["A".into(), "B3".into()] },
                        Clearance::Hold(hold),
                        Clearance::Ifr { route: vec![1, 2], altitude: 0.1 + 0.2, squawk: 0o4721 },
                        Clearance::Follow { id: 4, traffic: "Cessna, left base".into() },
                    ]),
                },
            ],
        };
        let text = toml::to_string(&file).unwrap();
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back, file, "{text}");
    }
}
