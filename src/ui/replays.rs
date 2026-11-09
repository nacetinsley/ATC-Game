//! Replay files: each shift's code, clock and commands, saved when it ends,
//! next to the settings file in a `replays` folder. Only the newest are kept.
//!
//! A co-op shift's file also holds who worked what, and the players' names.
//! Every player saves their own.

use super::settings::Settings;
use crate::sim::World;
use crate::sim::positions::{MAX_SEATS, Seat, Staffing};
use crate::sim::replay::{Command, ShiftCode, Summary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Replays kept; older ones are deleted when a new one is saved.
const KEEP: usize = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayFile {
    /// The game version that saved it.
    pub version: String,
    /// The shift code: airport, settings and seed.
    pub code: String,
    /// UTC at the shift's start, seconds since 1970: the clock on the strips
    /// and the radio, and when it was played.
    pub clock_start: f64,
    /// Co-op: each seat's player's name, "" where nobody sat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub players: Vec<String>,
    /// Co-op: the seat of the player who saved it, to watch from.
    #[serde(default)]
    pub seat: Seat,
    /// How the shift came out, to check a rebuild against.
    pub summary: Summary,
    /// Co-op: who worked what at the start. The code's staffing is then the
    /// single-player version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<Staffing>,
    #[serde(default)]
    pub command: Vec<Command>,
}

impl ReplayFile {
    /// The file for a shift that started as `start` and got to `end`, saved
    /// by the player in `seat`.
    pub fn new(code: &ShiftCode, start: &World, end: &World, names: &[Option<String>; MAX_SEATS], seat: Seat) -> ReplayFile {
        let seats = start.seated();
        let coop = seats.is_coop();
        ReplayFile {
            version: env!("CARGO_PKG_VERSION").to_string(),
            code: code.text(),
            clock_start: end.clock_start(),
            // Hot seat has no names.
            players: if coop && names.iter().any(Option::is_some) {
                names.iter().map(|n| n.clone().unwrap_or_default()).collect()
            } else {
                Vec::new()
            },
            seat: if coop { seat } else { 0 },
            summary: end.summary(),
            seats: coop.then_some(seats),
            command: end.commands().to_vec(),
        }
    }

    /// The players' names, by seat.
    pub fn names(&self) -> [Option<String>; MAX_SEATS] {
        let mut names: [Option<String>; MAX_SEATS] = Default::default();
        for (slot, name) in names.iter_mut().zip(&self.players) {
            *slot = (!name.is_empty()).then(|| name.clone());
        }
        names
    }

    /// "06 Nov 2026 14:20Z  KSUS  Local + Ground  45 min  score 312", or
    /// "Co-op: Sam, Alex" in place of the positions.
    pub fn label(&self) -> String {
        let when = date_label(self.clock_start);
        match ShiftCode::parse(&self.code) {
            Ok(code) => {
                let who = match self.seats {
                    Some(seats) => {
                        let names = self.names();
                        let players: Vec<String> = seats
                            .players()
                            .iter()
                            .map(|s| names[s as usize].clone().unwrap_or_else(|| format!("seat {}", s + 1)))
                            .collect();
                        format!("Co-op: {}", players.join(", "))
                    }
                    None => code.staffing.label().to_string(),
                };
                format!(
                    "{when}  {}  {who}  {}  score {}",
                    code.airport,
                    super::shift::label(code.minutes),
                    self.summary.points
                )
            }
            Err(_) => format!("{when}  {}", self.code),
        }
    }
}

/// The replays folder, if there's somewhere to keep it.
pub fn dir() -> Option<PathBuf> {
    Some(Settings::path()?.parent()?.join("replays"))
}

/// Save a finished shift. Returns where, if it could be written.
pub fn save(file: &ReplayFile) -> Option<PathBuf> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let code = ShiftCode::parse(&file.code).map(|c| c.airport).unwrap_or_default();
    let (y, mo, d, h, mi, s) = civil(file.clock_start);
    let mut path = dir.join(format!("{y:04}-{mo:02}-{d:02}-{h:02}{mi:02}{s:02}-{code}.toml"));
    // A shift taken over and finished again in the same second: keep both.
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{y:04}-{mo:02}-{d:02}-{h:02}{mi:02}{s:02}-{code}-{n}.toml"));
        n += 1;
    }
    let text = toml::to_string(file).ok()?;
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text).ok()?;
    std::fs::rename(&temp, &path).ok()?;
    prune(&dir);
    Some(path)
}

