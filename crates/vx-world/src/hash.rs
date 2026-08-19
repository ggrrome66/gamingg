//! Content hashes over world state.
//!
//! # Why the engine wants one
//!
//! Worldgen is a pure function of `(seed, position)` and agents are
//! bit-identical given the same inputs. Those are house rules, but until now
//! nothing has been able to *check* them across the whole system — a unit test
//! proves one drone digs the same hole twice, not that the world as a whole
//! came out the same. A cheap 64-bit summary of world state makes that
//! checkable, and it is the primitive the replay save format is built on.
//!
//! # Hashing logical content, not its representation
//!
//! The obvious implementation — fold the palette and the packed index words —
//! is wrong, and wrong in a way that would only show up much later. Two chunks
//! holding *identical blocks* can carry different palettes: one built by
//! generation, one restored from a save, one that had a block placed and broken
//! again. Their packed words differ; their content does not. A hash that
//! disagreed about those would report a false divergence on every reload.
//!
//! So the hash runs over `(local index, block name)` pairs, keyed on the
//! namespaced name for the same reason saves are — [`vx_core::BlockId`] is a
//! registration-order index and shifts when a mod is installed.
//!
//! Air is skipped rather than hashed. It is the overwhelming majority of any
//! chunk, it is the default a missing block decodes to, and skipping it makes
//! the hash both faster and independent of whether a chunk stores air
//! explicitly.
//!
//! # Cost
//!
//! Linear in non-air blocks, with one hash-map lookup per distinct block kind
//! per chunk and a couple of multiplies per block. That is fine at save time,
//! at keyframe time and in tests — the three places it is used. It is **not**
//! cheap enough for a per-frame call, and nothing should add one.

use std::collections::HashMap;

use vx_core::{BlockId, BlockPos, BlockRegistry, ChunkPos, CHUNK_SIZE};

use crate::chunk::Chunk;
use crate::world::World;

/// The splitmix64 finaliser the ore, flora and town lattices all use.
fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Hash a block's namespaced name.
fn name_hash(name: &str) -> u64 {
    // FNV-1a over the bytes, finished with splitmix64 so short names still
    // avalanche. Names are short and the result is memoised per chunk.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    mix(hash)
}

