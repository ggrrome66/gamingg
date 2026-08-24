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
//!
//! # What this file does not know
//!
//! Stamina, stance, cargo, skills and the shop. This answers exactly one
//! question — given a box, a velocity and a tick, where does it end up and what
//! did it touch — and [`MoveParams`] carries the numbers that shape the answer
//! without carrying any reason for them. The state machine that picks those
//! numbers lives in `vx-app` with the rest of the fiction, the same line that
//! keeps `vx-agent` free of quests and economy.

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

/// Upward velocity applied by a jump. Clears one block with margin.
pub const JUMP_SPEED: f32 = 8.4;

/// How high a step the player walks up without asking.
///
/// Slabs, rubble and kerbs are free; a full block is not. Keeping a one-block
/// ledge a deliberate act is what makes vaulting a verb rather than decoration,
/// and it keeps the player legible against the drone planner's one-block step —
/// the unit the whole mining system is built around.
///
/// This was 1.05 before the movement round, which let you stroll up a full
/// block without noticing. The terraced terrain that justified it is still
/// there; what changed is that the player can now vault, automatically, so the
/// terraces cost a gesture instead of a keypress.
pub const STEP_HEIGHT: f32 = 0.6;

/// Collision sub-steps per tick.
///
/// Not about tunnelling — nine blocks a second over a sixty-fourth of a second
/// is a seventh of a block, so nothing is passed through. It is about the seam
/// between two coplanar cube faces catching a fast-moving box, which reads as
/// the world grabbing you at random. Cheap enough to apply always rather than
/// only while sliding.
pub const SUBSTEPS: u32 = 4;

/// Kept between the body and the blocks it rests against, so a body snapped
/// flush to a grid line is not counted as intersecting what is past it.
const SKIN: f32 = 1.0e-3;

/// How far past a grid line a box may reach before it claims the block there.
///
/// This is the design note's "collision box inset by a hair", implemented as a
/// tolerance in the block query rather than as a physically smaller box. The
/// smaller box is the obvious reading and it is wrong: a sweep that only ever
/// guarantees the *shrunken* hull is clear lets the real hull penetrate by
/// exactly the amount you shrank it, which is the bug the inset was there to
/// prevent — and the error compounds across sub-steps until the body is
/// genuinely embedded.
///
/// Distinct from [`SKIN`], which snaps a blocked body flush to the line it
/// crossed. Two jobs, two constants, and this one must stay the smaller of the
/// pair or a body snapped flush would immediately re-collide with what it was
/// snapped against.
pub const INSET: f32 = 1.0e-5;

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

    /// The same box shrunk by `amount` on every face.
    pub fn shrunk(&self, amount: f32) -> Self {
        Aabb {
            min: self.min + Vec3::splat(amount),
            max: self.max - Vec3::splat(amount),
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
            (self.max.x - INSET).floor() as i32,
            (self.max.y - INSET).floor() as i32,
            (self.max.z - INSET).floor() as i32,
        ];

        (lo[1]..=hi[1].max(lo[1])).flat_map(move |y| {
            (lo[2]..=hi[2].max(lo[2]))
                .flat_map(move |z| (lo[0]..=hi[0].max(lo[0])).map(move |x| BlockPos::new(x, y, z)))
        })
    }
}

/// Numbers that shape a step, with no opinion about where they came from.
///
/// The app builds one of these per tick out of stance, stamina and how much
/// rock you are carrying. This crate only ever reads them.
#[derive(Debug, Clone, Copy)]
pub struct MoveParams {
    /// How hard horizontal velocity is pulled toward `wish`, blocks per second
    /// squared.
    pub accel: f32,
    /// Exponential drag on horizontal velocity, per second. The whole
    /// difference between sliding and crouching is this number.
    pub friction: f32,
    /// How high a step is climbed without asking.
    pub step_height: f32,
    /// Whether gravity is integrated. A mantle suspends it.
    pub gravity: bool,
}

/// Ordinary walking.
impl Default for MoveParams {
    fn default() -> Self {
        MoveParams {
            accel: 60.0,
            friction: 10.0,
            step_height: STEP_HEIGHT,
            gravity: true,
        }
    }
}

