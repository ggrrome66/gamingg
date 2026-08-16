//! Coordinate spaces and conversions between them.
//!
//! Three spaces exist, and mixing them up is the most common source of voxel
//! bugs, so they are distinct types rather than bare integer triples:
//!
//! - [`BlockPos`] — absolute world coordinates, signed and unbounded in x/z.
//! - [`ChunkPos`] — which chunk column, on a 16×16 grid.
//! - [`LocalPos`] — offset inside one chunk, always in range.
//!
//! Conversions use Euclidean division throughout. Truncating division is wrong
//! for negative coordinates (it rounds toward zero, so x = -1 and x = 0 would
//! land in the same chunk) and that bug is invisible until you walk west of the
//! origin.

/// Chunk width and depth in blocks.
pub const CHUNK_SIZE: i32 = 16;
/// Chunk height in blocks. Matches the 1.12-era world height we use as a
/// reference point.
pub const CHUNK_HEIGHT: i32 = 256;

/// Blocks in one chunk column.
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_HEIGHT) as usize;

/// Absolute position of a block in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Position of a chunk column on the world grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

/// Position within a chunk. Construction is checked, so holding one of these
/// is proof the coordinates are in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalPos {
    x: u8,
    y: u16,
    z: u8,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        BlockPos { x, y, z }
    }

    /// Which chunk column contains this block.
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x.div_euclid(CHUNK_SIZE),
            z: self.z.div_euclid(CHUNK_SIZE),
        }
    }

    /// Offset within the containing chunk, or `None` if `y` is outside the
    /// world's vertical bounds.
    pub fn local(self) -> Option<LocalPos> {
        LocalPos::new(
            self.x.rem_euclid(CHUNK_SIZE),
            self.y,
            self.z.rem_euclid(CHUNK_SIZE),
        )
    }

    /// True when `y` lies inside the buildable world.
    pub const fn in_vertical_bounds(self) -> bool {
        self.y >= 0 && self.y < CHUNK_HEIGHT
    }

    /// This position shifted by a face's unit offset.
    pub const fn offset(self, delta: [i32; 3]) -> Self {
        BlockPos {
            x: self.x + delta[0],
            y: self.y + delta[1],
            z: self.z + delta[2],
        }
    }

    pub const fn neighbour(self, face: crate::face::Face) -> Self {
        self.offset(face.offset())
    }
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        ChunkPos { x, z }
    }

    /// World position of this chunk's (0, 0, 0) corner.
    ///
    /// Saturating, because chunk coordinates arrive from disk: a region file
    /// naming a chunk near `i32::MAX` would overflow this multiply, which is a
    /// panic in debug and silently wrapped terrain in release. Nothing that
    /// far out is reachable in play — it is 2 billion blocks from spawn — so
    /// clamping there is strictly better than wrapping.
    pub const fn origin(self) -> BlockPos {
        BlockPos::new(
            self.x.saturating_mul(CHUNK_SIZE),
            0,
            self.z.saturating_mul(CHUNK_SIZE),
        )
    }

    /// Absolute position of a local offset within this chunk.
    pub const fn block(self, local: LocalPos) -> BlockPos {
        BlockPos::new(
            self.x.saturating_mul(CHUNK_SIZE).saturating_add(local.x as i32),
            local.y as i32,
            self.z.saturating_mul(CHUNK_SIZE).saturating_add(local.z as i32),
        )
    }

    /// Squared distance in chunks, for render-distance checks without a sqrt.
    pub const fn distance_squared(self, other: ChunkPos) -> i64 {
        let dx = (self.x - other.x) as i64;
        let dz = (self.z - other.z) as i64;
        dx * dx + dz * dz
    }
}

