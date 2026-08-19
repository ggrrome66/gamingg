//! The game binary.
//!
//! Wires the world, chunk streaming, renderer and window together and runs the
//! frame loop. Two modes:
//!
//! - default: open a window and play.
//! - `--screenshot <path>`: render one frame offscreen and exit. Needs no
//!   display, so it works over SSH, in CI, and against a software Vulkan
//!   driver — and is how the whole stack gets smoke-tested without a GPU.

mod awareness;
mod beacon;
mod board;
mod clock;
mod controller;
mod device;
mod economy;
mod hud;
mod journal;
mod map;
mod mining;
mod rig;
mod shop;
mod skills;
mod streaming;
mod view;
mod villagers;
mod wallet;

use std::sync::Arc;
use std::time::Instant;

use controller::{FlyController, MovementMode, WalkController};
use map::MapState;
use rig::Rig;
use skills::Skills;
use view::ViewMode;
use clock::TimeOfDay;
use journal::{Command, CommandLog};
use villagers::Villagers;
use wallet::Wallet;

/// Overlay slot assignments: the minimap, the HUD, and the modal panels.
const MAP_SLOT: usize = 0;
const HUD_SLOT: usize = 1;
const SHOP_SLOT: usize = 2;
const DEVICE_SLOT: usize = 3;
const FEED_SLOT: usize = 4;
const BOARD_SLOT: usize = 5;
use mining::Mining;
use streaming::{chunk_at, ChunkStreamer, StreamingConfig};

use vx_core::{BlockId, EventBus};
use vx_platform::InputState;
use vx_render::headless::{capture_frame, CAPTURE_FORMAT};
use vx_render::{Camera, GpuContext, Renderer, WindowSurface};
use vx_agent::{MineMethod, Operation, VoxelAabb};
use vx_world::{break_block, place_block, raycast_solid, PlayerBody, World, WorldSave};

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const DEFAULT_SEED: u64 = 2024;

/// How far a town's mast can hear other towns, in blocks. Sets how far away a
/// posting can send you, and costs nothing to widen — the towns inside it are
/// derived, not loaded.
const RADIO_RANGE: i32 = 2_000;

/// An open beacon console: the town, the work it is broadcasting, and the
/// trade runs it will pay somebody to carry.
type Console = (
    vx_world::town::TownSite,
    Vec<beacon::Posting>,
    Vec<(usize, (i32, i32))>,
);

/// How near a trade load has to pass before it is drawn as a real machine.
const CARAVAN_SIGHT: f32 = 220.0;

/// How high above the ground a load flies.
const CARAVAN_ALTITUDE: f32 = 26.0;

/// Re-run town discovery once the player has moved this far since the last
/// check. The lattice is cheap, but not free, and nobody discovers a town by
/// standing still.
const DISCOVERY_STEP: i32 = 8;

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
    /// Find an ore body near `at`, mine it, and screenshot the result.
    dig: Option<Dig>,
    /// Drone ticks to run before capturing, when digging.
    ticks: u64,
    /// Frame the first drone up close instead of the whole excavation.
    close: bool,
    /// Draw the supply shop panel open over the capture, stocked for show.
    shop: bool,
    /// Draw the beacon console open over the capture, with work posted.
    board: bool,
    /// Replay the saved world's command journal and report the ground it
    /// rebuilds, instead of opening a window.
    replay: bool,
    /// Drones a dispatch puts on the job.
    drones: u32,
    /// Frame the capture over the player's shoulder, body in shot.
    third_person: bool,
    /// Draw the handheld's fleet roster over the capture.
    device: bool,
    /// Hour of the in-game day to light the capture by, 0..1.
    time: f32,
}

