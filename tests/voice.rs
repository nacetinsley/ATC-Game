//! Radio voices: every transmission the game makes can be spoken with the
//! clips in `assets/sounds/voice`, and the right ones are heard.
//!
//! The clip vocabulary, `assets/sounds/voice/words.txt`, is written by
//! `cargo test --release --test voice -- --ignored write_vocabulary`, and
//! `tools/make_voice.py` renders a clip for each line of it.

use atcgame::sim::airspace::{Airport, KMEM_AIRPORT, KMEM_GROUND, KSUS_AIRPORT, KSUS_GROUND};
use atcgame::sim::clearance::Clearance;
use atcgame::sim::emergency::EmergencyRate;
use atcgame::sim::mistakes::PilotMistakes;
use atcgame::sim::positions::Staffing;
use atcgame::sim::speech::{self, PAUSE, STOP};
use atcgame::sim::weather::WeatherKind;
use atcgame::sim::{Radio, Speaker, Talker, World, WorldConfig};
use std::collections::BTreeSet;
use std::path::Path;

const WORDS: &str = include_str!("../assets/sounds/voice/words.txt");

/// Source files whose strings make up radio calls.
const RADIO_SOURCES: [&str; 22] = [
    "approach", "clearance", "conflict", "coordination", "delivery", "emergency", "ground", "ground_map", "holding",
    "mistakes", "positions", "ramp", "sky", "speech", "storms", "tower", "traffic", "tutorial", "units", "vfr",
    "visibility", "weather",
];

fn airports() -> Vec<(Airport, &'static str)> {
    vec![
        (Airport::default_airport(), include_str!("../assets/airports/fictional.toml")),
        (Airport::load(KSUS_AIRPORT, Some(KSUS_GROUND)).unwrap(), KSUS_AIRPORT),
        (Airport::load(KMEM_AIRPORT, Some(KMEM_GROUND)).unwrap(), KMEM_AIRPORT),
    ]
}

/// A player who says a bit of everything, every so often, to every aircraft:
/// enough to bring out the readbacks and the "unable"s.
fn player(world: &mut World, step: u32) {
    if !step.is_multiple_of(48) {
        return;
    }
    let ends = world.airport.runways.len();
    let fixes = world.airport.fixes.len().max(1);
    let nodes = world.airport.surface.as_ref().map_or(1, |m| m.nodes.len());
    let physical = world.airport.surface.as_ref().map_or(1, |m| m.runways.len());
    let ids: Vec<u32> = world.aircraft.iter().map(|a| a.id).collect();
    for (n, id) in ids.into_iter().enumerate() {
        let k = (step / 48) as usize + n * 7 + id as usize;
        let end = k % ends;
        let hold = world.find(id).map(|ac| world.hold_for(ac, k % fixes));
        let clearance = match k % 27 {
            0 => Clearance::Heading(((k * 37) % 360) as f32),
            1 => Clearance::Altitude(((k % 9) as f32 + 2.0) * 1000.0),
            2 => Clearance::Speed(170.0 + (k % 6) as f32 * 10.0),
            3 => Clearance::DirectFix(k % fixes),
            4 => Clearance::ResumeRoute,
            5 => Clearance::ClearedIls(end),
            6 => Clearance::ClearedRnav(end),
            7 => Clearance::ClearedVisual { runway: end, follow: None },
            8 => Clearance::GoAround,
            9 => Clearance::HandoffTower,
            10 => Clearance::HandoffCenter,
            11 => Clearance::Taxi { to: k % nodes, via: Vec::new() },
            12 => Clearance::HoldPosition,
            13 => Clearance::ContinueTaxi,
            14 => Clearance::CrossRunway(k % physical),
            15 => Clearance::LineUpAndWait(end),
            16 => Clearance::ClearedForTakeoff(end),
            17 => Clearance::ClearedToLand(end),
            18 => Clearance::ContactGround,
            19 => Clearance::ContactTower,
            20 => Clearance::ContactDeparture,
            21 => Clearance::EnterDownwind(end),
            22 => Clearance::ExtendDownwind,
            23 => Clearance::TurnBase,
            24 => Clearance::ClearedTouchAndGo(end),
            25 => Clearance::ClearedOption(end),
            _ => match hold {
                Some(h) => Clearance::Hold(h),
                None => Clearance::Squawk(4321),
            },
        };
        world.transmit(id, vec![clearance]);
    }
}

type Line = (String, Radio, Speaker, Vec<String>);

/// Every radio line from a set of busy shifts at each airport, worked out once.
fn radio_lines() -> &'static [Line] {
    static LINES: std::sync::OnceLock<Vec<Line>> = std::sync::OnceLock::new();
    LINES.get_or_init(|| play_shifts(11))
}

