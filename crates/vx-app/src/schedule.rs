//! Where everybody is: a schedule is worldgen for people.
//!
//! # A lookup, not a simulation
//!
//! The Stardew insight that makes forty towns of named people affordable:
//! where someone stands at tick T is a pure function of (person, day, hour).
//! Nothing walks anywhere until somebody looks — exactly like the economy,
//! exactly like the terrain — and because the answer is derived, the kestrel,
//! the roost and a stakeout all observe *consistent* lives. A villager seen
//! at the counter at noon is at the counter at noon in every session with
//! this seed, which is what makes "I know where the sheriff will be" a plan
//! rather than a hope.
//!
//! # The rule stack
//!
//! Ranked; the first match wins. The design note's stack, minus its rain rule
//! — this world has no weather yet, and a rule that can never fire is a lie
//! in a table.
//!
//! | Priority | Condition | Place |
//! |---|---|---|
//! | 1 | town alarmed | home, shutters |
//! | 2 | night (before 06:00, after 20:00) | home; window lit, then dark |
//! | 3 | market day, 10:00–16:00 | the square |
//! | 4 | work hours 07:00–17:00 | workplace |
//! | 5 | evening 17:00–20:00 | by archetype: Chatty→square, Steady→home porch, the rest walk |
//! | 6 | dawn edge | strolling |
//!
//! Every boundary is shifted ±20 minutes by the person's own hash, so the
//! town never moves in lockstep — the note's jitter, and the difference
//! between a place with habits and a place with a bell.

use vx_world::TownSite;

use crate::clock::TimeOfDay;
use crate::people::Archetype;

/// Simulation ticks in one game day: `DAY_SECONDS` at the journal's 8 Hz.
pub const TICKS_PER_DAY: u64 = (crate::clock::DAY_SECONDS as u64) * 8;

/// Where the schedule can put somebody. Coarse on purpose: the *walking*
/// stays with the villagers' existing wander and route machinery, and a
/// place is a patch for that machinery to wander in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Home,
    Workplace,
    Plaza,
    Stroll,
}

impl Place {
    /// How the terminal's roster says it. Uppercase for the bitmap font.
    pub fn name(self) -> &'static str {
        match self {
            Place::Home => "HOME",
            Place::Workplace => "AT WORK",
            Place::Plaza => "THE SQUARE",
            Place::Stroll => "WALKING",
        }
    }
}

/// A wander rectangle in town-relative offsets, the villagers' own idiom.
pub type Patch = (f32, f32, f32, f32);

