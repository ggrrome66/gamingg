//! World state: the set of loaded chunks and block access across them.
//!
//! `World` owns simulation state only. It never references the renderer or the
//! windowing layer, which is what keeps the client/server split available: the
//! same type can later run headless behind a socket without changes.

use std::collections::HashMap;

use vx_core::{BlockId, BlockPos, BlockRegistry, ChunkPos, Face};

use crate::chunk::{BlockView, Chunk};
use crate::gen::{TerrainBlocks, TerrainGenerator};

/// Loaded chunks plus the generator that fills in missing ones.
pub struct World {
    registry: BlockRegistry,
    generator: TerrainGenerator,
    chunks: HashMap<ChunkPos, Chunk>,
    /// Bumped on every effective block change. Cheap cache invalidation for
    /// anything derived from world contents — a consumer stores the count it
    /// built against and rebuilds when it moves. Monotonic, never reset.
    edit_count: u64,
    /// Chunks something is actively working in, and how many things.
    ///
    /// A drone reads the world through [`World::block`], which reports
    /// unloaded ground as air. That is deliberately conservative — a machine
    /// will not drive onto ground it cannot see — but it means a drone's
    /// decisions depend on which chunks happen to be resident, and residency
    /// follows the camera. Two runs of the same excavation could diverge
    /// purely because the player stood somewhere different.
    ///
    /// Pinning closes that: an operation declares the span it will work in,
    /// that span is loaded up front, and streaming may not evict it while the
    /// work lasts. Counted rather than a flag, so overlapping operations
    /// cannot unpin each other's ground.
    pinned: HashMap<ChunkPos, u32>,
}

