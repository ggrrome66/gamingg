//! Saving and loading worlds.
//!
//! # Blocks are stored by name, never by id
//!
//! [`BlockId`]s are dense indices assigned in registration order, so installing
//! a mod — or merely reordering the built-ins — shifts every id after the
//! insertion point. A save keyed on those numbers would silently decode into
//! entirely different blocks: your stone house reloads as dirt and glass.
//!
//! So each chunk carries its own palette of *namespaced names*, remapped
//! through the live [`BlockRegistry`] on load. A block whose mod is no longer
//! installed decodes to air rather than to whatever now occupies its number.
//!
//! # Only modified chunks are written
//!
//! Generation is a pure function of `(seed, position)`, so untouched terrain
//! costs nothing to store — it regenerates identically. Only chunks a player
//! has actually changed reach the disk, which keeps saves proportional to what
//! was built rather than to how far someone walked.
//!
//! # Chunks are grouped into regions
//!
//! One file per chunk would mean tens of thousands of tiny files. They are
//! grouped into 32×32 blocks of chunks instead, which matters on the kind of
//! slow storage handheld hardware tends to use.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use vx_core::{BlockId, BlockRegistry, ChunkPos};

use crate::chunk::Chunk;
use crate::storage::{PalettedStorage, StorageError};
use crate::world::World;

/// Chunks along one edge of a region file.
pub const REGION_SIZE: i32 = 32;

const WORLD_MAGIC: &[u8; 4] = b"VXWD";
const REGION_MAGIC: &[u8; 4] = b"VXRG";
const FORMAT_VERSION: u32 = 1;

/// An upper bound on palette entries, so a corrupt length cannot make us
/// allocate wildly before failing.
const MAX_PALETTE: u32 = 65_536;

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a {kind} file")]
    BadMagic { kind: &'static str },
    #[error("unsupported save format version {found} (this build reads {FORMAT_VERSION})")]
    BadVersion { found: u32 },
    #[error("save data ended unexpectedly")]
    Truncated,
    #[error("save data is not valid: {0}")]
    Corrupt(String),
    #[error("stored chunk could not be rebuilt: {0}")]
    Storage(#[from] StorageError),
}

/// Which region file holds a chunk.
pub fn region_of(pos: ChunkPos) -> (i32, i32) {
    (
        pos.x.div_euclid(REGION_SIZE),
        pos.z.div_euclid(REGION_SIZE),
    )
}

/// A chunk as it sits on disk: palette by name, indices still packed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredChunk {
    len: u32,
    bits: u32,
    palette: Vec<String>,
    data: Vec<u64>,
}

impl StoredChunk {
    fn from_chunk(chunk: &Chunk, registry: &BlockRegistry) -> Self {
        let storage = chunk.storage();
        StoredChunk {
            len: storage.len() as u32,
            bits: storage.bits_per_index(),
            palette: storage
                .palette()
                .iter()
                .map(|id| registry.get_or_air(*id).name.clone())
                .collect(),
            data: storage.packed_data().to_vec(),
        }
    }

    /// Rebuild against the live registry, mapping names back to ids.
    ///
    /// Names the registry does not know become air: the mod that defined them
    /// is gone, and guessing an id would corrupt the build.
    fn into_chunk(self, pos: ChunkPos, registry: &BlockRegistry) -> Result<Chunk, SaveError> {
        let palette: Vec<BlockId> = self
            .palette
            .iter()
            .map(|name| {
                registry.id_of(name).unwrap_or_else(|| {
                    log::warn!("save refers to unknown block '{name}'; loading as air");
                    BlockId::AIR
                })
            })
            .collect();

        // Two stored names can collapse onto air, leaving duplicate palette
        // entries. That is harmless — the indices still resolve — but it does
        // mean the palette is no longer minimal, so compact after building.
        let storage =
            PalettedStorage::from_parts(palette, self.bits, self.data, self.len as usize)?;
        let mut chunk = Chunk::from_storage(pos, storage);
        chunk.optimise();
        Ok(chunk)
    }
}

/// A world directory on disk.
pub struct WorldSave {
    root: PathBuf,
}

