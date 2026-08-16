//! Turning input into camera motion.
//!
//! Separated from the event loop so it can be tested without a window: the
//! logic is a pure function of (camera, input, elapsed time).

use glam::Vec3;
use vx_platform::InputState;
use vx_render::Camera;

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
        // Mouse look is consumed even when not captured, so that queued motion
        // does not snap the view the instant capture is enabled.
        let (dx, dy) = input.take_mouse_delta();
        if input.mouse_captured {
            camera.yaw += dx * self.sensitivity;
            // Screen y grows downward, so moving the mouse down should pitch
            // the view down.
            camera.pitch -= dy * self.sensitivity;
            camera.clamp_pitch();
        }

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

    #[test]
    fn turning_around_reverses_which_way_forward_goes() {
        let (mut camera, mut input, controller) = setup();
        camera.yaw = std::f32::consts::PI;
        input.press(KeyCode::KeyW);

        controller.apply(&mut camera, &mut input, 1.0);

        assert!(camera.position.z > 0.0, "yaw is not applied to movement");
    }
}

/// Ground-based movement: gravity, jumping, and walking that collides.
///
/// Owns the feel constants; the collision itself lives in [`vx_world::Body`].
#[derive(Debug, Clone, Copy)]
pub struct WalkController {
    /// Blocks per second on the ground.
    pub walk_speed: f32,
    pub sprint_multiplier: f32,
    /// Upward velocity applied on jump. 8.5 at this gravity clears just over
    /// a block, which is the point of jumping in a world of one-block steps.
    pub jump_speed: f32,
    /// Blocks per second squared, downward.
    pub gravity: f32,
    /// How quickly velocity converges on intent: per-second lerp rates.
    pub ground_accel: f32,
    pub air_accel: f32,
    pub sensitivity: f32,
    /// Eye height above the feet, for the camera.
    pub eye_height: f32,
}

impl Default for WalkController {
    fn default() -> Self {
        WalkController {
            walk_speed: 4.3,
            sprint_multiplier: 1.6,
            jump_speed: 8.5,
            gravity: 30.0,
            ground_accel: 12.0,
            air_accel: 2.5,
            sensitivity: 0.0022,
            eye_height: 1.62,
        }
    }
}

impl WalkController {
    /// Advance the body by `dt` seconds of input, then place the camera at
    /// its eyes.
    pub fn apply(
        &self,
        camera: &mut Camera,
        body: &mut vx_world::Body,
        input: &mut InputState,
        world: &vx_world::World,
        dt: f32,
    ) {
        let (dx, dy) = input.take_mouse_delta();
        if input.mouse_captured {
            camera.yaw += dx * self.sensitivity;
            camera.pitch -= dy * self.sensitivity;
            camera.clamp_pitch();
        }

        // In water: sink slowly, and Space swims up instead of jumping.
        let water = world.generator().blocks().water;
        let feet = vx_core::BlockPos::new(
            body.position.x.floor() as i32,
            (body.position.y + 0.4).floor() as i32,
            body.position.z.floor() as i32,
        );
        let swimming = world.block(feet) == water;

        // Horizontal intent in world space, level with the ground.
        let axes = input.movement_axes();
        let mut speed = self.walk_speed;
        if input.is_sprinting() {
            speed *= self.sprint_multiplier;
        }
        if swimming {
            speed *= 0.5;
        }
        let intent = (camera.right() * axes.x + camera.forward_level() * axes.z)
            .clamp_length_max(1.0)
            * speed;

        // Converge on the intent rather than snapping to it: ground turns are
        // quick, air control is deliberately weak.
        let rate = if body.on_ground {
            self.ground_accel
        } else {
            self.air_accel
        };
        let blend = (rate * dt).min(1.0);
        body.velocity.x += (intent.x - body.velocity.x) * blend;
        body.velocity.z += (intent.z - body.velocity.z) * blend;

        if swimming {
            // Buoyancy fights gravity to a slow sink; Space paddles upward.
            body.velocity.y -= self.gravity * 0.25 * dt;
            body.velocity.y = body.velocity.y.max(-3.0);
            if input.is_down(winit::keyboard::KeyCode::Space) {
                body.velocity.y = (body.velocity.y + 24.0 * dt).min(3.5);
            }
        } else {
            body.velocity.y -= self.gravity * dt;
            if body.on_ground && input.is_down(winit::keyboard::KeyCode::Space) {
                body.velocity.y = self.jump_speed;
            }
        }

        // Unloaded chunks are solid, so nobody falls into terrain that has
        // not streamed in yet: the body stands on the void until it loads.
        body.step(dt, |pos| {
            !world.is_loaded(pos.chunk()) || world.is_solid(pos)
        });

        camera.position = body.position + glam::Vec3::new(0.0, self.eye_height, 0.0);
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use glam::Vec3;
    use vx_core::ChunkPos;
    use vx_world::{Body, World};

    /// A loaded world, a body standing on its surface, and the fixings.
    fn setup() -> (World, Body, Camera, InputState, WalkController) {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 1);
        let surface = world.surface_y(8, 8).unwrap();
        let body = Body::player(Vec3::new(8.5, surface as f32 + 0.5, 8.5));
        (
            world,
            body,
            Camera::default(),
            InputState::new(),
            WalkController::default(),
        )
    }

