//! Picking blocks along a ray.
//!
//! Uses grid traversal rather than stepping a fixed distance and sampling. A
//! fixed step is wrong in both directions at once: too large and the ray tunnels
//! through a block corner without noticing, too small and it does far more work
//! than needed. Marching voxel to voxel visits every cell the ray actually
//! crosses, exactly once, in order.
//!
//! Each iteration advances whichever axis has its next grid boundary closest,
//! which is also what tells us which face the ray entered through — the
//! information needed to know where a placed block should go.
//!
//! # The ray starts from an exact cell
//!
//! The origin arrives in `f64` and is split, once, into the block it starts in
//! (exact integers) and the fraction inside that block (a small `f32`). The
//! whole march then runs in coordinates relative to that starting block and
//! adds the block back only when reporting a hit. So a ray from an eye three
//! thousand kilometres out is as precise as one at spawn: nothing large is
//! ever multiplied, and the four-millimetre slop an absolute `f32` origin
//! carries at fifty kilometres — the same class of bug as the body sticking
//! to walls — never enters the arithmetic.

use glam::{DVec3, Vec3};
use vx_core::{BlockId, BlockPos, BlockRegistry, Face};

use crate::chunk::BlockView;

/// What a ray struck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The block the ray struck.
    pub block: BlockPos,
    /// The face it entered through. Placing a block goes on this side.
    pub face: Face,
    /// Distance from the ray origin, in blocks.
    pub distance: f32,
    pub id: BlockId,
}

impl RayHit {
    /// Where a block placed against this hit would sit.
    pub fn placement(&self) -> BlockPos {
        self.block.neighbour(self.face)
    }
}

/// Cast a ray and return the first block satisfying `is_target`.
///
/// `direction` need not be normalised; `distance` is reported in blocks
/// regardless. `max_distance` bounds the search — it is also what stops the
/// traversal, since an unobstructed ray would otherwise run forever.
///
/// A ray starting inside a target block reports that block immediately, at
/// distance zero. The face is meaningless in that case and is reported as the
/// one the ray is travelling most directly against.
pub fn raycast(
    view: &impl BlockView,
    is_target: impl Fn(BlockId) -> bool,
    origin: DVec3,
    direction: Vec3,
    max_distance: f32,
) -> Option<RayHit> {
    // A zero-length ray has no direction to march along.
    if direction.length_squared() < f32::EPSILON {
        return None;
    }
    let direction = direction.normalize();

    // The block the ray starts in, as exact integers, and where inside it the
    // ray begins, as a fraction. From here on everything is measured from
    // `base`; `voxel` counts steps away from it.
    let base = [
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    ];
    let mut voxel = [0i32; 3];
    let origin = [
        (origin.x - base[0] as f64) as f32,
        (origin.y - base[1] as f64) as f32,
        (origin.z - base[2] as f64) as f32,
    ];
    let direction = [direction.x, direction.y, direction.z];

    let mut step = [0i32; 3];
    // Distance along the ray to the next grid boundary on each axis.
    let mut next_boundary = [f32::INFINITY; 3];
    // Distance along the ray between successive boundaries on each axis.
    let mut boundary_spacing = [f32::INFINITY; 3];

    for axis in 0..3 {
        if direction[axis] > 0.0 {
            step[axis] = 1;
            next_boundary[axis] = (voxel[axis] as f32 + 1.0 - origin[axis]) / direction[axis];
            boundary_spacing[axis] = 1.0 / direction[axis];
        } else if direction[axis] < 0.0 {
            step[axis] = -1;
            next_boundary[axis] = (voxel[axis] as f32 - origin[axis]) / direction[axis];
            boundary_spacing[axis] = -1.0 / direction[axis];
        }
        // A zero component never crosses a boundary on that axis, so its
        // entries stay at infinity and it is never chosen as the next step.
    }

    // The face for a hit in the starting voxel, where nothing was entered
    // through: the one most opposed to travel.
    let dominant = (0..3)
        .max_by(|a, b| direction[*a].abs().total_cmp(&direction[*b].abs()))
        .unwrap_or(0);
    let mut entered = face_of(dominant, step[dominant]);

    let mut distance = 0.0;

    loop {
        // The world-space block, for the view and the report; the relative
        // one, for the arithmetic.
        let position = BlockPos::new(base[0] + voxel[0], base[1] + voxel[1], base[2] + voxel[2]);
        let id = view.block_at(position.x, position.y, position.z);
        if is_target(id) {
            // A wounded block is not a box any more. Descend into its cells
            // and let the ray through if it found a hole — this is the whole
            // of "chip cover long enough and a firing hole opens", and it is
            // the only place in the engine that reads damage geometry.
            match view.mask_at(position.x, position.y, position.z) {
                Some(mask) => {
                    let relative = BlockPos::new(voxel[0], voxel[1], voxel[2]);
                    if let Some(cell_distance) =
                        pierce(mask, relative, &origin, &direction, distance)
                    {
                        return Some(RayHit {
                            block: position,
                            face: entered,
                            distance: cell_distance,
                            id,
                        });
                    }
                    // Every cell along the line was already gone: keep
                    // marching, the ray passed through the wound.
                }
                None => {
                    return Some(RayHit {
                        block: position,
                        face: entered,
                        distance,
                        id,
                    });
                }
            }
        }

        // Advance along whichever axis reaches its boundary soonest.
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

        distance = next_boundary[axis];
        if distance > max_distance {
            return None;
        }

        voxel[axis] += step[axis];
        next_boundary[axis] += boundary_spacing[axis];
        // Stepping in +x crosses into the new voxel through its -x face.
        entered = face_of(axis, step[axis]);
    }
}

