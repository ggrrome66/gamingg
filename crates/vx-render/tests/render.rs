//! End-to-end render tests.
//!
//! These drive the real pipeline — shader compilation, vertex layout, depth
//! test, face culling, texture sampling — and read the pixels back. They need
//! a Vulkan adapter but not a GPU or a display: a software implementation such
//! as lavapipe (`mesa-vulkan-drivers`) is enough, which is how they run in CI.
//!
//! Where no adapter exists at all the tests skip rather than fail, so a
//! contributor without Vulkan installed is not blocked.

use glam::Vec3;
use vx_core::{ChunkPos, CHUNK_SIZE};
use vx_mesh::build_mesh;
use vx_render::headless::{capture_frame, Capture, CAPTURE_FORMAT};
use vx_render::{Camera, GpuContext, Object, Renderer, SKY_COLOUR};
use vx_world::World;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

/// Sky colour as the 8-bit sRGB texels it becomes in the capture.
fn sky_rgb() -> [u8; 3] {
    // The target is sRGB, and the clear value is specified in linear space, so
    // the hardware encodes it on write. Match that conversion here rather than
    // hardcoding numbers that would drift if the sky colour changed.
    let encode = |linear: f64| -> u8 {
        let srgb = if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    [
        encode(SKY_COLOUR.r),
        encode(SKY_COLOUR.g),
        encode(SKY_COLOUR.b),
    ]
}

/// A context, or `None` when the machine has no usable adapter.
fn context() -> Option<GpuContext> {
    match GpuContext::headless_blocking() {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipping render test: no Vulkan adapter available ({error})");
            None
        }
    }
}

/// Build a world, mesh the chunks around the origin and upload them.
fn scene(context: &GpuContext, renderer: &mut Renderer, radius: i32) -> World {
    let mut world = World::new(2024);
    world.load_around(ChunkPos::new(0, 0), radius);

    let positions: Vec<ChunkPos> = (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .filter(|pos| world.is_loaded(*pos))
        .collect();

    for pos in positions {
        let origin = pos.origin();
        let mesh = build_mesh(&world, world.registry(), [origin.x, 0, origin.z]);
        renderer.set_chunk_mesh(&context.device, pos, &mesh);
    }
    world
}

/// Save a capture next to the build output, so failures can be eyeballed.
fn save(capture: &Capture, name: &str) {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(error) = capture.write_ppm(&path) {
        eprintln!("could not write {}: {error}", path.display());
    } else {
        eprintln!("wrote {}", path.display());
    }
}

#[test]
fn an_empty_world_renders_as_plain_sky() {
    let Some(context) = context() else { return };
    let renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);

    // The control case: with nothing uploaded, every pixel is the clear colour.
    assert_eq!(
        capture.distinct_colours(),
        1,
        "an empty scene should be a single flat colour"
    );
    assert_eq!(capture.pixel(WIDTH / 2, HEIGHT / 2)[0..3], sky_rgb());
}

#[test]
fn terrain_renders_and_fills_the_lower_half_of_the_frame() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    assert!(renderer.loaded_chunk_count() > 0, "no chunk meshes were uploaded");
    assert!(renderer.triangle_count() > 0, "uploaded meshes have no triangles");

    // Stand above the surface at the origin, looking slightly down.
    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.0, surface as f32 + 12.0, 24.0),
        yaw: 0.0,
        pitch: -0.45,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "terrain.ppm");

    let sky = sky_rgb();
    let covered = capture.fraction_differing_from(sky);
    assert!(
        covered > 0.25,
        "terrain covers only {:.1}% of the frame; the world may not be drawing",
        covered * 100.0
    );

    // Looking down, the bottom of the frame must be ground, and it must be
    // shaded rather than a single flat colour.
    let bottom_row: Vec<[u8; 4]> = (0..WIDTH).map(|x| capture.pixel(x, HEIGHT - 1)).collect();
    let bottom_is_sky = bottom_row
        .iter()
        .filter(|texel| texel[0..3] == sky)
        .count();
    assert!(
        bottom_is_sky < bottom_row.len() / 4,
        "the bottom of the frame is mostly sky; the camera may be looking the wrong way"
    );
    assert!(
        capture.distinct_colours() > 20,
        "only {} distinct colours: textures or lighting are not being applied",
        capture.distinct_colours()
    );
}

