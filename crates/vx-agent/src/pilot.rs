//! Manual control: the player takes the wheel.
//!
//! The whole point of this module is a single invariant — **a piloted machine
//! can do nothing an autonomous one could not.** Piloting is not a debug
//! teleport and not a flying camera; it drives the same one-cell-per-tick
//! simulation the planners were built against. A drone under manual control
//! still obeys [`crate::flow`]'s standability and one-block step rule, still
//! falls when it cuts its own floor, still cannot reach through rock; a flier
//! still climbs before it enters a column. Break that and every guarantee the
//! mine planners rest on — "every route in is a route out" above all — stops
//! being true the moment a player touches a machine.
//!
//! Input arrives as a [`PilotCommand`] sampled once per frame and re-issued on
//! each simulation tick, so driving is frame-rate independent exactly like the
//! autonomous path.

use vx_core::BlockPos;
use vx_world::World;

use crate::drone::{is_breakable, Drone, REACH_OFFSETS};
use crate::flier::{Flier, CLIMB_RATE};
use crate::flow;

/// A cardinal direction on the ground plane. No diagonals, for the same
/// reason [`crate::flow`] has none: a diagonal step squeezes through a corner
/// nothing could physically pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heading {
    PosX,
    NegX,
    PosZ,
    NegZ,
}

impl Heading {
    /// The cardinal closest to a look direction, or `None` when the look is
    /// too near-vertical to mean anything horizontal.
    pub fn from_look(dx: f32, dz: f32) -> Option<Heading> {
        if dx * dx + dz * dz < 1.0e-6 {
            return None;
        }
        Some(if dx.abs() >= dz.abs() {
            if dx >= 0.0 {
                Heading::PosX
            } else {
                Heading::NegX
            }
        } else if dz >= 0.0 {
            Heading::PosZ
        } else {
            Heading::NegZ
        })
    }

    /// Turn by quarter turns, positive going PosX → PosZ → NegX → NegZ.
    /// Strafing is "forward, rotated", so it needs no separate table.
    pub fn rotated(self, quarter_turns: i32) -> Heading {
        const ORDER: [Heading; 4] = [Heading::PosX, Heading::PosZ, Heading::NegX, Heading::NegZ];
        let at = ORDER.iter().position(|h| *h == self).unwrap_or(0) as i32;
        ORDER[(at + quarter_turns).rem_euclid(4) as usize]
    }

    /// The block offset one step along this heading.
    pub fn offset(self) -> [i32; 3] {
        match self {
            Heading::PosX => [1, 0, 0],
            Heading::NegX => [-1, 0, 0],
            Heading::PosZ => [0, 0, 1],
            Heading::NegZ => [0, 0, -1],
        }
    }
}

/// What the pilot is asking for on this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PilotCommand {
    /// Where to drive, if anywhere.
    pub heading: Option<Heading>,
    /// Whether the cutter is running.
    pub cut: bool,
    /// Fliers only: +1 up, -1 down, 0 hold.
    pub climb: i32,
}

/// What one tick of piloting actually managed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PilotReport {
    pub moved: bool,
    pub dug: u64,
    /// The machine refused something the pilot asked for — rock in the way, or
    /// a full cargo bed. The UI turns this into feedback rather than silence.
    pub blocked: bool,
}

impl Drone {
    /// Drive one cell along `heading`, obeying the same standability and
    /// one-block-step rule the flow field uses. False when nothing there is
    /// drivable — a wall, a two-block drop, a ledge too high.
    pub fn pilot_step(&mut self, world: &World, heading: Heading) -> bool {
        let offset = heading.offset();
        let ahead = self.position.offset(offset);
        // Every standable cell within one block up or down, nearest to level
        // first: exactly the candidates `flow::neighbours` would offer.
        let candidates = [0, 1, -1];
        for dy in candidates {
            let cell = ahead.offset([0, dy, 0]);
            if flow::is_standable(world, cell) {
                self.move_to(cell);
                return true;
            }
        }
        false
    }

    /// Gravity, exactly as the autonomous tick applies it. True if it fell.
    pub fn pilot_settle(&mut self, world: &World) -> bool {
        let settled = flow::settle(world, self.position);
        if settled != self.position {
            self.move_to(settled);
            return true;
        }
        false
    }

    /// The cell the cutter would take, biased toward `heading`.
    ///
    /// The drone's own reach, filtered to breakable blocks, preferring the
    /// direction the pilot is facing so the cutter does what the screen
    /// suggests. Undermining stays last and keeps the safety check, so a pilot
    /// cannot dig a hole the drone could not climb out of.
    pub fn pilot_target(&self, world: &World, heading: Option<Heading>) -> Option<BlockPos> {
        let breakable = |pos: &BlockPos| is_breakable(world, *pos);

        if let Some(heading) = heading {
            let offset = heading.offset();
            // Straight ahead, then the block above it: the two a driver means.
            let ahead = self.position.offset(offset);
            for cell in [ahead, ahead.offset([0, 1, 0])] {
                if breakable(&cell) {
                    return Some(cell);
                }
            }
        }

        REACH_OFFSETS
            .into_iter()
            .map(|offset| self.position.offset(offset))
            .find(|pos| breakable(pos))
            .or_else(|| {
                let below = self.position.offset([0, -1, 0]);
                (self.may_undermine(world) && breakable(&below)).then_some(below)
            })
    }
}

