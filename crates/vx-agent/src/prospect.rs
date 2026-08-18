//! Finding ore: by eye today, by scanner in stage 4.
//!
//! This module exists because the same ~40 lines of "sweep the surface for an
//! outcrop, then grow a box over the ore under it" had grown in two places —
//! the app's `--dig` path and the real-terrain integration test — and the
//! test's whole premise ("a failure here and a bad screenshot are the same
//! bug") only held while the copies stayed identical. One home, both callers.
//!
//! It is also, deliberately, where the flying drone's scanner lands next: an
//! outcrop sweep is a depth-0 scan, and the scanner is the same walk carried
//! `SCAN_DEPTH` blocks further down.

use std::collections::{HashMap, HashSet};

use vx_core::{BlockPos, CHUNK_SIZE};
use vx_world::World;

use crate::aabb::VoxelAabb;

/// Side length of a scan sector, in chunks.
pub const SECTOR_CHUNKS: i32 = 4;

/// Side length of a scan sector, in blocks.
pub const SECTOR_SIZE: i32 = SECTOR_CHUNKS * CHUNK_SIZE;

/// How far below the surface the scanner can sense ore.
///
/// Depth-0 hits are outcrops anyone can eyeball; depth 1 to `SCAN_DEPTH` is
/// what the scanner is *for*. Bodies deeper than this stay invisible until a
/// scanner upgrade raises it — the same shape as the drone's grade stat.
pub const SCAN_DEPTH: i32 = 24;

/// One square of the scan grid, in sector coordinates.
///
/// Fixed-size and chunk-aligned so scans compose: two sweeps of the same
/// sector cover the same ground, and a map can fill in sector by sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sector {
    pub x: i32,
    pub z: i32,
}

impl Sector {
    /// The sector containing the column `(x, z)`.
    pub fn containing(x: i32, z: i32) -> Sector {
        Sector {
            x: x.div_euclid(SECTOR_SIZE),
            z: z.div_euclid(SECTOR_SIZE),
        }
    }

    /// The lowest-coordinate column inside this sector.
    pub fn min_column(&self) -> (i32, i32) {
        (self.x * SECTOR_SIZE, self.z * SECTOR_SIZE)
    }

    /// Every column in the sector, row by row — also the order a lawnmower
    /// sweep covers them, which is what makes progressive scanning honest.
    pub fn columns(&self) -> impl Iterator<Item = (i32, i32)> {
        let (min_x, min_z) = self.min_column();
        (0..SECTOR_SIZE)
            .flat_map(move |dz| (0..SECTOR_SIZE).map(move |dx| (min_x + dx, min_z + dz)))
    }
}

/// One detected body: the scanner's product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    /// A point in the air over the body — where a marker hovers.
    pub position: BlockPos,
    /// Blocks between the surface and the shallowest ore under it. Zero is an
    /// outcrop; anything more is ore no eye could have found.
    pub depth: i32,
    /// Columns of the sector with ore under them in this body — a rough size,
    /// which is all a survey from the air should give away.
    pub ore_columns: u32,
}

/// Ore within scanner range of the surface at one column.
///
/// Returns `(depth, hover_y)`: how far under the surface the shallowest ore
/// sits, and the altitude a marker over this column should hover at. Walks
/// real blocks rather than querying the deposit lattice, deliberately: the
/// scanner then reflects the world *as it is* — a mined-out body honestly
/// stops pinging, and player-placed ore pings — and the cost is a bounded
/// `SCAN_DEPTH` walk per column.
pub(crate) fn column_hit(world: &World, x: i32, z: i32) -> Option<(i32, i32)> {
    let clear = world.surface_y(x, z)?;
    let top = clear - 1;
    (0..=SCAN_DEPTH)
        .find(|depth| is_ore(world, BlockPos::new(x, top - depth, z)))
        .map(|depth| (depth, clear + 2))
}

/// Cluster hit columns into one [`Ping`] per body.
///
/// Flood-fills 4-connected hits: two bodies whose columns touch merge into
/// one ping, which is what a survey from the air would genuinely be unable to
/// tell apart. Deterministic — hits are visited in sorted column order.
pub(crate) fn cluster_pings(hits: &HashMap<(i32, i32), (i32, i32)>) -> Vec<Ping> {
    let mut columns: Vec<(i32, i32)> = hits.keys().copied().collect();
    columns.sort_unstable();

    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let mut pings = Vec::new();

    for start in columns {
        if seen.contains(&start) {
            continue;
        }
        let mut cluster = Vec::new();
        let mut queue = vec![start];
        seen.insert(start);
        while let Some(column) = queue.pop() {
            cluster.push(column);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = (column.0 + dx, column.1 + dz);
                if hits.contains_key(&next) && seen.insert(next) {
                    queue.push(next);
                }
            }
        }

        let depth = cluster
            .iter()
            .map(|column| hits[column].0)
            .min()
            .expect("a cluster has at least one column");
        let centre_x = cluster.iter().map(|c| c.0).sum::<i32>() / cluster.len() as i32;
        let centre_z = cluster.iter().map(|c| c.1).sum::<i32>() / cluster.len() as i32;
        // The centroid of an L-shaped body can miss the body; hover over the
        // hit column nearest to it instead.
        let &(hover_x, hover_z) = cluster
            .iter()
            .min_by_key(|c| {
                let (dx, dz) = ((c.0 - centre_x) as i64, (c.1 - centre_z) as i64);
                (dx * dx + dz * dz, c.0, c.1)
            })
            .expect("a cluster has at least one column");

        pings.push(Ping {
            position: BlockPos::new(hover_x, hits[&(hover_x, hover_z)].1, hover_z),
            depth,
            ore_columns: cluster.len() as u32,
        });
    }

    pings
}

