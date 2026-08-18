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
