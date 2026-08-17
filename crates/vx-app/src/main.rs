//! The game binary.
//!
//! Wires the world, chunk streaming, renderer and window together and runs the
//! frame loop. Two modes:
//!
//! - default: open a window and play.
//! - `--screenshot <path>`: render one frame offscreen and exit. Needs no
//!   display, so it works over SSH, in CI, and against a software Vulkan
//!   driver — and is how the whole stack gets smoke-tested without a GPU.

mod controller;
mod streaming;

use std::sync::Arc;
use std::time::Instant;

use controller::{FlyController, MovementMode, WalkController};
use streaming::{chunk_at, ChunkStreamer, StreamingConfig};

use vx_core::{BlockId, EventBus};
use vx_platform::InputState;
use vx_render::headless::{capture_frame, CAPTURE_FORMAT};
use vx_render::{Camera, GpuContext, Renderer, WindowSurface};
use vx_world::{break_block, place_block, raycast_solid, PlayerBody, World, WorldSave};

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const DEFAULT_SEED: u64 = 2024;

/// Command line options.
struct Options {
    seed: u64,
    screenshot: Option<String>,
    width: u32,
    height: u32,
    /// Save directory name, under the platform data directory.
    world: String,
    /// World position to place the camera for a screenshot.
    at: (i32, i32),
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        seed: DEFAULT_SEED,
        screenshot: None,
        width: 1280,
        height: 720,
        world: "world".to_string(),
        at: (0, 0),
    };

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--seed" => {
                options.seed = value()?
                    .parse()
                    .map_err(|_| "--seed must be a number".to_string())?
            }
            "--screenshot" => options.screenshot = Some(value()?),
            "--world" => options.world = value()?,
            "--at" => {
                let text = value()?;
                let (x, z) = text
                    .split_once(',')
                    .ok_or_else(|| "--at wants X,Z".to_string())?;
                options.at = (
                    x.trim().parse().map_err(|_| "--at X must be a number".to_string())?,
                    z.trim().parse().map_err(|_| "--at Z must be a number".to_string())?,
                );
            }
            "--width" => {
                options.width = value()?
                    .parse()
                    .map_err(|_| "--width must be a number".to_string())?
            }
            "--height" => {
                options.height = value()?
                    .parse()
                    .map_err(|_| "--height must be a number".to_string())?
            }
            "--help" | "-h" => {
                println!(
                    "gamingg\n\n\
                     Options:\n  \
                     --seed <n>          world seed (default {DEFAULT_SEED})\n  \
                     --world <name>      save directory name (default \"world\")\n  \
                     --screenshot <path> render one frame to a PPM file and exit\n  \
                     --at <x,z>          world position to view from (screenshot)\n  \
                     --width <n>         window or image width\n  \
                     --height <n>        window or image height"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    Ok(options)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("error: {message}\ntry --help");
            std::process::exit(2);
        }
    };

    let result = match &options.screenshot {
        Some(path) => run_screenshot(&options, path),
        None => run_windowed(&options),
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

/// Build a world and mesh everything within range, without a window.
fn build_scene(
    context: &GpuContext,
    renderer: &mut Renderer,
    seed: u64,
    radius: i32,
    at: (i32, i32),
) -> (World, Camera) {
    let mut world = World::new(seed);
    let mut streamer = ChunkStreamer::new(StreamingConfig {
        render_distance: radius,
        // No frame to keep smooth here, so do all the work at once.
        generate_budget: usize::MAX,
        mesh_budget: usize::MAX,
    });

    let centre = vx_core::BlockPos::new(at.0, 0, at.1).chunk();
    world.load_around(centre, radius);
    streamer.update(&mut world, renderer, &context.device, centre, None);

    let surface = world.surface_y(at.0, at.1).unwrap_or(80);
    let camera = Camera {
        position: glam::Vec3::new(at.0 as f32, surface as f32 + 10.0, at.1 as f32 + 20.0),
        pitch: -0.35,
        ..Camera::default()
    };
    (world, camera)
}

fn run_screenshot(options: &Options, path: &str) -> Result<(), String> {
    let context = GpuContext::headless_blocking()
        .map_err(|error| format!("no graphics device for offscreen rendering: {error}"))?;

    let (width, height) = (options.width, options.height);
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, width, height);

    let (_world, mut camera) = build_scene(&context, &mut renderer, options.seed, 6, options.at);
    camera.aspect = width as f32 / height as f32;
    renderer.update_camera(&context.queue, &camera);

    let capture = capture_frame(&context, &renderer, width, height);
    capture
        .write_ppm(path)
        .map_err(|error| format!("could not write {path}: {error}"))?;

    println!(
        "wrote {path} ({width}x{height}, {} of {} chunks drawn, {} triangles)",
        renderer.visible_chunk_count(),
        renderer.loaded_chunk_count(),
        renderer.triangle_count()
    );
    Ok(())
}

