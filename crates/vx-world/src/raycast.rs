//! Finding which block a ray meets.
//!
//! This is how "what am I looking at" gets answered: a ray from the eye, and
//! the first filled voxel along it.
//!
//! Stepping the ray in fixed increments and sampling would be simpler, but it
//! is wrong at both ends — a step small enough not to tunnel through a corner
//! wastes most of its samples inside the block it already found, and any step
//! at all can skip a block the ray clips diagonally. This walks voxel to voxel
//! instead (Amanatides & Woo), visiting every voxel the ray actually touches,
//! in order, and no others.

use glam::Vec3;
use vx_core::{BlockPos, Face};

/// Where a ray met a filled block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The block the ray stopped in.
    pub block: BlockPos,
    /// The face the ray entered through.
    ///
    /// `None` when the ray began inside a filled block, having crossed no
    /// boundary to get there — there is no meaningful face in that case, and
    /// nowhere to build against.
    pub face: Option<Face>,
    /// Distance from the ray origin to the entry point, in blocks.
    pub distance: f32,
}

impl RayHit {
    /// The empty space against the struck face: where a placed block goes.
    pub fn placement(self) -> Option<BlockPos> {
        self.face.map(|face| self.block.neighbour(face))
    }
}

/// Walk the voxel grid along a ray, returning the first block `filled` accepts.
///
/// `direction` need not be normalised. `max_distance` is measured in blocks
/// from `origin`, so it is the player's reach.
pub fn cast_ray(
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    mut filled: impl FnMut(BlockPos) -> bool,
) -> Option<RayHit> {
    if !origin.is_finite() || !direction.is_finite() || max_distance <= 0.0 {
        return None;
    }
    let direction = direction.normalize_or_zero();
    if direction == Vec3::ZERO {
        return None;
    }

    let mut voxel = BlockPos::new(
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    );

    // Standing inside something counts as a hit, but with no entry face.
    if filled(voxel) {
        return Some(RayHit {
            block: voxel,
            face: None,
            distance: 0.0,
        });
    }

    let origin_axis = [origin.x, origin.y, origin.z];
    let direction_axis = [direction.x, direction.y, direction.z];
    let voxel_axis = [voxel.x, voxel.y, voxel.z];

    // Per axis: which way we walk, how far to the next boundary, and how far
    // between boundaries once we are on the grid.
    let mut step = [0i32; 3];
    let mut next_boundary = [f32::INFINITY; 3];
    let mut boundary_spacing = [f32::INFINITY; 3];

    for axis in 0..3 {
        let d = direction_axis[axis];
        let o = origin_axis[axis];
        let v = voxel_axis[axis] as f32;

        if d > 0.0 {
            step[axis] = 1;
            next_boundary[axis] = (v + 1.0 - o) / d;
            boundary_spacing[axis] = 1.0 / d;
        } else if d < 0.0 {
            step[axis] = -1;
            // `v - o` is negative and so is `d`, so this stays positive.
            next_boundary[axis] = (v - o) / d;
            boundary_spacing[axis] = -1.0 / d;
        }
        // A zero component never crosses a boundary on that axis, so its
        // distance stays infinite and it is never the nearest.
    }

    loop {
        // Cross whichever boundary is nearest. At least one axis is finite,
        // because a wholly zero direction returned above.
        let axis = if next_boundary[0] < next_boundary[1] {
            if next_boundary[0] < next_boundary[2] {
                0
            } else {
                2
            }
        } else if next_boundary[1] < next_boundary[2] {
            1
        } else {
            2
        };

        let distance = next_boundary[axis];
        if distance > max_distance {
            return None;
        }

        match axis {
            0 => voxel.x += step[0],
            1 => voxel.y += step[1],
            _ => voxel.z += step[2],
        }
        // Monotonically increasing, which is what guarantees this terminates.
        next_boundary[axis] += boundary_spacing[axis];

        // We entered through the face on the side we came from, so it points
        // back along our direction of travel.
        let face = match (axis, step[axis] > 0) {
            (0, true) => Face::NegX,
            (0, false) => Face::PosX,
            (1, true) => Face::NegY,
            (1, false) => Face::PosY,
            (2, true) => Face::NegZ,
            _ => Face::PosZ,
        };

        if filled(voxel) {
            return Some(RayHit {
                block: voxel,
                face: Some(face),
                distance,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A world made of exactly the listed blocks.
    fn world_of(blocks: &[BlockPos]) -> impl Fn(BlockPos) -> bool + '_ {
        let set: HashSet<BlockPos> = blocks.iter().copied().collect();
        move |pos| set.contains(&pos)
    }

    #[test]
    fn an_empty_world_is_never_hit() {
        let hit = cast_ray(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 100.0, |_| false);
        assert!(hit.is_none());
    }

    #[test]
    fn a_block_straight_ahead_is_hit_on_its_near_face() {
        let target = BlockPos::new(5, 0, 0);
        let hit = cast_ray(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::X,
            10.0,
            world_of(&[target]),
        )
        .expect("the ray should reach the block");

        assert_eq!(hit.block, target);
        assert_eq!(hit.face, Some(Face::NegX), "hit the wrong side");
        // Entry is at x = 5, from x = 0.5.
        assert!((hit.distance - 4.5).abs() < 1e-4, "distance {}", hit.distance);
    }

    #[test]
    fn the_nearest_block_wins() {
        let near = BlockPos::new(3, 0, 0);
        let far = BlockPos::new(6, 0, 0);
        let hit = cast_ray(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::X,
            20.0,
            world_of(&[far, near]),
        )
        .unwrap();

        assert_eq!(hit.block, near, "the ray passed through the nearer block");
    }

    #[test]
    fn reach_is_respected() {
        let target = BlockPos::new(10, 0, 0);
        let cast = |reach| cast_ray(Vec3::new(0.5, 0.5, 0.5), Vec3::X, reach, world_of(&[target]));

        // Entry face is at x = 10, i.e. 9.5 away.
        assert!(cast(9.0).is_none(), "reached a block beyond the limit");
        assert!(cast(10.0).is_some(), "failed to reach a block inside the limit");
    }

    #[test]
    fn every_face_is_reported_from_the_right_direction() {
        // Getting any of these backwards puts placed blocks inside the surface
        // you clicked instead of against it.
        let cases = [
            (Vec3::X, BlockPos::new(4, 0, 0), Face::NegX),
            (Vec3::NEG_X, BlockPos::new(-4, 0, 0), Face::PosX),
            (Vec3::Y, BlockPos::new(0, 4, 0), Face::NegY),
            (Vec3::NEG_Y, BlockPos::new(0, -4, 0), Face::PosY),
            (Vec3::Z, BlockPos::new(0, 0, 4), Face::NegZ),
            (Vec3::NEG_Z, BlockPos::new(0, 0, -4), Face::PosZ),
        ];

        for (direction, target, expected) in cases {
            let hit = cast_ray(
                Vec3::new(0.5, 0.5, 0.5),
                direction,
                10.0,
                world_of(&[target]),
            )
            .unwrap_or_else(|| panic!("no hit travelling {direction:?}"));

            assert_eq!(hit.block, target, "travelling {direction:?}");
            assert_eq!(hit.face, Some(expected), "travelling {direction:?}");
        }
    }

    #[test]
    fn the_entry_face_always_points_back_towards_the_origin() {
        // A property version of the case above, across a fan of directions.
        let origin = Vec3::new(0.5, 0.5, 0.5);
        for x in -2..=2 {
            for y in -2..=2 {
                for z in -2..=2 {
                    let direction = Vec3::new(x as f32, y as f32, z as f32);
                    if direction == Vec3::ZERO {
                        continue;
                    }
                    let hit = cast_ray(origin, direction, 40.0, |pos| {
                        pos != BlockPos::new(0, 0, 0) && pos.y.abs() < 30
                    });
                    let Some(hit) = hit else { continue };
                    let Some(face) = hit.face else { continue };

                    let normal = Vec3::from(face.normal());
                    assert!(
                        normal.dot(direction) < 0.0,
                        "face {face:?} faces away from a ray going {direction:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn placement_sits_against_the_struck_face() {
        let target = BlockPos::new(5, 0, 0);
        let hit = cast_ray(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 10.0, world_of(&[target])).unwrap();

        // One step back toward the player, never inside the block itself.
        assert_eq!(hit.placement(), Some(BlockPos::new(4, 0, 0)));
    }

    #[test]
    fn starting_inside_a_block_hits_it_with_no_face() {
        let inside = BlockPos::new(0, 0, 0);
        let hit = cast_ray(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 10.0, world_of(&[inside])).unwrap();

        assert_eq!(hit.block, inside);
        assert_eq!(hit.face, None);
        assert_eq!(hit.distance, 0.0);
        // Nothing to build against, so placement must refuse rather than
        // dropping a block on top of the player.
        assert_eq!(hit.placement(), None);
    }

    #[test]
    fn a_diagonal_ray_visits_every_voxel_it_touches() {
        // The tunnelling case: a wall one block thick, met at an angle. A
        // fixed-step sampler skips this; a grid walk cannot.
        let wall: Vec<BlockPos> = (-10..10)
            .flat_map(|y| (-10..10).map(move |z| BlockPos::new(4, y, z)))
            .collect();

        for offset in 0..20 {
            let direction = Vec3::new(1.0, offset as f32 * 0.05, offset as f32 * 0.03);
            let hit = cast_ray(Vec3::new(0.5, 0.5, 0.5), direction, 30.0, world_of(&wall));
            let hit = hit.unwrap_or_else(|| panic!("tunnelled through the wall at {direction:?}"));
            assert_eq!(hit.block.x, 4);
        }
    }

    #[test]
    fn axis_aligned_rays_do_not_drift_off_their_axis() {
        // A zero direction component must never accumulate steps, or a ray
        // fired straight down slowly wanders sideways.
        let hit = cast_ray(
            Vec3::new(0.5, 40.0, 0.5),
            Vec3::NEG_Y,
            60.0,
            world_of(&[BlockPos::new(0, 0, 0)]),
        )
        .unwrap();

        assert_eq!(hit.block, BlockPos::new(0, 0, 0));
        assert_eq!(hit.face, Some(Face::PosY));
    }

    #[test]
    fn negative_coordinates_walk_the_same_as_positive_ones() {
        // Euclidean flooring at the origin: a ray crossing zero must not skip
        // or repeat the voxel on either side of it.
        let target = BlockPos::new(-5, -1, -1);
        let hit = cast_ray(
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::NEG_X,
            10.0,
            world_of(&[target]),
        )
        .unwrap();

        assert_eq!(hit.block, target);
        assert_eq!(hit.face, Some(Face::PosX));
    }

    #[test]
    fn a_ray_never_skips_a_voxel_on_its_path() {
        // Record the walk and check it is a connected chain of single steps.
        let mut visited = Vec::new();
        cast_ray(
            Vec3::new(0.3, 0.7, 0.1),
            Vec3::new(0.7, 0.35, 0.6),
            25.0,
            |pos| {
                visited.push(pos);
                false
            },
        );

        assert!(visited.len() > 20, "only {} voxels walked", visited.len());
        for pair in visited.windows(2) {
            let manhattan = (pair[1].x - pair[0].x).abs()
                + (pair[1].y - pair[0].y).abs()
                + (pair[1].z - pair[0].z).abs();
            assert_eq!(manhattan, 1, "jumped from {:?} to {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn degenerate_inputs_are_refused_rather_than_looping_forever() {
        let solid = |_: BlockPos| true;
        let origin = Vec3::new(0.5, 0.5, 0.5);

        // A zero direction has no axis to advance along.
        assert!(cast_ray(origin, Vec3::ZERO, 10.0, |_| false).is_none());
        // Non-positive reach touches nothing, even standing in a block.
        assert!(cast_ray(origin, Vec3::X, 0.0, solid).is_none());
        assert!(cast_ray(origin, Vec3::X, -1.0, solid).is_none());
        // NaN and infinity must not reach the stepping loop.
        assert!(cast_ray(origin, Vec3::new(f32::NAN, 0.0, 0.0), 10.0, solid).is_none());
        assert!(cast_ray(origin, Vec3::new(f32::INFINITY, 0.0, 0.0), 10.0, solid).is_none());
        assert!(cast_ray(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::X, 10.0, solid).is_none());
    }

    #[test]
    fn an_unnormalised_direction_gives_the_same_answer() {
        // Distance is in blocks, so it must not scale with the input vector.
        let target = BlockPos::new(5, 0, 0);
        let unit = cast_ray(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 10.0, world_of(&[target])).unwrap();
        let long = cast_ray(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::X * 37.0,
            10.0,
            world_of(&[target]),
        )
        .unwrap();

        assert_eq!(unit.block, long.block);
        assert_eq!(unit.face, long.face);
        assert!((unit.distance - long.distance).abs() < 1e-4);
    }
}
