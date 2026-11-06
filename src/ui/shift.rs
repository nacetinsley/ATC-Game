//! The shift length the menu offers: an hour unless changed, and any whole
//! number of minutes from one to twelve hours.

/// The menu's starting choice, minutes.
pub const DEFAULT_MINUTES: u32 = 60;
pub const MIN_MINUTES: u32 = 1;
pub const MAX_MINUTES: u32 = 12 * 60;
/// One-click lengths on the menu, minutes.
pub const PRESETS: [u32; 6] = [15, 30, 60, 90, 120, 240];
/// Digits typed within this long of each other make one number, seconds.
const TYPING_GAP_S: f64 = 1.5;
/// Above this the coarse step grows, minutes.
const FINE_UP_TO: u32 = 120;

/// One step longer or shorter. A coarse step lands on the next multiple of 5
/// minutes (15 above two hours); a fine step is one minute.
pub fn step(minutes: u32, longer: bool, fine: bool) -> u32 {
    let size = |m: u32| match (fine, m < FINE_UP_TO) {
        (true, _) => 1,
        (false, true) => 5,
        (false, false) => 15,
    };
    let next = if longer {
        let size = size(minutes);
        (minutes / size + 1) * size
    } else {
        let size = size(minutes.saturating_sub(1));
        minutes.saturating_sub(1) / size * size
    };
    next.clamp(MIN_MINUTES, MAX_MINUTES)
}

/// How the menu writes a length: "45 min", "1 h", "2 h 15 min".
pub fn label(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Minutes typed on the menu's number keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typed {
    pub value: u32,
    /// When the last digit was typed, seconds.
    pub at: f64,
}

impl Typed {
    /// Adds a digit typed at `now`: it extends the number if it follows
    /// quickly and still fits, and starts a new one otherwise.
    pub fn digit(previous: Option<Typed>, digit: u32, now: f64) -> Typed {
        let value = match previous {
            Some(p) if now - p.at < TYPING_GAP_S && p.value * 10 + digit <= MAX_MINUTES => {
                p.value * 10 + digit
            }
            _ => digit,
        };
        Typed { value, at: now }
    }

    /// The shift length this sets, minutes.
    pub fn minutes(self) -> u32 {
        self.value.clamp(MIN_MINUTES, MAX_MINUTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_steps_land_on_round_numbers() {
        assert_eq!(step(60, true, false), 65);
        assert_eq!(step(60, false, false), 55);
        assert_eq!(step(62, true, false), 65);
        assert_eq!(step(62, false, false), 60);
        assert_eq!(step(115, true, false), 120);
        assert_eq!(step(120, true, false), 135);
        assert_eq!(step(135, false, false), 120);
        assert_eq!(step(120, false, false), 115);
    }

    #[test]
    fn fine_steps_are_a_minute() {
        assert_eq!(step(60, true, true), 61);
        assert_eq!(step(60, false, true), 59);
    }

    #[test]
    fn steps_stop_at_the_ends() {
        assert_eq!(step(5, false, false), MIN_MINUTES);
        assert_eq!(step(MIN_MINUTES, false, true), MIN_MINUTES);
        assert_eq!(step(MAX_MINUTES, true, false), MAX_MINUTES);
        assert_eq!(step(MAX_MINUTES, true, true), MAX_MINUTES);
    }

    #[test]
    fn labels_read_as_hours_and_minutes() {
        assert_eq!(label(45), "45 min");
        assert_eq!(label(60), "1 h");
        assert_eq!(label(135), "2 h 15 min");
    }

    #[test]
    fn typed_digits_make_one_number_until_a_pause() {
        let t = Typed::digit(None, 4, 0.0);
        let t = Typed::digit(Some(t), 5, 0.5);
        assert_eq!(t.minutes(), 45);
        let t = Typed::digit(Some(t), 9, 3.0);
        assert_eq!(t.minutes(), 9, "a pause starts a new number");
    }

    #[test]
    fn typed_numbers_stay_in_range() {
        let t = Typed::digit(None, 0, 0.0);
        assert_eq!(t.minutes(), MIN_MINUTES);
        // 720 fits; a fourth digit would not, so it starts again.
        let mut t = None;
        for (i, d) in [7, 2, 0, 5].into_iter().enumerate() {
            t = Some(Typed::digit(t, d, i as f64 * 0.1));
        }
        assert_eq!(t.unwrap().minutes(), 5);
        let t = Typed::digit(None, 9, 0.0);
        let t = Typed::digit(Some(t), 9, 0.1);
        let t = Typed::digit(Some(t), 9, 0.2);
        assert_eq!(t.value, 9, "999 is over the limit, so the 9 starts again");
    }
}
