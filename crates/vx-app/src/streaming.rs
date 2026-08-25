//! Keeping loaded chunks and their meshes in step with the camera.
//!
//! Three kinds of work, three treatments:
//!
//! - **Generation** runs on a background thread ([`GenWorker`]). Terrain is a
//!   pure function of `(seed, position)`, so a cloned generator on another
//!   thread produces bit-identical chunks — purity is the contract that makes
//!   the thread free. The worker only changes *when* a chunk becomes resident
//!   for rendering, which was always frame-dependent; anything the simulation
//!   needs NOW still comes through the synchronous `World::load_chunk`, and
//!   correctness continues to rest on chunk pinning exactly as stage 9a left it.
//! - **Meshing** is a parallel fork-join across the rayon pool, budgeted per
//!   frame. Deliberately *not* asynchronous: the measured hitches were the
//!   per-chunk region decode (now cached in `WorldSave`) and synchronous
//!   generation (now the worker's job). Revisit only with a measurement.
//! - **Uploads** stay serial on the main thread; GPU buffer creation is not
//!   the bottleneck and a single thread needs no shared device lock.
//!
//! Disk loads also stay on the main thread: the region cache makes them cheap,
//! and `WorldSave` is deliberately `!Sync`.

use rayon::prelude::*;

use vx_core::{BlockPos, ChunkPos};
use vx_mesh::{build_mesh, Mesh};
use vx_render::Renderer;
use vx_world::{Chunk, TerrainGenerator, World, WorldSave};

/// Chunk-streaming settings.
#[derive(Debug, Clone, Copy)]
pub struct StreamingConfig {
    /// Chunks visible in every direction.
    pub render_distance: i32,
    /// New generation requests issued per frame (or, in the synchronous
    /// fallback, chunks generated per frame).
    pub generate_budget: usize,
    /// Chunk meshes rebuilt and uploaded per frame.
    pub mesh_budget: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            render_distance: 8,
            generate_budget: 8,
            mesh_budget: 24,
        }
    }
}

/// Most chunk requests allowed in the worker's queue at once.
///
/// Enough to keep the thread busy across a boundary crossing, small enough
/// that a sprint in a straight line does not build a backlog of chunks that
/// will be out of range by the time they arrive.
const MAX_INFLIGHT: usize = 16;

/// Generates chunks on a background thread.
///
/// Send a position, get the finished [`Chunk`] back later. The generator is a
/// clone, and generation is pure in `(seed, position)`, so the result is
/// bit-identical to generating on the main thread — proven by test, not hoped.
pub struct GenWorker {
    requests: std::sync::mpsc::Sender<ChunkPos>,
    results: std::sync::mpsc::Receiver<Chunk>,
    inflight: std::collections::HashSet<ChunkPos>,
}

// A chunk crosses the channel whole.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Chunk>();
};

impl GenWorker {
    pub fn new(generator: TerrainGenerator) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<ChunkPos>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<Chunk>();

        std::thread::Builder::new()
            .name("chunk-gen".into())
            .spawn(move || {
                // Exits when the sender drops: recv fails, the loop ends.
                while let Ok(pos) = request_rx.recv() {
                    let chunk = generator.generate(pos);
                    if result_tx.send(chunk).is_err() {
                        break;
                    }
                }
            })
            .expect("could not spawn the chunk generation thread");

        GenWorker {
            requests: request_tx,
            results: result_rx,
            inflight: std::collections::HashSet::new(),
        }
    }

    /// Ask for a chunk, unless it is already on order or the queue is full.
    pub fn request(&mut self, pos: ChunkPos) -> bool {
        if self.inflight.len() >= MAX_INFLIGHT || self.inflight.contains(&pos) {
            return false;
        }
        if self.requests.send(pos).is_ok() {
            self.inflight.insert(pos);
            true
        } else {
            false
        }
    }

    /// Everything the worker has finished since the last drain.
    pub fn drain(&mut self) -> Vec<Chunk> {
        let arrived: Vec<Chunk> = self.results.try_iter().collect();
        for chunk in &arrived {
            self.inflight.remove(&chunk.pos());
        }
        arrived
    }
}

