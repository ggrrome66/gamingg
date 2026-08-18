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
use vx_agent::{options, DroneState, Fleet, FlierState, MineMethod, MinePlan, Operation, Sector, VoxelAabb};
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
    drone_yaws: Vec<f32>,
    flier_yaws: Vec<f32>,
    /// Drill/rotor angle, advanced while machines work.
    spin: f32,
    /// The rig shapes, built once.
    digger_rig: Rig,
    flier_rig: Rig,
}

impl Default for Mining {
    fn default() -> Self {
        Mining {
            corners: Vec::new(),
            area: None,
            plans: Vec::new(),
            chosen: 0,
            operation: None,
            pending: 0.0,
            fleet: Fleet::default(),
            drone_yaws: Vec::new(),
            flier_yaws: Vec::new(),
            spin: 0.0,
            digger_rig: Rig::digger(),
            flier_rig: Rig::flier(),
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
    pub fn mark(&mut self, world: &World, pos: BlockPos) {
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
    pub fn start(&mut self, world: &World) -> Option<MineMethod> {
        let plan = self.plans.get(self.chosen)?.clone();
        let start = vx_agent::settle(world, plan.portal);

        let mut operation = Operation::new(start);
        operation.add_drone(start);
        operation.post_plan(&plan);
        self.operation = Some(operation);
        Some(plan.method)
    }

    /// Abandon whatever is marked or running. The hole stays dug, and the
    /// fleet — the flier, its surveys, the base — is not the plan's to throw
    /// away, so it survives.
    pub fn cancel(&mut self) {
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

        let mut report = FleetReport::default();
        if self.operation.is_none() {
            // The fleet still flies with no dig running.
            self.pending += elapsed.as_secs_f64() * TICK_RATE;
            let ticks = (self.pending as u32).min(16);
            self.pending -= f64::from(ticks);
            self.pending = self.pending.min(1.0);
            for _ in 0..ticks {
                let tick = self.fleet.tick(world, &mut []);
                report.delivered += tick.delivered;
                report.sectors_completed += tick.sectors_completed;
                report.pings_found += tick.pings_found;
            }
            self.remember_yaws();
            return report;
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

        {
            let operation = self.operation.as_mut().expect("checked above");
            for _ in 0..ticks {
                operation.tick(world, events);
            }
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

    /// Update remembered nose directions from this tick's movement.
    fn remember_yaws(&mut self) {
        if let Some(operation) = &self.operation {
            self.drone_yaws.resize(operation.drones.len(), 0.0);
            for (yaw, drone) in self.drone_yaws.iter_mut().zip(&operation.drones) {
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
        for (yaw, flier) in self.flier_yaws.iter_mut().zip(&self.fleet.fliers) {
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
        let fraction = self.tick_fraction();
        let lerp = |from: BlockPos, to: BlockPos| -> Vec3 {
            let a = Vec3::new(from.x as f32 + 0.5, from.y as f32, from.z as f32 + 0.5);
            let b = Vec3::new(to.x as f32 + 0.5, to.y as f32, to.z as f32 + 0.5);
            a + (b - a) * fraction
        };

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
        if let Some(flier) = self.fleet.fliers.first() {
            match flier.state {
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
        let world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&world, BlockPos::new(0, 60, 0));

        assert!(mining.area.is_none());
        assert!(mining.selected_plan().is_none());
        // Still worth drawing, so the player can see the click landed.
        assert_eq!(mining.objects().len(), 1);
    }

    #[test]
    fn a_third_mark_starts_a_fresh_area() {
        // Otherwise a mis-click would need a separate cancel key to undo.
        let world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&world, BlockPos::new(0, 60, 0));
        mining.mark(&world, BlockPos::new(3, 63, 3));
        assert!(mining.area.is_some());

        mining.mark(&world, BlockPos::new(10, 60, 10));
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
        let world = World::new(1);
        let mut mining = Mining::default();
        mining.mark(&world, BlockPos::new(0, 60, 0));
        mining.mark(&world, BlockPos::new(3, 63, 3));

        mining.cancel();
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
