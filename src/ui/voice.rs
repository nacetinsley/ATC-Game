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
/// Digits and letters in a group run together with none.
const WORD_GAP_S: f32 = 0.02;
const PAUSE_S: f32 = 0.16;
const STOP_S: f32 = 0.3;
/// Neighbouring words overlap by this much, faded across, seconds.
const CROSSFADE_S: f32 = 0.012;
/// A clip's edges quieter than this share of its loudest moment are
/// trimmed, keeping this much before the first sound and after the last,
/// seconds: enough for a soft consonant's start or release.
const EDGE_QUIET: f32 = 0.03;
const KEEP_LEAD_S: f32 = 0.015;
const KEEP_TAIL_S: f32 = 0.03;
/// A quiet stretch inside a clip longer than this is a pause the voice
/// added between a phrase's words ("contact ... departure"), and is cut to
/// it, seconds. A consonant's own silence, as in "t" or "k", is shorter.
const MAX_INNER_QUIET_S: f32 = 0.1;
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

/// A clip without the quiet a voice leaves at its edges, or the pauses it
/// leaves between a phrase's words.
fn trimmed(samples: &[f32], rate: u32) -> Vec<f32> {
    let frame = (rate / 200).max(1) as usize;
    let rms: Vec<f32> =
        samples.chunks(frame).map(|f| (f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32).sqrt()).collect();
    let loudest = rms.iter().copied().fold(0.0f32, f32::max);
    let loud = |r: f32| r > EDGE_QUIET * loudest;
    let (Some(first), Some(last)) = (rms.iter().position(|&r| loud(r)), rms.iter().rposition(|&r| loud(r))) else {
        return samples.to_vec();
    };
    let start = (first * frame).saturating_sub((KEEP_LEAD_S * rate as f32) as usize);
    let end = ((last + 1) * frame + (KEEP_TAIL_S * rate as f32) as usize).min(samples.len());
    // Inside, keep only the first MAX_INNER_QUIET_S of a long quiet stretch.
    let longest = ((MAX_INNER_QUIET_S * rate as f32) as usize / frame).max(1);
    let mut out = Vec::with_capacity(end - start);
    let mut quiet = 0;
    for (n, chunk) in samples[start..end].chunks(frame).enumerate() {
        let index = start / frame + n;
        quiet = if rms.get(index).is_some_and(|&r| loud(r)) { 0 } else { quiet + 1 };
        if quiet <= longest {
            out.extend_from_slice(chunk);
        }
    }
    out
}

/// Said together with no gap, as a group of digits or letters is:
/// "one zero one", "two seven left", "three thousand".
fn runs_on(before: &str, after: &str) -> bool {
    let spelling = |w: &str| speech::DIGITS.contains(&w) || speech::PHONETIC.contains(&w);
    spelling(before) && (spelling(after) || matches!(after, "thousand" | "hundred" | "left" | "right" | "center"))
}

/// Add a word, overlapping the end of the one before by a short crossfade.
fn join(speech: &mut Vec<f32>, word: &[f32], overlap: usize) {
    let overlap = overlap.min(word.len()).min(speech.len());
    let start = speech.len() - overlap;
    for (i, &w) in word[..overlap].iter().enumerate() {
        let fade = (i as f32 + 0.5) / overlap as f32;
        speech[start + i] = speech[start + i] * (1.0 - fade) + w * fade;
    }
    speech.extend_from_slice(&word[overlap..]);
}

