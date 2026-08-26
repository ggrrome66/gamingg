//! Camera and its GPU uniform.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// A yaw/pitch fly camera.
///
/// Yaw turns around the world's up axis; pitch is clamped just short of
/// straight up or down, because exactly vertical makes the view matrix's
/// up-vector degenerate and the image flips.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    /// Radians, counter-clockwise from -Z.
    pub yaw: f32,
    /// Radians, positive looking up.
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub znear: f32,
    pub zfar: f32,
    /// Roll about the view axis, radians, positive leaning right.
    ///
    /// Presentation only — a slide's tilt, a strafe's lean. Deliberately not
    /// part of [`Camera::forward`]: that vector is what the game *aims* with,
    /// and a shot that drifted with the walk cycle would be a bug wearing a
    /// sensation's clothes. Only [`Camera::view_matrix`] reads this.
    pub roll: f32,
    /// Pitch added for the view alone, radians. Same contract as `roll`: the
    /// walk cycle's swing moves the picture, never the aim.
    pub pitch_offset: f32,
}

/// How close to vertical the pitch may get, in radians.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: Vec3::new(0.0, 80.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            aspect: 16.0 / 9.0,
            znear: 0.1,
            zfar: 1000.0,
            roll: 0.0,
            pitch_offset: 0.0,
        }
    }
}

impl Camera {
    /// Unit vector the camera looks along.
    pub fn forward(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_pitch * sin_yaw, sin_pitch, -cos_pitch * cos_yaw).normalize()
    }

    /// Unit vector to the camera's right, level with the horizon.
    pub fn right(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_yaw, 0.0, sin_yaw).normalize()
    }

    /// Forward projected onto the horizontal plane, for walk-style movement
    /// that does not drift upward when looking up.
    pub fn forward_level(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(sin_yaw, 0.0, -cos_yaw).normalize()
    }

    /// Clamp pitch away from vertical. Call after any pitch change.
    pub fn clamp_pitch(&mut self) {
        self.pitch = self.pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// The direction the *picture* faces: `forward`, plus whatever the feel
    /// layer is adding this frame. Everything that aims uses `forward`; only
    /// the view matrix comes through here.
    pub fn view_direction(&self) -> Vec3 {
        if self.pitch_offset == 0.0 {
            return self.forward();
        }
        let pitch = (self.pitch + self.pitch_offset).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_pitch * sin_yaw, sin_pitch, -cos_pitch * cos_yaw).normalize()
    }

    /// Which way is up for the view, once roll is applied. Rolling about the
    /// view axis keeps the direction fixed and turns the horizon, which is the
    /// difference between leaning into a slide and steering into one.
    pub fn view_up(&self) -> Vec3 {
        if self.roll == 0.0 {
            return Vec3::Y;
        }
        glam::Quat::from_axis_angle(self.view_direction(), self.roll) * Vec3::Y
    }

    pub fn view_matrix(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(self.position, self.view_direction(), self.view_up())
    }

    pub fn projection_matrix(&self) -> Mat4 {
        // The `directx` variant is the convention wgpu uses: Y-up NDC with
        // depth in 0..1. `opengl` would give -1..1 depth and `vulkan` is
        // Y-down, either of which renders the world flipped or depth-broken.
        glam::camera::rh::proj::directx::perspective(
            self.fov_y,
            self.aspect.max(0.001),
            self.znear,
            self.zfar,
        )
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    pub fn uniform(&self) -> CameraUniform {
        CameraUniform {
            view_projection: self.view_projection().to_cols_array_2d(),
            position: self.position.extend(1.0).into(),
        }
    }
}

