//! Camera and its GPU uniform.
//!
//! # The camera is the origin
//!
//! Stage 9a made chunk geometry chunk-local, so a vertex is a small offset
//! plus its chunk's corner. What that left was the *camera*: the corner was
//! still added back in absolute world space in the vertex shader, and the
//! view matrix subtracted an absolute position straight after — two large
//! numbers whose difference is what you actually wanted, each rounded before
//! the subtraction. That is the `f32` precision wall, and it is a property of
//! where you do the arithmetic rather than of the numbers.
//!
//! So the renderer draws relative to **the chunk corner the camera stands
//! in**. The camera's position is `f64`, and everything it hands the GPU is
//! measured from [`Camera::origin`]: the view matrix is built from the small
//! offset inside that chunk, and the origin itself goes up as exact integers so
//! the vertex shader can subtract it from each chunk's corner *before* adding
//! the local offset. Every chunk corner is a multiple of sixteen, every
//! integer below 2^24 is exact in `f32`, and the difference of two exact
//! integers is exact — so out to 2^28 blocks nothing large ever reaches a
//! float multiply. That is 268,000 km, and the `i32` block lattice runs out
//! not far past it.
//!
//! Nothing in the world moves for this. Block coordinates stay `i32`, chunk
//! uniforms stay absolute, and no mesh is re-uploaded when the camera crosses
//! a chunk line: the subtraction happens per vertex, on the GPU, exactly.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, IVec3, Mat4, Vec3};
use vx_core::CHUNK_SIZE;

/// A yaw/pitch fly camera.
///
/// Yaw turns around the world's up axis; pitch is clamped just short of
/// straight up or down, because exactly vertical makes the view matrix's
/// up-vector degenerate and the image flips.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Absolute world position, in `f64` — the one thing in the renderer
    /// that is allowed to be far from the origin, because it is the origin.
    pub position: DVec3,
    /// Radians, counter-clockwise from -Z.
    pub yaw: f32,
    /// Radians, positive looking up.
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub znear: f32,
    pub zfar: f32,
}

/// How close to vertical the pitch may get, in radians.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: DVec3::new(0.0, 80.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            aspect: 16.0 / 9.0,
            znear: 0.1,
            zfar: 1000.0,
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

    /// The chunk corner the renderer measures this frame from.
    ///
    /// `y` stays zero: height is bounded and every chunk corner already has
    /// `y = 0`, so there is nothing to gain and one more thing to get wrong.
    pub fn origin(&self) -> IVec3 {
        let size = CHUNK_SIZE as f64;
        IVec3::new(
            ((self.position.x / size).floor() * size) as i32,
            0,
            ((self.position.z / size).floor() * size) as i32,
        )
    }

    /// A world position as the GPU will see it: measured from
    /// [`Camera::origin`], in `f32`.
    ///
    /// The subtraction happens in `f64`, so a point within a few chunks of
    /// the camera comes out with its full `f32` precision however far from
    /// the world's origin both of them are.
    pub fn relative(&self, world: DVec3) -> Vec3 {
        (world - self.origin().as_dvec3()).as_vec3()
    }

    /// The camera's own position, measured from its origin — always inside
    /// one chunk, so always small.
    pub fn local_position(&self) -> Vec3 {
        self.relative(self.position)
    }

    pub fn view_matrix(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(self.local_position(), self.forward(), Vec3::Y)
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
        let origin = self.origin();
        CameraUniform {
            view_projection: self.view_projection().to_cols_array_2d(),
            position: self.local_position().extend(1.0).into(),
            origin: [origin.x as f32, origin.y as f32, origin.z as f32, 0.0],
        }
    }
}