fn person_hash(site: &TownSite, index: usize, salt: u64) -> u64 {
    vx_world::seed::finalise(
        site.seed
            ^ (index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ salt.wrapping_mul(0xd6e8_feb8_6659_fd93),
    )
}

/// This person's clock error against the town's, in minutes. ±20.
fn jitter_minutes(site: &TownSite, index: usize) -> i64 {
    (person_hash(site, index, 0x11) % 41) as i64 - 20
}

/// Which day of the week this town holds its market.
pub fn market_weekday(site: &TownSite) -> u32 {
    (vx_world::seed::finalise(site.seed ^ 0x3a97) % 7) as u32
}

pub fn is_market_day(site: &TownSite, day: u32) -> bool {
    day % 7 == market_weekday(site)
}

/// Where person `index` of `site` is, on `day` at `time`.
///
/// Pure. `alarmed` is the one live input — a fright is not derivable from a
/// clock, and rule one of the stack belongs to it.
pub fn where_is(site: &TownSite, index: usize, day: u32, time: TimeOfDay, alarmed: bool) -> Place {
    if alarmed {
        return Place::Home;
    }

    // The person's own clock: the town's hour, shifted by their jitter.
    let minutes =
        (time.fraction() * 24.0 * 60.0) as i64 + jitter_minutes(site, index);
    let hour = minutes.rem_euclid(24 * 60) as f32 / 60.0;

    if !(6.0..20.0).contains(&hour) {
        return Place::Home;
    }
    if is_market_day(site, day) && (10.0..16.0).contains(&hour) {
        return Place::Plaza;
    }
    if (7.0..17.0).contains(&hour) {
        return Place::Workplace;
    }
    if hour >= 17.0 {
        // Evening is where temperament shows in the streets.
        return match crate::people::person(site, index).temperament.archetype {
            Archetype::Chatty | Archetype::Craven => Place::Plaza,
            Archetype::Steady | Archetype::Anxious => Place::Home,
            Archetype::Gruff | Archetype::Proud => Place::Stroll,
        };
    }
    // The dawn hour before work.
    Place::Stroll
}

/// The patch a place maps to, for the wander machinery.
///
/// `stroll` is the person's own patch, passed through — the schedule narrows
/// where they are, the patch keeps *how they idle there* exactly as it was.
pub fn patch_for(index: usize, place: Place, stroll: Patch) -> Patch {
    match place {
        // Home is handled by the route machinery, but a patch is still
        // needed for the moments between arriving and the door: the yard.
        Place::Home => stroll,
        Place::Plaza => (-6.0, -6.0, 6.0, 4.0),
        Place::Workplace => match index {
            // One works the yard north of the plaza, one the east end of
            // the high street, one the west. Every rectangle is a sub-patch
            // of ground the roster already authored building-free, so the
            // no-walls guarantee is inherited rather than re-proven.
            0 => (-9.0, 4.0, -5.0, 12.0),
            1 => (4.0, -8.0, 9.0, 0.0),
            _ => (-8.0, -8.0, 0.0, 0.0),
        },
        Place::Stroll => stroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> TownSite {
        vx_world::town::home_site()
    }

    fn at(hour: f32) -> TimeOfDay {
        TimeOfDay::new(hour / 24.0)
    }

    #[test]
    fn where_is_is_pure_and_follows_the_stack() {
        let home = site();
        for index in 0..crate::people::PEOPLE {
            for hour in [0.0, 3.0, 8.5, 12.0, 18.5, 22.0] {
                let now = where_is(&home, index, 3, at(hour), false);
                assert_eq!(now, where_is(&home, index, 3, at(hour), false));
            }
            // Deep night is home for everyone, whatever their jitter.
            assert_eq!(where_is(&home, index, 3, at(2.0), false), Place::Home);
            // Midday on a plain day is work.
            let plain_day = market_weekday(&home) + 1;
            assert_eq!(
                where_is(&home, index, plain_day, at(12.0), false),
                Place::Workplace
            );
            // Midday on market day is the square.
            assert_eq!(
                where_is(&home, index, market_weekday(&home), at(12.0), false),
                Place::Plaza
            );
            // An alarm empties the streets at any hour.
            assert_eq!(where_is(&home, index, 3, at(12.0), true), Place::Home);
        }
    }

    #[test]
    fn the_town_does_not_move_in_lockstep() {
        // Right on a boundary, jitter separates people: at 06:10 some are
        // already out and some still behind the door. Sampled across several
        // towns because any one trio might happen to share a sign.
        let ground = |_: i32, _: i32| 90;
        let mut split = false;
        for town in vx_world::town::towns_near(2024, (0, 0), 6_000, &ground) {
            let places: Vec<Place> = (0..crate::people::PEOPLE)
                .map(|index| where_is(&town, index, 2, at(6.17), false))
                .collect();
            if places.iter().any(|place| *place != places[0]) {
                split = true;
                break;
            }
        }
        assert!(split, "every villager in every town keeps the same hours");
    }

    #[test]
    fn evenings_belong_to_temperament() {
        // Across enough towns, evening splits three ways. This is the note's
        // archetype table observable in the streets rather than in the data.
        let ground = |_: i32, _: i32| 90;
        let mut seen = std::collections::BTreeSet::new();
        for town in vx_world::town::towns_near(2024, (0, 0), 10_000, &ground) {
            let plain_day = market_weekday(&town) + 1;
            for index in 0..crate::people::PEOPLE {
                match where_is(&town, index, plain_day, at(18.4), false) {
                    Place::Plaza => seen.insert("plaza"),
                    Place::Home => seen.insert("home"),
                    Place::Stroll => seen.insert("stroll"),
                    Place::Workplace => seen.insert("work"),
                };
            }
        }
        assert!(
            seen.contains("plaza") && seen.contains("home") && seen.contains("stroll"),
            "evenings collapsed to {seen:?}"
        );
    }

    #[test]
    fn market_day_differs_between_towns() {
        let ground = |_: i32, _: i32| 90;
        let towns = vx_world::town::towns_near(2024, (0, 0), 10_000, &ground);
        let days: std::collections::BTreeSet<u32> =
            towns.iter().map(market_weekday).collect();
        assert!(days.len() > 1, "every market in the world is the same day");
    }
}