impl World {
    /// A world with the engine's built-in blocks registered.
    pub fn new(seed: u64) -> Self {
        let mut registry = BlockRegistry::new();
        let blocks = TerrainBlocks::register_builtins(&mut registry);
        World {
            registry,
            generator: TerrainGenerator::new(seed, blocks),
            chunks: HashMap::new(),
            edit_count: 0,
            pinned: HashMap::new(),
        }
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    /// How many effective block edits this world has seen.
    ///
    /// The number itself means nothing; only "has it changed since I looked"
    /// does. No-op writes (setting a block to what it already is) do not count,
    /// matching the dirty-marking rule above.
    pub fn edit_count(&self) -> u64 {
        self.edit_count
    }

    pub fn generator(&self) -> &TerrainGenerator {
        &self.generator
    }

    pub fn seed(&self) -> u64 {
        self.generator.seed()
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Every chunk currently resident, in unspecified order.
    pub fn loaded_chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    /// Insert an already-built chunk, replacing anything at its position.
    ///
    /// Used to bring a chunk in from disk rather than generating it.
    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert(chunk.pos(), chunk);
    }

    /// Load `pos`, generating it if it is not already resident.
    pub fn load_chunk(&mut self, pos: ChunkPos) -> &Chunk {
        self.chunks
            .entry(pos)
            .or_insert_with(|| self.generator.generate(pos))
    }

    /// Load every chunk within `radius` of `centre`, returning how many were
    /// newly generated.
    pub fn load_around(&mut self, centre: ChunkPos, radius: i32) -> usize {
        let mut generated = 0;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let pos = ChunkPos::new(centre.x + dx, centre.z + dz);
                if pos.distance_squared(centre) > (radius as i64) * (radius as i64) {
                    continue;
                }
                if !self.is_loaded(pos) {
                    self.load_chunk(pos);
                    generated += 1;
                }
            }
        }
        generated
    }

    /// Drop chunks further than `radius` from `centre`, returning how many
    /// were unloaded.
    ///
    /// Chunks still carrying unsaved edits are kept, so walking away from
    /// something you built cannot discard it before it reaches disk.
    pub fn unload_beyond(&mut self, centre: ChunkPos, radius: i32) -> usize {
        let limit = (radius as i64) * (radius as i64);
        let before = self.chunks.len();
        let pinned = &self.pinned;
        self.chunks.retain(|pos, chunk| {
            pos.distance_squared(centre) <= limit
                || chunk.is_modified()
                || pinned.contains_key(pos)
        });
        before - self.chunks.len()
    }

    /// Load every chunk overlapping a block box and hold it against eviction.
    ///
    /// Returns the chunks pinned, which is also the set
    /// [`World::unpin_span`] must be given to release them. Call this before
    /// dispatching anything that will read the world inside `min..=max`: what
    /// it buys is that the work reads real ground rather than the air an
    /// unloaded chunk reports, and therefore behaves the same however the
    /// player wandered while it ran.
    pub fn pin_span(&mut self, min: BlockPos, max: BlockPos) -> Vec<ChunkPos> {
        let span: Vec<ChunkPos> = crate::hash::chunks_overlapping(min, max).collect();
        for pos in &span {
            self.load_chunk(*pos);
            *self.pinned.entry(*pos).or_insert(0) += 1;
        }
        span
    }

    /// Release a pin taken by [`World::pin_span`]. Unbalanced calls are
    /// ignored rather than underflowing — losing a pin is recoverable, a panic
    /// in the middle of a mine is not.
    pub fn unpin_span(&mut self, span: &[ChunkPos]) {
        for pos in span {
            if let Some(count) = self.pinned.get_mut(pos) {
                *count -= 1;
                if *count == 0 {
                    self.pinned.remove(pos);
                }
            }
        }
    }

    pub fn is_pinned(&self, pos: ChunkPos) -> bool {
        self.pinned.contains_key(&pos)
    }

    pub fn pinned_count(&self) -> usize {
        self.pinned.len()
    }

    /// Block at a world position. Unloaded chunks and out-of-bounds heights
    /// read as air.
    pub fn block(&self, pos: BlockPos) -> BlockId {
        let Some(local) = pos.local() else {
            return BlockId::AIR;
        };
        self.chunks
            .get(&pos.chunk())
            .map_or(BlockId::AIR, |chunk| chunk.get(local))
    }

    /// Write a block, returning the previous one. Fails if the position is
    /// outside the world height or its chunk is not loaded.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> Option<BlockId> {
        let local = pos.local()?;
        let chunk = self.chunks.get_mut(&pos.chunk())?;
        let previous = chunk.set(local, block);

        // A block on a chunk edge changes what the neighbour's seam faces look
        // like, so that chunk needs remeshing too.
        if previous != block {
            self.dirty_touching_neighbours(pos);
            self.edit_count += 1;
        }
        Some(previous)
    }

    /// This block's wound, if violence has given it one.
    pub fn mask(&self, pos: BlockPos) -> Option<crate::micro::Mask> {
        let local = pos.local()?;
        self.chunks.get(&pos.chunk())?.mask(local)
    }

    /// Take a shape out of a block, and say what became of it.
    ///
    /// The block gains an interior the first time it is hit and loses
    /// exactly the cells the shape removed; when too little is left it
    /// becomes air, which is the only outcome the rest of the game already
    /// knew how to have. Everything here is integer arithmetic on one
    /// register, so two runs fed the same carves agree bit for bit — which
    /// is what lets wounds ride the replay oracle without the journal ever
    /// learning they exist.
    pub fn carve(&mut self, pos: BlockPos, shape: crate::micro::Mask) -> Carved {
        let Some(local) = pos.local() else {
            return Carved::Nothing;
        };
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return Carved::Nothing;
        };
        if chunk.get(local).is_air() {
            return Carved::Nothing;
        }
        let before = chunk.mask(local).unwrap_or(crate::micro::FULL);
        let after = crate::micro::carve(before, shape);
        if after == before {
            return Carved::Nothing;
        }
        if crate::micro::dead(after) {
            // What is left is rubble, not cover. The block goes, by the same
            // path every other break takes.
            let previous = chunk.set(local, BlockId::AIR);
            self.dirty_touching_neighbours(pos);
            self.edit_count += 1;
            return Carved::Broke(previous);
        }
        chunk.set_mask(local, after);
        // A wound on a chunk edge changes what the neighbour draws along the
        // seam, exactly as a broken block does.
        self.dirty_touching_neighbours(pos);
        self.edit_count += 1;
        Carved::Wounded(after)
    }

    /// Mark neighbouring chunks dirty when `pos` sits on a shared edge.
    fn dirty_touching_neighbours(&mut self, pos: BlockPos) {
        let own = pos.chunk();
        for face in Face::ALL {
            let neighbour_chunk = pos.neighbour(face).chunk();
            if neighbour_chunk != own {
                if let Some(chunk) = self.chunks.get_mut(&neighbour_chunk) {
                    chunk.mark_dirty();
                }
            }
        }
    }

    /// Chunks needing a mesh rebuild.
    pub fn dirty_chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks
            .iter()
            .filter(|(_, chunk)| chunk.is_dirty())
            .map(|(pos, _)| *pos)
    }

    pub fn clear_dirty(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.clear_dirty();
        }
    }

    /// True when the block at `pos` blocks movement.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        self.registry.is_solid(self.block(pos))
    }

    /// A safe standing height above `(x, z)`: one block above the surface.
    /// Returns `None` if the column's chunk is not loaded.
    pub fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        let chunk_pos = BlockPos::new(x, 0, z).chunk();
        let chunk = self.chunks.get(&chunk_pos)?;
        let local = BlockPos::new(x, 0, z).local()?;
        chunk
            .height_at(local.x(), local.z())
            .map(|top| top + 1)
    }
}

