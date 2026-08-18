//! Breadth-first distance fields over standable ground.
//!
//! # One field, many drones
//!
//! Pathfinding per drone per tick is what makes swarms unaffordable. A flow
//! field inverts it: one breadth-first sweep outward from the goal labels every
//! cell with its distance, and any number of drones then just step downhill.
//! Rebuilding is tied to the goal changing, not to how many drones are asking.
//!
//! # What "traversable" means for something that drives
//!
//! A ground drone occupies one block and needs floor under it, so a cell is
//! *standable* when the cell itself is clear and the block beneath is solid.
//! Moving between neighbouring columns is allowed while the floor changes by at
//! most one block, in either direction.
//!
//! **Climb and drop are deliberately equal.** A machine that could fall three
//! blocks but only climb one would happily strand itself at the bottom of its
//! own excavation, and the failure would appear hours later as a drone that
//! never came home. Keeping them symmetric means every route in is a route out.
//!
//! That one-block limit is also what the mine planners in [`crate::mine`] are
//! generating ramps *for*: a plan whose steps exceed it produces an excavation
//! no drone can drive, which is exactly what the reachability tests catch.

use std::collections::VecDeque;

use vx_core::BlockPos;
use vx_world::World;

use crate::aabb::VoxelAabb;

/// The distance recorded for a cell no route reaches.
pub const UNREACHABLE: u32 = u32::MAX;

/// Blocks of height change a ground drone can manage in one step, up or down.
pub const STEP: i32 = 1;

/// Can a drone stand here?
///
/// Needs clear space at `pos` and something solid directly beneath. Unloaded
/// chunks read as air, so a cell over unloaded ground is correctly not
/// standable rather than a hole a drone walks into.
pub fn is_standable(world: &World, pos: BlockPos) -> bool {
    if !pos.in_vertical_bounds() || pos.y == 0 {
        return false;
    }
    !world.is_solid(pos) && world.is_solid(pos.offset([0, -1, 0]))
}

/// The cell a drone at `pos` settles into once gravity has its say.
///
/// Digging out from under something is the common way a drone ends up floating,
/// and dropping it to the first solid footing below is both what would really
/// happen and what keeps the simulation from wedging.
pub fn settle(world: &World, pos: BlockPos) -> BlockPos {
    let mut at = pos;
    while at.y > 1 && !world.is_solid(at.offset([0, -1, 0])) {
        at = at.offset([0, -1, 0]);
    }
    at
}

/// The four horizontal directions. No diagonals: a diagonal step between two
/// solid blocks squeezes through a corner nothing could physically pass.
const DIRECTIONS: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

/// Standable cells reachable from `pos` in one step.
///
/// **Every** standable cell within `±STEP` in each neighbouring column, not
/// just the topmost. Taking only the first found was a bug worth remembering:
/// under an overhang a column holds standable cells at several heights, and
/// "topmost only" made edges one-directional — A could step to B while B's
/// search in A's column found a different cell, so BFS labelled cells reachable
/// that could never be stepped to, and routes silently stopped short of their
/// goals. Returning all candidates makes every edge symmetric by construction
/// (|dy| ≤ STEP reads the same from both ends), which restores the invariant
/// the module rests on: labelled reachable means walkable, and every route in
/// is a route out.
pub fn neighbours(world: &World, pos: BlockPos) -> impl Iterator<Item = BlockPos> + '_ {
    DIRECTIONS.into_iter().flat_map(move |direction| {
        let column = pos.offset(direction);
        // Highest first, so ties in `step_from` still prefer stepping up onto
        // a ledge over walking under it.
        (-STEP..=STEP)
            .rev()
            .map(move |dy| column.offset([0, dy, 0]))
            .filter(|candidate| is_standable(world, *candidate))
    })
}

/// Distance-to-goal for every cell in a bounded region.
///
/// The bounds are the caller's cost control: the sweep visits every cell inside
/// them, so a field spanning a whole world would be as slow as it sounds.
/// Callers build fields around a work site, not around the map.
#[derive(Debug, Clone)]
pub struct FlowField {
    bounds: VoxelAabb,
    distance: Vec<u32>,
}

impl FlowField {
    /// Sweep outward from `goals`, labelling every reachable cell.
    ///
    /// Goals outside `bounds` are ignored rather than being an error: a goal
    /// can be dug away between the caller choosing it and the field being
    /// rebuilt, and a swarm should slow down, not panic.
    pub fn build(world: &World, bounds: VoxelAabb, goals: impl IntoIterator<Item = BlockPos>) -> Self {
        let bounds = bounds.clamped_to_world();
        let mut field = FlowField {
            distance: vec![UNREACHABLE; bounds.volume() as usize],
            bounds,
        };

        let mut queue = VecDeque::new();
        for goal in goals {
            let Some(index) = field.index(goal) else {
                continue;
            };
            if !is_standable(world, goal) || field.distance[index] == 0 {
                continue;
            }
            field.distance[index] = 0;
            queue.push_back(goal);
        }

        while let Some(current) = queue.pop_front() {
            let steps = field.distance[field.index(current).expect("queued cell is in bounds")];
            for next in neighbours(world, current) {
                let Some(index) = field.index(next) else {
                    continue;
                };
                if field.distance[index] != UNREACHABLE {
                    continue;
                }
                field.distance[index] = steps + 1;
                queue.push_back(next);
            }
        }

        field
    }

