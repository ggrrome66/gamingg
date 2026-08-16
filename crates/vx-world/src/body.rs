//! A physical body in the voxel grid.
//!
//! Axis-separated AABB collision: each step moves the box along y, then x,
//! then z, clamping against solid blocks as it goes. Y first is what makes
//! ground detection reliable — landing is resolved before any sideways motion
//! can shave the corner off a block edge.
//!
//! Motion is substepped so no single move exceeds a fraction of a block.
//! Without that, one frame of terminal-velocity fall crosses several blocks
//! and sails straight through a floor — the classic tunnelling bug, and also
//! the cheapest way to break into somewhere walls should keep you out of.
//!
//! The body knows nothing about input, cameras or worlds. It takes a solidity
//! closure, so tests can build exact scenarios and the caller decides how
//! unloaded chunks behave (the app treats them as solid, so nobody falls into
//! terrain that has not streamed in yet).

use glam::Vec3;
use vx_core::BlockPos;

/// Gap kept between the body and any face it rests against, in blocks.
///
/// Without it, a body flush against a face sits exactly on the boundary and
/// floating-point noise flickers it between "touching" and "inside".
const SKIN: f32 = 1.0e-3;

/// Longest move one substep may make on any axis.
const MAX_SUBSTEP: f32 = 0.4;

/// Ceiling on substeps per call, so a hostile dt cannot buy unbounded work.
/// Motion beyond what fits is discarded — the body briefly moves slower than
/// it should, which beats either tunnelling or stalling the frame.
const MAX_SUBSTEPS: u32 = 24;

/// Fastest any axis may move, in blocks per second. Clamped, because velocity
/// is fed by accumulating forces and one bad dt would otherwise launch the
/// body far enough to make the substep cap truncate real motion.
const MAX_SPEED: f32 = 64.0;

/// What one step did, for the caller's state machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepResult {
    /// Hit something below while moving down.
    pub on_ground: bool,
    /// Hit something above while moving up.
    pub hit_head: bool,
    /// Hit something sideways.
    pub hit_wall: bool,
}

/// An axis-aligned body: feet-centred position, box extents, velocity.
#[derive(Debug, Clone, Copy)]
pub struct Body {
    /// Centre of the feet: the box spans `position.y..position.y + height`.
    pub position: Vec3,
    pub velocity: Vec3,
    /// Half the box width, in x and z alike.
    pub half_width: f32,
    pub height: f32,
    /// True after a step that ended standing on something.
    pub on_ground: bool,
}

impl Body {
    /// A player-shaped body: 0.6 wide, 1.8 tall.
    pub fn player(position: Vec3) -> Self {
        Body {
            position,
            velocity: Vec3::ZERO,
            half_width: 0.3,
            height: 1.8,
            on_ground: false,
        }
    }

