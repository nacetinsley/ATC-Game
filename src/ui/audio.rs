//! Sound: the equipment a real TRACON or tower controller hears, and a few
//! game cues no real facility makes.
//!
//! Equipment sounds follow the real systems' documented behaviour:
//! - **STARS Conflict Alert:** an aural alarm, continuous until the controller
//!   acknowledges it by slewing either track (here, selecting either
//!   aircraft). STARS has no separate loss-of-separation alarm; the CA keeps
//!   sounding.
//! - **STARS MSAW:** its own tone, modulated differently from the CA. The
//!   aural alarm stops on its own while the visual alert keeps flashing.
//! - **STARS special codes** (7700, 7600): a tone until the track is slewed.
//! - **ASDE-X:** a spoken alert, "Warning, runway two seven left, arrival,
//!   runway occupied".
//! - **VHF radio:** a key-up, the band-limited hiss of the carrier and a
//!   squelch tail for each pilot transmission. Aviation has no roger beep.
//!
//! The STARS tones' exact frequencies and waveforms aren't published, so
//! those are conservative alarm tones in the 600-1,000 Hz range that differ
//! the way the real ones are documented to. Handoffs make no sound in STARS;
//! the handoff ding is a game cue, and the game cues can be switched off.
//!
//! Tones are synthesised at startup. The ASDE-X words are clips rendered by
//! `tools/make_voice.py` with Piper, so every platform plays the same clips.
//! The radio voices (`voice.rs`) are rendered the same way.

use super::input::UiState;
use super::voice::Voices;
use crate::sim::aircraft::{Aircraft, Phase};
use crate::sim::clearance::spoken_runway;
use crate::sim::scoring::SimEvent;
use crate::sim::{Speaker, Talker, World};
use macroquad::audio::{
    PlaySoundParams, Sound, load_sound_from_bytes, play_sound, set_sound_volume, stop_sound,
};
use std::collections::{HashMap, HashSet};

pub const SAMPLE_RATE: u32 = 22050;
/// More new log lines than this in one frame is a jump, not radio traffic.
const MAX_LINES_SAID: usize = 6;
/// How long the MSAW aural alarm sounds before it stops by itself, seconds.
const MSAW_AURAL_S: f64 = 10.0;
/// ASDE-X repeats its alert this often while the hazard lasts, seconds.
const ASDEX_REPEAT_S: f64 = 5.0;
/// An arrival this close with the runway occupied sets off ASDE-X, NM.
const ASDEX_ARRIVAL_NM: f32 = super::surface::ARRIVAL_ALERT_NM;
/// Closest that one-shot cues may follow each other, seconds.
const ONE_SHOT_SPACING_S: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wave {
    Sine,
    /// Deterministic white noise.
    Noise,
}

/// One segment of a synthesised sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// Hz; ignored for noise. Zero with `Sine` is silence.
    pub freq: f32,
    pub secs: f32,
    pub wave: Wave,
    /// 0 to 1.
    pub gain: f32,
    /// Amplitude modulation: rate in Hz and depth 0 to 1.
    pub am: Option<(f32, f32)>,
}

impl Note {
    pub const fn tone(freq: f32, secs: f32, gain: f32) -> Note {
        Note {
            freq,
            secs,
            wave: Wave::Sine,
            gain,
            am: None,
        }
    }
    pub const fn rest(secs: f32) -> Note {
        Note {
            freq: 0.0,
            secs,
            wave: Wave::Sine,
            gain: 0.0,
            am: None,
        }
    }
    pub const fn noise(secs: f32, gain: f32) -> Note {
        Note {
            freq: 0.0,
            secs,
            wave: Wave::Noise,
            gain,
            am: None,
        }
    }
    pub const fn modulated(self, rate: f32, depth: f32) -> Note {
        Note {
            am: Some((rate, depth)),
            ..self
        }
    }
}