#[test]
fn the_camera_looking_up_sees_only_sky() {
    // Guards the projection's handedness: if the Y axis were flipped, pitching
    // up would show ground instead.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 1);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.0, surface as f32 + 4.0, 0.0),
        pitch: 1.4, // very nearly straight up
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let covered = capture.fraction_differing_from(sky_rgb());
    assert!(
        covered < 0.10,
        "looking up shows {:.1}% geometry; the vertical axis is probably inverted",
        covered * 100.0
    );
}

/// Build a quad facing the camera (+Z) at depth `z`, textured with `tile`.
///
/// Winding matches what the mesher emits for a `PosZ` face, so this exercises
/// the same front-face convention the pipeline is configured for.
fn facing_quad(z: f32, half_extent: f32, tile: u32) -> vx_mesh::Mesh {
    let (lo, hi) = (-half_extent, half_extent);
    let corners = [[lo, lo, z], [hi, lo, z], [hi, hi, z], [lo, hi, z]];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    let vertices = corners
        .into_iter()
        .zip(uvs)
        .map(|(position, uv)| vx_mesh::Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            uv,
            tile,
        })
        .collect();

    vx_mesh::Mesh {
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

#[test]
fn nearer_geometry_occludes_farther_geometry() {
    // Exercises the depth buffer directly. Two overlapping quads are uploaded
    // as separate chunks, so draw order depends on hash iteration order —
    // without a working depth test the winner would vary between runs.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);

    // Frustum culling derives a chunk's bounding box from its `ChunkPos` key,
    // which is correct for real meshes — the mesher never emits geometry
    // outside the chunk it was asked for. These synthetic quads deliberately
    // break that: they sit at the origin under keys that place them elsewhere,
    // so culling would (rightly) discard them. Turn it off; this test is about
    // the depth buffer, not visibility.
    renderer.set_culling_enabled(false);

    // Camera at the origin looking down -Z, which is the default orientation.
    let camera = Camera {
        position: glam::Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    // Near: pale sand. Far and larger: near-black bedrock.
    let near = facing_quad(-5.0, 2.0, vx_render::tiles::slot::SAND);
    let far = facing_quad(-20.0, 12.0, vx_render::tiles::slot::BEDROCK);
    renderer.set_chunk_mesh(&context.device, ChunkPos::new(0, 0), &near);
    renderer.set_chunk_mesh(&context.device, ChunkPos::new(1, 0), &far);
    assert_eq!(renderer.loaded_chunk_count(), 2);

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "depth.ppm");

    // Dead centre is covered by both quads; the near one must win.
    let centre = capture.pixel(WIDTH / 2, HEIGHT / 2);
    let brightness = centre[0] as u32 + centre[1] as u32 + centre[2] as u32;
    assert!(
        brightness > 300,
        "centre pixel {centre:?} is dark: the far quad is drawing over the near one"
    );

    // Away from the centre only the far quad covers the frame, so that region
    // must be dark — proving the far quad really is being drawn at all.
    let outer = capture.pixel(WIDTH / 2, HEIGHT / 2 + 90);
    let outer_brightness = outer[0] as u32 + outer[1] as u32 + outer[2] as u32;
    assert!(
        outer_brightness < brightness,
        "expected the far quad ({outer:?}) to be darker than the near one ({centre:?})"
    );
}

#[test]
fn solid_terrain_has_no_interior_faces() {
    // Interior faces are culled during meshing, so the inside of the rock is
    // genuinely empty geometry — and the shell's back faces are culled by the
    // rasteriser. A camera buried in stone therefore sees sky, not stone.
    //
    // This is by design and is what keeps triangle counts sane, but it is
    // surprising enough to be worth pinning down: if a future change starts
    // emitting interior faces, this test fails and says why.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.0, surface as f32 - 8.0, 0.0),
        pitch: 0.0,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "underground.ppm");

    let covered = capture.fraction_differing_from(sky_rgb());
    assert!(
        covered < 0.10,
        "{:.1}% of the frame is geometry from inside solid rock; \
         interior faces are no longer being culled",
        covered * 100.0
    );
}

