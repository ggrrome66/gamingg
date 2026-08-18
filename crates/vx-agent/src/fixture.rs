//! Hand-built worlds for tests.
//!
//! Generated terrain is the wrong thing to plan mines against in a unit test:
//! the answers depend on whatever the noise happened to produce, so a failure
//! says nothing about the planner. These build chunks directly instead, giving
//! an exactly known ground profile.
//!
//! Building chunks rather than editing generated ones also matters for
//! correctness, not just speed. Overwriting a slab of a generated world leaves
//! whatever the generator put *above* the slab in place, and `surface_y` then
//! reports that instead — which is a mistake worth only making once.

use vx_core::{BlockPos, ChunkPos, CHUNK_SIZE};
use vx_world::{Chunk, World};

use crate::aabb::VoxelAabb;

/// A world whose ground height is `height(world_x, world_z)` everywhere.
///
/// `radius` is in chunks. Declines can need a long run-up, so tests that plan
/// deep mines need a wide enough world for the portal to land inside it.
pub fn shaped_xz(radius: i32, height: impl Fn(i32, i32) -> i32) -> World {
    let mut world = World::new(1);
    let stone = world
        .registry()
        .id_of("engine:stone")
        .expect("the built-in registry has stone");

    for chunk_x in -radius..=radius {
        for chunk_z in -radius..=radius {
            let pos = ChunkPos::new(chunk_x, chunk_z);
            let mut chunk = Chunk::empty(pos);
            let origin = pos.origin();
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let top = height(origin.x + x, origin.z + z);
                    chunk.fill_column(x, z, 0, top + 1, stone);
                }
            }
            world.insert_chunk(chunk);
        }
    }
    world
}

/// A world whose ground height depends on `world_x` only.
pub fn shaped(radius: i32, height: impl Fn(i32) -> i32) -> World {
    shaped_xz(radius, move |x, _| height(x))
}

/// Flat stone up to and including `floor`.
pub fn flat(radius: i32, floor: i32) -> World {
    shaped(radius, move |_| floor)
}

/// Flat at `high` until `crest`, then falling `fall` blocks per block eastward.
///
/// The steepness matters to what gets planned, not just to how it looks. On a
/// gentle slope a decline can start part-way down and cut a short tunnel, so it
/// beats an adit; it takes real relief — a valley wall — before driving in
/// level is the cheaper way, which is exactly why real adits are in valley
/// walls.
pub fn slope(radius: i32, high: i32, crest: i32, fall: i32) -> World {
    shaped(radius, move |x| {
        if x < crest {
            high
        } else {
            (high - (x - crest) * fall).max(8)
        }
    })
}

/// A slope falling one block per block: gentle enough to drive up.
pub fn hillside(radius: i32, high: i32, crest: i32) -> World {
    slope(radius, high, crest, 1)
}

/// Replace a box with copper ore.
pub fn ore_body(world: &mut World, box_: VoxelAabb) {
    let ore = world
        .registry()
        .id_of("engine:copper_ore")
        .expect("the built-in registry has copper ore");
    for pos in box_.blocks() {
        world.set_block(pos, ore);
    }
}

/// Solid blocks inside a box.
pub fn solid_count(world: &World, region: VoxelAabb) -> u64 {
    region
        .clamped_to_world()
        .blocks()
        .filter(|pos| world.is_solid(*pos))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mine::ground_height;

    #[test]
    fn a_flat_fixture_reports_the_floor_it_was_asked_for() {
        // The bug this module exists to prevent: a world whose reported surface
        // is not the one the test thinks it built.
        let world = flat(2, 60);
        for x in [-30, -1, 0, 17, 29] {
            assert_eq!(ground_height(&world, x, 0), Some(60), "at x={x}");
        }
    }

    #[test]
    fn nothing_lurks_above_the_floor() {
        let world = flat(1, 40);
        for y in 41..vx_core::CHUNK_HEIGHT {
            assert!(!world.is_solid(BlockPos::new(0, y, 0)), "solid block at y={y}");
        }
    }

    #[test]
    fn a_hillside_falls_one_block_per_block() {
        let world = hillside(2, 60, 0);
        assert_eq!(ground_height(&world, -5, 0), Some(60));
        assert_eq!(ground_height(&world, 0, 0), Some(60));
        assert_eq!(ground_height(&world, 4, 0), Some(56));
        assert_eq!(ground_height(&world, 10, 0), Some(50));
    }

    #[test]
    fn the_world_ends_where_the_radius_says() {
        // Planners rely on `ground_height` returning None off the loaded world
        // to stop searching, so the edge has to be where it is claimed to be.
        let world = flat(1, 60);
        assert!(ground_height(&world, 20, 0).is_some());
        assert!(ground_height(&world, 400, 0).is_none());
    }
}