/// Samples for a run of notes. `fades` softens each note's ends so it
/// doesn't click; a loop leaves them off so it joins up seamlessly.
pub fn synth(notes: &[Note], fades: bool) -> Vec<i16> {
    let mut samples = Vec::new();
    let mut seed: u32 = 0x1234_5678;
    for note in notes {
        let count = (note.secs * SAMPLE_RATE as f32) as usize;
        for n in 0..count {
            let t = n as f32 / SAMPLE_RATE as f32;
            let raw = match note.wave {
                Wave::Sine if note.freq == 0.0 => 0.0,
                Wave::Sine => (t * note.freq * std::f32::consts::TAU).sin(),
                Wave::Noise => {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
                }
            };
            let am = note.am.map_or(1.0, |(rate, depth)| {
                1.0 - depth * 0.5 * (1.0 - (t * rate * std::f32::consts::TAU).cos())
            });
            let fade = if fades {
                (n.min(count - n) as f32 / 200.0).min(1.0)
            } else {
                1.0
            };
            samples.push((raw * am * fade * note.gain * i16::MAX as f32) as i16);
        }
    }
    samples
}

/// A telephone-speaker band, about 300 to 3,400 Hz: a one-pole high-pass then
/// a one-pole low-pass.
pub fn band_limit(samples: &[i16]) -> Vec<i16> {
    let dt = 1.0 / SAMPLE_RATE as f32;
    let rc_high = 1.0 / (std::f32::consts::TAU * 300.0);
    let rc_low = 1.0 / (std::f32::consts::TAU * 3400.0);
    let a_high = rc_high / (rc_high + dt);
    let a_low = dt / (rc_low + dt);
    let (mut prev_in, mut prev_high, mut low) = (0.0f32, 0.0f32, 0.0f32);
    samples
        .iter()
        .map(|&s| {
            let x = s as f32;
            let high = a_high * (prev_high + x - prev_in);
            prev_in = x;
            prev_high = high;
            low += a_low * (high - low);
            low.clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

/// A 16-bit mono WAV of the samples.
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// The 16-bit samples of a mono WAV clip, if it is one.
fn read_wav(bytes: &[u8]) -> Option<Vec<i16>> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8 + len).min(bytes.len()))?;
        if id == b"data" {
            return Some(
                body.chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect(),
            );
        }
        at += 8 + len + (len & 1);
    }
    None
}

/// The clip without the silence the synthesizer leaves around a word.
fn trim(samples: &[i16]) -> &[i16] {
    let loud = |s: &i16| s.unsigned_abs() > 600;
    let start = samples
        .iter()
        .position(loud)
        .unwrap_or(0)
        .saturating_sub(200);
    let end = samples
        .iter()
        .rposition(loud)
        .map_or(samples.len(), |e| (e + 200).min(samples.len()));
    &samples[start..end.max(start)]
}

macro_rules! clip {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!("../../assets/sounds/voice/", $name, ".wav")).as_slice(),
        )
    };
}

const VOICE: [(&str, &[u8]); 18] = [
    clip!("warning"),
    clip!("runway"),
    clip!("zero"),
    clip!("one"),
    clip!("two"),
    clip!("three"),
    clip!("four"),
    clip!("five"),
    clip!("six"),
    clip!("seven"),
    clip!("eight"),
    clip!("niner"),
    clip!("left"),
    clip!("right"),
    clip!("center"),
    clip!("arrival"),
    clip!("departure"),
    clip!("occupied"),
];

/// The words of an ASDE-X alert, with `None` for a pause: "Warning, runway
/// two seven left, arrival, runway occupied".
pub fn asdex_words(runway: &str, departure: bool) -> Vec<Option<&'static str>> {
    const DIGITS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "niner",
    ];
    let mut words = vec![Some("warning"), None, Some("runway")];
    for ch in runway.chars() {
        words.push(match ch {
            '0'..='9' => Some(DIGITS[ch as usize - '0' as usize]),
            'L' => Some("left"),
            'R' => Some("right"),
            'C' => Some("center"),
            _ => continue,
        });
    }
    words.push(None);
    words.push(Some(if departure { "departure" } else { "arrival" }));
    words.push(None);
    words.push(Some("runway"));
    words.push(Some("occupied"));
    words
}