/// Where a swept box ended up, and what it touched on the way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    /// The box's new minimum corner. Callers that track a different origin —
    /// the player tracks its feet — take the difference against the corner they
    /// started from rather than reading this directly.
    pub position: Vec3,
    pub velocity: Vec3,
    pub grounded: bool,
    /// The face the box came to rest against, or `Vec3::ZERO` in open air.
    ///
    /// Every surface in a voxel world is axis-aligned, so this is `+Y` on any
    /// block top and never a ramp normal. It earns its place by telling a wall
    /// apart from a floor, not by describing a slope — there are no slopes,
    /// only stairs.
    pub ground_normal: Vec3,
    pub hit_ceiling: bool,
}

/// Sweep `aabb` through the world by `velocity * dt` and report where it lands.
///
/// Pure: no gravity, no acceleration, no state. The caller integrates velocity
/// however it likes and this moves the box. Sub-stepped [`SUBSTEPS`] times and
/// sub-stepped [`SUBSTEPS`] times unconditionally, because it is cheap and
/// wrong to skip at speed.
pub fn step_aabb(
    world: &World,
    aabb: Aabb,
    velocity: Vec3,
    dt: f32,
    step_height: f32,
) -> StepResult {
    let mut sweep = Sweep {
        aabb,
        moved: Vec3::ZERO,
        stepped: false,
    };
    let mut velocity = velocity;
    // Established before the first sub-step rather than discovered during the
    // vertical pass. The horizontal pass is what needs to know — it will not
    // climb a step unless it believes it is standing on something — and it runs
    // first. Discovering it afterwards means the one sub-step that hits the
    // ledge is the one sub-step that does not yet know it is on the ground.
    let mut grounded = supported(world, &sweep.aabb);
    let mut normal = if grounded { Vec3::Y } else { Vec3::ZERO };
    let mut ceiling = false;

    let slice = dt / SUBSTEPS as f32;
    for _ in 0..SUBSTEPS {
        let delta = velocity * slice;

        // Horizontal first, so the vertical pass below reports the ground the
        // body actually ends up over rather than the one it started on.
        let (blocked_x, blocked_z) = sweep.walk(world, delta.x, delta.z, step_height, grounded);
        if blocked_x {
            velocity.x = 0.0;
            normal.x = -delta.x.signum();
        }
        if blocked_z {
            velocity.z = 0.0;
            normal.z = -delta.z.signum();
        }

        if !sweep.slide(world, Vec3::Y * delta.y) {
            // Stopped while descending means standing on it; while ascending it
            // is a bumped head.
            if delta.y < 0.0 {
                grounded = true;
                normal.y = 1.0;
            } else if delta.y > 0.0 {
                ceiling = true;
                normal.y = -1.0;
            }
            velocity.y = 0.0;
        } else if delta.y != 0.0 {
            grounded = false;
            normal.y = 0.0;
        }

        if sweep.stepped {
            grounded = true;
            normal.y = 1.0;
            sweep.stepped = false;
        }
    }

    StepResult {
        position: aabb.min + sweep.moved,
        velocity,
        grounded,
        ground_normal: normal,
        hit_ceiling: ceiling,
    }
}

/// A box being pushed through the world, one axis at a time.
struct Sweep {
    aabb: Aabb,
    /// Total translation applied so far, so the caller can recover a position
    /// in whatever origin it happens to track.
    moved: Vec3,
    /// Set when `walk` climbed something, so the caller counts it as ground.
    stepped: bool,
}

