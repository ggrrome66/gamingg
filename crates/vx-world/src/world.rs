//! World state: the set of loaded chunks and block access across them.
//!
//! `World` owns simulation state only. It never references the renderer or the
//! windowing layer, which is what keeps the client/server split available: the
//! same type can later run headless behind a socket without changes.

use std::collections::{HashMap, VecDeque};

use glam::Vec3;
use vx_core::{BlockId, BlockPos, BlockRegistry, ChunkPos, Face, CHUNK_HEIGHT, CHUNK_SIZE};

use crate::chunk::{BlockView, Chunk};
use crate::gen::{TerrainBlocks, TerrainGenerator};
use crate::inventory::{ItemStack, Recipe};
use crate::items::GameItems;
use crate::light::{Channel, LightQueue, MAX_LIGHT, RELIGHT_BUDGET};
use crate::raycast::{cast_ray, RayHit};
use crate::tick::{TickLimits, TickScheduler};

/// What one simulation step did, for the diagnostics readout.
///
/// Reported rather than logged because the interesting signal is a *rate*: a
/// steady stream of refusals means a limit is being hit continuously, which
/// looks very different from one busy step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Scheduled ticks executed.
    pub ticks_run: usize,
    /// Neighbour notifications processed.
    pub updates_processed: usize,
    /// Blocks that actually moved.
    pub blocks_moved: usize,
    /// Notifications discarded because the queue was full.
    pub updates_dropped: usize,
    /// Chunks whose lighting was recomputed.
    pub chunks_relit: usize,
}

/// Why a block edit was refused.
///
/// Edits are the command interface a client drives the world through, so a
/// refusal is a value to report rather than a panic: the same rejection has to
/// survive a round trip over a socket once this is multiplayer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// Nothing was in range of the ray.
    #[error("nothing within reach")]
    OutOfReach,
    /// Outside the world's height, or in a chunk that is not resident.
    #[error("position is outside the world or not loaded")]
    Unloaded,
    /// Tried to break empty space.
    #[error("there is no block there")]
    Empty,
    /// Bedrock, fluids, and anything else with no hardness.
    #[error("that block cannot be broken")]
    Unbreakable,
    /// Something solid is already there.
    #[error("that space is occupied")]
    Occupied,
    /// The selected slot is empty, or holds something with no block form.
    #[error("nothing to place")]
    NothingHeld,
}

/// Loaded chunks plus the generator that fills in missing ones.
pub struct World {
    registry: BlockRegistry,
    items: vx_core::ItemRegistry,
    game_items: GameItems,
    generator: TerrainGenerator,
    chunks: HashMap<ChunkPos, Chunk>,
    scheduler: TickScheduler,
    /// Positions to re-examine because a neighbour changed.
    updates: VecDeque<BlockPos>,
    updates_dropped: u64,
    /// Chunks whose lighting needs recomputing.
    relight_queue: std::collections::HashSet<ChunkPos>,
    /// Resource ceilings for simulation. A property of the world rather than
    /// an argument, so no call site can accidentally pass looser ones.
    limits: TickLimits,
}

impl World {
    /// A world with the engine's built-in blocks registered.
    pub fn new(seed: u64) -> Self {
        let mut registry = BlockRegistry::new();
        let blocks = TerrainBlocks::register_builtins(&mut registry);
        let mut items = vx_core::ItemRegistry::new();
        let game_items = GameItems::register_builtins(&mut items, &blocks);
        World {
            registry,
            items,
            game_items,
            generator: TerrainGenerator::new(seed, blocks),
            chunks: HashMap::new(),
            scheduler: TickScheduler::new(),
            updates: VecDeque::new(),
            updates_dropped: 0,
            relight_queue: std::collections::HashSet::new(),
            limits: TickLimits::default(),
        }
    }

    /// A world with non-default simulation ceilings. For tests that need to
    /// reach a limit without generating hundreds of thousands of blocks.
    pub fn with_limits(seed: u64, limits: TickLimits) -> Self {
        let mut world = World::new(seed);
        world.limits = limits;
        world
    }