/// The samples of a whole ASDE-X alert, through a speaker.
fn asdex_samples(clips: &HashMap<&str, Vec<i16>>, runway: &str, departure: bool) -> Vec<i16> {
    let gap = |secs: f32| vec![0i16; (secs * SAMPLE_RATE as f32) as usize];
    let mut out = Vec::new();
    for word in asdex_words(runway, departure) {
        match word.and_then(|w| clips.get(w)) {
            Some(clip) => {
                out.extend_from_slice(trim(clip));
                out.extend(gap(0.03));
            }
            None => out.extend(gap(0.18)),
        }
    }
    band_limit(&out)
}

/// The STARS alarms, one sounding at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alarm {
    ConflictAlert,
    Msaw,
    SpecialCode,
}

impl Alarm {
    fn name(self) -> &'static str {
        match self {
            Alarm::ConflictAlert => "conflict alert",
            Alarm::Msaw => "msaw",
            Alarm::SpecialCode => "special code",
        }
    }
}

/// What the alarms see this frame.
#[derive(Debug, Default)]
pub struct AlarmInput {
    /// Conflict and separation pairs involving the player's traffic.
    pub conflicts: Vec<(u32, u32)>,
    /// The player's aircraft below the MVA.
    pub low: Vec<u32>,
    /// Emergencies squawking that the player hasn't selected yet, anyone's.
    pub special: Vec<u32>,
    pub selected: Option<u32>,
    /// Real seconds.
    pub now: f64,
}

/// Which alarms have been acknowledged, and since when MSAW has sounded.
#[derive(Debug, Default)]
pub struct AlarmState {
    acknowledged_pairs: HashSet<(u32, u32)>,
    low_since: HashMap<u32, f64>,
    acknowledged_low: HashSet<u32>,
}

impl AlarmState {
    /// The alarm that should be sounding, highest priority first.
    pub fn update(&mut self, input: &AlarmInput) -> Option<Alarm> {
        // A pair that clears and comes back sounds again.
        self.acknowledged_pairs
            .retain(|p| input.conflicts.contains(p));
        if let Some(id) = input.selected {
            for &p in &input.conflicts {
                if p.0 == id || p.1 == id {
                    self.acknowledged_pairs.insert(p);
                }
            }
        }
        let conflict = input
            .conflicts
            .iter()
            .any(|p| !self.acknowledged_pairs.contains(p));

        self.low_since.retain(|id, _| input.low.contains(id));
        self.acknowledged_low.retain(|id| input.low.contains(id));
        for &id in &input.low {
            self.low_since.entry(id).or_insert(input.now);
            if input.selected == Some(id) {
                self.acknowledged_low.insert(id);
            }
        }
        let low = input.low.iter().any(|id| {
            !self.acknowledged_low.contains(id) && input.now - self.low_since[id] < MSAW_AURAL_S
        });

        if conflict {
            Some(Alarm::ConflictAlert)
        } else if low {
            Some(Alarm::Msaw)
        } else if !input.special.is_empty() {
            Some(Alarm::SpecialCode)
        } else {
            None
        }
    }
}

/// One-shot sounds, most important first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cue {
    Attention,
    Handoff,
    Done,
    Radio,
}

impl Cue {
    /// A sound no real facility makes.
    pub fn game_only(self) -> bool {
        !matches!(self, Cue::Radio)
    }
    fn name(self) -> &'static str {
        match self {
            Cue::Attention => "attention",
            Cue::Handoff => "handoff ding",
            Cue::Done => "done",
            Cue::Radio => "radio",
        }
    }
}

