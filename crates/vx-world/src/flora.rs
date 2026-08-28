//! Vegetation: trees and grass tufts.
//!
//! The same discipline as ore ([`crate::ore`]): everything is a pure function
//! of `(seed, position)`, gathered per chunk from a jittered lattice, so a
//! canopy that crosses a chunk border comes out identical whichever side
//! generates first and nothing is ever stored.
//!
//! Trees stand only where the ground is grass — above the beach band and
//! outside the starting village — and tufts follow the same rule, so the town
//! lawns stay mowed and the dunes stay bare.
//!
//! **What grows is [`crate::forest`]'s answer, not this module's.** A cell
//! asks which forest it stands in and then grows that forest's tree: a broad
//! hardwood with the odd emergent giant over it, a narrow subalpine spire
//! thinning to knee-high krummholz at the treeline, or a thin black spruce
//! off a peat bog. One lattice, three canopies, and the biome does the
//! choosing so a stand never mixes species by accident.

use vx_core::BlockPos;

use crate::forest::{self, Biome};
use crate::gen::SEA_LEVEL;
use crate::town::{self, TownSite};

/// Lattice cell size for trees, in blocks. One potential tree per cell.
pub const TREE_CELL: i32 = 12;

/// Fraction of cells that grow their tree, per forest. A bog is crowded with
/// thin stems, the high country is dense where it is not bare rock, and a
/// hardwood stand is open enough underneath to walk through.
const HARDWOOD_PRESENCE: f32 = 0.35;
const SUBALPINE_PRESENCE: f32 = 0.55;
const BOG_PRESENCE: f32 = 0.62;
const KRUMMHOLZ_PRESENCE: f32 = 0.72;

/// One cell in this many grows an emergent giant instead of an ordinary
/// hardwood — the tulip poplar standing a head above the closed canopy.
const GIANT_IN: u32 = 13;

/// Density of grass tufts on eligible columns.
const TUFT_DENSITY: f32 = 0.07;

/// Canopy half-width of the widest tree there is; also the gather margin for
/// chunk overlap. The giants set it, so it is one wider than the ordinary
/// crown that used to set it.
pub const CANOPY_REACH: i32 = 3;

/// What kind of tree, which is to say which forest it came out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    /// Broad-crowned deciduous: the mid-elevation cove forest.
    Hardwood,
    /// The same, but a head taller than everything around it.
    Giant,
    /// Narrow subalpine spire.
    Spruce,
    /// Thin black spruce off the peat.
    BogSpruce,
    /// A wind-flattened mat at the treeline. Barely a tree; still a tree.
    Krummholz,
}

impl Species {
    /// The tree a forest grows, before the giants are rolled for.
    pub fn of(biome: Biome) -> Species {
        match biome {
            Biome::Bog => Species::BogSpruce,
            Biome::Hardwood => Species::Hardwood,
            Biome::Subalpine => Species::Spruce,
        }
    }

    /// Is this one of the conifers? The mesher and the fell rules both want
    /// to know, and neither wants to learn the whole list.
    pub fn conifer(self) -> bool {
        matches!(self, Species::Spruce | Species::BogSpruce | Species::Krummholz)
    }
}

/// One tree: a trunk from the ground and a crown at the top, in whatever
/// shape its species wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tree {
    /// The surface block the trunk stands on.
    pub base: BlockPos,
    /// Trunk length above the base.
    pub height: i32,
    /// What it is.
    pub species: Species,
}

/// What part of a tree occupies a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreePart {
    Trunk,
    Leaves,
}