/// A 64-bit summary of one chunk's contents, including where it sits.
///
/// Independent of palette order and of whether air is stored explicitly, so
/// two chunks holding the same blocks hash the same however they were built.
pub fn chunk_hash(chunk: &Chunk, registry: &BlockRegistry) -> u64 {
    let mut names: HashMap<BlockId, u64> = HashMap::new();
    let pos = chunk.pos();

    // The position seeds the fold, so an identical chunk elsewhere in the world
    // is not the same chunk.
    let mut hash = mix(
        (pos.x as i64 as u64).wrapping_mul(0x2545_f491_4f6c_dd1d)
            ^ (pos.z as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ pos.body.0,
    );

    for (local, id) in chunk.iter_blocks() {
        if id == BlockId::AIR {
            continue;
        }
        let name = *names
            .entry(id)
            .or_insert_with(|| name_hash(&registry.get_or_air(id).name));
        // XOR-fold per block so the order blocks are visited in cannot matter.
        hash ^= mix(name ^ (local.index() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    }
    hash
}

/// A summary of every chunk currently loaded.
///
/// XOR-folded across chunks, so the iteration order of the chunk map cannot
/// leak into the result. Note this covers the **loaded set** — two worlds with
/// the same terrain but different chunks resident will differ. Use
/// [`region_hash`] to compare a fixed volume regardless of residency.
pub fn world_hash(world: &World) -> u64 {
    world
        .loaded_chunks()
        .filter_map(|pos| world.chunk(pos))
        .fold(0, |acc, chunk| acc ^ chunk_hash(chunk, world.registry()))
}

/// A summary of one block box, independent of which chunks happen to be loaded.
///
/// Blocks outside loaded chunks read as air and are skipped, exactly as
/// [`World::block`] reports them — so this answers "what does the world look
/// like here, as far as anything can currently tell".
pub fn region_hash(world: &World, min: BlockPos, max: BlockPos) -> u64 {
    let mut names: HashMap<BlockId, u64> = HashMap::new();
    let mut hash = 0u64;

    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let at = BlockPos::new(x, y, z);
                let id = world.block(at);
                if id == BlockId::AIR {
                    continue;
                }
                let name = *names
                    .entry(id)
                    .or_insert_with(|| name_hash(&world.registry().get_or_air(id).name));
                hash ^= mix(name
                    ^ mix((x as i64 as u64)
                        .wrapping_mul(0x2545_f491_4f6c_dd1d)
                        ^ (y as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        ^ (z as i64 as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9)));
            }
        }
    }
    hash
}

/// Every chunk overlapping a block box, for callers that need to pin or gather.
pub fn chunks_overlapping(min: BlockPos, max: BlockPos) -> impl Iterator<Item = ChunkPos> {
    let lo_x = min.x.div_euclid(CHUNK_SIZE);
    let hi_x = max.x.div_euclid(CHUNK_SIZE);
    let lo_z = min.z.div_euclid(CHUNK_SIZE);
    let hi_z = max.z.div_euclid(CHUNK_SIZE);
    (lo_x..=hi_x).flat_map(move |x| (lo_z..=hi_z).map(move |z| ChunkPos::new(x, z)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::CHUNK_HEIGHT;

    fn world_at(seed: u64, radius: i32) -> World {
        let mut world = World::new(seed);
        world.load_around(ChunkPos::new(0, 0), radius);
        world
    }

    #[test]
    fn the_same_world_hashes_the_same_and_a_different_seed_does_not() {
        let a = world_at(2024, 1);
        let b = world_at(2024, 1);
        assert_eq!(world_hash(&a), world_hash(&b));
        assert_ne!(
            world_hash(&a),
            world_hash(&world_at(99, 1)),
            "two seeds produced the same world"
        );
    }

    #[test]
    fn one_changed_block_changes_the_hash() {
        let mut world = world_at(2024, 0);
        let before = world_hash(&world);
        let ground = world.surface_y(3, 3).expect("the origin chunk is loaded");
        world.set_block(BlockPos::new(3, ground - 1, 3), BlockId::AIR);
        assert_ne!(before, world_hash(&world), "a broken block left no trace");
    }

    #[test]
    fn the_hash_does_not_depend_on_palette_order() {
        // The subtle one, and the reason this hashes names rather than packed
        // words: place a block and break it again and the chunk's palette has
        // grown an entry that the original never had. The contents are
        // identical; the representation is not.
        let mut world = world_at(2024, 0);
        let before = world_hash(&world);

        let sand = world.registry().id_of("engine:sand").expect("built-in sand");
        let at = BlockPos::new(5, CHUNK_HEIGHT - 4, 5);
        let previous = world.block(at);
        world.set_block(at, sand);
        assert_ne!(before, world_hash(&world));

        world.set_block(at, previous);
        assert_eq!(
            before,
            world_hash(&world),
            "putting the block back did not restore the hash"
        );
    }

    #[test]
    fn a_region_hash_ignores_what_is_loaded_outside_it() {
        // What the excavation determinism gate leans on: the same volume
        // hashes the same however much world surrounds it.
        let narrow = world_at(2024, 0);
        let wide = world_at(2024, 3);
        let min = BlockPos::new(1, 60, 1);
        let max = BlockPos::new(14, 80, 14);
        assert_eq!(
            region_hash(&narrow, min, max),
            region_hash(&wide, min, max),
            "a region's hash moved because unrelated chunks were resident"
        );
    }

    #[test]
    fn chunks_overlapping_covers_the_box_and_nothing_else() {
        let found: Vec<ChunkPos> =
            chunks_overlapping(BlockPos::new(-1, 0, 0), BlockPos::new(16, 0, 0)).collect();
        assert_eq!(
            found,
            vec![
                ChunkPos::new(-1, 0),
                ChunkPos::new(0, 0),
                ChunkPos::new(1, 0)
            ],
            "the span straddling two seams was not covered"
        );
    }

}