/// The same from shifts with other seeds: other callsigns, other traffic.
/// The vocabulary is judged on these, so it's judged on lines it wasn't
/// chosen from.
fn other_radio_lines() -> &'static [Line] {
    static LINES: std::sync::OnceLock<Vec<Line>> = std::sync::OnceLock::new();
    LINES.get_or_init(|| play_shifts(101))
}

fn play_shifts(first_seed: u64) -> Vec<Line> {
    let mut lines = Vec::new();
    for (airport, _) in airports() {
        let staffings = [Staffing::LOCAL, Staffing::LOCAL_GROUND, Staffing::APPROACH, Staffing::GROUND_DELIVERY, Staffing::ALL];
        for (n, staffing) in staffings.into_iter().enumerate() {
            let config = WorldConfig {
                seed: first_seed + n as u64,
                staffing,
                vfr: true,
                emergencies: EmergencyRate::Frequent,
                mistakes: PilotMistakes::Frequent,
                weather: WeatherKind::Storms,
                shift_seconds: 30.0 * 60.0,
                ..Default::default()
            };
            let mut world = World::new(airport.clone(), config);
            let mut step = 0;
            let mut seen = 0;
            while world.ended.is_none() {
                player(&mut world, step);
                world.tick();
                world.drain_events();
                step += 1;
                // The log keeps only the latest lines: read them as they come.
                let new = (world.said - seen).min(world.log.len() as u64) as usize;
                seen = world.said;
                let callsigns: Vec<String> = world.aircraft.iter().map(|a| a.callsign.clone()).collect();
                let callsigns: Vec<&str> = callsigns.iter().map(String::as_str).collect();
                for line in &world.log[world.log.len() - new..] {
                    if let Some(radio) = &line.radio {
                        let words = speech::spoken(&speech_text(line.speaker, &line.text), &callsigns);
                        lines.push((line.text.clone(), radio.clone(), line.speaker, words));
                    }
                }
            }
        }
    }
    lines
}

/// The words of a line, as the voices say it: a landline without its
/// "Approach to Tower:" heading, an automated controller without its name.
fn speech_text(speaker: Speaker, text: &str) -> String {
    match speaker {
        Speaker::Landline | Speaker::Controller => match text.split_once(": ") {
            Some((head, rest)) if !head.contains(',') => rest.to_string(),
            _ => text.to_string(),
        },
        _ => text.to_string(),
    }
}

/// The words in string literals of the radio sources, outside their tests.
fn literal_words() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut words = BTreeSet::new();
    for name in RADIO_SOURCES {
        let text = std::fs::read_to_string(root.join("src/sim").join(format!("{name}.rs"))).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for literal in string_literals(code) {
            // Leave out format placeholders.
            let mut plain = String::new();
            let mut depth = 0;
            for c in literal.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ if depth == 0 => plain.push(c),
                    _ => {}
                }
            }
            for word in speech::spoken(&plain, &[]) {
                words.insert(word);
            }
        }
    }
    words
}

fn string_literals(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = code.chars().peekable();
    let mut in_comment = false;
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
        } else if c == '/' && chars.peek() == Some(&'/') {
            in_comment = true;
        } else if c == '\'' && previous != '\\' {
            // A char literal: skip it, so '"' isn't taken for a string.
            let mut look = chars.clone();
            if let (Some(a), Some(b)) = (look.next(), look.next())
                && (b == '\'' || (a == '\\' && look.next() == Some('\'')))
            {
                chars.next();
                chars.next();
                if a == '\\' {
                    chars.next();
                }
            }
        } else if c == '"' {
            let mut literal = String::new();
            let mut escaped = false;
            for d in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == '"' {
                    break;
                } else {
                    literal.push(d);
                }
            }
            out.push(literal);
        }
        previous = c;
    }
    out
}

/// Names in the airport data: fixes, procedures, transitions, taxiways,
/// stands, spots and ramps.
fn airport_words() -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    for (_, text) in airports() {
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_string();
                continue;
            }
            let skip = matches!(section.as_str(), "traffic" | "city_pairs" | "references" | "mva" | "configs");
            if skip || line.starts_with('#') || line.starts_with("data_source") {
                continue;
            }
            for literal in string_literals(line) {
                for word in speech::spoken(&literal, &[]) {
                    words.insert(word);
                }
            }
        }
    }
    words
}

/// Words the speech rules use however a line reads.
fn rule_words() -> BTreeSet<String> {
    let mut words: BTreeSet<String> = speech::DIGITS.iter().chain(speech::PHONETIC.iter()).map(|w| w.to_string()).collect();
    for (_, w) in speech::ABBREVIATIONS {
        words.insert(w.to_string());
    }
    for (_, name) in speech::TELEPHONY.iter().chain(speech::AIRPORT_NAMES.iter()) {
        words.extend(name.split(' ').map(str::to_string));
    }
    for w in ["thousand", "hundred", "point", "flight", "level", "left", "right", "center"] {
        words.insert(w.to_string());
    }
    words
}

