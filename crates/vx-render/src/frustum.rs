//! View frustum extraction and box culling.
//!
//! The renderer draws every chunk it holds, but at render distance 8 only about
//! a third of those are ever on screen. Testing each chunk's bounding box
//! against the six planes of the view volume skips the rest before they cost a
//! draw call.
//!
//! The planes come straight out of the view-projection matrix: a point is
//! inside the volume exactly when its clip coordinates satisfy `-w <= x,y <= w`
//! and `0 <= z <= w`, and each of those six inequalities rearranges into a
//! plane equation over world space. That means the frustum can never disagree
//! with what the GPU actually clips, because it is derived from the same
//! matrix.

use glam::{Mat4, Vec3, Vec4};

/// The six bounding planes of the view volume.
///
/// Each plane is `(a, b, c, d)` with the normal pointing *inward*, so a point
/// is inside when `a·x + b·y + c·z + d >= 0` for all six.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Extract the planes from a view-projection matrix.
    pub fn from_view_projection(view_projection: Mat4) -> Self {
        let m = view_projection;
        // glam stores columns, and the plane identities are written in terms of
        // matrix *rows*, so gather them explicitly rather than reaching for
        // `x_axis` and getting a column by mistake.
        let row = |i: usize| Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));

        // Left/right/bottom/top from `-w <= x,y <= w`.
        // Near is `z >= 0` and far is `z <= w`: the 0..1 depth convention wgpu
        // uses. The OpenGL-style `-w <= z` would put the near plane in the
        // wrong place and cull geometry right in front of the camera.
        let planes = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            r2,      // near
            r3 - r2, // far
        ];

        Frustum {
            planes: planes.map(normalise),
        }
    }

    /// Does an axis-aligned box touch the view volume?
    ///
    /// Conservative: it may answer `true` for a box that is in fact outside,
    /// near a frustum corner. That direction is harmless — it draws something
    /// invisible. The opposite error would erase geometry the player can see,
    /// so the test is deliberately biased toward keeping things.
    pub fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for plane in &self.planes {
            let normal = plane.truncate();

            // The corner furthest along the inward normal. If even that one is
            // behind the plane, every corner is, and the box is fully outside.
            let furthest = Vec3::new(
                if normal.x >= 0.0 { max.x } else { min.x },
                if normal.y >= 0.0 { max.y } else { min.y },
                if normal.z >= 0.0 { max.z } else { min.z },
            );

            if normal.dot(furthest) + plane.w < 0.0 {
                return false;
            }
        }
        true
    }

    /// Is a point inside the view volume?
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.truncate().dot(point) + plane.w >= 0.0)
    }

    pub fn planes(&self) -> &[Vec4; 6] {
        &self.planes
    }
}