    /// The box corners as `(min, max)`.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (
            Vec3::new(
                self.position.x - self.half_width,
                self.position.y,
                self.position.z - self.half_width,
            ),
            Vec3::new(
                self.position.x + self.half_width,
                self.position.y + self.height,
                self.position.z + self.half_width,
            ),
        )
    }

    /// Advance by `dt` seconds under the current velocity, colliding against
    /// `solid`. Returns what was hit.
    ///
    /// The velocity components that hit something are zeroed, so gravity does
    /// not accumulate through a floor and a wall stops pushing.
    pub fn step(&mut self, dt: f32, solid: impl Fn(BlockPos) -> bool) -> StepResult {
        let mut result = StepResult::default();

        // Refuse rubbish rather than propagating it: a NaN position never
        // recovers, and every comparison against it is silently false.
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
            return result;
        }
        if !self.position.is_finite() || !dt.is_finite() || dt <= 0.0 {
            return result;
        }

        self.velocity = self.velocity.clamp(Vec3::splat(-MAX_SPEED), Vec3::splat(MAX_SPEED));

        let motion = self.velocity * dt;
        let longest = motion.abs().max_element();
        let substeps = ((longest / MAX_SUBSTEP).ceil() as u32).clamp(1, MAX_SUBSTEPS);
        let mut per_step = motion / substeps as f32;

        // If the cap truncated the substep count, dividing the full motion
        // across fewer steps would make each one huge and reopen the
        // tunnelling hole the substeps exist to close. Clamp the step instead:
        // the excess motion is discarded, exactly as promised above.
        let step_longest = per_step.abs().max_element();
        if step_longest > MAX_SUBSTEP {
            per_step *= MAX_SUBSTEP / step_longest;
        }

        self.on_ground = false;
        for _ in 0..substeps {
            // Y first: land before sliding, or sideways motion clips corners.
            for axis in [1usize, 0, 2] {
                let delta = per_step[axis];
                if delta == 0.0 {
                    continue;
                }
                let hit = self.move_axis(axis, delta, &solid);
                if hit {
                    self.velocity[axis] = 0.0;
                    match axis {
                        1 if delta < 0.0 => {
                            result.on_ground = true;
                            self.on_ground = true;
                        }
                        1 => result.hit_head = true,
                        _ => result.hit_wall = true,
                    }
                }
            }
        }

        result
    }

    /// Move along one axis, clamping against solid blocks. True on contact.
    fn move_axis(&mut self, axis: usize, delta: f32, solid: &impl Fn(BlockPos) -> bool) -> bool {
        let (old_min, old_max) = self.bounds();

        // The volume the moved box would occupy.
        let mut new_min = old_min;
        let mut new_max = old_max;
        new_min[axis] += delta;
        new_max[axis] += delta;

        // Every block that volume overlaps. The epsilon keeps a box exactly
        // flush with a face from counting the block beyond it.
        let lo = (new_min + SKIN).floor();
        let hi = (new_max - SKIN).floor();

        let mut clamped = delta;
        let mut hit = false;

        for bx in (lo.x as i32)..=(hi.x as i32) {
            for by in (lo.y as i32)..=(hi.y as i32) {
                for bz in (lo.z as i32)..=(hi.z as i32) {
                    if !solid(BlockPos::new(bx, by, bz)) {
                        continue;
                    }
                    let block_min = [bx as f32, by as f32, bz as f32][axis];
                    let block_max = block_min + 1.0;

                    let allowed = if delta > 0.0 {
                        block_min - old_max[axis] - SKIN
                    } else {
                        block_max - old_min[axis] + SKIN
                    };
                    // Never clamp into moving the other way: a body already
                    // overlapping something (spawned into gravel, say) stays
                    // put rather than being flung out.
                    let allowed = if delta > 0.0 {
                        allowed.clamp(0.0, delta)
                    } else {
                        allowed.clamp(delta, 0.0)
                    };
                    if allowed.abs() < clamped.abs() {
                        clamped = allowed;
                        hit = true;
                    }
                }
            }
        }

        self.position[axis] += clamped;
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A world of exactly the listed solid blocks.
    fn world_of(blocks: &[(i32, i32, i32)]) -> impl Fn(BlockPos) -> bool + '_ {
        let set: HashSet<BlockPos> = blocks
            .iter()
            .map(|(x, y, z)| BlockPos::new(*x, *y, *z))
            .collect();
        move |pos| set.contains(&pos)
    }

    /// A flat floor at y = 9 (top face at y = 10), wide enough to land on.
    fn floor() -> Vec<(i32, i32, i32)> {
        let mut blocks = Vec::new();
        for x in -6..6 {
            for z in -6..6 {
                blocks.push((x, 9, z));
            }
        }
        blocks
    }

    #[test]
    fn a_falling_body_lands_on_the_floor_and_stays_there() {
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 14.0, 0.5));
        body.velocity.y = -8.0;

        let mut landed = false;
        for _ in 0..120 {
            body.velocity.y -= 30.0 * 0.016;
            let result = body.step(0.016, &solid);
            landed |= result.on_ground;
        }

        assert!(landed, "the body never touched down");
        assert!(body.on_ground);
        // Resting on the top face at y = 10, within the skin.
        assert!(
            (body.position.y - 10.0).abs() < 0.01,
            "resting at {} instead of on the floor",
            body.position.y
        );
        assert_eq!(body.velocity.y, 0.0, "gravity accumulated through the floor");
    }

    #[test]
    fn one_frame_of_terminal_velocity_does_not_tunnel_through_a_floor() {
        // 64 blocks/s for a tenth of a second is 6.4 blocks of motion — six
        // block layers. Substepping is what makes the floor still catch it.
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 12.0, 0.5));
        body.velocity.y = -MAX_SPEED;

        body.step(0.1, &solid);

        assert!(
            body.position.y >= 10.0 - 0.01,
            "fell to {} — straight through the floor",
            body.position.y
        );
    }

    #[test]
    fn a_wall_stops_sideways_motion_at_its_face() {
        let mut blocks = floor();
        // A wall two high at x = 3.
        blocks.push((3, 10, 0));
        blocks.push((3, 11, 0));
        let solid = world_of(&blocks);

        let mut body = Body::player(Vec3::new(0.5, 10.0, 0.5));
        for _ in 0..60 {
            body.velocity.x = 6.0;
            body.velocity.y -= 30.0 * 0.016;
            body.step(0.016, &solid);
        }

        // The face is at x = 3; the box edge stops a skin short of it.
        assert!(
            (body.position.x + body.half_width) <= 3.0,
            "pushed into the wall at x = {}",
            body.position.x
        );
        assert!(
            body.position.x > 2.5,
            "stopped far from the wall at x = {}",
            body.position.x
        );
    }

    #[test]
    fn a_jump_clears_at_least_a_block_and_comes_back_down() {
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 10.0, 0.5));

        body.velocity.y = 8.5;
        let mut apex: f32 = 0.0;
        let mut landed_again = false;
        for _ in 0..200 {
            body.velocity.y -= 30.0 * 0.016;
            let result = body.step(0.016, &solid);
            apex = apex.max(body.position.y);
            if result.on_ground {
                landed_again = true;
                break;
            }
        }

        assert!(apex - 10.0 >= 1.0, "jump apex only reached {apex}");
        assert!(landed_again, "never came back down");
    }

    #[test]
    fn a_ceiling_stops_upward_motion() {
        let mut blocks = floor();
        for x in -2..3 {
            for z in -2..3 {
                blocks.push((x, 13, z)); // ceiling: underside at y = 13
            }
        }
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 10.0, 0.5));

        body.velocity.y = 10.0;
        let result = body.step(0.2, &solid);

        assert!(result.hit_head, "never bumped the ceiling");
        assert!(
            body.position.y + body.height <= 13.0,
            "head inside the ceiling at {}",
            body.position.y + body.height
        );
        assert_eq!(body.velocity.y, 0.0);
    }

    #[test]
    fn sliding_along_a_wall_keeps_the_other_axis_moving() {
        // Hitting a wall on x must not eat the z motion, or walking into any
        // wall glues you to it.
        let mut blocks = floor();
        for z in -6..6 {
            blocks.push((3, 10, z));
            blocks.push((3, 11, z));
        }
        let solid = world_of(&blocks);

        let mut body = Body::player(Vec3::new(2.5, 10.0, 0.5));
        let start_z = body.position.z;
        for _ in 0..30 {
            body.velocity.x = 4.0;
            body.velocity.z = 4.0;
            body.velocity.y -= 30.0 * 0.016;
            body.step(0.016, &solid);
        }

        assert!(
            body.position.z - start_z > 1.0,
            "z motion was eaten by the x collision"
        );
        assert!(body.position.x + body.half_width <= 3.0);
    }

    #[test]
    fn walking_off_an_edge_starts_a_fall() {
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 10.0, 0.5));

        // Walk east well past the platform's edge at x = 6.
        for _ in 0..200 {
            body.velocity.x = 5.0;
            body.velocity.y -= 30.0 * 0.016;
            body.step(0.016, &solid);
        }

        assert!(body.position.x > 6.0, "never left the platform");
        assert!(!body.on_ground, "still grounded in mid-air");
        assert!(body.position.y < 10.0 - 0.5, "did not start falling");
    }

    #[test]
    fn non_finite_input_is_swallowed_rather_than_propagated() {
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 10.5, 0.5));

        body.velocity = Vec3::new(f32::NAN, f32::INFINITY, 0.0);
        body.step(0.016, &solid);
        assert!(body.position.is_finite(), "NaN velocity corrupted the position");
        assert_eq!(body.velocity, Vec3::ZERO);

        // A rubbish dt moves nothing rather than exploding.
        body.velocity = Vec3::new(1.0, 0.0, 0.0);
        let before = body.position;
        body.step(f32::NAN, &solid);
        body.step(-1.0, &solid);
        assert_eq!(body.position, before);
    }

    #[test]
    fn a_hostile_dt_is_bounded_rather_than_buying_unbounded_work() {
        // An hour-long dt must not iterate millions of substeps. The motion
        // is truncated instead, and the body stays inside the world.
        let blocks = floor();
        let solid = world_of(&blocks);
        let mut body = Body::player(Vec3::new(0.5, 12.0, 0.5));
        body.velocity = Vec3::new(MAX_SPEED, -MAX_SPEED, MAX_SPEED);

        body.step(3600.0, &solid);

        let travelled = (body.position - Vec3::new(0.5, 12.0, 0.5)).length();
        assert!(
            travelled <= MAX_SUBSTEPS as f32 * MAX_SUBSTEP * 2.0,
            "moved {travelled} blocks in one step"
        );
    }

    #[test]
    fn a_body_spawned_overlapping_a_block_is_not_flung_out() {
        // Clamping must never move the body backwards past its start, or a
        // bad spawn teleports the player.
        let blocks = floor();
        let solid = world_of(&blocks);
        // Feet inside the floor layer.
        let mut body = Body::player(Vec3::new(0.5, 9.5, 0.5));
        let before = body.position;

        body.velocity = Vec3::new(2.0, -2.0, 0.0);
        body.step(0.016, &solid);

        assert!(
            (body.position - before).length() < 0.5,
            "an overlapping spawn was flung from {before} to {}",
            body.position
        );
    }
}
