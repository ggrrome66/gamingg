//! One ground drone: where it is, what it is doing, what it is carrying.
//!
//! # Whole blocks per tick
//!
//! The drone moves a block at a time rather than carrying a velocity. The
//! simulation is a discrete voxel problem — which cell, which block to dig —
//! and pretending otherwise would mean floating-point movement that then has to
//! be rounded back to cells anyway, with the rounding as a new source of bugs.
//! [`Drone::previous_position`] is kept so the renderer can interpolate between
//! ticks and the drone still looks like it glides.
//!
//! # It cuts ahead and above, and descends only through its own floor
//!
//! Reach covers the cells level with the drone and the layer above it — never
//! diagonally below. That one restriction is what keeps an excavation
//! self-consistent, and it took three wrong answers to find:
//!
//! - Cutting a block leaves anything resting on it hanging in the air, since
//!   blocks do not fall. Taking the layer above first, always, means nothing is
//!   ever left floating.
//! - Cutting diagonally *down* carves a full-height notch beside the drone and
//!   destroys the floor of every cell in it — including the cells the drone
//!   needed to stand in to advance. It then sees the next block along, cannot
//!   get to it, and hands the job back forever.
//! - Working the whole region top-down instead fixes both but cannot cut a
//!   level tunnel at all: the roof is two blocks up, and nothing can stand
//!   inside solid rock to reach it.
//!
//! Descending is by undermining — cutting the block directly underfoot, which
//! is allowed only when there is solid ground under *that*, so the drop is
//! exactly one block and the step back up exists. Every hole it makes for
//! itself, it can climb out of.

use vx_core::BlockPos;
use vx_world::World;

use crate::job::{DroneId, JobId};
use crate::stockpile::Stockpile;

/// What a drone is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroneState {
    /// Nothing to do: the board is empty or nothing is reachable.
    Idle,
    /// On the way to a job.
    Travelling(JobId),
    /// At the face, cutting.
    Digging(JobId),
    /// Full, heading for the stockpile.
    Hauling,
    /// Cannot reach its work and has given it back to the board.
    Stuck,
}

/// A single ground drone.
#[derive(Debug, Clone)]
pub struct Drone {
    pub id: DroneId,
    pub position: BlockPos,
    /// Where it was last tick, for the renderer to interpolate from.
    pub previous_position: BlockPos,
    pub state: DroneState,
    /// What is aboard, by block name.
    ///
    /// A [`Stockpile`] rather than a bare count, so waste rock and ore stay
    /// distinguishable all the way from the face to the pile — and so nothing
    /// keyed on a [`vx_core::BlockId`] can survive a mod shifting the ids.
    pub cargo: Stockpile,
    /// Blocks it can carry before it has to run them home.
    pub capacity: u64,
    /// Blocks of run per block of rise it needs on a ramp. A drone stat, and
    /// the number the mine planners generate excavations for.
    pub grade: i32,
    pub(crate) job: Option<JobId>,
}

/// Carry capacity of a starting drone. Small enough that the haul back matters,
/// large enough that a body is not a hundred round trips.
pub const DEFAULT_CAPACITY: u64 = 64;

/// Grade a starting drone needs: three blocks of run per block of drop.
pub const DEFAULT_GRADE: i32 = 3;

impl Drone {
    pub fn new(id: DroneId, position: BlockPos) -> Self {
        Drone {
            id,
            position,
            previous_position: position,
            state: DroneState::Idle,
            cargo: Stockpile::new(),
            capacity: DEFAULT_CAPACITY,
            grade: DEFAULT_GRADE,
            job: None,
        }
    }

    /// Blocks aboard, of every kind.
    pub fn carrying(&self) -> u64 {
        self.cargo.total()
    }

    pub fn is_full(&self) -> bool {
        self.carrying() >= self.capacity
    }

    pub fn job(&self) -> Option<JobId> {
        self.job
    }

    /// Move to `to`, remembering where it came from.
    pub(crate) fn move_to(&mut self, to: BlockPos) {
        self.previous_position = self.position;
        self.position = to;
    }