/// Scale a plane so its normal is unit length.
///
/// Distances then come out in world units, which keeps the maths comparable
/// across planes and makes any future margin or bias meaningful.
fn normalise(plane: Vec4) -> Vec4 {
    let length = plane.truncate().length();
    if length > f32::EPSILON {
        plane / length
    } else {
        plane
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;

    fn camera() -> Camera {
        Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            aspect: 16.0 / 9.0,
            znear: 0.1,
            zfar: 1000.0,
            ..Camera::default()
        }
    }

    fn frustum() -> Frustum {
        Frustum::from_view_projection(camera().view_projection())
    }

    /// A small box centred on a point.
    fn box_at(point: Vec3, half: f32) -> (Vec3, Vec3) {
        (point - Vec3::splat(half), point + Vec3::splat(half))
    }

    #[test]
    fn every_plane_normal_is_unit_length() {
        for plane in frustum().planes() {
            let length = plane.truncate().length();
            assert!((length - 1.0).abs() < 1e-4, "plane normal length {length}");
        }
    }

    #[test]
    fn a_point_straight_ahead_is_inside() {
        // The default camera looks down -Z.
        assert!(frustum().contains_point(Vec3::new(0.0, 0.0, -10.0)));
    }

    #[test]
    fn a_point_behind_the_camera_is_outside() {
        assert!(!frustum().contains_point(Vec3::new(0.0, 0.0, 10.0)));
    }

    #[test]
    fn a_point_far_off_to_the_side_is_outside() {
        // Ten blocks ahead but a hundred to the right is well outside a 70°
        // field of view.
        assert!(!frustum().contains_point(Vec3::new(100.0, 0.0, -10.0)));
        assert!(!frustum().contains_point(Vec3::new(0.0, 100.0, -10.0)));
    }

    #[test]
    fn a_point_beyond_the_far_plane_is_outside() {
        let camera = camera();
        assert!(frustum().contains_point(Vec3::new(0.0, 0.0, -(camera.zfar - 10.0))));
        assert!(!frustum().contains_point(Vec3::new(0.0, 0.0, -(camera.zfar + 10.0))));
    }

    #[test]
    fn a_box_in_front_is_kept_and_one_behind_is_dropped() {
        let frustum = frustum();

        let (min, max) = box_at(Vec3::new(0.0, 0.0, -30.0), 8.0);
        assert!(frustum.intersects_aabb(min, max), "box ahead was culled");

        let (min, max) = box_at(Vec3::new(0.0, 0.0, 30.0), 8.0);
        assert!(!frustum.intersects_aabb(min, max), "box behind was kept");
    }

    #[test]
    fn a_box_straddling_the_edge_is_kept() {
        // Partial overlap must count as visible, or chunks would pop out at the
        // screen edge as you turn.
        let frustum = frustum();
        // Big enough to poke into the volume from off to one side.
        let (min, max) = box_at(Vec3::new(30.0, 0.0, -30.0), 25.0);
        assert!(frustum.intersects_aabb(min, max));
    }

    #[test]
    fn a_box_swallowing_the_camera_is_kept() {
        // Standing inside a chunk must not cull it.
        let frustum = frustum();
        let (min, max) = box_at(Vec3::ZERO, 50.0);
        assert!(frustum.intersects_aabb(min, max));
    }

    #[test]
    fn culling_never_drops_a_box_with_a_visible_corner() {
        // The property that actually matters. Cross-check the plane test
        // against the projection itself: if any corner of a box lands inside
        // the clip volume, the frustum must not reject that box. A false
        // positive is wasted work; a false negative is geometry vanishing from
        // the screen, so only this direction is worth asserting.
        let camera = camera();
        let view_projection = camera.view_projection();
        let frustum = Frustum::from_view_projection(view_projection);

        let corner_visible = |min: Vec3, max: Vec3| -> bool {
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { min.x } else { max.x },
                    if i & 2 == 0 { min.y } else { max.y },
                    if i & 4 == 0 { min.z } else { max.z },
                );
                let clip = view_projection * corner.extend(1.0);
                if clip.w > 0.0
                    && clip.x.abs() <= clip.w
                    && clip.y.abs() <= clip.w
                    && (0.0..=clip.w).contains(&clip.z)
                {
                    return true;
                }
            }
            false
        };

        let mut checked = 0;
        let mut visible = 0;
        for x in (-120..=120).step_by(20) {
            for y in (-120..=120).step_by(20) {
                for z in (-240..=120).step_by(20) {
                    let (min, max) = box_at(
                        Vec3::new(x as f32, y as f32, z as f32),
                        8.0,
                    );
                    checked += 1;
                    if corner_visible(min, max) {
                        visible += 1;
                        assert!(
                            frustum.intersects_aabb(min, max),
                            "culled a box with a visible corner at ({x}, {y}, {z})"
                        );
                    }
                }
            }
        }
        assert!(checked > 500, "the sweep was too small to mean anything");
        assert!(visible > 20, "only {visible} boxes were visible; sweep is off");
    }

    #[test]
    fn culling_actually_rejects_a_useful_share_of_boxes() {
        // If the test kept everything it would pass vacuously while buying no
        // performance at all.
        let frustum = frustum();

        let mut total = 0;
        let mut kept = 0;
        for x in (-200..=200).step_by(25) {
            for z in (-200..=200).step_by(25) {
                let (min, max) = box_at(Vec3::new(x as f32, 0.0, z as f32), 8.0);
                total += 1;
                if frustum.intersects_aabb(min, max) {
                    kept += 1;
                }
            }
        }

        let kept_fraction = kept as f32 / total as f32;
        assert!(
            kept_fraction < 0.5,
            "kept {:.0}% of boxes; culling is not earning its keep",
            kept_fraction * 100.0
        );
        assert!(kept > 0, "culled absolutely everything");
    }

    #[test]
    fn turning_the_camera_changes_what_survives() {
        let ahead = Vec3::new(0.0, 0.0, -40.0);
        let (min, max) = box_at(ahead, 6.0);

        let facing = Frustum::from_view_projection(camera().view_projection());
        assert!(facing.intersects_aabb(min, max));

        let turned = Camera {
            yaw: std::f32::consts::PI,
            ..camera()
        };
        let behind = Frustum::from_view_projection(turned.view_projection());
        assert!(
            !behind.intersects_aabb(min, max),
            "turning around did not change visibility"
        );
    }
}
