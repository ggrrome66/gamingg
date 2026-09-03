//! Weather, as a pure function of the tick.
//!
//! **No stored state and no side RNG.** Minecraft's world is deterministic
//! from its seed but its weather is not — it runs on a separate generator, so
//! two players on the same seed get different storms. That is the mistake
//! this module exists to avoid, and avoiding it costs nothing here: the
//! engine already speaks in noise fields sampled at a position, and a tick is
//! just another axis to sample along.
//!
//! Everything below is `f(seed, tick, x, z)`. Ask twice and get the same
//! answer; ask on the other side of a replay and get the same answer again;
//! ask about next Tuesday and get next Tuesday's weather without having to
//! have lived through Monday.
//!
//! **Fronts move, they do not blink.** The field is sampled at a point
//! *advected* by a slow wind, so a storm slides across the country at a
//! readable speed and you can watch it coming over the hills. Sampling the
//! same lattice without the drift would give each region its own weather that
//! changed underfoot, which reads as a bug however correct it is.
//!
//! **Fuel moisture is an integral, not a state.** How dry the country is
//! depends on how much it has rained lately — which sounds like something to
//! remember, and is not: recent rain is just the same rain field sampled at
//! earlier ticks, weighted so the last hour counts for more than the one
//! before it. Six samples and a decay, and the fire half of the round has the
//! term it needs without a single byte of storage.

use crate::noise::signed_2d;
use crate::season;

/// How coarse the weather is, in blocks. A region is bigger than anything you
/// can see at once, so the sky over a valley is one sky rather than a patchy
/// quilt.
pub const REGION: f32 = 512.0;

/// Ticks between one step of the weather's own clock and the next.
///
/// Ninety seconds at sixty-four ticks: long enough that a front takes a while
/// to arrive, short enough that a session sees the sky change more than once.
pub const PERIOD: u64 = 64 * 90;

/// How far the field slides per step, in regions. This is what turns a static
/// pattern into weather that comes *from* somewhere.
const DRIFT: f32 = 0.22;

/// How hard the wind blows at its strongest, in blocks per second — the
/// number the fire's spread multiplier reads, not a force on anything.
pub const WIND_MAX: f32 = 14.0;

/// How far the year moves the bar a front has to cross to rain.
///
/// Small on purpose. A season should be the difference between a wet week and
/// a dry one, not the difference between a monsoon and a desert — the country
/// has one climate and twelve months of it.
const SEASON_BAR: f32 = 0.13;

/// How far the year leans on the temperature and the humidity fields. The
/// place still decides most of it; the month decides the rest.
const SEASON_WARMTH: f32 = 0.22;
const SEASON_DAMP: f32 = 0.12;

/// How much drier high summer is than the rain alone accounts for.
///
/// Rain damps fuel and fuel dries out again, and that much was already true.
/// This is the extra: in a fire season the ground stays tinder a few days
/// after a shower that would have kept it safe in April, which is the term
/// that makes a fire season legible rather than merely statistical.
const SEASON_TINDER: f32 = 0.16;

const SALT_FRONT: u64 = 0x5701_1e1e_0b17_7c0d;
const SALT_DAMP: u64 = 0x00c1_0bd5_0000_7a1e;
const SALT_WARM: u64 = 0x7e11_9e12_a7c1_2e00;
const SALT_WIND: u64 = 0x1d1e_a2d6_9e17_5eed;

/// What the sky is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Clear,
    Cloud,
    Rain,
    Storm,
}

impl State {
    /// Is anything falling?
    pub fn wet(self) -> bool {
        matches!(self, State::Rain | State::Storm)
    }

    /// Short enough for a status line.
    pub fn label(self) -> &'static str {
        match self {
            State::Clear => "CLEAR",
            State::Cloud => "OVERCAST",
            State::Rain => "RAIN",
            State::Storm => "STORM",
        }
    }
}

/// The weather over one place at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conditions {
    /// `0` bitter, `1` hot. A number for the fire and the sky to read, not a
    /// temperature in degrees of anything.
    pub temperature: f32,
    /// How much water the air is holding, `0..1`.
    pub humidity: f32,
    /// Where the wind is going, and how hard, in blocks per second.
    pub wind: (f32, f32),
    /// How hard it is coming down, `0..1`. Zero unless the state is wet.
    pub rain: f32,
    pub state: State,
}

/// Below this the ground freezes and what falls is snow.
///
/// One threshold, in one place. The temperature is mostly the place's — a
/// cove is warmer than a summit in any month — with the year leaning on it,
/// so the high country in midwinter is well under this, the lowlands in
/// summer are nowhere near it, and the shoulders of the year cross it on a
/// cold front and come back. That is what makes a frost a *weather* rather
/// than a calendar entry.
pub const FREEZING: f32 = 0.30;