/// The splitmix-style hash the tiles and ore lattice already use, mapped to
/// `0..1`.
fn hash01(seed: u64, salt: u64, x: i32, z: i32) -> f32 {
    crate::seed::unit(crate::seed::finalise(
        seed ^ salt
            ^ (x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
    ))
}

/// May a tree or tuft stand on this column, given its surface height?
///
/// Mirrors the generator's own surface rule: at or below the beach band the
/// top block is sand, and the village keeps its lawns clear.
fn ground_grows_grass(sites: &[TownSite], x: i32, z: i32, surface: i32) -> bool {
    surface > SEA_LEVEL + 1 && !town::footprint_contains(sites, x, z)
}

/// The tree in one lattice cell, if that cell grows one.
///
/// `height_at` must be the terrain height field — pure, so every chunk that
/// asks about this cell derives the identical tree. `natural_at` is the same
/// field *before* any town flattened a plot into it: the forest is decided by
/// the country's own shape, so a market square's plateau cannot turn the
/// hillside around it into alpine.
fn tree_in_cell(
    seed: u64,
    cell_x: i32,
    cell_z: i32,
    height_at: &impl Fn(i32, i32) -> i32,
    natural_at: &impl Fn(i32, i32) -> i32,
    sites: &[TownSite],
) -> Option<Tree> {
    // Jitter within the cell, keeping the canopy inside the cell's margin.
    let jitter_x = (hash01(seed, 0xf2, cell_x, cell_z) * (TREE_CELL - 1) as f32) as i32;
    let jitter_z = (hash01(seed, 0xf3, cell_x, cell_z) * (TREE_CELL - 1) as f32) as i32;
    let x = cell_x * TREE_CELL + jitter_x;
    let z = cell_z * TREE_CELL + jitter_z;

    let surface = height_at(x, z);
    if !ground_grows_grass(sites, x, z, surface) {
        return None;
    }

    // Above the last of the mats nothing stands at all: bare rock, ice and
    // wind. That is what makes a summit read as a summit.
    let natural = natural_at(x, z);
    if natural > forest::TREE_LIMIT_Y {
        return None;
    }

    let biome = forest::biome_at(seed, x, z, natural_at);
    // At the treeline an upright conifer gives way to a wind-flattened mat.
    let krummholz = biome == Biome::Subalpine && natural > forest::TREELINE_Y;
    let presence = match (biome, krummholz) {
        (_, true) => KRUMMHOLZ_PRESENCE,
        (Biome::Bog, _) => BOG_PRESENCE,
        (Biome::Hardwood, _) => HARDWOOD_PRESENCE,
        (Biome::Subalpine, _) => SUBALPINE_PRESENCE,
    };
    if hash01(seed, 0xf1, cell_x, cell_z) > presence {
        return None;
    }

    let roll = hash01(seed, 0xf4, x, z);
    let (species, height) = if krummholz {
        (Species::Krummholz, 1)
    } else {
        match Species::of(biome) {
            // One hardwood cell in GIANT_IN carries the emergent tree: the
            // one standing a head above the closed canopy, which is what an
            // old-growth cove forest looks like from across a valley.
            Species::Hardwood
                if crate::seed::finalise(
                    seed ^ 0x91a5
                        ^ (cell_x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        ^ (cell_z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
                )
                .is_multiple_of(GIANT_IN as u64) =>
            {
                (Species::Giant, 11 + (roll * 5.0) as i32) // 11..=15
            }
            Species::Hardwood => (Species::Hardwood, 5 + (roll * 3.0) as i32), // 5..=7
            Species::Spruce => (Species::Spruce, 9 + (roll * 6.0) as i32),     // 9..=14
            Species::BogSpruce => (Species::BogSpruce, 4 + (roll * 4.0) as i32), // 4..=7
            other => (other, 4),
        }
    };

    Some(Tree {
        base: BlockPos::new(x, surface, z),
        height,
        species,
    })
}

/// Every tree whose trunk or canopy could reach the box `min..=max` (x/z).
pub fn trees_overlapping(
    seed: u64,
    min: (i32, i32),
    max: (i32, i32),
    height_at: &impl Fn(i32, i32) -> i32,
    natural_at: &impl Fn(i32, i32) -> i32,
    sites: &[TownSite],
) -> Vec<Tree> {
    let lo_x = (min.0 - CANOPY_REACH).div_euclid(TREE_CELL);
    let hi_x = (max.0 + CANOPY_REACH).div_euclid(TREE_CELL);
    let lo_z = (min.1 - CANOPY_REACH).div_euclid(TREE_CELL);
    let hi_z = (max.1 + CANOPY_REACH).div_euclid(TREE_CELL);

    let mut found = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_z in lo_z..=hi_z {
            if let Some(tree) = tree_in_cell(seed, cell_x, cell_z, height_at, natural_at, sites) {
                found.push(tree);
            }
        }
    }
    found
}

/// The part of `tree` occupying a world position, if any.
///
/// Every species is one shape function over `(dx, dy, dz)` — pure, so the
/// canopy comes out identical from whichever chunk asks.
pub fn tree_part_at(tree: &Tree, x: i32, y: i32, z: i32) -> Option<TreePart> {
    let top = tree.base.y + tree.height;
    let (dx, dz) = (x - tree.base.x, z - tree.base.z);
    let (adx, adz) = (dx.abs(), dz.abs());
    let above = y - top;

    // The trunk is the same in every species: one column from the ground to
    // the crown. A mat has none worth drawing.
    if tree.species != Species::Krummholz
        && dx == 0
        && dz == 0
        && y > tree.base.y
        && y <= top
    {
        return Some(TreePart::Trunk);
    }

    let leaves = match tree.species {
        // The classic: two wide layers with the corners knocked off the top
        // one, then a narrow cap.
        Species::Hardwood => match above {
            -1 => adx <= 2 && adz <= 2,
            0 => adx <= 2 && adz <= 2 && !(adx == 2 && adz == 2),
            1 => adx <= 1 && adz <= 1,
            2 => adx + adz <= 1,
            _ => false,
        },
        // An emergent crown: wider, deeper, and rounded, because the tree it
        // belongs to is standing clear of everything around it.
        Species::Giant => match above {
            -3 | -2 => adx + adz <= 4 && adx <= 3 && adz <= 3,
            -1 => adx <= 3 && adz <= 3 && !(adx == 3 && adz == 3),
            0 => adx <= 2 && adz <= 2,
            1 => adx + adz <= 2,
            2 => adx + adz <= 1,
            _ => false,
        },
        // A spire: a point over the trunk, then diamond tiers widening as
        // they go down and pinched back in every third layer, so the
        // silhouette is notched the way a conifer is rather than a smooth
        // cone or — worse — a column with a flange at the bottom.
        Species::Spruce => {
            let crown = (tree.height - 3).max(3);
            let below = top - y;
            match above {
                1 => adx + adz == 0,
                0 => adx + adz <= 1,
                _ if (1..=crown).contains(&below) => {
                    // One block wider every third layer down, so the taper
                    // runs the whole length of the crown.
                    let radius = (below / 3 + 1).min(CANOPY_REACH);
                    adx + adz <= radius && !(below % 3 == 0 && adx + adz == radius)
                }
                _ => false,
            }
        }
        // Thin and see-through: a whisker of a crown at the top and a few
        // wisps below it. A bog stand is stems, not canopy.
        Species::BogSpruce => {
            let below = top - y;
            match below {
                0 => adx + adz <= 1,
                1 | 2 => adx + adz <= 1,
                3..=5 => adx + adz == 1 && (below + adx) % 2 == 0,
                _ => false,
            }
        }
        // A mat: knee-high, wider than it is tall, hugging the ground.
        Species::Krummholz => match y - tree.base.y {
            1 => adx <= 2 && adz <= 2 && adx + adz <= 3,
            2 => adx + adz <= 1,
            _ => false,
        },
    };
    leaves.then_some(TreePart::Leaves)
}

/// Does a grass tuft stand on this column? (Assuming the surface allows one —
/// the generator also requires the top block to be plain grass.)
pub fn tuft_at(seed: u64, x: i32, z: i32, surface: i32, sites: &[TownSite]) -> bool {
    ground_grows_grass(sites, x, z, surface) && hash01(seed, 0x7f, x, z) < TUFT_DENSITY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_height(_: i32, _: i32) -> i32 {
        80
    }

    /// The one town every world has, for the "keep out of town" rules.
    fn home() -> Vec<TownSite> {
        vec![town::home_site()]
    }

    #[test]
    fn the_forest_is_deterministic_and_clip_window_correct() {
        let all = trees_overlapping(
            7,
            (-200, -200),
            (200, 200),
            &flat_height,
            &flat_height,
            &home(),
        );
        assert!(!all.is_empty(), "no trees anywhere in a 400-block square");

        // Every tree found by a small window is in the big list, and every
        // big-list tree overlapping the window is found by it.
        let window = trees_overlapping(7, (0, 0), (15, 15), &flat_height, &flat_height, &home());
        for tree in &window {
            assert!(all.contains(tree));
            assert!(
                tree.base.x >= -CANOPY_REACH
                    && tree.base.x <= 15 + CANOPY_REACH
                    && tree.base.z >= -CANOPY_REACH
                    && tree.base.z <= 15 + CANOPY_REACH,
                "window returned a tree that cannot reach it: {tree:?}"
            );
        }
        for tree in &all {
            let reaches = tree.base.x >= -CANOPY_REACH
                && tree.base.x <= 15 + CANOPY_REACH
                && tree.base.z >= -CANOPY_REACH
                && tree.base.z <= 15 + CANOPY_REACH;
            assert_eq!(reaches, window.contains(tree), "clip window missed {tree:?}");
        }

        assert_eq!(
            trees_overlapping(7, (0, 0), (15, 15), &flat_height, &flat_height, &home()),
            window,
            "same seed, same window, different forest"
        );
    }

    #[test]
    fn no_tree_grows_in_town_or_on_the_beach() {
        let sites = home();
        let half = town::HOME_CORE_HALF;
        let in_town = trees_overlapping(
            7,
            (-half, -half),
            (half, half),
            &flat_height,
            &flat_height,
            &sites,
        );
        for tree in in_town {
            assert!(
                !town::footprint_contains(&sites, tree.base.x, tree.base.z),
                "a tree took root on main street: {tree:?}"
            );
        }

        let sea = |_: i32, _: i32| SEA_LEVEL;
        let on_beach = trees_overlapping(7, (300, 300), (500, 500), &sea, &sea, &sites);
        assert!(on_beach.is_empty(), "trees growing on the sea floor");

        assert!(
            !tuft_at(7, 0, 0, town::HOME_GROUND_Y, &sites),
            "a tuft on the plaza"
        );
    }

    #[test]
    fn nothing_at_all_stands_above_the_tree_limit() {
        // A plateau above the limit: bare rock, and that is what makes a
        // summit read as a summit.
        let bare = |_: i32, _: i32| forest::TREE_LIMIT_Y + 6;
        let summit = trees_overlapping(7, (600, 600), (800, 800), &bare, &bare, &home());
        assert!(summit.is_empty(), "trees above the tree limit: {summit:?}");

        // And just below it, mats — no upright conifer.
        let windy = |_: i32, _: i32| forest::TREELINE_Y + 8;
        let mats = trees_overlapping(7, (600, 600), (800, 800), &windy, &windy, &home());
        assert!(!mats.is_empty(), "nothing at all at the treeline");
        for tree in &mats {
            assert_eq!(
                tree.species,
                Species::Krummholz,
                "an upright tree above the treeline: {tree:?}"
            );
            assert_eq!(tree.height, 1);
        }
    }

    #[test]
    fn the_hardwood_shape_holds_together() {
        let tree = Tree {
            base: BlockPos::new(10, 80, 10),
            height: 5,
            species: Species::Hardwood,
        };
        // Trunk runs base+1..=base+height and nowhere else.
        assert_eq!(tree_part_at(&tree, 10, 81, 10), Some(TreePart::Trunk));
        assert_eq!(tree_part_at(&tree, 10, 85, 10), Some(TreePart::Trunk));
        assert_eq!(tree_part_at(&tree, 10, 80, 10), None, "trunk replaced the ground");
        assert_eq!(tree_part_at(&tree, 10, 86, 10), Some(TreePart::Leaves), "no crown");
        // Wide layer at the top of the trunk, minus its corners.
        assert_eq!(tree_part_at(&tree, 12, 85, 12), None, "square corners");
        assert_eq!(tree_part_at(&tree, 12, 85, 11), Some(TreePart::Leaves));
        assert_eq!(tree_part_at(&tree, 12, 84, 12), Some(TreePart::Leaves));
        // Cap narrows.
        assert_eq!(tree_part_at(&tree, 11, 87, 11), None);
        assert_eq!(tree_part_at(&tree, 11, 87, 10), Some(TreePart::Leaves));
        // Nothing outside the canopy reach.
        assert_eq!(tree_part_at(&tree, 13, 85, 10), None);

        // Pin the whole silhouette by cell count: trunk 5, the wide layer
        // minus the trunk cell 24, the corner-cut layer minus the trunk 20,
        // the 3x3 cap 9, the plus-shaped tip 5.
        assert_eq!(cells(&tree), 5 + 24 + 20 + 9 + 5);
    }

    /// Every cell a tree occupies, counted over a box that cannot clip it.
    fn cells(tree: &Tree) -> i32 {
        let mut count = 0;
        for y in tree.base.y - 2..=tree.base.y + tree.height + 4 {
            for x in tree.base.x - CANOPY_REACH..=tree.base.x + CANOPY_REACH {
                for z in tree.base.z - CANOPY_REACH..=tree.base.z + CANOPY_REACH {
                    if tree_part_at(tree, x, y, z).is_some() {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn every_species_is_a_solid_connected_tree() {
        // Whatever the shape, a tree has to be one piece: no floating crown,
        // no leaves adrift, and nothing below the ground it stands on.
        for (species, height) in [
            (Species::Hardwood, 6),
            (Species::Giant, 13),
            (Species::Spruce, 9),
            (Species::BogSpruce, 6),
            (Species::Krummholz, 1),
        ] {
            let tree = Tree {
                base: BlockPos::new(0, 80, 0),
                height,
                species,
            };
            assert!(cells(&tree) > 4, "{species:?} is barely there");
            for y in tree.base.y - 4..=tree.base.y {
                for x in -CANOPY_REACH..=CANOPY_REACH {
                    for z in -CANOPY_REACH..=CANOPY_REACH {
                        assert_eq!(
                            tree_part_at(&tree, x, y, z),
                            None,
                            "{species:?} grows into the ground at {y}"
                        );
                    }
                }
            }
            // Nothing wider than the gather margin, or a canopy would be
            // clipped by the chunk that stamps it.
            for y in tree.base.y..=tree.base.y + height + 4 {
                for x in -CANOPY_REACH - 2..=CANOPY_REACH + 2 {
                    for z in -CANOPY_REACH - 2..=CANOPY_REACH + 2 {
                        if x.abs() > CANOPY_REACH || z.abs() > CANOPY_REACH {
                            assert_eq!(
                                tree_part_at(&tree, x, y, z),
                                None,
                                "{species:?} reaches past the gather margin"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The widest the crown gets, and how deep it runs.
    fn silhouette(tree: &Tree) -> (i32, i32) {
        let (mut radius, mut lowest, mut highest) = (0, i32::MAX, i32::MIN);
        for y in tree.base.y..=tree.base.y + tree.height + 4 {
            for x in -CANOPY_REACH..=CANOPY_REACH {
                for z in -CANOPY_REACH..=CANOPY_REACH {
                    if tree_part_at(tree, tree.base.x + x, y, tree.base.z + z)
                        == Some(TreePart::Leaves)
                    {
                        radius = radius.max(x.abs().max(z.abs()));
                        lowest = lowest.min(y);
                        highest = highest.max(y);
                    }
                }
            }
        }
        (radius, highest - lowest + 1)
    }

    #[test]
    fn each_silhouette_is_the_shape_its_forest_should_read_as() {
        // The silhouette is the whole point of three forests: a spire has to
        // read as a spire from across a valley, and a bog stand has to be
        // thin enough to see the next stem through.
        let at = |species, height| Tree {
            base: BlockPos::new(0, 80, 0),
            height,
            species,
        };
        let (hardwood_r, _) = silhouette(&at(Species::Hardwood, 7));
        let (giant_r, _) = silhouette(&at(Species::Giant, 13));
        let (spruce_r, spruce_deep) = silhouette(&at(Species::Spruce, 9));
        let (bog_r, bog_deep) = silhouette(&at(Species::BogSpruce, 7));
        let (mat_r, mat_deep) = silhouette(&at(Species::Krummholz, 1));

        assert!(giant_r > hardwood_r, "the emergent tree is not emergent");
        assert!(bog_r < spruce_r, "a bog spruce is as fat as a subalpine one");
        // Both conifers are taller in the crown than they are wide across it.
        assert!(spruce_deep > 2 * spruce_r, "the spire is not a spire");
        assert!(bog_deep > 2 * bog_r, "the bog spruce is not a whisker");
        // And the mat is the other way round: wider than it is tall.
        assert!(mat_r * 2 > mat_deep, "krummholz stood up");
    }

    #[test]
    fn tufts_are_scattered_but_not_a_lawn_of_their_own() {
        let mut tufts = 0;
        let mut columns = 0;
        for x in 100..228 {
            for z in 100..228 {
                columns += 1;
                if tuft_at(7, x, z, 80, &home()) {
                    tufts += 1;
                }
            }
        }
        let rate = tufts as f32 / columns as f32;
        assert!(rate > 0.02, "tufts effectively absent: {rate}");
        assert!(rate < 0.15, "tufts carpet everything: {rate}");
    }
}
