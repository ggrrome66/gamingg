//! Who runs a town.
//!
//! # The offices were always there; nobody held them
//!
//! Stage 11 gave every claim an owner, and the town's own buildings — the
//! shop, the bank, the civic ground — answer to
//! [`Claimant::Office(Office::Mayor)`](crate::permits::Claimant). Its security
//! office answers to the sheriff. But an office was a *label*: the only thing
//! that could hold one was the player, through
//! [`Permits::take_office`](crate::permits::Permits::take_office) and the
//! `--sheriff` development flag. Every town in the world had a mayor's
//! property and no mayor.
//!
//! This module says who they are, and it does it the way this project says
//! everything: **derived, not stored**. A seat is a roster index hashed off
//! the town's own seed, so two machines agree on who the sheriff of a town
//! four kilometres away is without either of them having been there.
//!
//! # The hometown keeps its authored answer
//!
//! [`people::person`] already writes `THE MAYOR` at index 0 and `THE SHERIFF`
//! at index 1 for the town at the origin, with authored temperaments to match
//! — proud and steady. So the derivation is skipped there rather than fought
//! with: the town every player starts in has the people somebody wrote, and
//! the offices land on the names that already carry them.
//!
//! # Why the trade does not decide it
//!
//! It would be tidy for the CLERK to be the mayor and nobody else ever. It
//! would also mean every Depot in the world is run by the same job title,
//! which is the kind of pattern a player spots in an hour and never unsees.
//! The seat is its own draw off its own salt, and a town where the powderman
//! is mayor and the foreman wears the badge is a town with a story in it.

use vx_world::TownSite;

use crate::people::{self, Person, PEOPLE};
use crate::permits::Office;

/// Every office a town fills. Two, for now — the note's shop clerk is the
/// counter, which already exists as a building rather than as a title, and
/// deputies are mustered rather than seated.
pub const OFFICES: [Office; 2] = [Office::Mayor, Office::Sheriff];

/// The town's own hash stream for a seat, salted per office.
///
/// Shaped like [`people`]'s own per-person stream and salted well clear of
/// it, so a town's mayor is not quietly correlated with anybody's birthday.
fn draw(site: &TownSite, salt: u64) -> u64 {
    vx_world::seed::finalise(
        site.seed
            ^ 0x0ff1_ce00_0000_0001
            ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15),
    )
}

/// Which resident holds an office.
///
/// The sheriff is drawn from whoever is left after the mayor, so the two are
/// never the same person — a town where one man is both is not a town with a
/// warrant chain in it, and the chain is the point.
pub fn holder(site: &TownSite, office: Office) -> usize {
    // The authored trio. Their names *are* their offices; deriving over the
    // top would put the badge on Old Prat.
    if site.is_home() {
        return match office {
            Office::Mayor => 0,
            Office::Sheriff => 1,
        };
    }
    let mayor = (draw(site, 0x01) % PEOPLE as u64) as usize;
    match office {
        Office::Mayor => mayor,
        Office::Sheriff => {
            // Pick among the others by index, then shift back past the mayor.
            // Modulo `PEOPLE - 1` and a step over the taken seat, which is the
            // cheapest correct way to draw without replacement.
            let among = (draw(site, 0x02) % (PEOPLE as u64 - 1)) as usize;
            if among >= mayor {
                among + 1
            } else {
                among
            }
        }
    }
}

/// The office this resident holds, if any.
///
/// The inverse of [`holder`], for the panels and the roster verb: they walk
/// the roster and want to know what to print beside each name.
pub fn office_of(site: &TownSite, index: usize) -> Option<Office> {
    OFFICES
        .into_iter()
        .find(|office| holder(site, *office) == index)
}

/// The person in the seat.
pub fn seat(site: &TownSite, office: Office) -> Person {
    people::person(site, holder(site, office))
}

/// Who is minding the counter, when nobody is standing close enough to say.
///
/// The shop is a building rather than a person — the counter answers to the
/// mayor's claim like the rest of the town's own property — so a trade needs
/// somebody to be *with*. Whoever the schedule has at work is the honest
/// answer; on a market day the whole town is at the square and the counter
/// goes with them; and failing both, somebody always minds the shop.
pub fn at_the_counter(site: &TownSite, day: u32, time: crate::clock::TimeOfDay) -> usize {
    let place_of = |index: usize| crate::schedule::where_is(site, index, day, time, false);
    (0..PEOPLE)
        .find(|index| place_of(*index) == crate::schedule::Place::Workplace)
        .or_else(|| (0..PEOPLE).find(|index| place_of(*index) == crate::schedule::Place::Plaza))
        .unwrap_or(0)
}

