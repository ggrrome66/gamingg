//! Turning input into camera motion.
//!
//! Separated from the event loop so it can be tested without a window: the
//! logic is a pure function of (camera, input, elapsed time).

use glam::Vec3;
use vx_platform::InputState;
use vx_render::Camera;
use vx_world::{PlayerBody, World};

/// How the player moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementMode {
    /// Free flight, ignoring terrain. Useful for building and debugging.
    Fly,
    /// Walking, subject to gravity and collision.
    Walk,
}

impl MovementMode {
    pub fn toggled(self) -> Self {
        match self {
            MovementMode::Fly => MovementMode::Walk,
            MovementMode::Walk => MovementMode::Fly,
        }
    }
}

/// Apply accumulated mouse movement to the camera's orientation.
///
/// Shared by both controllers so look behaviour cannot drift between them. The
/// delta is consumed even when the mouse is not captured, so motion that
/// arrived while the pointer was free is discarded rather than replayed as a
/// snap the moment capture resumes.
pub fn apply_mouse_look(camera: &mut Camera, input: &mut InputState, sensitivity: f32) {
    let (dx, dy) = input.take_mouse_delta();
    if !input.mouse_captured {
        return;
    }
    camera.yaw += dx * sensitivity;
    // Screen y grows downward, so pushing the mouse down looks down.
    camera.pitch -= dy * sensitivity;
    camera.clamp_pitch();
}

/// Free-flying camera controls.
#[derive(Debug, Clone, Copy)]
pub struct FlyController {
    /// Blocks per second.
    pub speed: f32,
    /// Multiplier applied while sprinting.
    pub sprint_multiplier: f32,
    /// Radians of rotation per pixel of mouse movement.
    pub sensitivity: f32,
}

impl Default for FlyController {
    fn default() -> Self {
        FlyController {
            speed: 24.0,
            sprint_multiplier: 4.0,
            sensitivity: 0.0022,
        }
    }
}

impl FlyController {
    /// Advance the camera by `dt` seconds of input.
    pub fn apply(&self, camera: &mut Camera, input: &mut InputState, dt: f32) {
        apply_mouse_look(camera, input, self.sensitivity);

        let axes = input.movement_axes();
        if axes == Vec3::ZERO {
            return;
        }

        let speed = if input.is_sprinting() {
            self.speed * self.sprint_multiplier
        } else {
            self.speed
        };

        // Horizontal movement follows where you are looking, but stays level:
        // flying should not sink because you glanced down.
        let motion = camera.right() * axes.x + Vec3::Y * axes.y + camera.forward_level() * axes.z;
        camera.position += motion * speed * dt;
    }
}

/// Walking controls: input drives a physical body, and the camera rides its
/// eyes rather than being positioned directly.
#[derive(Debug, Clone, Copy)]
pub struct WalkController {
    /// Blocks per second on the ground.
    pub speed: f32,
    pub sprint_multiplier: f32,
    pub sensitivity: f32,
}

impl Default for WalkController {
    fn default() -> Self {
        WalkController {
            speed: 4.8,
            sprint_multiplier: 1.9,
            sensitivity: 0.0022,
        }
    }
}

