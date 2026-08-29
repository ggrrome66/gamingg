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

impl Conditions {
    /// The wind's strength on its own.
    pub fn wind_speed(&self) -> f32 {
        (self.wind.0 * self.wind.0 + self.wind.1 * self.wind.1).sqrt()
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
    let humidity = (signed_2d(seed ^ SALT_DAMP, u * 1.7, v * 1.7) * 0.5 + 0.5).clamp(0.0, 1.0);
    let temperature =
        (signed_2d(seed ^ SALT_WARM, u * 0.6 - 3.0, v * 0.6 + 5.0) * 0.5 + 0.5).clamp(0.0, 1.0);

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
    let wet = front + depth * 0.35;
    let (state, rain) = if wet > 0.62 {
        (State::Storm, ((wet - 0.62) / 0.30 + 0.6).clamp(0.6, 1.0))
    } else if wet > 0.34 {
        (State::Rain, ((wet - 0.34) / 0.28 * 0.55 + 0.15).clamp(0.15, 0.7))
    } else if wet > 0.10 {
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
pub fn fuel_moisture(seed: u64, tick: u64, x: i32, z: i32) -> f32 {
    1.0 - wetness(seed, tick, x, z)
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
}
