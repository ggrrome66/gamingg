//! Palette-compressed voxel storage.
//!
//! A chunk holds 65 536 blocks. Storing a `BlockId` per block costs 128 KiB,
//! and the overwhelming majority of chunks use only a handful of distinct
//! blocks — an all-air chunk uses exactly one. So instead of block ids we
//! store small indices into a per-chunk palette, packed at the narrowest bit
//! width that fits the palette.
//!
//! The degenerate single-entry case is special-cased to zero bits and an empty
//! backing buffer, which makes an untouched air chunk essentially free.
//!
//! Indices never straddle a `u64` boundary. That wastes a few bits at widths
//! that do not divide 64, and buys much simpler, branch-free accessors.

use vx_core::{BlockId, CHUNK_VOLUME};

/// Widest index we will pack. Beyond this the palette is bigger than the
/// direct representation would be, so packing stops paying for itself.
const MAX_BITS: u32 = 16;

/// Most palette entries a storage may hold, implied by [`MAX_BITS`].
pub const MAX_PALETTE: usize = 1 << MAX_BITS;

/// Why packed storage could not be rebuilt from its serialised parts.
///
/// Every one of these is reachable from a corrupt or hostile save file, so
/// they are values to report rather than assertions to trip. In particular
/// [`StorageError::IndexOutOfPalette`] guards a real memory-safety-adjacent
/// hazard: [`PalettedStorage::get`] indexes the palette directly, so an index
/// past its end is a panic in the middle of meshing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    #[error("palette is empty")]
    EmptyPalette,
    #[error("palette holds {0} entries, more than the {MAX_PALETTE} supported")]
    PaletteTooLarge(usize),
    #[error("bit width {0} exceeds the {MAX_BITS} supported")]
    BitsTooWide(u32),
    #[error("bit width {bits} cannot index a palette of {entries}")]
    BitsTooNarrow { bits: u32, entries: usize },
    #[error("uniform storage needs exactly one palette entry and no data words")]
    MalformedUniform,
    #[error("expected {expected} data words for {len} blocks at {bits} bits, found {actual}")]
    DataLengthMismatch {
        expected: usize,
        actual: usize,
        len: usize,
        bits: u32,
    },
    #[error("block {at} selects palette slot {index}, beyond the {entries} present")]
    IndexOutOfPalette {
        at: usize,
        index: usize,
        entries: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedStorage {
    palette: Vec<BlockId>,
    /// Bits per index. Zero means "uniform": every block is `palette[0]` and
    /// `data` is empty.
    bits: u32,
    data: Vec<u64>,
    len: usize,
}

impl PalettedStorage {
    /// Storage of `len` blocks, entirely `fill`.
    pub fn filled(len: usize, fill: BlockId) -> Self {
        PalettedStorage {
            palette: vec![fill],
            bits: 0,
            data: Vec::new(),
            len,
        }
    }

    /// A chunk-sized volume of air.
    pub fn empty_chunk() -> Self {
        Self::filled(CHUNK_VOLUME, BlockId::AIR)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True while every block is identical, so callers can skip work wholesale
    /// (an all-air chunk needs no mesh at all).
    pub fn is_uniform(&self) -> bool {
        self.bits == 0
    }

    /// The single block filling this storage, if it is uniform.
    pub fn uniform_block(&self) -> Option<BlockId> {
        self.is_uniform().then(|| self.palette[0])
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// The palette itself, for serialisation.
    pub fn palette(&self) -> &[BlockId] {
        &self.palette
    }

    /// The packed index words, for serialisation. Empty when uniform.
    pub fn raw_data(&self) -> &[u64] {
        &self.data
    }

    /// Rebuild storage from serialised parts, checking every invariant the
    /// accessors rely on.
    ///
    /// This is the trust boundary for saved worlds. The parts arrive from a
    /// file that may be corrupt, truncated, or written by something hostile,
    /// so nothing here may be assumed — least of all that packed indices are
    /// inside the palette, which [`PalettedStorage::get`] would otherwise use
    /// to index out of bounds.
    pub fn from_parts(
        palette: Vec<BlockId>,
        bits: u32,
        data: Vec<u64>,
        len: usize,
    ) -> Result<Self, StorageError> {
        if palette.is_empty() {
            return Err(StorageError::EmptyPalette);
        }
        if palette.len() > MAX_PALETTE {
            return Err(StorageError::PaletteTooLarge(palette.len()));
        }
        if bits > MAX_BITS {
            return Err(StorageError::BitsTooWide(bits));
        }

        if bits == 0 {
            // Uniform storage carries one entry and no words; anything else
            // means the width and the payload disagree.
            if palette.len() != 1 || !data.is_empty() {
                return Err(StorageError::MalformedUniform);
            }
            return Ok(PalettedStorage {
                palette,
                bits,
                data,
                len,
            });
        }

        if bits < Self::bits_for(palette.len()) {
            return Err(StorageError::BitsTooNarrow {
                bits,
                entries: palette.len(),
            });
        }

        // Derive the expected word count rather than trusting the one on disk.
        let expected = Self::words_needed(len, bits);
        if data.len() != expected {
            return Err(StorageError::DataLengthMismatch {
                expected,
                actual: data.len(),
                len,
                bits,
            });
        }

        let storage = PalettedStorage {
            palette,
            bits,
            data,
            len,
        };

        // O(len), paid once per chunk load, and the only thing standing
        // between a doctored file and an out-of-bounds palette read.
        for at in 0..storage.len {
            let index = storage.get_index(at);
            if index >= storage.palette.len() {
                return Err(StorageError::IndexOutOfPalette {
                    at,
                    index,
                    entries: storage.palette.len(),
                });
            }
        }

        Ok(storage)
    }

    pub fn bits_per_index(&self) -> u32 {
        self.bits
    }

    /// Bytes held by the packed index buffer. Excludes the palette, which is
    /// negligible.
    pub fn data_bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<u64>()
    }

    #[inline]
    fn indices_per_word(bits: u32) -> usize {
        debug_assert!(bits > 0);
        (u64::BITS / bits) as usize
    }

    #[inline]
    fn words_needed(len: usize, bits: u32) -> usize {
        let per_word = Self::indices_per_word(bits);
        len.div_ceil(per_word)
    }

    /// Narrowest width that can index `entries` palette slots.
    #[inline]
    fn bits_for(entries: usize) -> u32 {
        debug_assert!(entries >= 1);
        if entries <= 1 {
            0
        } else {
            // Number of bits needed to represent `entries - 1`.
            usize::BITS - (entries - 1).leading_zeros()
        }
    }

    #[inline]
    fn get_index(&self, at: usize) -> usize {
        if self.bits == 0 {
            return 0;
        }
        let per_word = Self::indices_per_word(self.bits);
        let word = at / per_word;
        let shift = (at % per_word) as u32 * self.bits;
        let mask = (1u64 << self.bits) - 1;
        ((self.data[word] >> shift) & mask) as usize
    }

    #[inline]
    fn set_index(&mut self, at: usize, index: usize) {
        debug_assert!(self.bits > 0);
        let per_word = Self::indices_per_word(self.bits);
        let word = at / per_word;
        let shift = (at % per_word) as u32 * self.bits;
        let mask = (1u64 << self.bits) - 1;
        let cleared = self.data[word] & !(mask << shift);
        self.data[word] = cleared | ((index as u64 & mask) << shift);
    }

    /// Re-pack every index at `new_bits`, widening the buffer.
    fn repack(&mut self, new_bits: u32) {
        debug_assert!(new_bits > self.bits);
        debug_assert!(new_bits <= MAX_BITS);

        let mut widened = PalettedStorage {
            palette: Vec::new(),
            bits: new_bits,
            data: vec![0; Self::words_needed(self.len, new_bits)],
            len: self.len,
        };
        for at in 0..self.len {
            let index = self.get_index(at);
            widened.set_index(at, index);
        }
        self.data = widened.data;
        self.bits = new_bits;
    }

    /// Palette slot for `block`, adding it (and widening if needed) if absent.
    fn intern(&mut self, block: BlockId) -> usize {
        if let Some(existing) = self.palette.iter().position(|&candidate| candidate == block) {
            return existing;
        }
        self.palette.push(block);
        let needed = Self::bits_for(self.palette.len());
        if needed > self.bits {
            assert!(
                needed <= MAX_BITS,
                "palette overflowed {MAX_BITS}-bit indices; a chunk cannot hold more than {} distinct blocks",
                1usize << MAX_BITS
            );
            self.repack(needed);
        }
        self.palette.len() - 1
    }

    pub fn get(&self, at: usize) -> BlockId {
        assert!(at < self.len, "index {at} out of bounds for {} blocks", self.len);
        self.palette[self.get_index(at)]
    }

    pub fn set(&mut self, at: usize, block: BlockId) {
        assert!(at < self.len, "index {at} out of bounds for {} blocks", self.len);

        // Writing the value a uniform storage already holds changes nothing,
        // which keeps all-air chunks from allocating on redundant writes.
        if self.bits == 0 && self.palette[0] == block {
            return;
        }
        let index = self.intern(block);
        self.set_index(at, index);
    }

    /// Rebuild the palette without entries that no longer appear, narrowing
    /// the packing if that lets us. Worth calling after bulk edits; not worth
    /// calling per block write.
    pub fn optimise(&mut self) {
        if self.bits == 0 {
            return;
        }

        let mut used = vec![false; self.palette.len()];
        for at in 0..self.len {
            used[self.get_index(at)] = true;
        }
        if used.iter().all(|&hit| hit) {
            return;
        }

        // Map old palette slots to new, compacted ones.
        let mut remap = vec![usize::MAX; self.palette.len()];
        let mut compacted = Vec::new();
        for (old, &hit) in used.iter().enumerate() {
            if hit {
                remap[old] = compacted.len();
                compacted.push(self.palette[old]);
            }
        }

        let new_bits = Self::bits_for(compacted.len());
        let mut rebuilt = PalettedStorage {
            palette: compacted,
            bits: new_bits,
            data: if new_bits == 0 {
                Vec::new()
            } else {
                vec![0; Self::words_needed(self.len, new_bits)]
            },
            len: self.len,
        };
        if new_bits > 0 {
            for at in 0..self.len {
                rebuilt.set_index(at, remap[self.get_index(at)]);
            }
        }
        *self = rebuilt;
    }

    pub fn iter(&self) -> impl Iterator<Item = BlockId> + '_ {
        (0..self.len).map(move |at| self.get(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: BlockId = BlockId(1);
    const DIRT: BlockId = BlockId(2);
    const GRASS: BlockId = BlockId(3);

    #[test]
    fn a_fresh_chunk_is_uniform_air_and_allocates_nothing() {
        let storage = PalettedStorage::empty_chunk();
        assert!(storage.is_uniform());
        assert_eq!(storage.uniform_block(), Some(BlockId::AIR));
        assert_eq!(storage.data_bytes(), 0);
        assert_eq!(storage.bits_per_index(), 0);
        assert!(storage.iter().all(|block| block.is_air()));
    }

    #[test]
    fn rewriting_a_uniform_storage_with_its_own_value_stays_free() {
        let mut storage = PalettedStorage::empty_chunk();
        storage.set(100, BlockId::AIR);
        storage.set(200, BlockId::AIR);
        assert!(storage.is_uniform());
        assert_eq!(storage.data_bytes(), 0);
    }

    #[test]
    fn writing_a_second_block_leaves_the_uniform_fast_path() {
        let mut storage = PalettedStorage::empty_chunk();
        storage.set(42, STONE);

        assert!(!storage.is_uniform());
        assert_eq!(storage.get(42), STONE);
        assert_eq!(storage.get(41), BlockId::AIR);
        assert_eq!(storage.get(43), BlockId::AIR);
        assert_eq!(storage.palette_len(), 2);
        assert_eq!(storage.bits_per_index(), 1);
    }

    #[test]
    fn reads_and_writes_round_trip_across_the_whole_volume() {
        let mut storage = PalettedStorage::empty_chunk();
        let blocks = [BlockId::AIR, STONE, DIRT, GRASS];

        for at in 0..storage.len() {
            storage.set(at, blocks[at % blocks.len()]);
        }
        for at in 0..storage.len() {
            assert_eq!(storage.get(at), blocks[at % blocks.len()], "mismatch at {at}");
        }
    }

    #[test]
    fn bit_width_grows_only_when_the_palette_demands_it() {
        // Widths are chosen from palette size: 2 entries need 1 bit, 3-4 need
        // 2, 5-8 need 3, and so on.
        assert_eq!(PalettedStorage::bits_for(1), 0);
        assert_eq!(PalettedStorage::bits_for(2), 1);
        assert_eq!(PalettedStorage::bits_for(3), 2);
        assert_eq!(PalettedStorage::bits_for(4), 2);
        assert_eq!(PalettedStorage::bits_for(5), 3);
        assert_eq!(PalettedStorage::bits_for(8), 3);
        assert_eq!(PalettedStorage::bits_for(9), 4);
        assert_eq!(PalettedStorage::bits_for(256), 8);
        assert_eq!(PalettedStorage::bits_for(257), 9);
    }

    #[test]
    fn widening_the_palette_preserves_every_existing_block() {
        let mut storage = PalettedStorage::filled(512, BlockId::AIR);

        // Fill with a pattern, then keep introducing new blocks to force
        // repeated repacks, checking the earlier data survives each one.
        for at in 0..512 {
            storage.set(at, BlockId((at % 3) as u16));
        }
        let snapshot: Vec<BlockId> = storage.iter().collect();

        for extra in 3..40u16 {
            storage.set(0, BlockId(extra));
            storage.set(0, snapshot[0]);
            for (at, &expected) in snapshot.iter().enumerate() {
                assert_eq!(storage.get(at), expected, "lost data at {at} after adding {extra}");
            }
        }
        assert!(storage.bits_per_index() >= 6);
    }

    #[test]
    fn optimise_drops_unused_palette_entries_and_narrows_packing() {
        let mut storage = PalettedStorage::filled(256, BlockId::AIR);
        for extra in 1..20u16 {
            storage.set(0, BlockId(extra));
        }
        storage.set(0, BlockId::AIR);
        assert!(storage.palette_len() > 2);

        storage.optimise();

        assert_eq!(storage.palette_len(), 1);
        assert!(storage.is_uniform());
        assert_eq!(storage.uniform_block(), Some(BlockId::AIR));
        assert!(storage.iter().all(|block| block.is_air()));
    }

    #[test]
    fn optimise_is_a_no_op_when_every_entry_is_in_use() {
        let mut storage = PalettedStorage::filled(64, BlockId::AIR);
        storage.set(0, STONE);
        storage.set(1, DIRT);
        let before = storage.clone();

        storage.optimise();

        assert_eq!(storage, before);
    }

    #[test]
    fn optimise_preserves_block_data_while_compacting() {
        let mut storage = PalettedStorage::filled(128, BlockId::AIR);
        for at in 0..128 {
            storage.set(at, if at % 2 == 0 { STONE } else { GRASS });
        }
        // Introduce and then remove a block, stranding a palette entry.
        storage.set(5, DIRT);
        storage.set(5, GRASS);

        let before: Vec<BlockId> = storage.iter().collect();
        storage.optimise();

        assert_eq!(storage.iter().collect::<Vec<_>>(), before);
        assert_eq!(storage.palette_len(), 2);
    }

    #[test]
    fn packing_saves_memory_against_a_flat_block_array() {
        let mut storage = PalettedStorage::empty_chunk();
        for at in 0..storage.len() {
            storage.set(at, if at % 2 == 0 { STONE } else { DIRT });
        }

        // Air is still interned from the initial fill even though no block is
        // air any more, so the palette holds 3 entries and needs 2 bits.
        assert_eq!(storage.palette_len(), 3);
        assert_eq!(storage.bits_per_index(), 2);

        // Compacting drops the stranded air entry, and two blocks then need
        // just 1 bit each — against 16 bits for a flat BlockId array.
        storage.optimise();
        assert_eq!(storage.palette_len(), 2);
        assert_eq!(storage.bits_per_index(), 1);
        let flat = CHUNK_VOLUME * std::mem::size_of::<BlockId>();
        assert!(
            storage.data_bytes() < flat / 8,
            "expected big savings, got {} vs {flat}",
            storage.data_bytes()
        );
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn reading_past_the_end_panics() {
        let storage = PalettedStorage::filled(16, BlockId::AIR);
        storage.get(16);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn writing_past_the_end_panics() {
        let mut storage = PalettedStorage::filled(16, BlockId::AIR);
        storage.set(16, STONE);
    }
}