impl Flier {
    /// One tick of piloted flight.
    ///
    /// Same rule as [`Flier::fly_towards`]: never enter a column while below
    /// its safe altitude — climb instead — and never change height by more
    /// than [`CLIMB_RATE`] in a tick. A pilot can fly a bird anywhere it could
    /// have flown itself, and nowhere it could not.
    pub fn pilot_step(&mut self, world: &World, heading: Option<Heading>, climb: i32) -> bool {
        let here = self.position;

        // Deliberate climbing first: it is also how a pilot clears a ridge.
        if climb != 0 && heading.is_none() {
            let step = climb.clamp(-CLIMB_RATE, CLIMB_RATE);
            let floor = Flier::safe_altitude(world, here.x, here.z, here.y);
            let wanted = (here.y + step).max(floor);
            if wanted != here.y {
                self.move_to(BlockPos::new(here.x, wanted, here.z));
                return true;
            }
            return false;
        }

        let Some(heading) = heading else { return false };
        let offset = heading.offset();
        let next = (here.x + offset[0], here.z + offset[2]);

        let needed = Flier::safe_altitude(world, next.0, next.1, here.y);
        if here.y < needed {
            // Climb before entering, never after.
            let step = (needed - here.y).min(CLIMB_RATE);
            self.move_to(here.offset([0, step, 0]));
            return true;
        }

        // Advance, honouring a descent the pilot asked for but never dropping
        // below the new column's floor.
        let wanted = (here.y + climb.clamp(-CLIMB_RATE, CLIMB_RATE)).max(needed);
        self.move_to(BlockPos::new(next.0, wanted, next.1));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    #[test]
    fn a_heading_comes_from_the_look_direction_and_rotates_by_quarters() {
        assert_eq!(Heading::from_look(1.0, 0.0), Some(Heading::PosX));
        assert_eq!(Heading::from_look(-1.0, 0.0), Some(Heading::NegX));
        assert_eq!(Heading::from_look(0.0, 1.0), Some(Heading::PosZ));
        assert_eq!(Heading::from_look(0.0, -1.0), Some(Heading::NegZ));
        // Ties go to x, matching the flier's own tie-break.
        assert_eq!(Heading::from_look(1.0, 1.0), Some(Heading::PosX));
        assert_eq!(Heading::from_look(0.0, 0.0), None);

        assert_eq!(Heading::PosX.rotated(1), Heading::PosZ);
        assert_eq!(Heading::PosX.rotated(-1), Heading::NegZ);
        assert_eq!(Heading::PosX.rotated(4), Heading::PosX);
        assert_eq!(Heading::PosZ.rotated(2), Heading::NegZ);
    }

    #[test]
    fn a_piloted_drone_steps_one_cell_per_tick() {
        let world = fixture::flat(3, 60);
        let mut drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 61, 8));

        assert!(drone.pilot_step(&world, Heading::PosX));
        assert_eq!(drone.position, BlockPos::new(9, 61, 8));
        assert_eq!(drone.previous_position, BlockPos::new(8, 61, 8));

