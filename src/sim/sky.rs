//! Cloud and visibility: how they change through a shift, how they're
//! reported, and what they rule out. Thunderstorms are in `storms`.

use super::weather::WeatherKind;

/// Clear enough for VFR: ceiling and visibility at least this, feet and SM.
pub const VFR_CEILING_FT: f32 = 1000.0;
pub const VFR_VISIBILITY_SM: f32 = 3.0;
/// Low visibility procedures below this ceiling or visibility.
pub const LVP_CEILING_FT: f32 = 800.0;
pub const LVP_VISIBILITY_SM: f32 = 2.0;
/// Category I ILS minimums: decision height 200 ft, visibility 1/2 SM.
pub const CAT1_CEILING_FT: f32 = 200.0;
pub const CAT1_VISIBILITY_SM: f32 = 0.5;
/// A ceiling this high or more isn't reported as one: the sky is clear.
const NO_CEILING_FT: f32 = 12000.0;
/// Visibility in a clear sky, SM.
const CLEAR_SM: f32 = 10.0;

/// The ceiling (lowest broken or overcast layer, feet above the field) and
/// the visibility (statute miles).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    pub ceiling: Option<f32>,
    pub visibility: f32,
}

impl Sky {
    pub const CLEAR: Sky = Sky { ceiling: None, visibility: CLEAR_SM };

    fn ceiling_or_top(&self) -> f32 {
        self.ceiling.unwrap_or(NO_CEILING_FT)
    }

    fn below(&self, ceiling: f32, visibility: f32) -> bool {
        self.ceiling_or_top() < ceiling || self.visibility < visibility
    }

    /// Too low for VFR: instrument meteorological conditions.
    pub fn imc(&self) -> bool {
        self.below(VFR_CEILING_FT, VFR_VISIBILITY_SM)
    }

    /// Low visibility procedures: the ILS critical area is protected.
    pub fn lvp(&self) -> bool {
        self.below(LVP_CEILING_FT, LVP_VISIBILITY_SM)
    }

    /// Below Category I minimums: only aircraft that can autoland get in.
    pub fn below_cat1(&self) -> bool {
        self.below(CAT1_CEILING_FT, CAT1_VISIBILITY_SM)
    }

    /// The flight category, as a briefing names it.
    pub fn category(&self) -> &'static str {
        if self.below(500.0, 1.0) {
            "LIFR"
        } else if self.below(1000.0, 3.0) {
            "IFR"
        } else if self.below(3000.0, 5.0) {
            "MVFR"
        } else {
            "VFR"
        }
    }

    /// Fog or mist, by visibility.
    fn obscuration(&self) -> Option<(&'static str, &'static str)> {
        if self.visibility < 0.625 {
            Some(("fog", "FG"))
        } else if self.visibility < 5.0 {
            Some(("mist", "BR"))
        } else {
            None
        }
    }

    fn cover(&self) -> (&'static str, &'static str) {
        if self.imc() { ("overcast", "OVC") } else { ("broken", "BKN") }
    }

    /// "Visibility 1/2, fog. Ceiling 200 overcast." for the ATIS.
    pub fn spoken(&self) -> String {
        let mut text = format!("Visibility {}", visibility_text(self.visibility));
        if let Some((words, _)) = self.obscuration() {
            text += &format!(", {words}");
        }
        text += ". ";
        match self.ceiling {
            Some(c) => text += &format!("Ceiling {} {}.", ceiling_text(c), self.cover().0),
            None => text += "Sky clear.",
        }
        text
    }

    /// "VIS 1/2 FG OVC002" for the status line.
    pub fn short(&self) -> String {
        let mut text = format!("VIS {}", visibility_text(self.visibility));
        if let Some((_, code)) = self.obscuration() {
            text += &format!(" {code}");
        }
        match self.ceiling {
            Some(c) => text += &format!(" {}{:03.0}", self.cover().1, report_ceiling(c) / 100.0),
            None => text += " SKC",
        }
        text
    }

    /// Part way from `self` to `to`.
    fn lerp(&self, to: &Sky, t: f32) -> Sky {
        let ceiling = self.ceiling_or_top() + (to.ceiling_or_top() - self.ceiling_or_top()) * t;
        Sky {
            ceiling: (ceiling < NO_CEILING_FT - 1.0).then_some(ceiling),
            visibility: self.visibility + (to.visibility - self.visibility) * t,
        }
    }
}

/// Visibilities as they're reported, SM.
const REPORTED_SM: [(f32, &str); 15] = [
    (0.25, "1/4"),
    (0.5, "1/2"),
    (0.75, "3/4"),
    (1.0, "1"),
    (1.25, "1 1/4"),
    (1.5, "1 1/2"),
    (1.75, "1 3/4"),
    (2.0, "2"),
    (2.5, "2 1/2"),
    (3.0, "3"),
    (4.0, "4"),
    (5.0, "5"),
    (6.0, "6"),
    (7.0, "7"),
    (10.0, "10"),
];

