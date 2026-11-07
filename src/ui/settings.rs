//! Settings kept between runs: sound, UI scale, colours, and the last choices
//! on the start menu. Saved as TOML in the user's settings folder:
//!
//! - Windows: `%APPDATA%\atcgame\settings.toml`
//! - macOS: `~/Library/Application Support/atcgame/settings.toml`
//! - Linux: `$XDG_CONFIG_HOME/atcgame/settings.toml` or `~/.config/atcgame/`
//!
//! Reading is forgiving: a missing file, or a value it doesn't recognise,
//! leaves that setting at its default.

use super::audio::Volume;
use super::difficulty::Difficulty;
use super::gfx::{Palette, SCALES};
use crate::sim::emergency::EmergencyRate;
use crate::sim::mistakes::PilotMistakes;
use crate::sim::positions::Staffing;
use crate::sim::traffic::Busyness;
use crate::sim::weather::WeatherKind;
use std::path::PathBuf;
use toml::{Table, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub volume: Volume,
    pub game_cues: bool,
    /// One of `gfx::SCALES`.
    pub ui_scale: f32,
    pub palette: Palette,
    /// The start menu's last choices.
    pub airport: String,
    pub staffing: Staffing,
    pub vfr: bool,
    pub shift_minutes: u32,
    pub difficulty: Difficulty,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            volume: Volume::default(),
            game_cues: true,
            ui_scale: 1.0,
            palette: Palette::Standard,
            airport: String::new(),
            staffing: Staffing::default(),
            vfr: true,
            shift_minutes: super::shift::DEFAULT_MINUTES,
            difficulty: Difficulty::STANDARD,
        }
    }
}

/// "Local + Ground", as the file names a position.
fn staffing_name(s: Staffing) -> String {
    let parts = [(s.approach, "Approach"), (s.local, "Local"), (s.ground, "Ground"), (s.delivery, "Delivery")];
    parts.iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect::<Vec<_>>().join(" + ")
}

/// The entry in `all` whose name (as `Debug` prints it) is `name`.
fn named<T: Copy + std::fmt::Debug>(all: &[T], name: Option<&str>) -> Option<T> {
    let name = name?;
    all.iter().copied().find(|v| format!("{v:?}") == name)
}

impl Settings {
    /// The settings file, if there's somewhere to keep it.
    pub fn path() -> Option<PathBuf> {
        let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = if cfg!(windows) {
            env("APPDATA")?
        } else if cfg!(target_os = "macos") {
            env("HOME")?.join("Library").join("Application Support")
        } else {
            env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?
        };
        Some(dir.join("atcgame").join("settings.toml"))
    }

