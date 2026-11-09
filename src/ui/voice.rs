//! Radio voices: transmissions spoken from pre-rendered clips.
//!
//! `tools/make_voice.py` renders a clip for every word and stock phrase in
//! `assets/sounds/voice/words.txt` in five public-domain Piper voices, packed
//! one file per voice: 8 kHz, µ-law, already band-limited the way VHF is. A
//! transmission is the clips of its words (`sim::speech`), stock phrases
//! taken whole where they fit, through a radio: a pilot's with a key-up,
//! hiss that grows with distance and a squelch tail; the player's own as a
//! clean sidetone; a landline as a telephone.
//!
//! One transmission plays at a time, as on a real frequency. Ones that wait
//! too long are dropped, so the voices keep up at x2 and x4.

use super::audio::{SAMPLE_RATE, wav};
use crate::sim::speech::{self, PAUSE, STOP};
use crate::sim::positions::{SOLO, Seat};
use crate::sim::{LogEntry, Speaker, Talker};
use macroquad::audio::{PlaySoundParams, Sound, load_sound_from_bytes, play_sound, stop_sound};
use std::collections::{HashMap, VecDeque};

/// The packs, one per voice.
const PACKS: [&[u8]; 5] = [
    include_bytes!("../../assets/sounds/voice/norman.pack"),
    include_bytes!("../../assets/sounds/voice/john.pack"),
    include_bytes!("../../assets/sounds/voice/kristin.pack"),
    include_bytes!("../../assets/sounds/voice/ljspeech.pack"),
    include_bytes!("../../assets/sounds/voice/cori.pack"),
];
/// The voices' names, in pack order.
pub const VOICE_NAMES: [&str; 5] = ["norman", "john", "kristin", "ljspeech", "cori"];
const CONTROLLER: Voice = Voice { pack: 0, pitch: 1.0, pace: 1.0 };
const PILOTS: [usize; 4] = [1, 2, 3, 4];
/// Pitches and paces (a transmission's length, against the voice's own)
/// that make four pilot voices into 36. Pitch changes keep the pace.
const PILOT_PITCHES: [f32; 3] = [0.93, 1.0, 1.07];
const PILOT_PACES: [f32; 3] = [0.84, 0.91, 0.98];
/// The other controller on a landline.
const FACILITY: Voice = Voice { pack: 1, pitch: 0.9, pace: 1.0 };
/// One pilot transmission in this many is keyed late, losing the start of
/// its first word.
const LATE_KEY_ONE_IN: u32 = 12;
const LATE_KEY_S: f32 = 0.09;
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

/// The clip keys for a line's words: stock phrases where they fit, said
/// as at a sentence's end or before a comma where they are, and a word with
/// no clip spelled out.
pub fn clip_keys(words: &[String], pack: &Pack) -> Vec<String> {
    let mut keys = Vec::new();
    for unit in speech::units(words, |k| pack.has(k)) {
        if unit == PAUSE || unit == STOP || pack.has(&unit) {
            keys.push(unit);
        } else {
            keys.extend(speech::spelled(&unit).into_iter().filter(|w| pack.has(w)));
        }
    }
    speech::with_endings(keys, |k| pack.has(k))
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

/// The first and last words of a clip key.
fn first_word(key: &str) -> &str {
    speech::key_words(key).split(' ').next().unwrap_or(key)
}

fn last_word(key: &str) -> &str {
    speech::key_words(key).rsplit(' ').next().unwrap_or(key)
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

/// Who says a transmission: a voice's pack, at a pitch and pace.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    pack: usize,
    /// Above 1, higher.
    pitch: f32,
    /// The transmission's length against the voice's own: below 1, quicker.
    pace: f32,
}

/// What a transmission comes through.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Set {
    /// The player's own voice in their headset.
    Sidetone,
    /// A pilot's radio, so far from the field, nautical miles. `seed` makes
    /// each aircraft's key-up and squelch its own; `late` keys up after the
    /// first word has started.
    Air { range_nm: f32, seed: u32, late: bool },
    /// Another facility, on the landline.
    Landline,
}

