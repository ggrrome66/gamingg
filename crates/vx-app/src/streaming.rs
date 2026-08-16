//! Keeping loaded chunks and their meshes in step with the camera.
//!
//! Meshing is throttled: rebuilding every newly visible chunk in one frame
//! produces a visible hitch when flying, so a bounded number are processed per
//! frame and the rest wait their turn.

use vx_core::{BlockPos, ChunkPos};
use vx_mesh::build_mesh;
use vx_render::Renderer;
use vx_world::World;

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
            generate_budget: 4,
            mesh_budget: 4,
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

    /// Chunks currently uploaded to the renderer.
    pub fn meshed_count(&self) -> usize {
        self.meshed.len()
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
    ) -> usize {
        let radius = self.config.render_distance;
        let wanted = chunks_in_range(centre, radius);

        // Generate a few of the nearest missing chunks.
        let mut generated = 0;
        for pos in &wanted {
            if generated >= self.config.generate_budget {
                break;
            }
            if !world.is_loaded(*pos) {
                world.load_chunk(*pos);
                generated += 1;
            }
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

        // Mesh whatever is loaded but not yet uploaded, plus anything the
        // world has flagged dirty after an edit.
        let dirty: std::collections::HashSet<ChunkPos> = world.dirty_chunks().collect();
        let mut uploaded = 0;
        for pos in &wanted {
            if uploaded >= self.config.mesh_budget {
                break;
            }
            if !world.is_loaded(*pos) {
                continue;
            }
            let needs_mesh = !self.meshed.contains(pos) || dirty.contains(pos);
            if !needs_mesh {
                continue;
            }

            let origin = pos.origin();
            let mesh = build_mesh(&*world, world.registry(), [origin.x, 0, origin.z]);
            renderer.set_chunk_mesh(device, *pos, &mesh);
            // Recorded even when the mesh was empty: an all-air chunk uploads
            // nothing, but must still count as handled or it would be re-meshed
            // every single frame.
            self.meshed.insert(*pos);
            world.clear_dirty(*pos);
            uploaded += 1;
        }

        uploaded
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
}