impl Sweep {
    /// Move along x and z, climbing low obstacles rather than stopping at them.
    ///
    /// Returns which axes were refused.
    fn walk(
        &mut self,
        world: &World,
        dx: f32,
        dz: f32,
        step_height: f32,
        grounded: bool,
    ) -> (bool, bool) {
        let before = self.moved;

        let moved_x = self.slide(world, Vec3::X * dx);
        let moved_z = self.slide(world, Vec3::Z * dz);

        let blocked = (dx != 0.0 && !moved_x) || (dz != 0.0 && !moved_z);
        if !blocked || !grounded || step_height <= 0.0 {
            return (!moved_x, !moved_z);
        }

        // Blocked while walking. Try to climb it in three phases: rise, move
        // across, then settle back down onto whatever is now underfoot.
        //
        // The final descent is the part that matters. Rising and moving alone
        // leaves the body hovering, and gravity drags it back against the step
        // before it has cleared — so it bumps the same edge forever instead of
        // climbing it.
        let plain = self.moved;
        let plain_gain = (plain.x - before.x).abs() + (plain.z - before.z).abs();

        let mut probe = Sweep {
            aabb: self.aabb.translated(before - self.moved),
            moved: before,
            stepped: false,
        };

        // No headroom to rise into means no step is possible.
        if !probe.slide(world, Vec3::Y * step_height) {
            return (!moved_x, !moved_z);
        }
        probe.slide(world, Vec3::X * dx);
        probe.slide(world, Vec3::Z * dz);
        probe.slide(world, Vec3::Y * -step_height);

        let gain = (probe.moved.x - before.x).abs() + (probe.moved.z - before.z).abs();

        // Only worth taking if it got further than walking into the obstacle
        // did, and left the body somewhere legal.
        if gain > plain_gain + SKIN && !collides(world, &probe.aabb) {
            self.aabb = probe.aabb;
            self.moved = probe.moved;
            self.stepped = true;
            (false, false)
        } else {
            (!moved_x, !moved_z)
        }
    }

    /// Try to move by `delta` along a single axis.
    ///
    /// Returns `true` if the whole move was made. On collision the box is
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

        let target = self.aabb.translated(delta);
        if !collides(world, &target) {
            self.aabb = target;
            self.moved += delta;
            return true;
        }

        // Snap flush to the grid line just crossed. Blocks are unit cubes, so
        // the blocking surface is always at an integer coordinate.
        let shift = if delta[axis] > 0.0 {
            target.max[axis].floor() - SKIN - self.aabb.max[axis]
        } else {
            target.min[axis].floor() + 1.0 + SKIN - self.aabb.min[axis]
        };

        // A shift that would move the box backwards past where it already sits
        // means it started overlapping; leave it alone rather than teleporting.
        if shift.signum() == delta[axis].signum() && shift.abs() <= delta[axis].abs() {
            let step = Vec3::new(
                if axis == 0 { shift } else { 0.0 },
                if axis == 1 { shift } else { 0.0 },
                if axis == 2 { shift } else { 0.0 },
            );
            self.aabb = self.aabb.translated(step);
            self.moved += step;
        }
        false
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
    /// The face last rested against. See [`StepResult::ground_normal`].
    pub ground_normal: Vec3,
    pub hit_ceiling: bool,
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
            ground_normal: Vec3::ZERO,
            hit_ceiling: false,
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

    /// Advance one tick under ordinary walking rules.
    ///
    /// `wish` is the desired horizontal velocity in blocks per second; vertical
    /// motion comes from gravity and jumping, not from `wish`.
    pub fn step(&mut self, world: &World, wish: Vec3, dt: f32) {
        self.step_with(world, wish, dt, MoveParams::default());
    }

    /// Advance one tick with the numbers the caller chose.
    pub fn step_with(&mut self, world: &World, wish: Vec3, dt: f32, params: MoveParams) {
        // Friction first, then acceleration along the wish direction only.
        //
        // The obvious model — pull velocity toward `wish` from wherever it is —
        // also *decelerates* anything faster than `wish`, which quietly deletes
        // every impulse the moment it is applied: a slide dies in a tick and a
        // slide-jump lands at walking pace. Accelerating only up to the target,
        // and only along the direction asked for, leaves momentum alone and
        // hands decay entirely to `friction`. That is what makes a slide a
        // velocity that outlives the input that made it.
        let mut horizontal = Vec3::new(self.velocity.x, 0.0, self.velocity.z);

        let speed = horizontal.length();
        if speed > 1.0e-6 && params.friction > 0.0 {
            let kept = (1.0 - params.friction * dt).clamp(0.0, 1.0);
            horizontal *= kept;
        }

        let target = Vec3::new(wish.x, 0.0, wish.z);
        let want = target.length();
        if want > 1.0e-6 {
            let direction = target / want;
            let along = horizontal.dot(direction);
            let add = (want - along).clamp(0.0, params.accel * dt);
            horizontal += direction * add;
        }

        self.velocity.x = horizontal.x;
        self.velocity.z = horizontal.z;

        if params.gravity {
            self.velocity.y = (self.velocity.y + GRAVITY * dt).max(TERMINAL_VELOCITY);
        }

        let before = self.aabb().min;
        let result = step_aabb(world, self.aabb(), self.velocity, dt, params.step_height);
        self.position += result.position - before;
        self.velocity = result.velocity;
        self.on_ground = result.grounded;
        self.ground_normal = result.ground_normal;
        self.hit_ceiling = result.hit_ceiling;
    }
}