#[test]
fn removing_a_chunk_drops_its_geometry() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    scene(&context, &mut renderer, 1);

    let before = renderer.loaded_chunk_count();
    assert!(before > 0);

    renderer.remove_chunk(ChunkPos::new(0, 0));
    assert_eq!(renderer.loaded_chunk_count(), before - 1);
}

#[test]
fn an_air_only_chunk_uploads_nothing() {
    // Empty meshes must not create zero-sized buffers, which wgpu rejects.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);

    let mut world = World::new(7);
    // A chunk high above the terrain is pure air.
    let pos = ChunkPos::new(500, 500);
    world.load_chunk(pos);
    let empty = vx_mesh::Mesh::default();
    renderer.set_chunk_mesh(&context.device, pos, &empty);

    assert_eq!(renderer.loaded_chunk_count(), 0);
    assert_eq!(renderer.triangle_count(), 0);
}

#[test]
fn resizing_rebuilds_the_depth_buffer_and_still_renders() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    scene(&context, &mut renderer, 1);

    let (wide, tall) = (WIDTH + 64, HEIGHT + 48);
    renderer.resize(&context, wide, tall);
    assert_eq!(renderer.size(), (wide, tall));

    // Rendering at the new size must not fail validation: a stale depth
    // texture of the old size would be rejected here.
    let capture = capture_frame(&context, &renderer, wide, tall);
    assert_eq!(capture.width, wide);
    assert_eq!(capture.pixels.len(), (wide * tall * 4) as usize);
}

#[test]
fn frustum_culling_does_not_change_the_image() {
    // The decisive test. Culling is only ever allowed to skip geometry the
    // camera cannot see, so a culled frame and an unculled one must be
    // byte-for-byte identical. Any mistake in the plane maths shows up here
    // immediately as chunks missing at the edges of the view.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 3);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.0, surface as f32 + 8.0, 0.0),
        yaw: 0.7,
        pitch: -0.2,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    renderer.set_culling_enabled(false);
    let uncalled = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let drawn_without = renderer.visible_chunk_count();

    renderer.set_culling_enabled(true);
    let culled = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let drawn_with = renderer.visible_chunk_count();
    save(&culled, "culled.ppm");

    assert_eq!(
        uncalled.pixels, culled.pixels,
        "culling changed what is on screen: it dropped geometry the camera can see"
    );

    // And it has to actually be skipping something, or the check above passes
    // for the boring reason.
    assert!(
        drawn_with < drawn_without,
        "culling skipped nothing: {drawn_with} of {drawn_without} chunks drawn"
    );
    eprintln!(
        "culling drew {drawn_with} of {drawn_without} chunks ({:.0}% skipped)",
        100.0 * (1.0 - drawn_with as f32 / drawn_without as f32)
    );
}

#[test]
fn culling_holds_up_from_several_directions() {
    // One camera angle could get lucky. Sweep the yaw all the way round and
    // check the invariant survives every orientation, including looking up and
    // down where the near and far planes do the work.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);
    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");

    for step in 0..8 {
        let camera = Camera {
            position: glam::Vec3::new(4.0, surface as f32 + 6.0, 4.0),
            yaw: step as f32 * std::f32::consts::FRAC_PI_4,
            pitch: if step % 3 == 0 { -0.6 } else { 0.3 },
            aspect: WIDTH as f32 / HEIGHT as f32,
            ..Camera::default()
        };
        renderer.update_camera(&context.queue, &camera);

        renderer.set_culling_enabled(false);
        let plain = capture_frame(&context, &renderer, WIDTH, HEIGHT);

        renderer.set_culling_enabled(true);
        let culled = capture_frame(&context, &renderer, WIDTH, HEIGHT);

        assert_eq!(
            plain.pixels, culled.pixels,
            "culling changed the image at yaw step {step}"
        );
    }
}

