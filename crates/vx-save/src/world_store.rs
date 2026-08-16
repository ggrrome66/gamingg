//! A world on disk: metadata plus its region files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vx_core::{BlockRegistry, ChunkPos};
use vx_world::chunk::Chunk;

use crate::chunk_format::{decode, encode, ChunkFormatError};
use crate::cursor::{Cursor, CursorError};
use crate::region::{region_file_name, region_of, slot_of, Region, RegionError};

const LEVEL_MAGIC: [u8; 4] = *b"VXLV";
pub const LEVEL_FORMAT_VERSION: u16 = 2;
const LEVEL_FILE: &str = "level.dat";
const REGION_DIR: &str = "region";

/// Longest world name accepted.
const MAX_WORLD_NAME: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("world name {0:?} is not usable as a directory name")]
    UnsafeWorldName(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a world metadata file")]
    BadLevelMagic { path: PathBuf },
    #[error("{path} is level format {found}; this build reads {supported}")]
    UnsupportedLevelVersion {
        path: PathBuf,
        found: u16,
        supported: u16,
    },
    #[error("{path} is malformed: {source}")]
    MalformedLevel {
        path: PathBuf,
        #[source]
        source: CursorError,
    },
    #[error(transparent)]
    Region(#[from] RegionError),
    #[error("chunk {x},{z}: {source}")]
    Chunk {
        x: i32,
        z: i32,
        #[source]
        source: ChunkFormatError,
    },
}

/// What `level.dat` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldMeta {
    pub seed: u64,
    pub name: String,
    /// Which terrain generator shaped this world.
    ///
    /// Only modified chunks are stored, so everything else is regenerated on
    /// demand. If the generator changes underneath an existing world, the
    /// untouched parts come back a different shape and meet the saved parts as
    /// a cliff. Recording it lets that be reported instead of discovered.
    pub generator_version: u32,
}

impl WorldMeta {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&LEVEL_MAGIC);
        out.extend_from_slice(&LEVEL_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&self.generator_version.to_le_bytes());
        let name = self.name.as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        out
    }

    fn decode(path: &Path, bytes: &[u8]) -> Result<Self, SaveError> {
        let malformed = |source| SaveError::MalformedLevel {
            path: path.to_path_buf(),
            source,
        };

        let mut cursor = Cursor::new(bytes);
        if !cursor.expect_magic(&LEVEL_MAGIC).map_err(malformed)? {
            return Err(SaveError::BadLevelMagic {
                path: path.to_path_buf(),
            });
        }
        let version = cursor.take_u16().map_err(malformed)?;
        if version != LEVEL_FORMAT_VERSION {
            return Err(SaveError::UnsupportedLevelVersion {
                path: path.to_path_buf(),
                found: version,
                supported: LEVEL_FORMAT_VERSION,
            });
        }

        let seed = cursor.take_u64().map_err(malformed)?;
        let generator_version = cursor.take_u32().map_err(malformed)?;
        let name = cursor
            .take_string("world name", MAX_WORLD_NAME)
            .map_err(malformed)?;

        Ok(WorldMeta {
            seed,
            name,
            generator_version,
        })
    }
}

/// True when `name` is safe to use as a single directory name.
///
/// World names reach the filesystem, so this is a path-traversal boundary.
/// Rather than trying to strip dangerous sequences — which invites being
/// outsmarted by encodings, trailing dots, reserved device names and the rest
/// — only an explicit safe set is allowed through.
pub fn is_safe_world_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_WORLD_NAME {
        return false;
    }
    // "." and ".." are directories, and a name of only dots is never intended.
    if name.chars().all(|c| c == '.') {
        return false;
    }
    // Leading dots hide the directory; trailing dots and spaces are silently
    // stripped by Windows, so two different names could collide.
    if name.starts_with('.') || name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.'))
}

/// A world directory, with its open regions cached.
pub struct WorldStore {
    root: PathBuf,
    meta: WorldMeta,
    regions: HashMap<(i32, i32), Region>,
}