/// FNV-1a: the same callsign always sounds the same.
fn hash(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// Which voice says a line, to the player in seat `me`: their own voice,
/// or another controller's.
fn voice_for(talker: &Talker, me: Seat) -> Voice {
    match talker {
        Talker::Controller(seat) if *seat == me => CONTROLLER,
        Talker::Controller(_) | Talker::Facility(_) => FACILITY,
        Talker::Pilot(callsign) => {
            let mut n = hash(callsign) as usize;
            let pack = PILOTS[n % PILOTS.len()];
            n /= PILOTS.len();
            let pitch = PILOT_PITCHES[n % PILOT_PITCHES.len()];
            n /= PILOT_PITCHES.len();
            Voice { pack, pitch, pace: PILOT_PACES[n % PILOT_PACES.len()] }
        }
    }
}

/// What a line comes through: `range_nm` is the aircraft's distance from
/// the field, if known, and `count` how many transmissions came before.
fn set_for(talker: &Talker, me: Seat, range_nm: Option<f32>, count: u32) -> Set {
    match talker {
        Talker::Controller(seat) if *seat == me => Set::Sidetone,
        // Another player only ever reaches you on the landline.
        Talker::Controller(_) | Talker::Facility(_) => Set::Landline,
        Talker::Pilot(callsign) => {
            let seed = hash(callsign);
            let late = count > 0 && (seed ^ count.wrapping_mul(0x9e37_79b9)).wrapping_mul(0x0100_0193) >> 8 & 0xff
                < 256 / LATE_KEY_ONE_IN;
            Set::Air { range_nm: range_nm.unwrap_or(8.0), seed, late }
        }
    }
}

/// A run of samples at `rate`, made `factor` times as long without changing
/// pitch: windowed pieces of it overlapped, each placed where it best
/// continues the one before (WSOLA).
fn stretched(x: &[f32], factor: f32, rate: u32) -> Vec<f32> {
    let n = ((rate as f32 * 0.03) as usize).max(8) & !1;
    let hop = n / 2;
    let tolerance = (rate as f32 * 0.008) as usize;
    if (factor - 1.0).abs() < 0.005 || x.len() < 2 * n {
        return x.to_vec();
    }
    let window: Vec<f32> =
        (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let out_len = (x.len() as f32 * factor) as usize;
    let mut out = vec![0.0f32; out_len + n];
    let mut weight = vec![0.0f32; out_len + n];
    let last = x.len() - n;
    let mut previous = 0usize;
    let mut at = 0usize;
    while at < out_len {
        let nominal = ((at as f32 / factor) as usize).min(last);
        let start = if at == 0 {
            0
        } else {
            // What would follow the last piece, had it gone on.
            let natural = (previous + hop).min(last);
            let target = &x[natural..natural + n];
            let (low, high) = (nominal.saturating_sub(tolerance), (nominal + tolerance).min(last));
            (low..=high)
                .max_by(|&a, &b| {
                    let score = |s: usize| x[s..s + n].iter().zip(target).map(|(p, q)| p * q).sum::<f32>();
                    score(a).total_cmp(&score(b))
                })
                .unwrap_or(nominal)
        };
        for i in 0..n {
            out[at + i] += x[start + i] * window[i];
            weight[at + i] += window[i];
        }
        previous = start;
        at += hop;
    }
    out.truncate(out_len);
    for (o, w) in out.iter_mut().zip(&weight) {
        if *w > 1e-3 {
            *o /= w;
        }
    }
    out
}

/// A second-order filter (RBJ's cookbook), run over the samples in place.
fn biquad(x: &mut [f32], freq: f32, high_pass: bool) {
    let w0 = std::f32::consts::TAU * freq / SAMPLE_RATE as f32;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let (b0, b1) = if high_pass { ((1.0 + cos) / 2.0, -(1.0 + cos)) } else { ((1.0 - cos) / 2.0, 1.0 - cos) };
    let (a0, a1, a2) = (1.0 + alpha, -2.0 * cos, 1.0 - alpha);
    let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b0 / a0, a1 / a0, a2 / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0, 0.0, 0.0);
    for s in x.iter_mut() {
        let y = b0 * *s + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        (x2, x1, y2, y1) = (x1, *s, y1, y);
        *s = y;
    }
}

/// Evens out loud and soft words, as a transmitter's AGC does.
fn compressed(x: &mut [f32]) {
    const THRESHOLD: f32 = 0.2;
    const RATIO: f32 = 3.0;
    const MAKEUP: f32 = 1.8;
    let attack = 1.0 - (-1.0 / (0.005 * SAMPLE_RATE as f32)).exp();
    let release = 1.0 - (-1.0 / (0.08 * SAMPLE_RATE as f32)).exp();
    let mut envelope = 0.0f32;
    for s in x.iter_mut() {
        let level = s.abs();
        envelope += (level - envelope) * if level > envelope { attack } else { release };
        if envelope > THRESHOLD {
            *s *= (THRESHOLD / envelope).powf(1.0 - 1.0 / RATIO);
        }
        // Made up, so the loudest words are about as loud as before.
        *s *= MAKEUP;
    }
}

/// White noise from a seed, -1 to 1.
fn noise(len: usize, seed: u32) -> impl Iterator<Item = f32> {
    let mut state = seed | 1;
    (0..len).map(move |_| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    })
}