/// The visibility as reported: the reportable value at or just below it.
pub fn visibility_text(sm: f32) -> &'static str {
    REPORTED_SM.iter().rev().find(|(v, _)| sm >= *v - 0.01).map_or("less than 1/4", |(_, t)| t)
}

/// Ceilings are reported to the 100 ft below 5,000, then to the 500.
fn report_ceiling(feet: f32) -> f32 {
    let step = if feet < 5000.0 { 100.0 } else { 500.0 };
    ((feet / step).floor() * step).max(100.0)
}

fn ceiling_text(feet: f32) -> String {
    super::clearance::thousands(report_ceiling(feet))
}

/// One change in the plan: from `at`, over `over` seconds, to `to`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Change {
    at: f32,
    over: f32,
    to: Sky,
}

/// How the sky will go through a shift, rolled at the start.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyPlan {
    start: Sky,
    changes: Vec<Change>,
    /// A line of storms comes through (Variable) or storms all shift (Storms).
    pub storms: bool,
}

/// Seconds in a minute, for readable plans.
const MIN: f32 = 60.0;

impl SkyPlan {
    /// The plan for a shift of `shift` seconds.
    pub fn roll(kind: WeatherKind, seed: u64, shift: f32) -> SkyPlan {
        let clear = SkyPlan { start: Sky::CLEAR, changes: Vec::new(), storms: false };
        if kind == WeatherKind::Good {
            return clear;
        }
        let mut rng = fastrand::Rng::with_seed(seed ^ 0x534B_5953_4B59_0001);
        let mut range = |lo: f32, hi: f32| lo + rng.f32() * (hi - lo);
        // A point in the shift, as a share of it.
        let when = |share: f32| share * shift;
        match kind {
            WeatherKind::Good => clear,
            WeatherKind::Variable => {
                let roll = range(0.0, 1.0);
                let vmc = Sky { ceiling: Some(range(3500.0, 8000.0)), visibility: range(6.0, 10.0) };
                if roll < 0.5 {
                    let later = Sky { ceiling: Some(range(3500.0, 8000.0)), visibility: range(6.0, 10.0) };
                    let at = when(range(0.3, 0.6));
                    SkyPlan { start: vmc, changes: vec![Change { at, over: 15.0 * MIN, to: later }], storms: false }
                } else if roll < 0.75 {
                    // Lowering through the shift: marginal, then IFR.
                    let start = Sky { ceiling: Some(range(3000.0, 4500.0)), visibility: 7.0 };
                    let marginal = Sky { ceiling: Some(range(1500.0, 2500.0)), visibility: range(4.0, 6.0) };
                    let low = Sky { ceiling: Some(range(500.0, 900.0)), visibility: range(1.5, 3.0) };
                    let first = when(range(0.25, 0.4));
                    let second = when(range(0.55, 0.75));
                    let changes = vec![
                        Change { at: first, over: 10.0 * MIN, to: marginal },
                        Change { at: second, over: 10.0 * MIN, to: low },
                    ];
                    SkyPlan { start, changes, storms: false }
                } else if roll < 0.9 {
                    let at = when(range(0.3, 0.55));
                    SkyPlan { start: vmc, changes: fog_bank(&mut range, at), storms: false }
                } else {
                    SkyPlan { start: vmc, changes: Vec::new(), storms: true }
                }
            }
            WeatherKind::LowIfr => {
                let start = Sky { ceiling: Some(range(200.0, 600.0)), visibility: range(1.0, 2.0) };
                let mut changes = Vec::new();
                if range(0.0, 1.0) < 0.5 {
                    // Fog thickens below minimums for a spell, then goes back.
                    let at = when(range(0.3, 0.6));
                    let thick = Sky { ceiling: Some(range(100.0, 150.0)), visibility: range(0.25, 0.4) };
                    let hold = range(8.0, 15.0) * MIN;
                    changes.push(Change { at, over: 4.0 * MIN, to: thick });
                    changes.push(Change { at: at + 4.0 * MIN + hold, over: 6.0 * MIN, to: start });
                }
                SkyPlan { start, changes, storms: false }
            }
            WeatherKind::Storms => {
                let start = Sky { ceiling: Some(range(3000.0, 6000.0)), visibility: range(5.0, 10.0) };
                SkyPlan { start, changes: Vec::new(), storms: true }
            }
        }
    }

    /// The same sky all shift: for tests and lessons.
    pub fn fixed(sky: Sky) -> SkyPlan {
        SkyPlan { start: sky, changes: Vec::new(), storms: false }
    }