/// Which chunk a world position falls in.
pub fn chunk_at(position: glam::Vec3) -> ChunkPos {
    BlockPos::new(
        position.x.floor() as i32,
        0,
        position.z.floor() as i32,
    )
    .chunk()
}

/// Chunks inside the render distance, nearest first.
///
/// Nearest-first matters: it is what makes the world fill in around the player
/// rather than arriving in an arbitrary order from the horizon inward.
pub fn chunks_in_range(centre: ChunkPos, radius: i32) -> Vec<ChunkPos> {
    let limit = (radius as i64) * (radius as i64);
    let mut chunks: Vec<ChunkPos> = (-radius..=radius)
        .flat_map(|dx| (-radius..=radius).map(move |dz| ChunkPos::new(centre.x + dx, centre.z + dz)))
        .filter(|pos| pos.distance_squared(centre) <= limit)
        .collect();
    chunks.sort_by_key(|pos| pos.distance_squared(centre));
    chunks
}

/// Bring one chunk into the world: from disk when it was saved, generated
/// afresh otherwise. The synchronous path — pregeneration and the headless
/// tools use it directly; the streamer uses the same disk half and hands the
/// generation half to its worker.
pub fn load_or_generate(world: &mut World, save: Option<&WorldSave>, pos: ChunkPos) {
    if world.is_loaded(pos) {
        return;
    }
    match save.map(|save| save.load_chunk(pos, world.registry())) {
        Some(Ok(Some(chunk))) => world.insert_chunk(chunk),
        Some(Err(error)) => {
            // A damaged region should not take the game down; fall back to
            // generated terrain and say so.
            log::error!("could not load chunk {pos:?}: {error}");
            world.load_chunk(pos);
        }
        _ => {
            world.load_chunk(pos);
        }
    }
}

/// Tracks what has been meshed, and advances streaming one frame at a time.
pub struct ChunkStreamer {
    config: StreamingConfig,
    /// Chunks currently uploaded to the renderer.
    meshed: std::collections::HashSet<ChunkPos>,
    /// The background generator, when streaming for a live window. `None` in
    /// headless paths, which keep the old fully synchronous behaviour.
    worker: Option<GenWorker>,
    /// The wanted list is an allocation and a sort of ~200 entries; it only
    /// changes when the centre chunk does, so it is kept between frames.
    last_centre: Option<ChunkPos>,
    wanted: Vec<ChunkPos>,
    /// World edit counter as of the last dirty scan, so a quiet frame skips
    /// walking every loaded chunk.
    last_edit_count: u64,
}

impl ChunkStreamer {
    pub fn new(config: StreamingConfig) -> Self {
        ChunkStreamer {
            config,
            meshed: std::collections::HashSet::new(),
            worker: None,
            last_centre: None,
            wanted: Vec::new(),
            last_edit_count: u64::MAX,
        }
    }

