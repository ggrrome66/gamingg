//! A single chunk column and its access helpers.

use std::collections::BTreeMap;

use vx_core::{BlockId, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE, CHUNK_VOLUME};

use crate::storage::PalettedStorage;

/// One 16×256×16 column of blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pos: ChunkPos,
    blocks: PalettedStorage,
    /// Wounds: the blocks in this chunk that violence has given an interior,
    /// by local index. Sparse by construction — a chunk nobody has shot at
    /// carries an empty map and costs nothing.
    composites: BTreeMap<u16, crate::micro::Mask>,
    /// Set when the contents change, cleared once a mesh has been rebuilt.
    dirty: bool,
    /// Set when the chunk no longer matches what generation would produce, and
    /// cleared once it has been written to disk.
    ///
    /// Distinct from `dirty`, which the mesher clears every time it rebuilds.
    /// Saving needs to know "has a player touched this?", which survives any
    /// number of remeshes.
    modified: bool,
}

impl Chunk {
    /// An air-filled chunk at `pos`.
    pub fn empty(pos: ChunkPos) -> Self {
        Chunk {
            pos,
            blocks: PalettedStorage::empty_chunk(),
            composites: BTreeMap::new(),
            dirty: false,
            modified: false,
        }
    }

    /// Rebuild a chunk around storage loaded from disk.
    pub fn from_storage(pos: ChunkPos, blocks: PalettedStorage) -> Self {
        Chunk {
            pos,
            blocks,
            composites: BTreeMap::new(),
            dirty: true,
            // Loaded chunks came from disk, so they are already saved.
            modified: false,
        }
    }

    pub fn pos(&self) -> ChunkPos {
        self.pos
    }

    pub fn get(&self, local: LocalPos) -> BlockId {
        self.blocks.get(local.index())
    }

    /// This block's wound, if it has one.
    pub fn mask(&self, local: LocalPos) -> Option<crate::micro::Mask> {
        self.composites.get(&(local.index() as u16)).copied()
    }

    /// Every wound in this chunk, for saving and for the mesher.
    pub fn composites(&self) -> impl Iterator<Item = (u16, crate::micro::Mask)> + '_ {
        self.composites.iter().map(|(index, mask)| (*index, *mask))
    }

    /// How many wounds this chunk carries.
    pub fn composite_count(&self) -> usize {
        self.composites.len()
    }

    /// Give a block a wound, or take one away.
    ///
    /// An intact mask removes the entry rather than storing sixty-four set
    /// bits: "no wound" has exactly one representation, which is what keeps
    /// the save small and the mesher's fast path honest.
    pub fn set_mask(&mut self, local: LocalPos, mask: crate::micro::Mask) {
        let index = local.index() as u16;
        let changed = if mask == crate::micro::FULL {
            self.composites.remove(&index).is_some()
        } else {
            self.composites.insert(index, mask) != Some(mask)
        };
        if changed {
            self.dirty = true;
            self.modified = true;
        }
    }

    /// Restore the wounds a save file was holding.
    pub fn load_composites(&mut self, wounds: impl IntoIterator<Item = (u16, crate::micro::Mask)>) {
        self.composites = wounds.into_iter().collect();
    }

    /// Write a block, returning the block that was there.
    pub fn set(&mut self, local: LocalPos, block: BlockId) -> BlockId {
        let index = local.index();
        let previous = self.blocks.get(index);
        if previous != block {
            self.blocks.set(index, block);
            // A block replaced is a block made whole: whatever was chewed
            // out of the old one has nothing to do with the new one, and a
            // stale mask here would haunt the block that replaced it.
            self.composites.remove(&(index as u16));
            self.dirty = true;
            self.modified = true;
        }
        previous
    }

    /// True when nothing in this chunk is worth meshing.
    pub fn is_empty(&self) -> bool {
        self.blocks.uniform_block().is_some_and(BlockId::is_air)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// True when this chunk differs from freshly generated terrain and so
    /// needs writing to disk.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn mark_modified(&mut self) {
        self.modified = true;
    }

    /// Called once the chunk has been written out.
    pub fn clear_modified(&mut self) {
        self.modified = false;
    }

    pub fn storage(&self) -> &PalettedStorage {
        &self.blocks
    }

    /// Compact the palette after bulk edits such as generation.
    pub fn optimise(&mut self) {
        self.blocks.optimise();
    }

    /// Highest non-air block in a column, or `None` if the column is empty.
    /// Used for spawn placement and, later, lighting.
    pub fn height_at(&self, x: i32, z: i32) -> Option<i32> {
        (0..CHUNK_HEIGHT).rev().find(|&y| {
            LocalPos::new(x, y, z).is_some_and(|local| !self.get(local).is_air())
        })
    }

    /// Fill a vertical run `[from_y, to_y)` in one column.
    pub fn fill_column(&mut self, x: i32, z: i32, from_y: i32, to_y: i32, block: BlockId) {
        for y in from_y.max(0)..to_y.min(CHUNK_HEIGHT) {
            if let Some(local) = LocalPos::new(x, y, z) {
                self.set(local, block);
            }
        }
    }

    /// Iterate every position paired with its block.
    pub fn iter_blocks(&self) -> impl Iterator<Item = (LocalPos, BlockId)> + '_ {
        (0..CHUNK_VOLUME).filter_map(move |index| {
            LocalPos::from_index(index).map(|local| (local, self.blocks.get(index)))
        })
    }

    /// Blocks that are not air. Cheap enough for diagnostics, not a hot path.
    pub fn non_air_count(&self) -> usize {
        self.blocks.iter().filter(|block| !block.is_air()).count()
    }
}

