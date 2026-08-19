//! The seed tree, and the hash every lattice in the engine is built on.
//!
//! # One seed becomes a path
//!
//! Worldgen is a pure function of `(seed, position)`, and everything derived
//! from it — ore deposits, trees, town sites — comes off a jittered lattice
//! keyed on that seed. That works exactly as well for a *tree* of seeds as for
//! one: give each level an index and fold it into its parent, and any location
//! in a universe is addressable by a short list of integers, generated purely,
//! with nothing stored.
//!
//! Today every world is [`SeedPath::root`], which folds to the bare seed it was
//! given — so a path costs nothing and changes nothing until there is somewhere
//! to descend to. That is the whole point of adding it now: the alternative is
//! discovering later that every persisted seed in every save is the wrong
//! shape.
//!
//! # The finaliser
//!
//! [`finalise`] is the splitmix64 finaliser that `ore`, `flora` and `town` each
//! had their own copy of. It is exported rather than reimplemented a fourth
//! time, and the copies now call it — byte for byte the same, which the terrain
//! tests hold to.

/// The splitmix64 finaliser: avalanche a mixed key into a full 64-bit hash.
///
/// Deliberately *not* the whole generator — callers mix their own coordinates
/// in first, with their own multipliers, so that two lattices keyed on the same
/// cell do not agree with each other.
pub fn finalise(mut hash: u64) -> u64 {
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

/// A finalised hash mapped to `0..1`.
///
/// Takes the top 24 bits, which are the best-mixed, and divides — so the result
/// is uniform and has exactly f32's worth of precision behind it.
pub fn unit(hash: u64) -> f32 {
    ((hash >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Full splitmix64, for deriving one seed from another.
fn splitmix(value: u64) -> u64 {
    finalise(value.wrapping_add(0x9e37_79b9_7f4a_7c15))
}

/// A path down the seed tree: root, then an index per level.
///
/// `root → galaxy → star → planet → face` is the intended shape. Nothing
/// descends yet; what exists now is the type that makes descending additive
/// rather than a rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeedPath {
    segments: Vec<u64>,
    folded: u64,
}

impl SeedPath {
    /// The world a bare seed names.
    ///
    /// Folds to exactly the seed given — no hashing at the root — so every
    /// world that already exists generates identically through this type.
    pub fn root(seed: u64) -> Self {
        SeedPath {
            segments: vec![seed],
            folded: seed,
        }
    }

    /// Descend one level.
    ///
    /// `index` is hashed before folding, so sibling 0 and sibling 1 are as far
    /// apart as any two seeds rather than differing in one bit.
    pub fn child(&self, index: u64) -> Self {
        let mut segments = self.segments.clone();
        segments.push(index);
        SeedPath {
            segments,
            folded: splitmix(self.folded ^ splitmix(index)),
        }
    }

    /// The seed this path names — what every lattice is keyed on.
    pub fn seed(&self) -> u64 {
        self.folded
    }

    /// How far down the tree this is. The root is zero.
    pub fn depth(&self) -> usize {
        self.segments.len() - 1
    }

    /// The indices, root first. What a save writes to name a location.
    pub fn segments(&self) -> &[u64] {
        &self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_path_folds_to_the_seed_it_was_given() {
        // Load-bearing: every world already on disk was generated from a bare
        // seed, and routing it through a path must not move one block.
        for seed in [0, 1, 2024, u64::MAX, 0x9e37_79b9_7f4a_7c15] {
            assert_eq!(SeedPath::root(seed).seed(), seed);
        }
        assert_eq!(SeedPath::root(7).depth(), 0);
        assert_eq!(SeedPath::root(7).segments(), &[7]);
    }

    #[test]
    fn siblings_are_far_apart_and_paths_are_reproducible() {
        let root = SeedPath::root(2024);
        let a = root.child(0);
        let b = root.child(1);
        assert_ne!(a.seed(), b.seed());
        // A one-bit difference in the index must not survive as a one-bit
        // difference in the seed, or two neighbouring planets share terrain.
        assert!(
            (a.seed() ^ b.seed()).count_ones() > 16,
            "sibling seeds differ in only {} bits",
            (a.seed() ^ b.seed()).count_ones()
        );
        assert_eq!(a.seed(), root.child(0).seed(), "a path is not reproducible");
        assert_eq!(a.depth(), 1);
        assert_eq!(a.segments(), &[2024, 0]);
    }

    #[test]
    fn depth_changes_the_seed_even_with_the_same_indices() {
        let root = SeedPath::root(5);
        assert_ne!(root.child(3).seed(), root.child(3).child(3).seed());
    }

    #[test]
    fn the_finaliser_avalanches() {
        // One flipped input bit should scramble roughly half the output. The
        // three lattices all lean on this; it is worth pinning once.
        let mut worst = 64;
        for bit in 0..64 {
            let flipped = (finalise(0) ^ finalise(1u64 << bit)).count_ones();
            worst = worst.min(flipped);
        }
        assert!(worst > 12, "one input bit moved only {worst} output bits");
    }

    #[test]
    fn unit_stays_inside_the_range_it_promises() {
        for value in 0..2_000u64 {
            let sample = unit(finalise(value.wrapping_mul(0x9e37_79b9_7f4a_7c15)));
            assert!((0.0..1.0).contains(&sample), "{sample} is outside 0..1");
        }
    }
}