fn run_windowed(options: &Options) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|error| {
        format!(
            "could not open a window system connection: {error}\n\
             If this machine has no display, render offscreen instead:\n  \
             gamingg --screenshot frame.ppm"
        )
    })?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(
        options.seed,
        options.width,
        options.height,
        options.world.clone(),
    );
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("event loop failed: {error}"))
}

/// Everything that only exists once there is a window.
struct Active {
    window: Arc<Window>,
    context: GpuContext,
    surface: WindowSurface,
    renderer: Renderer,
    world: World,
    streamer: ChunkStreamer,
    camera: Camera,
    /// The physical body. Only drives the camera in walk mode; in fly mode the
    /// camera moves freely and the body is carried along behind it.
    player: PlayerBody,
    mode: MovementMode,
    /// Carries block edits to any listeners. Mods hook in here in M3.
    events: EventBus,
    save: Option<WorldSave>,
    /// Blocks the player can place, and which one is selected.
    palette: Vec<BlockId>,
    selected: usize,
}

struct App {
    seed: u64,
    width: u32,
    height: u32,
    world_name: String,
    active: Option<Active>,
    fly: FlyController,
    walk: WalkController,
    input: InputState,
    last_frame: Instant,
    // Frame timing for the title bar readout.
    frames: u32,
    last_report: Instant,
}

impl App {
    fn new(seed: u64, width: u32, height: u32, world_name: String) -> Self {
        App {
            seed,
            width,
            height,
            world_name,
            active: None,
            fly: FlyController::default(),
            walk: WalkController::default(),
            input: InputState::new(),
            last_frame: Instant::now(),
            frames: 0,
            last_report: Instant::now(),
        }
    }

