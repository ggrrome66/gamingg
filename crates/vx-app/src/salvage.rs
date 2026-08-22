//! What is in the crates, and how it comes out.
//!
//! # A source, not another sink
//!
//! Every credit in this game so far has come from selling what you dug, and
//! every cost has been something to buy. Bunkers are the first place goods
//! come from that you cannot buy at any price and cannot mine at any depth:
//! somebody left them there, and the only question is whether you can get in.
//!
//! # Derived, so two visits agree
//!
//! A cache's contents are a pure function of where it stands — the same rule
//! the board's postings and the town lattice follow. Nothing is rolled at
//! generation time and nothing is written down, so a cache three kilometres
//! away costs nothing until somebody walks to it, and walking away and back
//! finds the same crate.
//!
//! What *is* remembered is that it was opened, and the world already
//! remembers it: salvaging clears the block, and a cleared block is a
//! modified chunk, which is the one place this engine keeps player changes.
//! A second ledger for the same fact would be a second thing to keep in step.

use vx_core::BlockPos;

use crate::skills;

/// What a cache can hold, with how likely each is and how much of it.
///
/// Named goods that already exist, so a cache feeds the pile the shop and the
/// fabricator already read. The concept sheet's rations, spirits, oil and
/// blankets want goods this game has not got yet, and inventing six of them
/// for one crate would be a trade round wearing a bunker's clothes.
const TABLE: &[(&str, u64, u64)] = &[
    ("engine:copper_bar", 2, 6),
    ("engine:copper_ore", 8, 24),
    ("engine:plank", 4, 12),
    ("engine:metal_wall", 2, 8),
    ("engine:log", 3, 9),
    ("engine:stone", 12, 40),
];

/// How many kinds of thing one crate holds.
const KINDS: usize = 3;

/// Prospecting reads a crate the way it reads a rock face: the levelled
/// bonus is a share of the haul, not a different table, so a cache is worth
/// finding early and worth revisiting a skill for later.
const SKILLED_SHARE: f32 = 0.04;

fn hash(at: BlockPos, salt: u64) -> u64 {
    vx_world::seed::finalise(
        salt ^ (at.x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (at.y as i64 as u64).wrapping_mul(0xd6e8_feb8_6659_fd93)
            ^ (at.z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
    )
}

/// What the crate at `at` holds, for somebody with this much Prospecting.
///
/// Pure in `(position, level)`: the same crate always holds the same things,
/// and the skill scales the haul rather than re-rolling it — so a player
/// cannot learn that waiting for a level makes it a *different* crate.
pub fn contents(at: BlockPos, prospecting: u32) -> Vec<(&'static str, u64)> {
    let mut found: Vec<(&'static str, u64)> = Vec::new();
    for pick in 0..KINDS {
        let roll = hash(at, 0x5a1_0000 + pick as u64);
        let (name, low, high) = TABLE[(roll % TABLE.len() as u64) as usize];
        if found.iter().any(|(held, _)| *held == name) {
            continue;
        }
        let span = high - low + 1;
        let base = low + (hash(at, 0x5a2_0000 + pick as u64) % span);
        let bonus = 1.0 + SKILLED_SHARE * prospecting.saturating_sub(1) as f32;
        found.push((name, ((base as f32) * bonus).round() as u64));
    }
    found
}

/// The experience a crate is worth. Opening one is prospecting: the find is
/// the skill, and the crate is the outcrop.
pub fn experience(at: BlockPos) -> u64 {
    60 + (hash(at, 0x5a3) % 40)
}

/// The skill a cache pays into.
pub const SKILL: &str = skills::PROSPECTING;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crate_holds_the_same_things_every_time_you_look() {
        let at = BlockPos::new(-1217, 96, 1116);
        assert_eq!(contents(at, 1), contents(at, 1));
        assert_ne!(
            contents(at, 1),
            contents(BlockPos::new(at.x + 1, at.y, at.z), 1),
            "two crates a block apart hold identical hauls"
        );
    }

    #[test]
    fn skill_scales_the_haul_without_rerolling_it() {
        let at = BlockPos::new(400, 70, -900);
        let green = contents(at, 1);
        let veteran = contents(at, 40);
        let names: Vec<&str> = green.iter().map(|(name, _)| *name).collect();
        let later: Vec<&str> = veteran.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, later, "levelling changed what is in the crate");
        for ((_, low), (_, high)) in green.iter().zip(&veteran) {
            assert!(high >= low, "levelling made a crate worse");
        }
        assert!(
            veteran.iter().map(|(_, count)| count).sum::<u64>()
                > green.iter().map(|(_, count)| count).sum::<u64>(),
            "forty levels of prospecting bought nothing"
        );
    }

    #[test]
    fn every_crate_holds_something_the_game_knows_about() {
        let mut registry = vx_core::BlockRegistry::new();
        vx_world::gen::TerrainBlocks::register_builtins(&mut registry);
        for step in 0..500 {
            let at = BlockPos::new(step * 7 - 1_000, 40 + step % 90, step * 13 - 2_000);
            let haul = contents(at, 1);
            assert!(!haul.is_empty(), "an empty crate at {at:?}");
            for (name, count) in haul {
                assert!(count > 0, "{name} came out as nothing");
                assert!(
                    registry.id_of(name).is_some(),
                    "a crate holds {name}, which is not a block this game has"
                );
            }
        }
    }
}