/// March a ray through one wounded block's cells.
///
/// A four-step mini-DDA in cell space, which is small enough to walk by
/// sampling: the segment inside a block is at most `sqrt(3)` long, so
/// stepping in quarter-cell increments cannot skip a cell. Returns the
/// distance at which the ray met material, or `None` when it found only
/// holes and should carry on to the next block.
///
/// `block` and `origin` are both relative to the ray's starting block, so
/// the sampling below never leaves small numbers.
fn pierce(
    mask: crate::micro::Mask,
    block: BlockPos,
    origin: &[f32; 3],
    direction: &[f32; 3],
    entry_distance: f32,
) -> Option<f32> {
    use crate::micro::SIDE;

    // Where the ray is when it meets this block, in cell coordinates.
    let step = 0.25 / SIDE as f32;
    // The far corner of a block is sqrt(3) away; walking that at a quarter
    // of a cell is a bounded, branch-free loop.
    let steps = (3.0f32.sqrt() / step).ceil() as i32 + 1;
    for taken in 0..steps {
        let along = entry_distance + taken as f32 * step;
        let point = [
            origin[0] + direction[0] * along,
            origin[1] + direction[1] * along,
            origin[2] + direction[2] * along,
        ];
        let cell = [
            ((point[0] - block.x as f32) * SIDE as f32).floor() as i32,
            ((point[1] - block.y as f32) * SIDE as f32).floor() as i32,
            ((point[2] - block.z as f32) * SIDE as f32).floor() as i32,
        ];
        // Left the block without meeting anything: the ray is through.
        if cell.iter().any(|c| !(0..SIDE).contains(c)) {
            // Only give up once we have actually entered and left; the first
            // sample can land a hair outside on the entry face.
            if taken > 0 {
                return None;
            }
            continue;
        }
        if crate::micro::has(mask, cell[0], cell[1], cell[2]) {
            return Some(along);
        }
    }
    None
}

/// The face crossed when stepping `step` along `axis`.
fn face_of(axis: usize, step: i32) -> Face {
    match (axis, step >= 0) {
        (0, true) => Face::NegX,
        (0, false) => Face::PosX,
        (1, true) => Face::NegY,
        (1, false) => Face::PosY,
        (2, true) => Face::NegZ,
        _ => Face::PosZ,
    }
}

/// Cast against blocks that block movement, which is what pointing at the world
/// should select: solid terrain yes, water and air no.
pub fn raycast_solid(
    view: &impl BlockView,
    registry: &BlockRegistry,
    origin: DVec3,
    direction: Vec3,
    max_distance: f32,
) -> Option<RayHit> {
    raycast(
        view,
        |id| registry.is_solid(id),
        origin,
        direction,
        max_distance,
    )
}

#[cfg(test)]
mod wound_tests {
    use super::*;
    use crate::micro::{self, Mask};
    use vx_core::BlockRegistry as Registry;

    /// One block of stone at the origin, with a wound in it.
    struct WoundedBlock {
        mask: Mask,
    }

    impl BlockView for WoundedBlock {
        fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
            if (x, y, z) == (0, 0, 0) {
                BlockId(1)
            } else {
                BlockId::AIR
            }
        }
        fn mask_at(&self, x: i32, y: i32, z: i32) -> Option<Mask> {
            ((x, y, z) == (0, 0, 0)).then_some(self.mask)
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry
            .register(vx_core::BlockDef::uniform("test:stone", 0))
            .unwrap();
        registry
    }