    /// Grab or release the pointer for mouse-look.
    fn set_capture(&mut self, captured: bool) {
        let Some(active) = &self.active else { return };

        if captured {
            // Locked is unavailable on some X11 setups; Confined is the
            // fallback that still lets the pointer drive the view.
            let grabbed = active
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| active.window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(error) = grabbed {
                log::warn!("could not capture the cursor: {error}");
                return;
            }
        } else if let Err(error) = active.window.set_cursor_grab(CursorGrabMode::None) {
            log::warn!("could not release the cursor: {error}");
        }

        active.window.set_cursor_visible(!captured);
        self.input.mouse_captured = captured;
        // Drop any motion accumulated across the transition.
        self.input.take_mouse_delta();
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Disjoint field borrows: the controllers and input are separate
        // fields from `active`, so this is fine and lets the camera be mutated
        // in place rather than through a copy.
        let Some(active) = &mut self.active else { return };

        match active.mode {
            MovementMode::Fly => {
                self.fly.apply(&mut active.camera, &mut self.input, dt);
                // Keep the body under the camera so switching to walk mode
                // drops the player where they are looking from, not wherever
                // they last stood.
                active.player.position =
                    active.camera.position - glam::Vec3::Y * active.player.eye_height;
                active.player.velocity = glam::Vec3::ZERO;
                active.player.on_ground = false;
            }
            MovementMode::Walk => {
                self.walk.apply(
                    &mut active.camera,
                    &mut active.player,
                    &active.world,
                    &mut self.input,
                    dt,
                );
            }
        }

        // Keep chunks in step with where the camera is.
        let centre = chunk_at(active.camera.position);
        active.streamer.update(
            &mut active.world,
            &mut active.renderer,
            &active.context.device,
            centre,
            active.save.as_ref(),
        );

        active
            .renderer
            .update_camera(&active.context.queue, &active.camera);

        use wgpu::CurrentSurfaceTexture as Acquired;

        let acquired = active.surface.surface.get_current_texture();
        // Suboptimal still hands over a usable texture, so draw this frame and
        // reconfigure afterwards rather than dropping it.
        let reconfigure = matches!(acquired, Acquired::Suboptimal(_));

        match acquired {
            Acquired::Success(frame) | Acquired::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = active.context.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("frame"),
                    },
                );
                active.renderer.render(&mut encoder, &view);
                active.context.queue.submit([encoder.finish()]);
                frame.present();
            }

            // Nothing to draw into this frame; try again next tick.
            Acquired::Timeout | Acquired::Occluded => {}

            // The surface went stale — on resize, or when the compositor
            // reconfigures. Rebuilding it is the documented recovery.
            Acquired::Outdated | Acquired::Lost => {
                let (width, height) = active.surface.size();
                active
                    .surface
                    .surface
                    .configure(&active.context.device, &active.surface.config);
                active.renderer.resize(&active.context, width, height);
            }

            Acquired::Validation => {
                log::error!("surface acquisition raised a validation error");
            }
        }

        if reconfigure {
            active
                .surface
                .surface
                .configure(&active.context.device, &active.surface.config);
        }

        self.report_framerate();
    }

    /// How far the player can reach to break or place, in blocks.
    const REACH: f32 = 6.0;

    /// Break whatever the player is looking at.
    fn break_target(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some(hit) = raycast_solid(
            &active.world,
            active.world.registry(),
            active.camera.position,
            active.camera.forward(),
            Self::REACH,
        ) else {
            return;
        };
        if let Err(error) = break_block(&mut active.world, &active.events, hit.block) {
            log::debug!("could not break {:?}: {error}", hit.block);
        }
    }

    /// Place the selected block against whatever the player is looking at.
    fn place_at_target(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some(&block) = active.palette.get(active.selected) else {
            return;
        };
        let Some(hit) = raycast_solid(
            &active.world,
            active.world.registry(),
            active.camera.position,
            active.camera.forward(),
            Self::REACH,
        ) else {
            return;
        };

        // The player's own box must not be built into, or you seal yourself in.
        let body = active.player.aabb();
        let result = place_block(
            &mut active.world,
            &active.events,
            &hit,
            block,
            |position| body.contains_block(position),
        );
        if let Err(error) = result {
            log::debug!("could not place at {:?}: {error}", hit.placement());
        }
    }

    /// React to a key going down, ignoring auto-repeat.
    fn handle_press(&mut self, code: KeyCode) {
        match code {
            KeyCode::Escape => self.set_capture(false),
            KeyCode::KeyF => {
                if let Some(active) = &mut self.active {
                    active.mode = active.mode.toggled();
                    // Entering walk mode drops the player from wherever the
                    // camera was, so clear any stale fall speed.
                    active.player.velocity = glam::Vec3::ZERO;
                    log::info!("movement mode: {:?}", active.mode);
                }
            }
            KeyCode::F5 => self.save_world(),
            // Number keys pick a block to build with.
            KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 => {
                if let Some(active) = &mut self.active {
                    let slot = match code {
                        KeyCode::Digit1 => 0,
                        KeyCode::Digit2 => 1,
                        KeyCode::Digit3 => 2,
                        _ => 3,
                    };
                    if slot < active.palette.len() {
                        active.selected = slot;
                        let name = &active.world.registry().get_or_air(active.palette[slot]).name;
                        log::info!("selected {name}");
                    }
                }
            }
            _ => {}
        }
    }

    /// Write the world to disk, reporting how it went.
    fn save_world(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some(save) = active.save.as_ref() else {
            return;
        };
        match save.save_world(&mut active.world) {
            Ok(0) => log::info!("nothing to save"),
            Ok(count) => log::info!("saved {count} chunks to {}", save.root().display()),
            Err(error) => log::error!("could not save the world: {error}"),
        }
    }

    fn report_framerate(&mut self) {
        self.frames += 1;
        let elapsed = self.last_report.elapsed();
        if elapsed.as_secs_f32() < 1.0 {
            return;
        }

        if let Some(active) = &self.active {
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            active.window.set_title(&format!(
                "gamingg - {fps:.0} fps - {} - {}/{} chunks drawn - {} tris",
                match active.mode {
                    MovementMode::Fly => "fly",
                    MovementMode::Walk => "walk",
                },
                active.renderer.visible_chunk_count(),
                active.renderer.loaded_chunk_count(),
                active.renderer.triangle_count()
            ));
        }
        self.frames = 0;
        self.last_report = Instant::now();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("gamingg")
            .with_inner_size(winit::dpi::LogicalSize::new(self.width, self.height));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                log::error!("could not create a window: {error}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let (context, surface) =
            match pollster::block_on(GpuContext::for_window(window.clone(), size.width, size.height))
            {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("could not initialise the graphics device: {error}");
                    event_loop.exit();
                    return;
                }
            };

        let mut renderer = Renderer::new(&context, surface.format(), size.width, size.height);

        // Open the save directory. A failure here is not fatal — the world is
        // still playable, it just will not persist — so it is reported and
        // play continues.
        let save_root = vx_platform::paths::saves_dir().join(&self.world_name);
        let save = match WorldSave::create(&save_root) {
            Ok(save) => Some(save),
            Err(error) => {
                log::error!(
                    "could not open {}: {error}; this session will not be saved",
                    save_root.display()
                );
                None
            }
        };

        // An existing world keeps its own seed, so reloading gives back the
        // same terrain regardless of what --seed says.
        let seed = save
            .as_ref()
            .filter(|save| save.exists())
            .and_then(|save| match save.read_meta() {
                Ok(seed) => {
                    log::info!("loading existing world (seed {seed})");
                    Some(seed)
                }
                Err(error) => {
                    log::error!("could not read the world metadata: {error}");
                    None
                }
            })
            .unwrap_or(self.seed);

        let mut world = World::new(seed);
        let streamer = ChunkStreamer::new(StreamingConfig::default());

        // Blocks the player can build with, resolved by name so this survives
        // any future id changes.
        let palette: Vec<BlockId> = [
            "engine:stone",
            "engine:dirt",
            "engine:grass",
            "engine:sand",
        ]
        .iter()
        .filter_map(|name| world.registry().id_of(name))
        .collect();

        // Stand the player on the terrain rather than at a fixed height that
        // might be inside a hill.
        world.load_chunk(vx_core::ChunkPos::new(0, 0));
        let ground = world.surface_y(0, 0).unwrap_or(90);
        let player = PlayerBody {
            position: glam::Vec3::new(0.5, ground as f32, 0.5),
            ..PlayerBody::default()
        };

        let camera = Camera {
            position: player.eye_position(),
            aspect: size.width as f32 / size.height.max(1) as f32,
            ..Camera::default()
        };
        renderer.update_camera(&context.queue, &camera);

        self.active = Some(Active {
            window,
            context,
            surface,
            renderer,
            world,
            streamer,
            camera,
            player,
            mode: MovementMode::Walk,
            events: EventBus::new(),
            save,
            palette,
            selected: 0,
        });

        self.last_frame = Instant::now();
        self.last_report = Instant::now();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                // Save before tearing down, or everything built this session
                // is lost on quit.
                self.save_world();
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(active) = &mut self.active {
                    if active
                        .surface
                        .resize(&active.context.device, size.width, size.height)
                    {
                        active
                            .renderer
                            .resize(&active.context, size.width, size.height);
                        active.camera.aspect =
                            size.width as f32 / size.height.max(1) as f32;
                    }
                }
            }

            WindowEvent::Focused(false) => {
                // Keys held when focus is lost would otherwise stay down.
                self.input.clear_keys();
                self.set_capture(false);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match event.state {
                    ElementState::Pressed => {
                        // Key repeat re-fires this, so anything that toggles
                        // state must ignore repeats or it flickers.
                        if !event.repeat {
                            self.handle_press(code);
                        }
                        self.input.press(code);
                    }
                    ElementState::Released => self.input.release(code),
                }
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                // The first click captures the pointer; once captured, clicks
                // edit the world. Without that, the click that focuses the
                // window would also punch a hole in the terrain.
                if !self.input.mouse_captured {
                    if button == MouseButton::Left {
                        self.set_capture(true);
                    }
                    return;
                }
                match button {
                    MouseButton::Left => self.break_target(),
                    MouseButton::Right => self.place_at_target(),
                    _ => {}
                }
            }

            WindowEvent::RedrawRequested => self.frame(),

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Raw motion, so mouse-look is unaffected by pointer acceleration and
        // does not stop at the screen edge.
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.add_mouse_delta(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(active) = &self.active {
            active.window.request_redraw();
        }
    }
}
