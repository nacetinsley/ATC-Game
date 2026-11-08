//! Radio voices: transmissions spoken from pre-rendered clips.
//!
//! `tools/make_voice.py` renders a clip for every word and stock phrase in
//! `assets/sounds/voice/words.txt` in four public-domain Piper voices, packed
//! one file per voice: 8 kHz, µ-law, already band-limited the way VHF is. A
//! transmission is the clips of its words (`sim::speech`), stock phrases
//! taken whole where they fit, between a key-up and a squelch tail.
//!
//! One transmission plays at a time, as on a real frequency. Ones that wait
//! too long are dropped, so the voices keep up at x2 and x4.

use super::audio::{Note, SAMPLE_RATE, band_limit, synth, wav};
use crate::sim::speech::{self, PAUSE, STOP};
use crate::sim::{LogEntry, Speaker, Talker};
use macroquad::audio::{PlaySoundParams, Sound, load_sound_from_bytes, play_sound, stop_sound};
use std::collections::{HashMap, VecDeque};

/// The packs, one per voice.
const PACKS: [&[u8]; 4] = [
    include_bytes!("../../assets/sounds/voice/norman.pack"),
    include_bytes!("../../assets/sounds/voice/john.pack"),
    include_bytes!("../../assets/sounds/voice/kristin.pack"),
    include_bytes!("../../assets/sounds/voice/ljspeech.pack"),
];
const CONTROLLER: usize = 0;
const PILOTS: [usize; 3] = [1, 2, 3];
/// Pitch shifts that make three pilot voices into nine.
const PILOT_PITCHES: [f32; 3] = [0.94, 1.0, 1.06];
/// The other controller on a landline.
const FACILITY: (usize, f32) = (1, 0.9);
/// The longest stock phrase, in words.
const LONGEST_PHRASE: usize = 6;
/// Silence between words, after a comma, and after a full stop, seconds.
const WORD_GAP_S: f32 = 0.035;
const PAUSE_S: f32 = 0.16;
const STOP_S: f32 = 0.3;
/// A transmission still waiting after this long is dropped, real seconds.
const STALE_S: f64 = 6.0;
/// Quiet between transmissions, real seconds.
const BETWEEN_S: f64 = 0.35;

/// One voice's clips: word or phrase to its µ-law samples.
pub struct Pack {
    rate: u32,
    clips: HashMap<&'static str, &'static [u8]>,
}

impl Pack {
    /// Read a pack written by `tools/make_voice.py`.
    pub fn read(bytes: &'static [u8]) -> Option<Pack> {
        let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
        if bytes.get(..4)? != b"ATCV" || u32_at(4)? != 1 {
            return None;
        }
        let rate = u32_at(8)?;
        let count = u32_at(12)? as usize;
        let mut at = 16;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
            let key = std::str::from_utf8(bytes.get(at + 2..at + 2 + len)?).ok()?;
            at += 2 + len;
            entries.push((key, u32_at(at)? as usize, u32_at(at + 4)? as usize));
            at += 8;
        }
        let data = bytes.get(at..)?;
        let clips = entries
            .into_iter()
            .map(|(key, offset, len)| Some((key, data.get(offset..offset + len)?)))
            .collect::<Option<HashMap<_, _>>>()?;
        Some(Pack { rate, clips })
    }

    pub fn has(&self, word: &str) -> bool {
        self.clips.contains_key(word)
    }

    pub fn count(&self) -> usize {
        self.clips.len()
    }
}

/// µ-law back to a sample, the continuous form `make_voice.py` writes.
fn from_mu_law(byte: u8) -> f32 {
    let y = byte as f32 / 127.5 - 1.0;
    y.signum() * ((1.0 + 255.0f32).powf(y.abs()) - 1.0) / 255.0
}