/// Which method a `--dig` run should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dig {
    /// Whatever the ranking recommends.
    Proposed,
    Forced(MineMethod),
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        seed: DEFAULT_SEED,
        screenshot: None,
        width: 1280,
        height: 720,
        world: "world".to_string(),
        at: (0, 0),
        dig: None,
        ticks: 20_000,
        close: false,
        shop: false,
        board: false,
        replay: false,
        drones: mining::DEFAULT_CREW,
        third_person: false,
        device: false,
        time: TimeOfDay::START.fraction(),
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
            "--dig" => {
                options.dig = Some(match value()?.as_str() {
                    // `auto` takes whatever the game would propose, which is
                    // the interesting default: it shows the ranking's answer.
                    "auto" => Dig::Proposed,
                    "adit" => Dig::Forced(MineMethod::Adit),
                    "decline" => Dig::Forced(MineMethod::Decline),
                    "pit" => Dig::Forced(MineMethod::Pit),
                    other => {
                        return Err(format!(
                            "--dig wants auto, adit, decline or pit (got {other})"
                        ))
                    }
                });
            }
            "--close" => options.close = true,
            "--shop" => options.shop = true,
            "--board" => options.board = true,
            "--replay" => options.replay = true,
            "--drones" => {
                options.drones = value()?
                    .parse()
                    .map_err(|_| "--drones must be a number".to_string())?
            }
            "--third-person" => options.third_person = true,
            "--device" => options.device = true,
            "--night" => options.time = TimeOfDay::MIDNIGHT.fraction(),
            "--dawn" => options.time = TimeOfDay::DAWN.fraction(),
            "--noon" => options.time = TimeOfDay::NOON.fraction(),
            "--dusk" => options.time = TimeOfDay::DUSK.fraction(),
            "--time" => {
                options.time = value()?
                    .parse()
                    .map_err(|_| "--time wants a fraction of a day, 0..1".to_string())?
            }
            "--ticks" => {
                options.ticks = value()?
                    .parse()
                    .map_err(|_| "--ticks must be a number".to_string())?
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
                     --dig <method>      find ore near --at and mine it before capturing\n  \
                                         (auto, adit, decline or pit)\n  \
                     --close             frame the first drone up close (with --dig)\n  \
                     --shop              draw the supply shop panel over the capture\n  \
                     --board             draw the beacon console over the capture\n  \
                     --drones <n>        drones a dispatch puts on the job (default 3)\n  \
                     --replay            replay the saved world's journal and report the\n  \
                                         ground it rebuilds, then exit\n  \
                     --third-person      frame over the player's shoulder, body in shot\n  \
                     --device            draw the handheld fleet roster over the capture\n  \
                     --time <0..1>       hour of the day to light the capture by\n  \
                     --night             shorthand for --time 0\n  \
                     --dawn --noon --dusk\n  \
                                         the other named hours\n  \
                     --ticks <n>         drone ticks to run when digging\n  \
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

    let result = if options.replay {
        run_replay(&options)
    } else {
        match &options.screenshot {
            Some(path) => run_screenshot(&options, path),
            None => run_windowed(&options),
        }
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

/// Replay a saved world's command journal and report what it rebuilds.
///
/// The determinism oracle, run from the command line. The journal records
/// *orders* — mine that area this way, run this many ticks — so replaying it
/// re-derives every block those orders produced. If the ground it rebuilds
/// disagrees with the ground on disk, something in worldgen, the agents or the
/// editing path has stopped being deterministic, and this says so.
///
/// Needs no GPU and no window, so it is what CI runs.
fn run_replay(options: &Options) -> Result<(), String> {
    let root = vx_platform::paths::saves_dir().join(&options.world);
    let save = WorldSave::new(&root);
    if !save.exists() {
        return Err(format!("no world saved at {}", root.display()));
    }
    let seed = save
        .read_meta()
        .map_err(|error| format!("could not read {}: {error}", root.display()))?;

    let journal = CommandLog::load(&root);
    if journal.is_empty() {
        println!("nothing recorded for {}", root.display());
    }
    println!(
        "replaying {} commands covering ticks {}..{} (seed {seed})",
        journal.len(),
        journal.keyframe_tick,
        journal.tick()
    );

    let mut world = World::new(seed);
    let events = EventBus::new();
    let started = Instant::now();
    journal::replay(&journal, &mut world, &events);
    let rebuilt = vx_world::world_hash(&world);

    println!(
        "rebuilt {} chunks in {:.2}s, hash {rebuilt:#018x}",
        world.loaded_chunk_count(),
        started.elapsed().as_secs_f32()
    );
    // The books are not rebuilt by a replay — the network's reach follows the
    // player, and where the player stood is not in the journal. Reported so
    // nobody mistakes an empty economy for a matching one.
    println!(
        "town books are not covered by replay (hash {:#018x} of an unrun network)",
        economy::Economy::new().books_hash()
    );

    // Only a journal that reaches all the way back to an empty world can be
    // checked against the saved ground; a tail after a keyframe describes only
    // part of it, and saying so is better than comparing the wrong things.
    if journal.keyframe_tick != 0 {
        println!("journal starts from a keyframe at tick {}; nothing to compare against", journal.keyframe_tick);
        return Ok(());
    }

    let mut saved = World::new(seed);
    for pos in world.loaded_chunks().collect::<Vec<_>>() {
        match save.load_chunk(pos, saved.registry()) {
            Ok(Some(chunk)) => saved.insert_chunk(chunk),
            Ok(None) => {
                saved.load_chunk(pos);
            }
            Err(error) => return Err(format!("could not read chunk {pos:?}: {error}")),
        }
    }
    let on_disk = vx_world::world_hash(&saved);

    if on_disk == rebuilt {
        println!("match: the journal rebuilds the saved world exactly");
        Ok(())
    } else {
        Err(format!(
            "DIVERGED: saved world hashes {on_disk:#018x}, the journal rebuilds {rebuilt:#018x}"
        ))
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

/// Mine the nearest body and report what happened.
fn dig_nearby(
    world: &mut World,
    at: (i32, i32),
    choice: Dig,
    ticks: u64,
) -> Result<(Operation, MineMethod, vx_core::BlockPos, VoxelAabb), String> {
    let body = vx_agent::find_body(world, at, 48)
        .ok_or_else(|| format!("no ore outcrop within 48 blocks of {at:?}"))?;

    let plan = match choice {
        Dig::Proposed => vx_agent::propose(world, body, vx_agent::DEFAULT_GRADE),
        Dig::Forced(method) => vx_agent::plan(world, body, vx_agent::DEFAULT_GRADE, method),
    }
    .ok_or_else(|| format!("{choice:?} does not apply to the body at {body:?}"))?;

    let start = vx_agent::settle(world, plan.portal);
    let mut operation = Operation::new(start);
    operation.add_drone(start);
    operation.post_plan(&plan);

    let events = EventBus::new();
    let (outcome, used) = operation.run(world, &events, ticks);
    println!(
        "{}: {outcome:?} after {used} ticks, {} blocks hauled ({} ore)",
        plan.method.name(),
        operation.stockpile.total(),
        operation.stockpile.count("engine:copper_ore"),
    );
    let workings = plan
        .access
        .iter()
        .chain(&plan.extraction)
        .copied()
        .reduce(VoxelAabb::union)
        .unwrap_or(body);
    Ok((operation, plan.method, start, workings))
}

fn run_screenshot(options: &Options, path: &str) -> Result<(), String> {
    let context = GpuContext::headless_blocking()
        .map_err(|error| format!("no graphics device for offscreen rendering: {error}"))?;

    let (width, height) = (options.width, options.height);
    let mut renderer = Renderer::new(&context, CAPTURE_FORMAT, width, height);

    let (mut world, mut camera) = build_scene(&context, &mut renderer, options.seed, 6, options.at);
    camera.aspect = width as f32 / height as f32;

    // Optionally run a whole excavation before capturing, so the frame shows a
    // real mine on real generated terrain rather than a hand-built fixture.
    if let Some(choice) = options.dig {
        let (operation, _, portal, workings) =
            dig_nearby(&mut world, options.at, choice, options.ticks)?;

        // Frame the shot: the whole excavation from a stand-off, or — with
        // `--close` — the first drone filling the frame, drill and all.
        if options.close {
            let drone = operation
                .drones
                .first()
                .ok_or("no drone to frame up close")?;
            let subject = glam::Vec3::new(
                drone.position.x as f32 + 0.5,
                drone.position.y as f32 + 0.4,
                drone.position.z as f32 + 0.5,
            );
            // Stand toward the portal — the one direction guaranteed open,
            // because the drone drove in through it.
            let doorway = glam::Vec3::new(
                portal.x as f32 + 0.5 - subject.x,
                0.0,
                portal.z as f32 + 0.5 - subject.z,
            );
            let out = if doorway.length() > 1.0 {
                doorway.normalize()
            } else {
                glam::Vec3::new(0.7, 0.0, 0.7).normalize()
            };
            camera.position = subject + out * 3.2 + glam::Vec3::Y * 1.2;
            look_at(&mut camera, subject);
        } else {
            let centre = glam::Vec3::new(
                workings.centre().x as f32,
                workings.max.y as f32,
                workings.centre().z as f32,
            );
            let span = workings.size();
            let stand_off = (span[0].max(span[2]) as f32 * 1.4).max(24.0);
            camera.position = centre
                + glam::Vec3::new(stand_off * 0.7, stand_off * 0.8, stand_off * 0.7);
            look_at(&mut camera, centre);
        }
        let _ = portal;

        // Sweep the sector too, so the capture shows the whole loop: the
        // workings, the drone, the flier and the pings it found.
        let mut fleet = vx_agent::Fleet::new();
        fleet.add_flier(vx_core::BlockPos::new(portal.x, portal.y + 12, portal.z));
        let sector = vx_agent::Sector::containing(options.at.0, options.at.1);
        fleet.dispatch_scan(sector);
        let mut scan_ticks = 0u32;
        while !fleet.is_surveyed(sector) {
            fleet.tick(&world, &mut []);
            scan_ticks += 1;
            if scan_ticks > 20_000 {
                return Err("the survey sweep never finished".into());
            }
        }
        let pings = fleet.pings();
        println!(
            "scan: {} pings over sector ({}, {}) in {scan_ticks} ticks",
            pings.len(),
            sector.x,
            sector.z
        );

        remesh_all(&context, &mut renderer, &mut world);
        renderer.update_camera(&context.queue, &camera);

        // The machines wear their rigs, mid-dig: drills rolled to a visible
        // angle, noses yawed toward the workings so the shot reads as work.
        let digger = Rig::digger();
        let mut objects: Vec<vx_render::Object> = Vec::new();
        for drone in &operation.drones {
            let position = glam::Vec3::new(
                drone.position.x as f32 + 0.5,
                drone.position.y as f32,
                drone.position.z as f32 + 0.5,
            );
            let toward = workings.centre();
            let yaw = rig::yaw_towards(
                toward.x as f32 + 0.5 - position.x,
                toward.z as f32 + 0.5 - position.z,
            )
            .unwrap_or(0.0);
            objects.extend(digger.objects(position, yaw, 0.9));
        }
        let flier_rig = Rig::flier();
        for flier in &fleet.fliers {
            let position = glam::Vec3::new(
                flier.position.x as f32 + 0.5,
                flier.position.y as f32,
                flier.position.z as f32 + 0.5,
            );
            objects.extend(flier_rig.objects(position, 0.0, 2.1));
        }
        for ping in &pings {
            let centre = glam::Vec3::new(
                ping.position.x as f32 + 0.5,
                ping.position.y as f32 + 1.0,
                ping.position.z as f32 + 0.5,
            );
            objects.push(vx_render::Object::box_between(
                centre - glam::Vec3::splat(0.25),
                centre + glam::Vec3::splat(0.25),
                vx_render::tiles::slot::COPPER_ORE,
            ));
        }
        // The player's handheld drill rides the camera, exactly as in play.
        let camera_forward = camera.forward();
        let camera_right = camera.right();
        let drill_position =
            camera.position + camera_forward * 0.85 + camera_right * 0.42 - glam::Vec3::Y * 0.38;
        let drill_yaw = rig::yaw_towards(camera_forward.x, camera_forward.z).unwrap_or(0.0);
        objects.extend(Rig::hand_drill().objects_pitched(
            drill_position,
            drill_yaw,
            camera.pitch,
            0.7,
        ));

        renderer.set_objects(&context.device, &context.queue, &objects);

        // The HUD panel, with a few levels earned so the capture shows the
        // skill sheet doing its job.
        let mut hud_skills = Skills::new();
        hud_skills.add_xp(skills::MINING, 4_000);
        hud_skills.add_xp(skills::PROSPECTING, 1_200);
        hud_skills.add_xp(skills::LOGISTICS, 600);
        hud_skills.add_xp(skills::MINING, 40);
        let hud_pixels = hud::render_hud(&hud::HudContent {
            skills: &hud_skills,
            time: TimeOfDay::new(options.time),
            status: Some(format!("MINING {} JOBS LEFT", operation.board.len())),
            drilling: Some(0.64),
            level_up: None,
            greeting: None,
            reconnecting: false,
        });
        renderer.set_overlay(
            HUD_SLOT,
            &context.device,
            &context.queue,
            (hud::HUD_WIDTH, hud::HUD_HEIGHT),
            &hud_pixels,
            vx_render::OverlayRect {
                x: 10.0,
                y: height as f32 - hud::HUD_HEIGHT as f32 * hud::HUD_SCALE - 10.0,
                width: hud::HUD_WIDTH as f32 * hud::HUD_SCALE,
                height: hud::HUD_HEIGHT as f32 * hud::HUD_SCALE,
            },
        );

        // And the minimap in the corner, exactly as the game draws it: every
        // loaded chunk counts as explored, the surveyed sector shades in, and
        // the dots are the drone, the flier and the pings.
        let mut map_state = MapState::new();
        for chunk in world.loaded_chunks().collect::<Vec<_>>() {
            map_state.explore(chunk);
        }
        let mut markers = Vec::new();
        for ping in &pings {
            markers.push(map::Marker {
                x: ping.position.x,
                z: ping.position.z,
                colour: map::colour::PING,
                radius: 2,
            });
        }
        for drone in &operation.drones {
            markers.push(map::Marker {
                x: drone.position.x,
                z: drone.position.z,
                colour: map::colour::DRONE,
                radius: 1,
            });
        }
        for flier in &fleet.fliers {
            markers.push(map::Marker {
                x: flier.position.x,
                z: flier.position.z,
                colour: map::colour::FLIER,
                radius: 1,
            });
        }
        let map_pixels = map::render_map(&world, &map_state, options.at, &markers);
        let margin = 10.0;
        let panel = map::MAP_SIZE as f32;
        renderer.set_overlay(
            MAP_SLOT,
            &context.device,
            &context.queue,
            (map::MAP_SIZE, map::MAP_SIZE),
            &map_pixels,
            vx_render::OverlayRect {
                x: width as f32 - panel - margin,
                y: margin,
                width: panel,
                height: panel,
            },
        );
    } else {
        renderer.update_camera(&context.queue, &camera);
        // The townsfolk, some way into their stroll, so a village capture
        // shows a town with life in it rather than an empty film set.
        let mut town = Villagers::new();
        // Nobody about to notice in a still capture, so the town simply
        // strolls: `Surroundings::empty()` is "clear air, and alone".
        let alone = awareness::Surroundings::empty();
        for _ in 0..900 {
            town.update(1.0 / 60.0, TimeOfDay::new(options.time), &alone);
        }
        let rigs = Villagers::rigs();
        renderer.set_objects(&context.device, &context.queue, &town.objects(&rigs));
    }

    if options.third_person {
        // Step the camera back over the shoulder and put a body in the shot,
        // exactly as the live view does it.
        let pivot = camera.position;
        let placed = view::camera_placement(&world, &camera, pivot, view::ViewMode::ThirdPerson);
        let body = pivot - glam::Vec3::Y * 1.62;
        let forward = camera.forward();
        let yaw = rig::yaw_towards(forward.x, forward.z).unwrap_or(0.0);
        let mut objects = Rig::player().objects(body, yaw, 0.0);
        // Keep the townsfolk that the village capture already drew.
        let mut town = Villagers::new();
        let alone = awareness::Surroundings::empty();
        for _ in 0..900 {
            town.update(1.0 / 60.0, TimeOfDay::new(options.time), &alone);
        }
        objects.extend(town.objects(&Villagers::rigs()));
        camera.position = placed;
        renderer.update_camera(&context.queue, &camera);
        renderer.set_objects(&context.device, &context.queue, &objects);
    }

    if options.device {
        // The handheld's roster, over whatever the frame already shows.
        let mut mining = Mining::default();
        let ground = world.surface_y(options.at.0, options.at.1).unwrap_or(80);
        mining.ensure_flier(glam::Vec3::new(
            options.at.0 as f32,
            ground as f32,
            options.at.1 as f32,
        ));
        let mut handheld = device::Device::new();
        handheld.open_list();
        handheld.feedback = Some("CONTROL TAKEN".into());
        let roster = mining.roster(camera.position);
        let pixels = device::render_device(&handheld, &roster);
        let panel_width = device::DEVICE_WIDTH as f32 * device::DEVICE_SCALE;
        let panel_height = device::DEVICE_HEIGHT as f32 * device::DEVICE_SCALE;
        renderer.set_overlay(
            DEVICE_SLOT,
            &context.device,
            &context.queue,
            (device::DEVICE_WIDTH, device::DEVICE_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );

        // And the banner that rides a live feed.
        if let Some(listing) = roster.first() {
            let banner = device::render_feed_banner(listing, true, 0.72);
            let banner_width = device::BANNER_WIDTH as f32 * device::BANNER_SCALE;
            let banner_height = device::BANNER_HEIGHT as f32 * device::BANNER_SCALE;
            renderer.set_overlay(
                FEED_SLOT,
                &context.device,
                &context.queue,
                (device::BANNER_WIDTH, device::BANNER_HEIGHT),
                &banner,
                vx_render::OverlayRect {
                    x: (width as f32 - banner_width) / 2.0,
                    y: 12.0,
                    width: banner_width,
                    height: banner_height,
                },
            );
        }
    }

    if options.shop {
        // The shop panel over the frame, stocked the way a good session
        // leaves it — this is the capture that eyeballs the trade UI.
        let mut pile = vx_agent::Stockpile::new();
        pile.add("engine:copper_ore", 240);
        pile.add("engine:log", 17);
        let mut walletbook = wallet::Wallet::new();
        walletbook.earn(350);
        let mut panel = shop::Shop::new();
        panel.open_at_counter();
        panel.feedback = Some("SOLD 60 COPPER ORE FOR 480 CR".into());
        let here = vx_world::town::home_site();
        let market = economy::Economy::new()
            .market(&here, 0)
            .clone();
        let pixels = shop::render_shop(&panel, Some(&pile), &walletbook, &here, &market);
        let panel_width = shop::SHOP_WIDTH as f32 * shop::SHOP_SCALE;
        let panel_height = shop::SHOP_HEIGHT as f32 * shop::SHOP_SCALE;
        renderer.set_overlay(
            SHOP_SLOT,
            &context.device,
            &context.queue,
            (shop::SHOP_WIDTH, shop::SHOP_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    if options.board {
        // A console at the hometown mast, one contract already taken so both
        // states show: work on offer, and work in hand pointing somewhere the
        // player has never been.
        let here = vx_world::town::home_site();
        let neighbours = world.generator().towns_near(here.centre, RADIO_RANGE);
        let postings = beacon::postings_for(&here, &neighbours);
        let mut ledger = beacon::Ledger::new();
        ledger.visit(here.centre);
        if let Some(first) = postings.first() {
            ledger.accept(first);
        }
        let mut walletbook = Wallet::new();
        walletbook.earn(1_240);
        let mut panel = board::Board::new();
        panel.open_at_beacon();
        let market = economy::Economy::new().market(&here, 0).clone();
        let pixels =
            board::render_board(&panel, &here, &postings, &ledger, &walletbook, &market, &[]);
        let panel_width = board::BOARD_WIDTH as f32 * board::BOARD_SCALE;
        let panel_height = board::BOARD_HEIGHT as f32 * board::BOARD_SCALE;
        renderer.set_overlay(
            BOARD_SLOT,
            &context.device,
            &context.queue,
            (board::BOARD_WIDTH, board::BOARD_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    // The capture is lit by an explicit hour, never by a wall clock — which
    // is what keeps two runs of the same command byte-identical.
    renderer.set_sun(
        &context.queue,
        clock::sun_uniform(clock::sky_at(TimeOfDay::new(options.time))),
    );

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

/// Point `camera` at `target` from where it stands.
///
/// Derived from the same yaw/pitch convention `Camera::forward` uses, so a
/// framing that looks right here looks the same in the window.
fn look_at(camera: &mut Camera, target: glam::Vec3) {
    let to = target - camera.position;
    camera.yaw = to.x.atan2(-to.z);
    camera.pitch = to.y.atan2((to.x * to.x + to.z * to.z).sqrt());
    camera.clamp_pitch();
}

/// Rebuild every loaded chunk's mesh. Used after a headless excavation, where
/// there is no streamer running to pick up the dirty chunks.
fn remesh_all(context: &GpuContext, renderer: &mut Renderer, world: &mut World) {
    let loaded: Vec<vx_core::ChunkPos> = world.loaded_chunks().collect();
    for pos in loaded {
        let origin = pos.origin();
        let mesh = vx_mesh::build_mesh(world, world.registry(), [origin.x, 0, origin.z]);
        renderer.set_chunk_mesh(&context.device, pos, &mesh);
        world.clear_dirty(pos);
    }
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
        options.drones,
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
    /// Marking a body and running a drone on it.
    mining: Mining,
    /// The fog-of-war minimap.
    map: MapState,
    /// The player's skill sheet.
    skills: Skills,
    /// The block being drilled and how far through it the bit has got.
    digging: Option<(vx_core::BlockPos, f32)>,
    /// A recent level-up, shown on the HUD for a moment.
    level_up: Option<(String, u32, Instant)>,
    /// The hour of the in-game day.
    clock: TimeOfDay,
    /// Whose eyes the player is looking through.
    view: ViewMode,
    /// The handheld, and whatever feed it has open.
    device: device::Device,
    /// The player's own look, parked while they stare at the handheld.
    body_yaw: f32,
    body_pitch: f32,
    /// True between hanging up and the player's own ground arriving back.
    resuming: bool,
    /// The player's own body, drawn in third person.
    player_rig: Rig,
    /// The handheld drill's shape, built once.
    hand_rig: Rig,
    /// The body a trade load borrows when one passes close enough to see.
    trade_rig: Rig,
    /// The viewmodel drill's accumulated rotation.
    drill_spin: f32,
    /// The town's inhabitants.
    villagers: Villagers,
    /// Their bodies, built once per variant.
    villager_rigs: Vec<Rig>,
    /// A villager's freshly spoken line, shown on the HUD for a moment.
    greeting: Option<(String, Instant)>,
    /// Credits and bought upgrades.
    wallet: Wallet,
    /// The supply shop's panel state.
    shop: shop::Shop,
    /// The beacon console's panel state.
    board: board::Board,
    /// Contracts taken, settled, and towns actually stood in.
    ledger: beacon::Ledger,
    /// Every order given since the last keyframe, and the clock it is kept in.
    journal: CommandLog,
    /// Town markets: what each place holds, makes and charges.
    economy: economy::Economy,
    /// The town whose counter is open. `TownSite` is `Copy`, so this can be
    /// taken out and handed to the shop beside a mutable borrow of the books.
    counter: Option<vx_world::town::TownSite>,
    /// The town whose beacon is open, and the work it is broadcasting.
    /// Derived when the console opens, not stored in the world.
    console: Option<Console>,
    /// The column discovery last ran at.
    last_scan: (i32, i32),
    /// The dispatch window the trade network was last run for.
    last_network: u64,
}

struct App {
    seed: u64,
    width: u32,
    height: u32,
    world_name: String,
    /// Drones a dispatch puts on the job.
    crew: u32,
    active: Option<Active>,
    fly: FlyController,
    walk: WalkController,
    input: InputState,
    /// The left button is held: the drill is running.
    drill_held: bool,
    last_frame: Instant,
    // Frame timing for the title bar readout.
    frames: u32,
    last_report: Instant,
}

impl App {
    fn new(seed: u64, width: u32, height: u32, world_name: String, crew: u32) -> Self {
        App {
            seed,
            width,
            height,
            world_name,
            crew,
            active: None,
            fly: FlyController::default(),
            drill_held: false,
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

        // Trading suspends walking and drilling: the shop owns the input
        // while it is open, and drifting away mid-deal would be silly.
        // The day rolls on. The dt above is already clamped, so a stalled
        // frame cannot lurch the clock forward by an hour.
        active.clock = active.clock.advance(dt);

        // The body stands still whenever the player's attention is elsewhere:
        // haggling at a counter, or staring at a handheld. For the feed that
        // is also the fiction — you are standing there looking at a screen.
        let feed = active.device.feed();
        let busy = active.shop.open || active.board.open || active.device.open || feed.is_some();
        if busy || active.resuming {
            active.player.velocity = glam::Vec3::ZERO;
        }
        let trading = busy;

        match active.mode {
            MovementMode::Fly if trading => {}
            MovementMode::Walk if trading => {}
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
            // Just back from a feed: the ground under the body may still be
            // streaming in. Stepping physics now drops it through the world.
            MovementMode::Walk if active.resuming => {}
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

        // One owner for where the camera physically sits. The controllers set
        // orientation (and, walking, the body); this decides first person
        // against over-the-shoulder, and pulls in past anything solid.
        let pivot = match active.mode {
            MovementMode::Fly => active.camera.position,
            MovementMode::Walk => active.player.eye_position(),
        };
        active.camera.position =
            view::camera_placement(&active.world, &active.camera, pivot, active.view);

        // A live feed steers the machine and rides its camera. Mouse-look
        // belongs entirely to the drone while it is up; the body's own view is
        // parked and handed back on hang-up.
        if let Some(machine) = feed {
            controller::apply_mouse_look(&mut active.camera, &mut self.input, self.walk.sensitivity);
            // The simulation is the authority on who has the wheel: the
            // handheld can ask, but only `Mining` grants it.
            let command = if active.mining.piloted() == Some(machine) {
                Self::pilot_command(&active.camera, &self.input, self.drill_held)
            } else {
                vx_agent::PilotCommand::default()
            };
            active.mining.set_pilot_command(command);
            active.mining.set_pilot_look(active.camera.yaw);
            if let Some(eye) = active.mining.machine_eye(machine) {
                active.camera.position = eye;
            }
        }


        // Once the body's own ground is back, gravity may have it again.
        if active.resuming && self.body_chunk_ready() {
            if let Some(active) = &mut self.active {
                active.resuming = false;
            }
        }

        // The player's drill runs before streaming too, for the same reason
        // the drones dig first: edits land in this frame's remesh.
        if !trading {
            self.update_drilling(dt);
        }
        let Some(active) = &mut self.active else { return };

        // The townsfolk take their stroll, notice what is about, and one of
        // them may say hello. Machines count as things worth watching, so a
        // villager will turn to follow a drone trundling past.
        let machines: Vec<(awareness::TargetKind, glam::Vec3)> = active
            .mining
            .drone_positions()
            .into_iter()
            .map(|at| {
                (
                    awareness::TargetKind::Digger,
                    glam::Vec3::new(at.x as f32 + 0.5, at.y as f32, at.z as f32 + 0.5),
                )
            })
            .chain(active.mining.fleet.fliers.iter().map(|flier| {
                (
                    awareness::TargetKind::Flier,
                    glam::Vec3::new(
                        flier.position.x as f32 + 0.5,
                        flier.position.y as f32,
                        flier.position.z as f32 + 0.5,
                    ),
                )
            }))
            .collect();
        let around = awareness::Surroundings {
            world: Some(&active.world),
            player: Some(active.player.position),
            machines: &machines,
        };
        active.villagers.update(dt, active.clock, &around);
        if let Some(line) = active.villagers.greeting_for() {
            active.greeting = Some((line.to_string(), Instant::now()));
        }

        // Drones dig before streaming, so the chunks their edits dirty get
        // re-meshed in the same frame rather than the next one.
        let report = active.mining.update(
            &mut active.world,
            &active.events,
            std::time::Duration::from_secs_f32(dt),
        );
        // Ticks, not seconds. How many ticks a frame is worth depends on frame
        // rate and stalls; how the world evolves across them does not.
        active
            .journal
            .record(Command::Advance {
                ticks: active.mining.last_ticks(),
            });

        // The fleet's work becomes the player's experience.
        if report.sectors_completed > 0 || report.pings_found > 0 {
            let xp = u64::from(report.sectors_completed) * skills::SWEEP_XP
                + u64::from(report.pings_found) * skills::PING_XP;
            if let Some(level) = active.skills.add_xp(skills::PROSPECTING, xp) {
                active.level_up = Some((skills::PROSPECTING.to_string(), level, Instant::now()));
            }
        }
        if report.delivered > 0 {
            let xp = report.delivered * skills::DELIVERY_XP;
            if let Some(level) = active.skills.add_xp(skills::LOGISTICS, xp) {
                active.level_up = Some((skills::LOGISTICS.to_string(), level, Instant::now()));
            }
        }
        // Levels feed straight back into the machines' stats, and bought
        // cargo upgrades multiply on top of what Logistics earned.
        let cargo_level = active.wallet.upgrade(wallet::CARGO);
        active.mining.apply_skills(
            skills::scan_depth(active.skills.level(skills::PROSPECTING)),
            wallet::boosted_capacity(
                skills::capacity(
                    vx_agent::DEFAULT_CAPACITY,
                    active.skills.level(skills::LOGISTICS),
                ),
                cargo_level,
            ),
            wallet::boosted_capacity(
                skills::capacity(
                    vx_agent::DEFAULT_FLIER_CAPACITY,
                    active.skills.level(skills::LOGISTICS),
                ),
                cargo_level,
            ),
        );

        // Keep chunks in step with where the camera is — which, on a feed, is
        // the machine. That is what lets you drive a drone across the map and
        // actually see where it goes.
        let centre = chunk_at(active.camera.position);
        active.streamer.update(
            &mut active.world,
            &mut active.renderer,
            &active.context.device,
            centre,
            active.save.as_ref(),
        );

        // Ground near the player is explored by being there; swept sectors
        // are explored from the air — half the reason to send the flier.
        // Exploration is earned by being somewhere yourself. A drone's camera
        // is not the player, and painting the map by remote would take the
        // scouting job away from the flier, whose swept sectors still count.
        if feed.is_none() {
            active.map.explore_around(centre, 8);
        }
        for sector in active.mining.fleet.surveyed_sectors().collect::<Vec<_>>() {
            for chunk in map::sector_chunks(sector) {
                active.map.explore(chunk);
            }
        }

        // Walking near a town finds it, and finding it is permanent: the
        // ledger is the only thing that says a town exists as far as the map
        // is concerned. Like exploration, it is earned in person — a drone
        // wandering into a settlement does not put it on your map.
        if feed.is_none() {
            let column = (
                active.player.position.x.floor() as i32,
                active.player.position.z.floor() as i32,
            );
            let moved = (column.0 - active.last_scan.0).saturating_abs()
                + (column.1 - active.last_scan.1).saturating_abs();
            if moved >= DISCOVERY_STEP {
                active.last_scan = column;
                let near = active
                    .world
                    .generator()
                    .towns_near(column, beacon::DISCOVERY_RANGE);
                for site in &near {
                    if active.ledger.visit(site.centre) {
                        log::info!(
                            "found {} ({}) at {} {}",
                            site.name,
                            site.speciality.name(),
                            site.centre.0,
                            site.centre.1
                        );
                    }
                }
                // Only the town you are standing in has people in it. Distant
                // towns are architecture until you arrive, which is what keeps
                // the frame budget flat however many of them exist.
                if let Some(site) = near.first() {
                    if site.centre != active.villagers.site().centre {
                        active.villagers = Villagers::for_site(site);
                    }
                }
            }
        }

        // The towns do business. Once a dispatch window, not once a frame:
        // the network only looks for work every four in-game minutes, and the
        // one real cost here is gathering who is within radio range.
        let now = active.journal.tick();
        let window = now / economy::DISPATCH_EVERY;
        if window != active.last_network {
            active.last_network = window;
            let column = (
                active.player.position.x.floor() as i32,
                active.player.position.z.floor() as i32,
            );
            let reachable = active.world.generator().towns_near(column, RADIO_RANGE);
            for landed in active.economy.run(&reachable, now) {
                // A load of the player's that has arrived: paid at the far
                // town's price, which is the whole reason to have sent it.
                let Some(site) = reachable.iter().find(|site| site.centre == landed.to) else {
                    continue;
                };
                let paid = landed.amount.round() as u64
                    * active.economy.market(site, now).price(landed.good);
                active.wallet.earn(paid);
                active.greeting = Some((
                    format!(
                        "{} DELIVERED AT {}. +{paid} CR",
                        shop::display_name(economy::GOODS[landed.good]),
                        site.name
                    ),
                    Instant::now(),
                ));
                log::info!("a run landed at {}: {paid} credits", site.name);
            }
        }

        self.refresh_minimap();
        let Some(active) = &mut self.active else { return };

        active
            .renderer
            .update_camera(&active.context.queue, &active.camera);
        active.renderer.set_sun(
            &active.context.queue,
            clock::sun_uniform(clock::sky_at(active.clock)),
        );
        // After the camera, because objects are culled against the frustum it
        // just refreshed.
        let mut objects = active.mining.objects();
        objects.extend(active.villagers.objects(&active.villager_rigs));

        // Trade traffic. A load in the air is not simulated — where it is now
        // is a sum — so drawing one costs a lerp and a height lookup, and only
        // for the handful close enough to see. Everything else on the network
        // stays pure bookkeeping.
        let eye = active.camera.position;
        let now = active.journal.tick();
        for load in active.economy.shipments() {
            let (x, z) = load.position_at(now);
            let (dx, dz) = (x - eye.x, z - eye.z);
            if dx * dx + dz * dz > CARAVAN_SIGHT * CARAVAN_SIGHT {
                continue;
            }
            let ground = active
                .world
                .generator()
                .height_at(x.floor() as i32, z.floor() as i32);
            let at = glam::Vec3::new(x, ground as f32 + CARAVAN_ALTITUDE, z);
            let heading = (load.to.1 - load.from.1) as f32;
            let yaw = ((load.to.0 - load.from.0) as f32).atan2(-heading);
            objects.extend(active.trade_rig.objects(at, yaw, active.mining.spin()));
        }

        // The held drill, drawn in camera space so it rides the view. The bit
        // spins up while it is cutting and idles otherwise; a slight bob sells
        // the vibration.
        active.drill_spin += dt * if active.digging.is_some() { 22.0 } else { 1.6 };
        let camera_forward = active.camera.forward();
        let camera_right = active.camera.right();
        let bob = if active.digging.is_some() {
            (active.drill_spin * 0.9).sin() * 0.02
        } else {
            0.0
        };
        let drill_position = active.camera.position + camera_forward * 0.85
            + camera_right * 0.42
            + glam::Vec3::Y * (-0.38 + bob);
        let drill_yaw = rig::yaw_towards(camera_forward.x, camera_forward.z).unwrap_or(0.0);
        if active.view.draws_viewmodel() {
            objects.extend(active.hand_rig.objects_pitched(
                drill_position,
                drill_yaw,
                active.camera.pitch,
                active.drill_spin,
            ));
        }

        // Your own body, once you can actually see it.
        if active.view.draws_body() {
            objects.extend(active.player_rig.objects(
                active.player.position,
                drill_yaw,
                0.0,
            ));
        }

        active
            .renderer
            .set_objects(&active.context.device, &active.context.queue, &objects);
        self.refresh_hud();
        self.refresh_shop();
        self.refresh_board();
        self.refresh_device();
        self.refresh_feed();
        let Some(active) = &mut self.active else { return };

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

    /// Run the drill for one frame, if it is held on something drillable.
    ///
    /// Hold-to-dig: progress accumulates while the bit stays on one block and
    /// resets when the aim moves — like lifting a real tool. The break itself
    /// still goes through `break_block`, so events fire and a mod's veto works
    /// exactly as it does for the drones.
    fn update_drilling(&mut self, dt: f32) {
        let drilling = self.drill_held && self.input.mouse_captured;
        let Some(active) = &mut self.active else { return };
        if !drilling {
            active.digging = None;
            return;
        }

        let Some(hit) = raycast_solid(
            &active.world,
            active.world.registry(),
            active.camera.position,
            active.camera.forward(),
            Self::REACH,
        ) else {
            active.digging = None;
            return;
        };

        // Hardness is the block's resistance; no hardness (bedrock) means the
        // bit just skates.
        let Some(hardness) = active
            .world
            .registry()
            .get(hit.id)
            .and_then(|def| def.hardness)
        else {
            active.digging = None;
            return;
        };

        let power = skills::drill_power(active.skills.level(skills::MINING))
            * wallet::drill_multiplier(active.wallet.upgrade(wallet::DRILL));
        let step = dt * power / hardness.max(0.05);

        match &mut active.digging {
            Some((target, progress)) if *target == hit.block => {
                *progress += step;
                if *progress < 1.0 {
                    return;
                }
            }
            other => {
                *other = Some((hit.block, step.min(1.0)));
                if step < 1.0 {
                    return;
                }
            }
        }

        // Through: break it, learn from it.
        active.digging = None;
        match break_block(&mut active.world, &active.events, hit.block) {
            Err(error) => log::debug!("could not break {:?}: {error}", hit.block),
            Ok(_) => {
                active.journal.record(Command::Break { at: hit.block });
                let xp = (hardness * skills::MINING_XP_PER_HARDNESS) as u64;
                if let Some(level) = active.skills.add_xp(skills::MINING, xp) {
                    active.level_up = Some((skills::MINING.to_string(), level, Instant::now()));
                }

                // Breaking the base container un-declares the base. The pile
                // it held is set aside for now; stage 7's inventory gives it
                // somewhere to go.
                let was_base = active
                    .mining
                    .fleet
                    .base
                    .as_ref()
                    .is_some_and(|base| base.position == hit.block);
                if was_base {
                    if let Some(pile) = active.mining.fleet.clear_base() {
                        log::info!(
                            "base container broken; {} blocks in it are set aside",
                            pile.total()
                        );
                    }
                }
            }
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
        // Placing a container declares it the fleet's base.
        if let Ok(position) = result {
            let name = active.world.registry().get_or_air(block).name.clone();
            active.journal.record(Command::Place {
                at: position,
                block: name,
            });
            let container = active.world.registry().id_of("engine:container");
            if container == Some(block) {
                active.mining.fleet.set_base(position);
                log::info!("base container set at {position:?}");
            }
        }
        if let Err(error) = result {
            log::debug!("could not place at {:?}: {error}", hit.placement());
        }
    }

    /// React to a key going down, ignoring auto-repeat.
    fn handle_press(&mut self, code: KeyCode) {
        // The handheld, while open, owns the keyboard the same way the shop
        // does: Enter must open a feed, never start a dig.
        if self.active.as_ref().is_some_and(|active| active.device.open) {
            let (roster_len, selected) = {
                let Some(active) = &self.active else { return };
                let from = active.player.eye_position();
                let roster = active.mining.roster(from);
                (roster.len(), active.device.selected(&roster))
            };
            match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    let Some(active) = &mut self.active else { return };
                    let delta = if code == KeyCode::ArrowUp { -1 } else { 1 };
                    active.device.move_cursor(delta, roster_len);
                }
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    if let Some(machine) = selected {
                        self.open_feed(machine);
                    }
                }
                KeyCode::KeyR => {
                    if let Some(machine) = selected {
                        self.open_feed(machine);
                        self.take_or_release_control();
                    }
                }
                KeyCode::KeyV | KeyCode::Escape => {
                    let Some(active) = &mut self.active else { return };
                    active.device.close();
                    active.renderer.clear_overlay(DEVICE_SLOT);
                }
                _ => {}
            }
            return;
        }

        // A live feed: the sticks belong to the machine.
        if self.active.as_ref().is_some_and(|active| active.device.feed().is_some()) {
            match code {
                KeyCode::KeyR => self.take_or_release_control(),
                KeyCode::Escape => self.close_feed(),
                KeyCode::KeyV => {
                    let Some(active) = &mut self.active else { return };
                    active.device.open_list();
                }
                _ => {}
            }
            return;
        }

        // The beacon console, while open, owns the keyboard for the same
        // reason the shop does.
        if self.active.as_ref().is_some_and(|active| active.board.open) {
            let Some(active) = &mut self.active else { return };
            // Taken out and put back so the console's own data can be lent to
            // `confirm` alongside a mutable borrow of the ledger beside it.
            let Some((here, postings, runs)) = active.console.take() else {
                active.board.close();
                return;
            };
            let rows = board::Board::rows_with_runs(here.centre, &postings, &active.ledger, &runs);
            match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    let delta = if code == KeyCode::ArrowUp { -1 } else { 1 };
                    active.board.move_cursor(delta, rows.len());
                }
                // A trade run settles against the network and the base pile,
                // not against the ledger, so it takes a different path.
                KeyCode::Enter | KeyCode::NumpadEnter
                    if matches!(
                        active.board.selected(&rows),
                        Some(board::Row::Ship { .. })
                    ) =>
                {
                    let Some(board::Row::Ship { good, to }) = active.board.selected(&rows) else {
                        unreachable!("just matched a trade run")
                    };
                    let now = active.journal.tick();
                    let pile = active
                        .mining
                        .fleet
                        .base
                        .as_mut()
                        .map(|base| &mut base.stockpile);
                    active
                        .board
                        .ship(&here, good, to, pile, &mut active.economy, now);
                }
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    // Snapshot the sweep record first: the closure and the
                    // base pile both live inside the fleet.
                    let swept: std::collections::HashSet<vx_agent::Sector> =
                        active.mining.fleet.surveyed_sectors().collect();
                    let pile = active
                        .mining
                        .fleet
                        .base
                        .as_mut()
                        .map(|base| &mut base.stockpile);
                    active.board.confirm(
                        &here,
                        &postings,
                        &mut active.ledger,
                        pile,
                        &mut active.wallet,
                        &|at| swept.contains(&vx_agent::Sector::containing(at.0, at.1)),
                    );
                }
                KeyCode::KeyE | KeyCode::Escape => {
                    active.board.close();
                    active.renderer.clear_overlay(BOARD_SLOT);
                }
                _ => {}
            }
            active.console = Some((here, postings, runs));
            return;
        }

        // The shop, while open, owns the keyboard: Enter must trade, never
        // fall through to `start_mining`.
        if self.active.as_ref().is_some_and(|active| active.shop.open) {
            let Some(active) = &mut self.active else { return };
            match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    let pile = active
                        .mining
                        .fleet
                        .base
                        .as_ref()
                        .map(|base| &base.stockpile);
                    let market = active
                        .counter
                        .map(|site| active.economy.market(&site, active.journal.tick()).clone());
                    let Some(market) = market else { return };
                    let rows = shop::Shop::rows(pile, &active.wallet, &market).len();
                    let delta = if code == KeyCode::ArrowUp { -1 } else { 1 };
                    active.shop.move_cursor(delta, rows);
                }
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    let pile = active
                        .mining
                        .fleet
                        .base
                        .as_mut()
                        .map(|base| &mut base.stockpile);
                    let Some(site) = active.counter else { return };
                    let now = active.journal.tick();
                    let market = active.economy.market_mut(&site, now);
                    active.shop.confirm(pile, &mut active.wallet, market);
                }
                KeyCode::KeyE | KeyCode::Escape => {
                    active.shop.close();
                    active.renderer.clear_overlay(SHOP_SLOT);
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Escape => self.set_capture(false),
            KeyCode::KeyE => self.interact(),
            KeyCode::KeyF => {
                if let Some(active) = &mut self.active {
                    active.mode = active.mode.toggled();
                    // Entering walk mode drops the player from wherever the
                    // camera was, so clear any stale fall speed.
                    active.player.velocity = glam::Vec3::ZERO;
                    log::info!("movement mode: {:?}", active.mode);
                }
            }
            KeyCode::KeyV => {
                if let Some(active) = &mut self.active {
                    active.device.open_list();
                }
            }
            KeyCode::KeyC => {
                if let Some(active) = &mut self.active {
                    active.view = active.view.cycled();
                    log::info!("view: {:?}", active.view);
                }
            }
            KeyCode::F5 => self.save_world(),
            KeyCode::KeyM => self.mark_target(),
            KeyCode::Tab => {
                if let Some(active) = &mut self.active {
                    active.mining.cycle_method();
                    if let Some(status) = active.mining.status() {
                        log::info!("{status}");
                    }
                }
            }
            KeyCode::Enter | KeyCode::NumpadEnter => self.start_mining(),
            KeyCode::KeyN => {
                if let Some(active) = &mut self.active {
                    active.map.visible = !active.map.visible;
                    if !active.map.visible {
                        active.renderer.clear_overlay(MAP_SLOT);
                    } else {
                        active.map.invalidate();
                    }
                }
            }
            KeyCode::BracketLeft => {
                if let Some(active) = &mut self.active {
                    active.map.zoom_out();
                    active.map.invalidate();
                }
            }
            KeyCode::BracketRight => {
                if let Some(active) = &mut self.active {
                    active.map.zoom_in();
                    active.map.invalidate();
                }
            }
            KeyCode::KeyG => {
                if let Some(active) = &mut self.active {
                    let at = active.camera.position;
                    let (x, z) = (at.x.floor() as i32, at.z.floor() as i32);
                    if active.mining.dispatch_scan(x, z) {
                        log::info!("flier dispatched to scan this sector");
                    } else {
                        log::info!("the flier is busy");
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(active) = &mut self.active {
                    active.mining.cancel(&mut active.world);
                    active.journal.record(Command::Cancel);
                    log::info!("mining plan cancelled");
                }
            }
            // Number keys pick a block to build with.
            KeyCode::Digit1
            | KeyCode::Digit2
            | KeyCode::Digit3
            | KeyCode::Digit4
            | KeyCode::Digit5 => {
                if let Some(active) = &mut self.active {
                    let slot = match code {
                        KeyCode::Digit1 => 0,
                        KeyCode::Digit2 => 1,
                        KeyCode::Digit3 => 2,
                        KeyCode::Digit4 => 3,
                        _ => 4,
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

    /// Mark a corner of a body on whatever the player is looking at.
    fn mark_target(&mut self) {
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
        active.mining.mark(&mut active.world, hit.block);
        match active.mining.status() {
            Some(status) => log::info!("{status}"),
            None => log::info!("marked {:?}; mark a second corner", hit.block),
        }
    }

    /// Put a drone on the selected plan.
    fn start_mining(&mut self) {
        let Some(active) = &mut self.active else { return };
        if active.mining.is_running() {
            return;
        }
        let area = active.mining.area();
        match active.mining.start(&mut active.world) {
            Some(method) => {
                if let Some(area) = area {
                    active.journal.record(Command::Dispatch { area, method });
                }
                log::info!(
                    "digging: {} with a crew of {}",
                    method.name(),
                    active.mining.crew()
                );
            }
            None => log::info!("nothing marked to dig"),
        }
    }

    /// Press E at a piece of town furniture: the counter opens the shop, the
    /// console at the foot of the mast opens the beacon board.
    ///
    /// The town behind a console is *derived* from where the console is, not
    /// stored with it — the same lattice the world was generated from answers
    /// "which town is this", which is why a beacon works in a town that was
    /// built on the spot two seconds ago.
    fn interact(&mut self) {
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
        match active.world.registry().get_or_air(hit.id).name.as_str() {
            "engine:counter" => {
                // A counter is a *local* market now, so it has to know which
                // town it stands in — derived from where it is, exactly as the
                // beacon console below does it.
                let column = (hit.block.x, hit.block.z);
                match active
                    .world
                    .generator()
                    .towns_near(column, vx_world::town::REACH)
                    .into_iter()
                    .next()
                {
                    Some(site) => {
                        active.counter = Some(site);
                        active.shop.open_at_counter();
                    }
                    None => log::warn!("a counter at {column:?} with no town behind it"),
                }
            }
            "engine:beacon" => {
                let column = (hit.block.x, hit.block.z);
                let Some(site) = active
                    .world
                    .generator()
                    .towns_near(column, vx_world::town::REACH)
                    .into_iter()
                    .next()
                else {
                    log::warn!("a beacon at {column:?} with no town behind it");
                    return;
                };
                let neighbours = active.world.generator().towns_near(site.centre, RADIO_RANGE);
                let postings = beacon::postings_for(&site, &neighbours);
                // What this town will pay somebody to carry, and where to: the
                // goods it is sitting on that a neighbour is short of.
                let now = active.journal.tick();
                let mut runs = Vec::new();
                for good in 0..economy::GOODS.len() {
                    let Some(target) = neighbours
                        .iter()
                        .filter(|other| other.centre != site.centre)
                        .find(|other| active.economy.market(other, now).wants(good))
                    else {
                        continue;
                    };
                    runs.push((good, target.centre));
                }
                active.ledger.visit(site.centre);
                active.console = Some((site, postings, runs));
                active.board.open_at_beacon();
            }
            _ => {}
        }
    }

    /// Turn this frame's held keys into one tick of pilot intent.
    ///
    /// Movement is camera-relative, exactly like walking: forward is where the
    /// machine is looking, strafe is that rotated a quarter turn. The result
    /// is a cardinal, because the simulation underneath moves in whole cells.
    fn pilot_command(
        camera: &Camera,
        input: &InputState,
        cutting: bool,
    ) -> vx_agent::PilotCommand {
        use vx_agent::Heading;

        let axes = input.movement_axes();
        let forward = camera.forward_level();
        let looking = Heading::from_look(forward.x, forward.z);

        // Forward wins over strafe when both are held: one cardinal per tick,
        // and picking the one the player is facing is the least surprising.
        let heading = if axes.z.abs() > 0.1 {
            looking.map(|heading| if axes.z > 0.0 { heading } else { heading.rotated(2) })
        } else if axes.x.abs() > 0.1 {
            looking.map(|heading| heading.rotated(if axes.x > 0.0 { -1 } else { 1 }))
        } else {
            None
        };

        vx_agent::PilotCommand {
            heading,
            cut: cutting,
            climb: if axes.y > 0.1 {
                1
            } else if axes.y < -0.1 {
                -1
            } else {
                0
            },
        }
    }

    /// Has the ground under the player's own feet arrived back?
    ///
    /// Piloting a machine far away re-centres streaming on it, which unloads
    /// the player's own chunks. Stepping physics before they come back drops
    /// the body through a world that is not there yet.
    fn body_chunk_ready(&self) -> bool {
        self.active.as_ref().is_some_and(|active| {
            active.world.is_loaded(chunk_at(active.player.position))
        })
    }

    /// Master override: take the wheel of whatever is being watched, or give
    /// it back. Keeps the handheld's idea of control and the simulation's in
    /// step by doing both in one place.
    fn take_or_release_control(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some((machine, taking)) = active.device.toggle_control() else {
            return;
        };
        if taking {
            if active.mining.take_control(machine) {
                active.device.feedback = Some("CONTROL TAKEN".into());
                log::info!("took control of {machine:?}");
            } else {
                // The simulation refused; do not leave the handheld claiming
                // a wheel it does not have.
                active.device.toggle_control();
                active.device.feedback = Some("CONTROL REFUSED".into());
            }
        } else {
            active.mining.release_control();
            active.device.feedback = Some("CONTROL RELEASED".into());
            log::info!("handed {machine:?} back");
        }
    }

    /// Take a machine's feed, parking the player's own view.
    fn open_feed(&mut self, machine: mining::MachineRef) {
        let Some(active) = &mut self.active else { return };
        if active.device.feed().is_none() {
            active.body_yaw = active.camera.yaw;
            active.body_pitch = active.camera.pitch;
        }
        active.device.view(machine);
        active.view = ViewMode::Fpv;
    }

    /// Hang up: hand any driven machine back and return to the body.
    fn close_feed(&mut self) {
        let Some(active) = &mut self.active else { return };
        if active.device.feed().is_none() {
            return;
        }
        if active.device.hand_back().is_some() {
            active.mining.release_control();
        }
        active.camera.yaw = active.body_yaw;
        active.camera.pitch = active.body_pitch;
        active.view = ViewMode::FirstPerson;
        // The body's own ground may have streamed out while we were away.
        active.resuming = true;
        active.renderer.clear_overlay(FEED_SLOT);
        active.renderer.clear_overlay(DEVICE_SLOT);
    }

    /// Rebuild and upload the handheld panel while it is open.
    fn refresh_device(&mut self) {
        let Some(active) = &mut self.active else { return };
        if !active.device.open {
            return;
        }
        let from = active.player.eye_position();
        let roster = active.mining.roster(from);
        let pixels = device::render_device(&active.device, &roster);
        let (width, height) = active.renderer.size();
        let panel_width = device::DEVICE_WIDTH as f32 * device::DEVICE_SCALE;
        let panel_height = device::DEVICE_HEIGHT as f32 * device::DEVICE_SCALE;
        active.renderer.set_overlay(
            DEVICE_SLOT,
            &active.context.device,
            &active.context.queue,
            (device::DEVICE_WIDTH, device::DEVICE_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    /// Rebuild and upload the banner sitting over a live feed.
    fn refresh_feed(&mut self) {
        let Some(active) = &mut self.active else { return };
        let Some(machine) = active.device.feed() else { return };
        let from = active.player.eye_position();
        let Some(listing) = active.mining.listing(machine, from) else { return };
        let strength = device::signal(listing.distance);
        let piloting = active.device.is_piloting();
        let pixels = device::render_feed_banner(&listing, piloting, strength);
        let (width, _) = active.renderer.size();
        let panel_width = device::BANNER_WIDTH as f32 * device::BANNER_SCALE;
        let panel_height = device::BANNER_HEIGHT as f32 * device::BANNER_SCALE;
        active.renderer.set_overlay(
            FEED_SLOT,
            &active.context.device,
            &active.context.queue,
            (device::BANNER_WIDTH, device::BANNER_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: 12.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    /// Rebuild and upload the shop panel while it is open. Closing clears the
    /// slot in the key handler, so an unset slot keeps frames byte-identical.
    fn refresh_shop(&mut self) {
        let Some(active) = &mut self.active else { return };
        if !active.shop.open {
            return;
        }
        let pile = active
            .mining
            .fleet
            .base
            .as_ref()
            .map(|base| &base.stockpile);
        let Some(site) = active.counter else { return };
        let market = active.economy.market(&site, active.journal.tick()).clone();
        let pixels = shop::render_shop(&active.shop, pile, &active.wallet, &site, &market);
        let (width, height) = active.renderer.size();
        let panel_width = shop::SHOP_WIDTH as f32 * shop::SHOP_SCALE;
        let panel_height = shop::SHOP_HEIGHT as f32 * shop::SHOP_SCALE;
        active.renderer.set_overlay(
            SHOP_SLOT,
            &active.context.device,
            &active.context.queue,
            (shop::SHOP_WIDTH, shop::SHOP_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    /// Rebuild and upload the beacon board while it is open.
    fn refresh_board(&mut self) {
        let Some(active) = &mut self.active else { return };
        if !active.board.open {
            return;
        }
        // Taken out so the books can be borrowed mutably alongside it.
        let Some((here, postings, runs)) = active.console.take() else {
            return;
        };
        let market = active.economy.market(&here, active.journal.tick()).clone();
        let pixels = board::render_board(
            &active.board,
            &here,
            &postings,
            &active.ledger,
            &active.wallet,
            &market,
            &runs,
        );
        active.console = Some((here, postings, runs));
        let (width, height) = active.renderer.size();
        let panel_width = board::BOARD_WIDTH as f32 * board::BOARD_SCALE;
        let panel_height = board::BOARD_HEIGHT as f32 * board::BOARD_SCALE;
        active.renderer.set_overlay(
            BOARD_SLOT,
            &active.context.device,
            &active.context.queue,
            (board::BOARD_WIDTH, board::BOARD_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: (width as f32 - panel_width) / 2.0,
                y: (height as f32 - panel_height) / 2.0,
                width: panel_width,
                height: panel_height,
            },
        );
    }

    /// Rebuild and upload the HUD panel. Cheap enough to run every frame,
    /// which the fast-moving drill bar wants anyway.
    fn refresh_hud(&mut self) {
        let Some(active) = &mut self.active else { return };

        let level_up = active.level_up.as_ref().and_then(|(skill, level, at)| {
            (at.elapsed().as_secs_f32() < 2.5).then(|| (skill.clone(), *level))
        });
        let greeting = active.greeting.as_ref().and_then(|(line, at)| {
            (at.elapsed().as_secs_f32() < 3.0).then(|| line.clone())
        });
        let content = hud::HudContent {
            skills: &active.skills,
            time: active.clock,
            reconnecting: active.resuming,
            status: active.mining.status(),
            drilling: active.digging.map(|(_, progress)| progress),
            level_up,
            greeting,
        };
        let pixels = hud::render_hud(&content);

        let (_, screen_height) = active.renderer.size();
        let margin = 12.0;
        let width = hud::HUD_WIDTH as f32 * hud::HUD_SCALE;
        let height = hud::HUD_HEIGHT as f32 * hud::HUD_SCALE;
        active.renderer.set_overlay(
            HUD_SLOT,
            &active.context.device,
            &active.context.queue,
            (hud::HUD_WIDTH, hud::HUD_HEIGHT),
            &pixels,
            vx_render::OverlayRect {
                x: margin,
                y: screen_height as f32 - height - margin,
                width,
                height,
            },
        );
    }

    /// Rebuild and upload the minimap overlay, on the redraw throttle.
    fn refresh_minimap(&mut self) {
        let Some(active) = &mut self.active else { return };
        if !active.map.visible || !active.map.should_redraw() {
            return;
        }

        // On a feed the map follows the machine — it is what you are looking
        // through — and the parked body becomes a marker so you can see how
        // far you have sent it.
        let feed_at = active
            .device
            .feed()
            .and_then(|machine| active.mining.machine_position(machine));
        let centre = match feed_at {
            Some(at) => (at.x, at.z),
            None => {
                let camera = active.camera.position;
                (camera.x.floor() as i32, camera.z.floor() as i32)
            }
        };

        let mut markers = Vec::new();
        if feed_at.is_some() {
            markers.push(map::Marker {
                x: active.player.position.x.floor() as i32,
                z: active.player.position.z.floor() as i32,
                colour: map::colour::PLAYER,
                radius: 2,
            });
        }
        if let Some(base) = &active.mining.fleet.base {
            markers.push(map::Marker {
                x: base.position.x,
                z: base.position.z,
                colour: map::colour::BASE,
                radius: 2,
            });
        }
        for ping in active.mining.fleet.pings() {
            markers.push(map::Marker {
                x: ping.position.x,
                z: ping.position.z,
                colour: map::colour::PING,
                radius: 2,
            });
        }
        for position in active.mining.drone_positions() {
            markers.push(map::Marker {
                x: position.x,
                z: position.z,
                colour: map::colour::DRONE,
                radius: 1,
            });
        }
        for flier in &active.mining.fleet.fliers {
            markers.push(map::Marker {
                x: flier.position.x,
                z: flier.position.z,
                colour: map::colour::FLIER,
                radius: 1,
            });
        }
        for town in active.ledger.visited() {
            markers.push(map::Marker {
                x: town.0,
                z: town.1,
                colour: map::colour::TOWN,
                radius: 2,
            });
        }
        // Trade traffic, wherever it is. Watching a load crawl across the map
        // toward a town you sold to is half the pleasure of the network.
        let now = active.journal.tick();
        for load in active.economy.shipments() {
            let (x, z) = load.position_at(now);
            markers.push(map::Marker {
                x: x as i32,
                z: z as i32,
                colour: map::colour::TRADE,
                radius: 1,
            });
        }
        // Contract pins draw over the fog on purpose: a posting can name a
        // town you have never seen, and going to find it is the job.
        for pin in active.ledger.pins() {
            markers.push(map::Marker {
                x: pin.0,
                z: pin.1,
                colour: map::colour::CONTRACT,
                radius: 3,
            });
        }
        // The player draws last, so nothing covers you.
        markers.push(map::Marker {
            x: centre.0,
            z: centre.1,
            colour: map::colour::PLAYER,
            radius: 1,
        });

        let pixels = map::render_map(&active.world, &active.map, centre, &markers);
        let (screen_width, _) = active.renderer.size();
        let margin = 12.0;
        let panel = map::MAP_SIZE as f32;
        active.renderer.set_overlay(
            MAP_SLOT,
            &active.context.device,
            &active.context.queue,
            (map::MAP_SIZE, map::MAP_SIZE),
            &pixels,
            vx_render::OverlayRect {
                x: screen_width as f32 - panel - margin,
                y: margin,
                width: panel,
                height: panel,
            },
        );
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
        match active.map.save(save.root()) {
            Ok(()) => log::info!("saved the map ({} explored chunks)", active.map.explored_count()),
            Err(error) => log::error!("could not save the map: {error}"),
        }
        if let Err(error) = active.skills.save(save.root()) {
            log::error!("could not save the skill sheet: {error}");
        }
        if let Err(error) = active.wallet.save(save.root()) {
            log::error!("could not save the wallet: {error}");
        }
        if let Err(error) = clock::save(active.clock, save.root()) {
            log::error!("could not save the clock: {error}");
        }
        if let Err(error) = active.ledger.save(save.root()) {
            log::error!("could not save the posting ledger: {error}");
        }
        // The region files above are the keyframe; the journal is everything
        // ordered since. Both are written every save for now — the journal
        // earns its keep as a determinism oracle first, and only once it has
        // been proving itself against real sessions is it worth letting it
        // *replace* region writes on disk.
        // Keyframe only once the tail is long enough to be worth bounding.
        // Regions are written every save regardless, so loading never depends
        // on the journal — it accumulates alongside as the record `--replay`
        // checks the simulation against.
        if active.journal.wants_keyframe() {
            active.journal.keyframed(vx_world::world_hash(&active.world));
        }
        if let Err(error) = active.journal.save(save.root()) {
            log::error!("could not save the command journal: {error}");
        }
        match active.economy.save(save.root()) {
            Ok(()) => log::info!("saved {} town markets", active.economy.tracked()),
            Err(error) => log::error!("could not save the town books: {error}"),
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
            "engine:container",
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

        let mut map = MapState::new();
        let mut skills = Skills::new();
        let mut wallet = Wallet::new();
        let mut clock = TimeOfDay::default();
        let mut ledger = beacon::Ledger::new();
        let mut journal = CommandLog::new();
        let mut economy = economy::Economy::new();
        if let Some(save) = &save {
            map.load(save.root());
            skills.load(save.root());
            wallet.load(save.root());
            clock = clock::load(save.root());
            ledger.load(save.root());
            journal = CommandLog::load(save.root());
            economy.load(save.root());
        }

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
            mining: {
                let mut mining = Mining::default();
                mining.set_crew(self.crew);
                mining.ensure_flier(camera.position);
                mining
            },
            map,
            skills,
            digging: None,
            level_up: None,
            clock,
            view: ViewMode::default(),
            device: device::Device::new(),
            body_yaw: 0.0,
            body_pitch: 0.0,
            resuming: false,
            player_rig: Rig::player(),
            hand_rig: Rig::hand_drill(),
            trade_rig: Rig::flier(),
            drill_spin: 0.0,
            villagers: Villagers::new(),
            villager_rigs: Villagers::rigs(),
            greeting: None,
            wallet,
            shop: shop::Shop::new(),
            board: board::Board::new(),
            ledger,
            journal,
            economy,
            counter: None,
            console: None,
            // Deliberately absurd, so the first frame always scans.
            last_scan: (i32::MAX, i32::MAX),
            last_network: u64::MAX,
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

            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Released {
                    if button == MouseButton::Left {
                        // Letting go stops the drill mid-block; progress on
                        // that block is abandoned, like lifting a real tool.
                        self.drill_held = false;
                    }
                    return;
                }
                // The first click captures the pointer; once captured, the
                // held left button runs the drill. Without the capture step,
                // the click that focuses the window would also start cutting.
                if !self.input.mouse_captured {
                    if button == MouseButton::Left {
                        self.set_capture(true);
                    }
                    return;
                }
                match button {
                    MouseButton::Left => self.drill_held = true,
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