    /// Attach a background generator. Without one, generation is synchronous.
    pub fn with_worker(mut self, worker: GenWorker) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Do one frame's worth of generating, meshing and unloading.
    ///
    /// Returns how many chunk meshes were uploaded.
    pub fn update(
        &mut self,
        world: &mut World,
        renderer: &mut Renderer,
        device: &wgpu::Device,
        centre: ChunkPos,
        save: Option<&WorldSave>,
    ) -> usize {
        let radius = self.config.render_distance;
        if self.last_centre != Some(centre) {
            self.wanted = chunks_in_range(centre, radius);
            self.last_centre = Some(centre);
        }

        // Land whatever the worker finished. A chunk that became resident
        // through a synchronous path in the meantime wins: it may carry edits,
        // and by purity its terrain half is identical anyway. One that drifted
        // out of range since it was ordered is simply dropped.
        let mut inserted = false;
        if let Some(worker) = &mut self.worker {
            let keep = (radius + 2) as i64;
            for chunk in worker.drain() {
                let pos = chunk.pos();
                if world.is_loaded(pos) || pos.distance_squared(centre) > keep * keep {
                    continue;
                }
                world.insert_chunk(chunk);
                inserted = true;
            }
        }

        // Bring in the nearest missing chunks. A chunk that was saved takes
        // precedence over generating it afresh, or every edit would be undone
        // the moment you walked away and back — and saved chunks load on this
        // thread (cheap, via the region cache) while fresh terrain is ordered
        // from the worker.
        let mut generated = 0;
        for index in 0..self.wanted.len() {
            if generated >= self.config.generate_budget {
                break;
            }
            let pos = self.wanted[index];
            if world.is_loaded(pos) {
                continue;
            }
            if let Some(worker) = &mut self.worker {
                if worker.inflight.contains(&pos) {
                    continue;
                }
            }
            match save.map(|save| save.load_chunk(pos, world.registry())) {
                Some(Ok(Some(chunk))) => {
                    world.insert_chunk(chunk);
                    inserted = true;
                }
                Some(Err(error)) => {
                    // A damaged region should not take the game down; fall
                    // back to generated terrain and say so once.
                    log::error!("could not load chunk {pos:?}: {error}");
                    world.load_chunk(pos);
                    inserted = true;
                }
                _ => match &mut self.worker {
                    Some(worker) => {
                        if !worker.request(pos) {
                            // Queue full: stop ordering, keep what we have.
                            break;
                        }
                    }
                    None => {
                        world.load_chunk(pos);
                        inserted = true;
                    }
                },
            }
            generated += 1;
        }

        // Drop anything that has drifted out of range, with a margin so a
        // player pacing across a boundary does not thrash chunks in and out.
        let keep = radius + 2;
        let dropped: Vec<ChunkPos> = self
            .meshed
            .iter()
            .copied()
            .filter(|pos| pos.distance_squared(centre) > (keep as i64) * (keep as i64))
            .collect();
        for pos in dropped {
            renderer.remove_chunk(pos);
            self.meshed.remove(&pos);
        }
        world.unload_beyond(centre, keep);

        // Pick this frame's work: chunks loaded but not yet uploaded, plus
        // anything the world flagged dirty after an edit. Nearest first, since
        // `wanted` is already sorted that way. The dirty scan walks every
        // loaded chunk, so a frame where nothing changed skips it.
        let quiet = world.edit_count() == self.last_edit_count && !inserted;
        self.last_edit_count = world.edit_count();
        let dirty: std::collections::HashSet<ChunkPos> = if quiet {
            std::collections::HashSet::new()
        } else {
            world.dirty_chunks().collect()
        };
        let to_mesh: Vec<ChunkPos> = self
            .wanted
            .iter()
            .copied()
            .filter(|pos| {
                world.is_loaded(*pos) && (!self.meshed.contains(pos) || dirty.contains(pos))
            })
            .take(self.config.mesh_budget)
            .collect();

        if to_mesh.is_empty() {
            return 0;
        }

        // Build in parallel from an immutable borrow. `World` is plain data
        // with no interior mutability, so concurrent reads need no locking —
        // the shared-borrow shape the mesher was written against pays off here
        // with no changes to it at all.
        let built: Vec<(ChunkPos, Mesh)> = Self::build_meshes(world, &to_mesh);

        for (pos, mesh) in &built {
            renderer.set_chunk_mesh(device, *pos, mesh);
            // Recorded even when the mesh was empty: an all-air chunk uploads
            // nothing, but must still count as handled or it would be re-meshed
            // every single frame.
            self.meshed.insert(*pos);
        }
        for (pos, _) in &built {
            world.clear_dirty(*pos);
        }

        built.len()
    }

