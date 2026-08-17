//! Integer boxes over block positions.

use vx_core::{BlockPos, CHUNK_HEIGHT};

/// An inclusive axis-aligned box of block positions.
///
/// Inclusive because voxels are cells, not points: a single block is a box with
/// `min == max`, and a half-open box would make that awkward to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelAabb {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl VoxelAabb {
    /// A box spanning the two corners, in any order.
    pub fn new(a: BlockPos, b: BlockPos) -> Self {
        VoxelAabb {
            min: BlockPos::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: BlockPos::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    /// The box containing exactly one block.
    pub fn single(pos: BlockPos) -> Self {
        VoxelAabb { min: pos, max: pos }
    }

    /// The smallest box containing every position, or `None` if there are none.
    pub fn containing(positions: impl IntoIterator<Item = BlockPos>) -> Option<Self> {
        let mut iter = positions.into_iter();
        let first = iter.next()?;
        Some(iter.fold(VoxelAabb::single(first), |box_, pos| box_.including(pos)))
    }

    pub fn including(self, pos: BlockPos) -> Self {
        VoxelAabb {
            min: BlockPos::new(
                self.min.x.min(pos.x),
                self.min.y.min(pos.y),
                self.min.z.min(pos.z),
            ),
            max: BlockPos::new(
                self.max.x.max(pos.x),
                self.max.y.max(pos.y),
                self.max.z.max(pos.z),
            ),
        }
    }

    /// The smallest box containing both.
    pub fn union(self, other: VoxelAabb) -> Self {
        self.including(other.min).including(other.max)
    }

    pub fn contains(&self, pos: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&pos.x)
            && (self.min.y..=self.max.y).contains(&pos.y)
            && (self.min.z..=self.max.z).contains(&pos.z)
    }

    pub fn intersects(&self, other: &VoxelAabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// Grow by `amount` on every axis.
    pub fn expanded(self, amount: i32) -> Self {
        VoxelAabb {
            min: self.min.offset([-amount, -amount, -amount]),
            max: self.max.offset([amount, amount, amount]),
        }
    }

    /// Clamp the vertical extent to the world, so a box near the ceiling or the
    /// bedrock does not make callers iterate positions that cannot exist.
    pub fn clamped_to_world(self) -> Self {
        VoxelAabb {
            min: BlockPos::new(self.min.x, self.min.y.max(0), self.min.z),
            max: BlockPos::new(self.max.x, self.max.y.min(CHUNK_HEIGHT - 1), self.max.z),
        }
    }

    pub fn size(&self) -> [i64; 3] {
        [
            (self.max.x - self.min.x) as i64 + 1,
            (self.max.y - self.min.y) as i64 + 1,
            (self.max.z - self.min.z) as i64 + 1,
        ]
    }

    pub fn volume(&self) -> u64 {
        let [x, y, z] = self.size();
        if x <= 0 || y <= 0 || z <= 0 {
            return 0;
        }
        (x * y * z) as u64
    }

    /// The centre, rounded down. Used for aiming excavations at a body.
    pub fn centre(&self) -> BlockPos {
        BlockPos::new(
            self.min.x + (self.max.x - self.min.x) / 2,
            self.min.y + (self.max.y - self.min.y) / 2,
            self.min.z + (self.max.z - self.min.z) / 2,
        )
    }

    /// Every block position inside, in y-major order.
    pub fn blocks(&self) -> impl Iterator<Item = BlockPos> + '_ {
        let (min, max) = (self.min, self.max);
        (min.y..=max.y).flat_map(move |y| {
            (min.z..=max.z).flat_map(move |z| (min.x..=max.x).map(move |x| BlockPos::new(x, y, z)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_may_be_given_in_any_order() {
        let a = VoxelAabb::new(BlockPos::new(5, 9, -1), BlockPos::new(1, 2, 4));
        let b = VoxelAabb::new(BlockPos::new(1, 2, 4), BlockPos::new(5, 9, -1));
        assert_eq!(a, b);
        assert_eq!(a.min, BlockPos::new(1, 2, -1));
        assert_eq!(a.max, BlockPos::new(5, 9, 4));
    }

    #[test]
    fn a_single_block_box_has_volume_one() {
        let box_ = VoxelAabb::single(BlockPos::new(3, 4, 5));
        assert_eq!(box_.volume(), 1);
        assert_eq!(box_.blocks().count(), 1);
        assert!(box_.contains(BlockPos::new(3, 4, 5)));
    }

    #[test]
    fn volume_matches_the_number_of_blocks_iterated() {
        // The two must never disagree: volume is used for cost estimates and
        // `blocks` for the actual work.
        let box_ = VoxelAabb::new(BlockPos::new(-2, 3, 7), BlockPos::new(1, 5, 9));
        assert_eq!(box_.volume(), 4 * 3 * 3);
        assert_eq!(box_.blocks().count() as u64, box_.volume());
    }

    #[test]
    fn every_iterated_block_is_inside_and_distinct() {
        let box_ = VoxelAabb::new(BlockPos::new(0, 0, 0), BlockPos::new(2, 2, 2));
        let blocks: Vec<BlockPos> = box_.blocks().collect();
        let unique: std::collections::HashSet<BlockPos> = blocks.iter().copied().collect();
        assert_eq!(unique.len(), blocks.len(), "a block was yielded twice");
        assert!(blocks.iter().all(|pos| box_.contains(*pos)));
    }

    #[test]
    fn contains_rejects_positions_just_outside() {
        let box_ = VoxelAabb::new(BlockPos::new(0, 0, 0), BlockPos::new(2, 2, 2));
        for offset in [[-1, 0, 0], [0, -1, 0], [0, 0, -1], [3, 0, 0], [0, 3, 0], [0, 0, 3]] {
            assert!(!box_.contains(BlockPos::new(0, 0, 0).offset(offset)));
        }
    }

    #[test]
    fn boxes_sharing_only_a_face_still_intersect() {
        // Inclusive bounds mean touching boxes overlap on their shared layer,
        // which is what callers checking for conflicting jobs expect.
        let a = VoxelAabb::new(BlockPos::new(0, 0, 0), BlockPos::new(2, 2, 2));
        let b = VoxelAabb::new(BlockPos::new(2, 0, 0), BlockPos::new(4, 2, 2));
        let apart = VoxelAabb::new(BlockPos::new(3, 0, 0), BlockPos::new(4, 2, 2));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&apart));
    }

    #[test]
    fn containing_wraps_every_position_given() {
        let positions = [
            BlockPos::new(4, 1, 9),
            BlockPos::new(-3, 8, 2),
            BlockPos::new(0, 0, 0),
        ];
        let box_ = VoxelAabb::containing(positions).unwrap();
        assert_eq!(box_.min, BlockPos::new(-3, 0, 0));
        assert_eq!(box_.max, BlockPos::new(4, 8, 9));
        assert!(positions.iter().all(|pos| box_.contains(*pos)));
        assert!(VoxelAabb::containing([]).is_none());
    }

    #[test]
    fn expanding_grows_on_every_axis() {
        let box_ = VoxelAabb::single(BlockPos::new(0, 40, 0)).expanded(2);
        assert_eq!(box_.size(), [5, 5, 5]);
    }

    #[test]
    fn clamping_keeps_a_box_inside_the_world_height() {
        let box_ = VoxelAabb::new(BlockPos::new(0, -20, 0), BlockPos::new(1, 900, 1))
            .clamped_to_world();
        assert_eq!(box_.min.y, 0);
        assert_eq!(box_.max.y, CHUNK_HEIGHT - 1);
        // Horizontal extent is untouched: the world is unbounded sideways.
        assert_eq!((box_.min.x, box_.max.x), (0, 1));
    }

    #[test]
    fn the_centre_of_an_even_span_rounds_toward_the_minimum() {
        let box_ = VoxelAabb::new(BlockPos::new(0, 0, 0), BlockPos::new(3, 3, 3));
        assert_eq!(box_.centre(), BlockPos::new(1, 1, 1));
    }
}