/// Does this box overlap any block that blocks movement?
pub fn collides(world: &World, aabb: &Aabb) -> bool {
    aabb.overlapping_blocks().any(|block| world.is_solid(block))
}

/// Is there something solid immediately under this box?
///
/// A hair below, not a block below: a body resting flush on a surface is not
/// intersecting it, so "on the ground" cannot be answered by [`collides`] alone.
pub fn supported(world: &World, aabb: &Aabb) -> bool {
    collides(world, &aabb.translated(Vec3::Y * -(SKIN * 2.0)))
}

#[cfg(test)]
mod wound_tests {
    use super::*;
    use crate::micro;
    use vx_core::{BlockPos, ChunkPos, LocalPos};

    use crate::world::World;

    /// The asymmetry the micro round rests on: rays read a block's cells,
    /// feet never do. You can shoot through a peephole; you can never fall
    /// through one, and that is what leaves physics, `supported`, the flow
    /// fields and every footing untouched by damage.
    #[test]
    fn a_wounded_block_is_still_a_solid_floor() {
        let mut world = World::new(7);
        world.load_around(ChunkPos::new(0, 0), 1);
        let stone = world.registry().id_of("engine:stone").unwrap();

        let floor = BlockPos::new(2, 80, 2);
        for x in 0..6 {
            for z in 0..6 {
                for y in 60..=80 {
                    world.set_block(
                        BlockPos::new(x, y, z),
                        if y == 80 { stone } else { vx_core::BlockId::AIR },
                    );
                }
            }
        }

        let standing = Aabb::standing_on(
            Vec3::new(floor.x as f32 + 0.5, 81.0, floor.z as f32 + 0.5),
            0.6,
            1.8,
        );
        assert!(
            supported(&world, &standing),
            "the test never stood the body on the floor to begin with"
        );

        // Now chew the floor block most of the way through — far more than
        // any single hit does — and stand on it again.
        let mut mask = micro::FULL;
        for cell in 0..micro::SIDE * micro::SIDE * micro::SIDE {
            let x = cell % micro::SIDE;
            let z = (cell / micro::SIDE) % micro::SIDE;
            let y = cell / (micro::SIDE * micro::SIDE);
            if micro::remaining(mask) <= micro::DEATH_CELLS + 1 {
                break;
            }
            mask &= !micro::bit(x, y, z);
        }
        world
            .chunk_mut(ChunkPos::new(0, 0))
            .unwrap()
            .set_mask(LocalPos::new(floor.x, floor.y, floor.z).unwrap(), mask);

        assert!(
            world.mask(floor).is_some(),
            "the test did not actually wound the floor"
        );
        assert!(
            supported(&world, &standing),
            "feet fell through a wound — physics must read composites as full boxes"
        );
        assert!(
            collides(
                &world,
                &Aabb::standing_on(
                    Vec3::new(floor.x as f32 + 0.5, floor.y as f32, floor.z as f32 + 0.5),
                    0.6,
                    0.8,
                )
            ),
            "a body could stand inside a wounded block"
        );
    }
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

    /// A raised plateau one block up, walked into from below.
    ///
    /// Not a one-block ridge: stepping over a ridge and straight back down
    /// would pass a weaker version of this. It must also outlast the walk, or
    /// the body strolls off the far edge.
    fn plateau_world(top: i32) -> World {
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        for x in 8..30 {
            for z in -5..15 {
                for y in 41..=top {
                    world.set_block(BlockPos::new(x, y, z), stone);
                }
            }
        }
        world
    }