    /// The saved settings, or the defaults.
    pub fn load() -> Settings {
        let text = Settings::path().and_then(|p| std::fs::read_to_string(p).ok());
        text.map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        let Ok(table) = text.parse::<Table>() else { return s };
        let string = |t: &Table, key: &str| t.get(key).and_then(Value::as_str).map(str::to_owned);
        let boolean = |t: &Table, key: &str| t.get(key).and_then(Value::as_bool);
        let sound = table.get("sound").and_then(Value::as_table).cloned().unwrap_or_default();
        let display = table.get("display").and_then(Value::as_table).cloned().unwrap_or_default();
        let menu = table.get("menu").and_then(Value::as_table).cloned().unwrap_or_default();
        let difficulty = table.get("difficulty").and_then(Value::as_table).cloned().unwrap_or_default();

        if let Some(v) = named(&Volume::ALL, string(&sound, "volume").as_deref()) {
            s.volume = v;
        }
        if let Some(v) = boolean(&sound, "game_cues") {
            s.game_cues = v;
        }
        let scale = display.get("ui_scale").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        if let Some(v) = scale.and_then(|v| SCALES.iter().copied().find(|s| (*s as f64 - v).abs() < 0.01)) {
            s.ui_scale = v;
        }
        if let Some(v) = named(&Palette::ALL, string(&display, "palette").as_deref()) {
            s.palette = v;
        }
        if let Some(v) = string(&menu, "airport") {
            s.airport = v;
        }
        let position = string(&menu, "position");
        if let Some(v) = Staffing::CHOICES.iter().copied().find(|c| Some(staffing_name(*c)) == position) {
            s.staffing = v;
        }
        if let Some(v) = boolean(&menu, "vfr") {
            s.vfr = v;
        }
        let minutes = menu.get("shift_minutes").and_then(Value::as_integer);
        if let Some(v) = minutes.filter(|m| (super::shift::MIN_MINUTES as i64..=super::shift::MAX_MINUTES as i64).contains(m)) {
            s.shift_minutes = v as u32;
        }
        let d = &mut s.difficulty;
        if let Some(v) = named(&Busyness::ALL, string(&difficulty, "traffic").as_deref()) {
            d.busyness = v;
        }
        if let Some(v) = named(&EmergencyRate::ALL, string(&difficulty, "emergencies").as_deref()) {
            d.emergencies = v;
        }
        if let Some(v) = named(&PilotMistakes::ALL, string(&difficulty, "pilot_mistakes").as_deref()) {
            d.mistakes = v;
        }
        if let Some(v) = named(&WeatherKind::ALL, string(&difficulty, "weather").as_deref()) {
            d.weather = v;
        }
        s
    }

    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        let section = |pairs: Vec<(&str, Value)>| {
            Value::Table(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let name = |v: &dyn std::fmt::Debug| Value::String(format!("{v:?}"));
        table.insert(
            "sound".into(),
            section(vec![("volume", name(&self.volume)), ("game_cues", Value::Boolean(self.game_cues))]),
        );
        table.insert(
            "display".into(),
            section(vec![("ui_scale", Value::Float(self.ui_scale as f64)), ("palette", name(&self.palette))]),
        );
        table.insert(
            "menu".into(),
            section(vec![
                ("airport", Value::String(self.airport.clone())),
                ("position", Value::String(staffing_name(self.staffing))),
                ("vfr", Value::Boolean(self.vfr)),
                ("shift_minutes", Value::Integer(self.shift_minutes as i64)),
            ]),
        );
        let d = &self.difficulty;
        table.insert(
            "difficulty".into(),
            section(vec![
                ("traffic", name(&d.busyness)),
                ("emergencies", name(&d.emergencies)),
                ("pilot_mistakes", name(&d.mistakes)),
                ("weather", name(&d.weather)),
            ]),
        );
        let body = toml::to_string(&table).unwrap_or_default();
        format!("# Air Traffic Control settings. Delete this file to go back to the defaults.\n\n{body}")
    }

    /// Write the settings out. Failing to is no reason to stop the game.
    pub fn save(&self) {
        let Some(path) = Settings::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Written aside and moved into place, so a crash can't leave half a file.
        let temp = path.with_extension("toml.tmp");
        if std::fs::write(&temp, self.to_toml()).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let s = Settings {
            volume: Volume::High,
            game_cues: false,
            ui_scale: 1.25,
            palette: Palette::RedGreen,
            airport: "KSUS".into(),
            staffing: Staffing::LOCAL_GROUND,
            vfr: false,
            shift_minutes: 45,
            difficulty: Difficulty {
                busyness: Busyness::Busy,
                emergencies: EmergencyRate::Frequent,
                mistakes: PilotMistakes::Realistic,
                weather: WeatherKind::Storms,
            },
        };
        let text = s.to_toml();
        assert!(text.contains("position = \"Local + Ground\""), "{text}");
        assert_eq!(Settings::parse(&text), s);
    }

    #[test]
    fn anything_unreadable_falls_back_to_the_default() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("not = [toml"), Settings::default());
        let s = Settings::parse(
            "[sound]\nvolume = \"Deafening\"\ngame_cues = false\n[display]\nui_scale = 7.0\npalette = 3\n[menu]\nshift_minutes = 0\n",
        );
        assert_eq!(s.volume, Volume::default());
        assert!(!s.game_cues, "the good value is kept");
        assert_eq!(s.ui_scale, 1.0);
        assert_eq!(s.palette, Palette::Standard);
        assert_eq!(s.shift_minutes, super::super::shift::DEFAULT_MINUTES);
    }
}
