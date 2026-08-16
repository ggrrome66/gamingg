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
mod hud;
mod interaction;
mod streaming;

use std::sync::Arc;
use std::time::{Duration, Instant};

use controller::FlyController;
use hud::{HudState, MenuAction, Menus, Screen, SimStats, Target};
use interaction::{hotbar_slot, Action, Hotbar};
use streaming::{chunk_at, ChunkStreamer, StreamingConfig};

use vx_platform::InputState;
use vx_render::headless::{capture_frame, CAPTURE_FORMAT};
use vx_render::{Camera, GpuContext, Renderer, WindowSurface};
use vx_save::WorldStore;
use vx_world::{TickClock, World, TICKS_PER_SECOND};

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
    /// Which UI to draw in a screenshot. Lets the menus be captured headlessly
    /// rather than only ever being seen by someone sitting at a window.
    ui: UiMode,
    /// World to load and save. `None` plays without touching disk, which is
    /// what screenshots and smoke tests want.
    world: Option<String>,
}

/// UI selection for `--screenshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    None,
    Hud,
    Menu(Screen),
}

impl UiMode {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "none" => Ok(UiMode::None),
            "hud" => Ok(UiMode::Hud),
            "main" => Ok(UiMode::Menu(Screen::Main)),
            "controls" => Ok(UiMode::Menu(Screen::Controls)),
            "world" => Ok(UiMode::Menu(Screen::World)),
            other => Err(format!(
                "unknown --ui value '{other}' (none, hud, main, controls, world)"
            )),
        }
    }
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        seed: DEFAULT_SEED,
        screenshot: None,
        width: 1280,
        height: 720,
        ui: UiMode::Hud,
        world: None,
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
            "--ui" => options.ui = UiMode::parse(&value()?)?,
            "--world" => {
                let name = value()?;
                if !vx_save::is_safe_world_name(&name) {
                    return Err(format!(
                        "--world {name:?} is not a usable world name (letters, digits, \
                         spaces, dashes, underscores and dots; no separators)"
                    ));
                }
                options.world = Some(name);
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
                     --world <name>      world to load and save under the saves\n                       \
                                         directory; omit to play without saving\n  \
                     --seed <n>          seed for a new world (default {DEFAULT_SEED});\n                       \
                                         an existing world keeps its own\n  \
                     --screenshot <path> render one frame to a PPM file and exit\n  \
                     --ui <mode>         UI to draw in a screenshot: none, hud,\n                       \
                                         main, controls, world (default hud)\n  \
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
        Some(path) => {
            if options.world.is_some() {
                // Better to say so than to let the flag look like it worked.
                eprintln!("warning: --world is ignored with --screenshot, which never touches disk");
            }
            run_screenshot(&options, path)
        }
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
) -> (World, Camera) {
    let mut world = World::new(seed);
    let mut streamer = ChunkStreamer::new(StreamingConfig {
        render_distance: radius,
        // No frame to keep smooth here, so do all the work at once.
        generate_budget: usize::MAX,
        mesh_budget: usize::MAX,
    });

    let centre = vx_core::ChunkPos::new(0, 0);
    // Loading is left to the streamer rather than done up front: it is what
    // lights each chunk as it arrives, and pre-loading would make it skip them
    // as already resident and mesh unlit terrain.
    // Screenshots never touch disk, so no store.
    streamer.update(&mut world, renderer, &context.device, centre, None);

    let surface = world.surface_y(0, 0).unwrap_or(80);
    // Standing height rather than a bird's-eye view: close enough that the
    // crosshair is actually on a block, so screenshots show the selection
    // outline and target readout doing their job.
    // `surface_y` is already one block clear of the ground, so this eye height
    // sits 2.4 blocks above it. At this pitch the crosshair lands about 4.6
    // blocks along the ray — inside the 6-block reach, so the outline and the
    // target readout both appear.
    let camera = Camera {
        position: glam::Vec3::new(0.5, surface as f32 + 1.4, 0.5),
        pitch: -0.55,
        yaw: 0.6,
        ..Camera::default()
    };
    (world, camera)
}