#[test]
fn culling_skips_most_of_the_world_when_looking_one_way() {
    // The point of the exercise: with a 70-degree view, most of a loaded disc
    // of chunks is behind or beside the camera and should never be submitted.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 4);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.0, surface as f32 + 4.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    let loaded = renderer.loaded_chunk_count();
    let drawn = renderer.visible_chunk_count();

    assert!(loaded > 20, "not enough chunks loaded to be a fair test");
    assert!(
        drawn * 2 < loaded,
        "drew {drawn} of {loaded} chunks; culling is barely helping"
    );
    assert!(drawn > 0, "culled the entire world");
}

#[test]
fn breaking_a_block_changes_what_is_drawn() {
    // The end-to-end check for M2: an edit must reach the screen. It touches
    // every link in the chain — raycast picks the block, the edit clears it and
    // dirties the chunk, the mesher rebuilds, and the frame comes back visibly
    // different.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let mut world = scene(&context, &mut renderer, 1);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        // Directly above the target, looking straight down at it.
        position: glam::Vec3::new(0.5, surface as f32 + 4.0, 0.5),
        pitch: -std::f32::consts::FRAC_PI_2 + 0.05,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);
    let before = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let triangles_before = renderer.triangle_count();

    // Dig a shaft straight down, so the change is unmistakable from above.
    let events = vx_core::EventBus::new();
    for depth in 1..=6 {
        let target = vx_core::BlockPos::new(0, surface - depth, 0);
        vx_world::break_block(&mut world, &events, target)
            .unwrap_or_else(|error| panic!("could not break {target:?}: {error}"));
    }

    // Re-mesh whatever the edit dirtied and re-upload it.
    let dirty: Vec<vx_core::ChunkPos> = world.dirty_chunks().collect();
    assert!(!dirty.is_empty(), "editing did not mark any chunk for remeshing");
    for pos in dirty {
        let origin = pos.origin();
        let mesh = build_mesh(&world, world.registry(), [origin.x, 0, origin.z]);
        renderer.set_chunk_mesh(&context.device, pos, &mesh);
        world.clear_dirty(pos);
    }

    let after = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&after, "edited.ppm");

    assert_ne!(
        before.pixels, after.pixels,
        "the frame is identical after digging a shaft; the edit never reached the screen"
    );
    // Exposing the shaft walls adds geometry that interior culling had removed.
    assert_ne!(
        triangles_before,
        renderer.triangle_count(),
        "triangle count unchanged after carving into solid terrain"
    );
}

/// A camera on the surface at the origin, looking level along -Z.
fn surface_camera(world: &World, height: f32, pitch: f32) -> Camera {
    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    Camera {
        position: Vec3::new(0.5, surface as f32 + height, 0.5),
        yaw: 0.0,
        pitch,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    }
}

#[test]
fn an_object_appears_where_it_is_placed_and_moves_with_its_transform() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    let camera = surface_camera(&world, 3.0, -0.15);
    renderer.update_camera(&context.queue, &camera);

    renderer.set_objects(&context.device, &context.queue, &[]);
    let empty = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    assert_eq!(renderer.visible_object_count(), 0);

    // A cube a few blocks ahead, floating clear of the ground so terrain
    // cannot be what changed.
    let ahead = Vec3::new(camera.position.x, camera.position.y, camera.position.z - 6.0);
    let object = Object::standing(ahead, 1.5, vx_render::tiles::slot::COPPER_ORE);
    renderer.set_objects(&context.device, &context.queue, &[object]);
    assert_eq!(renderer.visible_object_count(), 1);

    let placed = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&placed, "object.ppm");
    assert_ne!(
        empty.pixels, placed.pixels,
        "adding an object changed nothing on screen"
    );

    // Slide it well off to one side. Same object, different matrix, so a
    // different image — this is what proves the instance transform is being
    // applied rather than ignored.
    let aside = Object::standing(ahead + Vec3::new(4.0, 0.0, 0.0), 1.5, vx_render::tiles::slot::COPPER_ORE);
    renderer.set_objects(&context.device, &context.queue, &[aside]);
    let moved = capture_frame(&context, &renderer, WIDTH, HEIGHT);

    assert_ne!(
        placed.pixels, moved.pixels,
        "moving the object did not move what is drawn; the model matrix is being ignored"
    );
}

