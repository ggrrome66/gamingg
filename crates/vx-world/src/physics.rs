//! Player movement against the voxel grid.
//!
//! Collision is resolved **one axis at a time**. Resolving all three together
//! and pushing out along the shallowest overlap is the usual shortcut, and it
//! is what produces the snagging on flat walls that makes voxel movement feel
//! bad: sliding along a wall catches on every block seam. Moving and resolving
//! each axis independently means a blocked axis stops while the others keep
//! going, which is what "sliding along a wall" actually is.
//!
//! Lives in `vx-world` rather than the app so it stays on the simulation side
//! of the networking seam, and so it can be tested without a window.

use glam::Vec3;
use vx_core::BlockPos;

use crate::world::World;

/// Downward acceleration, blocks per second squared. Tuned for a jump that
/// feels responsive rather than physically accurate — real gravity at this
/// scale feels floaty.
pub const GRAVITY: f32 = -30.0;

/// Fastest a body may fall, so a long drop cannot tunnel through the ground
/// between frames.
pub const TERMINAL_VELOCITY: f32 = -60.0;

/// Upward velocity applied by a jump. Clears one block comfortably.
pub const JUMP_SPEED: f32 = 9.2;

/// How high a step the player walks up without jumping.
///
/// Just over one block, deliberately. Measured from the feet, so clearing a
/// block whose top is 1.0 above them needs slightly more than 1.0 — a smaller
/// value (0.6 is the common choice elsewhere) cannot climb a full block at all,
/// only half-height ones.
///
/// Terrain here is generated as one-block terraces, so requiring a jump for
/// every step would make walking anywhere miserable. The trade-off is that a
/// wall must be two blocks tall to stop anyone.
pub const STEP_HEIGHT: f32 = 1.05;

/// Kept between the body and the blocks it rests against, so a body sitting
/// exactly on a boundary is not counted as intersecting it.
const SKIN: f32 = 1.0e-3;

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// A box of `width` × `height` × `width` standing on `feet`, centred
    /// horizontally on it.
    pub fn standing_on(feet: Vec3, width: f32, height: f32) -> Self {
        let half = width * 0.5;
        Aabb {
            min: Vec3::new(feet.x - half, feet.y, feet.z - half),
            max: Vec3::new(feet.x + half, feet.y + height, feet.z + half),
        }
    }

    pub fn translated(&self, delta: Vec3) -> Self {
        Aabb {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }

    pub fn contains_block(&self, block: BlockPos) -> bool {
        let cube = Aabb {
            min: Vec3::new(block.x as f32, block.y as f32, block.z as f32),
            max: Vec3::new(
                block.x as f32 + 1.0,
                block.y as f32 + 1.0,
                block.z as f32 + 1.0,
            ),
        };
        self.intersects(&cube)
    }

    /// Every block position this box overlaps.
    ///
    /// The upper bound is nudged inward so a box whose face lies exactly on a
    /// grid line does not claim the block on the far side of it.
    pub fn overlapping_blocks(&self) -> impl Iterator<Item = BlockPos> + '_ {
        let lo = [
            self.min.x.floor() as i32,
            self.min.y.floor() as i32,
            self.min.z.floor() as i32,
        ];
        let hi = [
            (self.max.x - SKIN).floor() as i32,
            (self.max.y - SKIN).floor() as i32,
            (self.max.z - SKIN).floor() as i32,
        ];

        (lo[1]..=hi[1].max(lo[1])).flat_map(move |y| {
            (lo[2]..=hi[2].max(lo[2]))
                .flat_map(move |z| (lo[0]..=hi[0].max(lo[0])).map(move |x| BlockPos::new(x, y, z)))
        })
    }
}

/// A player-sized body that walks and falls.
#[derive(Debug, Clone, Copy)]
pub struct PlayerBody {
    /// Centre of the feet.
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub width: f32,
    pub height: f32,
    /// Height of the camera above the feet.
    pub eye_height: f32,
}