impl Conditions {
    /// The wind's strength on its own.
    pub fn wind_speed(&self) -> f32 {
        (self.wind.0 * self.wind.0 + self.wind.1 * self.wind.1).sqrt()
    }

    /// Is the ground freezing? Still water ices over and snow stays.
    pub fn freezing(&self) -> bool {
        self.temperature < FREEZING
    }

    /// Is what is falling falling as snow? Rain, cold.
    pub fn snowing(&self) -> bool {
        self.freezing() && self.state.wet()
    }

    /// The sky word for a status line, with the cold in it.
    pub fn sky_word(&self) -> &'static str {
        if self.snowing() {
            "SNOW"
        } else {
            self.state.label()
        }
    }
}

/// Where the field is sampled from at this tick: the position, pushed back
/// along the drift so the pattern slides over the country.
fn drifted(tick: u64, x: f32, z: f32) -> (f32, f32) {
    // Fractional steps, so the field slides smoothly instead of jumping once
    // a period.
    let phase = tick as f32 / PERIOD as f32;
    (
        x / REGION - phase * DRIFT,
        z / REGION - phase * DRIFT * 0.6,
    )
}

/// The weather over a place at a tick.
pub fn at(seed: u64, tick: u64, x: i32, z: i32) -> Conditions {
    let (u, v) = drifted(tick, x as f32, z as f32);

    // One low field decides the front, and a slower one decides how deep the
    // trough is — so storms are rarer than showers without a threshold table
    // saying so.
    let front = signed_2d(seed ^ SALT_FRONT, u, v);
    let depth = signed_2d(seed ^ SALT_FRONT, u * 0.31 + 11.0, v * 0.31 - 7.0);
    // The year's own two terms, sampled once. `warmth` runs -1..1 and `damp`
    // is the wet side of it, lagging a quarter-season.
    let warmth = season::warmth(tick);
    let damp = season::damp(tick);

    let humidity = (signed_2d(seed ^ SALT_DAMP, u * 1.7, v * 1.7) * 0.5 + 0.5 + damp * SEASON_DAMP)
        .clamp(0.0, 1.0);
    // The place still decides most of it — a cove is warmer than a summit in
    // any month — and the year leans on the answer. The place's own share
    // stops short of the ends so the year's lean is what crosses the
    // freezing line: no summit freezes at the height of the fire season, and
    // no cove stays open through midwinter.
    let temperature = (signed_2d(seed ^ SALT_WARM, u * 0.6 - 3.0, v * 0.6 + 5.0) * 0.4
        + 0.55
        + warmth * SEASON_WARMTH)
        .clamp(0.0, 1.0);

    // The wind turns slowly and always blows somewhere: a dead calm that
    // lasts is a fire model with nothing to say.
    let bearing = signed_2d(seed ^ SALT_WIND, u * 0.4, v * 0.4) * std::f32::consts::PI;
    let gust = 0.35 + 0.65 * (front * 0.5 + 0.5).clamp(0.0, 1.0);
    let wind = (
        bearing.cos() * gust * WIND_MAX,
        bearing.sin() * gust * WIND_MAX,
    );

    // Thresholds on the front, deepened by the slow field. The bands are
    // wide enough that most of the time it is simply weather.
    //
    // The season moves the *bar*, not the field: in the dry half of the year
    // a front has to be deeper to rain at all, and in the wet half a shower
    // that would have been a grey afternoon comes down. Shifting the
    // thresholds rather than scaling the rain is what keeps a summer storm a
    // proper summer storm when one does arrive.
    let bar = -damp * SEASON_BAR;
    let wet = front + depth * 0.35;
    let (state, rain) = if wet > 0.62 + bar {
        (State::Storm, ((wet - 0.62 - bar) / 0.30 + 0.6).clamp(0.6, 1.0))
    } else if wet > 0.34 + bar {
        (State::Rain, ((wet - 0.34 - bar) / 0.28 * 0.55 + 0.15).clamp(0.15, 0.7))
    } else if wet > 0.10 + bar {
        (State::Cloud, 0.0)
    } else {
        (State::Clear, 0.0)
    };

    Conditions {
        temperature,
        humidity,
        wind,
        rain,
        state,
    }
}