/// Sound volume, from the pause menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Volume {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Volume {
    pub const ALL: [Volume; 4] = [Volume::Off, Volume::Low, Volume::Medium, Volume::High];

    pub fn previous(self) -> Volume {
        match self {
            Volume::Off => Volume::High,
            Volume::Low => Volume::Off,
            Volume::Medium => Volume::Low,
            Volume::High => Volume::Medium,
        }
    }

    pub fn next(self) -> Volume {
        match self {
            Volume::Off => Volume::Low,
            Volume::Low => Volume::Medium,
            Volume::Medium => Volume::High,
            Volume::High => Volume::Off,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Volume::Off => "Off",
            Volume::Low => "Low",
            Volume::Medium => "Medium",
            Volume::High => "High",
        }
    }
    fn level(self) -> f32 {
        match self {
            Volume::Off => 0.0,
            Volume::Low => 0.35,
            Volume::Medium => 0.65,
            Volume::High => 1.0,
        }
    }
}

/// Sounds are optional: a machine with no audio device still gets a playable game.
pub struct Sounds {
    alarms: HashMap<Alarm, Sound>,
    cues: HashMap<Cue, Sound>,
    /// ASDE-X alerts by runway and "departure", built at load.
    asdex: HashMap<(String, bool), Sound>,
    pub volume: Volume,
    pub game_cues: bool,
    playing: Option<Alarm>,
    playing_volume: f32,
    state: AlarmState,
    last_cue: f64,
    said: u64,
    urgent: HashSet<u32>,
    /// Callsigns last seen on the player's frequencies, for exits.
    owned: HashSet<String>,
    /// When each ASDE-X hazard may sound again.
    asdex_next: HashMap<(String, bool), f64>,
    /// Pilots and controllers speaking on the radio and the landline.
    pub voices: Voices,
    debug: bool,
}

impl Sounds {
    /// Load every sound, including the ASDE-X alerts for each airport's runways.
    pub async fn load(runways: &[String]) -> Sounds {
        async fn make(samples: Vec<i16>) -> Option<Sound> {
            load_sound_from_bytes(&wav(&samples)).await.ok()
        }
        let mut alarms = HashMap::new();
        // Conflict Alert: a steady tone pulsed about twice a second.
        let ca = synth(&[Note::tone(960.0, 1.0, 0.32).modulated(2.0, 1.0)], false);
        // MSAW: the same family warbled faster and lower, so it's told apart by modulation.
        let msaw = synth(&[Note::tone(720.0, 1.0, 0.32).modulated(6.0, 0.8)], false);
        // Special code: a two-note chime, repeating.
        let spc = synth(
            &[
                Note::tone(840.0, 0.35, 0.30),
                Note::tone(630.0, 0.35, 0.30),
                Note::rest(0.5),
            ],
            true,
        );
        for (alarm, samples) in [
            (Alarm::ConflictAlert, ca),
            (Alarm::Msaw, msaw),
            (Alarm::SpecialCode, spc),
        ] {
            if let Some(s) = make(samples).await {
                alarms.insert(alarm, s);
            }
        }

        let mut cues = HashMap::new();
        // Radio: key-up click, hiss under the call, squelch tail.
        let radio = band_limit(&synth(
            &[
                Note::noise(0.006, 0.5),
                Note::noise(0.22, 0.05),
                Note::noise(0.07, 0.22).modulated(14.0, 0.6),
            ],
            true,
        ));
        let ding = synth(&[Note::tone(1047.0, 0.18, 0.22)], true);
        let attention = synth(
            &[
                Note::tone(1500.0, 0.03, 0.18),
                Note::rest(0.07),
                Note::tone(1500.0, 0.03, 0.18),
            ],
            true,
        );
        let done = synth(
            &[Note::tone(660.0, 0.08, 0.2), Note::tone(990.0, 0.12, 0.2)],
            true,
        );
        for (cue, samples) in [
            (Cue::Radio, radio),
            (Cue::Handoff, ding),
            (Cue::Attention, attention),
            (Cue::Done, done),
        ] {
            if let Some(s) = make(samples).await {
                cues.insert(cue, s);
            }
        }

        let clips: HashMap<&str, Vec<i16>> = VOICE
            .iter()
            .filter_map(|(name, bytes)| read_wav(bytes).map(|s| (*name, s)))
            .collect();
        let mut asdex = HashMap::new();
        for runway in runways {
            for departure in [false, true] {
                if let Some(s) = make(asdex_samples(&clips, runway, departure)).await {
                    asdex.insert((runway.clone(), departure), s);
                }
            }
        }

        Sounds {
            alarms,
            cues,
            asdex,
            volume: Volume::default(),
            game_cues: true,
            playing: None,
            playing_volume: 0.0,
            state: AlarmState::default(),
            last_cue: f64::NEG_INFINITY,
            said: 0,
            urgent: HashSet::new(),
            owned: HashSet::new(),
            asdex_next: HashMap::new(),
            voices: Voices::load(),
            debug: std::env::var_os("ATC_SOUND_LOG").is_some(),
        }
    }