        assert!(drone.pilot_step(&world, Heading::NegZ));
        assert_eq!(drone.position, BlockPos::new(9, 61, 7));
    }

    #[test]
    fn a_piloted_drone_can_step_up_and_down_one_block_but_no_more() {
        // A one-block ledge is drivable up and back down; a two-block wall is
        // not — the same rule the flow field enforces for the autonomous path.
        let mut world = fixture::flat(3, 60);
        let stone = world.registry().id_of("engine:stone").unwrap();
        world.set_block(BlockPos::new(9, 61, 8), stone);

        let mut drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 61, 8));
        assert!(drone.pilot_step(&world, Heading::PosX), "could not climb one block");
        assert_eq!(drone.position, BlockPos::new(9, 62, 8));

        // And back down the same ledge.
        assert!(drone.pilot_step(&world, Heading::NegX), "could not step down");
        assert_eq!(drone.position, BlockPos::new(8, 61, 8));

        // A wall two blocks high leaves nowhere to go: level is solid, one up
        // is solid, and one down is the floor itself.
        world.set_block(BlockPos::new(9, 62, 8), stone);
        assert!(!drone.pilot_step(&world, Heading::PosX), "climbed two blocks");
        assert_eq!(drone.position, BlockPos::new(8, 61, 8));
    }

    #[test]
    fn a_piloted_drone_cannot_drive_into_solid_rock() {
        let mut world = fixture::flat(3, 60);
        let stone = world.registry().id_of("engine:stone").unwrap();
        for y in 61..=63 {
            world.set_block(BlockPos::new(9, y, 8), stone);
        }
        let mut drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 61, 8));
        assert!(!drone.pilot_step(&world, Heading::PosX));
        assert_eq!(drone.position, BlockPos::new(8, 61, 8), "drove into the wall");
    }

    #[test]
    fn a_piloted_drone_falls_when_the_ground_goes_away() {
        let mut world = fixture::flat(3, 60);
        let air = vx_core::BlockId::AIR;
        world.set_block(BlockPos::new(8, 60, 8), air);
        world.set_block(BlockPos::new(8, 59, 8), air);

        let mut drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 61, 8));
        assert!(drone.pilot_settle(&world), "did not fall");
        assert_eq!(drone.position.y, 59);
        assert!(!drone.pilot_settle(&world), "kept falling through solid ground");
    }

    #[test]
    fn pilot_targets_never_include_bedrock() {
        // Standing on the world floor, the only thing underfoot is bedrock.
        let world = fixture::flat(3, 1);
        let drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 2, 8));
        let target = drone.pilot_target(&world, Some(Heading::PosX));
        if let Some(target) = target {
            assert!(
                is_breakable(&world, target),
                "pilot aimed at something unbreakable at {target:?}"
            );
        }
    }

    #[test]
    fn a_pilot_can_only_undermine_where_the_ai_could() {
        // Hovering one block above the floor: undermining here would drop the
        // drone two blocks, so it must be refused.
        let mut world = fixture::flat(3, 60);
        let air = vx_core::BlockId::AIR;
        world.set_block(BlockPos::new(8, 59, 8), air);

        let drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 61, 8));
        assert!(!drone.may_undermine(&world));
        // With nothing else in reach, there is nothing to cut rather than an
        // unsafe hole.
        let target = drone.pilot_target(&world, None);
        assert_ne!(target, Some(BlockPos::new(8, 60, 8)), "undermined unsafely");
    }

    #[test]
    fn a_piloted_flier_never_enters_a_column_below_its_safe_altitude() {
        let world = fixture::flat(3, 60);
        let mut flier = Flier::new(BlockPos::new(8, 62, 8));
        // Well below cruise: the first tick must climb, not advance.
        let before = flier.position;
        assert!(flier.pilot_step(&world, Some(Heading::PosX), 0));
        assert_eq!(flier.position.x, before.x, "advanced while too low");
        assert!(flier.position.y > before.y, "did not climb");

        // Once at altitude it advances and stays clear of the ground.
        for _ in 0..12 {
            flier.pilot_step(&world, Some(Heading::PosX), 0);
            let floor = Flier::safe_altitude(&world, flier.position.x, flier.position.z, flier.position.y);
            assert!(
                flier.position.y >= floor,
                "flew below the safe altitude at {:?}",
                flier.position
            );
        }
    }

    #[test]
    fn a_piloted_flier_climbs_at_most_the_climb_rate() {
        let world = fixture::flat(3, 60);
        let mut flier = Flier::new(BlockPos::new(8, 90, 8));
        let before = flier.position.y;
        flier.pilot_step(&world, None, 99);
        assert!(
            flier.position.y - before <= CLIMB_RATE,
            "climbed {} in one tick",
            flier.position.y - before
        );

        let before = flier.position.y;
        flier.pilot_step(&world, None, -99);
        assert!(before - flier.position.y <= CLIMB_RATE, "dived too fast");
    }

    #[test]
    fn a_piloted_drone_reaches_nowhere_the_ai_could_not() {
        // The invariant, exercised: drive a long, seeded, arbitrary route over
        // real fixture terrain and assert every cell it ever occupies is one
        // the flow field would call standable.
        let world = fixture::hillside(4, 70, 0);
        // Drop onto whatever the slope actually offers rather than guessing a
        // height and starting inside the hill.
        let mut drone = Drone::new(crate::job::DroneId(0), BlockPos::new(8, 120, 8));
        drone.pilot_settle(&world);
        assert!(flow::is_standable(&world, drone.position), "bad start");

        let mut seed = 0x5eedu64;
        for _ in 0..200 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let heading = match (seed >> 33) % 4 {
                0 => Heading::PosX,
                1 => Heading::NegX,
                2 => Heading::PosZ,
                _ => Heading::NegZ,
            };
            drone.pilot_step(&world, heading);
            drone.pilot_settle(&world);
            assert!(
                flow::is_standable(&world, drone.position),
                "piloted into a cell no drone could stand in: {:?}",
                drone.position
            );
        }
    }
}