    /// Offsets from a drone to the blocks it can cut, highest first.
    ///
    /// The layer above first, then the drone's own layer. Nothing below: see
    /// the module note for why that restriction is load-bearing. The block
    /// directly underfoot is handled separately by [`Drone::may_undermine`],
    /// because whether it is safe depends on the world rather than on geometry.
    ///
    /// Above before level matters within the list too: cutting a block at the
    /// drone's own level would leave whatever sits on it hanging, so the thing
    /// sitting on it goes first.
    ///
    /// This list is the single source of truth for reach. [`Drone::reach`]
    /// applies it forwards to find blocks; [`stations_for`] applies it
    /// backwards to find the cells a given block can be cut *from*. Two
    /// separate versions of the same rule would drift, and the symptom would be
    /// a drone standing next to a block it was sent to dig and refusing to.
    pub fn reach_offsets() -> Vec<[i32; 3]> {
        let mut offsets = Vec::with_capacity(17);
        for dy in [1, 0] {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if [dx, dy, dz] == [0, 0, 0] {
                        continue;
                    }
                    offsets.push([dx, dy, dz]);
                }
            }
        }
        offsets
    }

    /// May the block directly underfoot be cut?
    ///
    /// Only when there is solid ground immediately below it. Then removing it
    /// drops the drone exactly one block, which is a step it can climb back up.
    /// Undermining into open space is a fall it may not be able to climb out
    /// of, and that is how a drone is lost at the bottom of its own hole.
    pub fn may_undermine(&self, world: &World) -> bool {
        world.is_solid(self.position.offset([0, -2, 0]))
    }

    /// Blocks this drone can cut from where it stands, in cutting order.
    pub fn reach(&self, world: &World) -> Vec<BlockPos> {
        let mut cells: Vec<BlockPos> = Self::reach_offsets()
            .into_iter()
            .map(|offset| self.position.offset(offset))
            .collect();
        // Last resort, after everything above and beside it.
        if self.may_undermine(world) {
            cells.push(self.position.offset([0, -1, 0]));
        }
        cells
    }

    /// The first block in reach that `wanted` accepts, if any.
    pub fn target_in_reach(
        &self,
        world: &World,
        wanted: impl Fn(BlockPos) -> bool,
    ) -> Option<BlockPos> {
        self.reach(world)
            .into_iter()
            .find(|pos| world.is_solid(*pos) && wanted(*pos))
    }

    /// The next block to cut inside `region` from where the drone stands.
    ///
    /// Everything level with it and above first. Undermining is the last
    /// resort and carries one extra condition: **nothing in the region may
    /// still be left above the drone's own level.**
    ///
    /// That condition is the difference between an excavation that finishes and
    /// one that strands itself. Dropping a level removes the floor of the level
    /// above, so anything still up there loses every cell a drone could stand
    /// in to reach it — a block in plain sight that nothing can ever touch. The
    /// scan it costs is a whole-region sweep, but it only runs on the tick where
    /// the drone has genuinely run out of things to cut in front of it.
    pub fn next_cut(&self, world: &World, region: &crate::aabb::VoxelAabb) -> Option<BlockPos> {
        let ahead = Self::reach_offsets()
            .into_iter()
            .map(|offset| self.position.offset(offset))
            .find(|pos| region.contains(*pos) && world.is_solid(*pos));
        if ahead.is_some() {
            return ahead;
        }

        let below = self.position.offset([0, -1, 0]);
        if !region.contains(below) || !world.is_solid(below) || !self.may_undermine(world) {
            return None;
        }
        let work_above = region
            .clamped_to_world()
            .blocks()
            .any(|pos| pos.y > self.position.y && world.is_solid(pos));
        (!work_above).then_some(below)
    }
}