impl LocalPos {
    /// Build a local position, returning `None` if any component is out of
    /// range for a chunk.
    pub fn new(x: i32, y: i32, z: i32) -> Option<Self> {
        if (0..CHUNK_SIZE).contains(&x) && (0..CHUNK_HEIGHT).contains(&y) && (0..CHUNK_SIZE).contains(&z) {
            Some(LocalPos {
                x: x as u8,
                y: y as u16,
                z: z as u8,
            })
        } else {
            None
        }
    }

    pub const fn x(self) -> i32 {
        self.x as i32
    }

    pub const fn y(self) -> i32 {
        self.y as i32
    }

    pub const fn z(self) -> i32 {
        self.z as i32
    }

    /// Flat index into chunk storage.
    ///
    /// Y is the slowest-varying axis so that a horizontal slice is contiguous,
    /// which is the access pattern the mesher uses.
    pub const fn index(self) -> usize {
        (self.y as usize * (CHUNK_SIZE * CHUNK_SIZE) as usize)
            + (self.z as usize * CHUNK_SIZE as usize)
            + self.x as usize
    }

    /// Inverse of [`LocalPos::index`].
    pub fn from_index(index: usize) -> Option<Self> {
        if index >= CHUNK_VOLUME {
            return None;
        }
        let size = CHUNK_SIZE as usize;
        let layer = size * size;
        Some(LocalPos {
            x: (index % size) as u8,
            y: (index / layer) as u16,
            z: ((index % layer) / size) as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Face;

    #[test]
    fn positive_coordinates_map_to_the_expected_chunk() {
        assert_eq!(BlockPos::new(0, 0, 0).chunk(), ChunkPos::new(0, 0));
        assert_eq!(BlockPos::new(15, 0, 15).chunk(), ChunkPos::new(0, 0));
        assert_eq!(BlockPos::new(16, 0, 16).chunk(), ChunkPos::new(1, 1));
        assert_eq!(BlockPos::new(33, 0, 47).chunk(), ChunkPos::new(2, 2));
    }

    #[test]
    fn negative_coordinates_floor_instead_of_truncating() {
        // The bug this guards: with truncating division, -1 would land in
        // chunk 0 alongside +0, and the world would fold onto itself at the
        // origin.
        assert_eq!(BlockPos::new(-1, 0, -1).chunk(), ChunkPos::new(-1, -1));
        assert_eq!(BlockPos::new(-16, 0, -16).chunk(), ChunkPos::new(-1, -1));
        assert_eq!(BlockPos::new(-17, 0, -17).chunk(), ChunkPos::new(-2, -2));
        assert_ne!(
            BlockPos::new(-1, 0, 0).chunk(),
            BlockPos::new(0, 0, 0).chunk()
        );
    }

    #[test]
    fn local_offsets_are_always_non_negative() {
        for x in -40..40 {
            for z in -40..40 {
                let local = BlockPos::new(x, 0, z).local().unwrap();
                assert!((0..CHUNK_SIZE).contains(&local.x()));
                assert!((0..CHUNK_SIZE).contains(&local.z()));
            }
        }
    }

    #[test]
    fn chunk_and_local_round_trip_to_the_original_position() {
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(15, 128, 15),
            BlockPos::new(-1, 64, -1),
            BlockPos::new(-33, 255, 100),
            BlockPos::new(1000, 3, -1000),
        ] {
            let restored = pos.chunk().block(pos.local().unwrap());
            assert_eq!(restored, pos, "round trip failed for {pos:?}");
        }
    }

    #[test]
    fn out_of_range_local_positions_are_rejected() {
        assert!(LocalPos::new(0, -1, 0).is_none());
        assert!(LocalPos::new(0, CHUNK_HEIGHT, 0).is_none());
        assert!(LocalPos::new(CHUNK_SIZE, 0, 0).is_none());
        assert!(LocalPos::new(0, 0, -1).is_none());
        assert!(LocalPos::new(15, 255, 15).is_some());

        // A y outside the world has no local position at all.
        assert!(BlockPos::new(0, -1, 0).local().is_none());
        assert!(BlockPos::new(0, CHUNK_HEIGHT, 0).local().is_none());
    }

    #[test]
    fn index_is_a_bijection_over_the_chunk_volume() {
        let mut seen = vec![false; CHUNK_VOLUME];
        for y in 0..CHUNK_HEIGHT {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let local = LocalPos::new(x, y, z).unwrap();
                    let index = local.index();
                    assert!(index < CHUNK_VOLUME);
                    assert!(!seen[index], "index {index} produced twice");
                    seen[index] = true;
                    assert_eq!(LocalPos::from_index(index), Some(local));
                }
            }
        }
        assert!(seen.into_iter().all(|hit| hit));
    }