/// Scan every column of `sector` at once, clustering hits into pings.
///
/// The instantaneous version — what a completed sweep knows. The fleet's
/// progressive scanning covers the same columns a swath at a time and calls
/// [`cluster_pings`] over what it has so far.
pub fn scan_columns(world: &World, sector: Sector) -> Vec<Ping> {
    let hits: HashMap<(i32, i32), (i32, i32)> = sector
        .columns()
        .filter_map(|(x, z)| column_hit(world, x, z).map(|hit| ((x, z), hit)))
        .collect();
    cluster_pings(&hits)
}

/// Does this position hold ore, by the naming convention every ore block
/// follows (`engine:copper_ore`, and stage 9's kin after it)?
///
/// Name-suffix rather than id, for the standard reason: ids shift when mods
/// register blocks, and a modded ore named `*_ore` should prospect like one.
pub fn is_ore(world: &World, pos: BlockPos) -> bool {
    world
        .registry()
        .get(world.block(pos))
        .is_some_and(|def| def.name.ends_with("_ore"))
}

/// The ore body under the visible outcrop nearest to `at`, as a marked area.
///
/// Found the way a player finds one — by looking at the surface. Sweeps the
/// columns within `radius` for exposed ore, takes the nearest, and grows a box
/// over the connected ore beneath and around it. Returns `None` when nothing
/// breaks the surface nearby, which is most places; that scarcity is what the
/// stage-4 scanner exists to get past.
pub fn find_body(world: &World, at: (i32, i32), radius: i32) -> Option<VoxelAabb> {
    let mut nearest: Option<(i64, BlockPos)> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let (x, z) = (at.0 + dx, at.1 + dz);
            let Some(clear) = world.surface_y(x, z) else {
                continue;
            };
            let top = BlockPos::new(x, clear - 1, z);
            if !is_ore(world, top) {
                continue;
            }
            let distance = i64::from(dx) * i64::from(dx) + i64::from(dz) * i64::from(dz);
            if nearest.is_none_or(|(best, _)| distance < best) {
                nearest = Some((distance, top));
            }
        }
    }

    // Grow a box around the outcrop over the ore actually present under it.
    let (_, seed) = nearest?;
    VoxelAabb::containing(
        VoxelAabb::new(seed.offset([-8, -16, -8]), seed.offset([8, 1, 8]))
            .clamped_to_world()
            .blocks()
            .filter(|pos| is_ore(world, *pos)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{flat, ore_body};

    #[test]
    fn sectors_tile_the_plane_including_negative_coordinates() {
        assert_eq!(Sector::containing(0, 0), Sector { x: 0, z: 0 });
        assert_eq!(Sector::containing(63, 63), Sector { x: 0, z: 0 });
        assert_eq!(Sector::containing(64, 0), Sector { x: 1, z: 0 });
        // div_euclid, not integer division: column -1 is in sector -1.
        assert_eq!(Sector::containing(-1, -1), Sector { x: -1, z: -1 });
        assert_eq!(Sector::containing(-64, 0), Sector { x: -1, z: 0 });

        let sector = Sector { x: -1, z: 2 };
        assert!(sector.columns().all(|(x, z)| Sector::containing(x, z) == sector));
        assert_eq!(sector.columns().count(), (SECTOR_SIZE * SECTOR_SIZE) as usize);
    }

    #[test]
    fn two_separate_bodies_yield_two_pings() {
        let mut world = flat(5, 60);
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(5, 52, 5), BlockPos::new(8, 55, 8)));
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(40, 45, 40), BlockPos::new(44, 50, 44)));

        let pings = scan_columns(&world, Sector { x: 0, z: 0 });
        assert_eq!(pings.len(), 2, "got {pings:?}");
        // Sizes and depths distinguish them, whatever the order.
        let mut depths: Vec<i32> = pings.iter().map(|ping| ping.depth).collect();
        depths.sort_unstable();
        assert_eq!(depths, vec![5, 10]);
    }

    #[test]
    fn scanning_is_deterministic() {
        let mut world = flat(5, 60);
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(5, 52, 5), BlockPos::new(9, 56, 9)));
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(30, 40, 30), BlockPos::new(34, 44, 34)));

        let sector = Sector { x: 0, z: 0 };
        assert_eq!(scan_columns(&world, sector), scan_columns(&world, sector));
    }

    #[test]
    fn a_surfaced_body_is_found_and_boxed() {
        let mut world = flat(3, 60);
        let body = VoxelAabb::new(BlockPos::new(4, 55, 4), BlockPos::new(7, 60, 7));
        ore_body(&mut world, body);

        let found = find_body(&world, (0, 0), 20).expect("the outcrop was missed");
        assert_eq!(found, body, "the grown box does not match the body");
    }

    #[test]
    fn a_fully_buried_body_is_invisible_to_the_eye() {
        // The negative case is the important one: find_body models *looking*,
        // and a body with no outcrop must not be findable, or the scanner the
        // flier carries would have nothing to be for.
        let mut world = flat(3, 60);
        let buried = VoxelAabb::new(BlockPos::new(4, 40, 4), BlockPos::new(7, 45, 7));
        ore_body(&mut world, buried);

        assert!(find_body(&world, (5, 5), 20).is_none());
    }

    #[test]
    fn the_nearest_of_two_outcrops_wins() {
        let mut world = flat(4, 60);
        let near = VoxelAabb::new(BlockPos::new(6, 57, 6), BlockPos::new(8, 60, 8));
        let far = VoxelAabb::new(BlockPos::new(30, 57, 30), BlockPos::new(32, 60, 32));
        ore_body(&mut world, near);
        ore_body(&mut world, far);

        let found = find_body(&world, (0, 0), 48).expect("nothing found at all");
        assert_eq!(found, near);
    }
}