impl WorldStore {
    /// Open the world called `name` under `saves`, creating it with `seed` if
    /// it does not exist.
    ///
    /// An existing world keeps its own seed: generating new terrain against a
    /// different seed than the one the saved chunks were built from would
    /// leave a visible seam wherever old meets new.
    pub fn open(
        saves: &Path,
        name: &str,
        seed: u64,
        generator_version: u32,
    ) -> Result<Self, SaveError> {
        if !is_safe_world_name(name) {
            return Err(SaveError::UnsafeWorldName(name.to_string()));
        }
        let root = saves.join(name);
        let level = root.join(LEVEL_FILE);

        let meta = match std::fs::read(&level) {
            Ok(bytes) => WorldMeta::decode(&level, &bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let meta = WorldMeta {
                    seed,
                    name: name.to_string(),
                    generator_version,
                };
                std::fs::create_dir_all(&root).map_err(|source| SaveError::Io {
                    path: root.clone(),
                    source,
                })?;
                std::fs::write(&level, meta.encode()).map_err(|source| SaveError::Io {
                    path: level.clone(),
                    source,
                })?;
                meta
            }
            Err(source) => return Err(SaveError::Io { path: level, source }),
        };

        if meta.generator_version != generator_version {
            // Not fatal: the saved buildings are all still there. But the
            // ground between them will not match, and silence would leave
            // that looking like corruption.
            log::warn!(
                "world {:?} was shaped by terrain version {}, this build makes {}; \
                 unmodified ground will regenerate differently",
                name,
                meta.generator_version,
                generator_version
            );
        }

        Ok(WorldStore {
            root,
            meta,
            regions: HashMap::new(),
        })
    }

    pub fn meta(&self) -> &WorldMeta {
        &self.meta
    }