/// A whole transmission through a radio: key-up, the words at this pitch,
/// a squelch tail, all band-limited.
fn transmission(keys: &[String], pack: &Pack, pitch: f32) -> Vec<i16> {
    let gap = |secs: f32| vec![0.0f32; (secs * SAMPLE_RATE as f32) as usize];
    let mut speech: Vec<f32> = gap(0.06);
    let step = pack.rate as f32 / SAMPLE_RATE as f32 * pitch;
    let overlap = (CROSSFADE_S * SAMPLE_RATE as f32) as usize;
    // The word before, if nothing came between: a pause starts afresh.
    let mut previous: Option<&str> = None;
    for key in keys {
        match key.as_str() {
            PAUSE | STOP => {
                speech.extend(gap(if key == PAUSE { PAUSE_S } else { STOP_S }));
                previous = None;
            }
            _ => {
                let Some(clip) = pack.clips.get(key.as_str()) else { continue };
                let decoded: Vec<f32> = clip.iter().map(|&b| from_mu_law(b)).collect();
                let samples = trimmed(&decoded, pack.rate);
                let samples = samples.as_slice();
                let mut word = Vec::with_capacity((samples.len() as f32 / step) as usize + 1);
                let mut t = 0.0f32;
                while (t as usize) + 1 < samples.len() {
                    let j = t as usize;
                    let frac = t - j as f32;
                    word.push(samples[j] + (samples[j + 1] - samples[j]) * frac);
                    t += step;
                }
                match previous {
                    Some(before) if runs_on(before, key) => join(&mut speech, &word, overlap),
                    Some(_) => {
                        speech.extend(gap(WORD_GAP_S));
                        join(&mut speech, &word, 0);
                    }
                    None => join(&mut speech, &word, 0),
                }
                previous = Some(key.as_str());
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

/// One line of the listening set: what it's called, who says it, and how
/// the log writes it.
pub struct Sample {
    pub name: &'static str,
    pub speaker: Speaker,
    pub talker: Talker,
    pub text: &'static str,
}

/// A fixed set of transmissions to listen to before and after a change to
/// the voices (`cargo run --release --example radio_samples`), and to
/// measure in the tests: every voice, the commonest kinds of call, the
/// longest ones, a mayday and a landline.
pub fn samples() -> Vec<Sample> {
    use crate::sim::positions::Position;
    let me = || Talker::Controller;
    let pilot = |c: &str| Talker::Pilot(c.to_string());
    let s = |name, speaker, talker, text| Sample { name, speaker, talker, text };
    vec![
        s("descend", Speaker::Controller, me(), "NVA101, turn left heading 240, descend and maintain 3,000."),
        s("descend-readback", Speaker::Pilot, pilot("NVA101"), "left heading 240, descend and maintain 3,000, NVA101."),
        s("ils", Speaker::Controller, me(), "BRZ682, maintain 3,000 until established on the localizer, cleared ILS runway 27L approach."),
        s("ils-readback", Speaker::Pilot, pilot("BRZ682"), "maintain 3,000 until established on the localizer, cleared ILS runway 27L approach, BRZ682."),
        s("tower-checkin", Speaker::Pilot, pilot("ORN481"), "Tower, ORN481, 10 mile final runway 27L, with Sierra."),
        s("land", Speaker::Controller, me(), "BRZ652, runway 27R, cleared to land, caution wake turbulence."),
        s("land-readback", Speaker::Pilot, pilot("BRZ652"), "runway 27R, cleared to land, BRZ652."),
        s("luaw", Speaker::Controller, me(), "N613WW, runway 09L, line up and wait."),
        s("takeoff-readback", Speaker::Pilot, pilot("ORN640"), "runway 09L, cleared for takeoff, ORN640."),
        s("taxi", Speaker::Controller, me(), "TRL932, taxi via B, D, A, E, hold short of runway 27L."),
        s("taxi-readback", Speaker::Pilot, pilot("TRL932"), "taxi via B, D, A, E, hold short of runway 27L, TRL932."),
        s("holding-short", Speaker::Pilot, pilot("TRL932"), "Tower, TRL932, holding short runway 27R."),
        s("ground-call", Speaker::Pilot, pilot("MRD291"), "Ground, MRD291, at G1, ready to taxi."),
        s("clearance-call", Speaker::Pilot, pilot("ORN938"), "Clearance, ORN938, at C1, IFR to XQRY, ready to copy, with information E."),
        s("hold", Speaker::Controller, me(), "MRD757, hold northwest of NORWD on the 136 course inbound, right turns, 1-minute legs, expect further clearance 0021."),
        s("hold-readback", Speaker::Pilot, pilot("MRD464"), "hold northeast of ALDER as published, expect further clearance 0035, MRD464."),
        s("direct", Speaker::Controller, me(), "MRD987, direct LARCH."),
        s("vfr-downwind", Speaker::Pilot, pilot("N613WW"), "N613WW, midfield right downwind runway 27R, full stop."),
        s("vfr-option", Speaker::Controller, me(), "N613WW, runway 27R, cleared for the option, traffic holding in position."),
        s("vfr-extend", Speaker::Pilot, pilot("N613WW"), "extend downwind, I'll call your base, N613WW."),
        s("fedex", Speaker::Controller, me(), "FDX1407, turn right heading 180, descend and maintain 4,000."),
        s("fedex-readback", Speaker::Pilot, pilot("FDX1407"), "right heading 180, descend and maintain 4,000, FDX1407."),
        s("mayday", Speaker::Emergency, pilot("BRZ622"), "Mayday, mayday, mayday, Approach, BRZ622, engine failure, request return, runway 27L, 120 souls, 1 hour 30 minutes of fuel."),
        s("emergency-level", Speaker::Emergency, pilot("BRZ622"), "BRZ622, level 10000, request vectors to land."),
        s("landline-release", Speaker::Landline, Talker::Facility(Position::Approach), "Approach to Tower: departures released, resume normal."),
        s("landline-emergency", Speaker::Landline, Talker::Facility(Position::Approach), "Approach to Tower: emergency, NVA101, medical emergency, 120 souls, 1 hour 30 minutes of fuel, runway 27L."),
        s("contact-departure", Speaker::Controller, me(), "TRL932, contact departure."),
        s("go-around", Speaker::Pilot, pilot("ORN481"), "go around, ORN481."),
    ]
}

/// What the tests and the harness check without listening.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    pub seconds: f32,
    /// Words said (digits and letters count one each).
    pub words: usize,
    /// Over the speech, from the first word to the last.
    pub words_per_minute: f32,
    /// Samples at full scale.
    pub clipped: usize,
    /// The longest quiet stretch between the first word and the last, seconds.
    pub longest_gap_s: f32,
}

/// Measure a rendered transmission of `words` words.
pub fn measure(samples: &[i16], words: usize) -> Measure {
    const FRAME: usize = SAMPLE_RATE as usize / 100;
    // Speech is far above the hiss under it.
    const QUIET_RMS: f32 = 1200.0;
    let loud: Vec<bool> = samples
        .chunks(FRAME)
        .map(|f| (f.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / f.len() as f32).sqrt() >= QUIET_RMS)
        .collect();
    let first = loud.iter().position(|&l| l).unwrap_or(0);
    let last = loud.iter().rposition(|&l| l).unwrap_or(0);
    let (mut longest, mut run) = (0usize, 0usize);
    for &l in &loud[first..=last] {
        run = if l { 0 } else { run + 1 };
        longest = longest.max(run);
    }
    let speech_s = ((last - first + 1) * FRAME) as f32 / SAMPLE_RATE as f32;
    Measure {
        seconds: samples.len() as f32 / SAMPLE_RATE as f32,
        words,
        words_per_minute: words as f32 / speech_s * 60.0,
        clipped: samples.iter().filter(|&&s| s == i16::MAX || s == i16::MIN).count(),
        longest_gap_s: (longest * FRAME) as f32 / SAMPLE_RATE as f32,
    }
}

struct Waiting {
    since: f64,
    words: Vec<String>,
    voice: (usize, f32),
    /// The player's own, with their voice off: it holds the frequency for
    /// as long as it takes to say, without a sound.
    silent: bool,
}

pub struct Voices {
    packs: Vec<Pack>,
    queue: VecDeque<Waiting>,
    /// The transmission on the air, if it's heard.
    playing: Option<Sound>,
    /// When the frequency is free again.
    busy_until: f64,
    pub enabled: bool,
    /// The player's own transmissions are spoken too.
    pub own: bool,
}

impl Voices {
    pub fn load() -> Voices {
        let packs = PACKS.iter().filter_map(|b| Pack::read(b)).collect::<Vec<_>>();
        let packs = if packs.len() == PACKS.len() { packs } else { Vec::new() };
        Voices { packs, queue: VecDeque::new(), playing: None, busy_until: 0.0, enabled: true, own: true }
    }

    /// Queue a line from the log, if it's a transmission the player hears.
    /// True if a voice will say it: false for the player's own with their
    /// voice off, though it still holds the frequency.
    pub fn hear(&mut self, line: &LogEntry, callsigns: &[&str], now: f64) -> bool {
        let Some(radio) = line.radio.as_ref().filter(|r| r.heard) else { return false };
        if !self.enabled || self.packs.is_empty() {
            return false;
        }
        let words = speech::spoken(speech_text(line.speaker, &line.text), callsigns);
        if words.is_empty() {
            return false;
        }
        let silent = !self.own && radio.talker == Talker::Controller;
        self.queue.push_back(Waiting { since: now, words, voice: voice_for(&radio.talker), silent });
        !silent
    }

    /// A line rendered as it would go out on the air, and how many words it
    /// says. None without the voice packs, or for a line with nothing to say.
    pub fn render(&self, speaker: Speaker, talker: &Talker, text: &str, callsigns: &[&str]) -> Option<(Vec<i16>, usize)> {
        let words = speech::spoken(speech_text(speaker, text), callsigns);
        let said = words.iter().filter(|w| *w != PAUSE && *w != STOP).count();
        let (index, pitch) = voice_for(talker);
        let pack = self.packs.get(index)?;
        (said > 0).then(|| (transmission(&clip_keys(&words, pack), pack, pitch), said))
    }

    /// Put the next transmission on the air once the frequency is free.
    pub fn update(&mut self, now: f64, volume: f32) {
        if now < self.busy_until {
            return;
        }
        self.playing = None;
        while let Some(next) = self.queue.pop_front() {
            if now - next.since > STALE_S || volume <= 0.0 {
                continue;
            }
            let pack = &self.packs[next.voice.0];
            let samples = transmission(&clip_keys(&next.words, pack), pack, next.voice.1);
            if !next.silent {
                let Some(sound) = sound_now(&wav(&samples)) else { continue };
                play_sound(&sound, PlaySoundParams { looped: false, volume });
                self.playing = Some(sound);
            }
            self.busy_until = now + samples.len() as f64 / SAMPLE_RATE as f64 + BETWEEN_S;
            break;
        }
    }

    /// Stop talking and forget what was waiting.
    pub fn silence(&mut self) {
        if let Some(sound) = self.playing.take() {
            stop_sound(&sound);
        }
        self.busy_until = 0.0;
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

    fn voices() -> Voices {
        let voices = Voices::load();
        assert!(!voices.packs.is_empty(), "the packs load");
        voices
    }

    fn line(speaker: Speaker, talker: Talker, text: &str) -> LogEntry {
        LogEntry { time: 0.0, speaker, text: text.into(), radio: Some(crate::sim::Radio { talker, heard: true }) }
    }

    #[test]
    fn your_own_transmissions_can_be_left_unspoken_but_hold_the_frequency() {
        let mut v = voices();
        v.own = false;
        assert!(!v.hear(&line(Speaker::Controller, Talker::Controller, "NVA101, turn left heading 240."), &[], 0.0));
        assert!(v.hear(&line(Speaker::Pilot, Talker::Pilot("NVA101".into()), "left heading 240, NVA101."), &[], 0.0));
        // The first goes "out" silently: the frequency is busy, the readback waits.
        v.update(0.0, 1.0);
        assert!(v.playing.is_none());
        assert!(v.busy_until > 1.0, "held for as long as it takes to say: {}", v.busy_until);
        assert_eq!(v.queue.len(), 1);
    }

    #[test]
    fn digits_and_letters_run_together() {
        assert!(runs_on("one", "zero"));
        assert!(runs_on("two", "seven"));
        assert!(runs_on("seven", "left"));
        assert!(runs_on("three", "thousand"));
        assert!(runs_on("bravo", "two"));
        assert!(!runs_on("runway", "two"));
        assert!(!runs_on("left", "heading"));
    }

    #[test]
    fn a_digit_group_has_no_gap_in_it() {
        let v = voices();
        let (audio, words) = v.render(Speaker::Controller, &Talker::Controller, "NVA101", &[]).unwrap();
        let m = measure(&audio, words);
        assert!(m.longest_gap_s < 0.05, "{m:?}");
    }

    /// The listening set, measured: no clipping, gaps no longer than a full
    /// stop's, and a pace near real radio talk.
    #[test]
    fn the_listening_set_measures_well() {
        let v = voices();
        let (mut words, mut speech_s) = (0usize, 0.0f32);
        for sample in samples() {
            let (audio, said) = v.render(sample.speaker, &sample.talker, sample.text, &[]).unwrap();
            let m = measure(&audio, said);
            assert_eq!(m.clipped, 0, "{}: {m:?}", sample.name);
            assert!(m.longest_gap_s <= STOP_S + 0.1, "{}: {m:?}", sample.name);
            words += m.words;
            speech_s += m.words as f32 / m.words_per_minute * 60.0;
        }
        let pace = words as f32 / speech_s * 60.0;
        assert!((160.0..=260.0).contains(&pace), "{pace:.0} words a minute");
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
