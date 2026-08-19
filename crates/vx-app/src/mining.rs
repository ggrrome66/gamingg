//! Marking a body, choosing how to mine it, and watching a drone do it.
//!
//! This is the thin layer between the player and [`vx_agent`], which knows
//! nothing about windows, input or rendering. Everything here is presentation
//! and intent: pick two corners, look at what the game proposes, disagree if
//! you like, then press go.
//!
//! # The game proposes and the player disposes
//!
//! [`vx_agent::options`] ranks every method that applies, cheapest first, and
//! the top one is preselected. Cycling through the rest shows what disagreeing
//! costs. That is deliberately a *suggestion*: the ranking is a defensible
//! model, not a fact, and a player who wants a pit because a pit looks better
//! should be able to have one.

use std::time::Duration;

use glam::Vec3;
use vx_agent::{
    options, DroneState, Fleet, FlierState, MineMethod, MinePlan, Operation, PilotCommand, Sector,
    VoxelAabb,
};
use vx_core::{BlockPos, EventBus};
use vx_agent::FleetReport;
use vx_render::tiles::slot;
use vx_render::Object;
use vx_world::World;

use crate::rig::{self, Rig};

/// Drone ticks per second.
///
/// A drone cuts one block per tick, so this is also its digging rate. Slow
/// enough to watch the excavation take shape, which is most of the appeal.
const TICK_RATE: f64 = 8.0;

/// Blocks of run per block of rise the starting drone's ramps are cut to.
const GRADE: i32 = vx_agent::DEFAULT_GRADE;

/// Edge length of the cubes drawn at the corners of a marked area.
const MARKER_SIZE: f32 = 0.35;

/// Edge length of a hovering ping marker.
const PING_SIZE: f32 = 0.5;

/// Radians per second the drill and rotor turn while working.
const SPIN_RATE: f32 = 9.0;

/// Drones a dispatch puts on the job by default.
///
/// Small enough that a hole still reads as a hole rather than a scrum, big
/// enough that the job board's claim-and-release paths actually run.
pub const DEFAULT_CREW: u32 = 3;

/// What the player is currently doing with the mining tools.
#[derive(Debug)]
pub struct Mining {
    /// Corners picked so far. Two makes an area.
    corners: Vec<BlockPos>,
    /// The marked area, once both corners are down.
    area: Option<VoxelAabb>,
    /// Every method that applies here, cheapest first.
    plans: Vec<MinePlan>,
    /// Which of `plans` is selected.
    chosen: usize,
    /// The running excavation, once started.
    operation: Option<Operation>,
    /// Fractional ticks carried between frames, so the drone runs at a steady
    /// rate rather than at whatever the frame rate happens to be.
    pending: f64,
    /// The air side: the flier, its surveys, and the base container.
    pub fleet: Fleet,
    /// Nose directions, remembered so parked machines do not twitch.
    /// Chunks held resident for the running operation, released when it ends.
    pinned: Vec<vx_core::ChunkPos>,
    /// Ticks the last `update` turned its elapsed time into. What the command
    /// journal records, because wall time is not reproducible and this is.
    last_ticks: u32,
    /// Drones a dispatch puts on the job.
    crew: u32,
    drone_yaws: Vec<f32>,
    flier_yaws: Vec<f32>,
    /// Drill/rotor angle, advanced while machines work.
    spin: f32,
    /// The rig shapes, built once.
    digger_rig: Rig,
    flier_rig: Rig,
    /// The machine the player has the wheel of, if any.
    piloted: Option<MachineRef>,
    /// This frame's held pilot input, re-issued on every simulation tick so
    /// driving is frame-rate independent exactly like the autonomous path.
    pilot_command: PilotCommand,
    /// Where the pilot is looking: a driven machine points its nose along the
    /// player's view rather than along its last step.
    pilot_look: f32,
}

/// One of the fleet's machines, by kind and index.
///
/// A thin seam rather than a shared entity abstraction: drones and fliers have
/// almost nothing in common beyond "the player can look through it", and
/// inventing a trait for two cases would cost more than it saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineRef {
    Digger(usize),
    Flier(usize),
}

/// One row of the handheld's roster.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineListing {
    pub machine: MachineRef,
    /// "DIGGER 1", "FLIER 1".
    pub name: String,
    /// What it is up to, in a word.
    pub state: String,
    /// How far the player is from it, in blocks.
    pub distance: f32,
    pub cargo: u64,
    pub capacity: u64,
}

