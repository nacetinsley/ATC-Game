//! Radio lines as spoken words, for the radio voices.
//!
//! The comms log is written the way a controller writes: "NVA101, descend and
//! maintain 3,000, runway 27L". This turns a line into the words a pilot or
//! controller says, "Nova one zero one, descend and maintain three thousand,
//! runway two seven left", following the number and callsign rules of JO
//! 7110.65 2-4-17 (numbers) and 2-4-20 (callsigns). The words are keys into
//! the voice clips: lower case, one word each, with `,` and `.` for pauses.

/// A short pause, at a comma.
pub const PAUSE: &str = ",";
/// A longer pause, at the end of a sentence.
pub const STOP: &str = ".";

/// The ICAO phonetic alphabet (7110.65 2-4-16).
pub const PHONETIC: [&str; 26] = [
    "alfa", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliett", "kilo", "lima",
    "mike", "november", "oscar", "papa", "quebec", "romeo", "sierra", "tango", "uniform", "victor", "whiskey",
    "x-ray", "yankee", "zulu",
];

/// Digits as spoken on the radio (7110.65 2-4-17): "niner" for 9.
pub const DIGITS: [&str; 10] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "niner"];

/// Abbreviations said letter by letter, or as a word of their own. The clip
/// is rendered from the spelling on the right.
pub const ABBREVIATIONS: [(&str, &str); 7] = [
    ("VFR", "vfr"),
    ("IFR", "ifr"),
    ("ILS", "ils"),
    ("RNAV", "rnav"),
    ("EFC", "efc"),
    ("ATIS", "atis"),
    ("MVA", "mva"),
];

/// Airline telephony: the game's fictional airlines, then the real ones in its
/// airport data, from FAA Order JO 7340.2 chapter 3.
pub const TELEPHONY: [(&str, &str); 17] = [
    ("NVA", "nova"),
    ("BRZ", "bronze"),
    ("TRL", "trail"),
    ("ORN", "orion"),
    ("MRD", "meridian"),
    ("CWN", "crown"),
    ("AAL", "american"),
    ("AAY", "allegiant"),
    ("DAL", "delta"),
    ("EDV", "endeavor"),
    ("ENY", "envoy"),
    ("FDX", "fedex"),
    ("GTW", "gateway"),
    ("MTN", "mountain"),
    ("SKW", "skywest"),
    ("SPT", "servicios sup"),
    ("SWA", "southwest"),
];

/// Airports by the name a pilot or controller uses for them ("cleared to
/// O'Hare airport"): the real ones in the game's traffic, and names for its
/// fictional ones.
pub const AIRPORT_NAMES: [(&str, &str); 77] = [
    ("KADS", "addison"),
    ("KAPA", "centennial"),
    ("KATL", "atlanta"),
    ("KAUS", "austin"),
    ("KBNA", "nashville"),
    ("KBOS", "boston"),
    ("KCLT", "charlotte"),
    ("KCMH", "columbus"),
    ("KCVG", "cincinnati"),
    ("KDAL", "dallas love"),
    ("KDEN", "denver"),
    ("KDFW", "dallas fort worth"),
    ("KDSM", "des moines"),
    ("KDTW", "detroit"),
    ("KELP", "el paso"),
    ("KEWR", "newark"),
    ("KHOU", "houston hobby"),
    ("KIAD", "dulles"),
    ("KIAH", "houston intercontinental"),
    ("KIND", "indianapolis"),
    ("KJAN", "jackson"),
    ("KJFK", "kennedy"),
    ("KLAS", "las vegas"),
    ("KLAX", "los angeles"),
    ("KMCI", "kansas city"),
    ("KMCO", "orlando"),
    ("KMDW", "midway"),
    ("KMEM", "memphis"),
    ("KMIA", "miami"),
    ("KMKC", "kansas city downtown"),
    ("KMKE", "milwaukee"),
    ("KMSP", "minneapolis"),
    ("KMSY", "new orleans"),
    ("KOAK", "oakland"),
    ("KOMA", "omaha"),
    ("KONT", "ontario"),
    ("KOPF", "opa locka"),
    ("KORD", "o'hare"),
    ("KPBI", "palm beach"),
    ("KPDK", "peachtree dekalb"),
    ("KPHL", "philadelphia"),
    ("KPHX", "phoenix"),
    ("KPWK", "chicago executive"),
    ("KRDU", "raleigh durham"),
    ("KSAT", "san antonio"),
    ("KSEA", "seattle"),
    ("KSFO", "san francisco"),
    ("KSLC", "salt lake"),
    ("KSTL", "saint louis"),
    ("KSUS", "spirit"),
    ("KTEB", "teterboro"),
    ("KTPA", "tampa"),
    ("KTUL", "tulsa"),
    ("KYIP", "willow run"),
    ("MMMX", "mexico city"),
    ("PANC", "anchorage"),
    ("XAVN", "avon"),
    ("XBEL", "belford"),
    ("XBRM", "bramley"),
    ("XCLV", "clevedon"),
    ("XDNE", "dunmore"),
    ("XELS", "elston"),
    ("XFAR", "farley"),
    ("XGRN", "greenfield"),
    ("XHGT", "harrowgate"),
    ("XHOL", "holbrook"),
    ("XIVE", "iverton"),
    ("XKST", "kingston"),
    ("XLWN", "lowden"),
    ("XMRS", "marston"),
    ("XNTH", "northam"),
    ("XPLM", "palmer"),
    ("XQRY", "quarry"),
    ("XRDG", "redgate"),
    ("XSTV", "stavely"),
    ("XTRN", "thornton"),
    ("XWLD", "wealdon"),
];