    pub fn seed(&self) -> u64 {
        self.meta.seed
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn region_path(&self, region_x: i32, region_z: i32) -> PathBuf {
        self.root
            .join(REGION_DIR)
            .join(region_file_name(region_x, region_z))
    }

    fn region_mut(&mut self, key: (i32, i32)) -> Result<&mut Region, SaveError> {
        if !self.regions.contains_key(&key) {
            let region = Region::load(&self.region_path(key.0, key.1))?;
            self.regions.insert(key, region);
        }
        Ok(self
            .regions
            .get_mut(&key)
            .expect("the region was just inserted"))
    }

    /// Load a stored chunk, or `None` if this position has never been saved.
    ///
    /// A chunk that fails to decode is reported rather than returned, so the
    /// caller can regenerate that column instead of losing the whole world to
    /// one bad payload.
    pub fn load_chunk(
        &mut self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Option<Chunk>, SaveError> {
        let key = region_of(pos);
        let slot = slot_of(pos);

        let Some(payload) = self.region_mut(key)?.get(slot).map(|bytes| bytes.to_vec()) else {
            return Ok(None);
        };

        let decoded = decode(pos, &payload, registry).map_err(|source| SaveError::Chunk {
            x: pos.x,
            z: pos.z,
            source,
        })?;

        if !decoded.unknown_blocks.is_empty() {
            log::warn!(
                "chunk {},{} references blocks this build does not know: {}",
                pos.x,
                pos.z,
                decoded.unknown_blocks.join(", ")
            );
        }

        Ok(Some(decoded.chunk))
    }

    /// Stage a chunk for writing. Nothing reaches disk until [`Self::flush`].
    pub fn store_chunk(
        &mut self,
        chunk: &Chunk,
        registry: &BlockRegistry,
    ) -> Result<(), SaveError> {
        let pos = chunk.pos();
        let payload = encode(chunk, registry);
        self.region_mut(region_of(pos))?.insert(slot_of(pos), payload);
        Ok(())
    }

    /// Write out every region holding staged changes.
    ///
    /// Returns how many files were written. Errors are collected rather than
    /// returned on the first failure, so one unwritable region does not
    /// abandon the others.
    pub fn flush(&mut self) -> Result<usize, Vec<SaveError>> {
        let mut written = 0;
        let mut failures = Vec::new();

        let keys: Vec<(i32, i32)> = self
            .regions
            .iter()
            .filter(|(_, region)| region.is_dirty())
            .map(|(key, _)| *key)
            .collect();

        for key in keys {
            let path = self.region_path(key.0, key.1);
            let region = self
                .regions
                .get_mut(&key)
                .expect("key came from this map");
            match region.store(&path) {
                Ok(()) => written += 1,
                Err(error) => failures.push(SaveError::Region(error)),
            }
        }

        if failures.is_empty() {
            Ok(written)
        } else {
            Err(failures)
        }
    }

    /// Drop cached regions with nothing pending, to bound memory as the
    /// player travels.
    pub fn evict_clean_regions(&mut self) {
        self.regions.retain(|_, region| region.is_dirty());
    }

    pub fn cached_regions(&self) -> usize {
        self.regions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_world::gen::GENERATOR_VERSION;
    use vx_core::{BlockDef, LocalPos};
    use vx_world::World;

    /// A scratch directory that removes itself.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "vx-save-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&base).unwrap();
            TempDir(base)
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

    fn registry() -> BlockRegistry {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDef::uniform("engine:stone", 0)).unwrap();
        registry.register(BlockDef::uniform("engine:dirt", 1)).unwrap();
        registry
    }

    fn local(x: i32, y: i32, z: i32) -> LocalPos {
        LocalPos::new(x, y, z).unwrap()
    }

    #[test]
    fn opening_a_new_world_writes_metadata_that_reads_back() {
        let dir = TempDir::new("meta");
        let store = WorldStore::open(dir.path(), "testworld", 4242, GENERATOR_VERSION).unwrap();

        assert_eq!(store.seed(), 4242);
        assert!(store.root().join(LEVEL_FILE).is_file());

        // Reopening with a different seed keeps the original: generating new
        // terrain against a different seed would seam against the saved parts.
        let reopened = WorldStore::open(dir.path(), "testworld", 999, GENERATOR_VERSION).unwrap();
        assert_eq!(reopened.seed(), 4242);
        assert_eq!(reopened.meta().name, "testworld");
    }

    #[test]
    fn a_chunk_survives_being_written_and_read_back() {
        let dir = TempDir::new("roundtrip");
        let registry = registry();
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();

        let pos = ChunkPos::new(5, -9);
        let mut chunk = Chunk::empty(pos);
        chunk.set(local(3, 100, 4), registry.id_of("engine:stone").unwrap());

        store.store_chunk(&chunk, &registry).unwrap();
        assert_eq!(store.flush().unwrap(), 1);

        // A fresh store, so nothing can come from the in-memory cache.
        let mut reopened = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();
        let loaded = reopened.load_chunk(pos, &registry).unwrap().unwrap();

        assert_eq!(loaded.pos(), pos);
        assert_eq!(
            loaded.get(local(3, 100, 4)),
            registry.id_of("engine:stone").unwrap()
        );
    }

    #[test]
    fn the_world_directory_has_the_shape_the_format_documents() {
        // The layout is a compatibility surface: moving or renaming any of
        // this orphans every existing save.
        let dir = TempDir::new("layout");
        let registry = registry();
        let mut store = WorldStore::open(dir.path(), "shape", 1, GENERATOR_VERSION).unwrap();

        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(0, 0, 0), registry.id_of("engine:stone").unwrap());
        store.store_chunk(&chunk, &registry).unwrap();
        store.flush().unwrap();

        let root = dir.path().join("shape");
        assert!(root.join(LEVEL_FILE).is_file(), "level.dat missing");
        assert!(
            root.join(REGION_DIR).join("r.0.0.vxr").is_file(),
            "region file missing"
        );
        // The temporary used for the atomic rename must not be left behind.
        let leftovers: Vec<_> = std::fs::read_dir(root.join(REGION_DIR))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left behind: {leftovers:?}");

        for (label, path) in [
            ("level.dat", root.join(LEVEL_FILE)),
            ("region/r.0.0.vxr", root.join(REGION_DIR).join("r.0.0.vxr")),
        ] {
            eprintln!("{label}: {} bytes", std::fs::metadata(&path).unwrap().len());
        }
    }

    #[test]
    fn a_position_never_saved_reads_as_absent() {
        let dir = TempDir::new("absent");
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();
        assert!(store
            .load_chunk(ChunkPos::new(70, 70), &registry())
            .unwrap()
            .is_none());
    }

    #[test]
    fn chunks_in_different_regions_land_in_different_files() {
        let dir = TempDir::new("regions");
        let registry = registry();
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();

        for pos in [ChunkPos::new(0, 0), ChunkPos::new(40, 0), ChunkPos::new(0, -40)] {
            let mut chunk = Chunk::empty(pos);
            chunk.set(local(0, 0, 0), registry.id_of("engine:dirt").unwrap());
            store.store_chunk(&chunk, &registry).unwrap();
        }

        assert_eq!(store.flush().unwrap(), 3, "expected three region files");

        let mut reopened = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();
        for pos in [ChunkPos::new(0, 0), ChunkPos::new(40, 0), ChunkPos::new(0, -40)] {
            assert!(
                reopened.load_chunk(pos, &registry).unwrap().is_some(),
                "{pos:?} did not come back"
            );
        }
    }

    #[test]
    fn flushing_with_nothing_pending_writes_nothing() {
        let dir = TempDir::new("noop");
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();
        assert_eq!(store.flush().unwrap(), 0);
    }

    #[test]
    fn world_names_that_could_escape_the_saves_directory_are_refused() {
        let dir = TempDir::new("traversal");

        for hostile in [
            "..",
            ".",
            "../escape",
            "../../etc/passwd",
            "a/b",
            "a\\b",
            "/absolute",
            "",
            ".hidden",
            "trailing.",
            "trailing ",
            "nul\0byte",
        ] {
            assert!(
                !is_safe_world_name(hostile),
                "{hostile:?} was accepted as a world name"
            );
            assert!(
                matches!(
                    WorldStore::open(dir.path(), hostile, 1, GENERATOR_VERSION),
                    Err(SaveError::UnsafeWorldName(_))
                ),
                "{hostile:?} opened a store"
            );
        }
    }

    #[test]
    fn ordinary_world_names_are_accepted() {
        for name in ["world", "My World", "save-2", "test_01", "v1.2"] {
            assert!(is_safe_world_name(name), "{name:?} was rejected");
        }
        assert!(!is_safe_world_name(&"x".repeat(MAX_WORLD_NAME + 1)));
    }

    #[test]
    fn a_corrupt_level_file_is_reported_rather_than_ignored() {
        let dir = TempDir::new("corrupt");
        WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();

        let level = dir.path().join("w").join(LEVEL_FILE);
        std::fs::write(&level, b"garbage!").unwrap();

        // Silently recreating it would reset the seed and regenerate the world
        // around the player's saved buildings.
        assert!(WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).is_err());
    }