impl BlockView for World {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.block(BlockPos::new(x, y, z))
    }

    fn mask_at(&self, x: i32, y: i32, z: i32) -> Option<crate::micro::Mask> {
        self.mask(BlockPos::new(x, y, z))
    }
}

/// What a carve did to a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carved {
    /// Nothing there to damage, or the shape took nothing this block still had.
    Nothing,
    /// Still standing, with this much of it left.
    Wounded(crate::micro::Mask),
    /// Too little left to be a block: it is air now, and this is what it was.
    Broke(BlockId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{CHUNK_HEIGHT, CHUNK_SIZE};

    #[test]
    fn a_new_world_has_no_chunks_loaded() {
        let world = World::new(1);
        assert_eq!(world.loaded_chunk_count(), 0);
        assert!(!world.is_loaded(ChunkPos::new(0, 0)));
        // Reading unloaded space is air, not a panic.
        assert!(world.block(BlockPos::new(0, 64, 0)).is_air());
    }

    #[test]
    fn loading_a_chunk_generates_it_once() {
        let mut world = World::new(42);
        world.load_chunk(ChunkPos::new(0, 0));
        assert_eq!(world.loaded_chunk_count(), 1);

        world.load_chunk(ChunkPos::new(0, 0));
        assert_eq!(world.loaded_chunk_count(), 1, "reloading must not duplicate");
    }

    #[test]
    fn load_around_fills_a_disc_and_reports_new_chunks() {
        let mut world = World::new(3);
        let generated = world.load_around(ChunkPos::new(0, 0), 2);

        assert_eq!(generated, world.loaded_chunk_count());
        assert!(world.is_loaded(ChunkPos::new(0, 0)));
        assert!(world.is_loaded(ChunkPos::new(2, 0)));
        // Corners fall outside the radius.
        assert!(!world.is_loaded(ChunkPos::new(2, 2)));

        // Loading again generates nothing new.
        assert_eq!(world.load_around(ChunkPos::new(0, 0), 2), 0);
    }

    #[test]
    fn unloading_drops_only_distant_chunks() {
        let mut world = World::new(3);
        world.load_around(ChunkPos::new(0, 0), 3);
        let before = world.loaded_chunk_count();

        let dropped = world.unload_beyond(ChunkPos::new(0, 0), 1);

        assert!(dropped > 0);
        assert_eq!(world.loaded_chunk_count(), before - dropped);
        assert!(world.is_loaded(ChunkPos::new(0, 0)));
        assert!(world.is_loaded(ChunkPos::new(1, 0)));
        assert!(!world.is_loaded(ChunkPos::new(3, 0)));
    }

    #[test]
    fn writing_a_block_reads_back_and_reports_the_previous_value() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4); // well above terrain, so it is air

        let previous = world.set_block(pos, stone);

        assert_eq!(previous, Some(BlockId::AIR));
        assert_eq!(world.block(pos), stone);
        assert!(world.is_solid(pos));
    }

    #[test]
    fn writing_into_unloaded_or_out_of_bounds_space_fails_cleanly() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Chunk not loaded.
        assert_eq!(world.set_block(BlockPos::new(1000, 64, 1000), stone), None);
        // Above and below the world.
        assert_eq!(world.set_block(BlockPos::new(0, CHUNK_HEIGHT, 0), stone), None);
        assert_eq!(world.set_block(BlockPos::new(0, -1, 0), stone), None);
    }

    #[test]
    fn editing_a_chunk_edge_dirties_the_neighbouring_chunk() {
        // Without this, seam faces go stale and you get holes between chunks.
        let mut world = World::new(11);
        world.load_chunk(ChunkPos::new(0, 0));
        world.load_chunk(ChunkPos::new(-1, 0));
        for pos in [ChunkPos::new(0, 0), ChunkPos::new(-1, 0)] {
            world.clear_dirty(pos);
        }

        let stone = world.registry().id_of("engine:stone").unwrap();
        world.set_block(BlockPos::new(0, 200, 4), stone);

        let dirty: Vec<_> = world.dirty_chunks().collect();
        assert!(dirty.contains(&ChunkPos::new(0, 0)), "edited chunk must be dirty");
        assert!(
            dirty.contains(&ChunkPos::new(-1, 0)),
            "chunk across the seam must be dirty too"
        );
    }

    #[test]
    fn editing_the_middle_of_a_chunk_leaves_neighbours_clean() {
        let mut world = World::new(11);
        world.load_around(ChunkPos::new(0, 0), 1);
        let positions: Vec<_> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
            .collect();
        for pos in &positions {
            world.clear_dirty(*pos);
        }

        let stone = world.registry().id_of("engine:stone").unwrap();
        world.set_block(BlockPos::new(8, 200, 8), stone);

        let dirty: Vec<_> = world.dirty_chunks().collect();
        assert_eq!(dirty, vec![ChunkPos::new(0, 0)]);
    }

    #[test]
    fn a_redundant_write_does_not_dirty_anything() {
        let mut world = World::new(11);
        world.load_chunk(ChunkPos::new(0, 0));
        world.clear_dirty(ChunkPos::new(0, 0));

        // Rewriting air over air changes nothing.
        world.set_block(BlockPos::new(0, 200, 0), BlockId::AIR);

        assert_eq!(world.dirty_chunks().count(), 0);
    }

    #[test]
    fn block_view_reads_across_chunk_boundaries() {
        let mut world = World::new(5);
        world.load_chunk(ChunkPos::new(0, 0));
        world.load_chunk(ChunkPos::new(-1, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Last column of the chunk to the west.
        world.set_block(BlockPos::new(-1, 200, 0), stone);

        assert_eq!(world.block_at(-1, 200, 0), stone);
        assert_eq!(world.block_at(0, 200, 0), BlockId::AIR);
        // Beyond loaded chunks reads as air rather than panicking.
        assert!(world.block_at(9999, 200, 9999).is_air());
    }

    #[test]
    fn surface_y_lands_just_above_the_terrain() {
        let mut world = World::new(2468);
        world.load_chunk(ChunkPos::new(0, 0));

        for x in 0..CHUNK_SIZE {
            let spawn = world.surface_y(x, 0).unwrap();
            assert!(!world.is_solid(BlockPos::new(x, spawn, 0)), "spawn is inside a block");
            assert!(
                world.block(BlockPos::new(x, spawn - 1, 0)) != BlockId::AIR,
                "spawn is floating above the surface"
            );
        }

        assert_eq!(world.surface_y(500, 500), None, "unloaded columns have no surface");
    }

    #[test]
    fn pinned_ground_survives_the_player_walking_away() {
        // The whole point of pinning: an operation's ground must not be
        // evicted just because the camera left. Without this a drone would
        // start reading air where it had been digging rock.
        let mut world = World::new(2024);
        let span = world.pin_span(BlockPos::new(0, 0, 0), BlockPos::new(31, 0, 31));
        assert_eq!(span.len(), 4, "a 32x32 box spans four chunks");
        assert!(span.iter().all(|pos| world.is_loaded(*pos)));

        // Stream far away. Ordinarily every one of these would go.
        world.unload_beyond(ChunkPos::new(500, 500), 2);
        assert!(
            span.iter().all(|pos| world.is_loaded(*pos)),
            "streaming evicted ground an operation was working in"
        );

        world.unpin_span(&span);
        assert_eq!(world.pinned_count(), 0);
        world.unload_beyond(ChunkPos::new(500, 500), 2);
        assert!(
            span.iter().all(|pos| !world.is_loaded(*pos)),
            "released ground was still held"
        );
    }

    #[test]
    fn overlapping_operations_cannot_unpin_each_other() {
        // Counted, not a flag. Two mines sharing a chunk, one finishing, must
        // not pull the ground out from under the other.
        let mut world = World::new(2024);
        let first = world.pin_span(BlockPos::new(0, 0, 0), BlockPos::new(15, 0, 15));
        let second = world.pin_span(BlockPos::new(8, 0, 8), BlockPos::new(20, 0, 20));
        let shared = ChunkPos::new(0, 0);
        assert!(world.is_pinned(shared));

        world.unpin_span(&first);
        assert!(world.is_pinned(shared), "the surviving operation lost its ground");

        world.unpin_span(&second);
        assert!(!world.is_pinned(shared));
    }

    #[test]
    fn an_unbalanced_release_is_ignored_rather_than_underflowing() {
        // Losing a pin is recoverable; a panic in the middle of a mine is not.
        let mut world = World::new(2024);
        let span = world.pin_span(BlockPos::new(0, 0, 0), BlockPos::new(1, 0, 1));
        world.unpin_span(&span);
        world.unpin_span(&span);
        assert_eq!(world.pinned_count(), 0);
    }
}