    #[test]
    fn from_index_rejects_out_of_range_indices() {
        assert!(LocalPos::from_index(CHUNK_VOLUME).is_none());
        assert!(LocalPos::from_index(CHUNK_VOLUME - 1).is_some());
    }

    #[test]
    fn neighbour_moves_one_block_along_the_faces_axis() {
        let pos = BlockPos::new(5, 10, -3);
        assert_eq!(pos.neighbour(Face::PosX), BlockPos::new(6, 10, -3));
        assert_eq!(pos.neighbour(Face::NegX), BlockPos::new(4, 10, -3));
        assert_eq!(pos.neighbour(Face::PosY), BlockPos::new(5, 11, -3));
        assert_eq!(pos.neighbour(Face::NegZ), BlockPos::new(5, 10, -4));
    }

    #[test]
    fn chunk_origin_is_the_corner_block() {
        assert_eq!(ChunkPos::new(0, 0).origin(), BlockPos::new(0, 0, 0));
        assert_eq!(ChunkPos::new(2, 3).origin(), BlockPos::new(32, 0, 48));
        assert_eq!(ChunkPos::new(-1, -1).origin(), BlockPos::new(-16, 0, -16));
    }

    #[test]
    fn extreme_chunk_coordinates_saturate_rather_than_overflowing() {
        // Chunk coordinates come out of region files, so they are untrusted.
        // Multiplying by the chunk size overflows well before `i32::MAX`,
        // which panics in debug and wraps terrain in release.
        for chunk in [
            ChunkPos::new(i32::MAX, i32::MAX),
            ChunkPos::new(i32::MIN, i32::MIN),
            ChunkPos::new(i32::MAX, i32::MIN),
            ChunkPos::new(i32::MAX / 8, i32::MIN / 8),
        ] {
            // Reaching these at all is the test: an overflowing multiply
            // panics here in debug rather than returning anything.
            let origin = chunk.origin();
            let corner = chunk.block(LocalPos::new(15, 0, 15).unwrap());

            // Clamped to the extreme, and the corner stays anchored to it.
            assert!(origin.x.saturating_sub(corner.x).abs() <= CHUNK_SIZE);
            assert!(origin.z.saturating_sub(corner.z).abs() <= CHUNK_SIZE);
        }

        // Ordinary coordinates are unaffected by the saturation.
        assert_eq!(ChunkPos::new(2, 3).origin(), BlockPos::new(32, 0, 48));
    }

    #[test]
    fn distance_squared_is_symmetric_and_zero_on_the_diagonal() {
        let a = ChunkPos::new(3, -4);
        let b = ChunkPos::new(0, 0);
        assert_eq!(a.distance_squared(b), 25);
        assert_eq!(b.distance_squared(a), 25);
        assert_eq!(a.distance_squared(a), 0);
    }

    #[test]
    fn vertical_bounds_match_the_world_height() {
        assert!(!BlockPos::new(0, -1, 0).in_vertical_bounds());
        assert!(BlockPos::new(0, 0, 0).in_vertical_bounds());
        assert!(BlockPos::new(0, CHUNK_HEIGHT - 1, 0).in_vertical_bounds());
        assert!(!BlockPos::new(0, CHUNK_HEIGHT, 0).in_vertical_bounds());
    }
}