    /// A ray straight along +x at the centre of the cell row `(y, z)`.
    fn along_x(view: &WoundedBlock, y: i32, z: i32) -> Option<RayHit> {
        let at = |cell: i32| (cell as f32 + 0.5) / micro::SIDE as f32;
        raycast_solid(
            view,
            &registry(),
            DVec3::new(-2.0, at(y) as f64, at(z) as f64),
            Vec3::new(1.0, 0.0, 0.0),
            8.0,
        )
    }

    #[test]
    fn an_intact_block_still_stops_everything() {
        // The floor this round must not move: a block nobody has shot at
        // behaves exactly as it did before micro existed.
        let view = WoundedBlock { mask: micro::FULL };
        for y in 0..micro::SIDE {
            for z in 0..micro::SIDE {
                assert!(along_x(&view, y, z).is_some(), "clear line through intact rock");
            }
        }
    }

    #[test]
    fn a_ray_passes_through_a_cleared_channel_and_nothing_else() {
        // The note's headline test. Clear one row of cells end to end and
        // that row — and only that row — lets a ray through.
        let (open_y, open_z) = (2, 1);
        let mut mask = micro::FULL;
        for x in 0..micro::SIDE {
            mask &= !micro::bit(x, open_y, open_z);
        }
        let view = WoundedBlock { mask };

        assert!(
            along_x(&view, open_y, open_z).is_none(),
            "the channel did not let the ray through"
        );
        for y in 0..micro::SIDE {
            for z in 0..micro::SIDE {
                if (y, z) == (open_y, open_z) {
                    continue;
                }
                assert!(
                    along_x(&view, y, z).is_some(),
                    "the wound leaked at {y},{z} — a peephole is not a doorway"
                );
            }
        }
    }

    #[test]
    fn one_intact_cell_in_the_channel_blocks_it_again() {
        // A firing hole is only a hole while every cell along it is gone.
        let (open_y, open_z) = (2, 1);
        let mut mask = micro::FULL;
        for x in 0..micro::SIDE {
            mask &= !micro::bit(x, open_y, open_z);
        }
        // Put one cell back in the middle of the channel.
        mask |= micro::bit(2, open_y, open_z);
        let view = WoundedBlock { mask };
        assert!(
            along_x(&view, open_y, open_z).is_some(),
            "a plugged channel still let the ray through"
        );
    }

    #[test]
    fn a_wounded_block_reports_the_distance_to_the_material_it_met() {
        // Not the block boundary: the cell. A hit two cells deep is half a
        // metre further along than the face, and cover that has been chewed
        // should read as thinner, not merely as damaged.
        let (y, z) = (1, 1);
        let mut mask = micro::FULL;
        mask &= !micro::bit(0, y, z);
        mask &= !micro::bit(1, y, z);
        let view = WoundedBlock { mask };
        let hit = along_x(&view, y, z).expect("the ray should have met the third cell");
        // Entry at x = 0 is two metres along; the third cell begins half a
        // metre into the block.
        assert!(
            (hit.distance - 2.5).abs() < 0.1,
            "met material at {} rather than half a metre in",
            hit.distance
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockDef, ChunkPos, LocalPos};

    use crate::chunk::{Chunk, SoloChunkView};

    const STONE: BlockId = BlockId(1);

    fn registry() -> BlockRegistry {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDef::uniform("test:stone", 0)).unwrap();
        registry
    }

    /// A chunk at the origin with a single stone block at `(x, y, z)`.
    fn chunk_with_block(x: i32, y: i32, z: i32) -> Chunk {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(LocalPos::new(x, y, z).unwrap(), STONE);
        chunk
    }

    fn cast(chunk: &Chunk, origin: Vec3, direction: Vec3, max: f32) -> Option<RayHit> {
        let registry = registry();
        raycast_solid(&SoloChunkView(chunk), &registry, origin.as_dvec3(), direction, max)
    }

    #[test]
    fn a_ray_through_empty_space_hits_nothing() {
        let chunk = Chunk::empty(ChunkPos::new(0, 0));
        assert!(cast(&chunk, Vec3::new(8.5, 40.5, 8.5), Vec3::NEG_Z, 50.0).is_none());
    }

    #[test]
    fn a_zero_length_direction_hits_nothing_rather_than_looping() {
        let chunk = chunk_with_block(8, 40, 8);
        assert!(cast(&chunk, Vec3::new(8.5, 40.5, 12.5), Vec3::ZERO, 50.0).is_none());
    }