    /// The sky at time `t`.
    pub fn at(&self, t: f32) -> Sky {
        let mut sky = self.start;
        for change in &self.changes {
            if t < change.at {
                break;
            }
            let progress = if change.over > 0.0 { ((t - change.at) / change.over).min(1.0) } else { 1.0 };
            sky = sky.lerp(&change.to, progress);
            if progress < 1.0 {
                // Mid-change: what's later hasn't started.
                break;
            }
        }
        sky
    }

    /// A fog bank somewhere in the plan.
    #[cfg(test)]
    fn has_fog(&self) -> bool {
        self.changes.iter().any(|c| c.to.visibility < 1.0)
    }
}

/// A fog bank rolling in at `at`: down to under a mile in five minutes,
/// staying a while, then lifting.
fn fog_bank(range: &mut impl FnMut(f32, f32) -> f32, at: f32) -> Vec<Change> {
    let fog = Sky { ceiling: Some(range(100.0, 400.0)), visibility: range(0.25, 1.0) };
    let hold = range(10.0, 20.0) * MIN;
    let after = Sky { ceiling: Some(range(800.0, 1500.0)), visibility: range(3.0, 5.0) };
    vec![Change { at, over: 5.0 * MIN, to: fog }, Change { at: at + 5.0 * MIN + hold, over: 10.0 * MIN, to: after }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_sit_where_the_rules_put_them() {
        let sky = |c: f32, v: f32| Sky { ceiling: Some(c), visibility: v };
        assert!(!sky(1000.0, 3.0).imc());
        assert!(sky(999.0, 3.0).imc() && sky(1000.0, 2.9).imc());
        assert!(!sky(800.0, 2.0).lvp());
        assert!(sky(799.0, 5.0).lvp() && sky(3000.0, 1.9).lvp());
        assert!(!sky(200.0, 0.5).below_cat1());
        assert!(sky(199.0, 1.0).below_cat1() && sky(1000.0, 0.45).below_cat1());
        assert!(!Sky::CLEAR.imc());
    }

    #[test]
    fn reports_read_like_an_atis() {
        let fog = Sky { ceiling: Some(230.0), visibility: 0.55 };
        assert_eq!(fog.spoken(), "Visibility 1/2, fog. Ceiling 200 overcast.");
        assert_eq!(fog.short(), "VIS 1/2 FG OVC002");
        let fine = Sky { ceiling: Some(6200.0), visibility: 10.0 };
        assert_eq!(fine.spoken(), "Visibility 10. Ceiling 6,000 broken.");
        assert_eq!(Sky::CLEAR.short(), "VIS 10 SKC");
        assert_eq!(visibility_text(1.4), "1 1/4");
        assert_eq!(visibility_text(0.1), "less than 1/4");
    }

    #[test]
    fn good_weather_never_changes() {
        let plan = SkyPlan::roll(WeatherKind::Good, 3, 3600.0);
        assert_eq!(plan.at(0.0), Sky::CLEAR);
        assert_eq!(plan.at(3000.0), Sky::CLEAR);
    }

    #[test]
    fn low_ifr_is_imc_all_shift_and_any_fog_spell_goes_below_minimums_and_back() {
        for seed in 0..40 {
            let plan = SkyPlan::roll(WeatherKind::LowIfr, seed, 3600.0);
            let mut below = false;
            for minute in 0..60 {
                let sky = plan.at(minute as f32 * MIN);
                assert!(sky.imc() && sky.lvp(), "seed {seed} minute {minute}: {sky:?}");
                below |= sky.below_cat1();
            }
            assert_eq!(below, plan.has_fog(), "seed {seed}");
            assert!(!plan.at(3599.0).below_cat1(), "seed {seed}");
        }
    }

    #[test]
    fn variable_weather_brings_fog_in_some_shifts_and_lifts_it() {
        let mut fog = 0;
        for seed in 0..200 {
            let plan = SkyPlan::roll(WeatherKind::Variable, seed, 3600.0);
            assert!(!plan.at(0.0).imc(), "seed {seed} starts {:?}", plan.at(0.0));
            if plan.has_fog() {
                fog += 1;
                let worst = (0..60).map(|m| plan.at(m as f32 * MIN).visibility).fold(f32::MAX, f32::min);
                assert!(worst < 1.01, "seed {seed}");
            }
        }
        assert!((15..=50).contains(&fog), "{fog} fog banks in 200 shifts");
    }

    #[test]
    fn changes_ease_in() {
        let plan = SkyPlan {
            start: Sky { ceiling: Some(4000.0), visibility: 10.0 },
            changes: vec![Change { at: 100.0, over: 100.0, to: Sky { ceiling: Some(2000.0), visibility: 2.0 } }],
            storms: false,
        };
        assert_eq!(plan.at(50.0).visibility, 10.0);
        assert!((plan.at(150.0).visibility - 6.0).abs() < 0.01);
        assert_eq!(plan.at(250.0).ceiling, Some(2000.0));
    }
}
