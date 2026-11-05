//! Aircraft registrations used as radio callsigns by general aviation.
//!
//! U.S. registrations follow 14 CFR 47.15: "N" then one to five characters.
//! The first is a digit 1-9, any letters come last (at most two), and the
//! letters I and O are never used. One- and two-character numbers are
//! reserved or special-request, so traffic doesn't use them.
//!
//! Callsigns are written the way they're filed and shown on radar: no hyphen,
//! so Canadian C-GABC appears as CGABC.

/// Letters allowed in an N-number: A-Z without I and O.
const N_LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Shape of an N-number after the "N": digits then letters, and how often it's issued.
const N_SHAPES: [(usize, usize, u32); 6] = [
    (5, 0, 30), // N12345
    (3, 2, 30), // N123AB
    (4, 1, 20), // N1234A
    (4, 0, 10), // N1234
    (3, 1, 5),  // N123A
    (2, 2, 5),  // N12AB
];

/// Foreign registrations seen at U.S. business airports: (prefix choices, letters after, weight).
const FOREIGN: [(&[&str], usize, u32); 8] = [
    (&["CF", "CG"], 3, 5),           // Canada, C-Fxxx / C-Gxxx
    (&["XA", "XB"], 3, 3),           // Mexico
    (&["G"], 4, 1),                  // United Kingdom
    (&["DA", "DB", "DC", "DI"], 3, 1), // Germany
    (&["HB"], 3, 1),                 // Switzerland
    (&["VPC"], 2, 1),                // Cayman Islands
    (&["VPB"], 2, 1),                // Bermuda
    (&["PR", "PP", "PS"], 3, 1),     // Brazil
];

fn pick_weighted<T: Copy>(rng: &mut fastrand::Rng, items: &[T], weight: impl Fn(&T) -> u32) -> T {
    let total: u32 = items.iter().map(&weight).sum();
    let mut roll = rng.u32(0..total);
    for item in items {
        let w = weight(item);
        if roll < w {
            return *item;
        }
        roll -= w;
    }
    unreachable!("roll is below the total weight")
}

fn letters(rng: &mut fastrand::Rng, alphabet: &[u8], count: usize) -> String {
    (0..count).map(|_| alphabet[rng.usize(0..alphabet.len())] as char).collect()
}

/// A random U.S. registration such as N4521K or N88TW.
pub fn n_number(rng: &mut fastrand::Rng) -> String {
    let (digits, tail, _) = pick_weighted(rng, &N_SHAPES, |s| s.2);
    let mut out = String::from("N");
    out.push(char::from(b'0' + rng.u8(1..10)));
    for _ in 1..digits {
        out.push(char::from(b'0' + rng.u8(0..10)));
    }
    out.push_str(&letters(rng, N_LETTERS, tail));
    out
}

/// A random foreign registration in filed form, such as CGLRX or XBNGA.
pub fn foreign(rng: &mut fastrand::Rng) -> String {
    let (prefixes, count, _) = pick_weighted(rng, &FOREIGN, |f| f.2);
    let prefix = prefixes[rng.usize(0..prefixes.len())];
    format!("{prefix}{}", letters(rng, LETTERS, count))
}

/// True if `s` is a valid U.S. registration under 14 CFR 47.15.
pub fn is_valid_n_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('N') else { return false };
    let bytes = rest.as_bytes();
    if bytes.is_empty() || bytes.len() > 5 || !(b'1'..=b'9').contains(&bytes[0]) {
        return false;
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    let tail = &bytes[digits..];
    tail.len() <= 2 && tail.iter().all(|b| N_LETTERS.contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_n_numbers_are_legal_and_varied() {
        let mut rng = fastrand::Rng::with_seed(7);
        let mut shapes = std::collections::BTreeSet::new();
        for _ in 0..5000 {
            let n = n_number(&mut rng);
            assert!(is_valid_n_number(&n), "{n}");
            assert!(n.len() >= 4, "short numbers are reserved: {n}");
            shapes.insert(n.chars().map(|c| if c.is_ascii_digit() { '9' } else { 'A' }).collect::<String>());
        }
        assert_eq!(shapes.len(), N_SHAPES.len(), "{shapes:?}");
    }

    #[test]
    fn the_rules_reject_what_47_15_forbids() {
        for good in ["N1", "N12345", "N1234Z", "N123AB", "N88TW"] {
            assert!(is_valid_n_number(good), "{good}");
        }
        for bad in ["N", "N0123", "N123456", "N12ABC", "N12A3", "N123IO", "N12O", "C123", "N-123"] {
            assert!(!is_valid_n_number(bad), "{bad}");
        }
    }

    #[test]
    fn foreign_registrations_have_no_hyphen_and_never_start_with_n() {
        let mut rng = fastrand::Rng::with_seed(3);
        for _ in 0..1000 {
            let r = foreign(&mut rng);
            assert!(!r.starts_with('N') && !r.contains('-'), "{r}");
            assert!((4..=6).contains(&r.len()), "{r}");
            assert!(r.chars().all(|c| c.is_ascii_uppercase()), "{r}");
        }
    }
}
