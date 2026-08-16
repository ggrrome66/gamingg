//! End-to-end render tests.
//!
//! These drive the real pipeline — shader compilation, vertex layout, depth
//! test, face culling, texture sampling — and read the pixels back. They need
//! a Vulkan adapter but not a GPU or a display: a software implementation such
//! as lavapipe (`mesa-vulkan-drivers`) is enough, which is how they run in CI.
//!
//! Where no adapter exists at all the tests skip rather than fail, so a
//! contributor without Vulkan installed is not blocked.

use vx_core::{BlockPos, ChunkPos, CHUNK_SIZE};
use vx_mesh::build_mesh;
use vx_render::headless::{capture_frame, Capture, CAPTURE_FORMAT};
use vx_render::{Camera, GpuContext, Renderer, SKY_COLOUR};
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

/// Pixels close to the selection colour, as it appears after sRGB encoding.
///
/// Matched loosely rather than exactly: the cage is alpha-blended over
/// whatever is behind it, so no pixel is the pure constant. Phosphor green is
/// far enough from terrain and sky that "much greener than it is red" is an
/// unambiguous test.
fn cage_pixels(capture: &Capture) -> usize {
    capture
        .pixels
        .chunks_exact(4)
        .filter(|texel| {
            let (r, g, b) = (texel[0] as i32, texel[1] as i32, texel[2] as i32);
            g > 150 && g - r > 60 && g - b > 60
        })
        .count()
}

/// A camera looking straight down at the column through `(x, z)`.
fn looking_down_at(x: i32, z: i32, height: f32) -> Camera {
    Camera {
        position: glam::Vec3::new(x as f32 + 0.5, height, z as f32 + 0.5),
        yaw: 0.0,
        pitch: -std::f32::consts::FRAC_PI_2 + 0.001,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    }
}

#[test]
fn the_selection_cage_reaches_the_screen() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 1);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = looking_down_at(0, 0, surface as f32 + 3.0);
    renderer.update_camera(&context.queue, &camera);

    let before = cage_pixels(&capture_frame(&context, &renderer, WIDTH, HEIGHT));

    // The topmost solid block, directly under the camera.
    renderer.set_selection(&context.queue, Some(BlockPos::new(0, surface - 1, 0)));
    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "selection-visible.ppm");

    assert_eq!(before, 0, "the terrain already contains cage-coloured pixels");
    assert!(
        cage_pixels(&capture) > 50,
        "the selection outline drew {} matching pixels",
        cage_pixels(&capture)
    );
}

#[test]
fn a_buried_block_still_shows_its_cage() {
    // The regression this guards: with a single depth-tested pass, a block
    // with solid neighbours on every side has no visible edges at all, so you
    // cannot tell which of two stacked blocks you are pointing at. The
    // occluded pass draws the buried remainder faintly.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 1);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = looking_down_at(0, 0, surface as f32 + 3.0);
    renderer.update_camera(&context.queue, &camera);

    // Well below the surface, so every face of it is against solid rock.
    let buried = BlockPos::new(0, surface - 6, 0);
    assert!(world.is_solid(buried), "the test block is not underground");
    assert!(
        world.is_solid(buried.offset([0, 1, 0])) && world.is_solid(buried.offset([1, 0, 0])),
        "the test block is not fully enclosed"
    );

    renderer.set_selection(&context.queue, Some(buried));
    let capture = capture_frame(&context, &renderer, WIDTH, HEIGHT);
    save(&capture, "selection-buried.ppm");

    assert!(
        cage_pixels(&capture) > 20,
        "a buried block drew {} cage pixels; the occluded pass is not drawing",
        cage_pixels(&capture)
    );
}

#[test]
fn the_cage_never_occludes_the_world() {
    // It is depth-tested but must write no depth of its own, and must not
    // paint over sky it does not cover. Comparing sky pixel counts catches
    // both: a depth-writing cage would punch a hole in the terrain behind it.
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 2);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = Camera {
        position: glam::Vec3::new(0.5, surface as f32 + 2.0, 0.5),
        yaw: 0.4,
        pitch: -0.5,
        aspect: WIDTH as f32 / HEIGHT as f32,
        ..Camera::default()
    };
    renderer.update_camera(&context.queue, &camera);

    let sky = sky_rgb();
    let count_sky = |capture: &Capture| {
        capture
            .pixels
            .chunks_exact(4)
            .filter(|texel| texel[0..3] == sky)
            .count()
    };

    let without = count_sky(&capture_frame(&context, &renderer, WIDTH, HEIGHT));
    renderer.set_selection(&context.queue, Some(BlockPos::new(0, surface - 1, 0)));
    let with = count_sky(&capture_frame(&context, &renderer, WIDTH, HEIGHT));

    assert_eq!(with, without, "the cage changed how much sky is visible");
}

#[test]
fn clearing_the_selection_removes_the_cage() {
    let Some(context) = context() else { return };
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, WIDTH, HEIGHT);
    let world = scene(&context, &mut renderer, 1);

    let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
    let camera = looking_down_at(0, 0, surface as f32 + 3.0);
    renderer.update_camera(&context.queue, &camera);

    renderer.set_selection(&context.queue, Some(BlockPos::new(0, surface - 1, 0)));
    assert!(renderer.selection().is_some());
    let shown = cage_pixels(&capture_frame(&context, &renderer, WIDTH, HEIGHT));

    renderer.set_selection(&context.queue, None);
    let cleared = cage_pixels(&capture_frame(&context, &renderer, WIDTH, HEIGHT));

    assert!(shown > 0);
    assert_eq!(cleared, 0, "the outline survived being cleared");
    assert_eq!(renderer.selection(), None);
}