    fn log(&self, what: &str) {
        if self.debug {
            eprintln!("sound: {what}");
        }
    }

    /// Stop everything: paused, or not playing.
    pub fn silence(&mut self) {
        self.voices.silence();
        if let Some(alarm) = self.playing.take()
            && let Some(s) = self.alarms.get(&alarm)
        {
            stop_sound(s);
            self.log(&format!("{} stops", alarm.name()));
        }
    }

    /// Start of a shift: forget the last one.
    pub fn reset(&mut self, world: &World) {
        self.silence();
        self.state = AlarmState::default();
        self.said = world.said;
        self.urgent.clear();
        self.owned.clear();
        self.asdex_next.clear();
    }

    fn set_alarm(&mut self, alarm: Option<Alarm>) {
        let level = self.volume.level();
        if self.playing == alarm {
            if let Some(s) = alarm.and_then(|a| self.alarms.get(&a))
                && self.playing_volume != level
            {
                set_sound_volume(s, level);
                self.playing_volume = level;
            }
            return;
        }
        self.silence();
        if let Some(a) = alarm
            && let Some(s) = self.alarms.get(&a)
        {
            play_sound(
                s,
                PlaySoundParams {
                    looped: true,
                    volume: level,
                },
            );
            self.playing = alarm;
            self.playing_volume = level;
            self.log(&format!("{} starts", a.name()));
        }
    }

    fn one_shot(&mut self, cues: &mut Vec<Cue>, now: f64) {
        if !self.game_cues {
            cues.retain(|c| !c.game_only());
        }
        cues.sort();
        let Some(&cue) = cues.first() else { return };
        if now - self.last_cue < ONE_SHOT_SPACING_S || self.volume == Volume::Off {
            return;
        }
        if let Some(s) = self.cues.get(&cue) {
            play_sound(
                s,
                PlaySoundParams {
                    looped: false,
                    volume: self.volume.level(),
                },
            );
            self.last_cue = now;
            self.log(cue.name());
        }
    }

    fn asdex(&mut self, runway: String, departure: bool, now: f64) {
        let key = (runway, departure);
        if self.asdex_next.get(&key).is_some_and(|&t| now < t) || self.volume == Volume::Off {
            return;
        }
        if let Some(s) = self.asdex.get(&key) {
            play_sound(
                s,
                PlaySoundParams {
                    looped: false,
                    volume: self.volume.level(),
                },
            );
            self.log(&format!(
                "asdex {} {}",
                key.0,
                if departure { "departure" } else { "arrival" }
            ));
        }
        self.asdex_next.insert(key, now + ASDEX_REPEAT_S);
    }