/// How much rain has fallen here lately, `0..1`.
///
/// The whole of "has it been dry?" — and it is an integral of the same field
/// rather than a number anybody had to keep. Six samples back through the
/// last few weather periods, weighted so this hour counts for more than the
/// one before it.
pub fn wetness(seed: u64, tick: u64, x: i32, z: i32) -> f32 {
    let mut sum = 0.0;
    let mut weight = 0.0;
    for step in 0..6u64 {
        let then = tick.saturating_sub(step * PERIOD / 2);
        let older = 0.72f32.powi(step as i32);
        sum += at(seed, then, x, z).rain * older;
        weight += older;
    }
    (sum / weight.max(1.0e-6)).clamp(0.0, 1.0)
}

/// How dry the fuel is, `0` sodden and `1` tinder.
///
/// Ignition rises sharply once dead fuel drops below about a fifth of its
/// saturated moisture, which is what makes the difference between a strike
/// that does nothing and a strike that takes a hillside. Here that is one
/// subtraction: the dry side of how wet it has been.
///
/// The season arrives here twice over, and both are wanted. It is already in
/// [`wetness`], because the thresholds it moved decided whether it rained at
/// all; and it is added directly on top, because a hot dry August evaporates
/// what did fall faster than an April one. So high summer is tinder within
/// days of a shower and midwinter never quite gets there.
pub fn fuel_moisture(seed: u64, tick: u64, x: i32, z: i32) -> f32 {
    let dried = season::warmth(tick).max(0.0) * SEASON_TINDER;
    (1.0 - wetness(seed, tick, x, z) + dried).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A day of ticks, for walking the sky forward.
    const DAY: u64 = 64 * 1200;

    #[test]
    fn the_same_tick_gives_the_same_sky_every_time() {
        for (tick, x, z) in [(0u64, 0, 0), (99_991, -4_120, 883), (5_000_000, 12_345, -54_321)] {
            let first = at(7, tick, x, z);
            assert_eq!(first, at(7, tick, x, z), "the weather changed its mind");
            assert_eq!(
                fuel_moisture(7, tick, x, z),
                fuel_moisture(7, tick, x, z),
                "the fuel changed its mind"
            );
        }
        // And two seeds are two countries.
        let mine = (0..40).map(|step| at(7, step * PERIOD, 0, 0).state).collect::<Vec<_>>();
        let yours = (0..40).map(|step| at(8, step * PERIOD, 0, 0).state).collect::<Vec<_>>();
        assert_ne!(mine, yours, "two seeds got the same fortnight of weather");
    }

    #[test]
    fn every_kind_of_day_happens_and_none_of_them_is_all_of_them() {
        // Over a season and a wide country: it is mostly not raining, it
        // rains often enough to matter, and it storms rarely.
        let (mut clear, mut cloud, mut rain, mut storm) = (0, 0, 0, 0);
        let mut total = 0;
        for day in 0..30u64 {
            for (x, z) in [(0, 0), (900, -400), (-2_100, 1_700), (5_000, 5_000)] {
                for hour in 0..8u64 {
                    total += 1;
                    match at(7, day * DAY + hour * DAY / 8, x, z).state {
                        State::Clear => clear += 1,
                        State::Cloud => cloud += 1,
                        State::Rain => rain += 1,
                        State::Storm => storm += 1,
                    }
                }
            }
        }
        let share = |count: i32| 100.0 * count as f32 / total as f32;
        assert!(share(clear) > 20.0, "never a clear day: {:.0}%", share(clear));
        assert!(cloud > 0, "the sky is never merely grey");
        assert!(
            (2.0..45.0).contains(&share(rain)),
            "rain is {:.0}% of the month",
            share(rain)
        );
        assert!(storm > 0, "it never storms anywhere, ever");
        assert!(
            share(storm) < share(rain) + 5.0,
            "storms are as common as showers"
        );
    }

    #[test]
    fn a_front_moves_across_the_country_rather_than_blinking() {
        // The same weather turns up downwind later: a front that changed in
        // place would show as two unrelated skies.
        let ahead = at(7, 0, 0, 0);
        let mut matched = false;
        for step in 1..8u64 {
            let later = at(7, step * PERIOD, (step as i32) * 112, (step as i32) * 67);
            if later.state == ahead.state && (later.rain - ahead.rain).abs() < 0.2 {
                matched = true;
            }
        }
        assert!(matched, "the weather is not travelling anywhere");

        // And it is not the same everywhere at once.
        let here = at(7, 0, 0, 0).state;
        let far = (0..12)
            .map(|step| at(7, 0, step * 900, step * -700).state)
            .any(|state| state != here);
        assert!(far, "one sky over the whole world");
    }

    #[test]
    fn the_country_dries_out_after_it_rains() {
        // Find a place and time it is raining hard, then walk forward: the
        // fuel has to come back to dry, or nothing will ever burn again.
        let mut found = None;
        'hunt: for day in 0..40u64 {
            for hour in 0..12u64 {
                let tick = day * DAY + hour * DAY / 12;
                if at(7, tick, 0, 0).rain > 0.5 {
                    found = Some(tick);
                    break 'hunt;
                }
            }
        }
        let wet_tick = found.expect("it never once rained at the origin in forty days");
        let soaked = fuel_moisture(7, wet_tick, 0, 0);
        assert!(soaked < 0.6, "rain did not damp the fuel: {soaked}");

        let mut driest: f32 = soaked;
        for step in 1..24u64 {
            driest = driest.max(fuel_moisture(7, wet_tick + step * PERIOD, 0, 0));
        }
        assert!(driest > 0.85, "it never dried out again: {driest}");
    }

    #[test]
    fn the_wind_always_blows_somewhere_and_never_a_gale() {
        for step in 0..60u64 {
            let conditions = at(7, step * PERIOD, 400, -400);
            let speed = conditions.wind_speed();
            assert!(speed > 0.5, "a dead calm at step {step}");
            assert!(speed <= WIND_MAX + 0.01, "a hurricane at step {step}: {speed}");
        }
        // A storm is windier than a clear day, on the whole.
        let mut calm = (0.0, 0);
        let mut rough = (0.0, 0);
        for step in 0..400u64 {
            let conditions = at(7, step * PERIOD, 0, 0);
            match conditions.state {
                State::Clear => {
                    calm.0 += conditions.wind_speed();
                    calm.1 += 1;
                }
                State::Storm => {
                    rough.0 += conditions.wind_speed();
                    rough.1 += 1;
                }
                _ => {}
            }
        }
        if calm.1 > 0 && rough.1 > 0 {
            assert!(
                rough.0 / rough.1 as f32 > calm.0 / calm.1 as f32,
                "storms are no windier than clear days"
            );
        }
    }

    /// A season is a *redistribution*. Summer is drier than winter over the
    /// same country, and the year as a whole rains as much as it always did
    /// — a calendar that quietly turned the taps down would be a nerf
    /// wearing a season's clothes.
    #[test]
    fn summer_is_drier_than_winter_and_the_year_is_not() {
        use crate::season::{Season, DAY_TICKS, SEASON_DAYS, SEASON_TICKS, YEAR_DAYS};
        let places = [(0, 0), (900, -400), (-2_100, 1_700), (5_000, 5_000)];
        let over = |season: Season| {
            let start = season.index() as u64 * SEASON_TICKS;
            let mut sum = 0.0;
            let mut count = 0;
            for day in 0..SEASON_DAYS {
                for hour in 0..8u64 {
                    for (x, z) in places {
                        sum += at(7, start + day * DAY_TICKS + hour * DAY_TICKS / 8, x, z).rain;
                        count += 1;
                    }
                }
            }
            sum / count as f32
        };
        let (summer, winter) = (over(Season::Summer), over(Season::Winter));
        assert!(
            summer < winter * 0.85,
            "summer ({summer:.3}) is not meaningfully drier than winter ({winter:.3})"
        );
        assert!(summer > 0.0, "it never rains all summer anywhere");

        // And the season *redistributes* the year rather than turning the
        // taps down: the year's own mean sits between the two extremes it
        // created, so what summer went without, some other season had.
        //
        // Not "a year later is identical" — a year later is not identical and
        // should not be, because the front field keeps drifting under the
        // calendar. The country repeats its seasons, not its weather.
        let mut year = 0.0;
        let mut count = 0;
        for day in 0..YEAR_DAYS {
            for hour in 0..8u64 {
                for (x, z) in places {
                    year += at(7, day * DAY_TICKS + hour * DAY_TICKS / 8, x, z).rain;
                    count += 1;
                }
            }
        }
        let year = year / count as f32;
        assert!(
            summer < year && year < winter,
            "the year ({year:.3}) is not between its summer ({summer:.3}) and its winter ({winter:.3})"
        );
        // The bar the season moves averages to nothing over the year, so the
        // whole-year total lands near the middle of what it swings between
        // rather than at one end of it.
        let middle = (summer + winter) / 2.0;
        assert!(
            (year - middle).abs() < (winter - summer) * 0.5,
            "the calendar has a thumb on the scale: {year:.3} against a middle of {middle:.3}"
        );
    }

    /// The fire season is real: the fuel is at its driest in high summer and
    /// its wettest in the depths of winter, over a country's worth of
    /// samples. This is the term `fire::strike` reads, so this test is the
    /// whole reason lightning bites in August and does nothing in February.
    #[test]
    fn the_fuel_is_tinder_in_summer_and_sodden_in_winter() {
        use crate::season::{Season, DAY_TICKS, SEASON_DAYS, SEASON_TICKS};
        let mean = |season: Season| {
            let start = season.index() as u64 * SEASON_TICKS;
            let mut sum = 0.0;
            let mut count = 0;
            for day in 0..SEASON_DAYS {
                for (x, z) in [(0, 0), (900, -400), (-2_100, 1_700), (5_000, 5_000)] {
                    sum += fuel_moisture(7, start + day * DAY_TICKS, x, z);
                    count += 1;
                }
            }
            sum / count as f32
        };
        let seasons = [
            (Season::Spring, mean(Season::Spring)),
            (Season::Summer, mean(Season::Summer)),
            (Season::Autumn, mean(Season::Autumn)),
            (Season::Winter, mean(Season::Winter)),
        ];
        let driest = seasons.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let wettest = seasons.iter().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(driest.0, Season::Summer, "the driest season is {:?}", driest.0);
        assert_eq!(wettest.0, Season::Winter, "the wettest season is {:?}", wettest.0);
        assert!(
            driest.1 - wettest.1 > 0.08,
            "the year barely moves the fuel: {:.3} to {:.3}",
            wettest.1,
            driest.1
        );
    }

    /// A season leans; it does not take over. Two places on the same day are
    /// still two skies, and the sky over one place still changes through a
    /// season — a calendar that flattened the weather into "it is summer"
    /// would have taken something away.
    #[test]
    fn the_season_leans_on_the_weather_rather_than_replacing_it() {
        use crate::season::{DAY_TICKS, SEASON_DAYS, SEASON_TICKS};
        for season_start in [0, SEASON_TICKS, 2 * SEASON_TICKS, 3 * SEASON_TICKS] {
            let mut states = std::collections::BTreeSet::new();
            for day in 0..SEASON_DAYS {
                for hour in 0..8u64 {
                    let tick = season_start + day * DAY_TICKS + hour * DAY_TICKS / 8;
                    for (x, z) in [(0, 0), (3_000, -1_500), (-4_400, 2_900)] {
                        states.insert(at(7, tick, x, z).state);
                    }
                }
            }
            assert!(
                states.len() > 2,
                "a whole season of one kind of sky: {states:?}"
            );
        }
    }

    /// The cold is a weather, not a calendar entry: nowhere sampled freezes
    /// in the fire season, somewhere freezes in midwinter, and the country
    /// as a whole is not frozen solid for a season — the lowlands thaw.
    #[test]
    fn the_cold_is_a_winter_thing_and_not_the_whole_of_winter() {
        use crate::season::{DAY_TICKS, SEASON_DAYS, SEASON_TICKS};
        let places = [(0, 0), (900, -400), (-2_100, 1_700), (5_000, 5_000), (-3_300, -2_200)];
        let mut summer_froze = 0;
        let mut winter_froze = 0;
        let mut winter_thawed = 0;
        let mut snowed = 0;
        for day in 0..SEASON_DAYS {
            for hour in 0..8u64 {
                let summer = SEASON_TICKS + day * DAY_TICKS + hour * DAY_TICKS / 8;
                let winter = 3 * SEASON_TICKS + day * DAY_TICKS + hour * DAY_TICKS / 8;
                for (x, z) in places {
                    if at(7, summer, x, z).freezing() {
                        summer_froze += 1;
                    }
                    let cold = at(7, winter, x, z);
                    if cold.freezing() {
                        winter_froze += 1;
                    } else {
                        winter_thawed += 1;
                    }
                    if cold.snowing() {
                        snowed += 1;
                    }
                }
            }
        }
        assert_eq!(summer_froze, 0, "it froze {summer_froze} times in the fire season");
        assert!(winter_froze > 0, "nowhere froze all winter");
        assert!(winter_thawed > 0, "the whole country was frozen solid all winter");
        assert!(snowed > 0, "it never once snowed");
        // And the word follows the weather.
        let flake = Conditions {
            temperature: 0.1,
            humidity: 0.8,
            wind: (1.0, 0.0),
            rain: 0.5,
            state: State::Rain,
        };
        assert_eq!(flake.sky_word(), "SNOW");
        assert_eq!(Conditions { temperature: 0.6, ..flake }.sky_word(), "RAIN");
        assert!(!Conditions { state: State::Clear, rain: 0.0, ..flake }.snowing());
        assert!(Conditions { state: State::Clear, rain: 0.0, ..flake }.freezing());
    }
}
