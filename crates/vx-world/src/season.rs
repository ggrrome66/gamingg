//! The year, as a pure function of the tick.
//!
//! # Two clocks were already running, and neither knew the month
//!
//! [`crate::weather`] is pure in the tick and reads beautifully — fronts
//! drift, rain falls, and how dry the country is falls out of an integral
//! over the same field. But August and February were the same sky. "It has
//! been dry lately" was a coincidence rather than a season, and lightning was
//! as likely to bite in the wet half of the year as the dry one.
//!
//! Meanwhile the game has counted a twenty-eight day year since stage 23,
//! purely so that villagers have birthdays. This module is the other use for
//! that number: four weeks, four seasons, one week each. A full year is about
//! an hour of play, and one season is exactly one election term — the
//! calendars line up because there is only one calendar.
//!
//! # Nothing is stored, and nothing is edited
//!
//! A season is a *term inside functions that were already pure in the tick*.
//! It changes what the sky rolls and how fast the woods come back; it does
//! not touch a block, and it does not add a byte to any save file. That is
//! deliberate and it is checked: the oracle test in `vx-app` runs the same
//! journal across a year of seasons and demands the same ground hash.
//!
//! # Why the phase and not just the season
//!
//! [`Season::of`] is what the status line wants. Everything else wants
//! [`phase`] — a continuous `0..1` through the year — because a term that
//! snapped on a boundary would put a hard edge in the sky and a hard edge in
//! the tile atlas, and the player would read the edge as a bug. Winter
//! arrives; it does not switch on.

/// Ticks in one game day: twenty minutes at the simulation's 64 Hz.
///
/// The app's own clock owns the day length; this is the same number written
/// where `vx-world` can see it, and a test over in `vx-app` holds the two
/// together so neither can drift.
pub const DAY_TICKS: u64 = 64 * 1_200;

/// Days in a year — the same twenty-eight the roster hands out birthdays in.
pub const YEAR_DAYS: u64 = 28;

/// Ticks in one year.
pub const YEAR_TICKS: u64 = DAY_TICKS * YEAR_DAYS;

/// Days in one season. Seven: four seasons make the year, and one season is
/// one election term.
pub const SEASON_DAYS: u64 = YEAR_DAYS / 4;

/// Ticks in one season.
pub const SEASON_TICKS: u64 = YEAR_TICKS / 4;

/// Which quarter of the year it is.
///
/// The game starts at tick zero in spring, which is the only defensible
/// choice: a new save opens on the season everything is growing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

/// Every season, in the order the year runs them.
pub const SEASONS: [Season; 4] = [
    Season::Spring,
    Season::Summer,
    Season::Autumn,
    Season::Winter,
];

impl Season {
    /// The season a tick falls in.
    pub fn of(tick: u64) -> Season {
        SEASONS[((tick % YEAR_TICKS) / SEASON_TICKS) as usize]
    }

    /// Short enough for a status line.
    pub fn label(self) -> &'static str {
        match self {
            Season::Spring => "SPRING",
            Season::Summer => "SUMMER",
            Season::Autumn => "AUTUMN",
            Season::Winter => "WINTER",
        }
    }

    /// Where this season starts in the year, `0..1` — the value [`phase`]
    /// takes on its first tick.
    pub fn start(self) -> f32 {
        self.index() as f32 / 4.0
    }

    /// Its place in the year, `0` spring through `3` winter.
    pub fn index(self) -> usize {
        match self {
            Season::Spring => 0,
            Season::Summer => 1,
            Season::Autumn => 2,
            Season::Winter => 3,
        }
    }

    /// The season parsed off a word, for the capture flag and the terminal.
    pub fn parse(word: &str) -> Option<Season> {
        SEASONS
            .into_iter()
            .find(|season| season.label().eq_ignore_ascii_case(word.trim()))
    }
}