/// Assemble one frame's overlay.
///
/// Scanlines go on last so they lie over the world, the HUD and the menus
/// alike — a CRT does not selectively skip the interesting parts.
fn build_overlay(
    renderer: &Renderer,
    menus: &Menus,
    state: &HudState,
) -> vx_render::OverlayBuilder {
    let mut ui = renderer.overlay_builder();

    if !menus.is_open() {
        hud::draw_crosshair(&mut ui);
        hud::draw_hud(&mut ui, state);
    }
    hud::draw_menu(&mut ui, menus, state);
    hud::draw_scanlines(&mut ui);

    ui
}

/// Describe what the camera is looking at, for the HUD.
fn describe_target(world: &World, camera: &Camera) -> Option<Target> {
    let hit = interaction::target(world, camera)?;
    let block = world.block(hit.block);
    Some(Target {
        position: hit.block,
        name: world
            .registry()
            .get(block)
            .map_or("UNKNOWN", |def| def.display_name.as_str())
            .to_string(),
    })
}

fn run_screenshot(options: &Options, path: &str) -> Result<(), String> {
    let context = GpuContext::headless_blocking()
        .map_err(|error| format!("no graphics device for offscreen rendering: {error}"))?;

    let (width, height) = (options.width, options.height);
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, width, height);

    let (world, mut camera) = build_scene(&context, &mut renderer, options.seed, 6);
    camera.aspect = width as f32 / height as f32;
    renderer.update_camera(&context.queue, &camera);

    let hotbar = Hotbar::from_registry(world.registry());
    let target = describe_target(&world, &camera);
    renderer.set_selection(&context.queue, target.as_ref().map(|t| t.position));

    let mut menus = Menus::default();
    if let UiMode::Menu(screen) = options.ui {
        menus.open();
        menus.set_screen(screen);
    }

    if options.ui != UiMode::None {
        let state = HudState {
            fps: 60.0,
            camera: camera.position,
            chunks_loaded: renderer.loaded_chunk_count(),
            chunks_meshed: renderer.loaded_chunk_count(),
            triangles: renderer.triangle_count(),
            seed: world.seed(),
            hotbar: hotbar.names(world.registry()),
            selected_slot: hotbar.selected_slot(),
            target,
            sim: SimStats::default(),
        };
        let ui = build_overlay(&renderer, &menus, &state);
        renderer.set_overlay(&context.device, &context.queue, &ui);
    }

    let capture = capture_frame(&context, &renderer, width, height);
    capture
        .write_ppm(path)
        .map_err(|error| format!("could not write {path}: {error}"))?;

    println!(
        "wrote {path} ({width}x{height}, {} chunks, {} triangles)",
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
    hotbar: Hotbar,
    menus: Menus,
    /// `None` when playing without a world on disk.
    store: Option<WorldStore>,
}

/// How often modified chunks are written out while playing.
///
/// Staging on unload is not enough on its own: a chunk the player is standing
/// in never unloads, so without a timer a crash would cost the whole session's
/// building.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

struct App {
    seed: u64,
    width: u32,
    height: u32,
    /// World name to load and save, or `None` to play without touching disk.
    world: Option<String>,
    active: Option<Active>,
    controller: FlyController,
    input: InputState,
    last_frame: Instant,
    // Frame timing for the HUD readout.
    frames: u32,
    last_report: Instant,
    /// Smoothed over the last reporting interval, so the HUD does not flicker
    /// through a new number every frame.
    last_fps: f32,
    last_save: Instant,
    /// Drives simulation at a fixed rate, independent of frame rate.
    clock: TickClock,
}