/// Camera data as the shader sees it.
///
/// `position` and the view-projection are measured from `origin`; `origin` is
/// the chunk corner itself, as exact integers, for the vertex shader to
/// subtract from each chunk's own corner.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
    pub view_projection: [[f32; 4]; 4],
    pub position: [f32; 4],
    pub origin: [f32; 4],
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
        let ahead = camera.position + camera.forward().as_dvec3() * 10.0;
        let clip = camera.view_projection() * camera.relative(ahead).extend(1.0);

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
        let behind = camera.position - camera.forward().as_dvec3() * 10.0;
        let clip = camera.view_projection() * camera.relative(behind).extend(1.0);
        assert!(clip.w < 0.0, "point behind the camera should have negative w");
    }

    #[test]
    fn nearer_geometry_gets_a_smaller_depth() {
        // Depth ordering drives the depth test; inverted, everything renders
        // back to front.
        let camera = Camera::default();
        let near = camera.position + camera.forward().as_dvec3() * 5.0;
        let far = camera.position + camera.forward().as_dvec3() * 50.0;

        let depth_of = |point: DVec3| {
            let clip = camera.view_projection() * camera.relative(point).extend(1.0);
            clip.z / clip.w
        };
        assert!(depth_of(near) < depth_of(far), "depth is inverted");
    }

    #[test]
    fn the_uniform_is_plain_data_of_the_expected_size() {
        let uniform = Camera::default().uniform();
        assert_eq!(std::mem::size_of::<CameraUniform>(), 64 + 16 + 16);
        assert_eq!(uniform.position[3], 1.0);
        assert!(bytemuck::bytes_of(&uniform).len() == 96);
    }

    /// The origin is the chunk corner under the camera, and the position the
    /// GPU sees is the offset inside that chunk — always small, and exact.
    #[test]
    fn the_origin_is_the_chunk_corner_and_the_local_position_is_inside_it() {
        for (x, z) in [(0.5, 0.5), (-0.5, 17.9), (3_000_000.25, -7_654_321.75)] {
            let camera = Camera {
                position: DVec3::new(x, 80.0, z),
                ..Camera::default()
            };
            let origin = camera.origin();
            assert_eq!(origin.x % CHUNK_SIZE, 0);
            assert_eq!(origin.z % CHUNK_SIZE, 0);
            assert_eq!(origin.y, 0);
            let local = camera.local_position();
            assert!((0.0..CHUNK_SIZE as f32).contains(&local.x), "{local:?}");
            assert!((0.0..CHUNK_SIZE as f32).contains(&local.z), "{local:?}");
            // And it round-trips exactly: origin + local is the position.
            let back = origin.as_dvec3() + local.as_dvec3();
            assert!((back - camera.position).length() < 1.0e-6, "{back:?} vs {}", camera.position);
        }
    }

    /// The whole point: the same scene seen from the same place relative to
    /// the camera projects to the same clip coordinates whether the pair of
    /// them stands at spawn or three thousand kilometres out. Byte-identical,
    /// not close — there is no rounding to be close about.
    #[test]
    fn projection_is_identical_at_the_origin_and_three_thousand_kilometres_out() {
        let far = DVec3::new(3_000_000.0, 0.0, 3_000_000.0);
        let here = Camera {
            position: DVec3::new(5.25, 83.5, -2.75),
            yaw: 0.7,
            pitch: -0.2,
            ..Camera::default()
        };
        let there = Camera {
            position: here.position + far,
            ..here
        };
        for (dx, dy, dz) in [(1.0, 0.0, -6.0), (-9.5, 3.0, -30.0), (12.0, -4.0, -2.0)] {
            let offset = DVec3::new(dx, dy, dz);
            let a = here.view_projection() * here.relative(here.position + offset).extend(1.0);
            let b = there.view_projection() * there.relative(there.position + offset).extend(1.0);
            assert_eq!(a, b, "the projection drifted between spawn and far out");
        }
        // The uniform's origin really is the chunk corner as exact integers.
        let uniform = there.uniform();
        assert_eq!(uniform.origin[0], 3_000_000.0);
        assert_eq!(uniform.origin[2], 2_999_984.0);
    }
}