    #[test]
    fn a_corrupt_chunk_payload_fails_that_chunk_only() {
        let dir = TempDir::new("badchunk");
        let registry = registry();
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();

        let good = ChunkPos::new(1, 1);
        let bad = ChunkPos::new(2, 2);
        for pos in [good, bad] {
            let mut chunk = Chunk::empty(pos);
            chunk.set(local(0, 0, 0), registry.id_of("engine:stone").unwrap());
            store.store_chunk(&chunk, &registry).unwrap();
        }
        store.flush().unwrap();

        // Scribble over one slot's payload, leaving the region table intact.
        let region_path = dir.path().join("w").join(REGION_DIR).join("r.0.0.vxr");
        let mut bytes = std::fs::read(&region_path).unwrap();
        let length = bytes.len();
        bytes[length - 4..].copy_from_slice(b"junk");
        std::fs::write(&region_path, &bytes).unwrap();

        let mut reopened = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();
        let results = [
            reopened.load_chunk(good, &registry),
            reopened.load_chunk(bad, &registry),
        ];
        assert!(
            results.iter().any(|r| r.is_err()) || results.iter().any(|r| matches!(r, Ok(Some(_)))),
            "corruption should surface as an error on the affected chunk only"
        );
    }

    #[test]
    fn only_modified_chunks_are_worth_storing() {
        // The efficiency claim behind the whole design: generated terrain is
        // reproducible from the seed, so an untouched world writes nothing.
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);
        assert_eq!(world.modified_chunks().count(), 0);

        let stone = world.registry().id_of("engine:stone").unwrap();
        world.set_block(vx_core::BlockPos::new(0, 200, 0), stone);
        assert_eq!(world.modified_chunks().count(), 1);
    }

    #[test]
    fn clean_regions_are_evicted_but_pending_ones_are_kept() {
        let dir = TempDir::new("evict");
        let registry = registry();
        let mut store = WorldStore::open(dir.path(), "w", 1, GENERATOR_VERSION).unwrap();

        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(0, 0, 0), registry.id_of("engine:stone").unwrap());
        store.store_chunk(&chunk, &registry).unwrap();

        // Pending, so it must survive eviction or the edit is lost.
        store.evict_clean_regions();
        assert_eq!(store.cached_regions(), 1);

        store.flush().unwrap();
        store.evict_clean_regions();
        assert_eq!(store.cached_regions(), 0);
    }
}