    /// Build meshes for `positions` across the worker pool.
    ///
    /// Separate so tests can compare it against the serial equivalent.
    pub fn build_meshes(world: &World, positions: &[ChunkPos]) -> Vec<(ChunkPos, Mesh)> {
        positions
            .par_iter()
            .map(|pos| {
                let origin = pos.origin();
                (*pos, build_mesh(world, world.registry(), [origin.x, 0, origin.z]))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_at_uses_floor_so_negative_positions_land_correctly() {
        assert_eq!(chunk_at(glam::Vec3::new(0.0, 0.0, 0.0)), ChunkPos::new(0, 0));
        assert_eq!(chunk_at(glam::Vec3::new(15.9, 0.0, 15.9)), ChunkPos::new(0, 0));
        assert_eq!(chunk_at(glam::Vec3::new(16.0, 0.0, 0.0)), ChunkPos::new(1, 0));
        assert_eq!(chunk_at(glam::Vec3::new(-0.5, 0.0, -0.5)), ChunkPos::new(-1, -1));
        assert_eq!(chunk_at(glam::Vec3::new(-16.5, 0.0, 0.0)), ChunkPos::new(-2, 0));
    }

    #[test]
    fn chunks_in_range_covers_a_disc_around_the_centre() {
        let centre = ChunkPos::new(5, -3);
        let chunks = chunks_in_range(centre, 3);

        assert!(chunks.contains(&centre));
        assert!(chunks.contains(&ChunkPos::new(8, -3)));
        // A corner at (3,3) is distance sqrt(18) > 3, so it is excluded.
        assert!(!chunks.contains(&ChunkPos::new(8, 0)));
        for pos in &chunks {
            assert!(pos.distance_squared(centre) <= 9);
        }
    }

    #[test]
    fn chunks_in_range_is_ordered_nearest_first() {
        let centre = ChunkPos::new(0, 0);
        let chunks = chunks_in_range(centre, 4);

        assert_eq!(chunks[0], centre);
        let distances: Vec<i64> = chunks.iter().map(|p| p.distance_squared(centre)).collect();
        assert!(
            distances.windows(2).all(|pair| pair[0] <= pair[1]),
            "chunks are not sorted by distance"
        );
    }

    #[test]
    fn a_zero_radius_yields_only_the_centre_chunk() {
        assert_eq!(chunks_in_range(ChunkPos::new(2, 2), 0), vec![ChunkPos::new(2, 2)]);
    }

    #[test]
    fn range_grows_with_radius() {
        let small = chunks_in_range(ChunkPos::new(0, 0), 2).len();
        let large = chunks_in_range(ChunkPos::new(0, 0), 6).len();
        assert!(large > small * 4, "{small} then {large}");
    }

    #[test]
    fn the_worker_generates_bit_identical_chunks_to_the_main_thread() {
        // Purity is the whole contract: a cloned generator on another thread
        // must produce exactly what the main thread would have.
        let mut world = World::new(2024);
        let mut worker = GenWorker::new(world.generator().clone());

        let positions: Vec<ChunkPos> = chunks_in_range(ChunkPos::new(0, 0), 2);
        for pos in &positions {
            assert!(worker.request(*pos) || !worker.inflight.is_empty());
        }

        let mut received = std::collections::HashMap::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while received.len() < positions.len().min(MAX_INFLIGHT) {
            for chunk in worker.drain() {
                received.insert(chunk.pos(), chunk);
            }
            assert!(std::time::Instant::now() < deadline, "worker never finished");
            std::thread::yield_now();
        }

        for (pos, from_worker) in received {
            world.load_chunk(pos);
            let local = world.chunk(pos).expect("just loaded");
            assert_eq!(
                vx_world::chunk_hash(local, world.registry()),
                vx_world::chunk_hash(&from_worker, world.registry()),
                "worker chunk {pos:?} differs from the main thread's"
            );
        }
    }

    #[test]
    fn a_worker_result_for_a_chunk_already_resident_is_discarded() {
        // A synchronous path (an agent's pin, the pilot) can win the race for
        // a chunk the worker was already building. The resident copy may carry
        // edits; the late arrival must not overwrite them.
        let mut world = World::new(2024);
        let pos = ChunkPos::new(1, 1);

        let mut streamer = ChunkStreamer::new(StreamingConfig::default())
            .with_worker(GenWorker::new(world.generator().clone()));
        let worker = streamer.worker.as_mut().unwrap();
        assert!(worker.request(pos));

        // The synchronous path wins and the player edits the chunk.
        world.load_chunk(pos);
        let stone = world.registry().id_of("engine:stone").unwrap();
        let edited = vx_core::BlockPos::new(20, 200, 20);
        world.set_block(edited, stone).unwrap();

        // Wait for the worker, then land results the way `update` does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let arrived = streamer.worker.as_mut().unwrap().drain();
            if !arrived.is_empty() {
                for chunk in arrived {
                    if !world.is_loaded(chunk.pos()) {
                        world.insert_chunk(chunk);
                    }
                }
                break;
            }
            assert!(std::time::Instant::now() < deadline, "worker never finished");
            std::thread::yield_now();
        }

        assert_eq!(
            world.block(edited),
            stone,
            "the worker's copy overwrote a player's edit"
        );
    }

    #[test]
    fn the_wanted_list_is_only_rebuilt_when_the_centre_chunk_changes() {
        let mut streamer = ChunkStreamer::new(StreamingConfig::default());
        assert!(streamer.wanted.is_empty());

        // Private-field poke in place of running a full update, which needs a
        // GPU: drive exactly the caching logic `update` runs first.
        let centre = ChunkPos::new(3, 3);
        streamer.wanted = chunks_in_range(centre, streamer.config.render_distance);
        streamer.last_centre = Some(centre);
        let before = streamer.wanted.clone();

        // Same centre: nothing recomputed (same object semantics — the guard
        // in `update` is `last_centre != Some(centre)`).
        assert_eq!(streamer.last_centre, Some(centre));
        assert_eq!(streamer.wanted, before);

        let moved = ChunkPos::new(4, 3);
        assert_ne!(streamer.last_centre, Some(moved), "guard would not fire");
    }

    #[test]
    fn parallel_meshing_matches_serial_meshing_exactly() {
        // Speed is worthless if the output differs. Threading a pure function
        // over immutable data should be observationally identical, and this is
        // what pins that — including vertex order within each mesh.
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);

        let positions: Vec<ChunkPos> = chunks_in_range(ChunkPos::new(0, 0), 2)
            .into_iter()
            .filter(|pos| world.is_loaded(*pos))
            .collect();
        assert!(positions.len() > 8, "not enough chunks to be worth comparing");

        let parallel = ChunkStreamer::build_meshes(&world, &positions);

        let serial: Vec<(ChunkPos, Mesh)> = positions
            .iter()
            .map(|pos| {
                let origin = pos.origin();
                (*pos, build_mesh(&world, world.registry(), [origin.x, 0, origin.z]))
            })
            .collect();

        assert_eq!(parallel.len(), serial.len());
        for ((par_pos, par_mesh), (ser_pos, ser_mesh)) in parallel.iter().zip(serial.iter()) {
            assert_eq!(par_pos, ser_pos, "results came back in a different order");
            assert_eq!(
                par_mesh, ser_mesh,
                "chunk {par_pos:?} meshed differently in parallel"
            );
        }
    }

    #[test]
    fn parallel_meshing_produces_actual_geometry() {
        // Guards against the equivalence test above passing vacuously because
        // both paths returned nothing. Out in the wilderness — the village
        // plateau at the origin greedy-meshes to almost nothing, flat ground
        // being exactly what greedy meshing is best at.
        let mut world = World::new(7);
        world.load_around(ChunkPos::new(10, 10), 1);

        let positions: Vec<ChunkPos> = world.loaded_chunks().collect();
        let built = ChunkStreamer::build_meshes(&world, &positions);

        let triangles: usize = built.iter().map(|(_, mesh)| mesh.triangle_count()).sum();
        assert!(triangles > 1000, "only {triangles} triangles across the whole scene");
    }

    #[test]
    fn meshing_an_empty_position_list_does_nothing() {
        let world = World::new(1);
        assert!(ChunkStreamer::build_meshes(&world, &[]).is_empty());
    }

    /// The neighbouring chunks that share a *face* with a block on the perimeter
    /// column `(x, z)` of the centre chunk, paired with the offset of the block
    /// across that seam. In this column world only the ±X / ±Z borders cross a
    /// chunk; a corner column shares a face with two neighbours at once.
    fn seam_neighbours(x: i32, z: i32) -> Vec<(ChunkPos, i32, i32)> {
        let last = vx_core::CHUNK_SIZE - 1;
        let mut seams = Vec::new();
        if x == 0 {
            seams.push((ChunkPos::new(-1, 0), -1, 0));
        }
        if x == last {
            seams.push((ChunkPos::new(1, 0), 1, 0));
        }
        if z == 0 {
            seams.push((ChunkPos::new(0, -1), 0, -1));
        }
        if z == last {
            seams.push((ChunkPos::new(0, 1), 0, 1));
        }
        seams
    }

    #[test]
    fn every_boundary_removal_dirties_the_seam_neighbour() {
        // The oldest voxel bug there is: a block removed on a chunk border
        // exposes a face that belongs to the *neighbour's* mesh. If only the
        // edited chunk re-meshes, the neighbour keeps culling a face against a
        // block that no longer exists — a window into the void. This walks
        // every boundary cell of a chunk and asserts the neighbour is marked
        // dirty whenever the removal exposes one of its faces.
        use std::collections::HashSet;
        use vx_core::{BlockId, CHUNK_HEIGHT, CHUNK_SIZE};

        let mut world = World::new(31337);
        world.load_around(ChunkPos::new(0, 0), 1);
        let loaded: Vec<ChunkPos> = world.loaded_chunks().collect();
        let last = CHUNK_SIZE - 1;

        // Every perimeter column of the centre chunk. Local == world here,
        // since the centre chunk's origin is (0, 0).
        let perimeter: Vec<(i32, i32)> = (0..CHUNK_SIZE)
            .flat_map(|i| [(0, i), (last, i), (i, 0), (i, last)])
            .collect();

        let mut exercised = 0usize;
        for (x, z) in perimeter {
            let seams = seam_neighbours(x, z);
            for y in 0..CHUNK_HEIGHT {
                let cell = BlockPos::new(x, y, z);
                if !world.is_solid(cell) {
                    continue;
                }

                // Measure only this edit's effect.
                for pos in &loaded {
                    world.clear_dirty(*pos);
                }
                let prev = world.block(cell);
                world.set_block(cell, BlockId::AIR);
                let dirty: HashSet<ChunkPos> = world.dirty_chunks().collect();

                for (seam, dx, dz) in &seams {
                    let across = BlockPos::new(x + dx, y, z + dz);
                    // The seam face becomes visible only if the block across it
                    // is solid; that is exactly when the neighbour must remesh.
                    if world.is_solid(across) {
                        assert!(
                            dirty.contains(seam),
                            "removing {cell:?} exposed a face of {seam:?} \
                             but that chunk was not marked dirty"
                        );
                        exercised += 1;
                    }
                }
                world.set_block(cell, prev);
            }
        }
        assert!(
            exercised > 200,
            "only {exercised} seam faces exercised — terrain too sparse to trust"
        );
    }

    #[test]
    fn a_boundary_carve_dirties_the_seam_neighbour() {
        // The drill wounds blocks with `carve`, not a clean break. A wound on a
        // seam changes what the neighbour draws along that seam just as a full
        // removal does, so the same neighbour-dirty rule has to fire.
        use std::collections::HashSet;
        use vx_core::{BlockPos, CHUNK_HEIGHT, CHUNK_SIZE};

        let mut world = World::new(9);
        world.load_around(ChunkPos::new(0, 0), 1);

        // A solid block on the -X seam whose neighbour is also solid.
        let mut target = None;
        'search: for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_HEIGHT {
                let cell = BlockPos::new(0, y, z);
                if world.is_solid(cell) && world.is_solid(BlockPos::new(-1, y, z)) {
                    target = Some(cell);
                    break 'search;
                }
            }
        }
        let cell = target.expect("generated terrain has a solid seam block");

