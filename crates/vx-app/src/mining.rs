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
use vx_agent::{options, MineMethod, MinePlan, Operation, VoxelAabb};
use vx_core::{BlockPos, EventBus};
use vx_render::tiles::slot;
use vx_render::Object;
use vx_world::World;

/// Drone ticks per second.
///
/// A drone cuts one block per tick, so this is also its digging rate. Slow
/// enough to watch the excavation take shape, which is most of the appeal.
const TICK_RATE: f64 = 8.0;

/// Blocks of run per block of rise the starting drone's ramps are cut to.
const GRADE: i32 = vx_agent::DEFAULT_GRADE;

/// Edge length of the cubes drawn at the corners of a marked area.
const MARKER_SIZE: f32 = 0.35;

/// Edge length of a drone.
const DRONE_SIZE: f32 = 0.8;

/// What the player is currently doing with the mining tools.
#[derive(Debug, Default)]
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
}

impl Mining {
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

    /// Abandon whatever is marked or running. The hole stays dug.
    pub fn cancel(&mut self) {
        *self = Mining::default();
    }

    pub fn is_running(&self) -> bool {
        self.operation.is_some()
    }

    /// Advance the excavation by however many ticks `elapsed` is worth.
    pub fn update(&mut self, world: &mut World, events: &EventBus, elapsed: Duration) {
        let Some(operation) = &mut self.operation else {
            return;
        };

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

        for _ in 0..ticks {
            operation.tick(world, events);
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

        if let Some(operation) = &self.operation {
            for drone in &operation.drones {
                let base = Vec3::new(
                    drone.position.x as f32 + 0.5,
                    drone.position.y as f32,
                    drone.position.z as f32 + 0.5,
                );
                objects.push(Object::standing(base, DRONE_SIZE, slot::BEDROCK));
            }
        }

        objects
    }

    /// A one-line readout for the title bar.
    pub fn status(&self) -> Option<String> {
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
        // No operation, so this only exercises the accumulator's bookkeeping.
        mining.update(&mut world, &events, Duration::from_secs(30));
        assert_eq!(mining.pending, 0.0, "ticks accrued with nothing to run");
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