    /// Bring the sound in line with the world after a frame.
    pub fn update(&mut self, world: &World, ui: &UiState, events: &[SimEvent]) {
        let now = macroquad::time::get_time();
        if ui.paused || self.volume == Volume::Off {
            self.silence();
            self.said = world.said;
            return;
        }
        let mine = |id: u32| world.find(id).is_some_and(|a| world.is_seats(ui.seat, a));

        // STARS alarms.
        let alerts = &world.alerts;
        let input = AlarmInput {
            conflicts: alerts
                .conflict
                .iter()
                .chain(alerts.separation.iter())
                .copied()
                .filter(|&(a, b)| mine(a) || mine(b))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect(),
            low: alerts
                .terrain
                .iter()
                .copied()
                .filter(|&id| mine(id))
                .collect(),
            special: world
                .aircraft
                .iter()
                // A 7700 or 7600 squawk alerts every display, whoever is working it.
                .filter(|a| a.in_emergency() && !ui.acknowledged.contains(&a.id))
                .map(|a| a.id)
                .collect(),
            selected: ui.selected,
            now,
        };
        let alarm = self.state.update(&input);
        self.set_alarm(alarm);

        // ASDE-X, in the tower.
        if ui.staffing.tower() {
            for (runway, departure) in asdex_hazards(world, events, ui.selected) {
                self.asdex(runway, departure, now);
            }
        }

        // One-shot cues.
        let mut cues = Vec::new();
        let new_lines =
            (world.said - self.said.min(world.said)).min(world.log.len() as u64) as usize;
        self.said = world.said;
        let callsigns: Vec<&str> = world.aircraft.iter().map(|a| a.callsign.as_str()).collect();
        let mut radio_cue = false;
        // A jump (a replay seek, a takeover) brings many lines at once: they
        // aren't said, as no one hears them as they happened.
        let jumped = new_lines > MAX_LINES_SAID;
        self.voices.seat = ui.seat;
        for line in &world.log[world.log.len() - new_lines..] {
            // A pilot sounds further off the further the aircraft is.
            let range_nm = line.radio.as_ref().and_then(|r| match &r.talker {
                Talker::Pilot(c) => world.aircraft.iter().find(|a| &a.callsign == c).map(|a| a.pos.length()),
                _ => None,
            });
            let voiced = !jumped && self.voices.hear(line, &callsigns, range_nm, now);
            if voiced {
                self.log(&format!("voice: {}", line.text));
            }
            // A voiced transmission makes its own key-up and squelch. With
            // voices on, a pilot on a frequency the player doesn't work isn't
            // heard at all.
            let heard = !self.voices.enabled || line.radio.as_ref().is_none_or(|r| r.heard_by.has(ui.seat));
            radio_cue |= !voiced && heard && matches!(line.speaker, Speaker::Pilot | Speaker::Emergency);
        }
        if radio_cue {
            cues.push(Cue::Radio);
        }
        self.voices.update(now, self.volume.level());
        for event in events {
            match event {
                // Only the player who has to act on it hears the ding.
                SimEvent::HandoffOffered { .. } if ui.staffing.approach => cues.push(Cue::Handoff),
                SimEvent::CoordinationRequest { callsign }
                    if world
                        .incoming_for(ui.seat)
                        .any(|c| world.find(c.aircraft).is_some_and(|a| &a.callsign == callsign)) =>
                {
                    cues.push(Cue::Handoff)
                }
                SimEvent::Landed { callsign, .. }
                | SimEvent::Parked { callsign }
                | SimEvent::Exited { callsign }
                    if self.owned.contains(callsign) =>
                {
                    cues.push(Cue::Done)
                }
                _ => {}
            }
        }
        let urgent: HashSet<u32> = world
            .aircraft
            .iter()
            .filter(|a| world.awaiting_for(ui.seat, a).is_some_and(|w| w.urgent()))
            .map(|a| a.id)
            .collect();
        if urgent.iter().any(|id| !self.urgent.contains(id)) {
            cues.push(Cue::Attention);
        }
        self.urgent = urgent;
        self.owned = world
            .aircraft
            .iter()
            .filter(|a| world.is_seats(ui.seat, a))
            .map(|a| a.callsign.clone())
            .collect();
        self.one_shot(&mut cues, now);
    }
}