/// Read-only block access spanning chunk boundaries.
///
/// The mesher needs to look one block past a chunk's edge to decide whether a
/// face is hidden by its neighbour. Implementors decide what happens off the
/// edge of loaded data.
///
/// Coordinates are **absolute world coordinates**, never chunk-local. Every
/// implementor must agree on that, or geometry silently lands in the wrong
/// place — or vanishes.
pub trait BlockView {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId;

    /// The damage mask of a *composite* block, if this one has been wounded.
    ///
    /// Defaulted to `None` — an intact world — so every existing view keeps
    /// working untouched and only the ones that actually hold wounds
    /// override it. That default is the whole reason micro-on-damage reaches
    /// the raycast, the mesher and every sight query without any of them
    /// changing shape.
    fn mask_at(&self, _x: i32, _y: i32, _z: i32) -> Option<crate::micro::Mask> {
        None
    }
}

/// A chunk meshed in isolation, treating everything outside it as air.
///
/// Takes world coordinates like any other [`BlockView`] and maps them through
/// the chunk's own origin.
///
/// Correct only for chunks with no loaded neighbours; otherwise it draws faces
/// along the seams that the neighbouring chunk actually hides.
pub struct SoloChunkView<'a>(pub &'a Chunk);

impl BlockView for SoloChunkView<'_> {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        let origin = self.0.pos().origin();
        let local_x = x - origin.x;
        let local_z = z - origin.z;
        if !(0..CHUNK_SIZE).contains(&local_x) || !(0..CHUNK_SIZE).contains(&local_z) {
            return BlockId::AIR;
        }
        LocalPos::new(local_x, y, local_z).map_or(BlockId::AIR, |local| self.0.get(local))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: BlockId = BlockId(1);
    const DIRT: BlockId = BlockId(2);

    fn local(x: i32, y: i32, z: i32) -> LocalPos {
        LocalPos::new(x, y, z).expect("test coordinates are in range")
    }

    #[test]
    fn a_new_chunk_is_empty_air_and_clean() {
        let chunk = Chunk::empty(ChunkPos::new(3, -2));
        assert_eq!(chunk.pos(), ChunkPos::new(3, -2));
        assert!(chunk.is_empty());
        assert!(!chunk.is_dirty());
        assert_eq!(chunk.non_air_count(), 0);
        assert_eq!(chunk.height_at(0, 0), None);
    }

    #[test]
    fn setting_a_block_reports_the_previous_one_and_dirties_the_chunk() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));

        let previous = chunk.set(local(1, 2, 3), STONE);
        assert_eq!(previous, BlockId::AIR);
        assert_eq!(chunk.get(local(1, 2, 3)), STONE);
        assert!(chunk.is_dirty());
        assert!(!chunk.is_empty());

        let previous = chunk.set(local(1, 2, 3), DIRT);
        assert_eq!(previous, STONE);
    }

    #[test]
    fn writing_an_identical_block_does_not_dirty_the_chunk() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(0, 0, 0), STONE);
        chunk.clear_dirty();

        chunk.set(local(0, 0, 0), STONE);
        assert!(!chunk.is_dirty(), "a redundant write should not force a remesh");

        chunk.set(local(0, 0, 0), DIRT);
        assert!(chunk.is_dirty());
    }

    #[test]
    fn height_finds_the_topmost_solid_block() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.fill_column(4, 5, 0, 64, STONE);
        assert_eq!(chunk.height_at(4, 5), Some(63));

        chunk.set(local(4, 200, 5), DIRT);
        assert_eq!(chunk.height_at(4, 5), Some(200));

        // A neighbouring column is unaffected.
        assert_eq!(chunk.height_at(5, 5), None);
    }

    #[test]
    fn fill_column_clamps_to_the_world_bounds() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.fill_column(0, 0, -10, CHUNK_HEIGHT + 10, STONE);

        assert_eq!(chunk.get(local(0, 0, 0)), STONE);
        assert_eq!(chunk.get(local(0, CHUNK_HEIGHT - 1, 0)), STONE);
        assert_eq!(chunk.height_at(0, 0), Some(CHUNK_HEIGHT - 1));
        assert_eq!(chunk.non_air_count(), CHUNK_HEIGHT as usize);
    }

    #[test]
    fn iter_blocks_visits_the_whole_volume_once() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(2, 3, 4), STONE);

        let visited: Vec<_> = chunk.iter_blocks().collect();
        assert_eq!(visited.len(), CHUNK_VOLUME);

        let stone: Vec<_> = visited
            .iter()
            .filter(|(_, block)| *block == STONE)
            .map(|(pos, _)| *pos)
            .collect();
        assert_eq!(stone, vec![local(2, 3, 4)]);
    }

    #[test]
    fn a_solo_view_maps_world_coordinates_through_the_chunk_origin() {
        // A chunk away from the origin must still be readable at its real
        // world coordinates, or its geometry is meshed as empty.
        let pos = ChunkPos::new(2, -3);
        let mut chunk = Chunk::empty(pos);
        chunk.set(local(0, 40, 0), STONE);
        let view = SoloChunkView(&chunk);

        let origin = pos.origin();
        assert_eq!(origin.x, 32);
        assert_eq!(origin.z, -48);
        assert_eq!(view.block_at(origin.x, 40, origin.z), STONE);
        // Local coordinates are *not* world coordinates for this chunk.
        assert_eq!(view.block_at(0, 40, 0), BlockId::AIR);
        // One block outside the chunk's own span.
        assert_eq!(view.block_at(origin.x - 1, 40, origin.z), BlockId::AIR);
        assert_eq!(view.block_at(origin.x + CHUNK_SIZE, 40, origin.z), BlockId::AIR);
    }

    #[test]
    fn a_solo_view_treats_everything_outside_the_chunk_as_air() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(0, 0, 0), STONE);
        let view = SoloChunkView(&chunk);

        assert_eq!(view.block_at(0, 0, 0), STONE);
        // Off every edge, including vertically.
        assert_eq!(view.block_at(-1, 0, 0), BlockId::AIR);
        assert_eq!(view.block_at(CHUNK_SIZE, 0, 0), BlockId::AIR);
        assert_eq!(view.block_at(0, 0, -1), BlockId::AIR);
        assert_eq!(view.block_at(0, -1, 0), BlockId::AIR);
        assert_eq!(view.block_at(0, CHUNK_HEIGHT, 0), BlockId::AIR);
    }
}