/// Delete all but the newest replays.
fn prune(dir: &Path) {
    let mut files = replay_paths(dir);
    files.sort();
    let extra = files.len().saturating_sub(KEEP);
    for path in &files[..extra] {
        let _ = std::fs::remove_file(path);
    }
}

fn replay_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect()
}

pub fn load(path: &Path) -> Result<ReplayFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.to_string())
}

/// Saved replays, newest first. Files that don't read are left out.
pub fn list() -> Vec<(PathBuf, ReplayFile)> {
    let Some(dir) = dir() else { return Vec::new() };
    let mut files: Vec<(PathBuf, ReplayFile)> =
        replay_paths(&dir).into_iter().filter_map(|p| load(&p).ok().map(|f| (p, f))).collect();
    files.sort_by(|a, b| b.1.clock_start.total_cmp(&a.1.clock_start).then_with(|| b.0.cmp(&a.0)));
    files
}

/// Year, month, day, hour, minute and second, UTC, from seconds since 1970.
fn civil(unix: f64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix.max(0.0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day, (rem / 3600) as u32, (rem / 60 % 60) as u32, (rem % 60) as u32)
}

/// "06 Nov 2026 14:20Z".
pub fn date_label(unix: f64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, mo, d, h, mi, _) = civil(unix);
    format!("{d:02} {} {y} {h:02}{mi:02}Z", MONTHS[(mo - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil(0.0), (1970, 1, 1, 0, 0, 0));
        // 2026-11-06 14:20:05 UTC
        assert_eq!(civil(1_793_974_805.0), (2026, 11, 6, 14, 20, 5));
        assert_eq!(civil(951_782_400.0), (2000, 2, 29, 0, 0, 0));
        assert_eq!(date_label(1_793_974_805.0), "06 Nov 2026 1420Z");
    }

    #[test]
    fn files_round_trip() {
        use crate::sim::clearance::Clearance;
        use crate::sim::replay::Action;
        let file = ReplayFile {
            version: "0.1.0".into(),
            code: "KSUS-0000-0000-00000".into(),
            clock_start: 1_793_974_805.25,
            players: Vec::new(),
            seat: 0,
            summary: Summary { ticks: 4800, points: -12, incidents: 3 },
            seats: None,
            command: vec![Command { tick: 3, id: 9, action: Action::Transmit(vec![Clearance::ClearedIls(2)]), seat: 0 }],
        };
        let text = toml::to_string(&file).unwrap();
        assert_eq!(toml::from_str::<ReplayFile>(&text).unwrap(), file, "{text}");
        assert!(!text.contains("seats") && !text.contains("players"), "a single-player file is as before: {text}");
        // A co-op one keeps the split and the names.
        use crate::sim::positions::Position;
        let split = Staffing::NONE.with_seat(Position::Approach, 0).with_seat(Position::Local, 1);
        let coop = ReplayFile {
            players: vec!["Sam".into(), "Alex".into()],
            seat: 1,
            seats: Some(split),
            command: vec![Command { tick: 5, id: 0, action: Action::Solo, seat: 0 }],
            ..file
        };
        let text = toml::to_string(&coop).unwrap();
        let back = toml::from_str::<ReplayFile>(&text).unwrap();
        assert_eq!(back, coop, "{text}");
        assert_eq!(back.names(), [Some("Sam".into()), Some("Alex".into()), None, None]);
    }
}
