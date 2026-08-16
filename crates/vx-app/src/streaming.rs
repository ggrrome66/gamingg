//! Keeping loaded chunks and their meshes in step with the camera.
//!
//! Meshing is throttled: rebuilding every newly visible chunk in one frame
//! produces a visible hitch when flying, so a bounded number are processed per
//! frame and the rest wait their turn.

use vx_core::{BlockPos, ChunkPos, Face};
use vx_mesh::build_mesh;
use vx_render::Renderer;
use vx_save::WorldStore;
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
        mut store: Option<&mut WorldStore>,
    ) -> usize {
        let radius = self.config.render_distance;
        let wanted = chunks_in_range(centre, radius);

        // Fill in the nearest missing chunks, preferring what is on disk.
        let mut generated = 0;
        for pos in &wanted {
            if generated >= self.config.generate_budget {
                break;
            }
            if world.is_loaded(*pos) {
                continue;
            }

            // A chunk that fails to decode is logged and regenerated rather
            // than taking the session down: losing one column beats losing the
            // world to a single bad payload.
            let restored = match store.as_deref_mut() {
                Some(store) => match store.load_chunk(*pos, world.registry()) {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        log::error!("could not load chunk {},{}: {error}", pos.x, pos.z);
                        None
                    }
                },
                None => None,
            };

            match restored {
                Some(chunk) => world.insert_chunk(chunk),
                None => {
                    world.load_chunk(*pos);
                }
            }

            // Light before meshing, or the chunk is uploaded pitch black and
            // only corrects itself when something else dirties it. Neighbours
            // are queued rather than done here: light spilling across the new
            // seam changes them too, but that can wait for a tick.
            world.relight_chunk(*pos);
            for face in [Face::NegX, Face::PosX, Face::NegZ, Face::PosZ] {
                let offset = face.offset();
                world.request_relight(ChunkPos::new(pos.x + offset[0], pos.z + offset[2]));
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

        // Anything the player changed has to be staged before it leaves
        // memory. This is the moment edits used to be lost.
        let unloaded = world.unload_beyond(centre, keep);
        if let Some(store) = store {
            for chunk in unloaded.iter().filter(|chunk| chunk.is_modified()) {
                if let Err(error) = store.store_chunk(chunk, world.registry()) {
                    log::error!(
                        "could not stage chunk {},{} for saving: {error}",
                        chunk.pos().x,
                        chunk.pos().z
                    );
                }
            }
        }

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

    /// Everything meshed up front, so a later update only reflects edits.
    fn settled() -> ChunkStreamer {
        ChunkStreamer::new(StreamingConfig {
            render_distance: 1,
            generate_budget: usize::MAX,
            mesh_budget: usize::MAX,
        })
    }

    #[test]
    fn an_edit_reaches_the_uploaded_geometry() {
        // The seam the unit tests either side of it cannot cover: an edit marks
        // its chunk dirty, the streamer notices on the next update, and the
        // renderer ends up holding different triangles. Break any link in that
        // chain and the world silently stops responding to clicks.
        let Ok(context) = vx_render::GpuContext::headless_blocking() else {
            eprintln!("skipping: no Vulkan adapter available");
            return;
        };

        let mut renderer = vx_render::Renderer::new(
            &context,
            vx_render::headless::CAPTURE_FORMAT,
            64,
            64,
        );
        let mut world = vx_world::World::new(2024);
        let mut streamer = settled();
        let centre = ChunkPos::new(0, 0);

        world.load_around(centre, 1);
        streamer.update(&mut world, &mut renderer, &context.device, centre, None);

        // A second update with nothing dirty must upload nothing, or the
        // "changed" assertions below prove nothing.
        let idle = streamer.update(&mut world, &mut renderer, &context.device, centre, None);
        assert_eq!(idle, 0, "the streamer re-meshed a clean world");
        let before = renderer.triangle_count();

        // A block floating clear of the terrain, so it touches nothing and
        // hides none of its own faces: exactly six quads, twelve triangles.
        let surface = world.surface_y(8, 8).expect("centre chunk is loaded");
        let floating = BlockPos::new(8, surface + 5, 8);
        let stone = world.registry().id_of("engine:stone").unwrap();
        world.place_block(floating, stone).unwrap();

        let uploaded = streamer.update(&mut world, &mut renderer, &context.device, centre, None);

        assert_eq!(uploaded, 1, "the edited chunk was not re-meshed");
        assert_eq!(
            renderer.triangle_count(),
            before + 12,
            "placing an isolated block should add six faces of geometry"
        );

        // And breaking it puts the world back exactly as it was.
        world.break_block(floating).unwrap();
        streamer.update(&mut world, &mut renderer, &context.device, centre, None);

        assert_eq!(
            renderer.triangle_count(),
            before,
            "breaking the block did not undo the geometry it added"
        );
    }

    /// A scratch saves directory that cleans up after itself.
    struct TempSaves(std::path::PathBuf);

    impl TempSaves {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "vx-app-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempSaves(path)
        }
    }

    impl Drop for TempSaves {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_build_survives_walking_away_and_coming_back() {
        // The whole point of persistence, end to end through the streamer:
        // place a block, travel far enough that its chunk unloads, then return
        // with a fresh world and store. Before this, `unload_beyond` dropped
        // the chunk and the edit was gone.
        let Ok(context) = vx_render::GpuContext::headless_blocking() else {
            eprintln!("skipping: no Vulkan adapter available");
            return;
        };
        let saves = TempSaves::new("roundtrip");

        let mut renderer =
            vx_render::Renderer::new(&context, vx_render::headless::CAPTURE_FORMAT, 64, 64);
        let mut streamer = settled();
        let origin = ChunkPos::new(0, 0);
        let marker = BlockPos::new(4, 200, 4);

        let placed = {
            let mut store = vx_save::WorldStore::open(&saves.0, "test", 2024, vx_world::gen::GENERATOR_VERSION).unwrap();
            let mut world = vx_world::World::new(store.seed());

            streamer.update(
                &mut world,
                &mut renderer,
                &context.device,
                origin,
                Some(&mut store),
            );

            let stone = world.registry().id_of("engine:stone").unwrap();
            world.place_block(marker, stone).unwrap();
            assert_eq!(world.modified_chunks().count(), 1);

            // Travel far enough that the edited chunk falls outside the keep
            // radius, which is what stages it for writing.
            streamer.update(
                &mut world,
                &mut renderer,
                &context.device,
                ChunkPos::new(400, 400),
                Some(&mut store),
            );
            assert!(!world.is_loaded(origin), "the chunk never unloaded");

            store.flush().unwrap();
            stone
        };

        // Nothing shared with the first session but the directory on disk.
        let mut store = vx_save::WorldStore::open(&saves.0, "test", 999, vx_world::gen::GENERATOR_VERSION).unwrap();
        assert_eq!(store.seed(), 2024, "the saved seed was not honoured");

        let mut world = vx_world::World::new(store.seed());
        let mut streamer = settled();
        streamer.update(
            &mut world,
            &mut renderer,
            &context.device,
            origin,
            Some(&mut store),
        );

        assert_eq!(
            world.block(marker),
            placed,
            "the block did not survive the round trip"
        );
    }

    #[test]
    fn untouched_terrain_is_never_written_to_disk() {
        // Generation is reproducible from the seed, so streaming across a
        // world without building anything must leave the save empty.
        let Ok(context) = vx_render::GpuContext::headless_blocking() else {
            eprintln!("skipping: no Vulkan adapter available");
            return;
        };
        let saves = TempSaves::new("clean");

        let mut renderer =
            vx_render::Renderer::new(&context, vx_render::headless::CAPTURE_FORMAT, 64, 64);
        let mut store = vx_save::WorldStore::open(&saves.0, "test", 7, vx_world::gen::GENERATOR_VERSION).unwrap();
        let mut world = vx_world::World::new(store.seed());
        let mut streamer = settled();

        for centre in [ChunkPos::new(0, 0), ChunkPos::new(50, 0), ChunkPos::new(0, 50)] {
            streamer.update(
                &mut world,
                &mut renderer,
                &context.device,
                centre,
                Some(&mut store),
            );
        }

        assert_eq!(
            store.flush().unwrap(),
            0,
            "generated terrain was written to disk"
        );
    }

    #[test]
    fn streaming_without_a_store_still_works() {
        // Screenshots and smoke tests run with no world on disk at all.
        let Ok(context) = vx_render::GpuContext::headless_blocking() else {
            eprintln!("skipping: no Vulkan adapter available");
            return;
        };
        let mut renderer =
            vx_render::Renderer::new(&context, vx_render::headless::CAPTURE_FORMAT, 64, 64);
        let mut world = vx_world::World::new(3);
        let mut streamer = settled();

        let uploaded = streamer.update(
            &mut world,
            &mut renderer,
            &context.device,
            ChunkPos::new(0, 0),
            None,
        );
        assert!(uploaded > 0);
    }

    #[test]
    fn an_edit_across_a_chunk_seam_re_meshes_both_sides() {
        // A block on a chunk edge changes the neighbour's seam faces too.
        // Missing that leaves a visible hole along the boundary.
        let Ok(context) = vx_render::GpuContext::headless_blocking() else {
            eprintln!("skipping: no Vulkan adapter available");
            return;
        };

        let mut renderer = vx_render::Renderer::new(
            &context,
            vx_render::headless::CAPTURE_FORMAT,
            64,
            64,
        );
        let mut world = vx_world::World::new(2024);
        let mut streamer = settled();
        let centre = ChunkPos::new(0, 0);

        world.load_around(centre, 1);
        streamer.update(&mut world, &mut renderer, &context.device, centre, None);
        assert_eq!(
            streamer.update(&mut world, &mut renderer, &context.device, centre, None),
            0
        );

        // x = 0 is the first column of chunk 0, so its NegX neighbour lives in
        // chunk -1.
        let surface = world.surface_y(0, 8).expect("centre chunk is loaded");
        let seam = BlockPos::new(0, surface + 5, 8);
        let stone = world.registry().id_of("engine:stone").unwrap();
        world.place_block(seam, stone).unwrap();

        let uploaded = streamer.update(&mut world, &mut renderer, &context.device, centre, None);

        assert_eq!(
            uploaded, 2,
            "an edit on a chunk edge must re-mesh the chunk across the seam too"
        );
    }
}