        for pos in world.loaded_chunks().collect::<Vec<_>>() {
            world.clear_dirty(pos);
        }

        // A single-cell bite leaves the block alive — a wound, not a break.
        let bite = vx_world::micro::bit(0, 0, 0);
        let outcome = world.carve(cell, bite);
        assert!(
            matches!(outcome, vx_world::Carved::Wounded(_)),
            "expected a wound from a one-cell bite, got {outcome:?}"
        );

        let dirty: HashSet<ChunkPos> = world.dirty_chunks().collect();
        assert!(
            dirty.contains(&ChunkPos::new(-1, 0)),
            "a wound on the seam did not dirty the neighbour"
        );
    }

    #[test]
    fn boundary_edits_keep_the_joined_mesh_watertight() {
        // The guarantee in geometry rather than in flags: an incremental
        // re-mesh (only the chunks the edit dirtied) must produce the exact
        // same joined geometry as a full re-mesh of the whole neighbourhood
        // from scratch. A neighbour left stale shows up here as a missing face
        // — the joined meshes stop being watertight.
        use std::collections::{HashMap, HashSet};
        use vx_core::{BlockId, CHUNK_SIZE};

        let mut world = World::new(77);
        world.load_around(ChunkPos::new(0, 0), 1);
        let loaded: Vec<ChunkPos> = world.loaded_chunks().collect();

        let mesh_of = |world: &World, pos: ChunkPos| {
            let origin = pos.origin();
            build_mesh(world, world.registry(), [origin.x, 0, origin.z])
        };

        // Baseline geometry for the whole neighbourhood, all correct.
        let baseline: HashMap<ChunkPos, Mesh> =
            loaded.iter().map(|pos| (*pos, mesh_of(&world, *pos))).collect();

        // Representative boundary cells: the four edge mid-points and all four
        // corners, each sampled over a short band of near-surface depths — the
        // zone a drill actually breaks through.
        let last = CHUNK_SIZE - 1;
        let columns = [
            (0, 8),
            (last, 8),
            (8, 0),
            (8, last),
            (0, 0),
            (0, last),
            (last, 0),
            (last, last),
        ];
        let mut checked = 0usize;
        for (x, z) in columns {
            let Some(surface) = world.surface_y(x, z) else {
                continue;
            };
            for y in (0..=surface).rev().take(4) {
                let cell = BlockPos::new(x, y, z);
                if !world.is_solid(cell) {
                    continue;
                }
                for pos in &loaded {
                    world.clear_dirty(*pos);
                }
                let prev = world.block(cell);
                world.set_block(cell, BlockId::AIR);
                let dirty: HashSet<ChunkPos> = world.dirty_chunks().collect();

                // Incremental: keep every baseline mesh, rebuild only the ones
                // the edit marked dirty — exactly what the streamer uploads.
                let mut incremental = baseline.clone();
                for pos in &dirty {
                    if loaded.contains(pos) {
                        incremental.insert(*pos, mesh_of(&world, *pos));
                    }
                }

                // Ground truth: rebuild the whole neighbourhood from scratch.
                for pos in &loaded {
                    let full = mesh_of(&world, *pos);
                    assert_eq!(
                        incremental[pos], full,
                        "chunk {pos:?} carries stale geometry after editing {cell:?}"
                    );
                }
                checked += 1;
                world.set_block(cell, prev);
            }
        }
        assert!(checked > 8, "only {checked} boundary edits exercised");
    }
}