impl Default for Mining {
    fn default() -> Self {
        Mining {
            corners: Vec::new(),
            area: None,
            plans: Vec::new(),
            chosen: 0,
            operation: None,
            pinned: Vec::new(),
            last_ticks: 0,
            crew: DEFAULT_CREW,
            pending: 0.0,
            fleet: Fleet::default(),
            drone_yaws: Vec::new(),
            flier_yaws: Vec::new(),
            spin: 0.0,
            digger_rig: Rig::digger(),
            flier_rig: Rig::flier(),
            piloted: None,
            pilot_command: PilotCommand::default(),
            pilot_look: 0.0,
        }
    }
}

impl Mining {
    /// Make sure the fleet has its aircraft, hovering near `position`.
    ///
    /// Idempotent so callers need not track whether it happened; one flier is
    /// this milestone's scope.
    pub fn ensure_flier(&mut self, position: Vec3) {
        if self.fleet.fliers.is_empty() {
            self.fleet.add_flier(BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32 + 6,
                position.z.floor() as i32,
            ));
        }
    }

    /// Send the flier to sweep the sector containing the column `(x, z)`.
    pub fn dispatch_scan(&mut self, x: i32, z: i32) -> bool {
        self.fleet.dispatch_scan(Sector::containing(x, z))
    }

    /// Add a corner at `pos`, replacing the previous pair once two are down.
    pub fn mark(&mut self, world: &mut World, pos: BlockPos) {
        if self.operation.is_some() {
            return;
        }
        if self.corners.len() >= 2 {
            self.corners.clear();
            self.plans.clear();
            self.area = None;
        }
        self.corners.push(pos);

        if let [first, second] = self.corners[..] {
            let area = VoxelAabb::new(first, second);
            // Planning reads the world too — it looks for a hillside to drive
            // into and a grade it can climb — so the ground has to be resident
            // before it is consulted, not just before a drone drives on it.
            // Released by `cancel`, or replaced by the plan's own span at
            // `start`.
            self.release_ground(world);
            let scouted = vx_agent::working_span(area, area.min);
            self.pinned = world.pin_span(scouted.min, scouted.max);

            self.area = Some(area);
            self.plans = options(world, area, GRADE);
            self.chosen = 0;
        }
    }

    /// Choose the next method in the list.
    pub fn cycle_method(&mut self) {
        if !self.plans.is_empty() && self.operation.is_none() {
            self.chosen = (self.chosen + 1) % self.plans.len();
        }
    }

    pub fn selected_plan(&self) -> Option<&MinePlan> {
        self.plans.get(self.chosen)
    }

    /// Post the selected plan and put a drone on it.
    ///
    /// The drone starts at the portal and the stockpile sits there too, so the
    /// haul route is the excavation itself — which is the whole point of
    /// choosing a method the drone can drive.
    pub fn start(&mut self, world: &mut World) -> Option<MineMethod> {
        let plan = self.plans.get(self.chosen)?.clone();
        let start = vx_agent::settle(world, plan.portal);

        // Hold the ground the drones will read for as long as the job lasts.
        // Without this a drone near the edge of the loaded set sees air where
        // there is rock, and the same dispatch digs a different hole depending
        // on where the player happened to stand while it ran.
        let span = vx_agent::working_span(plan.span(), start);
        self.release_ground(world);
        self.pinned = world.pin_span(span.min, span.max);

        let mut operation = Operation::new(start);
        // A crew, not a single machine. The job board has always been built for
        // contention — claims, releases, nearest-first — and until now exactly
        // one drone ever existed, so none of that ever ran.
        for _ in 0..self.crew.max(1) {
            operation.add_drone(start);
        }
        operation.post_plan(&plan);
        self.operation = Some(operation);
        Some(plan.method)
    }

    /// Release the ground an operation was holding. Safe to call with nothing
    /// pinned, which is what makes it safe to call from `cancel`.
    pub fn release_ground(&mut self, world: &mut World) {
        let span = std::mem::take(&mut self.pinned);
        world.unpin_span(&span);
    }

    /// Abandon whatever is marked or running. The hole stays dug, and the
    /// fleet — the flier, its surveys, the base — is not the plan's to throw
    /// away, so it survives.
    pub fn cancel(&mut self, world: &mut World) {
        // Let the ground go before dropping the state that remembers it, or
        // the pin outlives the job and streaming can never evict that span.
        self.release_ground(world);
        let fleet = std::mem::take(&mut self.fleet);
        *self = Mining {
            fleet,
            ..Mining::default()
        };
    }

    pub fn is_running(&self) -> bool {
        self.operation.is_some()
    }

    /// Where the ground drones are, for the minimap's dots.
    pub fn drone_positions(&self) -> Vec<BlockPos> {
        self.operation
            .iter()
            .flat_map(|operation| operation.drones.iter().map(|drone| drone.position))
            .collect()
    }

    /// Advance the excavation by however many ticks `elapsed` is worth.
    ///
    /// Returns the fleet's accumulated report, so the caller can turn work
    /// into experience without the fleet knowing players exist.
    pub fn update(&mut self, world: &mut World, events: &EventBus, elapsed: Duration) -> FleetReport {
        // Machines that are working turn their drills and rotors.
        let working = self
            .operation
            .as_ref()
            .is_some_and(|operation| {
                operation
                    .drones
                    .iter()
                    .any(|drone| matches!(drone.state, DroneState::Digging(_)))
            })
            || self
                .fleet
                .fliers
                .iter()
                .any(|flier| flier.state != FlierState::Idle);
        if working {
            self.spin += elapsed.as_secs_f32() * SPIN_RATE;
        }

        self.pending += elapsed.as_secs_f64() * TICK_RATE;
        // Cap the catch-up. After a long stall — loading, or a dragged window —
        // running hundreds of ticks in one frame would teleport the drone and
        // spike the frame it lands on.
        let ticks = (self.pending as u32).min(16);
        self.pending -= f64::from(ticks);
        // And *discard* the rest of the backlog rather than keeping it: the
        // first version only subtracted the ticks run, so a 30-second stall
        // drained at the cap for seconds afterwards — a ~120x fast-forward,
        // the very teleport the cap exists to prevent. Time lost to a stall
        // stays lost; the drone simply pauses with the game.
        self.pending = self.pending.min(1.0);

        self.last_ticks = ticks;
        self.advance(world, events, ticks)
    }

    /// How far the machines' rotors and drills have turned. Shared so anything
    /// else that draws a machine spins in step with the fleet.
    pub fn spin(&self) -> f32 {
        self.spin
    }

    /// The marked area, if two corners are down. What the journal records as
    /// the dispatch, since the plan itself is derivable from it.
    pub fn area(&self) -> Option<VoxelAabb> {
        self.area
    }

    /// How many drones a dispatch puts on the job.
    pub fn set_crew(&mut self, crew: u32) {
        self.crew = crew.max(1);
    }

    pub fn crew(&self) -> u32 {
        self.crew
    }

    /// Ticks the last [`Mining::update`] ran. Recorded by the journal.
    pub fn last_ticks(&self) -> u32 {
        self.last_ticks
    }

    /// Run exactly `ticks` simulation ticks.
    ///
    /// The seam the command log records against. Wall time decides *how many*
    /// ticks a frame is worth, and that answer depends on frame rate, stalls
    /// and how long a window was dragged — none of which is reproducible. The
    /// tick count is. So the log stores ticks and replays through here, and a
    /// session recorded on a machine managing nine frames a second replays
    /// identically on one managing three hundred.
    pub fn advance(&mut self, world: &mut World, events: &EventBus, ticks: u32) -> FleetReport {
        let mut report = FleetReport::default();

        if self.operation.is_none() {
            // The fleet still flies with no dig running.
            for _ in 0..ticks {
                self.pilot_sub_tick(world, events);
                let tick = self.fleet.tick(world, &mut []);
                report.delivered += tick.delivered;
                report.sectors_completed += tick.sectors_completed;
                report.pings_found += tick.pings_found;
            }
            self.remember_yaws();
            return report;
        }

        for _ in 0..ticks {
            self.pilot_sub_tick(world, events);
            let operation = self.operation.as_mut().expect("checked above");
            operation.tick(world, events);
        }
        for _ in 0..ticks {
            // The air side reads the world immutably and trades with the
            // mine's stockpile.
            let mines = self.operation.as_mut().map(std::slice::from_mut).unwrap_or(&mut []);
            let tick = self.fleet.tick(world, mines);
            report.delivered += tick.delivered;
            report.sectors_completed += tick.sectors_completed;
            report.pings_found += tick.pings_found;
        }
        self.remember_yaws();
        report
    }

    /// One simulation tick of the player's held controls.
    ///
    /// Runs inside the same accumulator loop as the autonomous tick, so a
    /// piloted machine covers exactly the same ground per second as one
    /// driving itself — the override is a change of driver, not of physics.
    fn pilot_sub_tick(&mut self, world: &mut World, events: &EventBus) {
        match self.piloted {
            Some(MachineRef::Digger(_)) => {
                if let Some(operation) = &mut self.operation {
                    operation.pilot_tick(world, events, self.pilot_command);
                }
            }
            Some(MachineRef::Flier(_)) => {
                self.fleet.pilot_tick(world, self.pilot_command);
            }
            None => {}
        }
    }

    /// Update remembered nose directions from this tick's movement.
    fn remember_yaws(&mut self) {
        // A piloted machine points where the player is looking. Deriving its
        // nose from movement instead would leave a stationary drone facing
        // whatever direction its last step happened to take while the player
        // spins the camera around it.
        let piloted = self.piloted;
        if let Some(operation) = &self.operation {
            self.drone_yaws.resize(operation.drones.len(), 0.0);
            for (index, (yaw, drone)) in
                self.drone_yaws.iter_mut().zip(&operation.drones).enumerate()
            {
                if piloted == Some(MachineRef::Digger(index)) {
                    *yaw = self.pilot_look;
                    continue;
                }
                let delta = drone.position;
                let previous = drone.previous_position;
                if let Some(new) = rig::yaw_towards(
                    (delta.x - previous.x) as f32,
                    (delta.z - previous.z) as f32,
                ) {
                    *yaw = new;
                }
            }
        }
        self.flier_yaws.resize(self.fleet.fliers.len(), 0.0);
        for (index, (yaw, flier)) in self.flier_yaws.iter_mut().zip(&self.fleet.fliers).enumerate()
        {
            if piloted == Some(MachineRef::Flier(index)) {
                *yaw = self.pilot_look;
                continue;
            }
            if let Some(new) = rig::yaw_towards(
                (flier.position.x - flier.previous_position.x) as f32,
                (flier.position.z - flier.previous_position.z) as f32,
            ) {
                *yaw = new;
            }
        }
    }

    /// How far between the last tick and the next the visuals should sit.
    fn tick_fraction(&self) -> f32 {
        (self.pending as f32).clamp(0.0, 1.0)
    }

    /// Apply the player's current skill effects to the machines.
    ///
    /// Called by the app whenever levels change (and cheaply on every grant):
    /// the fleet and drones never know skills exist, they just have stats.
    pub fn apply_skills(&mut self, scan_depth: i32, drone_capacity: u64, flier_capacity: u64) {
        self.fleet.scan_depth = scan_depth;
        for flier in &mut self.fleet.fliers {
            flier.capacity = flier_capacity;
        }
        if let Some(operation) = &mut self.operation {
            for drone in &mut operation.drones {
                drone.capacity = drone_capacity;
            }
        }
    }

    /// Where a machine sits *right now*, between its last tick and its next.
    ///
    /// The single source of truth for machine positions on screen. Both the
    /// rigs and the FPV camera go through it, which is what stops a piloted
    /// machine's view from drifting against its own body.
    fn interpolated(&self, from: BlockPos, to: BlockPos) -> Vec3 {
        let fraction = self.tick_fraction();
        let a = Vec3::new(from.x as f32 + 0.5, from.y as f32, from.z as f32 + 0.5);
        let b = Vec3::new(to.x as f32 + 0.5, to.y as f32, to.z as f32 + 0.5);
        a + (b - a) * fraction
    }

    /// Eye height above a machine's ground point, per kind.
    fn eye_height(machine: MachineRef) -> f32 {
        match machine {
            // Roughly the digger's cab and the flier's canopy.
            MachineRef::Digger(_) => 0.8,
            MachineRef::Flier(_) => 0.65,
        }
    }

    /// Where a machine's camera sits this frame.
    pub fn machine_eye(&self, machine: MachineRef) -> Option<Vec3> {
        let base = match machine {
            MachineRef::Digger(index) => {
                let drone = self.operation.as_ref()?.drones.get(index)?;
                self.interpolated(drone.previous_position, drone.position)
            }
            MachineRef::Flier(index) => {
                let flier = self.fleet.fliers.get(index)?;
                self.interpolated(flier.previous_position, flier.position)
            }
        };
        Some(base + Vec3::Y * Self::eye_height(machine))
    }

    /// Where a machine is, in whole blocks.
    pub fn machine_position(&self, machine: MachineRef) -> Option<BlockPos> {
        match machine {
            MachineRef::Digger(index) => {
                Some(self.operation.as_ref()?.drones.get(index)?.position)
            }
            MachineRef::Flier(index) => Some(self.fleet.fliers.get(index)?.position),
        }
    }

    /// The roster the handheld lists, nearest to `from` first.
    pub fn roster(&self, from: Vec3) -> Vec<MachineListing> {
        let mut rows = Vec::new();
        if let Some(operation) = &self.operation {
            for (index, drone) in operation.drones.iter().enumerate() {
                let machine = MachineRef::Digger(index);
                rows.push(MachineListing {
                    machine,
                    name: format!("DIGGER {}", index + 1),
                    state: match drone.state {
                        DroneState::Idle => "IDLE".into(),
                        DroneState::Travelling(_) => "DRIVING".into(),
                        DroneState::Digging(_) => "DIGGING".into(),
                        DroneState::Hauling => "HAULING".into(),
                        DroneState::Stuck => "STUCK".into(),
                        DroneState::Manual => "PILOTED".into(),
                    },
                    distance: self
                        .machine_eye(machine)
                        .map_or(0.0, |at| (at - from).length()),
                    cargo: drone.carrying(),
                    capacity: drone.capacity,
                });
            }
        }
        for (index, flier) in self.fleet.fliers.iter().enumerate() {
            let machine = MachineRef::Flier(index);
            rows.push(MachineListing {
                machine,
                name: format!("FLIER {}", index + 1),
                state: match flier.state {
                    FlierState::Idle => "IDLE".into(),
                    FlierState::Scanning { .. } => "SCANNING".into(),
                    FlierState::ToPickup { .. } => "COLLECTING".into(),
                    FlierState::ToBase => "RETURNING".into(),
                    FlierState::Manual => "PILOTED".into(),
                },
                distance: self
                    .machine_eye(machine)
                    .map_or(0.0, |at| (at - from).length()),
                cargo: flier.carrying(),
                capacity: flier.capacity,
            });
        }
        rows.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        rows
    }

    /// One machine's row, for the feed banner.
    pub fn listing(&self, machine: MachineRef, from: Vec3) -> Option<MachineListing> {
        self.roster(from)
            .into_iter()
            .find(|row| row.machine == machine)
    }

    /// Take the wheel. The machine's own work is suspended, not discarded.
    pub fn take_control(&mut self, machine: MachineRef) -> bool {
        if self.piloted.is_some() {
            return false;
        }
        let taken = match machine {
            MachineRef::Digger(index) => self
                .operation
                .as_mut()
                .is_some_and(|operation| operation.take_control(index)),
            MachineRef::Flier(index) => self.fleet.take_control(index),
        };
        if taken {
            self.piloted = Some(machine);
            self.pilot_command = PilotCommand::default();
        }
        taken
    }

    /// Hand the machine back to its own devices.
    pub fn release_control(&mut self) {
        let Some(machine) = self.piloted.take() else {
            return;
        };
        match machine {
            MachineRef::Digger(index) => {
                if let Some(operation) = &mut self.operation {
                    operation.release_control(index);
                }
            }
            MachineRef::Flier(index) => {
                self.fleet.release_control(index);
            }
        }
        self.pilot_command = PilotCommand::default();
    }

    /// Which machine the player is driving.
    pub fn piloted(&self) -> Option<MachineRef> {
        self.piloted
    }

    /// This frame's held controls.
    pub fn set_pilot_command(&mut self, command: PilotCommand) {
        self.pilot_command = command;
    }

    /// Where the pilot is looking, for the driven machine's nose.
    pub fn set_pilot_look(&mut self, yaw: f32) {
        self.pilot_look = yaw;
    }

    /// The cubes to draw this frame: corner markers and drones.
    pub fn objects(&self) -> Vec<Object> {
        let mut objects = Vec::new();

        if let Some(area) = self.area {
            for corner in area_corners(area) {
                objects.push(Object::box_between(
                    corner - Vec3::splat(MARKER_SIZE * 0.5),
                    corner + Vec3::splat(MARKER_SIZE * 0.5),
                    slot::COPPER_ORE,
                ));
            }
        } else {
            // One corner down: show it on its own, so it is obvious the second
            // click is still owed.
            for corner in &self.corners {
                let centre = Vec3::new(corner.x as f32 + 0.5, corner.y as f32 + 1.0, corner.z as f32 + 0.5);
                objects.push(Object::box_between(
                    centre - Vec3::splat(MARKER_SIZE * 0.5),
                    centre + Vec3::splat(MARKER_SIZE * 0.5),
                    slot::COPPER_ORE,
                ));
            }
        }

        // Machines are rigs, gliding between their tick positions.
        let lerp = |from: BlockPos, to: BlockPos| -> Vec3 { self.interpolated(from, to) };

        if let Some(operation) = &self.operation {
            for (index, drone) in operation.drones.iter().enumerate() {
                let yaw = self.drone_yaws.get(index).copied().unwrap_or(0.0);
                let spin = if matches!(drone.state, DroneState::Digging(_)) {
                    self.spin
                } else {
                    0.0
                };
                objects.extend(self.digger_rig.objects(
                    lerp(drone.previous_position, drone.position),
                    yaw,
                    spin,
                ));
            }
        }

        for (index, flier) in self.fleet.fliers.iter().enumerate() {
            let yaw = self.flier_yaws.get(index).copied().unwrap_or(0.0);
            objects.extend(self.flier_rig.objects(
                lerp(flier.previous_position, flier.position),
                yaw,
                self.spin * 2.5,
            ));
        }

        // Pings hover and read as copper, since copper is what they promise.
        for ping in self.fleet.pings() {
            let centre = Vec3::new(
                ping.position.x as f32 + 0.5,
                ping.position.y as f32 + 1.0,
                ping.position.z as f32 + 0.5,
            );
            objects.push(Object::box_between(
                centre - Vec3::splat(PING_SIZE * 0.5),
                centre + Vec3::splat(PING_SIZE * 0.5),
                slot::COPPER_ORE,
            ));
        }

        objects
    }

    /// A one-line readout for the title bar.
    pub fn status(&self) -> Option<String> {
        // A working flier outranks the mining readout: it is the thing the
        // player just ordered.
        if let Some(machine) = self.piloted {
            let name = match machine {
                MachineRef::Digger(index) => format!("DIGGER {}", index + 1),
                MachineRef::Flier(index) => format!("FLIER {}", index + 1),
            };
            return Some(format!("piloting {name}"));
        }

        if let Some(flier) = self.fleet.fliers.first() {
            match flier.state {
                FlierState::Manual => {}
                FlierState::Scanning { .. } => {
                    return Some(format!(
                        "scanning: {} pings so far",
                        self.fleet.pings().len()
                    ));
                }
                FlierState::ToPickup { .. } | FlierState::ToBase => {
                    return Some(format!("ferrying: {} aboard", flier.carrying()));
                }
                FlierState::Idle => {}
            }
        }

        if let Some(operation) = &self.operation {
            let drone = operation.drones.first()?;
            return Some(format!(
                "mining: {} jobs left, {} hauled, drone {:?}",
                operation.board.len(),
                operation.stockpile.total(),
                drone.state
            ));
        }

        let plan = self.selected_plan()?;
        Some(format!(
            "{} — {} blocks (option {}/{}, Tab to change, Enter to dig)",
            plan.method.name(),
            plan.volume,
            self.chosen + 1,
            self.plans.len()
        ))
    }
}