/// What to print for an office: `MAYOR`, `SHERIFF`.
pub fn title(office: Office) -> &'static str {
    match office {
        Office::Mayor => "MAYOR",
        Office::Sheriff => "SHERIFF",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_world::town;

    fn somewhere(seed: u64) -> Vec<TownSite> {
        town::towns_near(seed, (0, 0), 6_000, &|_, _| 90)
    }

    /// Derived means derived: the same town gives the same seats however many
    /// times you ask, and asking is all anybody ever does — nothing is stored.
    #[test]
    fn a_towns_offices_are_stable_in_its_own_seed() {
        for site in somewhere(2024) {
            for office in OFFICES {
                assert_eq!(holder(&site, office), holder(&site, office));
            }
        }
    }

    /// One man cannot be both the sheriff and the mayor he has to ask.
    #[test]
    fn the_mayor_is_never_the_sheriff() {
        for site in somewhere(7) {
            let mayor = holder(&site, Office::Mayor);
            let sheriff = holder(&site, Office::Sheriff);
            assert_ne!(
                mayor, sheriff,
                "{} seated one person twice",
                site.name.head()
            );
            assert!(mayor < PEOPLE && sheriff < PEOPLE);
        }
    }

    /// The hometown's people were written, not rolled, and the badge goes on
    /// the one whose name is on it.
    #[test]
    fn the_hometown_keeps_its_authored_pair() {
        let home = town::home_site();
        assert_eq!(holder(&home, Office::Mayor), 0);
        assert_eq!(holder(&home, Office::Sheriff), 1);
        assert_eq!(seat(&home, Office::Mayor).name, "THE MAYOR");
        assert_eq!(seat(&home, Office::Sheriff).name, "THE SHERIFF");
    }

    /// `office_of` really is the inverse, and the third resident holds
    /// nothing — somebody in every town is just a neighbour.
    #[test]
    fn the_seats_invert_and_somebody_holds_neither() {
        for site in somewhere(11) {
            let mut held = 0;
            for index in 0..PEOPLE {
                if let Some(office) = office_of(&site, index) {
                    assert_eq!(holder(&site, office), index);
                    held += 1;
                }
            }
            assert_eq!(held, OFFICES.len(), "a town seated the wrong number");
        }
    }

    /// Somebody is always available to trade with: the counter never
    /// answers "nobody", however odd the hour, because a shop that could not
    /// name whose it was would be a trade with nobody on the other side.
    #[test]
    fn the_counter_always_has_somebody_behind_it() {
        use crate::clock::TimeOfDay;
        for site in somewhere(7) {
            for day in 0..8 {
                for hour in 0..24 {
                    let time = TimeOfDay::new(hour as f32 / 24.0);
                    assert!(at_the_counter(&site, day, time) < PEOPLE);
                }
            }
        }
    }

    /// And through the working day it really is whoever is at work, rather
    /// than a constant wearing a function's clothes.
    #[test]
    fn the_counter_is_whoever_is_actually_at_work() {
        use crate::clock::TimeOfDay;
        use crate::schedule::{where_is, Place};
        let site = somewhere(2024)
            .into_iter()
            .find(|site| !site.is_home())
            .expect("no frontier town");
        let day = 3;
        let mut ever_at_work = false;
        for hour in 8..16 {
            let time = TimeOfDay::new(hour as f32 / 24.0);
            let index = at_the_counter(&site, day, time);
            if where_is(&site, index, day, time, false) == Place::Workplace {
                ever_at_work = true;
            }
        }
        assert!(ever_at_work, "nobody was ever at the counter in office hours");
    }

    /// Not every Depot is run by the clerk. The seat is its own draw, so the
    /// trade in office varies across the world — the check that the offices
    /// are not quietly a function of the speciality.
    #[test]
    fn the_office_is_not_just_the_job_title() {
        let mut trades = std::collections::BTreeSet::new();
        for site in somewhere(2024) {
            trades.insert(seat(&site, Office::Mayor).trade);
        }
        assert!(
            trades.len() > 1,
            "every mayor in the world does the same job: {trades:?}"
        );
    }
}