/// How far through the year it is, `0..1`, continuous.
///
/// Zero is the first morning of spring. This is what every seasonal term
/// downstream reads.
pub fn phase(tick: u64) -> f32 {
    (tick % YEAR_TICKS) as f32 / YEAR_TICKS as f32
}

/// Which day of the year it is, `0..YEAR_DAYS`. For the status line, and for
/// lining a season up against a birthday.
pub fn day_of_year(tick: u64) -> u64 {
    (tick % YEAR_TICKS) / DAY_TICKS
}

/// How warm the year is right now, `-1` deep winter through `+1` high summer.
///
/// A cosine on the phase, offset so the peak lands in the middle of summer
/// rather than on its first tick — a year's warmth lags its calendar, and a
/// term that peaked on a boundary would make the boundary visible.
pub fn warmth(tick: u64) -> f32 {
    // Summer's midpoint is 3/8 of the way through the year, so shift the
    // cosine to peak there.
    let turned = phase(tick) - 0.375;
    (turned * std::f32::consts::TAU).cos()
}

/// How wet the year is right now, `-1` at its driest through `+1` at its
/// wettest. The other side of [`warmth`] — a dry season is a warm one.
///
/// Not simply `-warmth`: the wettest part of the year is the back half of
/// autumn rather than midwinter, which is when it is merely frozen. The lag
/// is a quarter-season, and it is the difference between a country that
/// drains and a country that is one sine wave.
pub fn damp(tick: u64) -> f32 {
    let turned = phase(tick) - 0.375 - 0.5 + 0.0625;
    (turned * std::f32::consts::TAU).cos()
}

/// Is the country in its fire season?
///
/// The dry half of the year with the warmth well up — high summer, and the
/// shoulder of it either side. Nothing reads this to decide anything; the
/// fire model reads [`crate::weather::fuel_moisture`] like it always has.
/// This is the *word* for it, so a status line can warn you.
pub fn fire_season(tick: u64) -> bool {
    warmth(tick) > 0.35
}

/// How fast anything green grows right now, `0` stopped through `1` flat out.
///
/// The growing season: away in spring and summer, slowing through autumn,
/// **stopped** over winter. A raised cosine clamped at zero, so the shoulder
/// seasons are a taper rather than a switch, and the dead of winter really is
/// dead rather than merely slow.
pub fn growth(tick: u64) -> f32 {
    (warmth(tick) * 1.35 + 0.35).clamp(0.0, 1.0)
}

/// Growing ticks banked between two ticks — the integral of [`growth`],
/// normalised so a calendar year banks exactly a calendar year.
///
/// # Why an integral rather than a branch
///
/// The obvious way to stop the woods over winter is to skip the step that
/// advances them. That is wrong here, and quietly: a stand's stage has to be
/// a pure function of `(disturbed_at, tick)` or a reload disagrees with the
/// session that saved it, and "how many times did we happen to call advance
/// while it was warm" is not that function. So the season is a *cost on the
/// age* instead. Ask at any tick, from any tick, on either side of a save,
/// and get the same answer.
///
/// # Why it is normalised
///
/// The same rule the weather keeps: a season **redistributes**, it does not
/// take anything away. A stand still takes the same number of days to come
/// back as it did before there were seasons — it just does none of it in
/// January and twice as much in June. Left unnormalised the woods would
/// simply be half as fast, which is a nerf wearing a calendar's clothes.
///
/// Quantised to whole days, so the arithmetic stays in `u64` and two machines
/// cannot disagree in the last bit of a float.
pub fn grown_between(from: u64, to: u64) -> u64 {
    banked(to).saturating_sub(banked(from.min(to)))
}