impl Default for PlayerBody {
    fn default() -> Self {
        PlayerBody {
            position: Vec3::new(0.0, 80.0, 0.0),
            velocity: Vec3::ZERO,
            on_ground: false,
            width: 0.6,
            height: 1.8,
            eye_height: 1.62,
        }
    }
}

impl PlayerBody {
    pub fn aabb(&self) -> Aabb {
        Aabb::standing_on(self.position, self.width, self.height)
    }

    /// Where the camera sits.
    pub fn eye_position(&self) -> Vec3 {
        self.position + Vec3::Y * self.eye_height
    }

    /// Start a jump, if standing on something.
    pub fn jump(&mut self) -> bool {
        if !self.on_ground {
            return false;
        }
        self.velocity.y = JUMP_SPEED;
        self.on_ground = false;
        true
    }

    /// Advance one tick.
    ///
    /// `wish` is the desired horizontal velocity in blocks per second; vertical
    /// motion comes from gravity and jumping, not from `wish`.
    pub fn step(&mut self, world: &World, wish: Vec3, dt: f32) {
        // Horizontal velocity is set directly rather than accelerated: crisp
        // control suits a building game better than momentum.
        self.velocity.x = wish.x;
        self.velocity.z = wish.z;

        self.velocity.y = (self.velocity.y + GRAVITY * dt).max(TERMINAL_VELOCITY);

        let delta = self.velocity * dt;

        // Horizontal first, so `on_ground` from the vertical pass below
        // reflects where the body ends up this tick.
        self.move_horizontally(world, delta.x, delta.z);
        self.move_vertically(world, delta.y);
    }

    /// Move along x and z, attempting to step up over low obstacles.
    fn move_horizontally(&mut self, world: &World, dx: f32, dz: f32) {
        let before = self.position;

        let moved_x = self.slide(world, Vec3::X * dx);
        let moved_z = self.slide(world, Vec3::Z * dz);

        let blocked = (dx != 0.0 && !moved_x) || (dz != 0.0 && !moved_z);
        if !blocked || !self.on_ground {
            if !moved_x {
                self.velocity.x = 0.0;
            }
            if !moved_z {
                self.velocity.z = 0.0;
            }
            return;
        }

        // Blocked while walking. Try to step over it in three phases: rise,
        // move across, then settle back down onto whatever is now underfoot.
        //
        // The final descent is the part that matters. Rising and moving alone
        // leaves the body hovering, and gravity drags it back against the step
        // before it has cleared — so it bumps the same edge forever instead of
        // climbing it.
        let plain = self.position;
        let plain_gain = (plain.x - before.x).abs() + (plain.z - before.z).abs();

        let mut probe = *self;
        probe.position = before;

        let stop = |body: &mut PlayerBody, moved_x: bool, moved_z: bool| {
            if !moved_x {
                body.velocity.x = 0.0;
            }
            if !moved_z {
                body.velocity.z = 0.0;
            }
        };

        // No headroom to rise into means no step is possible.
        if !probe.slide(world, Vec3::Y * STEP_HEIGHT) {
            self.position = plain;
            stop(self, moved_x, moved_z);
            return;
        }
        probe.slide(world, Vec3::X * dx);
        probe.slide(world, Vec3::Z * dz);
        probe.slide(world, Vec3::Y * -STEP_HEIGHT);

        let stepped_gain = (probe.position.x - before.x).abs() + (probe.position.z - before.z).abs();

        // Only worth taking if it actually got further than walking into the
        // obstacle did, and left the body somewhere legal.
        if stepped_gain > plain_gain + SKIN && !collides(world, &probe.aabb()) {
            self.position = probe.position;
            self.on_ground = true;
        } else {
            self.position = plain;
            stop(self, moved_x, moved_z);
        }
    }

    /// Move along y, updating `on_ground`.
    fn move_vertically(&mut self, world: &World, dy: f32) {
        let landed = !self.slide(world, Vec3::Y * dy);
        if landed {
            // Hitting something while descending means we are standing on it;
            // while ascending it just means a bumped head.
            self.on_ground = dy < 0.0;
            self.velocity.y = 0.0;
        } else if dy != 0.0 {
            self.on_ground = false;
        }
    }