/// Cells a drone could stand in and cut `block` from.
///
/// The inverse of [`Drone::reach_offsets`], and the reason it is shared: this
/// is what tells a drone *where to drive to* in order to dig something, so if
/// the two disagreed a drone would arrive somewhere it cannot work from and
/// hand the job straight back.
pub fn stations_for(world: &World, block: BlockPos) -> Vec<BlockPos> {
    let mut cells: Vec<BlockPos> = Drone::reach_offsets()
        .into_iter()
        .map(|[dx, dy, dz]| block.offset([-dx, -dy, -dz]))
        .collect();
    // Standing on top of it counts only where undermining would be safe, which
    // is the one case the offset list cannot decide by itself.
    if world.is_solid(block.offset([0, -1, 0])) {
        cells.push(block.offset([0, 1, 0]));
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drone() -> Drone {
        Drone::new(DroneId(0), BlockPos::new(10, 64, -3))
    }

    #[test]
    fn a_new_drone_is_idle_and_empty() {
        let drone = drone();
        assert_eq!(drone.state, DroneState::Idle);
        assert_eq!(drone.carrying(), 0);
        assert!(!drone.is_full());
        assert!(drone.job().is_none());
        assert_eq!(drone.previous_position, drone.position);
    }

    #[test]
    fn moving_records_where_it_came_from() {
        // What the renderer interpolates along. Losing it makes drones teleport
        // between ticks.
        let mut drone = drone();
        let from = drone.position;
        let to = from.offset([1, 0, 0]);
        drone.move_to(to);

        assert_eq!(drone.position, to);
        assert_eq!(drone.previous_position, from);
    }

    /// Solid stone below `floor`, air above, so undermining is safe.
    fn bedrock_world(floor: i32) -> World {
        crate::fixture::flat(1, floor)
    }

    /// One solid block at `shelf` with nothing under it: the case where
    /// undermining is a fall rather than a step.
    fn shelf_world(floor: i32, shelf: i32) -> World {
        let mut world = bedrock_world(floor);
        let stone = world.registry().id_of("engine:stone").unwrap();
        world.set_block(BlockPos::new(0, shelf, 0), stone);
        world
    }

    #[test]
    fn reach_covers_the_drones_own_layer_and_the_one_above_it() {
        // Two 3x3 layers less the drone's own cell. Nothing below at all: that
        // restriction is what keeps excavations self-consistent.
        let offsets = Drone::reach_offsets();
        assert_eq!(offsets.len(), 17);
        assert!(!offsets.contains(&[0, 0, 0]), "it can dig its own cell");
        assert!(
            offsets.iter().all(|offset| offset[1] >= 0),
            "reach dips below the drone: {offsets:?}"
        );
        assert!(offsets.contains(&[0, 1, 0]), "it cannot dig straight up");
    }

    #[test]
    fn reach_excludes_the_diagonal_below_which_is_what_carves_notches() {
        // Cutting down-and-across looks like the natural way to descend, and it
        // is exactly what strands a drone: it removes the floor of every cell in
        // the notch, including the ones needed to advance the face.
        let offsets = Drone::reach_offsets();
        assert!(!offsets.contains(&[1, -1, 0]));
        assert!(!offsets.contains(&[0, -1, 1]));
    }

    #[test]
    fn reach_is_ordered_from_the_top_down() {
        let heights: Vec<i32> = Drone::reach_offsets().iter().map(|offset| offset[1]).collect();
        assert!(
            heights.windows(2).all(|pair| pair[0] >= pair[1]),
            "reach is not ordered highest first: a face would be cut from underneath"
        );
    }

    #[test]
    fn undermining_is_allowed_over_solid_ground_and_refused_over_a_void() {
        // One block of undermining is a step down. Undermining into open space
        // is a fall, and possibly a lost drone.
        let world = bedrock_world(60);
        let drone = Drone::new(DroneId(0), BlockPos::new(0, 40, 0));
        assert!(drone.may_undermine(&world), "solid rock below should be safe");
        assert!(drone.reach(&world).contains(&BlockPos::new(0, 39, 0)));

        // Standing on a one-block-thick shelf ten blocks over the ground.
        let world = shelf_world(60, 70);
        let drone = Drone::new(DroneId(0), BlockPos::new(0, 71, 0));
        assert!(!drone.may_undermine(&world), "the shelf is the only floor");
        assert!(
            !drone.reach(&world).contains(&BlockPos::new(0, 70, 0)),
            "it is willing to cut the shelf out from under itself"
        );
    }

    #[test]
    fn undermining_comes_last_so_the_face_is_cut_from_above_first() {
        let world = bedrock_world(60);
        let drone = Drone::new(DroneId(0), BlockPos::new(0, 40, 0));
        let reach = drone.reach(&world);
        assert_eq!(reach.last(), Some(&BlockPos::new(0, 39, 0)));
    }

    #[test]
    fn a_block_can_be_cut_from_exactly_the_cells_that_reach_it() {
        // The pairing that keeps "drive there" and "dig that" agreeing. If a
        // station is not a cell that reaches the block, a drone arrives and
        // refuses to work; if a reachable cell is not a station, it is never
        // sent there in the first place.
        let world = bedrock_world(60);
        let block = BlockPos::new(4, 40, 4);

        for station in super::stations_for(&world, block) {
            let drone = Drone::new(DroneId(0), station);
            assert!(
                drone.reach(&world).contains(&block),
                "a drone at {station:?} is sent to dig {block:?} but cannot reach it"
            );
        }
    }

    #[test]
    fn every_cell_that_reaches_a_block_is_listed_as_a_station() {
        let world = bedrock_world(60);
        let block = BlockPos::new(4, 40, 4);
        let stations = super::stations_for(&world, block);

        for dy in -2..=2 {
            for dz in -2..=2 {
                for dx in -2..=2 {
                    let candidate = block.offset([dx, dy, dz]);
                    let drone = Drone::new(DroneId(0), candidate);
                    if drone.reach(&world).contains(&block) {
                        assert!(
                            stations.contains(&candidate),
                            "{candidate:?} reaches {block:?} but is not offered as a station"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn capacity_decides_when_it_is_full() {
        let mut drone = drone();
        drone.capacity = 3;
        drone.cargo.add("engine:stone", 2);
        assert!(!drone.is_full());
        drone.cargo.add("engine:copper_ore", 1);
        assert!(drone.is_full(), "capacity counts every kind aboard, not one");
    }
}