#[test]
fn terrain_hides_an_object_buried_behind_it() {
    // The reason objects share the terrain pass and its depth buffer. A drone
    // underground must be hidden by the rock above it, and one above ground
    // must not be. Drawing objects into a separate pass, or after a depth
    // clear, would show both.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    // Straight down at the ground, so anything below the surface is behind it.
    let camera = surface_camera(&world, 10.0, -std::f32::consts::FRAC_PI_2 + 0.05);
    renderer.update_camera(&context.queue, &camera);

    renderer.set_objects(&context.device, &context.queue, &[]);
    let nothing = capture_frame(&context, &renderer, WIDTH, HEIGHT);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded") as f32;
    let below = Object::standing(Vec3::new(0.5, surface - 20.0, 0.5), 3.0, 0);
    renderer.set_objects(&context.device, &context.queue, &[below]);
    // It is inside the frustum — culling is not what hides it.
    assert_eq!(renderer.visible_object_count(), 1);
    let buried = capture_frame(&context, &renderer, WIDTH, HEIGHT);

    assert_eq!(
        nothing.pixels, buried.pixels,
        "an object 20 blocks underground is showing through the rock"
    );

    let above = Object::standing(Vec3::new(0.5, surface + 1.0, 0.5), 3.0, 0);
    renderer.set_objects(&context.device, &context.queue, &[above]);
    let visible = capture_frame(&context, &renderer, WIDTH, HEIGHT);

    assert_ne!(
        nothing.pixels, visible.pixels,
        "an object sitting on the surface is not being drawn at all"
    );
}

#[test]
fn culling_stays_pixel_identical_with_objects_present() {
    // The M2.5 guarantee, re-checked now a second draw path shares the frustum.
    // Objects are culled on the CPU as they are uploaded, so a mistake in the
    // bounds would drop one the camera can actually see.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 3);

    let camera = surface_camera(&world, 6.0, -0.2);
    renderer.update_camera(&context.queue, &camera);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded") as f32;
    // A ring of cubes all the way round the camera: some ahead and visible,
    // most behind or beside it and cullable.
    let objects: Vec<Object> = (0..24)
        .map(|step| {
            let angle = step as f32 * std::f32::consts::TAU / 24.0;
            let centre = Vec3::new(
                camera.position.x + angle.cos() * 12.0,
                surface + 1.0,
                camera.position.z + angle.sin() * 12.0,
            );
            Object::standing(centre, 1.2, vx_render::tiles::slot::SAND)
        })
        .collect();

    // Objects are culled at upload, so each configuration needs its own upload.
    renderer.set_culling_enabled(false);
    renderer.set_objects(&context.device, &context.queue, &objects);
    let uncalled = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let drawn_without = renderer.visible_object_count();

    renderer.set_culling_enabled(true);
    renderer.set_objects(&context.device, &context.queue, &objects);
    let culled = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    let drawn_with = renderer.visible_object_count();
    save(&culled, "objects_culled.ppm");

    assert_eq!(
        uncalled.pixels, culled.pixels,
        "culling with objects present changed the image: a visible object was dropped"
    );
    assert_eq!(drawn_without, objects.len() as u32);
    assert!(
        drawn_with < drawn_without,
        "object culling skipped nothing: {drawn_with} of {drawn_without} drawn"
    );
    eprintln!("objects: drew {drawn_with} of {drawn_without}");
}