    /// Try to move by `delta` along a single axis.
    ///
    /// Returns `true` if the whole move was made. On collision the body is
    /// placed flush against the blocking surface and `false` is returned.
    fn slide(&mut self, world: &World, delta: Vec3) -> bool {
        let axis = if delta.x != 0.0 {
            0
        } else if delta.y != 0.0 {
            1
        } else if delta.z != 0.0 {
            2
        } else {
            return true;
        };
        let amount = delta[axis];

        let target = self.aabb().translated(delta);
        if !collides(world, &target) {
            self.position += delta;
            return true;
        }

        // Snap flush to the grid line just crossed. Blocks are unit cubes, so
        // the blocking surface is always at an integer coordinate.
        //
        // The box's faces are offset from `position` by fixed amounts (the
        // origin is the feet, not the centre), so those offsets are taken from
        // the *current* position before it is moved.
        let extent = self.aabb();
        let offset_to_max = extent.max[axis] - self.position[axis];
        let offset_to_min = self.position[axis] - extent.min[axis];

        if amount > 0.0 {
            // Leading face stops just short of the boundary it crossed.
            let barrier = target.max[axis].floor();
            self.position[axis] = barrier - offset_to_max - SKIN;
        } else {
            let barrier = target.min[axis].floor() + 1.0;
            self.position[axis] = barrier + offset_to_min + SKIN;
        }
        false
    }
}

