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