    #[test]
    fn a_straight_ray_hits_the_block_in_its_path() {
        let chunk = chunk_with_block(8, 40, 4);
        // Start further along +z, look toward -z.
        let hit = cast(&chunk, Vec3::new(8.5, 40.5, 12.5), Vec3::NEG_Z, 50.0).unwrap();

        assert_eq!(hit.block, BlockPos::new(8, 40, 4));
        assert_eq!(hit.id, STONE);
        // Travelling -z, so it enters through the +z face.
        assert_eq!(hit.face, Face::PosZ);
        // From z=12.5 to the block's far edge at z=5.0.
        assert!((hit.distance - 7.5).abs() < 1e-4, "distance {}", hit.distance);
    }

    #[test]
    fn the_face_reported_matches_the_direction_of_travel() {
        let chunk = chunk_with_block(8, 40, 8);
        let centre = Vec3::new(8.5, 40.5, 8.5);

        for (offset, direction, expected) in [
            (Vec3::new(6.0, 0.0, 0.0), Vec3::NEG_X, Face::PosX),
            (Vec3::new(-6.0, 0.0, 0.0), Vec3::X, Face::NegX),
            (Vec3::new(0.0, 6.0, 0.0), Vec3::NEG_Y, Face::PosY),
            (Vec3::new(0.0, -6.0, 0.0), Vec3::Y, Face::NegY),
            (Vec3::new(0.0, 0.0, 6.0), Vec3::NEG_Z, Face::PosZ),
            (Vec3::new(0.0, 0.0, -6.0), Vec3::Z, Face::NegZ),
        ] {
            let hit = cast(&chunk, centre + offset, direction, 20.0).unwrap();
            assert_eq!(hit.block, BlockPos::new(8, 40, 8));
            assert_eq!(hit.face, expected, "wrong face approaching from {offset:?}");
        }
    }

    #[test]
    fn placement_sits_against_the_face_that_was_hit() {
        let chunk = chunk_with_block(8, 40, 8);
        // Looking down at the top of the block.
        let hit = cast(&chunk, Vec3::new(8.5, 48.0, 8.5), Vec3::NEG_Y, 20.0).unwrap();

        assert_eq!(hit.face, Face::PosY);
        assert_eq!(hit.placement(), BlockPos::new(8, 41, 8));
    }

    #[test]
    fn a_ray_stops_at_the_first_block_not_the_furthest() {
        let mut chunk = chunk_with_block(8, 40, 4);
        chunk.set(LocalPos::new(8, 40, 6).unwrap(), STONE);

        let hit = cast(&chunk, Vec3::new(8.5, 40.5, 12.5), Vec3::NEG_Z, 50.0).unwrap();

        // The nearer block is at z=6, not the one behind it at z=4.
        assert_eq!(hit.block, BlockPos::new(8, 40, 6));
    }

    #[test]
    fn max_distance_is_respected() {
        let chunk = chunk_with_block(8, 40, 4);
        let origin = Vec3::new(8.5, 40.5, 12.5);

        // The block is 7.5 blocks away.
        assert!(cast(&chunk, origin, Vec3::NEG_Z, 5.0).is_none());
        assert!(cast(&chunk, origin, Vec3::NEG_Z, 8.0).is_some());
    }

    #[test]
    fn a_ray_starting_inside_a_block_reports_it_immediately() {
        let chunk = chunk_with_block(8, 40, 8);
        let hit = cast(&chunk, Vec3::new(8.5, 40.5, 8.5), Vec3::NEG_Z, 10.0).unwrap();

        assert_eq!(hit.block, BlockPos::new(8, 40, 8));
        assert_eq!(hit.distance, 0.0);
    }

    #[test]
    fn a_diagonal_ray_finds_a_block_off_the_axes() {
        // Grid traversal must handle the case where several axes advance; a
        // naive stepper is most likely to tunnel here.
        let chunk = chunk_with_block(10, 42, 6);
        let origin = Vec3::new(8.5, 40.5, 12.5);
        let target = Vec3::new(10.5, 42.5, 6.5);

        let hit = cast(&chunk, origin, target - origin, 50.0).unwrap();
        assert_eq!(hit.block, BlockPos::new(10, 42, 6));
    }

    #[test]
    fn an_unnormalised_direction_still_reports_distance_in_blocks() {
        let chunk = chunk_with_block(8, 40, 4);
        let origin = Vec3::new(8.5, 40.5, 12.5);

        let unit = cast(&chunk, origin, Vec3::NEG_Z, 50.0).unwrap();
        let scaled = cast(&chunk, origin, Vec3::NEG_Z * 37.0, 50.0).unwrap();

        assert_eq!(unit.block, scaled.block);
        assert!((unit.distance - scaled.distance).abs() < 1e-4);
    }