#[test]
fn a_swarm_of_objects_draws_in_one_call() {
    // Instancing exists so a swarm costs one draw. Uploading far more objects
    // than the batch's initial capacity also exercises the buffer growth path,
    // which would otherwise only be hit once the game had a real swarm.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    let camera = surface_camera(&world, 20.0, -0.9);
    renderer.update_camera(&context.queue, &camera);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded") as f32;
    let swarm: Vec<Object> = (0..200)
        .map(|index| {
            let x = (index % 20) as f32 - 10.0;
            let z = -(index / 20) as f32 - 2.0;
            Object::standing(
                Vec3::new(camera.position.x + x, surface + 6.0, camera.position.z + z),
                0.8,
                vx_render::tiles::slot::BEDROCK,
            )
        })
        .collect();

    renderer.set_objects(&context.device, &context.queue, &swarm);
    assert!(
        renderer.visible_object_count() > 50,
        "only {} of 200 objects survived culling; the swarm is not in view",
        renderer.visible_object_count()
    );

    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "swarm.ppm");
    assert!(
        capture.fraction_differing_from(sky_rgb()) > 0.1,
        "the swarm covers almost nothing; instances may not be drawing"
    );
}

#[test]
fn the_overlay_draws_only_when_set_and_vanishes_when_cleared() {
    // The overlay's contract: never set means byte-identical to a build
    // without it — which is what keeps every culling pixel-equality guarantee
    // intact — and clearing restores exactly that.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 1);

    let camera = surface_camera(&world, 6.0, -0.2);
    renderer.update_camera(&context.queue, &camera);

    let bare = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    assert!(!renderer.has_overlay(0));

    // Two slots at once — a magenta square and a green one — because the HUD
    // and the minimap will always be up together.
    let size = 32u32;
    let magenta: Vec<u8> = (0..size * size).flat_map(|_| [255, 0, 255, 255]).collect();
    let green: Vec<u8> = (0..size * size).flat_map(|_| [0, 255, 0, 255]).collect();
    renderer.set_overlay(
        0,
        &context.device,
        &context.queue,
        (size, size),
        &magenta,
        vx_render::OverlayRect {
            x: 8.0,
            y: 8.0,
            width: 64.0,
            height: 64.0,
        },
    );
    renderer.set_overlay(
        1,
        &context.device,
        &context.queue,
        (size, size),
        &green,
        vx_render::OverlayRect {
            x: 120.0,
            y: 8.0,
            width: 64.0,
            height: 64.0,
        },
    );
    let overlaid = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    assert_ne!(bare.pixels, overlaid.pixels, "the overlay drew nothing");
    // Each quad is where its rect says, in top-left pixel coordinates.
    assert_eq!(overlaid.pixel(40, 40)[0..3], [255, 0, 255]);
    assert_eq!(overlaid.pixel(150, 40)[0..3], [0, 255, 0]);
    // And outside them, the frame is untouched.
    assert_eq!(overlaid.pixel(200, 200), bare.pixel(200, 200));

    // Clearing one slot leaves the other.
    renderer.clear_overlay(0);
    let half = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    assert_eq!(half.pixel(40, 40), bare.pixel(40, 40));
    assert_eq!(half.pixel(150, 40)[0..3], [0, 255, 0]);

    renderer.clear_overlay(1);
    let cleared = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    assert_eq!(bare.pixels, cleared.pixels, "clearing did not fully remove the overlay");
}

#[test]
fn greedy_meshing_keeps_the_triangle_count_modest() {
    // A sanity bound on the mesher at real scale: 25 chunks of terrain should
    // be tens of thousands of triangles, not millions.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    scene(&context, &mut renderer, 2);

    let triangles = renderer.triangle_count();
    let chunks = renderer.loaded_chunk_count() as u32;
    let per_chunk = triangles / chunks.max(1);

    eprintln!("{chunks} chunks, {triangles} triangles ({per_chunk} per chunk)");
    assert!(
        per_chunk < 20_000,
        "{per_chunk} triangles per chunk suggests merging is not working \
         (a {CHUNK_SIZE}-wide column of naive cubes would be far more)"
    );
}