/// The runways ASDE-X is warning about now, and whether the traffic at risk
/// is a departure (otherwise an arrival). An aircraft the controller has
/// selected doesn't repeat its warning.
fn asdex_hazards(world: &World, events: &[SimEvent], selected: Option<u32>) -> Vec<(String, bool)> {
    let Some(map) = world.airport.surface.as_ref() else {
        return Vec::new();
    };
    let in_use = world.ends_in_use();
    let named = |physical: usize| spoken_runway(&world.airport, map, physical, &in_use);
    let find = |callsign: &str| world.aircraft.iter().find(|a| a.callsign == callsign);
    let rolling_on = |physical: usize| {
        world.aircraft.iter().any(|a| {
            a.on_ground
                && a.phase == Phase::TowerRelease
                && a.departure_end
                    .is_some_and(|e| map.runway_of_end[e] == physical)
        })
    };
    // The runway an aircraft on the ground is on or entering.
    let entering = |ac: &Aircraft| -> Option<usize> {
        let taxi = ac.taxi.as_ref()?;
        map.nodes[taxi.node].zones.first().copied().or_else(|| {
            taxi.next()
                .and_then(|n| map.nodes[n].zones.first().copied())
        })
    };

    let mut out = Vec::new();
    for event in events {
        match event {
            SimEvent::RunwayIncursion { callsign } => {
                if let Some(physical) = find(callsign).and_then(entering) {
                    out.push((named(physical), rolling_on(physical)));
                }
            }
            // A pilot onto a runway, or rolling, without a clearance, with
            // traffic in the way.
            SimEvent::PilotDeviation { callsign, .. } => {
                use crate::sim::mistakes::SlipKind;
                let Some(ac) = find(callsign) else { continue };
                match ac.slip.as_ref().map(|s| &s.kind) {
                    Some(SlipKind::Incursion { runway, conflict: true, .. }) => {
                        out.push((named(*runway), rolling_on(*runway)));
                    }
                    Some(SlipKind::EarlyRoll { conflict: true, .. }) => {
                        if let Some(e) = ac.departure_end {
                            out.push((named(map.runway_of_end[e]), true));
                        }
                    }
                    _ => {}
                }
            }
            SimEvent::RunwaySeparation { callsign } => {
                if let Some(e) = find(callsign).and_then(|a| a.departure_end) {
                    out.push((named(map.runway_of_end[e]), true));
                }
            }
            _ => {}
        }
    }
    // An arrival about to land on an occupied runway.
    for ac in &world.aircraft {
        if ac.phase != Phase::TowerHandedOff || selected == Some(ac.id) {
            continue;
        }
        let Some(end) = ac.approach_runway else {
            continue;
        };
        let physical = map.runway_of_end[end];
        if world.airport.runways[end].along(ac.pos) < ASDEX_ARRIVAL_NM
            && world.runway_occupied(physical, ac.id)
        {
            out.push((named(physical), false));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_describes_the_samples() {
        let samples = synth(&[Note::tone(440.0, 0.1, 0.3)], true);
        let wav = wav(&samples);
        let count = (0.1 * SAMPLE_RATE as f32) as usize;
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + count * 2);
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len as usize, count * 2);
        assert_eq!(read_wav(&wav).unwrap(), samples);
    }

    #[test]
    fn noise_is_the_same_every_time() {
        let a = synth(&[Note::noise(0.05, 0.5)], true);
        assert_eq!(a, synth(&[Note::noise(0.05, 0.5)], true));
        assert!(a.iter().any(|&s| s > 1000) && a.iter().any(|&s| s < -1000));
    }

    #[test]
    fn a_loop_has_no_fade_at_its_ends() {
        let s = synth(&[Note::tone(960.0, 1.0, 0.32).modulated(2.0, 1.0)], false);
        // Whole cycles of tone and pulse: wrapping round is no bigger a step
        // than any two neighbouring samples, so the loop doesn't click.
        let step = s
            .windows(2)
            .map(|w| (w[1] as i32 - w[0] as i32).abs())
            .max()
            .unwrap();
        let wrap = (s[0] as i32 - s[s.len() - 1] as i32).abs();
        assert!(wrap <= step, "wrap {wrap} vs step {step}");
    }

    #[test]
    fn the_voice_clips_are_readable_mono_wavs() {
        for (name, bytes) in VOICE {
            let samples = read_wav(bytes).unwrap_or_else(|| panic!("{name}"));
            assert!(
                trim(&samples).len() > SAMPLE_RATE as usize / 10,
                "{name} is too short"
            );
        }
    }

    #[test]
    fn asdex_says_the_runway_digit_by_digit() {
        let words: Vec<&str> = asdex_words("27L", false).into_iter().flatten().collect();
        assert_eq!(
            words,
            [
                "warning", "runway", "two", "seven", "left", "arrival", "runway", "occupied"
            ]
        );
        let words: Vec<&str> = asdex_words("09R", true).into_iter().flatten().collect();
        assert_eq!(words[2..5], ["zero", "niner", "right"]);
        assert_eq!(words[5], "departure");
    }

    fn input(
        conflicts: &[(u32, u32)],
        low: &[u32],
        special: &[u32],
        selected: Option<u32>,
        now: f64,
    ) -> AlarmInput {
        AlarmInput {
            conflicts: conflicts.to_vec(),
            low: low.to_vec(),
            special: special.to_vec(),
            selected,
            now,
        }
    }

    #[test]
    fn conflict_alert_sounds_until_either_aircraft_is_selected() {
        let mut s = AlarmState::default();
        assert_eq!(
            s.update(&input(&[(1, 2)], &[], &[], None, 0.0)),
            Some(Alarm::ConflictAlert)
        );
        assert_eq!(
            s.update(&input(&[(1, 2)], &[], &[], None, 5.0)),
            Some(Alarm::ConflictAlert),
            "still going"
        );
        assert_eq!(
            s.update(&input(&[(1, 2)], &[], &[], Some(2), 6.0)),
            None,
            "slewed"
        );
        assert_eq!(
            s.update(&input(&[(1, 2)], &[], &[], None, 7.0)),
            None,
            "stays acknowledged"
        );
        assert_eq!(
            s.update(&input(&[(1, 2), (2, 3)], &[], &[], None, 8.0)),
            Some(Alarm::ConflictAlert),
            "a new pair"
        );
    }

    #[test]
    fn a_conflict_that_clears_and_returns_sounds_again() {
        let mut s = AlarmState::default();
        s.update(&input(&[(1, 2)], &[], &[], Some(1), 0.0));
        assert_eq!(s.update(&input(&[], &[], &[], None, 1.0)), None);
        assert_eq!(
            s.update(&input(&[(1, 2)], &[], &[], None, 2.0)),
            Some(Alarm::ConflictAlert)
        );
    }

    #[test]
    fn msaw_stops_by_itself_or_when_selected() {
        let mut s = AlarmState::default();
        assert_eq!(
            s.update(&input(&[], &[4], &[], None, 0.0)),
            Some(Alarm::Msaw)
        );
        assert_eq!(
            s.update(&input(&[], &[4], &[], None, 9.0)),
            Some(Alarm::Msaw)
        );
        assert_eq!(
            s.update(&input(&[], &[4], &[], None, 10.5)),
            None,
            "the aural alarm times out"
        );
        let mut s = AlarmState::default();
        s.update(&input(&[], &[4], &[], None, 0.0));
        assert_eq!(s.update(&input(&[], &[4], &[], Some(4), 1.0)), None);
    }

    #[test]
    fn alarms_take_turns_by_priority() {
        let mut s = AlarmState::default();
        assert_eq!(
            s.update(&input(&[(1, 2)], &[3], &[5], None, 0.0)),
            Some(Alarm::ConflictAlert)
        );
        assert_eq!(
            s.update(&input(&[(1, 2)], &[3], &[5], Some(1), 1.0)),
            Some(Alarm::Msaw)
        );
        assert_eq!(
            s.update(&input(&[(1, 2)], &[3], &[5], Some(3), 2.0)),
            Some(Alarm::SpecialCode)
        );
        assert_eq!(s.update(&input(&[(1, 2)], &[3], &[], None, 3.0)), None);
    }

    #[test]
    fn only_the_radio_is_real_equipment_among_the_one_shots() {
        assert!(!Cue::Radio.game_only());
        assert!(Cue::Handoff.game_only() && Cue::Attention.game_only() && Cue::Done.game_only());
        assert!(
            Cue::Attention < Cue::Radio,
            "an attention cue wins over a radio click"
        );
    }
}
