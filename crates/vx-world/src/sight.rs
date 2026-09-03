//! Line of sight: can one point in the world see another?
//!
//! A thin, deliberate layer over [`crate::raycast`]. The question "is there
//! terrain between these two points" is pure world geometry, so it lives here
//! beside the raycast rather than in whichever crate happens to want it — the
//! villagers want it today, and hostiles will want exactly the same answer
//! later without depending on the app.
//!
//! The subtlety is the wall a target is *standing against*. Casting at
//! somebody pressed up to a pillar hits the pillar a hair before it reaches
//! them, so a naive "did the ray hit anything" test says they are invisible
//! while they are in plain view. [`SIGHT_SLACK`] is the tolerance that fixes
//! it: an obstruction only counts when it is meaningfully nearer than the
//! target.

use glam::DVec3;
use vx_core::BlockRegistry;

use crate::chunk::BlockView;
use crate::raycast::{raycast_solid, RayHit};

/// How much nearer than the target a block must be to count as blocking, in
/// blocks. Covers the wall a target is leaning against.
pub const SIGHT_SLACK: f32 = 0.1;

/// The first solid block on the segment `from` → `to`, if one is in the way.
///
/// Returns `None` when the line is clear, when the two points coincide, or
/// when the only thing hit is at or past the target itself. An origin inside
/// a solid block *is* an obstruction — you cannot see out of solid rock —
/// which is the distance-zero case the raycast reports.
pub fn obstruction(
    view: &impl BlockView,
    registry: &BlockRegistry,
    from: DVec3,
    to: DVec3,
) -> Option<RayHit> {
    // The difference is taken in `f64` and only then narrowed: two eyes far
    // from the origin are close to each other, and that is the number that
    // has to be exact.
    let along = (to - from).as_vec3();
    let reach = along.length();
    if reach < f32::EPSILON {
        // Standing in the same spot: nothing can be between you.
        return None;
    }
    raycast_solid(view, registry, from, along, reach)
        .filter(|hit| hit.distance < reach - SIGHT_SLACK)
}

/// Is `to` visible from `from`: within `range`, with nothing solid between?
pub fn sees(
    view: &impl BlockView,
    registry: &BlockRegistry,
    from: DVec3,
    to: DVec3,
    range: f32,
) -> bool {
    if (to - from).length() > range as f64 {
        return false;
    }
    obstruction(view, registry, from, to).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockDef, BlockId, ChunkPos, LocalPos};

    use crate::chunk::{Chunk, SoloChunkView};

    const STONE: BlockId = BlockId(1);

    fn registry() -> BlockRegistry {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDef::uniform("test:stone", 0)).unwrap();
        registry
    }

    /// A chunk holding a one-block wall at `(x, y, z)`.
    fn wall_at(x: i32, y: i32, z: i32) -> Chunk {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(LocalPos::new(x, y, z).unwrap(), STONE);
        chunk
    }

    fn visible(chunk: &Chunk, from: DVec3, to: DVec3, range: f32) -> bool {
        sees(&SoloChunkView(chunk), &registry(), from, to, range)
    }

    #[test]
    fn clear_air_is_always_visible() {
        let empty = Chunk::empty(ChunkPos::new(0, 0));
        assert!(visible(
            &empty,
            DVec3::new(2.5, 40.5, 2.5),
            DVec3::new(12.5, 40.5, 12.5),
            40.0
        ));
    }

    #[test]
    fn a_wall_between_two_points_blocks_sight() {
        let chunk = wall_at(8, 40, 8);
        assert!(!visible(
            &chunk,
            DVec3::new(8.5, 40.5, 2.5),
            DVec3::new(8.5, 40.5, 14.5),
            40.0
        ));
    }

    #[test]
    fn a_target_standing_against_a_wall_is_still_visible() {
        // The classic false negative: the ray clips the block the target is
        // pressed up against a hair before reaching them.
        let chunk = wall_at(8, 40, 8);
        let watcher = DVec3::new(8.5, 40.5, 2.5);
        let leaning = DVec3::new(8.5, 40.5, 7.95);
        assert!(
            visible(&chunk, watcher, leaning, 40.0),
            "somebody leaning on a wall went invisible"
        );
    }

    #[test]
    fn an_observer_inside_a_solid_sees_nothing() {
        // Distance-zero hits are obstructions: you cannot see out of rock.
        let chunk = wall_at(8, 40, 8);
        assert!(!visible(
            &chunk,
            DVec3::new(8.5, 40.5, 8.5),
            DVec3::new(8.5, 40.5, 14.5),
            40.0
        ));
    }

    #[test]
    fn sight_stops_at_the_range_limit() {
        let empty = Chunk::empty(ChunkPos::new(0, 0));
        let watcher = DVec3::new(2.5, 40.5, 2.5);
        let far = DVec3::new(2.5, 40.5, 22.5);
        assert!(visible(&empty, watcher, far, 25.0));
        assert!(!visible(&empty, watcher, far, 10.0), "range was not honoured");
    }

    #[test]
    fn a_zero_length_line_is_clear_rather_than_looping() {
        let chunk = wall_at(8, 40, 8);
        let here = DVec3::new(4.5, 40.5, 4.5);
        assert!(obstruction(&SoloChunkView(&chunk), &registry(), here, here).is_none());
        assert!(visible(&chunk, here, here, 1.0));
    }

    #[test]
    fn sight_is_symmetric() {
        // If I can see you, you can see me — an asymmetry here would make
        // one-sided stares possible and is exactly the sort of thing that
        // shows up as a bug much later.
        let chunk = wall_at(8, 40, 8);
        let a = DVec3::new(8.5, 40.5, 2.5);
        let b = DVec3::new(8.5, 40.5, 14.5);
        assert_eq!(visible(&chunk, a, b, 40.0), visible(&chunk, b, a, 40.0));

        let open = DVec3::new(3.5, 40.5, 14.5);
        assert_eq!(visible(&chunk, a, open, 40.0), visible(&chunk, open, a, 40.0));
    }
}