impl WalkController {
    /// Advance the body by `dt` seconds of input, then move the camera to its
    /// eyes.
    pub fn apply(
        &self,
        camera: &mut Camera,
        player: &mut PlayerBody,
        world: &World,
        input: &mut InputState,
        dt: f32,
    ) {
        apply_mouse_look(camera, input, self.sensitivity);

        let axes = input.movement_axes();
        let speed = if input.is_sprinting() {
            self.speed * self.sprint_multiplier
        } else {
            self.speed
        };

        // Walking follows the camera's heading but stays level, so looking up
        // does not slow you down or launch you.
        let wish = (camera.right() * axes.x + camera.forward_level() * axes.z) * speed;

        // Space jumps rather than flying upward.
        if axes.y > 0.0 {
            player.jump();
        }

        player.step(world, wish, dt);
        camera.position = player.eye_position();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    fn setup() -> (Camera, InputState, FlyController) {
        (
            Camera {
                position: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                ..Camera::default()
            },
            InputState::new(),
            FlyController::default(),
        )
    }

    #[test]
    fn with_no_input_the_camera_does_not_move() {
        let (mut camera, mut input, controller) = setup();
        controller.apply(&mut camera, &mut input, 0.016);
        assert_eq!(camera.position, Vec3::ZERO);
        assert_eq!(camera.yaw, 0.0);
    }

    #[test]
    fn pressing_forward_moves_along_the_view_direction() {
        let (mut camera, mut input, controller) = setup();
        input.press(KeyCode::KeyW);

        controller.apply(&mut camera, &mut input, 1.0);

        // Default orientation looks down -Z.
        assert!(camera.position.z < 0.0, "moved {:?}", camera.position);
        assert!((camera.position.length() - controller.speed).abs() < 1e-3);
    }

    #[test]
    fn movement_scales_with_elapsed_time() {
        let (mut camera, mut input, controller) = setup();
        input.press(KeyCode::KeyW);

        controller.apply(&mut camera, &mut input, 0.5);
        let half = camera.position.length();

        camera.position = Vec3::ZERO;
        input.press(KeyCode::KeyW);
        controller.apply(&mut camera, &mut input, 1.0);
        let full = camera.position.length();

        assert!((full - half * 2.0).abs() < 1e-3, "{half} then {full}");
    }

    #[test]
    fn sprinting_is_faster_than_walking() {
        let (mut camera, mut input, controller) = setup();
        input.press(KeyCode::KeyW);
        controller.apply(&mut camera, &mut input, 1.0);
        let walk = camera.position.length();

        camera.position = Vec3::ZERO;
        input.press(KeyCode::ControlLeft);
        controller.apply(&mut camera, &mut input, 1.0);
        let sprint = camera.position.length();

        assert!((sprint / walk - controller.sprint_multiplier).abs() < 1e-3);
    }

    #[test]
    fn looking_up_does_not_change_where_forward_takes_you() {
        // Fly movement is level: pitching up then pressing forward should not
        // gain altitude, or the camera drifts skyward whenever you look up.
        let (mut camera, mut input, controller) = setup();
        camera.pitch = 1.0;
        input.press(KeyCode::KeyW);

        controller.apply(&mut camera, &mut input, 1.0);

        assert!(
            camera.position.y.abs() < 1e-5,
            "forward movement gained {} altitude",
            camera.position.y
        );
    }

    #[test]
    fn space_and_shift_move_straight_up_and_down() {
        let (mut camera, mut input, controller) = setup();
        input.press(KeyCode::Space);
        controller.apply(&mut camera, &mut input, 1.0);
        assert!(camera.position.y > 0.0);
        assert!(camera.position.x.abs() < 1e-6 && camera.position.z.abs() < 1e-6);

        input.release(KeyCode::Space);
        input.press(KeyCode::ShiftLeft);
        controller.apply(&mut camera, &mut input, 1.0);
        assert!(camera.position.y.abs() < 1e-5, "up then down should cancel");
    }

    #[test]
    fn mouse_look_only_applies_while_captured() {
        let (mut camera, mut input, controller) = setup();

        input.mouse_captured = false;
        input.add_mouse_delta(100.0, 50.0);
        controller.apply(&mut camera, &mut input, 0.016);
        assert_eq!(camera.yaw, 0.0, "view moved while the mouse was free");

        input.mouse_captured = true;
        input.add_mouse_delta(100.0, 0.0);
        controller.apply(&mut camera, &mut input, 0.016);
        assert!(camera.yaw > 0.0, "view did not turn while captured");
    }

    #[test]
    fn releasing_the_mouse_does_not_bank_up_motion_for_later() {
        // Motion arriving while uncaptured must be discarded, not replayed as
        // a snap the moment capture is re-enabled.
        let (mut camera, mut input, controller) = setup();
        input.mouse_captured = false;
        input.add_mouse_delta(500.0, 500.0);
        controller.apply(&mut camera, &mut input, 0.016);

        input.mouse_captured = true;
        controller.apply(&mut camera, &mut input, 0.016);

        assert_eq!(camera.yaw, 0.0);
        assert_eq!(camera.pitch, 0.0);
    }

    #[test]
    fn moving_the_mouse_down_pitches_the_view_down() {
        let (mut camera, mut input, controller) = setup();
        input.mouse_captured = true;
        input.add_mouse_delta(0.0, 100.0);

        controller.apply(&mut camera, &mut input, 0.016);

        assert!(camera.pitch < 0.0, "inverted vertical look");
    }

    #[test]
    fn pitch_cannot_pass_straight_up() {
        let (mut camera, mut input, controller) = setup();
        input.mouse_captured = true;
        for _ in 0..200 {
            input.add_mouse_delta(0.0, -500.0);
            controller.apply(&mut camera, &mut input, 0.016);
        }
        assert!(camera.pitch < std::f32::consts::FRAC_PI_2);
        assert!(camera.forward().is_finite());
    }

    fn walk_world() -> World {
        let mut world = World::new(2024);
        world.load_around(vx_core::ChunkPos::new(0, 0), 1);
        world
    }

    /// A body standing on the terrain at the origin, already settled.
    fn standing(world: &World) -> PlayerBody {
        let surface = world.surface_y(0, 0).expect("origin chunk is loaded");
        let mut player = PlayerBody {
            position: Vec3::new(0.5, surface as f32, 0.5),
            ..PlayerBody::default()
        };
        for _ in 0..60 {
            player.step(world, Vec3::ZERO, 1.0 / 60.0);
        }
        player
    }

    #[test]
    fn movement_mode_toggles_between_the_two_states() {
        assert_eq!(MovementMode::Fly.toggled(), MovementMode::Walk);
        assert_eq!(MovementMode::Walk.toggled(), MovementMode::Fly);
        assert_eq!(MovementMode::Fly.toggled().toggled(), MovementMode::Fly);
    }

    #[test]
    fn walking_moves_the_body_and_carries_the_camera_along() {
        let world = walk_world();
        let mut player = standing(&world);
        let mut camera = Camera { yaw: 0.0, pitch: 0.0, ..Camera::default() };
        let mut input = InputState::new();
        let controller = WalkController::default();

        input.press(KeyCode::KeyW);
        let start = player.position;
        for _ in 0..60 {
            controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
        }

        assert!(player.position.z < start.z - 1.0, "did not walk forward");
        // The camera rides the body's eyes, not its feet.
        assert_eq!(camera.position, player.eye_position());
        assert!(camera.position.y > player.position.y);
    }

    #[test]
    fn walking_does_not_sink_into_the_ground() {
        let world = walk_world();
        let mut player = standing(&world);
        let mut camera = Camera::default();
        let mut input = InputState::new();
        let controller = WalkController::default();
        input.press(KeyCode::KeyD);

        for _ in 0..180 {
            controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
            assert!(
                !vx_world::collides(&world, &player.aabb()),
                "walked into geometry at {:?}",
                player.position
            );
        }

        // Terrain has real slopes now, so the walk may well end mid-stride down
        // a hillside — being airborne at an arbitrary tick is correct, not a
        // bug. Let go of the controls and let the body settle before checking
        // it found ground again.
        input.release(KeyCode::KeyD);
        for _ in 0..240 {
            controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
        }

        assert!(player.on_ground, "never landed after walking");
        assert!(
            !vx_world::collides(&world, &player.aabb()),
            "settled inside geometry at {:?}",
            player.position
        );
        assert!(
            player.position.y > 1.0,
            "sank through the world to y={}",
            player.position.y
        );
    }

    #[test]
    fn space_jumps_rather_than_flying_upward() {
        let world = walk_world();
        let mut player = standing(&world);
        let mut camera = Camera::default();
        let mut input = InputState::new();
        let controller = WalkController::default();

        let ground = player.position.y;
        input.press(KeyCode::Space);
        controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
        assert!(player.velocity.y > 0.0, "space did not jump");

        // Holding space must not levitate. It does hop repeatedly — each
        // landing allows the next jump, which is the usual behaviour — so the
        // check is that height stays bounded, not that the body is grounded at
        // any particular tick.
        let mut peak: f32 = ground;
        for _ in 0..300 {
            controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
            peak = peak.max(player.position.y);
        }
        assert!(peak < ground + 3.0, "held space climbed {} blocks", peak - ground);

        // Releasing it settles back onto the ground.
        input.release(KeyCode::Space);
        for _ in 0..180 {
            controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
        }
        assert!(player.on_ground, "never landed after releasing jump");
        assert!((player.position.y - ground).abs() < 0.05);
    }

    #[test]
    fn sprinting_covers_more_ground_than_walking() {
        let world = walk_world();
        let controller = WalkController::default();
        let mut camera = Camera { yaw: 0.0, pitch: 0.0, ..Camera::default() };

        let distance = |sprint: bool| {
            let mut player = standing(&world);
            let mut input = InputState::new();
            input.press(KeyCode::KeyW);
            if sprint {
                input.press(KeyCode::ControlLeft);
            }
            let start = player.position;
            let mut camera = camera;
            for _ in 0..60 {
                controller.apply(&mut camera, &mut player, &world, &mut input, 1.0 / 60.0);
            }
            (player.position - start).length()
        };

        let walked = distance(false);
        let sprinted = distance(true);
        assert!(sprinted > walked * 1.5, "walk {walked}, sprint {sprinted}");
        let _ = &mut camera;
    }

    #[test]
    fn turning_around_reverses_which_way_forward_goes() {
        let (mut camera, mut input, controller) = setup();
        camera.yaw = std::f32::consts::PI;
        input.press(KeyCode::KeyW);

        controller.apply(&mut camera, &mut input, 1.0);

        assert!(camera.position.z > 0.0, "yaw is not applied to movement");
    }
}