    #[test]
    fn axis_aligned_rays_do_not_drift_onto_neighbouring_columns() {
        // A ray exactly along an axis has two zero direction components. Those
        // must never be selected as the stepping axis, or the ray wanders.
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for y in 0..50 {
            chunk.set(LocalPos::new(8, y, 8).unwrap(), STONE);
        }

        let hit = cast(&chunk, Vec3::new(8.5, 60.0, 8.5), Vec3::NEG_Y, 30.0).unwrap();
        assert_eq!(hit.block, BlockPos::new(8, 49, 8));
        assert_eq!(hit.face, Face::PosY);
    }

    #[test]
    fn water_is_looked_through_rather_than_selected() {
        // Non-solid blocks must not be targetable, or you cannot reach the bed
        // of a lake to build on it.
        let mut registry = BlockRegistry::new();
        let stone = registry.register(BlockDef::uniform("test:stone", 0)).unwrap();
        let water = registry
            .register(BlockDef::uniform("test:water", 1).translucent().non_solid())
            .unwrap();

        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(LocalPos::new(8, 40, 8).unwrap(), water);
        chunk.set(LocalPos::new(8, 38, 8).unwrap(), stone);

        let hit = raycast_solid(
            &SoloChunkView(&chunk),
            &registry,
            DVec3::new(8.5, 46.0, 8.5),
            Vec3::NEG_Y,
            20.0,
        )
        .unwrap();

        assert_eq!(hit.block, BlockPos::new(8, 38, 8), "the ray stopped at water");
        assert_eq!(hit.id, stone);
    }

    #[test]
    fn a_custom_predicate_can_target_anything_including_water() {
        let mut registry = BlockRegistry::new();
        let water = registry
            .register(BlockDef::uniform("test:water", 1).translucent().non_solid())
            .unwrap();

        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(LocalPos::new(8, 40, 8).unwrap(), water);

        let hit = raycast(
            &SoloChunkView(&chunk),
            |id| !id.is_air(),
            DVec3::new(8.5, 46.0, 8.5),
            Vec3::NEG_Y,
            20.0,
        )
        .unwrap();

        assert_eq!(hit.id, water);
        let _ = registry;
    }

    #[test]
    fn every_hit_lies_within_the_requested_range() {
        // Sweep a fan of directions and check the reported distance is
        // consistent with where the block actually is.
        let chunk = chunk_with_block(8, 40, 8);
        let origin = Vec3::new(8.5, 40.5, 14.5);

        for step in -20..20 {
            let direction = Vec3::new(step as f32 * 0.02, 0.0, -1.0);
            if let Some(hit) = cast(&chunk, origin, direction, 30.0) {
                assert_eq!(hit.block, BlockPos::new(8, 40, 8));
                assert!(
                    (0.0..=30.0).contains(&hit.distance),
                    "distance {} out of range",
                    hit.distance
                );
                // The hit point must actually be on the block's surface.
                let point = origin + direction.normalize() * hit.distance;
                assert!(
                    (point.z - 9.0).abs() < 1e-3,
                    "hit point {point:?} is not on the +z face"
                );
            }
        }
    }

    /// A ray from three thousand kilometres out hits the same face at the
    /// same fraction as the same ray at spawn. The origin is split into an
    /// exact cell and a fraction before anything is multiplied, so there is
    /// no distance at which the aim goes soft.
    #[test]
    fn a_ray_far_from_the_origin_is_as_exact_as_one_at_spawn() {
        use vx_core::BlockPos;
        /// A floor at y = 77, everywhere.
        struct Floor;
        impl BlockView for Floor {
            fn block_at(&self, _x: i32, y: i32, _z: i32) -> BlockId {
                if y == 77 {
                    BlockId(1)
                } else {
                    BlockId::AIR
                }
            }
        }
        let registry = registry();
        let direction = Vec3::new(0.3, -0.8, 0.5);
        let here = raycast_solid(&Floor, &registry, DVec3::new(0.4, 80.5, 0.6), direction, 20.0)
            .expect("the ray at spawn missed the floor");
        let far = 3_000_000;
        let there = raycast_solid(
            &Floor,
            &registry,
            DVec3::new(0.4 + far as f64, 80.5, 0.6 + far as f64),
            direction,
            20.0,
        )
        .expect("the ray far out missed the floor");
        assert_eq!(there.block, BlockPos::new(here.block.x + far, here.block.y, here.block.z + far));
        assert_eq!(there.face, here.face);
        assert_eq!(there.distance, here.distance, "the distance drifted with the origin");
    }
}