    fn index(&self, pos: BlockPos) -> Option<usize> {
        if !self.bounds.contains(pos) {
            return None;
        }
        let [width, _, depth] = self.bounds.size();
        let x = (pos.x - self.bounds.min.x) as i64;
        let y = (pos.y - self.bounds.min.y) as i64;
        let z = (pos.z - self.bounds.min.z) as i64;
        Some(((y * depth + z) * width + x) as usize)
    }

    pub fn bounds(&self) -> VoxelAabb {
        self.bounds
    }

    /// Steps from `pos` to the nearest goal, or `None` when no route exists or
    /// the cell is outside the field.
    pub fn distance(&self, pos: BlockPos) -> Option<u32> {
        let index = self.index(pos)?;
        (self.distance[index] != UNREACHABLE).then_some(self.distance[index])
    }

    pub fn is_reachable(&self, pos: BlockPos) -> bool {
        self.distance(pos).is_some()
    }

    /// Cells with a route to a goal.
    pub fn reachable_count(&self) -> usize {
        self.distance.iter().filter(|d| **d != UNREACHABLE).count()
    }

    /// The next cell on the way to the goal, or `None` if already there.
    pub fn step_from(&self, world: &World, pos: BlockPos) -> Option<BlockPos> {
        let here = self.distance(pos)?;
        if here == 0 {
            return None;
        }
        neighbours(world, pos)
            .filter_map(|next| self.distance(next).map(|steps| (steps, next)))
            .filter(|(steps, _)| *steps < here)
            // Ties broken by position so a field gives the same route every
            // run: a swarm that wandered differently between runs would make
            // determinism tests meaningless.
            .min_by_key(|(steps, next)| (*steps, next.x, next.y, next.z))
            .map(|(_, next)| next)
    }