/// The eight corners of a marked area, in world coordinates.
///
/// Blocks are cells, so the far corner of the box is one block past `max`.
fn area_corners(area: VoxelAabb) -> Vec<Vec3> {
    let lo = Vec3::new(area.min.x as f32, area.min.y as f32, area.min.z as f32);
    let hi = Vec3::new(
        area.max.x as f32 + 1.0,
        area.max.y as f32 + 1.0,
        area.max.z as f32 + 1.0,
    );
    let mut corners = Vec::with_capacity(8);
    for x in [lo.x, hi.x] {
        for y in [lo.y, hi.y] {
            for z in [lo.z, hi.z] {
                corners.push(Vec3::new(x, y, z));
            }
        }
    }
    corners
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with ground near the origin and a fleet with one flier.
    fn flying_world() -> (World, Mining) {
        let mut world = World::new(2024);
        world.load_around(vx_core::ChunkPos::new(0, 0), 2);
        let ground = world.surface_y(0, 0).unwrap_or(80);
        let mut mining = Mining::default();
        mining.ensure_flier(Vec3::new(0.5, ground as f32, 0.5));
        (world, mining)
    }

    #[test]
    fn the_roster_lists_every_machine_nearest_first() {
        let (world, mut mining) = flying_world();
        mining.fleet.add_flier(BlockPos::new(120, 100, 120));
        let _ = &world;

        let rows = mining.roster(Vec3::new(0.5, 80.0, 0.5));
        assert_eq!(rows.len(), 2, "both fliers should be listed");
        assert!(
            rows[0].distance <= rows[1].distance,
            "roster is not nearest first: {:?}",
            rows.iter().map(|row| row.distance).collect::<Vec<_>>()
        );
        assert_eq!(rows[0].name, "FLIER 1");
        assert!(rows.iter().all(|row| !row.state.is_empty()));
    }

    #[test]
    fn a_machine_eye_matches_where_its_rig_is_drawn() {
        // The view and the body must come from one interpolation, or a piloted
        // machine's camera drifts against its own hull between ticks.
        let (_, mut mining) = flying_world();
        mining.fleet.fliers[0].previous_position = BlockPos::new(0, 90, 0);
        mining.fleet.fliers[0].position = BlockPos::new(4, 94, 4);

        for fraction in [0.0f64, 0.25, 0.5, 0.9] {
            mining.pending = fraction;
            let drawn = mining.interpolated(
                mining.fleet.fliers[0].previous_position,
                mining.fleet.fliers[0].position,
            );
            let eye = mining.machine_eye(MachineRef::Flier(0)).unwrap();
            let lift = Mining::eye_height(MachineRef::Flier(0));
            assert!(
                (eye - (drawn + Vec3::Y * lift)).length() < 1.0e-5,
                "eye {eye:?} does not sit on the drawn hull {drawn:?} at fraction {fraction}"
            );
        }
    }

    #[test]
    fn taking_control_of_a_flier_marks_it_piloted_and_hand_back_restores_it() {
        let (_, mut mining) = flying_world();
        assert_eq!(mining.piloted(), None);

        assert!(mining.take_control(MachineRef::Flier(0)));
        assert_eq!(mining.piloted(), Some(MachineRef::Flier(0)));
        assert_eq!(mining.fleet.fliers[0].state, FlierState::Manual);
        assert_eq!(
            mining.roster(Vec3::ZERO)[0].state,
            "PILOTED",
            "the roster should say who has the wheel"
        );
        // One pair of hands.
        assert!(!mining.take_control(MachineRef::Flier(0)));

        mining.release_control();
        assert_eq!(mining.piloted(), None);
        assert_ne!(mining.fleet.fliers[0].state, FlierState::Manual);
    }

    #[test]
    fn a_piloted_machine_takes_its_yaw_from_the_look_not_the_movement() {
        let (_, mut mining) = flying_world();
        mining.take_control(MachineRef::Flier(0));
        // Move it one way, look another.
        mining.fleet.fliers[0].previous_position = BlockPos::new(0, 90, 0);
        mining.fleet.fliers[0].position = BlockPos::new(1, 90, 0);
        mining.set_pilot_look(2.5);
        mining.remember_yaws();
        assert_eq!(mining.flier_yaws[0], 2.5, "the nose ignored the pilot's view");

        // And an autonomous one still follows its own travel.
        mining.release_control();
        mining.remember_yaws();
        assert_ne!(mining.flier_yaws[0], 2.5);
    }

    #[test]
    fn a_piloted_machine_covers_the_same_ground_per_second_as_an_autonomous_one() {
        // Piloting is a change of driver, not of physics: one cell per tick.
        let (mut world, mut mining) = flying_world();
        let events = EventBus::new();
        mining.take_control(MachineRef::Flier(0));
        mining.set_pilot_command(PilotCommand {
            heading: Some(vx_agent::Heading::PosX),
            ..Default::default()
        });

        let start = mining.fleet.fliers[0].position;
        // Two seconds of held input at the fixed tick rate.
        for _ in 0..120 {
            mining.update(&mut world, &events, Duration::from_secs_f32(1.0 / 60.0));
        }
        let travelled = mining.fleet.fliers[0].position.x - start.x;
        let expected = (2.0 * TICK_RATE) as i32;
        assert!(
            (travelled - expected).abs() <= 2,
            "piloted travel was {travelled} cells, the tick rate says about {expected}"
        );
    }

    #[test]
    fn held_pilot_input_is_reissued_once_per_sub_tick_not_once_per_frame() {
        // One long frame must advance the machine as many cells as the ticks
        // it covers, or piloting would stutter with the frame rate.
        let (mut world, mut mining) = flying_world();
        let events = EventBus::new();
        mining.take_control(MachineRef::Flier(0));
        mining.set_pilot_command(PilotCommand {
            heading: Some(vx_agent::Heading::PosX),
            ..Default::default()
        });

        let start = mining.fleet.fliers[0].position;
        // Half a second in a single frame is four ticks at 8 Hz.
        mining.update(&mut world, &events, Duration::from_secs_f32(0.5));
        let travelled = mining.fleet.fliers[0].position.x - start.x;
        assert!(
            travelled >= 3,
            "one long frame only advanced {travelled} cells; input was not re-issued"
        );
    }

    #[test]
    fn a_marked_area_has_eight_distinct_corners() {
        let area = VoxelAabb::new(BlockPos::new(0, 60, 0), BlockPos::new(3, 63, 3));
        let corners = area_corners(area);
        assert_eq!(corners.len(), 8);

        // The box wraps the blocks rather than sitting on their origins, so it
        // spans four blocks, not three.
        let xs: Vec<f32> = corners.iter().map(|corner| corner.x).collect();
        assert_eq!(
            xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - xs.iter().cloned().fold(f32::INFINITY, f32::min),
            4.0
        );
    }

    #[test]
    fn the_first_corner_alone_does_not_make_an_area() {
        let mut world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&mut world, BlockPos::new(0, 60, 0));

        assert!(mining.area.is_none());
        assert!(mining.selected_plan().is_none());
        // Still worth drawing, so the player can see the click landed.
        assert_eq!(mining.objects().len(), 1);
    }

    #[test]
    fn a_third_mark_starts_a_fresh_area() {
        // Otherwise a mis-click would need a separate cancel key to undo.
        let mut world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&mut world, BlockPos::new(0, 60, 0));
        mining.mark(&mut world, BlockPos::new(3, 63, 3));
        assert!(mining.area.is_some());

        mining.mark(&mut world, BlockPos::new(10, 60, 10));
        assert!(mining.area.is_none());
        assert_eq!(mining.corners, vec![BlockPos::new(10, 60, 10)]);
    }

    #[test]
    fn cycling_wraps_around_the_available_methods() {
        let mut mining = Mining::default();
        // No plans: cycling must not divide by zero or panic.
        mining.cycle_method();
        assert_eq!(mining.chosen, 0);
    }

    #[test]
    fn cancelling_clears_everything() {
        let mut world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&mut world, BlockPos::new(0, 60, 0));
        mining.mark(&mut world, BlockPos::new(3, 63, 3));

        mining.cancel(&mut world);
        assert!(mining.area.is_none());
        assert!(mining.objects().is_empty());
        assert!(!mining.is_running());
    }

    #[test]
    fn a_long_stall_does_not_run_hundreds_of_ticks_at_once() {
        // Catch-up has to be capped, or the frame after a window drag runs the
        // whole excavation in one go and the drone appears to teleport.
        let mut mining = Mining::default();
        let mut world = World::new(1);
        let events = EventBus::new();
        // No operation running — but the fleet still metes out its ticks
        // through the same accumulator, so the invariant is the clamp, not
        // emptiness.
        mining.update(&mut world, &events, Duration::from_secs(30));
        assert!(mining.pending <= 1.0, "backlog survived the stall frame");
    }

    #[test]
    fn the_backlog_from_a_stall_is_discarded_not_drained() {
        // Review finding A4: the cap alone kept the backlog and drained it at
        // 16 ticks per frame — a ~120x fast-forward for seconds after a stall.
        // Time lost to a stall must stay lost.
        let mut mining = Mining {
            operation: Some(Operation::new(BlockPos::new(0, 61, 0))),
            ..Mining::default()
        };

        let mut world = World::new(1);
        let events = EventBus::new();
        mining.update(&mut world, &events, Duration::from_secs(30));

        assert!(
            mining.pending <= 1.0,
            "{} ticks still queued after the stall frame; the next frames will fast-forward",
            mining.pending
        );
    }
}