    /// Run enough frames for gravity to settle the body onto the ground.
    fn settle(
        controller: &WalkController,
        camera: &mut Camera,
        body: &mut Body,
        input: &mut InputState,
        world: &World,
    ) {
        for _ in 0..60 {
            controller.apply(camera, body, input, world, 0.016);
        }
    }

    #[test]
    fn gravity_settles_the_body_onto_the_terrain() {
        let (world, mut body, mut camera, mut input, controller) = setup();
        settle(&controller, &mut camera, &mut body, &mut input, &world);

        assert!(body.on_ground, "never landed");
        let surface = world.surface_y(8, 8).unwrap();
        assert!(
            (body.position.y - surface as f32).abs() < 0.1,
            "resting at {} instead of the surface at {surface}",
            body.position.y
        );
        // And the camera is at eye height above the feet.
        assert!(
            (camera.position.y - body.position.y - controller.eye_height).abs() < 1e-4,
            "camera is not at the eyes"
        );
    }

    #[test]
    fn walking_moves_and_stopping_stops() {
        let (world, mut body, mut camera, mut input, controller) = setup();
        settle(&controller, &mut camera, &mut body, &mut input, &world);

        input.press(winit::keyboard::KeyCode::KeyW);
        let start = body.position;
        for _ in 0..60 {
            controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
        }
        let walked = (body.position - start).length();
        assert!(walked > 2.0, "only moved {walked} blocks in a second");

        input.release(winit::keyboard::KeyCode::KeyW);
        for _ in 0..60 {
            controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
        }
        let horizontal = Vec3::new(body.velocity.x, 0.0, body.velocity.z).length();
        assert!(horizontal < 0.2, "still sliding at {horizontal} blocks/s");
    }

    #[test]
    fn jumping_leaves_the_ground_and_lands_again() {
        let (world, mut body, mut camera, mut input, controller) = setup();
        settle(&controller, &mut camera, &mut body, &mut input, &world);
        let rest_y = body.position.y;

        input.press(winit::keyboard::KeyCode::Space);
        controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
        input.release(winit::keyboard::KeyCode::Space);

        let mut apex: f32 = rest_y;
        for _ in 0..120 {
            controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
            apex = apex.max(body.position.y);
        }

        assert!(apex - rest_y >= 1.0, "jump only reached {} above rest", apex - rest_y);
        assert!(body.on_ground, "never landed after the jump");
    }

    #[test]
    fn holding_space_bounces_rather_than_ascending() {
        // Space only jumps from the ground, so the apex over a long hold must
        // stay a single jump high rather than climbing.
        let (world, mut body, mut camera, mut input, controller) = setup();
        settle(&controller, &mut camera, &mut body, &mut input, &world);
        let rest_y = body.position.y;

        input.press(winit::keyboard::KeyCode::Space);
        let mut apex: f32 = rest_y;
        for _ in 0..240 {
            controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
            apex = apex.max(body.position.y);
        }

        assert!(
            apex - rest_y < 2.0,
            "holding space climbed {} blocks",
            apex - rest_y
        );
    }

    #[test]
    fn the_edge_of_the_loaded_world_is_solid_ground() {
        // Standing where the neighbouring chunk has not streamed in yet must
        // not drop the player into the terrain that will appear there.
        let (world, _, mut camera, mut input, controller) = setup();
        // Well outside the loaded radius.
        let mut body = Body::player(Vec3::new(80.5, 200.0, 80.5));

        for _ in 0..30 {
            controller.apply(&mut camera, &mut body, &mut input, &world, 0.016);
        }

        assert!(
            body.position.y > 199.0,
            "fell to {} through an unloaded chunk",
            body.position.y
        );
    }
}