/// Stock phrases rendered as one clip each, so they're said in one breath
/// rather than word by word. Played wherever the words come in this order.
pub const PHRASES: [&str; 44] = [
    "cleared for takeoff",
    "cleared to land",
    "line up and wait",
    "continue holding",
    "cleared for the option",
    "cleared touch and go",
    "closed traffic",
    "departure approved",
    "until established on the localizer",
    "and maintain",
    "turn left heading",
    "turn right heading",
    "fly runway heading",
    "hold short of runway",
    "holding short runway",
    "hold position",
    "continue taxi",
    "cross runway",
    "contact ground",
    "contact tower",
    "contact departure",
    "contact center",
    "frequency change approved",
    "resume own navigation",
    "go around",
    "going around",
    "caution wake turbulence",
    "traffic holding in position",
    "extend downwind",
    "i'll call your base",
    "turn base",
    "expect further clearance",
    "advise clear of weather",
    "radar contact",
    "ready to taxi",
    "ready to copy",
    "with information",
    "inbound for landing",
    "mile final runway",
    "request vectors",
    "proceeding direct",
    "climbing to",
    "stop immediately",
    "i say again",
];

/// The most words one clip holds.
pub const LONGEST_PHRASE: usize = 8;

/// Clip key suffixes for a word or phrase said at the end of a sentence (a
/// fall in pitch), or before a comma (held up, to go on): "cleared to land
/// ." and "runway two seven left ,". Without one, the clip is from the middle
/// of a sentence.
pub const AT_END: &str = " .";
pub const BEFORE_COMMA: &str = " ,";