/// The clip keys for a line's words: stock phrases where they fit, and a
/// word with no clip spelled out.
pub fn clip_keys(words: &[String], pack: &Pack) -> Vec<String> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let longest = (words.len() - i).min(LONGEST_PHRASE);
        let phrase = (2..=longest).rev().map(|n| words[i..i + n].join(" ")).zip((2..=longest).rev()).find(|(p, _)| pack.has(p));
        if let Some((phrase, n)) = phrase {
            keys.push(phrase);
            i += n;
            continue;
        }
        let word = &words[i];
        if word == PAUSE || word == STOP || pack.has(word) {
            keys.push(word.clone());
        } else {
            keys.extend(speech::spelled(word).into_iter().filter(|w| pack.has(w)));
        }
        i += 1;
    }
    keys
}

/// A whole transmission through a radio: key-up, the words at this pitch,
/// a squelch tail, all band-limited.
fn transmission(keys: &[String], pack: &Pack, pitch: f32) -> Vec<i16> {
    let gap = |secs: f32| vec![0.0f32; (secs * SAMPLE_RATE as f32) as usize];
    let mut speech: Vec<f32> = gap(0.06);
    let step = pack.rate as f32 / SAMPLE_RATE as f32 * pitch;
    for key in keys {
        match key.as_str() {
            PAUSE => speech.extend(gap(PAUSE_S)),
            STOP => speech.extend(gap(STOP_S)),
            _ => {
                let Some(clip) = pack.clips.get(key.as_str()) else { continue };
                let samples: Vec<f32> = clip.iter().map(|&b| from_mu_law(b)).collect();
                let mut t = 0.0f32;
                while (t as usize) + 1 < samples.len() {
                    let j = t as usize;
                    let frac = t - j as f32;
                    speech.push(samples[j] + (samples[j + 1] - samples[j]) * frac);
                    t += step;
                }
                speech.extend(gap(WORD_GAP_S));
            }
        }
    }
    speech.extend(gap(0.05));
    // A touch of drive, as a transmitter has, then the carrier's hiss under it.
    let level = 0.55 * i16::MAX as f32;
    let hiss = synth(&[Note::noise(speech.len() as f32 / SAMPLE_RATE as f32, 0.025)], false);
    let mut out: Vec<i16> = synth(&[Note::noise(0.008, 0.45)], true);
    out.extend(speech.iter().zip(hiss.iter().chain(std::iter::repeat(&0))).map(|(s, h)| {
        let driven = (s * 1.6).tanh() * level;
        (driven + *h as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }));
    out.extend(synth(&[Note::noise(0.08, 0.2).modulated(14.0, 0.6)], true));
    band_limit(&out)
}

/// Which voice says a line, and at what pitch.
fn voice_for(talker: &Talker) -> (usize, f32) {
    match talker {
        Talker::Controller => (CONTROLLER, 1.0),
        Talker::Facility(_) => FACILITY,
        Talker::Pilot(callsign) => {
            // FNV-1a: the same callsign always sounds the same.
            let hash = callsign.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
            let n = hash as usize % (PILOTS.len() * PILOT_PITCHES.len());
            (PILOTS[n % PILOTS.len()], PILOT_PITCHES[n / PILOTS.len()])
        }
    }
}

/// The text a voice says: a landline without its "Approach to Tower:"
/// heading.
pub fn speech_text(speaker: Speaker, text: &str) -> &str {
    match (speaker, text.split_once(": ")) {
        (Speaker::Landline, Some((_, rest))) => rest,
        _ => text,
    }
}

struct Waiting {
    since: f64,
    words: Vec<String>,
    voice: (usize, f32),
}

pub struct Voices {
    packs: Vec<Pack>,
    queue: VecDeque<Waiting>,
    /// The transmission on the air, and when it ends.
    playing: Option<(Sound, f64)>,
    pub enabled: bool,
}

impl Voices {
    pub fn load() -> Voices {
        let packs = PACKS.iter().filter_map(|b| Pack::read(b)).collect::<Vec<_>>();
        let packs = if packs.len() == PACKS.len() { packs } else { Vec::new() };
        Voices { packs, queue: VecDeque::new(), playing: None, enabled: true }
    }

    /// Queue a line from the log, if it's a transmission the player hears.
    /// True if a voice will say it.
    pub fn hear(&mut self, line: &LogEntry, callsigns: &[&str], now: f64) -> bool {
        let Some(radio) = line.radio.as_ref().filter(|r| r.heard) else { return false };
        if !self.enabled || self.packs.is_empty() {
            return false;
        }
        let words = speech::spoken(speech_text(line.speaker, &line.text), callsigns);
        if words.is_empty() {
            return false;
        }
        self.queue.push_back(Waiting { since: now, words, voice: voice_for(&radio.talker) });
        true
    }

    /// Put the next transmission on the air once the frequency is free.
    pub fn update(&mut self, now: f64, volume: f32) {
        if self.playing.as_ref().is_some_and(|(_, end)| now < *end) {
            return;
        }
        self.playing = None;
        while let Some(next) = self.queue.pop_front() {
            if now - next.since > STALE_S || volume <= 0.0 {
                continue;
            }
            let pack = &self.packs[next.voice.0];
            let samples = transmission(&clip_keys(&next.words, pack), pack, next.voice.1);
            let Some(sound) = sound_now(&wav(&samples)) else { continue };
            play_sound(&sound, PlaySoundParams { looped: false, volume });
            let end = now + samples.len() as f64 / SAMPLE_RATE as f64 + BETWEEN_S;
            self.playing = Some((sound, end));
            break;
        }
    }

    /// Stop talking and forget what was waiting.
    pub fn silence(&mut self) {
        if let Some((sound, _)) = self.playing.take() {
            stop_sound(&sound);
        }
        self.queue.clear();
    }
}

/// Make a sound now. Loading from memory doesn't wait on anything on the
/// desktop, so the future is ready the first time it's polled.
fn sound_now(bytes: &[u8]) -> Option<Sound> {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let mut future = std::pin::pin!(load_sound_from_bytes(bytes));
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(sound) => sound.ok(),
        Poll::Pending => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packs() -> Vec<Pack> {
        PACKS.iter().map(|b| Pack::read(b).expect("a pack")).collect()
    }

    #[test]
    fn every_pack_has_every_word() {
        let words: Vec<&str> =
            include_str!("../../assets/sounds/voice/words.txt").lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        for pack in packs() {
            assert_eq!(pack.count(), words.len());
            for w in &words {
                assert!(pack.has(w), "no clip for {w:?}");
                assert!(!pack.clips[w].is_empty(), "{w:?} is empty");
            }
        }
    }

    #[test]
    fn stock_phrases_are_used_whole() {
        let pack = &packs()[CONTROLLER];
        let words = speech::spoken("NVA101, runway 27L, cleared to land.", &[]);
        let keys = clip_keys(&words, pack);
        assert!(keys.contains(&"cleared to land".to_string()), "{keys:?}");
        assert_eq!(keys[..4], ["nova", "one", "zero", "one"]);
    }

    #[test]
    fn a_word_with_no_clip_is_spelled() {
        let pack = &packs()[CONTROLLER];
        let keys = clip_keys(&["zzqx".to_string()], pack);
        assert_eq!(keys, ["zulu", "zulu", "quebec", "x-ray"]);
    }

    #[test]
    fn a_transmission_is_sound() {
        let pack = &packs()[1];
        let keys = clip_keys(&speech::spoken("Tower, NVA101, holding short runway 27R.", &[]), pack);
        let samples = transmission(&keys, pack, 1.06);
        assert!(samples.len() > SAMPLE_RATE as usize, "at least a second: {}", samples.len());
        assert!(samples.iter().any(|s| s.unsigned_abs() > 3000), "loud enough");
    }

    #[test]
    fn each_aircraft_keeps_its_voice() {
        let a = voice_for(&Talker::Pilot("NVA101".into()));
        assert_eq!(a, voice_for(&Talker::Pilot("NVA101".into())));
        let voices: std::collections::HashSet<(usize, u32)> = (0..60)
            .map(|n| voice_for(&Talker::Pilot(format!("TRL{n}"))))
            .map(|(v, p)| (v, (p * 100.0) as u32))
            .collect();
        assert!(voices.len() >= 6, "a spread of voices: {voices:?}");
        assert!(voices.iter().all(|(v, _)| *v != CONTROLLER));
    }
}