    /// The whole route from `pos` to a goal, goal included.
    pub fn path_from(&self, world: &World, pos: BlockPos) -> Option<Vec<BlockPos>> {
        self.distance(pos)?;
        let mut route = vec![pos];
        let mut at = pos;
        while let Some(next) = self.step_from(world, at) {
            route.push(next);
            at = next;
        }
        Some(route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockId, ChunkPos};

    /// A flat slab of stone with air above, from y=0 up to and including
    /// `floor`. Everything above `floor` is cleared, so the standable layer is
    /// `floor + 1`.
    fn flat_world(radius: i32, floor: i32) -> (World, i32) {
        let mut world = World::new(1);
        world.load_around(ChunkPos::new(0, 0), radius);
        let stone = world.registry().id_of("engine:stone").unwrap();

        let span = radius * 16 + 16;
        for x in -span..span {
            for z in -span..span {
                for y in 0..=floor {
                    world.set_block(BlockPos::new(x, y, z), stone);
                }
                for y in (floor + 1)..(floor + 8) {
                    world.set_block(BlockPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        (world, floor + 1)
    }

    fn wall(world: &mut World, box_: VoxelAabb) {
        let stone = world.registry().id_of("engine:stone").unwrap();
        for pos in box_.blocks() {
            world.set_block(pos, stone);
        }
    }

    #[test]
    fn standing_needs_clear_space_over_solid_ground() {
        let (world, ground) = flat_world(1, 60);
        assert!(is_standable(&world, BlockPos::new(0, ground, 0)));
        // Inside the rock: no clear space.
        assert!(!is_standable(&world, BlockPos::new(0, ground - 1, 0)));
        // In the air with nothing beneath.
        assert!(!is_standable(&world, BlockPos::new(0, ground + 3, 0)));
    }

    #[test]
    fn unloaded_ground_is_not_standable() {
        // Unloaded chunks read as air. Treating that as walkable would send
        // drones off the edge of the loaded world.
        let (world, ground) = flat_world(1, 60);
        assert!(!is_standable(&world, BlockPos::new(9_000, ground, 9_000)));
    }

    #[test]
    fn distance_grows_with_the_walk() {
        let (world, ground) = flat_world(1, 60);
        let goal = BlockPos::new(0, ground, 0);
        let bounds = VoxelAabb::new(BlockPos::new(-12, ground - 2, -12), BlockPos::new(12, ground + 2, 12));
        let field = FlowField::build(&world, bounds, [goal]);

        assert_eq!(field.distance(goal), Some(0));
        assert_eq!(field.distance(BlockPos::new(3, ground, 0)), Some(3));
        // Manhattan, because movement is four-connected.
        assert_eq!(field.distance(BlockPos::new(3, ground, 4)), Some(7));
    }

    #[test]
    fn following_the_field_descends_every_single_step() {
        // The property the whole thing rests on: a drone that always takes the
        // step the field offers must reach the goal, never loop, never stall.
        let (world, ground) = flat_world(1, 60);
        let goal = BlockPos::new(-4, ground, 5);
        let bounds = VoxelAabb::new(BlockPos::new(-14, ground - 2, -14), BlockPos::new(14, ground + 2, 14));
        let field = FlowField::build(&world, bounds, [goal]);

        let start = BlockPos::new(7, ground, -6);
        let route = field.path_from(&world, start).expect("no route on flat ground");

        assert_eq!(route.first(), Some(&start));
        assert_eq!(route.last(), Some(&goal));
        let distances: Vec<u32> = route.iter().map(|pos| field.distance(*pos).unwrap()).collect();
        assert!(
            distances.windows(2).all(|pair| pair[1] + 1 == pair[0]),
            "the route does not descend one step at a time: {distances:?}"
        );
        assert_eq!(route.len() as u32, field.distance(start).unwrap() + 1);
    }

    #[test]
    fn a_route_goes_around_an_obstacle_rather_than_through_it() {
        let (mut world, ground) = flat_world(1, 60);
        // A wall across the direct line, with a gap at one end.
        wall(
            &mut world,
            VoxelAabb::new(
                BlockPos::new(0, ground, -8),
                BlockPos::new(0, ground + 2, 3),
            ),
        );

        let goal = BlockPos::new(-6, ground, 0);
        let start = BlockPos::new(6, ground, 0);
        let bounds = VoxelAabb::new(
            BlockPos::new(-14, ground - 2, -14),
            BlockPos::new(14, ground + 4, 14),
        );
        let field = FlowField::build(&world, bounds, [goal]);

        let route = field.path_from(&world, start).expect("the gap should be usable");
        assert!(
            route.iter().all(|pos| !world.is_solid(*pos)),
            "the route passes through solid rock"
        );
        // Detouring round the wall is longer than the 12-step straight line.
        assert!(
            route.len() > 13,
            "route of {} steps is too short to have gone around the wall",
            route.len()
        );
    }

    #[test]
    fn cells_walled_off_entirely_are_unreachable() {
        let (mut world, ground) = flat_world(1, 60);
        // Box a cell in on all four sides.
        for offset in [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
            wall(
                &mut world,
                VoxelAabb::new(
                    BlockPos::new(5, ground, 5).offset(offset),
                    BlockPos::new(5, ground + 2, 5).offset(offset),
                ),
            );
        }

        let bounds = VoxelAabb::new(
            BlockPos::new(-10, ground - 2, -10),
            BlockPos::new(12, ground + 4, 12),
        );
        let field = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);

        assert!(!field.is_reachable(BlockPos::new(5, ground, 5)));
        assert!(field.is_reachable(BlockPos::new(4, ground, 4)));
        assert!(field.path_from(&world, BlockPos::new(5, ground, 5)).is_none());
    }

    #[test]
    fn a_single_step_up_is_climbable_but_two_are_not() {
        // The constant that decides whether an excavation is drivable. If this
        // ever changes, every ramp generator in `mine` has to change with it.
        let (mut world, ground) = flat_world(1, 60);
        wall(
            &mut world,
            VoxelAabb::new(BlockPos::new(2, ground, -6), BlockPos::new(2, ground, 6)),
        );
        wall(
            &mut world,
            VoxelAabb::new(BlockPos::new(6, ground, -6), BlockPos::new(6, ground + 1, 6)),
        );

        let bounds = VoxelAabb::new(
            BlockPos::new(-4, ground - 2, -8),
            BlockPos::new(10, ground + 6, 8),
        );
        let field = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);

        // One block up: reachable, standing on top of the low wall.
        assert!(
            field.is_reachable(BlockPos::new(2, ground + 1, 0)),
            "a one-block step should be climbable"
        );
        // Two blocks up: not.
        assert!(
            !field.is_reachable(BlockPos::new(6, ground + 2, 0)),
            "a two-block step should stop a ground drone"
        );
    }

    #[test]
    fn climbing_and_dropping_have_the_same_limit() {
        // Asymmetry here is how drones strand themselves at the bottom of a
        // hole they drove into. Every reachable cell must be able to get back.
        let (mut world, ground) = flat_world(1, 60);
        wall(
            &mut world,
            VoxelAabb::new(BlockPos::new(3, ground, -4), BlockPos::new(5, ground, 4)),
        );

        let bounds = VoxelAabb::new(
            BlockPos::new(-8, ground - 2, -8),
            BlockPos::new(10, ground + 6, 8),
        );
        let outward = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);
        let homeward_goal = BlockPos::new(4, ground + 1, 0);
        let homeward = FlowField::build(&world, bounds, [homeward_goal]);

        assert!(outward.is_reachable(homeward_goal));
        assert!(
            homeward.is_reachable(BlockPos::new(0, ground, 0)),
            "a cell reachable outward is not reachable back: drones will strand"
        );
    }

    #[test]
    fn a_goal_that_is_not_standable_is_ignored() {
        let (world, ground) = flat_world(1, 60);
        let bounds = VoxelAabb::new(
            BlockPos::new(-6, ground - 2, -6),
            BlockPos::new(6, ground + 2, 6),
        );
        // Mid-air, and inside the rock: neither is somewhere to walk to.
        let field = FlowField::build(
            &world,
            bounds,
            [BlockPos::new(0, ground + 3, 0), BlockPos::new(0, ground - 2, 0)],
        );
        assert_eq!(field.reachable_count(), 0);
        assert!(field.distance(BlockPos::new(0, ground, 0)).is_none());
    }

    #[test]
    fn cells_outside_the_bounds_are_simply_absent() {
        let (world, ground) = flat_world(1, 60);
        let bounds = VoxelAabb::new(
            BlockPos::new(-3, ground - 1, -3),
            BlockPos::new(3, ground + 1, 3),
        );
        let field = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);

        assert!(field.is_reachable(BlockPos::new(3, ground, 0)));
        assert!(!field.is_reachable(BlockPos::new(4, ground, 0)));
        assert!(field.distance(BlockPos::new(40, ground, 0)).is_none());
    }

    #[test]
    fn edges_stay_symmetric_under_an_overhang() {
        // The regression that motivated returning every candidate per column
        // rather than the topmost. A shelf splits a column into two standable
        // cells; "topmost only" made the edge into the lower one one-way, so
        // BFS could label a cell reachable that could not actually be stepped
        // to, and a drone would stand beside its goal, Stuck.
        let (mut world, ground) = flat_world(1, 60);
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Column C: dig the floor out one, then roof it — standable both under
        // the shelf (at ground-1) and on top of it (at ground+1).
        let c = [5, 5];
        world.set_block(BlockPos::new(c[0], ground - 1, c[1]), BlockId::AIR);
        world.set_block(BlockPos::new(c[0], ground, c[1]), stone);

        let under_shelf = BlockPos::new(c[0], ground - 1, c[1]);
        let beside = BlockPos::new(c[0] - 1, ground, c[1]);
        assert!(is_standable(&world, under_shelf));
        assert!(is_standable(&world, beside));

        let bounds = VoxelAabb::new(
            BlockPos::new(-4, ground - 3, -4),
            BlockPos::new(10, ground + 3, 10),
        );
        let field = FlowField::build(&world, bounds, [under_shelf]);

        // The neighbouring cell must be labelled AND able to act on the label.
        assert!(field.is_reachable(beside), "the cell beside the shelf is unreachable");
        let route = field
            .path_from(&world, beside)
            .expect("no route from directly beside the goal");
        assert_eq!(
            route.last(),
            Some(&under_shelf),
            "the route stopped short of the goal: an edge exists one way only"
        );

        // And the general contract across the whole field: any cell with a
        // positive distance can take a step. One dangling label anywhere means
        // a drone can be sent somewhere it will stand and do nothing.
        for pos in bounds.blocks() {
            if let Some(distance) = field.distance(pos) {
                if distance > 0 {
                    assert!(
                        field.step_from(&world, pos).is_some(),
                        "{pos:?} is labelled distance {distance} but cannot step"
                    );
                }
            }
        }
    }

    #[test]
    fn settling_drops_a_drone_onto_the_first_floor_below() {
        let (world, ground) = flat_world(1, 60);
        assert_eq!(
            settle(&world, BlockPos::new(0, ground + 5, 0)),
            BlockPos::new(0, ground, 0)
        );
        // Already standing: nothing moves.
        assert_eq!(
            settle(&world, BlockPos::new(0, ground, 0)),
            BlockPos::new(0, ground, 0)
        );
    }

    #[test]
    fn building_the_same_field_twice_gives_the_same_answer() {
        let (world, ground) = flat_world(1, 60);
        let bounds = VoxelAabb::new(
            BlockPos::new(-10, ground - 2, -10),
            BlockPos::new(10, ground + 2, 10),
        );
        let start = BlockPos::new(9, ground, -7);
        let a = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);
        let b = FlowField::build(&world, bounds, [BlockPos::new(0, ground, 0)]);
        assert_eq!(a.path_from(&world, start), b.path_from(&world, start));
    }
}