    pub fn limits(&self) -> &TickLimits {
        &self.limits
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    pub fn items(&self) -> &vx_core::ItemRegistry {
        &self.items
    }

    pub fn game_items(&self) -> &GameItems {
        &self.game_items
    }

    /// What breaking `block` yields.
    pub fn drop_for(&self, block: vx_core::BlockId) -> Option<ItemStack> {
        self.game_items.drop_for(block, &self.generator.blocks())
    }

    /// The recipes this world offers.
    pub fn recipes(&self) -> Vec<Recipe> {
        self.game_items.recipes()
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

    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    /// Load `pos`, generating it if it is not already resident.
    pub fn load_chunk(&mut self, pos: ChunkPos) -> &Chunk {
        if !self.chunks.contains_key(&pos) {
            let chunk = self.generator.generate(pos);
            self.chunks.insert(pos, chunk);
            // Freshly generated blocks carry no lighting. Queue it here rather
            // than relying on every caller to remember, or terrain loaded by
            // any route but the streamer renders black.
            self.relight_queue.insert(pos);
        }
        self.chunks.get(&pos).expect("just inserted")
    }

    /// Load every chunk within `radius` of `centre`, returning how many were
    /// newly generated. Lighting is queued, not computed — call
    /// [`World::relight_chunk`] or run a tick before meshing.
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

    /// Take a chunk that came from somewhere other than the generator —
    /// off disk, or across a network once that exists.
    ///
    /// Replaces whatever was resident at that position.
    pub fn insert_chunk(&mut self, chunk: Chunk) {
        let pos = chunk.pos();
        self.chunks.insert(pos, chunk);
        self.relight_queue.insert(pos);
    }

    /// Remove chunks further than `radius` from `centre` and hand them back.
    ///
    /// Returned rather than dropped so the caller can persist anything
    /// modified first. Dropping them here is how edits used to disappear the
    /// moment you walked away from them.
    pub fn unload_beyond(&mut self, centre: ChunkPos, radius: i32) -> Vec<Chunk> {
        let limit = (radius as i64) * (radius as i64);
        let mut unloaded = Vec::new();
        self.chunks.retain(|pos, chunk| {
            if pos.distance_squared(centre) <= limit {
                return true;
            }
            // `retain` cannot move the value out, so swap a husk into its place
            // and take the real one. Cheaper than a second pass to collect the
            // keys and remove them.
            let taken = std::mem::replace(chunk, Chunk::empty(*pos));
            unloaded.push(taken);
            false
        });

        // Drop queued work for chunks that are leaving. Without this the queue
        // grows without bound as the player travels, holding ticks for terrain
        // that is no longer resident.
        for chunk in &unloaded {
            self.scheduler.forget_chunk(chunk.pos());
        }
        if !unloaded.is_empty() {
            let gone: std::collections::HashSet<ChunkPos> =
                unloaded.iter().map(|chunk| chunk.pos()).collect();
            self.updates.retain(|pos| !gone.contains(&pos.chunk()));
            self.relight_queue.retain(|pos| !gone.contains(pos));
        }

        unloaded
    }

    /// Every resident chunk holding unsaved changes.
    pub fn modified_chunks(&self) -> impl Iterator<Item = &Chunk> + '_ {
        self.chunks.values().filter(|chunk| chunk.is_modified())
    }

    /// Mark a resident chunk as written to disk.
    pub fn mark_saved(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.mark_saved();
        }
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
            // Anything touching this position may now behave differently —
            // sand above a block that was just mined, for instance. The
            // position itself is included, so placing sand in mid-air falls.
            self.notify(pos);
            for face in Face::ALL {
                self.notify(pos.neighbour(face));
            }

            // Light is derived from the blocks, so it is now stale. Deferred
            // to the tick rather than recomputed here: a chunk relight is far
            // too heavy to run inside every single block placement.
            self.relight_queue.insert(pos.chunk());
            for face in Face::ALL {
                let neighbour = pos.neighbour(face).chunk();
                if neighbour != pos.chunk() && self.chunks.contains_key(&neighbour) {
                    self.relight_queue.insert(neighbour);
                }
            }
        }
        Some(previous)
    }

    /// Packed light at a world position: sky in the high nibble, block in the
    /// low. Unloaded space reads as full daylight rather than darkness, so the
    /// edge of the loaded world is not ringed with black.
    pub fn light(&self, pos: BlockPos) -> u8 {
        let Some(local) = pos.local() else {
            // Above the world is open sky; below it is solid rock.
            return if pos.y >= CHUNK_HEIGHT { 0xf0 } else { 0 };
        };
        match self.chunks.get(&pos.chunk()) {
            Some(chunk) => chunk.light().packed(local.index()),
            None => 0xf0,
        }
    }

    /// Sky light only, 0 where nothing is loaded.
    fn sky_light(&self, pos: BlockPos) -> u8 {
        let Some(local) = pos.local() else { return 0 };
        self.chunks
            .get(&pos.chunk())
            .map_or(0, |chunk| chunk.light().sky(local.index()))
    }

    fn block_light(&self, pos: BlockPos) -> u8 {
        let Some(local) = pos.local() else { return 0 };
        self.chunks
            .get(&pos.chunk())
            .map_or(0, |chunk| chunk.light().block(local.index()))
    }

    /// Raise a light level, reporting whether it actually changed. Only ever
    /// raises: the flood fill takes the brightest contribution.
    fn raise_light(&mut self, pos: BlockPos, channel: Channel, level: u8) -> bool {
        let Some(local) = pos.local() else { return false };
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return false;
        };
        let index = local.index();
        let grid = chunk.light_mut();
        let current = match channel {
            Channel::Sky => grid.sky(index),
            Channel::Block => grid.block(index),
        };
        if current >= level {
            return false;
        }
        match channel {
            Channel::Sky => grid.set_sky(index, level),
            Channel::Block => grid.set_block(index, level),
        }
        true
    }

    /// Queue a chunk to have its lighting recomputed.
    pub fn request_relight(&mut self, pos: ChunkPos) {
        if self.is_loaded(pos) {
            self.relight_queue.insert(pos);
        }
    }

    pub fn pending_relights(&self) -> usize {
        self.relight_queue.len()
    }

    /// Recompute lighting for one chunk, spreading into loaded neighbours.
    ///
    /// Returns every chunk whose light changed, so the caller can remesh them.
    /// Light is a pure function of nearby blocks, so this can always be redone
    /// from scratch — which is why none of it is persisted.
    pub fn relight_chunk(&mut self, pos: ChunkPos) -> Vec<ChunkPos> {
        if !self.is_loaded(pos) {
            return Vec::new();
        }
        // This request is now satisfied however it was made.
        self.relight_queue.remove(&pos);

        let mut touched: std::collections::HashSet<ChunkPos> = std::collections::HashSet::new();
        touched.insert(pos);

        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.light_mut().clear();
        }

        let origin = pos.origin();
        let mut sky = LightQueue::new();
        let mut block = LightQueue::new();

        // Daylight falls straight down a column at full strength until
        // something opaque stops it. Everything below that starts dark and is
        // reached, if at all, by the flood fill.
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                for y in (0..CHUNK_HEIGHT).rev() {
                    let at = BlockPos::new(origin.x + local_x, y, origin.z + local_z);
                    let here = self.block(at);
                    if self.registry.is_opaque(here) {
                        break;
                    }
                    self.raise_light(at, Channel::Sky, MAX_LIGHT);
                }
            }
        }

        // Seed both fills. Only cells that actually border something darker
        // need to spread; seeding every lit cell would push tens of thousands
        // of entries that immediately do nothing.
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                for y in 0..CHUNK_HEIGHT {
                    let at = BlockPos::new(origin.x + local_x, y, origin.z + local_z);

                    let emitted = self.registry.emission(self.block(at));
                    if emitted > 0 {
                        self.raise_light(at, Channel::Block, emitted);
                        block.push(at, emitted);
                    }

                    let level = self.sky_light(at);
                    if level > 1 && self.borders_darkness(at, Channel::Sky, level) {
                        sky.push(at, level);
                    }
                    let level = self.block_light(at);
                    if level > 1 && self.borders_darkness(at, Channel::Block, level) {
                        block.push(at, level);
                    }
                }
            }
        }

        self.flood(&mut sky, Channel::Sky, &mut touched);
        self.flood(&mut block, Channel::Block, &mut touched);

        for chunk in &touched {
            if let Some(chunk) = self.chunks.get_mut(chunk) {
                chunk.mark_dirty();
            }
        }
        touched.into_iter().collect()
    }

    /// True when some neighbour is transparent and darker than `level - 1`,
    /// so this cell has somewhere to spread to.
    fn borders_darkness(&self, pos: BlockPos, channel: Channel, level: u8) -> bool {
        Face::ALL.iter().any(|face| {
            let next = pos.neighbour(*face);
            if self.registry.is_opaque(self.block(next)) {
                return false;
            }
            let there = match channel {
                Channel::Sky => self.sky_light(next),
                Channel::Block => self.block_light(next),
            };
            there + 1 < level
        })
    }

    /// Breadth-first spread until the frontier empties or the budget runs out.
    fn flood(
        &mut self,
        queue: &mut LightQueue,
        channel: Channel,
        touched: &mut std::collections::HashSet<ChunkPos>,
    ) {
        while let Some(pending) = queue.pop(RELIGHT_BUDGET) {
            if pending.level <= 1 {
                continue;
            }
            let spread = pending.level - 1;

            for face in Face::ALL {
                let next = pending.pos.neighbour(face);
                if !next.in_vertical_bounds() || !self.is_loaded(next.chunk()) {
                    continue;
                }
                // Opaque blocks stop light dead.
                if self.registry.is_opaque(self.block(next)) {
                    continue;
                }
                if self.raise_light(next, channel, spread) {
                    touched.insert(next.chunk());
                    queue.push(next, spread);
                }
            }
        }

        if queue.exhausted(RELIGHT_BUDGET) {
            // Lighting is left subtly wrong in a very large volume rather than
            // the frame stalling. Worth knowing about.
            log::warn!(
                "relight hit its {RELIGHT_BUDGET}-block budget with {} still queued",
                queue.len()
            );
        }
    }

    /// The tick queue, for diagnostics and for the app's readout.
    pub fn scheduler(&self) -> &TickScheduler {
        &self.scheduler
    }

    /// Notifications discarded because the queue was full. Non-zero means the
    /// world is generating updates faster than it can consume them.
    pub fn updates_dropped(&self) -> u64 {
        self.updates_dropped
    }

    pub fn pending_updates(&self) -> usize {
        self.updates.len()
    }

    /// Queue `pos` to be re-examined, bounded.
    ///
    /// Dropping the newest rather than growing is deliberate: a queue that
    /// grows to hold every consequence of a cascade is unbounded memory driven
    /// by world content. Anything lost here is re-derived the next time
    /// something near it changes.
    fn notify(&mut self, pos: BlockPos) {
        if self.updates.len() >= self.limits.max_pending_updates {
            self.updates_dropped += 1;
            return;
        }
        self.updates.push_back(pos);
    }

    /// Run one simulation step.
    ///
    /// Every phase is bounded by `limits`. Work over budget stays queued for
    /// the next step, so a busy world runs behind rather than stalling the
    /// frame it is running on.
    pub fn tick(&mut self) -> TickReport {
        let limits = self.limits;
        let mut report = TickReport::default();

        // One chunk per step. Relighting is the most expensive thing the tick
        // does, and spreading it out costs nothing but a frame of latency on
        // a shadow moving.
        if let Some(pos) = self.relight_queue.iter().next().copied() {
            self.relight_queue.remove(&pos);
            report.chunks_relit = self.relight_chunk(pos).len();
        }

        // Notifications first: a block that just lost its support should be
        // queued before this step's ticks run, not after.
        let budget = limits.max_updates_per_step.min(self.updates.len());
        for _ in 0..budget {
            let Some(pos) = self.updates.pop_front() else {
                break;
            };
            report.updates_processed += 1;

            if self.registry.has_gravity(self.block(pos)) && self.can_fall_into(pos) {
                // Refusals are expected here — already queued is the common
                // case when several neighbours change at once.
                let _ = self.scheduler.schedule(pos, 1, &limits);
            }
        }

        for pos in self.scheduler.advance(limits.max_per_step) {
            report.ticks_run += 1;
            if self.fall(pos) {
                report.blocks_moved += 1;
            }
        }

        report.updates_dropped = self.updates_dropped as usize;
        report
    }

    /// True when the space under `pos` would accept a falling block.
    ///
    /// An unloaded chunk below counts as blocked. Letting a block fall into
    /// space that is not resident would either destroy it or write into a
    /// chunk that is about to be generated over.
    fn can_fall_into(&self, pos: BlockPos) -> bool {
        let below = pos.offset([0, -1, 0]);
        below.in_vertical_bounds() && self.is_loaded(below.chunk()) && self.is_replaceable(below)
    }

    /// Move a falling block down one, and queue the next step of its fall.
    fn fall(&mut self, pos: BlockPos) -> bool {
        let block = self.block(pos);
        if !self.registry.has_gravity(block) || !self.can_fall_into(pos) {
            // Conditions changed between scheduling and running — something
            // filled the gap, or the block was mined.
            return false;
        }

        let below = pos.offset([0, -1, 0]);
        self.set_block(pos, BlockId::AIR);
        self.set_block(below, block);

        // Keep falling. The notification raised by the writes above would also
        // catch this, but scheduling directly keeps a fall at one block per
        // step regardless of how busy the update queue is.
        let limits = self.limits;
        let _ = self.scheduler.schedule(below, 1, &limits);
        true
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

    /// True when a block can be built over. Air and fluids yield; anything
    /// solid has to be broken first.
    pub fn is_replaceable(&self, pos: BlockPos) -> bool {
        !self.is_solid(pos)
    }

    /// The first solid block along a ray, within `reach` blocks.
    ///
    /// Fluids are passed through, matching [`World::is_replaceable`]: you
    /// target what you could stand on, not the water in front of it.
    pub fn raycast_solid(&self, origin: Vec3, direction: Vec3, reach: f32) -> Option<RayHit> {
        cast_ray(origin, direction, reach, |pos| self.is_solid(pos))
    }

    /// Remove the block at `pos`, returning what was removed.
    pub fn break_block(&mut self, pos: BlockPos) -> Result<BlockId, EditError> {
        if !pos.in_vertical_bounds() || !self.is_loaded(pos.chunk()) {
            return Err(EditError::Unloaded);
        }
        let existing = self.block(pos);
        if existing.is_air() {
            return Err(EditError::Empty);
        }
        if !self.registry.is_breakable(existing) {
            return Err(EditError::Unbreakable);
        }

        self.set_block(pos, BlockId::AIR)
            .ok_or(EditError::Unloaded)?;
        Ok(existing)
    }

    /// Put `block` at `pos`, if the space is free.
    pub fn place_block(&mut self, pos: BlockPos, block: BlockId) -> Result<(), EditError> {
        if !pos.in_vertical_bounds() || !self.is_loaded(pos.chunk()) {
            return Err(EditError::Unloaded);
        }
        if !self.is_replaceable(pos) {
            return Err(EditError::Occupied);
        }

        self.set_block(pos, block).ok_or(EditError::Unloaded)?;
        Ok(())
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

    fn light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        self.light(BlockPos::new(x, y, z))
    }
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

        let dropped = world.unload_beyond(ChunkPos::new(0, 0), 1).len();

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
    fn breaking_a_block_clears_it_and_reports_what_was_there() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4);
        world.set_block(pos, stone);

        assert_eq!(world.break_block(pos), Ok(stone));
        assert!(world.block(pos).is_air());
    }

    #[test]
    fn breaking_empty_space_is_refused() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        assert_eq!(
            world.break_block(BlockPos::new(4, 200, 4)),
            Err(EditError::Empty)
        );
    }

    #[test]
    fn bedrock_and_water_cannot_be_broken() {
        // Both carry `hardness: None`. Without this check the player can dig
        // through the world floor.
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let bedrock = world.registry().id_of("engine:bedrock").unwrap();
        let water = world.registry().id_of("engine:water").unwrap();

        for (block, pos) in [
            (bedrock, BlockPos::new(4, 200, 4)),
            (water, BlockPos::new(5, 200, 4)),
        ] {
            world.set_block(pos, block);
            assert_eq!(world.break_block(pos), Err(EditError::Unbreakable));
            assert_eq!(world.block(pos), block, "the block was removed anyway");
        }
    }

    #[test]
    fn editing_outside_loaded_space_is_refused() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();

        for pos in [
            BlockPos::new(1000, 64, 1000),      // chunk not resident
            BlockPos::new(0, CHUNK_HEIGHT, 0),  // above the world
            BlockPos::new(0, -1, 0),            // below it
        ] {
            assert_eq!(world.break_block(pos), Err(EditError::Unloaded), "{pos:?}");
            assert_eq!(
                world.place_block(pos, stone),
                Err(EditError::Unloaded),
                "{pos:?}"
            );
        }
    }

    #[test]
    fn placing_fills_empty_space() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4);

        assert_eq!(world.place_block(pos, stone), Ok(()));
        assert_eq!(world.block(pos), stone);
    }

    #[test]
    fn placing_into_a_solid_block_is_refused() {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();
        let dirt = world.registry().id_of("engine:dirt").unwrap();
        let pos = BlockPos::new(4, 200, 4);
        world.set_block(pos, stone);

        assert_eq!(world.place_block(pos, dirt), Err(EditError::Occupied));
        assert_eq!(world.block(pos), stone, "the existing block was overwritten");
    }

    #[test]
    fn water_is_replaceable_but_stone_is_not() {
        // Building into a lake should work; building into a hillside should
        // not.
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let water = world.registry().id_of("engine:water").unwrap();
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4);

        world.set_block(pos, water);
        assert!(world.is_replaceable(pos));
        assert_eq!(world.place_block(pos, stone), Ok(()));
        assert_eq!(world.block(pos), stone);
        assert!(!world.is_replaceable(pos));
    }

    #[test]
    fn an_edit_dirties_the_chunk_so_the_mesh_is_rebuilt() {
        // Without this the edit is invisible until something else forces a
        // remesh of that chunk.
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4);

        world.clear_dirty(ChunkPos::new(0, 0));
        world.place_block(pos, stone).unwrap();
        assert!(world.dirty_chunks().any(|p| p == ChunkPos::new(0, 0)));

        world.clear_dirty(ChunkPos::new(0, 0));
        world.break_block(pos).unwrap();
        assert!(world.dirty_chunks().any(|p| p == ChunkPos::new(0, 0)));
    }

    #[test]
    fn a_raycast_finds_the_terrain_surface_below_the_camera() {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 1);
        let surface = world.surface_y(8, 8).unwrap();

        let eye = Vec3::new(8.5, surface as f32 + 5.0, 8.5);
        let hit = world
            .raycast_solid(eye, Vec3::NEG_Y, 20.0)
            .expect("looking straight down at terrain should hit something");

        // The first solid block under the camera is the surface itself.
        assert_eq!(hit.block, BlockPos::new(8, surface - 1, 8));
        assert_eq!(hit.face, Some(Face::PosY), "hit the underside of the ground");
        // And the place to build is the empty block on top of it.
        assert_eq!(hit.placement(), Some(BlockPos::new(8, surface, 8)));
    }

    #[test]
    fn a_raycast_into_the_sky_finds_nothing() {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 1);
        let surface = world.surface_y(8, 8).unwrap();

        let eye = Vec3::new(8.5, surface as f32 + 5.0, 8.5);
        assert!(world.raycast_solid(eye, Vec3::Y, 20.0).is_none());
    }

    #[test]
    fn breaking_what_a_ray_found_always_succeeds() {
        // The two halves of the interaction have to agree: whatever the ray
        // targets must be a legal thing to edit, barring unbreakables.
        let mut world = World::new(99);
        world.load_around(ChunkPos::new(0, 0), 1);
        let surface = world.surface_y(8, 8).unwrap();
        let eye = Vec3::new(8.5, surface as f32 + 3.0, 8.5);

        let hit = world.raycast_solid(eye, Vec3::NEG_Y, 10.0).unwrap();
        let target = hit.block;
        assert!(world.break_block(target).is_ok());
        assert!(world.block(target).is_air());

        // And the space it left is now placeable.
        let stone = world.registry().id_of("engine:stone").unwrap();
        assert_eq!(world.place_block(target, stone), Ok(()));
    }

    /// A world with one loaded chunk and nothing in the way.
    fn sandbox() -> World {
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        // Loading queues a relight; settle it so tests start from a world
        // with nothing outstanding.
        world.relight_chunk(ChunkPos::new(0, 0));
        world
    }

    fn sand(world: &World) -> BlockId {
        world.registry().id_of("engine:sand").unwrap()
    }

    #[test]
    fn a_tick_with_nothing_to_do_reports_nothing() {
        let mut world = sandbox();
        assert_eq!(world.tick(), TickReport::default());
    }

    #[test]
    fn unsupported_sand_falls_one_block_per_tick() {
        let mut world = sandbox();
        let held = sand(&world);
        let stone = world.registry().id_of("engine:stone").unwrap();

        // A floor two below, so the sand falls exactly one block and stops.
        let floor = BlockPos::new(4, 200, 4);
        let start = floor.offset([0, 2, 0]);
        world.place_block(floor, stone).unwrap();
        world.place_block(start, held).unwrap();

        let report = world.tick();
        assert_eq!(report.blocks_moved, 1, "the sand did not move on the first step");
        assert!(world.block(start).is_air(), "the sand did not leave");
        assert_eq!(world.block(start.offset([0, -1, 0])), held);

        // One block per step, and it settles on the floor rather than through it.
        for _ in 0..8 {
            world.tick();
        }
        assert_eq!(world.block(floor.offset([0, 1, 0])), held);
        assert_eq!(world.block(floor), stone, "the sand fell through the floor");
    }

    #[test]
    fn falling_sand_comes_to_rest_on_solid_ground() {
        let mut world = sandbox();
        let surface = world.surface_y(4, 4).unwrap();
        let start = BlockPos::new(4, surface + 6, 4);
        world.place_block(start, sand(&world)).unwrap();

        for _ in 0..64 {
            world.tick();
        }

        assert_eq!(
            world.block(BlockPos::new(4, surface, 4)),
            sand(&world),
            "sand did not settle on the surface"
        );
        // And it stops: no further movement once it has landed.
        assert_eq!(world.tick().blocks_moved, 0);
    }

    #[test]
    fn blocks_without_gravity_stay_where_they_are_put() {
        let mut world = sandbox();
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pos = BlockPos::new(4, 200, 4);
        world.place_block(pos, stone).unwrap();

        for _ in 0..10 {
            world.tick();
        }
        assert_eq!(world.block(pos), stone, "stone fell");
    }

    #[test]
    fn mining_a_support_makes_what_it_held_up_fall() {
        // The notification path: nothing touches the sand directly, so it only
        // falls if the edit below it raised an update.
        let mut world = sandbox();
        let support = BlockPos::new(4, 200, 4);
        let above = support.offset([0, 1, 0]);
        let stone = world.registry().id_of("engine:stone").unwrap();

        world.place_block(support, stone).unwrap();
        world.place_block(above, sand(&world)).unwrap();
        for _ in 0..4 {
            world.tick();
        }
        assert_eq!(world.block(above), sand(&world), "sand fell while supported");

        world.break_block(support).unwrap();
        for _ in 0..4 {
            world.tick();
        }

        assert!(world.block(above).is_air(), "sand ignored losing its support");
    }

    #[test]
    fn a_column_of_sand_is_neither_duplicated_nor_lost() {
        // Block duplication is the classic voxel exploit, and a cascade that
        // moves several blocks in one step is exactly where it would appear.
        let mut world = sandbox();
        let held = sand(&world);
        let column = 12;
        for offset in 0..column {
            world
                .place_block(BlockPos::new(4, 200 + offset, 4), held)
                .unwrap();
        }

        let ground = world.surface_y(4, 4).unwrap();
        let count_sand = |world: &World| {
            (0..256)
                .filter(|y| world.block(BlockPos::new(4, *y, 4)) == held)
                .count()
        };
        assert_eq!(count_sand(&world), column as usize);

        for _ in 0..512 {
            world.tick();
        }

        assert_eq!(
            count_sand(&world),
            column as usize,
            "the cascade changed how much sand exists"
        );
        // And it settles into one contiguous column standing on something
        // solid. The resting height is not predicted: sand sinks through
        // water, so where it stops depends on whether this column is flooded.
        let settled: Vec<i32> = (0..256)
            .filter(|y| world.block(BlockPos::new(4, *y, 4)) == held)
            .collect();
        assert_eq!(settled.len(), column as usize);
        assert!(
            settled.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "the settled sand has gaps in it: {settled:?}"
        );
        assert!(
            world.is_solid(BlockPos::new(4, settled[0] - 1, 4)),
            "the column is resting on nothing"
        );
        assert!(
            settled[0] <= ground,
            "sand settled above where it started falling from"
        );
    }

    #[test]
    fn sand_displaces_water_rather_than_stopping_on_it() {
        let mut world = sandbox();
        let water = world.registry().id_of("engine:water").unwrap();
        let held = sand(&world);
        let pool = BlockPos::new(6, 200, 6);
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Water sitting on a bed, or the sand sinks straight through it.
        world.place_block(pool.offset([0, -1, 0]), stone).unwrap();
        world.set_block(pool, water);
        world.place_block(pool.offset([0, 2, 0]), held).unwrap();

        for _ in 0..16 {
            world.tick();
        }

        assert_eq!(world.block(pool), held, "sand did not sink into the water");
    }

    #[test]
    fn sand_will_not_fall_into_an_unloaded_chunk() {
        // Falling into space that is not resident would either destroy the
        // block or write into a chunk that is about to be generated over.
        let mut world = World::new(7);
        world.load_chunk(ChunkPos::new(0, 0));
        let held = sand(&world);

        // Sitting on the very edge, with the chunk to the west absent.
        let edge = BlockPos::new(0, 200, 0);
        world.place_block(edge, held).unwrap();
        assert!(!world.is_loaded(ChunkPos::new(-1, 0)));

        for _ in 0..8 {
            world.tick();
        }
        // It falls within its own column, which is loaded, and never leaves.
        assert!(world.block(edge).is_air() || world.block(edge) == held);
    }

    #[test]
    fn unloading_a_chunk_forgets_the_work_queued_in_it() {
        // Otherwise travelling grows the queue without bound.
        let mut world = World::new(7);
        world.load_around(ChunkPos::new(0, 0), 1);
        let held = sand(&world);
        world.place_block(BlockPos::new(20, 200, 4), held).unwrap();
        world.tick();

        assert!(world.scheduler().pending() > 0, "nothing was queued");

        world.unload_beyond(ChunkPos::new(0, 0), 0);

        assert_eq!(world.scheduler().pending(), 0, "queued work outlived its chunk");
        assert_eq!(world.pending_updates(), 0);
    }

    #[test]
    fn a_step_never_exceeds_its_tick_budget() {
        // A wide field of unsupported sand is the amplification case: one edit
        // per block, all falling at once. The step must run behind rather than
        // doing unbounded work in the frame it was called from.
        let limits = TickLimits {
            max_per_step: 8,
            ..TickLimits::default()
        };
        let mut world = World::with_limits(7, limits);
        world.load_chunk(ChunkPos::new(0, 0));
        let held = sand(&world);

        for x in 0..16 {
            for z in 0..16 {
                world.place_block(BlockPos::new(x, 200, z), held).unwrap();
            }
        }

        for _ in 0..32 {
            let report = world.tick();
            assert!(
                report.ticks_run <= limits.max_per_step,
                "ran {} ticks against a budget of {}",
                report.ticks_run,
                limits.max_per_step
            );
        }
    }

    #[test]
    fn the_update_queue_refuses_to_grow_without_bound() {
        // World content drives this queue, so an unbounded one is memory
        // exhaustion driven by whatever is in the world.
        let limits = TickLimits {
            max_pending_updates: 32,
            max_updates_per_step: 1,
            ..TickLimits::default()
        };
        let mut world = World::with_limits(7, limits);
        world.load_chunk(ChunkPos::new(0, 0));
        let held = sand(&world);

        for x in 0..16 {
            for z in 0..16 {
                let _ = world.place_block(BlockPos::new(x, 210, z), held);
            }
        }

        assert!(
            world.pending_updates() <= limits.max_pending_updates,
            "queue reached {} against a ceiling of {}",
            world.pending_updates(),
            limits.max_pending_updates
        );
        assert!(world.updates_dropped() > 0, "nothing was refused");
    }

    #[test]
    fn a_full_tick_queue_does_not_stall_the_simulation() {
        // Reaching the ceiling must degrade the world, not wedge it: whatever
        // is queued still runs, and refusals are counted rather than hidden.
        let limits = TickLimits {
            max_pending: 4,
            ..TickLimits::default()
        };
        let mut world = World::with_limits(7, limits);
        world.load_chunk(ChunkPos::new(0, 0));
        let held = sand(&world);

        for x in 0..16 {
            for z in 0..16 {
                let _ = world.place_block(BlockPos::new(x, 200, z), held);
            }
        }

        let mut moved = 0;
        for _ in 0..64 {
            moved += world.tick().blocks_moved;
        }

        assert!(moved > 0, "the simulation stopped making progress");
        assert!(world.scheduler().pending() <= limits.max_pending);
    }

    /// Hollow out a sealed room and report its interior bounds.
    fn carve_room(world: &mut World, centre: BlockPos, half: i32) {
        for x in -half..=half {
            for y in -half..=half {
                for z in -half..=half {
                    world.set_block(centre.offset([x, y, z]), BlockId::AIR);
                }
            }
        }
    }

    #[test]
    fn open_sky_is_fully_lit_and_solid_ground_is_dark() {
        let mut world = sandbox();
        world.relight_chunk(ChunkPos::new(0, 0));

        let surface = world.surface_y(8, 8).unwrap();
        let sky = BlockPos::new(8, surface + 20, 8);
        assert_eq!(world.light(sky) >> 4, MAX_LIGHT, "open sky is not lit");

        // Well inside the rock, with daylight blocked by everything above.
        let buried = BlockPos::new(8, surface - 20, 8);
        assert_eq!(world.light(buried), 0, "light reached solid ground");
    }

    #[test]
    fn a_sealed_room_underground_stays_dark() {
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        let centre = BlockPos::new(8, surface - 12, 8);

        carve_room(&mut world, centre, 2);
        world.relight_chunk(ChunkPos::new(0, 0));

        assert_eq!(
            world.light(centre),
            0,
            "a sealed room has no light source and no way in"
        );
    }

    #[test]
    fn a_lamp_lights_its_room_and_falls_off_with_distance() {
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        let centre = BlockPos::new(8, surface - 12, 8);
        carve_room(&mut world, centre, 3);

        let lamp = world.registry().id_of("engine:lamp").unwrap();
        let emission = world.registry().emission(lamp);
        world.set_block(centre, lamp);
        world.relight_chunk(ChunkPos::new(0, 0));

        // Dimmer the further out you go, and never brighter than the source.
        let near = world.light(centre.offset([1, 0, 0])) & 0x0f;
        let far = world.light(centre.offset([3, 0, 0])) & 0x0f;

        assert!(near > 0, "the lamp lit nothing");
        assert!(near <= emission, "light exceeded what the lamp emits");
        assert!(far < near, "light did not fall off: {near} then {far}");
    }

    #[test]
    fn light_does_not_leak_through_solid_rock() {
        // The wall of a lit room must leave the rock behind it dark, or every
        // cave would glow through its own ceiling.
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        let centre = BlockPos::new(8, surface - 12, 8);
        carve_room(&mut world, centre, 2);

        let lamp = world.registry().id_of("engine:lamp").unwrap();
        world.set_block(centre, lamp);
        world.relight_chunk(ChunkPos::new(0, 0));

        // Four blocks out is past the room wall and into solid stone.
        let through_the_wall = centre.offset([4, 0, 0]);
        assert!(world.is_solid(through_the_wall));
        assert_eq!(
            world.light(through_the_wall) & 0x0f,
            0,
            "block light passed through rock"
        );
    }

    #[test]
    fn opening_a_roof_lets_daylight_down_the_shaft() {
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        world.relight_chunk(ChunkPos::new(0, 0));

        let below = BlockPos::new(8, surface - 3, 8);
        assert_eq!(world.light(below) >> 4, 0, "already lit before digging");

        // Dig a shaft straight down from the surface.
        for y in (surface - 3)..=surface {
            world.set_block(BlockPos::new(8, y, 8), BlockId::AIR);
        }
        world.relight_chunk(ChunkPos::new(0, 0));

        assert_eq!(
            world.light(below) >> 4,
            MAX_LIGHT,
            "daylight did not reach the bottom of an open shaft"
        );
    }

    #[test]
    fn an_edit_queues_a_relight_that_the_tick_carries_out() {
        let mut world = sandbox();
        world.relight_chunk(ChunkPos::new(0, 0));
        assert_eq!(world.pending_relights(), 0);

        let surface = world.surface_y(8, 8).unwrap();
        world.break_block(BlockPos::new(8, surface - 1, 8)).unwrap();

        assert!(world.pending_relights() > 0, "the edit did not stale the light");

        let report = world.tick();
        assert_eq!(report.chunks_relit, 1);
        assert_eq!(world.pending_relights(), 0);
    }

    #[test]
    fn relighting_is_reproducible() {
        // Light is derived state, which is the whole reason it is not saved.
        // Recomputing must give the same answer every time or that breaks.
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        let centre = BlockPos::new(8, surface - 10, 8);
        carve_room(&mut world, centre, 3);
        world.set_block(centre, world.registry().id_of("engine:lamp").unwrap());

        world.relight_chunk(ChunkPos::new(0, 0));
        let first: Vec<u8> = (0..40)
            .map(|offset| world.light(centre.offset([0, 0, 0]).offset([offset % 5, 0, offset / 5])))
            .collect();

        world.relight_chunk(ChunkPos::new(0, 0));
        let second: Vec<u8> = (0..40)
            .map(|offset| world.light(centre.offset([0, 0, 0]).offset([offset % 5, 0, offset / 5])))
            .collect();

        assert_eq!(first, second, "relighting the same world gave a different answer");
    }

    #[test]
    fn spreading_light_never_brightens_it() {
        // A flood fill that raised a level anywhere would feed itself and
        // light the whole world from one lamp.
        let mut world = sandbox();
        let surface = world.surface_y(8, 8).unwrap();
        let centre = BlockPos::new(8, surface - 10, 8);
        carve_room(&mut world, centre, 3);

        let lamp = world.registry().id_of("engine:lamp").unwrap();
        let emission = world.registry().emission(lamp);
        world.set_block(centre, lamp);
        world.relight_chunk(ChunkPos::new(0, 0));

        for x in -3..=3 {
            for y in -3..=3 {
                for z in -3..=3 {
                    let packed = world.light(centre.offset([x, y, z]));
                    assert!(
                        packed & 0x0f <= emission,
                        "block light {} exceeds the lamp's {emission}",
                        packed & 0x0f
                    );
                    assert!(packed >> 4 <= MAX_LIGHT);
                }
            }
        }
    }

    #[test]
    fn unloading_a_chunk_drops_its_queued_relight() {
        let mut world = World::new(7);
        world.load_around(ChunkPos::new(0, 0), 1);
        // Settle everything loading queued, so what remains is deliberate.
        while let Some(pos) = world.relight_queue.iter().next().copied() {
            world.relight_chunk(pos);
        }

        let leaving = ChunkPos::new(1, 0);
        world.request_relight(leaving);
        assert_eq!(world.pending_relights(), 1);

        world.unload_beyond(ChunkPos::new(0, 0), 0);
        assert_eq!(world.pending_relights(), 0, "relight outlived its chunk");
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
}
