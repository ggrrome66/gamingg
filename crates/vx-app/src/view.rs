//! Where the camera actually sits, and what that means for what gets drawn.
//!
//! The renderer needs no idea any of this exists. [`Camera`] has no look
//! target — the view matrix is built from `position` plus `yaw`/`pitch` alone
//! — so a third-person camera is *only* a different position, computed after
//! movement and before the frustum is rebuilt. The controllers keep owning
//! orientation; this module owns placement.
//!
//! The pull-in is the part worth being careful about. Backing into a wall
//! must slide the camera forward rather than push it through the rock, and a
//! pivot that is itself inside a block (standing in a doorway as it closes,
//! say) reports an obstruction at distance zero — without a floor on the
//! distance the camera would collapse onto the player's own head and render
//! the inside of their skull.

use glam::Vec3;
use vx_render::Camera;
use vx_world::World;

/// How far behind the pivot the follow camera sits when nothing is in the way.
pub const THIRD_PERSON_DISTANCE: f32 = 4.2;
/// How far above the eye pivot the orbit is anchored.
pub const THIRD_PERSON_LIFT: f32 = 0.35;
/// The camera never comes closer to the pivot than this, whatever the terrain.
pub const MIN_ORBIT_DISTANCE: f32 = 0.6;
/// Clearance kept between the camera and a wall it has pulled in against.
pub const CAMERA_SKIN: f32 = 0.25;

/// Whose eyes the player is looking through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Down the player's own nose.
    #[default]
    FirstPerson,
    /// Over the player's shoulder.
    ThirdPerson,
    /// Through a machine's camera; the body is elsewhere.
    Fpv,
}

impl ViewMode {
    /// The toggle. A feed is left alone — you leave it by hanging up, not by
    /// pressing the camera key.
    pub fn cycled(self) -> Self {
        match self {
            ViewMode::FirstPerson => ViewMode::ThirdPerson,
            ViewMode::ThirdPerson => ViewMode::FirstPerson,
            ViewMode::Fpv => ViewMode::Fpv,
        }
    }

    /// Should the player's body be drawn? Only when they can see it — first
    /// person would put geometry inside the near plane, and a machine's feed
    /// is looking somewhere else entirely.
    pub fn draws_body(self) -> bool {
        matches!(self, ViewMode::ThirdPerson)
    }

    /// Should the handheld drill be drawn? Only when it is in your hands.
    pub fn draws_viewmodel(self) -> bool {
        matches!(self, ViewMode::FirstPerson)
    }
}

/// How far back the orbit may sit before terrain gets in the way.
///
/// `back` is the unit direction from the pivot toward the camera. Never
/// returns less than [`MIN_ORBIT_DISTANCE`].
pub fn clear_orbit_distance(world: &World, pivot: Vec3, back: Vec3, wanted: f32) -> f32 {
    let blocked = vx_world::raycast_solid(world, world.registry(), pivot, back, wanted)
        .map(|hit| hit.distance - CAMERA_SKIN)
        .unwrap_or(wanted);
    blocked.clamp(MIN_ORBIT_DISTANCE, wanted)
}

/// The camera's position for this frame, given where its owner's eyes are.
///
/// `camera` supplies orientation only; the returned position replaces its own.
pub fn camera_placement(world: &World, camera: &Camera, pivot: Vec3, mode: ViewMode) -> Vec3 {
    match mode {
        // A feed's position is set by whatever it is a feed of.
        ViewMode::FirstPerson | ViewMode::Fpv => pivot,
        ViewMode::ThirdPerson => {
            let anchor = pivot + Vec3::Y * THIRD_PERSON_LIFT;
            let back = -camera.forward();
            let distance = clear_orbit_distance(world, anchor, back, THIRD_PERSON_DISTANCE);
            anchor + back * distance
        }
    }
}

/// How far in front of the eye the tool sits when nothing is in the way.
pub const VIEWMODEL_REACH: f32 = 0.85;
/// Clearance kept between the tool and the wall it has been pushed against.
pub const VIEWMODEL_SKIN: f32 = 0.35;
/// How far the tool drops, in metres, when fully stowed against a wall.
pub const VIEWMODEL_DROP: f32 = 0.30;
/// How far it swings down, in radians, when fully stowed.
pub const VIEWMODEL_TILT: f32 = 0.85;
/// How quickly the tool stows and comes back, in stow-fractions per second.
/// A tenth of a second either way: fast enough not to feel like a delay,
/// slow enough not to snap.
pub const VIEWMODEL_EASE: f32 = 10.0;