fn known_words() -> BTreeSet<&'static str> {
    WORDS.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

/// A phrase becomes a clip when the lines would use it this often.
const PHRASE_MIN: usize = 30;
/// And at most this many phrases, the most useful first.
const PHRASE_BUDGET: usize = 2500;
/// A clip gets its own sentence-end or before-comma form when it comes
/// there this often.
const ENDING_MIN: usize = 30;

/// The words of every heard transmission.
fn heard_words(lines: &'static [Line]) -> Vec<&'static [String]> {
    lines.iter().filter(|(_, r, _, _)| r.heard).map(|(_, _, _, w)| w.as_slice()).collect()
}

/// A digit or letter of the phonetic alphabet: part of a number, callsign,
/// taxiway or code, which vary from one transmission to the next.
fn spelling(word: &str) -> bool {
    speech::DIGITS.contains(&word) || speech::PHONETIC.contains(&word)
}

/// Fit to be a phrase: no more than two digits or letters in a row, so no
/// one callsign, code or heading becomes a clip of its own.
fn general(run: &[String]) -> bool {
    run.windows(3).all(|w| !w.iter().all(|x| spelling(x)))
}

/// Every two-digit run: numbers are said in pairs and threes.
fn digit_pairs() -> Vec<String> {
    speech::DIGITS.iter().flat_map(|a| speech::DIGITS.iter().map(move |b| format!("{a} {b}"))).collect()
}

/// How often each clip is used across the lines, with these clips known.
fn usage(lines: &[&[String]], known: &BTreeSet<String>) -> std::collections::BTreeMap<String, usize> {
    let mut used = std::collections::BTreeMap::new();
    for words in lines {
        for unit in speech::units(words, |k| known.contains(k)) {
            *used.entry(unit).or_insert(0) += 1;
        }
    }
    used
}

/// The phrases worth a clip: the stock ones, digit pairs, and every run of
/// words the lines would use at least PHRASE_MIN times.
fn phrases(words: &BTreeSet<String>) -> BTreeSet<String> {
    let lines = heard_words(radio_lines());
    let fixed: BTreeSet<String> = speech::PHRASES.iter().map(|p| p.to_string()).chain(digit_pairs()).collect();
    // Candidates: every run that occurs often enough, inside one stretch
    // between pauses.
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for words in &lines {
        for stretch in words.split(|w| w == PAUSE || w == STOP) {
            for n in 2..=speech::LONGEST_PHRASE.min(stretch.len()) {
                for run in stretch.windows(n).filter(|r| general(r)) {
                    *counts.entry(run.join(" ")).or_insert(0) += 1;
                }
            }
        }
    }
    let mut chosen: BTreeSet<String> =
        counts.into_iter().filter(|(_, c)| *c >= PHRASE_MIN).map(|(p, _)| p).chain(fixed.iter().cloned()).collect();
    // Keep only those still used often once longer ones take their place.
    for _ in 0..4 {
        let known: BTreeSet<String> = words.union(&chosen).cloned().collect();
        let used = usage(&lines, &known);
        let mut kept: Vec<(usize, String)> = chosen
            .iter()
            .filter_map(|p| {
                let n = used.get(p).copied().unwrap_or(0);
                (fixed.contains(p) || n >= PHRASE_MIN).then(|| (n * (p.split(' ').count() - 1), p.clone()))
            })
            .collect();
        kept.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        kept.truncate(PHRASE_BUDGET.max(fixed.len()));
        chosen = kept.into_iter().map(|(_, p)| p).chain(fixed.iter().cloned()).collect();
    }
    chosen
}

/// The clips said often enough at a sentence's end or before a comma to
/// have their own form there.
fn endings(known: &BTreeSet<String>) -> BTreeSet<String> {
    let lines = heard_words(radio_lines());
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let every = |k: &str| {
        known.contains(k) || k.strip_suffix(speech::AT_END).or_else(|| k.strip_suffix(speech::BEFORE_COMMA)).is_some_and(|b| known.contains(b))
    };
    for words in &lines {
        let units = speech::units(words, |k| known.contains(k));
        for key in speech::with_endings(units, every) {
            if key.ends_with(speech::AT_END) || key.ends_with(speech::BEFORE_COMMA) {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    counts.into_iter().filter(|(_, c)| *c >= ENDING_MIN).map(|(k, _)| k).collect()
}

/// Writes `assets/sounds/voice/words.txt`: single words, then the phrases,
/// then the sentence-end and before-comma forms.
#[test]
#[ignore]
fn write_vocabulary() {
    let mut words = rule_words();
    words.extend(literal_words());
    words.extend(airport_words());
    for (_, _, _, spoken) in radio_lines() {
        words.extend(spoken.iter().cloned());
    }
    words.retain(|w| w != PAUSE && w != STOP && !w.is_empty());
    let phrases = phrases(&words);
    let known: BTreeSet<String> = words.union(&phrases).cloned().collect();
    let endings = endings(&known);
    let mut text = String::from(
        "# The radio voice vocabulary: one clip per line. Written by\n# `cargo test --release --test voice -- --ignored write_vocabulary`.\n",
    );
    for line in words.iter().chain(phrases.iter()).chain(endings.iter()) {
        text.push_str(line);
        text.push('\n');
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/sounds/voice/words.txt");
    std::fs::write(path, text).unwrap();
    println!("{} words, {} phrases, {} endings", words.len(), phrases.len(), endings.len());
}

/// Most words are said inside a clip of several, not stitched one by one,
/// in shifts other than those the phrases were chosen from.
#[test]
fn most_words_come_in_phrases() {
    let known: BTreeSet<String> = known_words().into_iter().map(str::to_string).collect();
    let (mut total, mut in_phrases) = (0usize, 0usize);
    for words in heard_words(other_radio_lines()) {
        for unit in speech::units(words, |k| known.contains(k)) {
            if unit == PAUSE || unit == STOP {
                continue;
            }
            let n = unit.split(' ').count();
            total += n;
            if n > 1 {
                in_phrases += n;
            }
        }
    }
    let share = in_phrases as f32 / total as f32;
    println!("{:.0}% of words in phrases", share * 100.0);
    assert!(share >= 0.6, "{:.0}% of words in phrases", share * 100.0);
}

#[test]
fn every_radio_line_can_be_spoken() {
    let known = known_words();
    let mut missing = BTreeSet::new();
    for (text, _, _, words) in radio_lines() {
        for w in words {
            if w != PAUSE && w != STOP && !known.contains(w.as_str()) {
                missing.insert(format!("{w:?} in {text:?}"));
            }
        }
    }
    assert!(missing.is_empty(), "{} words with no clip:\n{}", missing.len(), missing.into_iter().collect::<Vec<_>>().join("\n"));
}

#[test]
fn every_phrase_is_made_of_known_words() {
    let known = known_words();
    for phrase in speech::PHRASES {
        assert!(known.contains(phrase), "{phrase:?} isn't in words.txt");
    }
    for key in known.iter().filter(|k| k.contains(' ')) {
        let words = speech::key_words(key);
        assert!(known.contains(words), "{key:?}'s words aren't a clip of their own");
        for w in words.split(' ') {
            assert!(known.contains(w), "{w:?} of {key:?} isn't in words.txt");
        }
    }
}

/// The player's own transmissions are heard; an automated controller isn't.
/// Pilots are heard on the player's frequencies only: a readback to the
/// automated Ground isn't.
#[test]
fn who_is_heard() {
    let lines = radio_lines();
    let mut heard_pilot = false;
    let mut unheard_pilot = false;
    for (text, radio, speaker, _) in lines {
        match (&radio.talker, speaker) {
            (Talker::Controller, Speaker::Controller) => assert!(radio.heard, "{text}"),
            (Talker::Facility(_), Speaker::Controller) => assert!(!radio.heard, "{text}"),
            (Talker::Pilot(_), _) => {
                heard_pilot |= radio.heard;
                unheard_pilot |= !radio.heard;
            }
            _ => {}
        }
    }
    assert!(heard_pilot && unheard_pilot);
    // An automated Ground taxi instruction, and its readback, from a Local shift.
    let readback = lines.iter().find(|(t, r, s, _)| *s == Speaker::Pilot && t.contains(", taxi via ") && !r.heard);
    assert!(readback.is_some(), "a readback to the automated Ground");
}

/// A landline is heard only with the player on one end, and speaks as the
/// controller who called.
#[test]
fn landlines() {
    let mut any = false;
    for (text, radio, speaker, words) in radio_lines() {
        if *speaker == Speaker::Landline {
            any = true;
            let (from, _) = text.split_once(" to ").expect("Approach to Tower: ...");
            match &radio.talker {
                Talker::Facility(p) => assert_eq!(p.name(), from, "{text}"),
                Talker::Controller => assert!(radio.heard),
                Talker::Pilot(_) => panic!("{text}"),
            }
            assert_ne!(words.first().map(String::as_str), Some("approach"), "the heading is left out: {text}");
        }
    }
    assert!(any);
}