/// A burst of noise, faded in and out, `rate` Hz flutter of `depth`.
fn burst(secs: f32, gain: f32, seed: u32, flutter: (f32, f32)) -> Vec<f32> {
    let len = (secs * SAMPLE_RATE as f32) as usize;
    let fade = (len / 8).max(1);
    noise(len, seed)
        .enumerate()
        .map(|(i, x)| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let am = 1.0 - flutter.1 * 0.5 * (1.0 - (t * flutter.0 * std::f32::consts::TAU).cos());
            x * gain * am * (i.min(len - i) as f32 / fade as f32).min(1.0)
        })
        .collect()
}

/// The words of a transmission joined, at the pack's own rate.
fn spoken(keys: &[String], pack: &Pack) -> Vec<f32> {
    let rate = pack.rate as f32;
    let gap = |secs: f32| vec![0.0f32; (secs * rate) as usize];
    let overlap = (CROSSFADE_S * rate) as usize;
    let mut speech = Vec::new();
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
                let word = trimmed(&decoded, pack.rate);
                match previous {
                    Some(before) if runs_on(last_word(before), first_word(key)) => join(&mut speech, &word, overlap),
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
    speech
}

/// A whole transmission: the words in this voice, through this set.
fn transmission(keys: &[String], pack: &Pack, voice: Voice, set: Set) -> Vec<i16> {
    let speech = spoken(keys, pack);
    // Stretched by the pitch as well as the pace, since resampling to the
    // pitch then shortens it by as much.
    let speech = stretched(&speech, voice.pitch * voice.pace, pack.rate);
    let step = pack.rate as f32 / SAMPLE_RATE as f32 * voice.pitch;
    let mut words = Vec::with_capacity((speech.len() as f32 / step) as usize + 1);
    let mut t = 0.0f32;
    while (t as usize) + 1 < speech.len() {
        let j = t as usize;
        words.push(speech[j] + (speech[j + 1] - speech[j]) * (t - j as f32));
        t += step;
    }
    let secs = |s: f32| (s * SAMPLE_RATE as f32) as usize;
    if let Set::Air { late: true, .. } = set {
        // Keyed a moment after starting to talk: the first syllable is cut.
        let cut = secs(LATE_KEY_S).min(words.len() / 3);
        words.drain(..cut);
        for (i, w) in words.iter_mut().take(secs(0.004)).enumerate() {
            *w *= i as f32 / secs(0.004) as f32;
        }
    }
    compressed(&mut words);
    // Level, drive, hiss, key-up, squelch and band for each kind of set.
    let (level, drive, hiss, band, seed) = match set {
        Set::Sidetone => (0.5, 1.2, 0.003, (200.0, 3800.0), 7),
        Set::Landline => (0.5, 1.4, 0.006, (300.0, 3400.0), 11),
        Set::Air { range_nm, seed, .. } => {
            let far = (range_nm / 50.0).clamp(0.0, 1.0);
            (0.6 - 0.12 * far, 1.6, 0.012 + 0.04 * far, (300.0, 3000.0), seed)
        }
    };
    let lead = secs(0.06);
    let mut out = vec![0.0f32; lead];
    out.extend(words.iter().map(|s| (s * drive).tanh() * level));
    out.extend(std::iter::repeat_n(0.0, secs(0.05)));
    for (o, h) in out.iter_mut().zip(noise(usize::MAX, seed ^ 0x5bd1_e995)) {
        *o += h * hiss;
    }
    let pick = |shift: u32, low: f32, high: f32| low + (high - low) * ((seed >> shift) & 0xff) as f32 / 255.0;
    match set {
        Set::Air { .. } => {
            // Key-up click over the lead-in, and a squelch tail.
            let click = burst(pick(0, 0.006, 0.014), pick(8, 0.3, 0.5), seed, (0.0, 0.0));
            for (o, c) in out.iter_mut().zip(&click) {
                *o += c;
            }
            out.extend(burst(pick(16, 0.05, 0.12), pick(24, 0.15, 0.25), seed.rotate_left(7), (pick(4, 10.0, 18.0), 0.6)));
        }
        Set::Sidetone => {
            let click = burst(0.004, 0.12, seed, (0.0, 0.0));
            for (o, c) in out.iter_mut().zip(&click) {
                *o += c;
            }
        }
        Set::Landline => {}
    }
    biquad(&mut out, band.0, true);
    biquad(&mut out, band.1, false);
    let full = i16::MAX as f32 - 1.0;
    out.iter().map(|s| (s * i16::MAX as f32).clamp(-full, full) as i16).collect()
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
    let me = || Talker::Controller(0);
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
    voice: Voice,
    set: Set,
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
    /// Transmissions heard so far, which vary from one to the next.
    count: u32,
    /// The player listening: their own transmissions are in their voice.
    pub seat: Seat,
}

impl Voices {
    pub fn load() -> Voices {
        let packs = PACKS.iter().filter_map(|b| Pack::read(b)).collect::<Vec<_>>();
        let packs = if packs.len() == PACKS.len() { packs } else { Vec::new() };
        Voices { packs, queue: VecDeque::new(), playing: None, busy_until: 0.0, enabled: true, own: true, count: 0, seat: SOLO }
    }

    /// Queue a line from the log, if it's a transmission the player hears,
    /// from an aircraft `range_nm` from the field if it's a pilot's.
    /// True if a voice will say it: false for the player's own with their
    /// voice off, though it still holds the frequency.
    pub fn hear(&mut self, line: &LogEntry, callsigns: &[&str], range_nm: Option<f32>, now: f64) -> bool {
        let Some(radio) = line.radio.as_ref().filter(|r| r.heard_by.has(self.seat)) else { return false };
        if !self.enabled || self.packs.is_empty() {
            return false;
        }
        let words = speech::spoken(speech_text(line.speaker, &line.text), callsigns);
        if words.is_empty() {
            return false;
        }
        let silent = !self.own && radio.talker == Talker::Controller(self.seat);
        self.count = self.count.wrapping_add(1);
        let set = set_for(&radio.talker, self.seat, range_nm, self.count);
        let voice = voice_for(&radio.talker, self.seat);
        self.queue.push_back(Waiting { since: now, words, voice, set, silent });
        !silent
    }

    /// A line rendered as it would go out on the air, and how many words it
    /// says. None without the voice packs, or for a line with nothing to say.
    pub fn render(&self, speaker: Speaker, talker: &Talker, text: &str, callsigns: &[&str]) -> Option<(Vec<i16>, usize)> {
        let words = speech::spoken(speech_text(speaker, text), callsigns);
        let said = words.iter().filter(|w| *w != PAUSE && *w != STOP).count();
        let voice = voice_for(talker, SOLO);
        let pack = self.packs.get(voice.pack)?;
        (said > 0).then(|| (transmission(&clip_keys(&words, pack), pack, voice, set_for(talker, SOLO, None, 0)), said))
    }

    /// One clip on its own, in a voice by its index in `VOICE_NAMES`, for
    /// listening through the vocabulary. None if there's no such clip.
    pub fn clip(&self, voice: usize, key: &str) -> Option<Vec<i16>> {
        let pack = self.packs.get(voice)?;
        pack.has(key).then(|| {
            transmission(&[key.to_string()], pack, Voice { pack: voice, pitch: 1.0, pace: 1.0 }, Set::Landline)
        })
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
            let pack = &self.packs[next.voice.pack];
            let samples = transmission(&clip_keys(&next.words, pack), pack, next.voice, next.set);
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
        let pack = &packs()[CONTROLLER.pack];
        let words = speech::spoken("NVA101, runway 27L, cleared to land.", &[]);
        let keys = clip_keys(&words, pack);
        assert!(keys.iter().any(|k| speech::key_words(k) == "cleared to land"), "{keys:?}");
        let said: Vec<&str> = keys.iter().map(|k| speech::key_words(k)).collect();
        assert_eq!(said[..3].join(" "), "nova one zero one", "{keys:?}");
    }

    #[test]
    fn a_word_with_no_clip_is_spelled() {
        let pack = &packs()[CONTROLLER.pack];
        let keys = clip_keys(&["zzqx".to_string()], pack);
        assert_eq!(keys, ["zulu", "zulu", "quebec", "x-ray"]);
    }

    #[test]
    fn a_transmission_is_sound() {
        let pack = &packs()[1];
        let keys = clip_keys(&speech::spoken("Tower, NVA101, holding short runway 27R.", &[]), pack);
        let voice = Voice { pack: 1, pitch: 1.07, pace: 1.0 };
        let samples = transmission(&keys, pack, voice, Set::Air { range_nm: 5.0, seed: 1, late: false });
        assert!(samples.len() > SAMPLE_RATE as usize, "at least a second: {}", samples.len());
        assert!(samples.iter().any(|s| s.unsigned_abs() > 3000), "loud enough");
    }

    #[test]
    fn a_pitch_change_keeps_the_pace() {
        let pack = &packs()[1];
        let keys = clip_keys(&speech::spoken("Tower, NVA101, holding short runway 27R.", &[]), pack);
        let set = Set::Landline;
        let len = |pitch, pace| transmission(&keys, pack, Voice { pack: 1, pitch, pace }, set).len() as f32;
        let plain = len(1.0, 1.0);
        for pitch in [0.9, 1.07] {
            assert!((len(pitch, 1.0) / plain - 1.0).abs() < 0.03, "pitch {pitch}: {} against {plain}", len(pitch, 1.0));
        }
        assert!((len(1.0, 0.9) / plain - 0.9).abs() < 0.05, "quicker: {} against {plain}", len(1.0, 0.9));
    }

    #[test]
    fn stretching_keeps_a_tone() {
        let rate = 8000;
        let tone: Vec<f32> = (0..8000).map(|i| (i as f32 * 0.3).sin() * 0.5).collect();
        let long = stretched(&tone, 1.2, rate);
        assert_eq!(long.len(), 9600);
        // Same pitch: as many zero crossings a second.
        let crossings = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / x.len() as f32;
        let (a, b) = (crossings(&tone), crossings(&long[400..9200]));
        assert!((a / b - 1.0).abs() < 0.03, "{a} against {b}");
    }

    #[test]
    fn a_far_aircraft_is_noisier() {
        let pack = &packs()[1];
        let voice = Voice { pack: 1, pitch: 1.0, pace: 1.0 };
        let quiet_rms = |range_nm| {
            let audio = transmission(&[], pack, voice, Set::Air { range_nm, seed: 3, late: false });
            // The middle of the lead-in and tail: hiss alone.
            let hiss = &audio[audio.len() / 2 - 200..audio.len() / 2 + 200];
            (hiss.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / hiss.len() as f32).sqrt()
        };
        assert!(quiet_rms(45.0) > 2.0 * quiet_rms(2.0), "{} against {}", quiet_rms(45.0), quiet_rms(2.0));
    }

    #[test]
    fn some_pilot_transmissions_are_keyed_late() {
        let talker = Talker::Pilot("NVA101".into());
        let late = (1..=600).filter(|&n| matches!(set_for(&talker, SOLO, None, n), Set::Air { late: true, .. })).count();
        assert!((20..=90).contains(&late), "{late} in 600");
        assert_eq!(set_for(&Talker::Controller(0), SOLO, Some(3.0), 5), Set::Sidetone);
    }

    fn voices() -> Voices {
        let voices = Voices::load();
        assert!(!voices.packs.is_empty(), "the packs load");
        voices
    }

    fn line(speaker: Speaker, talker: Talker, text: &str) -> LogEntry {
        LogEntry { time: 0.0, speaker, text: text.into(), radio: Some(crate::sim::Radio { talker, heard_by: crate::sim::positions::Seats::of(0) }) }
    }

    #[test]
    fn your_own_transmissions_can_be_left_unspoken_but_hold_the_frequency() {
        let mut v = voices();
        v.own = false;
        assert!(!v.hear(&line(Speaker::Controller, Talker::Controller(0), "NVA101, turn left heading 240."), &[], None, 0.0));
        assert!(v.hear(&line(Speaker::Pilot, Talker::Pilot("NVA101".into()), "left heading 240, NVA101."), &[], None, 0.0));
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
        let (audio, words) = v.render(Speaker::Controller, &Talker::Controller(0), "NVA101", &[]).unwrap();
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
        let a = voice_for(&Talker::Pilot("NVA101".into()), SOLO);
        assert_eq!(a, voice_for(&Talker::Pilot("NVA101".into()), SOLO));
        let voices: std::collections::HashSet<(usize, u32, u32)> = (0..200)
            .map(|n| voice_for(&Talker::Pilot(format!("TRL{n}")), SOLO))
            .map(|v| (v.pack, (v.pitch * 100.0) as u32, (v.pace * 100.0) as u32))
            .collect();
        assert!(voices.len() >= 24, "a spread of voices: {voices:?}");
        assert!(voices.iter().all(|(v, _, _)| *v != CONTROLLER.pack));
        assert!(PILOTS.iter().all(|p| voices.iter().any(|(v, _, _)| v == p)), "every pilot voice is used");
    }
}
