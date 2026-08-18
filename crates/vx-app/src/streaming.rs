//! Keeping loaded chunks and their meshes in step with the camera.
//!
//! Meshing runs on a worker pool. It is still throttled per frame — rebuilding
//! everything newly visible in one go produces a visible hitch when flying —
//! but the budget can be far higher now that the work is spread across cores
//! instead of blocking the frame on one.
//!
//! The split is: mutate the world first (generate, unload), then build meshes
//! from an immutable borrow in parallel, then upload serially. Uploads stay on
//! one thread because GPU buffer creation is not the bottleneck and keeping it
//! single-threaded avoids needing a shared device lock.

use rayon::prelude::*;

use vx_core::{BlockPos, ChunkPos};
use vx_mesh::{build_mesh, Mesh};
use vx_render::Renderer;
use vx_world::{World, WorldSave};

/// Chunk-streaming settings.
#[derive(Debug, Clone, Copy)]
pub struct StreamingConfig {
    /// Chunks visible in every direction.
    pub render_distance: i32,
    /// Chunks generated per frame.
    pub generate_budget: usize,
    /// Chunk meshes rebuilt and uploaded per frame.
    pub mesh_budget: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            render_distance: 8,
            // Generation is still serial and cheap; meshing is the expensive
            // half and is now parallel, so its budget can be much larger.
            generate_budget: 8,
            mesh_budget: 24,
        }
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

/// Tracks what has been meshed, and advances streaming one frame at a time.
pub struct ChunkStreamer {
    config: StreamingConfig,
    /// Chunks currently uploaded to the renderer.
    meshed: std::collections::HashSet<ChunkPos>,
}

impl ChunkStreamer {
    pub fn new(config: StreamingConfig) -> Self {
        ChunkStreamer {
            config,
            meshed: std::collections::HashSet::new(),
        }
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
        let wanted = chunks_in_range(centre, radius);

        // Bring in a few of the nearest missing chunks. A chunk that was saved
        // takes precedence over generating it afresh, or every edit would be
        // undone the moment you walked away and back.
        let mut generated = 0;
        for pos in &wanted {
            if generated >= self.config.generate_budget {
                break;
            }
            if world.is_loaded(*pos) {
                continue;
            }
            match save.map(|save| save.load_chunk(*pos, world.registry())) {
                Some(Ok(Some(chunk))) => world.insert_chunk(chunk),
                Some(Err(error)) => {
                    // A damaged region should not take the game down; fall
                    // back to generated terrain and say so once.
                    log::error!("could not load chunk {pos:?}: {error}");
                    world.load_chunk(*pos);
                }
                _ => {
                    world.load_chunk(*pos);
                }
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
        // `wanted` is already sorted that way.
        let dirty: std::collections::HashSet<ChunkPos> = world.dirty_chunks().collect();
        let to_mesh: Vec<ChunkPos> = wanted
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
}