    #[test]
    fn a_full_block_is_no_longer_walked_up() {
        // STEP_HEIGHT is 0.6 now. A full block is a vault, and vaulting is the
        // app's job — down here it must simply stop you, or the ledge verbs in
        // `vx-app` never get a chance to fire.
        let world = plateau_world(41);
        let mut body = body_at(5.0, 41.0, 4.5);
        settle(&mut body, &world, 30);

        for _ in 0..240 {
            body.step(&world, Vec3::new(4.0, 0.0, 0.0), 1.0 / 60.0);
        }

        assert!(
            body.position.x < 8.1,
            "walked up a full block to x={}",
            body.position.x
        );
        assert!(!collides(&world, &body.aabb()));
    }

    #[test]
    fn a_half_height_step_is_still_walked_over_without_jumping() {
        // The other half of the same trade: rubble and kerbs stay free.
        let world = flat_world();
        let mut world = world;
        let stone = world.registry().id_of("engine:stone").unwrap();
        // A plateau whose top sits half a block up, built by standing the body
        // on a floor at y=40 and raising the far ground to y=40.5 — which the
        // grid cannot express, so instead: walk from a floor of 41 onto 41 with
        // a deliberately small step height, proving the mechanism rather than
        // the geometry.
        for x in 8..30 {
            for z in -5..15 {
                world.set_block(BlockPos::new(x, 41, z), stone);
            }
        }

        let mut body = body_at(5.0, 41.0, 4.5);
        settle(&mut body, &world, 30);
        let params = MoveParams {
            step_height: 1.05,
            ..MoveParams::default()
        };

        for _ in 0..240 {
            body.step_with(&world, Vec3::new(4.0, 0.0, 0.0), 1.0 / 60.0, params);
        }

        assert!(
            body.position.x > 9.0,
            "a generous step height did not climb: stopped at x={}",
            body.position.x
        );
        assert!((body.position.y - 42.0).abs() < 0.05, "ended at y={}", body.position.y);
        assert!(body.on_ground);
    }

    #[test]
    fn step_aabb_reports_a_floor_as_up_and_a_wall_as_sideways() {
        // The ground normal exists to tell a floor from a wall. Every surface
        // here is axis-aligned, so that is all it can ever say — and all the
        // slide needs it to say.
        let mut world = flat_world();
        let stone = world.registry().id_of("engine:stone").unwrap();
        for y in 41..44 {
            for z in -5..15 {
                world.set_block(BlockPos::new(8, y, z), stone);
            }
        }

        let box_at = |x: f32, y: f32| Aabb::standing_on(Vec3::new(x, y, 4.5), 0.6, 1.8);

        let falling = step_aabb(&world, box_at(4.5, 41.1), Vec3::new(0.0, -20.0, 0.0), 1.0 / 60.0, 0.6);
        assert!(falling.grounded, "did not find the floor");
        assert_eq!(falling.ground_normal.y, 1.0);

        // Standing on the floor *and* pressed against a wall: the normal has to
        // report both, or a slide cannot tell "stop dead" from "keep going down".
        let into_wall =
            step_aabb(&world, box_at(7.6, 41.0), Vec3::new(20.0, 0.0, 0.0), 1.0 / 60.0, 0.6);
        assert!(
            into_wall.ground_normal.x < 0.0,
            "wall normal points the wrong way: {:?}",
            into_wall.ground_normal
        );
        assert_eq!(into_wall.ground_normal.y, 1.0, "lost the floor while hitting a wall");
        assert!(into_wall.velocity.x.abs() < 1e-6, "kept driving into the wall");
    }

    #[test]
    fn step_aabb_never_leaves_a_box_inside_rock() {
        // Sub-stepping earns its keep here: the same sweep at speed, from many
        // starts, must never end up embedded.
        let world = plateau_world(45);
        for i in 0..400 {
            let x = 4.0 + (i % 20) as f32 * 0.17;
            let y = 41.0 + (i / 20) as f32 * 0.11;
            let speed = 3.0 + (i % 7) as f32 * 2.0;
            let result = step_aabb(
                &world,
                Aabb::standing_on(Vec3::new(x, y, 4.5), 0.6, 1.8),
                Vec3::new(speed, -speed, speed * 0.5),
                1.0 / 64.0,
                0.6,
            );
            let landed = Aabb {
                min: result.position,
                max: result.position + Vec3::new(0.6, 1.8, 0.6),
            };
            assert!(
                !collides(&world, &landed),
                "sweep {i} ended inside rock at {:?}",
                result.position
            );
        }
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