impl App {
    fn new(seed: u64, width: u32, height: u32, world: Option<String>) -> Self {
        App {
            seed,
            width,
            height,
            world,
            active: None,
            controller: FlyController::default(),
            input: InputState::new(),
            last_frame: Instant::now(),
            frames: 0,
            last_report: Instant::now(),
            last_fps: 0.0,
            last_save: Instant::now(),
            clock: TickClock::new(TICKS_PER_SECOND),
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

    fn menu_is_open(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.menus.is_open())
    }

    /// Drive the menu from the keyboard. Only reached while one is open, so
    /// none of these keys leak through to the world.
    fn menu_key(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) {
        let Some(active) = &mut self.active else { return };

        let action = match code {
            KeyCode::Escape => {
                active.menus.back();
                None
            }
            KeyCode::KeyW | KeyCode::ArrowUp => {
                active.menus.move_selection(-1);
                None
            }
            KeyCode::KeyS | KeyCode::ArrowDown => {
                active.menus.move_selection(1);
                None
            }
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => active.menus.activate(),
            _ => None,
        };

        match action {
            Some(MenuAction::Quit) => {
                self.save();
                event_loop.exit();
            }
            // Held keys would otherwise stay down through the transition and
            // send the camera flying the instant the world resumes.
            Some(MenuAction::Resume) => self.input.clear_keys(),
            None => {}
        }
    }

    /// Stage every modified resident chunk and write out what is pending.
    ///
    /// Chunks that unloaded were already staged on the way out; this covers
    /// the ones still in memory, which includes the one the player is standing
    /// in and has been building in all session.
    fn save(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some(store) = &mut active.store else { return };

        let pending: Vec<vx_core::ChunkPos> = active
            .world
            .modified_chunks()
            .map(|chunk| chunk.pos())
            .collect();

        for pos in &pending {
            let Some(chunk) = active.world.chunk(*pos) else {
                continue;
            };
            if let Err(error) = store.store_chunk(chunk, active.world.registry()) {
                log::error!("could not stage chunk {},{}: {error}", pos.x, pos.z);
            }
        }

        match store.flush() {
            Ok(0) => {}
            Ok(written) => {
                // Only clear the modified flags once the bytes are actually
                // down. Clearing them on staging would lose the edits for good
                // if the write then failed.
                for pos in &pending {
                    active.world.mark_saved(*pos);
                }
                store.evict_clean_regions();
                log::info!("saved {} chunks across {written} regions", pending.len());
            }
            Err(failures) => {
                for error in failures {
                    log::error!("save failed: {error}");
                }
            }
        }

        self.last_save = Instant::now();
    }

    /// Break or place against whatever the player is looking at.
    ///
    /// The edit marks its chunk dirty, so the streamer rebuilds that mesh on
    /// the next frame; nothing here touches the renderer directly.
    fn edit(&mut self, action: Action) {
        let Some(active) = &mut self.active else { return };
        let Some(holding) = active.hotbar.selected() else { return };

        match interaction::apply(&mut active.world, &active.camera, action, holding) {
            Ok(pos) => log::debug!("{action:?} at {pos:?}"),
            // Refusals are ordinary play: clicking at the sky, or at bedrock.
            Err(error) => log::debug!("{action:?} refused: {error}"),
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let frame_started = self.last_frame;
        let dt = (now - frame_started).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Disjoint field borrows: the controller and input are separate fields
        // from `active`, so this is fine and lets the camera be mutated in
        // place rather than through a copy.
        let Some(active) = &mut self.active else { return };

        // A menu freezes the world rather than pausing it: chunks still stream
        // so the view behind the panel stays coherent, but nothing the player
        // does moves the camera.
        if !active.menus.is_open() {
            self.controller.apply(&mut active.camera, &mut self.input, dt);
        } else {
            // Drain motion that arrived while the menu was up, or the view
            // lurches the moment it closes.
            self.input.take_mouse_delta();
        }

        // Fixed-step simulation, decoupled from the frame rate. `advance` caps
        // the catch-up, so a long stall skips time rather than trying to
        // simulate all of it at once and falling further behind each frame.
        let steps = self
            .clock
            .advance(now - frame_started, active.world.limits().max_catchup_steps);
        for _ in 0..steps {
            active.world.tick();
        }

        // Keep chunks in step with where the camera is.
        let centre = chunk_at(active.camera.position);
        active.streamer.update(
            &mut active.world,
            &mut active.renderer,
            &active.context.device,
            centre,
            active.store.as_mut(),
        );

        active
            .renderer
            .update_camera(&active.context.queue, &active.camera);

        // Outline whatever the crosshair is on, and rebuild the overlay.
        let target = describe_target(&active.world, &active.camera);
        active
            .renderer
            .set_selection(&active.context.queue, target.as_ref().map(|t| t.position));

        let state = HudState {
            fps: self.last_fps,
            camera: active.camera.position,
            chunks_loaded: active.renderer.loaded_chunk_count(),
            chunks_meshed: active.streamer.meshed_count(),
            triangles: active.renderer.triangle_count(),
            seed: active.world.seed(),
            hotbar: active.hotbar.names(active.world.registry()),
            selected_slot: active.hotbar.selected_slot(),
            target,
            sim: SimStats {
                pending_ticks: active.world.scheduler().pending(),
                pending_updates: active.world.pending_updates(),
                refused: active.world.scheduler().refused(),
                dropped: active.world.updates_dropped(),
                skipped: self.clock.skipped(),
            },
        };
        let ui = build_overlay(&active.renderer, &active.menus, &state);
        active
            .renderer
            .set_overlay(&active.context.device, &active.context.queue, &ui);

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

        if self.last_save.elapsed() >= AUTOSAVE_INTERVAL {
            self.save();
        }
    }

    fn report_framerate(&mut self) {
        self.frames += 1;
        let elapsed = self.last_report.elapsed();
        if elapsed.as_secs_f32() < 1.0 {
            return;
        }

        self.last_fps = self.frames as f32 / elapsed.as_secs_f32();

        if let Some(active) = &self.active {
            // The readout moved on-screen; the title just names the window and
            // what is held, for anyone reading a taskbar.
            active.window.set_title(&format!(
                "gamingg - {} ({} of {})",
                active.hotbar.selected_name(active.world.registry()),
                active.hotbar.selected_slot() + 1,
                active.hotbar.len(),
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

        let renderer = Renderer::new(&context, surface.format(), size.width, size.height);

        // Open the world before building it, because an existing one supplies
        // its own seed and generating against a different one would seam
        // against whatever is already saved.
        let store = match &self.world {
            Some(name) => match WorldStore::open(
                &vx_platform::paths::saves_dir(),
                name,
                self.seed,
                vx_world::gen::GENERATOR_VERSION,
            )
            {
                Ok(store) => {
                    log::info!("world {:?} at {}", name, store.root().display());
                    Some(store)
                }
                Err(error) => {
                    log::error!("could not open world {name:?}: {error}");
                    event_loop.exit();
                    return;
                }
            },
            None => None,
        };

        let seed = store.as_ref().map_or(self.seed, |store| store.seed());
        let world = World::new(seed);
        let hotbar = Hotbar::from_registry(world.registry());
        let streamer = ChunkStreamer::new(StreamingConfig::default());
        let camera = Camera {
            position: glam::Vec3::new(0.0, 90.0, 0.0),
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
            hotbar,
            menus: Menus::default(),
            store,
        });

        // Drop the player onto the terrain rather than leaving them at a fixed
        // height that might be inside a hill.
        if let Some(active) = &mut self.active {
            let centre = vx_core::ChunkPos::new(0, 0);
            active.world.load_chunk(centre);
            if let Some(surface_y) = active.world.surface_y(0, 0) {
                active.camera.position.y = surface_y as f32 + 2.0;
            }
        }

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
                self.save();
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
                        if self.menu_is_open() {
                            self.menu_key(code, event_loop);
                            return;
                        }
                        if code == KeyCode::Escape {
                            // Escape leaves the world rather than only freeing
                            // the pointer: the menu is where quitting lives.
                            self.set_capture(false);
                            if let Some(active) = &mut self.active {
                                active.menus.open();
                            }
                            return;
                        }
                        if let Some(slot) = hotbar_slot(code) {
                            if let Some(active) = &mut self.active {
                                active.hotbar.select(slot);
                            }
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
                // The menu is keyboard-driven; clicking through it would grab
                // the pointer while a panel is still up.
                if self.menu_is_open() {
                    return;
                }
                // The click that grabs the pointer must not also swing a pick:
                // the player is clicking to enter the window, not to dig.
                if !self.input.mouse_captured {
                    if button == MouseButton::Left {
                        self.set_capture(true);
                    }
                    return;
                }
                match button {
                    MouseButton::Left => self.edit(Action::Break),
                    MouseButton::Right => self.edit(Action::Place),
                    _ => {}
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if self.menu_is_open() {
                    return;
                }
                let scrolled = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    // Trackpads report pixels; only the sign matters here.
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                if scrolled != 0.0 {
                    if let Some(active) = &mut self.active {
                        // Scrolling up should advance through the hotbar.
                        active.hotbar.cycle(if scrolled > 0.0 { 1 } else { -1 });
                    }
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