/// How much the tool in your hands must be stowed, 0 free .. 1 fully down.
///
/// Walk up to a hillside with the drill out and the barrel goes through it:
/// the viewmodel is drawn most of a metre in front of the eye, and nothing was
/// checking whether that metre was occupied. Every shooter that has met this
/// problem solves it the same way — as you close on a wall, the weapon comes
/// up and across rather than through — and the check is one ray, because the
/// tool sits at a known offset from a camera that already knows where it is.
///
/// Returned as a fraction rather than a position so the caller decides what
/// stowing *looks* like; the geometry of "how close is too close" lives here.
pub fn viewmodel_stow(world: &World, camera: &Camera, eye: Vec3) -> f32 {
    let ahead = camera.forward();
    let wanted = VIEWMODEL_REACH + VIEWMODEL_SKIN;
    let clear = vx_world::raycast_solid(world, world.registry(), eye, ahead, wanted)
        .map(|hit| hit.distance)
        .unwrap_or(wanted);
    // Free at full reach, fully stowed once the wall is at the skin.
    ((wanted - clear) / VIEWMODEL_REACH).clamp(0.0, 1.0)
}

/// Ease the stow fraction toward its target by one frame.
///
/// Frame time rather than the sim tick, and deliberately: this is the one
/// piece of the round that has no effect a capture can see — the viewmodel is
/// not drawn in the captures the oracle takes, which are third-person or
/// headless — and tying it to the movement tick would make a tool held still
/// against a wall in a paused game keep moving.
pub fn eased_stow(current: f32, target: f32, dt: f32) -> f32 {
    let step = VIEWMODEL_EASE * dt;
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockPos, ChunkPos};

    /// A world with real terrain around the origin.
    fn world() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 1);
        world
    }

    fn camera_looking(yaw: f32, pitch: f32) -> Camera {
        Camera { yaw, pitch, ..Camera::default() }
    }

    /// A pivot well clear of the village buildings, in open air.
    fn open_pivot(world: &World) -> Vec3 {
        let ground = world.surface_y(0, 0).expect("origin loaded");
        Vec3::new(0.5, ground as f32 + 6.0, 0.5)
    }

    #[test]
    fn first_person_puts_the_camera_exactly_on_the_pivot() {
        let world = world();
        let camera = camera_looking(0.7, -0.2);
        let pivot = open_pivot(&world);
        assert_eq!(
            camera_placement(&world, &camera, pivot, ViewMode::FirstPerson),
            pivot
        );
        // A feed's position comes from the machine, so placement passes it
        // straight through too.
        assert_eq!(camera_placement(&world, &camera, pivot, ViewMode::Fpv), pivot);
    }

    #[test]
    fn third_person_sits_behind_and_above_the_pivot() {
        let world = world();
        let camera = camera_looking(0.0, 0.0);
        let pivot = open_pivot(&world);

        let placed = camera_placement(&world, &camera, pivot, ViewMode::ThirdPerson);
        let anchor = pivot + Vec3::Y * THIRD_PERSON_LIFT;
        let back = -camera.forward();

        // Directly along -forward from the lifted anchor, at full distance in
        // open air.
        let offset = placed - anchor;
        assert!(
            (offset.length() - THIRD_PERSON_DISTANCE).abs() < 1e-3,
            "sat {} away, wanted {THIRD_PERSON_DISTANCE}",
            offset.length()
        );
        assert!(
            offset.normalize().dot(back) > 0.999,
            "camera is not behind the pivot"
        );
        assert!(placed.y > pivot.y - THIRD_PERSON_DISTANCE, "sank below the pivot");
    }

    #[test]
    fn the_orbit_camera_pulls_in_when_a_wall_is_behind_you() {
        let mut world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let pivot = Vec3::new(0.5, ground as f32 + 2.0, 0.5);
        let stone = world.registry().id_of("engine:stone").unwrap();

        let camera = camera_looking(0.0, 0.0); // looking -Z, so the camera sits +Z
        let open = camera_placement(&world, &camera, pivot, ViewMode::ThirdPerson);

        // Drop a wall two blocks behind the pivot.
        for dy in -1..=2 {
            for dx in -1..=1 {
                world.set_block(
                    BlockPos::new(dx, ground + 2 + dy, 2),
                    stone,
                );
            }
        }
        let blocked = camera_placement(&world, &camera, pivot, ViewMode::ThirdPerson);

        assert!(
            (blocked - pivot).length() < (open - pivot).length(),
            "the wall did not pull the camera in"
        );
        assert!(
            !world.is_solid(BlockPos::new(
                blocked.x.floor() as i32,
                blocked.y.floor() as i32,
                blocked.z.floor() as i32
            )),
            "the camera ended up inside the wall at {blocked:?}"
        );
    }

    #[test]
    fn the_orbit_camera_never_collapses_onto_the_pivot() {
        // A pivot inside solid rock makes the raycast report an obstruction
        // at distance zero. Without the floor the camera would land on the
        // player's own head.
        let world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let buried = Vec3::new(0.5, ground as f32 - 4.0, 0.5);
        let camera = camera_looking(1.1, 0.0);

        let placed = camera_placement(&world, &camera, buried, ViewMode::ThirdPerson);
        let anchor = buried + Vec3::Y * THIRD_PERSON_LIFT;
        assert!(
            (placed - anchor).length() >= MIN_ORBIT_DISTANCE - 1e-4,
            "camera collapsed to {} from the pivot",
            (placed - anchor).length()
        );
    }

    #[test]
    fn the_camera_keeps_clear_of_geometry_all_the_way_round() {
        // Sweeping the view in a real world is the check that matters: any
        // yaw where the camera ends up inside a hillside is a frame of
        // looking at the inside of the ground.
        let world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let pivot = Vec3::new(0.5, ground as f32 + 1.6, 0.5);

        for step in 0..64 {
            let yaw = step as f32 * std::f32::consts::TAU / 64.0;
            for pitch in [-0.6f32, 0.0, 0.6] {
                let camera = camera_looking(yaw, pitch);
                let placed = camera_placement(&world, &camera, pivot, ViewMode::ThirdPerson);
                let cell = BlockPos::new(
                    placed.x.floor() as i32,
                    placed.y.floor() as i32,
                    placed.z.floor() as i32,
                );
                assert!(
                    !world.is_solid(cell),
                    "camera inside geometry at yaw {yaw}, pitch {pitch}: {placed:?}"
                );
            }
        }
    }

    #[test]
    fn the_orbit_camera_stays_clear_of_solids_over_positions_and_angles() {
        // The single-pivot sweep above, widened into the property test the
        // polish round asked for: many stand points, every boom angle. A
        // clamped camera that lands inside a hillside is a frame of looking
        // through the world. The pivots are lifted well into open air so the
        // wall is never inside `MIN_ORBIT_DISTANCE` — the one case where the
        // pull-in's own floor can seat the camera in rock — which keeps the
        // guarantee here the strong one: the placed cell is always air.
        let world = world();
        for &(x, z) in &[
            (2, 2),
            (5, 3),
            (8, 8),
            (11, 6),
            (13, 12),
            (3, 13),
            (14, 2),
            (7, 10),
            (10, 14),
        ] {
            let Some(ground) = world.surface_y(x, z) else {
                continue;
            };
            for lift in [5.0f32, 8.0, 12.0] {
                let pivot = Vec3::new(x as f32 + 0.5, ground as f32 + lift, z as f32 + 0.5);
                for step in 0..48 {
                    let yaw = step as f32 * std::f32::consts::TAU / 48.0;
                    for pitch in [-1.2f32, -0.6, 0.0, 0.6, 1.2] {
                        let camera = camera_looking(yaw, pitch);
                        let placed =
                            camera_placement(&world, &camera, pivot, ViewMode::ThirdPerson);
                        let cell = BlockPos::new(
                            placed.x.floor() as i32,
                            placed.y.floor() as i32,
                            placed.z.floor() as i32,
                        );
                        assert!(
                            !world.is_solid(cell),
                            "camera inside geometry at ({x},{z}) lift {lift}, \
                             yaw {yaw}, pitch {pitch}: {placed:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn placing_the_camera_repeatedly_from_its_own_eye_does_not_drift() {
        // The fly-mode bug, in miniature. `camera_placement` returns a point
        // metres behind the eye in third person, so a caller that feeds the
        // result back in as the next frame's pivot adds the boom again every
        // frame and the camera walks backwards through the world. The frame
        // loop keeps the eye separately for exactly this reason; this pins
        // the property that makes that necessary.
        let world = world();
        let camera = camera_looking(0.4, -0.1);
        let eye = open_pivot(&world);

        // Placed from a *fixed* eye, the answer is the same every time.
        let first = camera_placement(&world, &camera, eye, ViewMode::ThirdPerson);
        for _ in 0..30 {
            let again = camera_placement(&world, &camera, eye, ViewMode::ThirdPerson);
            assert_eq!(again, first, "placement is not a pure function of its eye");
        }

        // Fed its own output, it runs away — which is the bug, stated so that
        // anyone tempted to simplify the frame loop back into one variable
        // finds out here rather than in a playtest.
        let mut drifting = eye;
        for _ in 0..30 {
            drifting = camera_placement(&world, &camera, drifting, ViewMode::ThirdPerson);
        }
        assert!(
            (drifting - eye).length() > THIRD_PERSON_DISTANCE,
            "feeding placement its own output should compound; it moved only {}",
            (drifting - eye).length()
        );
    }

    #[test]
    fn the_tool_is_free_in_the_open_and_stowed_against_a_wall() {
        // The complaint: walk up to a hillside with the drill out and the
        // barrel goes through it.
        let mut world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let eye = Vec3::new(0.5, ground as f32 + 2.0, 0.5);
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Looking -Z into open air.
        let camera = camera_looking(0.0, 0.0);
        assert_eq!(
            viewmodel_stow(&world, &camera, eye),
            0.0,
            "the tool stowed with nothing in front of it"
        );

        // A wall one block ahead, well inside the tool's reach.
        for dy in -1..=2 {
            for dx in -1..=1 {
                world.set_block(BlockPos::new(dx, ground + 2 + dy, -1), stone);
            }
        }
        let stowed = viewmodel_stow(&world, &camera, eye);
        assert!(stowed > 0.0, "the wall did not stow the tool");
        assert!(stowed <= 1.0, "stow left its range: {stowed}");
    }

    #[test]
    fn the_closer_the_wall_the_further_the_tool_stows() {
        // A step-change would read as the tool snapping; it has to be gradual
        // or walking slowly at a wall looks broken. Built as a cleared
        // corridor with one wall in it, so the answer depends on the wall
        // rather than on whatever the terrain happened to put nearby.
        let mut world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let air = vx_core::BlockId::AIR;
        let stone = world.registry().id_of("engine:stone").unwrap();
        let head = ground + 2;

        for z in -1..=3 {
            for dy in -1..=2 {
                for dx in -1..=1 {
                    world.set_block(BlockPos::new(dx, head + dy, z), air);
                }
            }
        }
        // The wall spans z in [-1, 0), so its near face is at z = 0.
        for dy in -1..=2 {
            for dx in -1..=1 {
                world.set_block(BlockPos::new(dx, head + dy, -1), stone);
            }
        }

        let camera = camera_looking(0.0, 0.0); // looking -Z, into the wall
        let far = viewmodel_stow(&world, &camera, Vec3::new(0.5, head as f32, 1.1));
        let near = viewmodel_stow(&world, &camera, Vec3::new(0.5, head as f32, 0.3));

        assert!(far > 0.0, "the wall was not seen at all from 1.1 m");
        assert!(
            near > far,
            "stowing did not increase as the wall closed: {far} then {near}"
        );
    }

    #[test]
    fn stowing_eases_rather_than_snapping() {
        // Both directions, and both arrive.
        let mut stow = 0.0;
        for _ in 0..3 {
            stow = eased_stow(stow, 1.0, 1.0 / 60.0);
        }
        assert!(stow > 0.0 && stow < 1.0, "stow snapped straight to {stow}");

        for _ in 0..60 {
            stow = eased_stow(stow, 1.0, 1.0 / 60.0);
        }
        assert_eq!(stow, 1.0, "stow never arrived");

        for _ in 0..60 {
            stow = eased_stow(stow, 0.0, 1.0 / 60.0);
        }
        assert_eq!(stow, 0.0, "the tool never came back up");
    }

    #[test]
    fn a_tool_pointed_at_the_sky_is_never_stowed() {
        // Looking up from open ground there is nothing to hit, and a tool that
        // stowed itself in mid-air would be worse than the bug.
        let world = world();
        let ground = world.surface_y(0, 0).unwrap();
        let eye = Vec3::new(0.5, ground as f32 + 3.0, 0.5);
        let camera = camera_looking(0.0, 1.2);
        assert_eq!(viewmodel_stow(&world, &camera, eye), 0.0);
    }

    #[test]
    fn cycling_the_view_returns_to_first_person() {
        assert_eq!(ViewMode::FirstPerson.cycled(), ViewMode::ThirdPerson);
        assert_eq!(ViewMode::FirstPerson.cycled().cycled(), ViewMode::FirstPerson);
        // A live feed is left alone by the camera key.
        assert_eq!(ViewMode::Fpv.cycled(), ViewMode::Fpv);
    }

    #[test]
    fn only_third_person_draws_the_body_and_only_first_person_the_viewmodel() {
        assert!(!ViewMode::FirstPerson.draws_body());
        assert!(ViewMode::ThirdPerson.draws_body());
        assert!(!ViewMode::Fpv.draws_body());

        assert!(ViewMode::FirstPerson.draws_viewmodel());
        assert!(!ViewMode::ThirdPerson.draws_viewmodel());
        assert!(!ViewMode::Fpv.draws_viewmodel());
    }
}