impl WorldSave {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        WorldSave { root: root.into() }
    }

    /// Create the directory if it does not exist.
    pub fn create(root: impl Into<PathBuf>) -> Result<Self, SaveError> {
        let save = WorldSave::new(root);
        std::fs::create_dir_all(&save.root)?;
        Ok(save)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn meta_path(&self) -> PathBuf {
        self.root.join("level.vx")
    }

    fn region_path(&self, region: (i32, i32)) -> PathBuf {
        self.root.join(format!("r.{}.{}.vxr", region.0, region.1))
    }

    /// True when this directory holds a world.
    pub fn exists(&self) -> bool {
        self.meta_path().is_file()
    }

    pub fn write_meta(&self, seed: u64) -> Result<(), SaveError> {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(WORLD_MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&seed.to_le_bytes());
        write_atomically(&self.meta_path(), &bytes)
    }

    pub fn read_meta(&self) -> Result<u64, SaveError> {
        let bytes = std::fs::read(self.meta_path())?;
        let mut reader = Reader::new(&bytes);
        reader.expect_magic(WORLD_MAGIC, "world")?;
        reader.expect_version()?;
        reader.u64()
    }

    /// Write every chunk that has been modified, clearing their flags.
    ///
    /// Returns how many chunks were written.
    pub fn save_world(&self, world: &mut World) -> Result<usize, SaveError> {
        self.write_meta(world.seed())?;

        let pending: Vec<ChunkPos> = world
            .loaded_chunks()
            .filter(|pos| world.chunk(*pos).is_some_and(Chunk::is_modified))
            .collect();
        if pending.is_empty() {
            return Ok(0);
        }

        // Group by region so each file is rewritten once, not once per chunk.
        let mut by_region: HashMap<(i32, i32), Vec<ChunkPos>> = HashMap::new();
        for pos in pending {
            by_region.entry(region_of(pos)).or_default().push(pos);
        }

        let mut written = 0;
        for (region, positions) in by_region {
            // Merge into whatever the region already holds, so chunks saved in
            // an earlier session are not dropped.
            let mut stored = self.read_region(region)?;
            for pos in &positions {
                let chunk = world.chunk(*pos).expect("position came from the loaded set");
                stored.insert(*pos, StoredChunk::from_chunk(chunk, world.registry()));
            }
            self.write_region(region, &stored)?;

            for pos in positions {
                if let Some(chunk) = world.chunk_mut(pos) {
                    chunk.clear_modified();
                }
                written += 1;
            }
        }
        Ok(written)
    }

    /// Load one chunk, or `None` if it was never saved.
    pub fn load_chunk(
        &self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Option<Chunk>, SaveError> {
        let stored = self.read_region(region_of(pos))?;
        stored
            .remove_owned(pos)
            .map(|chunk| chunk.into_chunk(pos, registry))
            .transpose()
    }

    fn read_region(&self, region: (i32, i32)) -> Result<Region, SaveError> {
        let path = self.region_path(region);
        if !path.is_file() {
            return Ok(Region::default());
        }
        let bytes = std::fs::read(&path)?;
        Region::decode(&bytes)
    }

    fn write_region(&self, region: (i32, i32), contents: &Region) -> Result<(), SaveError> {
        write_atomically(&self.region_path(region), &contents.encode())
    }
}

/// The chunks held by one region file.
#[derive(Debug, Default)]
struct Region {
    chunks: HashMap<ChunkPos, StoredChunk>,
}

impl Region {
    fn insert(&mut self, pos: ChunkPos, chunk: StoredChunk) {
        self.chunks.insert(pos, chunk);
    }

    fn remove_owned(mut self, pos: ChunkPos) -> Option<StoredChunk> {
        self.chunks.remove(&pos)
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(REGION_MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());

        // Sorted, so a save is byte-identical given identical contents rather
        // than varying with hash iteration order.
        let mut positions: Vec<&ChunkPos> = self.chunks.keys().collect();
        positions.sort();

        for pos in positions {
            let chunk = &self.chunks[pos];
            out.extend_from_slice(&pos.x.to_le_bytes());
            out.extend_from_slice(&pos.z.to_le_bytes());
            out.extend_from_slice(&chunk.len.to_le_bytes());
            out.extend_from_slice(&chunk.bits.to_le_bytes());

            out.extend_from_slice(&(chunk.palette.len() as u32).to_le_bytes());
            for name in &chunk.palette {
                out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                out.extend_from_slice(name.as_bytes());
            }

            out.extend_from_slice(&(chunk.data.len() as u32).to_le_bytes());
            for word in &chunk.data {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(REGION_MAGIC, "region")?;
        reader.expect_version()?;

        let count = reader.u32()?;
        let mut region = Region::default();

        for _ in 0..count {
            let pos = ChunkPos::new(reader.i32()?, reader.i32()?);
            let len = reader.u32()?;
            let bits = reader.u32()?;

            let palette_len = reader.u32()?;
            if palette_len > MAX_PALETTE {
                return Err(SaveError::Corrupt(format!(
                    "palette of {palette_len} entries exceeds the {MAX_PALETTE} maximum"
                )));
            }
            let mut palette = Vec::with_capacity(palette_len as usize);
            for _ in 0..palette_len {
                palette.push(reader.string()?);
            }

            let data_len = reader.u32()?;
            let mut data = Vec::with_capacity(data_len as usize);
            for _ in 0..data_len {
                data.push(reader.u64()?);
            }

            region.insert(
                pos,
                StoredChunk {
                    len,
                    bits,
                    palette,
                    data,
                },
            );
        }
        Ok(region)
    }
}

/// Write via a temporary file and rename, so an interrupted save cannot leave a
/// half-written world behind.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Bounds-checked reader over the byte buffer.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], SaveError> {
        let end = self.at.checked_add(count).ok_or(SaveError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(SaveError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn expect_magic(&mut self, magic: &[u8; 4], kind: &'static str) -> Result<(), SaveError> {
        if self.take(4)? != magic {
            return Err(SaveError::BadMagic { kind });
        }
        Ok(())
    }

    fn expect_version(&mut self) -> Result<(), SaveError> {
        let found = self.u32()?;
        if found != FORMAT_VERSION {
            return Err(SaveError::BadVersion { found });
        }
        Ok(())
    }

    fn u32(&mut self) -> Result<u32, SaveError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("took exactly four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    fn i32(&mut self) -> Result<i32, SaveError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, SaveError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("took exactly eight bytes");
        Ok(u64::from_le_bytes(bytes))
    }

    fn string(&mut self) -> Result<String, SaveError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| SaveError::Corrupt(format!("block name is not valid UTF-8: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockDef, BlockPos};

    /// A scratch directory that deletes itself.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            // Unique per test without pulling in a temp-file dependency.
            let unique = format!(
                "vx-save-{label}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("could not create scratch directory");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A world with one chunk loaded and a block changed in it.
    fn edited_world() -> (World, BlockPos, BlockId) {
        let mut world = World::new(4242);
        world.load_around(ChunkPos::new(0, 0), 1);

        let stone = world.registry().id_of("engine:stone").unwrap();
        let position = BlockPos::new(4, 200, 4);
        world.set_block(position, stone).unwrap();
        (world, position, stone)
    }

    #[test]
    fn region_grouping_floors_for_negative_chunks() {
        assert_eq!(region_of(ChunkPos::new(0, 0)), (0, 0));
        assert_eq!(region_of(ChunkPos::new(31, 31)), (0, 0));
        assert_eq!(region_of(ChunkPos::new(32, 0)), (1, 0));
        // The bug this guards is the same one as chunk lookup: truncating
        // division would fold -1 and 0 into the same region.
        assert_eq!(region_of(ChunkPos::new(-1, -1)), (-1, -1));
        assert_eq!(region_of(ChunkPos::new(-32, 0)), (-1, 0));
        assert_eq!(region_of(ChunkPos::new(-33, 0)), (-2, 0));
    }

    #[test]
    fn a_world_round_trips_through_disk() {
        let dir = TempDir::new("roundtrip");
        let save = WorldSave::create(dir.path()).unwrap();
        let (mut world, position, stone) = edited_world();

        assert_eq!(save.save_world(&mut world).unwrap(), 1);

        let loaded = save
            .load_chunk(position.chunk(), world.registry())
            .unwrap()
            .expect("the edited chunk should have been saved");

        assert_eq!(loaded.get(position.local().unwrap()), stone);
    }

    #[test]
    fn the_seed_survives_a_round_trip() {
        let dir = TempDir::new("seed");
        let save = WorldSave::create(dir.path()).unwrap();
        let mut world = World::new(987654321);
        world.load_chunk(ChunkPos::new(0, 0));

        save.save_world(&mut world).unwrap();

        assert!(save.exists());
        assert_eq!(save.read_meta().unwrap(), 987654321);
    }

    #[test]
    fn only_modified_chunks_are_written() {
        // Untouched terrain regenerates from the seed, so writing it would be
        // pure waste.
        let dir = TempDir::new("modified-only");
        let save = WorldSave::create(dir.path()).unwrap();
        let (mut world, _, _) = edited_world();

        assert!(world.loaded_chunk_count() > 1, "expected several chunks loaded");
        assert_eq!(save.save_world(&mut world).unwrap(), 1);

        // An untouched neighbour was never stored.
        let untouched = ChunkPos::new(1, 0);
        assert!(save.load_chunk(untouched, world.registry()).unwrap().is_none());
    }

    #[test]
    fn saving_twice_writes_nothing_the_second_time() {
        let dir = TempDir::new("idempotent");
        let save = WorldSave::create(dir.path()).unwrap();
        let (mut world, _, _) = edited_world();

        assert_eq!(save.save_world(&mut world).unwrap(), 1);
        assert_eq!(
            save.save_world(&mut world).unwrap(),
            0,
            "an unchanged world should not be rewritten"
        );
    }

    #[test]
    fn a_later_save_does_not_drop_chunks_from_an_earlier_one() {
        // Both chunks live in the same region file, which is rewritten whole.
        let dir = TempDir::new("merge");
        let save = WorldSave::create(dir.path()).unwrap();

        let mut world = World::new(11);
        world.load_around(ChunkPos::new(0, 0), 1);
        let stone = world.registry().id_of("engine:stone").unwrap();

        world.set_block(BlockPos::new(4, 200, 4), stone).unwrap();
        save.save_world(&mut world).unwrap();

        // A second session edits a different chunk.
        world.set_block(BlockPos::new(20, 200, 4), stone).unwrap();
        save.save_world(&mut world).unwrap();

        for pos in [ChunkPos::new(0, 0), ChunkPos::new(1, 0)] {
            assert!(
                save.load_chunk(pos, world.registry()).unwrap().is_some(),
                "chunk {pos:?} was lost"
            );
        }
    }

    #[test]
    fn blocks_survive_a_registry_whose_ids_have_all_shifted() {
        // The test this format exists for. Saving with one block set, then
        // loading with extra blocks registered *first*, shifts every numeric
        // id. Keying on names is what keeps the build intact.
        let dir = TempDir::new("id-shift");
        let save = WorldSave::create(dir.path()).unwrap();

        let (mut world, position, _) = edited_world();
        save.save_world(&mut world).unwrap();

        // Rebuild a registry with a mod's blocks inserted ahead of the
        // built-ins, so "engine:stone" lands on a completely different id.
        let mut shifted = BlockRegistry::new();
        for extra in 0..5 {
            shifted
                .register(BlockDef::uniform(format!("mymod:filler{extra}"), 0))
                .unwrap();
        }
        let rebuilt = crate::gen::TerrainBlocks::register_builtins(&mut shifted);

        let original_stone = world.registry().id_of("engine:stone").unwrap();
        assert_ne!(
            original_stone, rebuilt.stone,
            "the test is meaningless unless the ids actually moved"
        );

        let loaded = save.load_chunk(position.chunk(), &shifted).unwrap().unwrap();

        assert_eq!(
            loaded.get(position.local().unwrap()),
            rebuilt.stone,
            "the block decoded to the wrong type after ids shifted"
        );
    }

    #[test]
    fn a_block_whose_mod_is_gone_loads_as_air() {
        let dir = TempDir::new("missing-mod");
        let save = WorldSave::create(dir.path()).unwrap();

        let mut world = World::new(5);
        world.load_chunk(ChunkPos::new(0, 0));
        // Register a block that the loading registry will not have.
        let mut registry = world.registry().clone();
        let exotic = registry
            .register(BlockDef::uniform("mymod:exotic", 9))
            .unwrap();

        let position = BlockPos::new(2, 200, 2);
        world.set_block(position, exotic).unwrap();
        save.save_world(&mut world).unwrap();

        // Load against a registry without the mod.
        let plain = World::new(5);
        let loaded = save
            .load_chunk(position.chunk(), plain.registry())
            .unwrap()
            .unwrap();

        assert!(
            loaded.get(position.local().unwrap()).is_air(),
            "an unknown block should decode to air, not to whatever holds its old id"
        );
    }

    #[test]
    fn loading_a_chunk_that_was_never_saved_returns_nothing() {
        let dir = TempDir::new("absent");
        let save = WorldSave::create(dir.path()).unwrap();
        let world = World::new(1);

        assert!(save
            .load_chunk(ChunkPos::new(77, -12), world.registry())
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_loaded_chunk_is_not_marked_for_saving_again() {
        let dir = TempDir::new("clean-load");
        let save = WorldSave::create(dir.path()).unwrap();
        let (mut world, position, _) = edited_world();
        save.save_world(&mut world).unwrap();

        let loaded = save
            .load_chunk(position.chunk(), world.registry())
            .unwrap()
            .unwrap();

        assert!(!loaded.is_modified(), "a freshly loaded chunk needs no save");
        assert!(loaded.is_dirty(), "a freshly loaded chunk does need meshing");
    }

    #[test]
    fn a_truncated_region_file_is_rejected_rather_than_panicking() {
        let dir = TempDir::new("truncated");
        let save = WorldSave::create(dir.path()).unwrap();
        let (mut world, position, _) = edited_world();
        save.save_world(&mut world).unwrap();

        let path = save.region_path(region_of(position.chunk()));
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();

        let result = save.load_chunk(position.chunk(), world.registry());
        assert!(matches!(
            result,
            Err(SaveError::Truncated) | Err(SaveError::Corrupt(_)) | Err(SaveError::Storage(_))
        ), "got {result:?}");
    }

    #[test]
    fn a_file_that_is_not_a_region_is_rejected() {
        let dir = TempDir::new("bad-magic");
        let save = WorldSave::create(dir.path()).unwrap();
        let pos = ChunkPos::new(0, 0);
        std::fs::write(save.region_path(region_of(pos)), b"this is not a region file").unwrap();

        let world = World::new(1);
        assert!(matches!(
            save.load_chunk(pos, world.registry()),
            Err(SaveError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_future_format_version_is_refused() {
        let dir = TempDir::new("version");
        let save = WorldSave::create(dir.path()).unwrap();
        let pos = ChunkPos::new(0, 0);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(REGION_MAGIC);
        bytes.extend_from_slice(&(FORMAT_VERSION + 99).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(save.region_path(region_of(pos)), bytes).unwrap();

        let world = World::new(1);
        assert!(matches!(
            save.load_chunk(pos, world.registry()),
            Err(SaveError::BadVersion { .. })
        ));
    }

    #[test]
    fn encoding_is_stable_for_identical_contents() {
        // Byte-identical saves make it obvious when a write was unnecessary,
        // and stop hash ordering from churning the file every session.
        let (mut world, _, _) = edited_world();
        let chunk = world.chunk(ChunkPos::new(0, 0)).unwrap();

        let mut region = Region::default();
        region.insert(
            ChunkPos::new(0, 0),
            StoredChunk::from_chunk(chunk, world.registry()),
        );
        let mut other = Region::default();
        other.insert(
            ChunkPos::new(0, 0),
            StoredChunk::from_chunk(chunk, world.registry()),
        );

        assert_eq!(region.encode(), other.encode());
        let _ = world.chunk_mut(ChunkPos::new(0, 0));
    }

    #[test]
    fn many_edits_across_regions_all_come_back() {
        let dir = TempDir::new("many");
        let save = WorldSave::create(dir.path()).unwrap();

        let mut world = World::new(31337);
        let stone = world.registry().id_of("engine:stone").unwrap();

        // Chunks far enough apart to land in different region files.
        let targets = [
            BlockPos::new(4, 200, 4),
            BlockPos::new(600, 200, 4),
            BlockPos::new(-600, 200, -600),
        ];
        for target in targets {
            world.load_chunk(target.chunk());
            world.set_block(target, stone).unwrap();
        }

        assert_eq!(save.save_world(&mut world).unwrap(), targets.len());

        for target in targets {
            let loaded = save
                .load_chunk(target.chunk(), world.registry())
                .unwrap()
                .unwrap_or_else(|| panic!("chunk for {target:?} was not saved"));
            assert_eq!(loaded.get(target.local().unwrap()), stone);
        }
    }
}