/// Does this box overlap any block that blocks movement?
pub fn collides(world: &World, aabb: &Aabb) -> bool {
    aabb.overlapping_blocks().any(|block| world.is_solid(block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockId, ChunkPos};

    /// A world with a solid floor at y=40 across the origin chunk, and nothing
    /// above it — predictable ground to test against.
    fn flat_world() -> World {
        let mut world = World::new(1);
        world.load_around(ChunkPos::new(0, 0), 1);

        let stone = world.registry().id_of("engine:stone").unwrap();
        // Clear everything, then lay a single floor layer.
        for x in -16..32 {
            for z in -16..32 {
                for y in 0..80 {
                    let pos = BlockPos::new(x, y, z);
                    let fill = if y == 40 { stone } else { BlockId::AIR };
                    world.set_block(pos, fill);
                }
            }
        }
        world
    }

    fn body_at(x: f32, y: f32, z: f32) -> PlayerBody {
        PlayerBody {
            position: Vec3::new(x, y, z),
            ..PlayerBody::default()
        }
    }

    /// Run physics until the body settles or the budget runs out.
    fn settle(body: &mut PlayerBody, world: &World, ticks: usize) {
        for _ in 0..ticks {
            body.step(world, Vec3::ZERO, 1.0 / 60.0);
        }
    }

    #[test]
    fn an_aabb_reports_the_blocks_it_covers() {
        let aabb = Aabb {
            min: Vec3::new(0.2, 0.2, 0.2),
            max: Vec3::new(0.8, 0.8, 0.8),
        };
        let blocks: Vec<_> = aabb.overlapping_blocks().collect();
        assert_eq!(blocks, vec![BlockPos::new(0, 0, 0)]);
    }

    #[test]
    fn a_box_flush_against_a_grid_line_does_not_claim_the_next_block() {
        // Without the inward nudge, a body resting exactly on y=41 would be
        // treated as inside the block above and never stop falling.
        let aabb = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        let blocks: Vec<_> = aabb.overlapping_blocks().collect();
        assert_eq!(blocks, vec![BlockPos::new(0, 0, 0)]);
    }

    #[test]
    fn a_falling_body_lands_on_the_floor_and_stays_there() {
        let world = flat_world();
        let mut body = body_at(4.5, 60.0, 4.5);

        settle(&mut body, &world, 300);

        assert!(body.on_ground, "never landed");
        // The floor block spans y=40..41, so the feet rest at 41.
        assert!(
            (body.position.y - 41.0).abs() < 0.01,
            "settled at {} instead of 41",
            body.position.y
        );
        assert!(body.velocity.y.abs() < 0.01, "still moving after landing");
    }

    #[test]
    fn a_landed_body_does_not_sink_over_time() {
        // Catches an off-by-epsilon that lets the body creep down each tick.
        let world = flat_world();
        let mut body = body_at(4.5, 45.0, 4.5);
        settle(&mut body, &world, 120);
        let landed = body.position.y;

        settle(&mut body, &world, 600);

        assert!(
            (body.position.y - landed).abs() < 1e-3,
            "sank from {landed} to {}",
            body.position.y
        );
    }

    #[test]
    fn a_body_never_ends_up_inside_a_solid_block() {
        let world = flat_world();
        let mut body = body_at(4.5, 60.0, 4.5);

        for _ in 0..400 {
            body.step(&world, Vec3::new(3.0, 0.0, 2.0), 1.0 / 60.0);
            assert!(
                !collides(&world, &body.aabb()),
                "body is inside geometry at {:?}",
                body.position
            );
        }
    }

    #[test]
    fn walking_into_a_wall_stops_horizontal_movement() {
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        // A wall two blocks tall, too high to step over.
        for y in 41..43 {
            for z in 0..10 {
                world.set_block(BlockPos::new(8, y, z), stone);
            }
        }

        let mut body = body_at(4.5, 41.0, 4.5);
        settle(&mut body, &world, 30);

        for _ in 0..200 {
            body.step(&world, Vec3::new(6.0, 0.0, 0.0), 1.0 / 60.0);
        }

        assert!(
            body.position.x < 8.0,
            "walked through the wall to x={}",
            body.position.x
        );
        assert!(!collides(&world, &body.aabb()));
    }

    #[test]
    fn a_blocked_axis_does_not_stop_the_other_one() {
        // Sliding along a wall: pushing diagonally into it should still move
        // along it. This is what per-axis resolution buys.
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        for y in 41..43 {
            for z in -10..20 {
                world.set_block(BlockPos::new(8, y, z), stone);
            }
        }

        let mut body = body_at(6.0, 41.0, 4.5);
        settle(&mut body, &world, 30);
        let start_z = body.position.z;

        for _ in 0..120 {
            body.step(&world, Vec3::new(6.0, 0.0, 6.0), 1.0 / 60.0);
        }

        assert!(body.position.x < 8.0, "passed through the wall");
        assert!(
            body.position.z > start_z + 5.0,
            "did not slide along the wall: z moved {} to {}",
            start_z,
            body.position.z
        );
    }

    #[test]
    fn a_single_block_step_is_walked_over_without_jumping() {
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        // A raised plateau, not a one-block ridge: stepping over a ridge and
        // straight back down would pass a weaker version of this test. It must
        // also outlast the walk below, or the body strolls off the far edge.
        for x in 8..30 {
            for z in -5..15 {
                world.set_block(BlockPos::new(x, 41, z), stone);
            }
        }

        let mut body = body_at(5.0, 41.0, 4.5);
        settle(&mut body, &world, 30);

        for _ in 0..240 {
            body.step(&world, Vec3::new(4.0, 0.0, 0.0), 1.0 / 60.0);
        }

        assert!(
            body.position.x > 9.0,
            "did not step up: stopped at x={}",
            body.position.x
        );
        assert!(
            (body.position.y - 42.0).abs() < 0.05,
            "ended at y={} rather than on top of the plateau",
            body.position.y
        );
        assert!(body.on_ground);
    }

    #[test]
    fn a_two_block_wall_still_stops_the_player() {
        // The counterpart to auto-stepping a full block: walls must be two
        // tall to work, and this pins that they actually do.
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        for y in 41..43 {
            for z in -5..15 {
                world.set_block(BlockPos::new(8, y, z), stone);
            }
        }

        let mut body = body_at(5.0, 41.0, 4.5);
        settle(&mut body, &world, 30);
        for _ in 0..240 {
            body.step(&world, Vec3::new(4.0, 0.0, 0.0), 1.0 / 60.0);
        }

        assert!(body.position.x < 8.0, "climbed a two-block wall");
        assert!(!collides(&world, &body.aabb()));
    }

    #[test]
    fn jumping_only_works_from_the_ground() {
        let world = flat_world();
        let mut body = body_at(4.5, 60.0, 4.5);

        // Mid-air: refused.
        assert!(!body.jump());

        settle(&mut body, &world, 200);
        assert!(body.on_ground);
        assert!(body.jump());
        assert!(body.velocity.y > 0.0);
        // Cannot double jump.
        assert!(!body.jump());
    }

    #[test]
    fn a_jump_clears_one_block_and_comes_back_down() {
        let world = flat_world();
        let mut body = body_at(4.5, 45.0, 4.5);
        settle(&mut body, &world, 120);
        let ground = body.position.y;

        body.jump();
        let mut peak: f32 = ground;
        for _ in 0..200 {
            body.step(&world, Vec3::ZERO, 1.0 / 60.0);
            peak = peak.max(body.position.y);
        }

        assert!(peak >= ground + 1.0, "jump only reached {}", peak - ground);
        assert!(body.on_ground, "never came back down");
        assert!((body.position.y - ground).abs() < 0.01);
    }

    #[test]
    fn hitting_a_ceiling_stops_upward_motion_without_grounding() {
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        for x in 0..10 {
            for z in 0..10 {
                world.set_block(BlockPos::new(x, 44, z), stone);
            }
        }

        let mut body = body_at(4.5, 41.0, 4.5);
        settle(&mut body, &world, 30);
        body.jump();
        body.step(&world, Vec3::ZERO, 1.0 / 60.0);
        // Force it up into the ceiling.
        for _ in 0..10 {
            body.velocity.y = 20.0;
            body.step(&world, Vec3::ZERO, 1.0 / 60.0);
        }

        assert!(!collides(&world, &body.aabb()), "clipped into the ceiling");
        assert!(
            body.position.y + body.height <= 44.0 + SKIN * 2.0,
            "head at {} passed the ceiling at 44",
            body.position.y + body.height
        );
    }

    #[test]
    fn water_does_not_hold_the_player_up() {
        // Water is registered non-solid, so it must not collide.
        let mut world = flat_world();
        let water = world.registry().id_of("engine:water").unwrap();
        for x in 0..10 {
            for z in 0..10 {
                for y in 41..50 {
                    world.set_block(BlockPos::new(x, y, z), water);
                }
            }
        }

        let mut body = body_at(4.5, 60.0, 4.5);
        settle(&mut body, &world, 400);

        assert!(body.on_ground, "floated on water instead of sinking");
        assert!((body.position.y - 41.0).abs() < 0.01);
    }

    #[test]
    fn terminal_velocity_prevents_tunnelling_through_the_floor() {
        let world = flat_world();
        // Start very high so the fall is long enough to build up speed.
        let mut body = body_at(4.5, 250.0, 4.5);

        for _ in 0..2000 {
            body.step(&world, Vec3::ZERO, 1.0 / 60.0);
            assert!(body.velocity.y >= TERMINAL_VELOCITY - 0.001);
            if body.on_ground {
                break;
            }
        }

        assert!(body.on_ground, "fell forever");
        assert!(
            (body.position.y - 41.0).abs() < 0.01,
            "tunnelled to {}",
            body.position.y
        );
    }

    #[test]
    fn the_eye_sits_above_the_feet_inside_the_body() {
        let body = body_at(0.0, 41.0, 0.0);
        let eye = body.eye_position();
        assert!(eye.y > body.position.y);
        assert!(eye.y < body.position.y + body.height);
        assert_eq!(eye.x, body.position.x);
    }

    #[test]
    fn a_body_in_unloaded_space_falls_freely_rather_than_catching() {
        // Unloaded chunks read as air, so nothing should stop the body.
        let world = World::new(1);
        let mut body = body_at(1000.5, 100.0, 1000.5);
        let start = body.position.y;

        settle(&mut body, &world, 60);

        assert!(body.position.y < start, "did not fall through unloaded space");
        assert!(!body.on_ground);
    }
}