/// Camera data as the shader sees it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
    pub view_projection: [[f32; 4]; 4],
    pub position: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_camera_looks_along_negative_z() {
        let camera = Camera::default();
        let forward = camera.forward();
        assert!((forward - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5, "got {forward:?}");
    }

    #[test]
    fn yawing_a_quarter_turn_looks_along_positive_x() {
        let camera = Camera {
            yaw: std::f32::consts::FRAC_PI_2,
            ..Camera::default()
        };
        let forward = camera.forward();
        assert!((forward - Vec3::X).length() < 1e-5, "got {forward:?}");
    }

    #[test]
    fn right_is_perpendicular_to_forward_and_level() {
        for yaw_steps in 0..16 {
            let camera = Camera {
                yaw: yaw_steps as f32 * 0.4,
                pitch: 0.3,
                ..Camera::default()
            };
            let right = camera.right();
            assert!(right.y.abs() < 1e-6, "right drifted off the horizon");
            assert!(
                camera.forward().dot(right).abs() < 1e-5,
                "right is not perpendicular to forward"
            );
        }
    }

    #[test]
    fn level_forward_ignores_pitch() {
        let looking_up = Camera { pitch: 1.2, ..Camera::default() };
        let looking_down = Camera { pitch: -1.2, ..Camera::default() };
        assert!((looking_up.forward_level() - looking_down.forward_level()).length() < 1e-6);
        assert!(looking_up.forward_level().y.abs() < 1e-6);
    }

    #[test]
    fn pitch_is_clamped_short_of_vertical() {
        // Straight up degenerates the view matrix and flips the image.
        let mut camera = Camera { pitch: 10.0, ..Camera::default() };
        camera.clamp_pitch();
        assert!(camera.pitch < std::f32::consts::FRAC_PI_2);
        assert!(camera.forward().is_finite());

        camera.pitch = -10.0;
        camera.clamp_pitch();
        assert!(camera.pitch > -std::f32::consts::FRAC_PI_2);
        assert!(camera.view_matrix().is_finite());
    }

    #[test]
    fn forward_stays_a_unit_vector_across_the_whole_range() {
        for yaw_steps in -8..8 {
            for pitch_steps in -8..8 {
                let camera = Camera {
                    yaw: yaw_steps as f32 * 0.7,
                    pitch: (pitch_steps as f32 * 0.19).clamp(-PITCH_LIMIT, PITCH_LIMIT),
                    ..Camera::default()
                };
                assert!((camera.forward().length() - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_point_ahead_projects_into_the_clip_volume() {
        // The real check that the projection convention matches wgpu: a point
        // in front of the camera must land inside clip space with 0 <= z <= w.
        let camera = Camera::default();
        let ahead = camera.position + camera.forward() * 10.0;
        let clip = camera.view_projection() * ahead.extend(1.0);

        assert!(clip.w > 0.0, "point ahead landed behind the camera");
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0, "point off screen: {ndc:?}");
        assert!(
            (0.0..=1.0).contains(&ndc.z),
            "depth {} is outside wgpu's 0..1 range",
            ndc.z
        );
    }

    #[test]
    fn a_point_behind_the_camera_is_clipped() {
        let camera = Camera::default();
        let behind = camera.position - camera.forward() * 10.0;
        let clip = camera.view_projection() * behind.extend(1.0);
        assert!(clip.w < 0.0, "point behind the camera should have negative w");
    }

    #[test]
    fn nearer_geometry_gets_a_smaller_depth() {
        // Depth ordering drives the depth test; inverted, everything renders
        // back to front.
        let camera = Camera::default();
        let near = camera.position + camera.forward() * 5.0;
        let far = camera.position + camera.forward() * 50.0;

        let depth_of = |point: Vec3| {
            let clip = camera.view_projection() * point.extend(1.0);
            clip.z / clip.w
        };
        assert!(depth_of(near) < depth_of(far), "depth is inverted");
    }

    #[test]
    fn roll_and_pitch_offset_never_move_the_aim() {
        // The contract the feel pass rests on. `forward` is what the game
        // shoots and digs along; if a walk cycle could nudge it, every effect
        // in the feel layer would be a gameplay change in disguise.
        let plain = Camera {
            yaw: 0.9,
            pitch: -0.3,
            ..Camera::default()
        };
        let leaning = Camera {
            roll: 0.25,
            pitch_offset: 0.05,
            ..plain
        };

        assert_eq!(plain.forward(), leaning.forward(), "roll moved the aim");
        assert_eq!(plain.right(), leaning.right(), "roll moved the strafe axis");
        assert_eq!(
            plain.forward_level(),
            leaning.forward_level(),
            "roll moved the walk direction"
        );
        // But the picture did move, or the effect would be doing nothing.
        assert_ne!(
            plain.view_matrix(),
            leaning.view_matrix(),
            "the lean never reached the view"
        );
    }

    #[test]
    fn rolling_turns_the_horizon_without_turning_the_camera() {
        let upright = Camera { yaw: 0.4, ..Camera::default() };
        let rolled = Camera { roll: 0.5, ..upright };

        // Same direction of view...
        let direction = rolled.view_direction();
        assert!(
            (direction - upright.forward()).length() < 1e-6,
            "rolling changed where the camera looks"
        );
        // ...but a tilted up vector, still perpendicular to it and still unit.
        let up = rolled.view_up();
        assert!((up.length() - 1.0).abs() < 1e-5, "up vector is not unit");
        assert!(up.dot(direction).abs() < 1e-5, "up is not perpendicular to the view");
        assert!(up.dot(Vec3::Y) < 0.9999, "the horizon did not tilt");
    }

    #[test]
    fn a_default_camera_is_upright_and_unrolled() {
        let camera = Camera::default();
        assert_eq!(camera.roll, 0.0);
        assert_eq!(camera.pitch_offset, 0.0);
        assert_eq!(camera.view_up(), Vec3::Y);
        assert_eq!(camera.view_direction(), camera.forward());
    }

    #[test]
    fn a_view_pitch_offset_cannot_flip_the_camera_over_the_top() {
        // The same clamp the real pitch gets. A bob applied at the top of a
        // look-up would otherwise tip the view past vertical and spin the
        // world round.
        let camera = Camera {
            pitch: PITCH_LIMIT,
            pitch_offset: 1.0,
            ..Camera::default()
        };
        let direction = camera.view_direction();
        assert!(direction.y < 1.0, "the view went past vertical");
        assert!((direction.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_uniform_is_plain_data_of_the_expected_size() {
        let uniform = Camera::default().uniform();
        assert_eq!(std::mem::size_of::<CameraUniform>(), 64 + 16);
        assert_eq!(uniform.position[3], 1.0);
        assert!(bytemuck::bytes_of(&uniform).len() == 80);
    }
}