/// The calendar tick by which `growing` growing-ticks have been banked since
/// `from` — the inverse of [`grown_between`].
///
/// "This stand is back by the middle of May" is a question the panels and the
/// tests both want to ask, and asking it by walking the calendar a day at a
/// time would be the sort of loop that quietly becomes a frame cost. A binary
/// search over a monotone integral answers it in a few dozen steps.
///
/// Returns the *first* such tick, which matters: the integral is flat all
/// winter, so "when will it be grown" has one answer and it is the spring
/// morning, not any of the frozen days that follow it.
pub fn when_grown(from: u64, growing: u64) -> u64 {
    if growing == 0 {
        return from;
    }
    // A year banks a year, so the answer is inside a few years of the target
    // however unlucky the season it started in. Double out until it is.
    let mut span = growing.max(DAY_TICKS);
    while grown_between(from, from + span) < growing {
        span = span.saturating_mul(2);
    }
    let (mut low, mut high) = (from, from + span);
    while low < high {
        let middle = low + (high - low) / 2;
        if grown_between(from, middle) < growing {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// Growing ticks from the first morning of year zero up to `tick`.
fn banked(tick: u64) -> u64 {
    let scale = DAY_TICKS as f64 * YEAR_DAYS as f64 / year_bank();
    let mut total = (tick / YEAR_TICKS) * YEAR_TICKS;
    let into = tick % YEAR_TICKS;
    let whole = into / DAY_TICKS;
    let mut days = 0.0f64;
    for day in 0..whole {
        days += growth(day * DAY_TICKS + DAY_TICKS / 2) as f64;
    }
    let rest = into % DAY_TICKS;
    if rest > 0 {
        days += growth(whole * DAY_TICKS + rest / 2) as f64 * (rest as f64 / DAY_TICKS as f64);
    }
    total += (days * scale) as u64;
    total
}

/// Growing days in one year, before normalisation. Computed once — it is a
/// property of the curve, not of anything anybody plays.
fn year_bank() -> f64 {
    static BANK: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *BANK.get_or_init(|| {
        (0..YEAR_DAYS)
            .map(|day| growth(day * DAY_TICKS + DAY_TICKS / 2) as f64)
            .sum()
    })
}

/// How turned the leaves are, `0` full green through `1` bare.
///
/// Its own curve rather than a reading of [`warmth`], because leaves do not
/// follow the temperature. They flush hard in the first half of spring, hold
/// green right through the heat, turn over one fast season in autumn, and
/// then stay off the branch all winter — only breaking at the very end of it,
/// which is where the year meets itself and the flush picks the number up.
/// That asymmetry is the whole look of the year: three seasons of change and
/// one long green plateau, rather than a sine wave in a coat.
pub fn leaf_turn(tick: u64) -> f32 {
    let phase = phase(tick);
    if phase < 0.125 {
        // Leaf-out, over the first half of spring: what winter handed over,
        // spent down to nothing.
        BUDDING * (1.0 - phase / 0.125)
    } else if phase < 0.5 {
        // Late spring and all summer: green, and staying green.
        0.0
    } else if phase < 0.75 {
        // Autumn: the turn, over one season and all of it.
        (phase - 0.5) / 0.25
    } else if phase < 0.90 {
        // Bare.
        1.0
    } else {
        // The last of winter, with the buds swelling — this is the value
        // spring's flush starts from, and holding the two equal is what keeps
        // the year continuous where it wraps.
        1.0 - (1.0 - BUDDING) * (phase - 0.90) / 0.10
    }
}

/// How turned the leaves still are when winter hands the year over: the buds
/// have broken but nothing is out yet.
const BUDDING: f32 = 0.85;

#[cfg(test)]
mod tests {
    use super::*;

    /// Derived means derived. Ask twice, get the same year.
    #[test]
    fn the_calendar_is_pure_in_the_tick() {
        for tick in [0u64, 1, DAY_TICKS, YEAR_TICKS - 1, YEAR_TICKS * 9 + 77] {
            assert_eq!(Season::of(tick), Season::of(tick));
            assert_eq!(phase(tick), phase(tick));
            assert_eq!(warmth(tick), warmth(tick));
            // And a year later is the same day of the year.
            assert_eq!(Season::of(tick), Season::of(tick + YEAR_TICKS));
            assert!((phase(tick) - phase(tick + YEAR_TICKS)).abs() < 1.0e-6);
        }
    }

    /// Four equal quarters of the twenty-eight day year, in order, starting
    /// in spring.
    #[test]
    fn the_year_is_four_equal_seasons_of_seven_days() {
        assert_eq!(SEASON_DAYS, 7);
        assert_eq!(SEASON_TICKS * 4, YEAR_TICKS);
        assert_eq!(Season::of(0), Season::Spring);

        let mut counted = [0u64; 4];
        for day in 0..YEAR_DAYS {
            counted[Season::of(day * DAY_TICKS).index()] += 1;
        }
        assert_eq!(counted, [7, 7, 7, 7], "the seasons are not equal quarters");

        // And they run in the order the year runs them.
        for (index, season) in SEASONS.into_iter().enumerate() {
            assert_eq!(Season::of(index as u64 * SEASON_TICKS), season);
            assert_eq!(season.index(), index);
        }
    }

    /// Nothing snaps. Every seasonal term is continuous across every
    /// boundary, including the wrap from winter back into spring — a step
    /// there would show as the sky and the whole country flickering at
    /// midnight on one particular day.
    #[test]
    fn every_seasonal_term_is_continuous_across_the_wrap() {
        /// A named seasonal term, so the loop below can walk all of them.
        type Term = (&'static str, fn(u64) -> f32);
        let terms: [Term; 4] = [
            ("warmth", warmth),
            ("damp", damp),
            ("growth", growth),
            ("leaf_turn", leaf_turn),
        ];
        // Every boundary, plus the wrap, plus a scatter of ordinary hours.
        let edges: Vec<u64> = (0..=4)
            .map(|quarter| quarter * SEASON_TICKS)
            .chain((0..28).map(|day| day * DAY_TICKS))
            .collect();
        for (name, term) in terms {
            for edge in &edges {
                let before = term(edge.saturating_sub(1) + YEAR_TICKS);
                let after = term(edge + YEAR_TICKS);
                assert!(
                    (before - after).abs() < 0.01,
                    "{name} jumps {:.3} at tick {edge}",
                    (before - after).abs()
                );
            }
        }
    }

    /// The shape of the year: summer is the warm end, winter the cold one,
    /// and the shoulders sit between them rather than at an extreme.
    #[test]
    fn summer_is_the_warm_end_and_winter_the_cold_one() {
        let mean = |season: Season| {
            let start = season.index() as u64 * SEASON_TICKS;
            let sum: f32 = (0..SEASON_DAYS)
                .map(|day| warmth(start + day * DAY_TICKS))
                .sum();
            sum / SEASON_DAYS as f32
        };
        let (spring, summer, autumn, winter) = (
            mean(Season::Spring),
            mean(Season::Summer),
            mean(Season::Autumn),
            mean(Season::Winter),
        );
        assert!(summer > spring && summer > autumn, "summer is not the peak");
        assert!(winter < spring && winter < autumn, "winter is not the floor");
        assert!(summer > 0.5 && winter < -0.5, "the year is barely a year");
        // Over a whole year it averages out: a season redistributes the year,
        // it does not tilt it.
        let over_the_year: f32 = (0..YEAR_DAYS).map(|day| warmth(day * DAY_TICKS)).sum();
        assert!(
            (over_the_year / YEAR_DAYS as f32).abs() < 0.05,
            "the year has a thumb on the scale"
        );
    }

    /// Fire season is high summer and its shoulders — a slice of the year,
    /// not most of it and not a single afternoon.
    #[test]
    fn the_fire_season_is_a_season_rather_than_the_whole_year() {
        let days = (0..YEAR_DAYS).filter(|day| fire_season(day * DAY_TICKS)).count();
        assert!(
            (5..=11).contains(&days),
            "the fire season is {days} days of a {YEAR_DAYS} day year"
        );
        // And it is centred on summer.
        assert!(fire_season(SEASON_TICKS + SEASON_TICKS / 2), "midsummer is not fire season");
        assert!(!fire_season(3 * SEASON_TICKS + SEASON_TICKS / 2), "midwinter is fire season");
    }

    /// The woods stop over winter and run in spring — the thing that makes a
    /// burn set in autumn still be there in February.
    #[test]
    fn nothing_grows_over_the_winter() {
        for day in 0..SEASON_DAYS {
            let deep = 3 * SEASON_TICKS + day * DAY_TICKS;
            assert!(growth(deep) < 0.35, "winter day {day} is still growing");
        }
        assert_eq!(growth(3 * SEASON_TICKS + SEASON_TICKS / 2), 0.0, "midwinter grows");
        assert!(growth(SEASON_TICKS + SEASON_TICKS / 2) > 0.95, "midsummer is not growing");
        // A year of growth is a fraction of a year of ticks, but not nothing.
        let banked: f32 = (0..YEAR_DAYS).map(|day| growth(day * DAY_TICKS)).sum();
        let share = banked / YEAR_DAYS as f32;
        assert!((0.35..0.75).contains(&share), "the growing season is {share:.2} of the year");
    }

    /// Leaves hold green through the heat, turn in autumn and stay off until
    /// spring — the asymmetry that makes the year look like a year.
    #[test]
    fn the_leaves_turn_in_autumn_and_come_back_in_spring() {
        let at = |season: Season| leaf_turn(season.index() as u64 * SEASON_TICKS + SEASON_TICKS / 2);
        assert!(at(Season::Spring) < 0.1, "spring is not green");
        assert!(at(Season::Summer) < 0.05, "summer is not green");
        assert!((0.3..0.8).contains(&at(Season::Autumn)), "autumn is not turning");
        assert!(at(Season::Winter) > 0.8, "winter is not bare");
        // The turn only ever goes one way through autumn.
        let mut last = leaf_turn(2 * SEASON_TICKS);
        for step in 1..=SEASON_DAYS {
            let now = leaf_turn(2 * SEASON_TICKS + step * DAY_TICKS);
            assert!(now >= last - 1.0e-6, "the leaves went back to green mid-autumn");
            last = now;
        }
    }

    /// The wet half of the year is the other half from the warm one, and it
    /// lags rather than mirroring — autumn is the wettest, not midwinter.
    #[test]
    fn the_country_is_wettest_in_the_late_autumn() {
        let mut wettest = (f32::MIN, 0u64);
        for day in 0..YEAR_DAYS {
            let value = damp(day * DAY_TICKS);
            if value > wettest.0 {
                wettest = (value, day);
            }
        }
        assert!(
            (17..=23).contains(&wettest.1),
            "the wettest day of the year is day {}",
            wettest.1
        );
        // Broadly the opposite of the warmth, without being its negative.
        for day in 0..YEAR_DAYS {
            let tick = day * DAY_TICKS;
            assert!((damp(tick) + warmth(tick)).abs() < 1.3);
        }
        let mirrored = (0..YEAR_DAYS)
            .all(|day| (damp(day * DAY_TICKS) + warmth(day * DAY_TICKS)).abs() < 0.05);
        assert!(!mirrored, "damp is just warmth with a minus sign");
    }

    /// The word round-trips, for the capture flag and the terminal.
    #[test]
    fn a_season_parses_off_its_own_label() {
        for season in SEASONS {
            assert_eq!(Season::parse(season.label()), Some(season));
            assert_eq!(Season::parse(&season.label().to_lowercase()), Some(season));
        }
        assert_eq!(Season::parse(" autumn "), Some(Season::Autumn));
        assert_eq!(Season::parse("harvest"), None);
    }

    /// The growing clock is an integral, so it is pure in its endpoints —
    /// which is the whole reason it is an integral. A reload has to land on
    /// the same stage as the session that saved it.
    #[test]
    fn the_growing_clock_is_pure_in_its_endpoints() {
        for (from, to) in [
            (0u64, DAY_TICKS),
            (SEASON_TICKS, 3 * SEASON_TICKS),
            (YEAR_TICKS * 3 + 17, YEAR_TICKS * 5 + 991),
        ] {
            assert_eq!(grown_between(from, to), grown_between(from, to));
            // And it composes: banking a span in two goes banks the same.
            let middle = from + (to - from) / 2;
            assert_eq!(
                grown_between(from, to),
                grown_between(from, middle) + grown_between(middle, to),
                "the growing clock is not additive"
            );
        }
        // Backwards is nothing rather than a panic.
        assert_eq!(grown_between(YEAR_TICKS, 0), 0);
    }

    /// A season redistributes the year: a whole year of calendar banks a
    /// whole year of growing, but a winter in it banks almost none and a
    /// summer banks well over its share.
    #[test]
    fn a_year_of_calendar_is_still_a_year_of_growing() {
        let year = grown_between(0, YEAR_TICKS);
        assert!(
            (year as i64 - YEAR_TICKS as i64).unsigned_abs() < DAY_TICKS / 4,
            "a year banks {year} of {YEAR_TICKS}"
        );
        // Ten years likewise, so the quantisation does not drift.
        let decade = grown_between(0, YEAR_TICKS * 10);
        assert!(
            (decade as i64 - (YEAR_TICKS * 10) as i64).unsigned_abs() < DAY_TICKS,
            "ten years bank {decade}"
        );

        let winter = grown_between(3 * SEASON_TICKS, 4 * SEASON_TICKS);
        let summer = grown_between(SEASON_TICKS, 2 * SEASON_TICKS);
        assert!(
            winter < SEASON_TICKS / 8,
            "winter banked {winter} of a {SEASON_TICKS} tick season"
        );
        assert!(
            summer > SEASON_TICKS * 3 / 2,
            "summer banked only {summer} of a {SEASON_TICKS} tick season"
        );
        assert_eq!(
            winter + summer + grown_between(0, SEASON_TICKS)
                + grown_between(2 * SEASON_TICKS, 3 * SEASON_TICKS),
            year,
            "the four seasons do not add up to their year"
        );
    }

    /// The inverse really inverts, and it answers with the spring morning
    /// rather than some frozen day after it.
    #[test]
    fn the_growing_clock_runs_backwards_too() {
        for from in [0u64, SEASON_TICKS / 3, 2 * SEASON_TICKS, YEAR_TICKS * 2 + 5_000] {
            for growing in [DAY_TICKS, DAY_TICKS * 5, YEAR_TICKS, YEAR_TICKS * 3] {
                let when = when_grown(from, growing);
                assert!(
                    grown_between(from, when) >= growing,
                    "when_grown came back early"
                );
                assert!(
                    grown_between(from, when.saturating_sub(1)) < growing,
                    "when_grown came back late — the tick before already had it"
                );
            }
        }
        assert_eq!(when_grown(1_234, 0), 1_234);

        // A stand cleared at the back end of autumn waits out the winter.
        // Two days of growing, asked for on the last day before winter, does
        // not arrive until the spring — the whole point of having a year.
        //
        // The *front* of autumn still grows, and should: the leaves turn
        // before the ground gives up, which is what makes an autumn an autumn
        // rather than a short winter.
        let cleared = 3 * SEASON_TICKS - DAY_TICKS;
        let back = when_grown(cleared, DAY_TICKS * 2);
        assert!(
            back > 3 * SEASON_TICKS,
            "an autumn clearing grew straight through the winter"
        );
        assert_eq!(
            Season::of(back),
            Season::Spring,
            "the woods came back in {:?}",
            Season::of(back)
        );
    }
}
