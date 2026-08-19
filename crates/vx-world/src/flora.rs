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

use vx_core::BlockPos;

use crate::gen::SEA_LEVEL;
use crate::town::{self, TownSite};

/// Lattice cell size for trees, in blocks. One potential tree per cell.
pub const TREE_CELL: i32 = 12;

/// Fraction of cells that grow their tree.
const TREE_PRESENCE: f32 = 0.35;

/// Density of grass tufts on eligible columns.
const TUFT_DENSITY: f32 = 0.07;

/// Canopy half-width; also the gather margin for chunk overlap.
pub const CANOPY_REACH: i32 = 2;

/// One tree: a trunk from the ground and a blob of leaves at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tree {
    /// The surface block the trunk stands on.
    pub base: BlockPos,
    /// Trunk length above the base.
    pub height: i32,
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
/// asks about this cell derives the identical tree.
fn tree_in_cell(
    seed: u64,
    cell_x: i32,
    cell_z: i32,
    height_at: &impl Fn(i32, i32) -> i32,
    sites: &[TownSite],
) -> Option<Tree> {
    if hash01(seed, 0xf1, cell_x, cell_z) > TREE_PRESENCE {
        return None;
    }
    // Jitter within the cell, keeping the canopy inside the cell's margin.
    let jitter_x = (hash01(seed, 0xf2, cell_x, cell_z) * (TREE_CELL - 1) as f32) as i32;
    let jitter_z = (hash01(seed, 0xf3, cell_x, cell_z) * (TREE_CELL - 1) as f32) as i32;
    let x = cell_x * TREE_CELL + jitter_x;
    let z = cell_z * TREE_CELL + jitter_z;

    let surface = height_at(x, z);
    if !ground_grows_grass(sites, x, z, surface) {
        return None;
    }

    let height = 4 + (hash01(seed, 0xf4, x, z) * 3.0) as i32; // 4..=6
    Some(Tree {
        base: BlockPos::new(x, surface, z),
        height,
    })
}

/// Every tree whose trunk or canopy could reach the box `min..=max` (x/z).
pub fn trees_overlapping(
    seed: u64,
    min: (i32, i32),
    max: (i32, i32),
    height_at: &impl Fn(i32, i32) -> i32,
    sites: &[TownSite],
) -> Vec<Tree> {
    let lo_x = (min.0 - CANOPY_REACH).div_euclid(TREE_CELL);
    let hi_x = (max.0 + CANOPY_REACH).div_euclid(TREE_CELL);
    let lo_z = (min.1 - CANOPY_REACH).div_euclid(TREE_CELL);
    let hi_z = (max.1 + CANOPY_REACH).div_euclid(TREE_CELL);

    let mut found = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_z in lo_z..=hi_z {
            if let Some(tree) = tree_in_cell(seed, cell_x, cell_z, height_at, sites) {
                found.push(tree);
            }
        }
    }
    found
}

/// The part of `tree` occupying a world position, if any.
///
/// The classic shape: a one-block trunk, two wide leaf layers around the
/// crown with the corners knocked off the top one, then a narrow cap.
pub fn tree_part_at(tree: &Tree, x: i32, y: i32, z: i32) -> Option<TreePart> {
    let top = tree.base.y + tree.height;
    let (dx, dz) = (x - tree.base.x, z - tree.base.z);

    if dx == 0 && dz == 0 && y > tree.base.y && y <= top {
        return Some(TreePart::Trunk);
    }

    let (adx, adz) = (dx.abs(), dz.abs());
    let leaves = match y - top {
        -1 => adx <= CANOPY_REACH && adz <= CANOPY_REACH,
        0 => adx <= CANOPY_REACH && adz <= CANOPY_REACH && !(adx == CANOPY_REACH && adz == CANOPY_REACH),
        1 => adx <= 1 && adz <= 1,
        2 => adx + adz <= 1,
        _ => false,
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
        let all = trees_overlapping(7, (-200, -200), (200, 200), &flat_height, &home());
        assert!(!all.is_empty(), "no trees anywhere in a 400-block square");

        // Every tree found by a small window is in the big list, and every
        // big-list tree overlapping the window is found by it.
        let window = trees_overlapping(7, (0, 0), (15, 15), &flat_height, &home());
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
            trees_overlapping(7, (0, 0), (15, 15), &flat_height, &home()),
            window,
            "same seed, same window, different forest"
        );
    }

    #[test]
    fn no_tree_grows_in_town_or_on_the_beach() {
        let sites = home();
        let half = town::HOME_CORE_HALF;
        let in_town = trees_overlapping(7, (-half, -half), (half, half), &flat_height, &sites);
        for tree in in_town {
            assert!(
                !town::footprint_contains(&sites, tree.base.x, tree.base.z),
                "a tree took root on main street: {tree:?}"
            );
        }

        let on_beach = trees_overlapping(7, (300, 300), (500, 500), &|_, _| SEA_LEVEL, &sites);
        assert!(on_beach.is_empty(), "trees growing on the sea floor");

        assert!(
            !tuft_at(7, 0, 0, town::HOME_GROUND_Y, &sites),
            "a tuft on the plaza"
        );
    }

    #[test]
    fn the_tree_shape_holds_together() {
        let tree = Tree {
            base: BlockPos::new(10, 80, 10),
            height: 5,
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
        let mut cells = 0;
        for y in 78..92 {
            for x in 5..16 {
                for z in 5..16 {
                    if tree_part_at(&tree, x, y, z).is_some() {
                        cells += 1;
                    }
                }
            }
        }
        assert_eq!(cells, 5 + 24 + 20 + 9 + 5);
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