/// A line's words grouped into clips: the longest run that `has` a clip,
/// up to LONGEST_PHRASE words and never across a pause, else the word on its
/// own. Pauses are kept as they are.
pub fn units(words: &[String], has: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let run = words[i..].iter().take_while(|w| *w != PAUSE && *w != STOP).count();
        let longest = run.min(LONGEST_PHRASE);
        let phrase = (2..=longest).rev().map(|n| (words[i..i + n].join(" "), n)).find(|(p, _)| has(p));
        match phrase {
            Some((phrase, n)) => {
                out.push(phrase);
                i += n;
            }
            None => {
                out.push(words[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// Each clip before a comma, or at the end of a sentence, as said there,
/// where `has` a clip for it.
pub fn with_endings(units: Vec<String>, has: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::with_capacity(units.len());
    for (i, unit) in units.iter().enumerate() {
        if unit == PAUSE || unit == STOP {
            out.push(unit.clone());
            continue;
        }
        let suffix = match units.get(i + 1).map(String::as_str) {
            Some(PAUSE) => BEFORE_COMMA,
            Some(STOP) | None => AT_END,
            Some(_) => "",
        };
        let ended = format!("{unit}{suffix}");
        out.push(if !suffix.is_empty() && has(&ended) { ended } else { unit.clone() });
    }
    out
}

/// The words of a clip key, without its ending.
pub fn key_words(key: &str) -> &str {
    key.strip_suffix(AT_END).or_else(|| key.strip_suffix(BEFORE_COMMA)).unwrap_or(key)
}

/// Words that come before a number read digit by digit, even a round one.
const DIGIT_CONTEXT: [&str; 4] = ["squawk", "code", "heading", "runway"];

/// The spoken words of a radio line. `callsigns` are the aircraft about, so
/// registrations that don't look like one (a Canadian CGABC) are still read
/// as callsigns.
pub fn spoken(text: &str, callsigns: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut previous = String::new();
    for raw in text.split_whitespace() {
        let trailing: String = raw.chars().rev().take_while(|c| matches!(c, ',' | '.' | '?' | '!' | ':' | ';')).collect();
        let token = raw.trim_end_matches([',', '.', '?', '!', ':', ';']);
        for part in token.split('-').filter(|p| !p.is_empty()) {
            let words = token_words(part, &previous, callsigns);
            if let Some(last) = words.last() {
                previous = last.clone();
            }
            out.extend(words);
        }
        if trailing.contains(['.', '?', '!']) {
            push_pause(&mut out, STOP);
        } else if !trailing.is_empty() {
            push_pause(&mut out, PAUSE);
        }
    }
    while out.last().is_some_and(|w| w == PAUSE || w == STOP) {
        out.pop();
    }
    out
}

fn push_pause(out: &mut Vec<String>, pause: &str) {
    match out.last().map(String::as_str) {
        None => {}
        Some(PAUSE) if pause == STOP => *out.last_mut().expect("checked") = STOP.into(),
        Some(PAUSE | STOP) => {}
        Some(_) => out.push(pause.into()),
    }
}

fn token_words(token: &str, previous: &str, callsigns: &[&str]) -> Vec<String> {
    let token = token.trim_matches(|c: char| matches!(c, '(' | ')' | '"'));
    if token.is_empty() {
        return Vec::new();
    }
    if let Some((_, name)) = AIRPORT_NAMES.iter().find(|(code, _)| *code == token) {
        return name.split(' ').map(str::to_string).collect();
    }
    if callsigns.contains(&token) || looks_like_callsign(token) {
        return callsign(token);
    }
    let digits_only = token.chars().all(|c| c.is_ascii_digit() || c == ',');
    if digits_only && token.chars().any(|c| c.is_ascii_digit()) {
        return number(token, previous);
    }
    if let Some(rest) = token.strip_prefix("FL").filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())) {
        let mut words = vec!["flight".to_string(), "level".to_string()];
        words.extend(rest.chars().map(digit));
        return words;
    }
    if let Some(words) = decimal(token) {
        return words;
    }
    if token == "I" {
        return vec!["i".to_string()];
    }
    if let Some((_, word)) = ABBREVIATIONS.iter().find(|(a, _)| *a == token) {
        return vec![word.to_string()];
    }
    if is_runway(token) {
        return runway(token);
    }
    let letters = token.chars().all(|c| c.is_ascii_alphabetic() || c == '\'');
    if letters {
        let upper = token.chars().all(|c| c.is_ascii_uppercase());
        return match token.len() {
            // A taxiway, or a letter on its own.
            1 if upper => vec![phonetic(token.chars().next().expect("one"))],
            // A fix or procedure name, said as a word.
            _ => vec![token.to_lowercase()],
        };
    }
    // A fix or procedure with a number ("ELVIS2"), a taxiway or gate ("B5",
    // "G12"), or a type designator ("B738").
    let split = token.find(|c: char| c.is_ascii_digit()).unwrap_or(token.len());
    let (head, tail) = token.split_at(split);
    let mut words = Vec::new();
    if head.len() >= 3 && tail.chars().all(|c| c.is_ascii_digit()) {
        words.push(head.to_lowercase());
        words.extend(tail.chars().map(digit));
        return words;
    }
    for c in token.chars() {
        if c.is_ascii_digit() {
            words.push(digit(c));
        } else if c.is_ascii_alphabetic() {
            words.push(phonetic(c));
        }
    }
    words
}

/// "NVA101", or an N-number such as "N3969U".
fn looks_like_callsign(token: &str) -> bool {
    let b = token.as_bytes();
    let airline = b.len() > 3 && b[..3].iter().all(u8::is_ascii_uppercase) && b[3..].iter().all(u8::is_ascii_digit);
    let n_number = b.len() >= 3
        && b[0] == b'N'
        && b[1].is_ascii_digit()
        && b[1] != b'0'
        && b[1..].iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    airline || n_number
}

/// A callsign as spoken: telephony and the flight number, or a registration
/// in the phonetic alphabet.
pub fn callsign(callsign: &str) -> Vec<String> {
    let b = callsign.as_bytes();
    let airline = b.len() > 3 && b[..3].iter().all(u8::is_ascii_uppercase) && b[3..].iter().all(u8::is_ascii_digit);
    if airline && let Some((_, name)) = TELEPHONY.iter().find(|(code, _)| *code == &callsign[..3]) {
        let mut words: Vec<String> = name.split(' ').map(str::to_string).collect();
        words.extend(callsign[3..].chars().map(digit));
        return words;
    }
    callsign
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| if c.is_ascii_digit() { digit(c) } else { phonetic(c) })
        .collect()
}

/// "27L", "09R", "4".
fn is_runway(token: &str) -> bool {
    let (number, side) = token.split_at(token.len() - 1);
    matches!(side, "L" | "R" | "C") && (1..=2).contains(&number.len()) && number.chars().all(|c| c.is_ascii_digit())
}

fn runway(token: &str) -> Vec<String> {
    let (number, side) = token.split_at(token.len() - 1);
    let mut words: Vec<String> = number.chars().map(digit).collect();
    words.push(
        match side {
            "L" => "left",
            "R" => "right",
            _ => "center",
        }
        .to_string(),
    );
    words
}

/// An altitude in thousands and hundreds ("three thousand five hundred",
/// "one zero thousand"), anything else digit by digit.
fn number(token: &str, previous: &str) -> Vec<String> {
    let plain: String = token.chars().filter(char::is_ascii_digit).collect();
    let value: u32 = plain.parse().unwrap_or(0);
    let digits_wanted = DIGIT_CONTEXT.contains(&previous);
    let altitude = !digits_wanted && plain.len() >= 3 && value >= 1000 && value.is_multiple_of(100);
    let altitude = altitude || (!digits_wanted && token.contains(','));
    if !altitude {
        return plain.chars().map(digit).collect();
    }
    let thousands = value / 1000;
    let hundreds = value % 1000 / 100;
    let mut words: Vec<String> = Vec::new();
    if thousands > 0 {
        words.extend(thousands.to_string().chars().map(digit));
        words.push("thousand".into());
    }
    if hundreds > 0 {
        words.push(DIGITS[hundreds as usize].into());
        words.push("hundred".into());
    }
    if words.is_empty() {
        words.push("zero".into());
    }
    words
}

/// "1.5" as "one point five".
fn decimal(token: &str) -> Option<Vec<String>> {
    let (whole, part) = token.split_once('.')?;
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !numeric(whole) || !numeric(part) {
        return None;
    }
    let mut words: Vec<String> = whole.chars().map(digit).collect();
    words.push("point".into());
    words.extend(part.chars().map(digit));
    Some(words)
}

fn digit(c: char) -> String {
    DIGITS[c.to_digit(10).unwrap_or(0) as usize].to_string()
}

fn phonetic(c: char) -> String {
    let i = (c.to_ascii_uppercase() as u8).saturating_sub(b'A') as usize;
    PHONETIC.get(i).copied().unwrap_or("").to_string()
}

/// A word spelled out in the phonetic alphabet, when there's no clip for it.
pub fn spelled(word: &str) -> Vec<String> {
    word.chars().filter(char::is_ascii_alphanumeric).map(|c| if c.is_ascii_digit() { digit(c) } else { phonetic(c) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(text: &str) -> String {
        spoken(text, &["CGABC"]).join(" ")
    }

    #[test]
    fn callsigns() {
        assert_eq!(say("NVA101"), "nova one zero one");
        assert_eq!(say("FDX1234"), "fedex one two three four");
        assert_eq!(say("SPT12"), "servicios sup one two");
        assert_eq!(say("N3969U"), "november three niner six niner uniform");
        assert_eq!(say("CGABC"), "charlie golf alfa bravo charlie");
        // An airline the game doesn't know is spelled.
        assert_eq!(say("XYZ12"), "x-ray yankee zulu one two");
    }

    #[test]
    fn runways_altitudes_and_numbers() {
        assert_eq!(say("runway 27L"), "runway two seven left");
        assert_eq!(say("runway 09R, cleared to land."), "runway zero niner right , cleared to land");
        assert_eq!(say("maintain 3,000"), "maintain three thousand");
        assert_eq!(say("climbing to 12,000"), "climbing to one two thousand");
        assert_eq!(say("at 2,500"), "at two thousand five hundred");
        assert_eq!(say("climbing to 3000."), "climbing to three thousand");
        assert_eq!(say("FL230"), "flight level two three zero");
        assert_eq!(say("heading 240"), "heading two four zero");
        assert_eq!(say("heading 100"), "heading one zero zero");
        assert_eq!(say("squawk 1200"), "squawk one two zero zero");
        assert_eq!(say("wind 347 at 9"), "wind three four seven at niner");
        assert_eq!(say("10 mile final"), "one zero mile final");
        assert_eq!(say("1.5 miles"), "one point five miles");
    }

    #[test]
    fn taxiways_fixes_and_words() {
        assert_eq!(say("taxi via B, B5, hold short of runway 18C."), "taxi via bravo , bravo five , hold short of runway one eight center");
        assert_eq!(say("at G12"), "at golf one two");
        assert_eq!(say("proceed direct ELVIS"), "proceed direct elvis");
        assert_eq!(say("the ELVIS2 departure"), "the elvis two departure");
        assert_eq!(say("go-around, VFR, ILS"), "go around , vfr , ils");
        assert_eq!(say("we'll divert."), "we'll divert");
        assert_eq!(say("stop immediately, I say again"), "stop immediately , i say again");
        assert_eq!(say("cleared to KORD airport"), "cleared to o'hare airport");
        assert_eq!(say("traffic, A320, 5 mile final"), "traffic , alfa three two zero , five mile final");
    }

    #[test]
    fn units_take_the_longest_phrase_and_never_cross_a_pause() {
        let has = |k: &str| ["cleared to land", "two seven", "two seven left"].contains(&k);
        let words = spoken("runway 27L, cleared to land.", &[]);
        assert_eq!(units(&words, has), ["runway", "two seven left", ",", "cleared to land"]);
        let words = spoken("27, left", &[]);
        assert_eq!(units(&words, has), ["two seven", ",", "left"]);
    }

    #[test]
    fn endings_go_before_a_comma_and_at_the_end() {
        let has = |k: &str| ["runway ,", "land .", "taxi ."].contains(&k);
        let u: Vec<String> = ["runway", ",", "cleared", "to", "land"].iter().map(|s| s.to_string()).collect();
        assert_eq!(with_endings(u, has), ["runway ,", ",", "cleared", "to", "land ."]);
        assert_eq!(key_words("land ."), "land");
        assert_eq!(key_words("runway ,"), "runway");
        assert_eq!(key_words("runway"), "runway");
    }

    #[test]
    fn spelling() {
        assert_eq!(spelled("XQ1").join(" "), "x-ray quebec one");
    }
}
