//! Chunks grouped into region files.
//!
//! One file per chunk would mean a filesystem entry per 16×16 column, which
//! most filesystems handle badly at scale. Chunks are grouped into 32×32
//! regions instead, each one file with a fixed slot table at the front.
//!
//! A region is read fully into memory and rewritten whole when flushed. That
//! is deliberately simpler than an in-place allocator with a free list: only
//! chunks somebody has actually modified are ever stored, so a region holds
//! far less than its 1024 slots in practice, and rewriting through a temporary
//! file makes a torn write impossible — the old file stays intact until the
//! rename succeeds. If regions ever do grow large, this is the thing to
//! revisit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vx_core::ChunkPos;

use crate::cursor::{Cursor, CursorError};

/// Region edge, in chunks.
pub const REGION_SIZE: i32 = 32;
/// Chunk slots per region.
pub const REGION_SLOTS: usize = (REGION_SIZE * REGION_SIZE) as usize;

const MAGIC: [u8; 4] = *b"VXRG";
pub const REGION_FORMAT_VERSION: u16 = 1;

/// Header is magic, version, reserved, then the slot table.
const HEADER_BYTES: usize = 4 + 2 + 2 + REGION_SLOTS * 8;

/// Ceiling on one stored chunk. A chunk at the widest packing is 512 KiB of
/// indices plus a palette; this leaves generous headroom while still refusing
/// a length field that claims a gigabyte.
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RegionError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a region file")]
    BadMagic { path: PathBuf },
    #[error("{path} is region format {found}; this build reads {supported}")]
    UnsupportedVersion {
        path: PathBuf,
        found: u16,
        supported: u16,
    },
    #[error("{path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: CursorError,
    },
    #[error("{path}: slot {slot} points at {offset}..{end}, outside the {size}-byte file")]
    SlotOutOfBounds {
        path: PathBuf,
        slot: usize,
        offset: usize,
        end: usize,
        size: usize,
    },
    #[error("{path}: slot {slot} claims {length} bytes, above the {MAX_PAYLOAD} allowed")]
    PayloadTooLarge {
        path: PathBuf,
        slot: usize,
        length: usize,
    },
}

/// Which region contains a chunk.
pub fn region_of(pos: ChunkPos) -> (i32, i32) {
    (pos.x.div_euclid(REGION_SIZE), pos.z.div_euclid(REGION_SIZE))
}

/// Slot a chunk occupies within its region.
pub fn slot_of(pos: ChunkPos) -> usize {
    let x = pos.x.rem_euclid(REGION_SIZE) as usize;
    let z = pos.z.rem_euclid(REGION_SIZE) as usize;
    z * REGION_SIZE as usize + x
}

/// File name for a region. Built from integers only, so no caller-supplied
/// text ever reaches the path.
pub fn region_file_name(region_x: i32, region_z: i32) -> String {
    format!("r.{region_x}.{region_z}.vxr")
}

/// One region's chunk payloads, held in memory.
#[derive(Debug, Default)]
pub struct Region {
    payloads: HashMap<usize, Vec<u8>>,
    dirty: bool,
}

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn stored_chunks(&self) -> usize {
        self.payloads.len()
    }

    pub fn get(&self, slot: usize) -> Option<&[u8]> {
        self.payloads.get(&slot).map(|bytes| bytes.as_slice())
    }

    pub fn insert(&mut self, slot: usize, payload: Vec<u8>) {
        debug_assert!(slot < REGION_SLOTS);
        self.payloads.insert(slot, payload);
        self.dirty = true;
    }

    /// Parse a region file's bytes.
    pub fn decode(path: &Path, bytes: &[u8]) -> Result<Self, RegionError> {
        let malformed = |source| RegionError::Malformed {
            path: path.to_path_buf(),
            source,
        };

        let mut cursor = Cursor::new(bytes);
        if !cursor.expect_magic(&MAGIC).map_err(malformed)? {
            return Err(RegionError::BadMagic {
                path: path.to_path_buf(),
            });
        }
        let version = cursor.take_u16().map_err(malformed)?;
        if version != REGION_FORMAT_VERSION {
            return Err(RegionError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: version,
                supported: REGION_FORMAT_VERSION,
            });
        }
        let _reserved = cursor.take_u16().map_err(malformed)?;

        // Read the whole table before touching any payload, so a bad entry is
        // caught before it is used to slice.
        let mut table = Vec::with_capacity(REGION_SLOTS);
        for _ in 0..REGION_SLOTS {
            let offset = cursor.take_u32().map_err(malformed)? as usize;
            let length = cursor.take_u32().map_err(malformed)? as usize;
            table.push((offset, length));
        }

        let mut payloads = HashMap::new();
        for (slot, (offset, length)) in table.into_iter().enumerate() {
            // Zero length means the slot was never written.
            if length == 0 {
                continue;
            }
            if length > MAX_PAYLOAD {
                return Err(RegionError::PayloadTooLarge {
                    path: path.to_path_buf(),
                    slot,
                    length,
                });
            }
            // Checked, because an offset near usize::MAX plus a length would
            // otherwise wrap to something that looks in range.
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len() && offset >= HEADER_BYTES)
                .ok_or_else(|| RegionError::SlotOutOfBounds {
                    path: path.to_path_buf(),
                    slot,
                    offset,
                    end: offset.saturating_add(length),
                    size: bytes.len(),
                })?;

            payloads.insert(slot, bytes[offset..end].to_vec());
        }

        Ok(Region {
            payloads,
            dirty: false,
        })
    }

    /// Serialise the whole region.
    pub fn encode(&self) -> Vec<u8> {
        let total: usize = self.payloads.values().map(|bytes| bytes.len()).sum();
        let mut out = Vec::with_capacity(HEADER_BYTES + total);

        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&REGION_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved

        // Lay the payloads out first so the table can be written in one pass.
        let mut placed: Vec<(usize, usize)> = vec![(0, 0); REGION_SLOTS];
        let mut body = Vec::with_capacity(total);
        for (slot, payload) in &self.payloads {
            placed[*slot] = (HEADER_BYTES + body.len(), payload.len());
            body.extend_from_slice(payload);
        }

        for (offset, length) in placed {
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&(length as u32).to_le_bytes());
        }
        out.extend_from_slice(&body);
        out
    }

    /// Read a region from disk, or an empty one if the file does not exist.
    pub fn load(path: &Path) -> Result<Self, RegionError> {
        match std::fs::read(path) {
            Ok(bytes) => Region::decode(path, &bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Region::new()),
            Err(source) => Err(RegionError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Write the region out, through a temporary file and a rename.
    ///
    /// The rename is what makes this safe to interrupt: until it succeeds the
    /// previous file is untouched, so a crash mid-save costs the newest edits
    /// rather than the whole region.
    pub fn store(&mut self, path: &Path) -> Result<(), RegionError> {
        let write_error = |source| RegionError::Write {
            path: path.to_path_buf(),
            source,
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(write_error)?;
        }

        let temporary = path.with_extension("vxr.tmp");
        std::fs::write(&temporary, self.encode()).map_err(|source| RegionError::Write {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(write_error)?;

        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> PathBuf {
        PathBuf::from("r.0.0.vxr")
    }

    #[test]
    fn chunks_map_onto_regions_and_slots_without_collisions() {
        // Every chunk in a region must land in its own slot, and the region
        // must floor rather than truncate or the world folds at the origin.
        let mut seen = std::collections::HashSet::new();
        for x in 0..REGION_SIZE {
            for z in 0..REGION_SIZE {
                let pos = ChunkPos::new(x, z);
                assert_eq!(region_of(pos), (0, 0));
                assert!(seen.insert(slot_of(pos)), "slot collision at {pos:?}");
            }
        }
        assert_eq!(seen.len(), REGION_SLOTS);
    }

    #[test]
    fn negative_chunks_floor_into_the_region_to_the_west() {
        assert_eq!(region_of(ChunkPos::new(-1, -1)), (-1, -1));
        assert_eq!(region_of(ChunkPos::new(-32, -32)), (-1, -1));
        assert_eq!(region_of(ChunkPos::new(-33, -33)), (-2, -2));
        // And the slot stays in range rather than going negative.
        for x in -70..70 {
            assert!(slot_of(ChunkPos::new(x, x)) < REGION_SLOTS);
        }
    }

    #[test]
    fn region_names_contain_only_generated_digits() {
        // No caller-supplied text reaches the filename, so there is nothing to
        // traverse with.
        let name = region_file_name(-3, 12);
        assert_eq!(name, "r.-3.12.vxr");
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains(".."));
    }

    #[test]
    fn an_empty_region_round_trips() {
        let region = Region::new();
        let bytes = region.encode();
        assert_eq!(bytes.len(), HEADER_BYTES);

        let decoded = Region::decode(&path(), &bytes).unwrap();
        assert_eq!(decoded.stored_chunks(), 0);
        assert!(!decoded.is_dirty());
    }

    #[test]
    fn payloads_round_trip_in_their_own_slots() {
        let mut region = Region::new();
        region.insert(0, b"first".to_vec());
        region.insert(1023, b"last".to_vec());
        region.insert(500, b"middle".to_vec());
        assert!(region.is_dirty());

        let decoded = Region::decode(&path(), &region.encode()).unwrap();

        assert_eq!(decoded.get(0), Some(b"first".as_slice()));
        assert_eq!(decoded.get(1023), Some(b"last".as_slice()));
        assert_eq!(decoded.get(500), Some(b"middle".as_slice()));
        assert_eq!(decoded.get(7), None);
        assert_eq!(decoded.stored_chunks(), 3);
    }

    #[test]
    fn rewriting_a_slot_replaces_it_rather_than_appending() {
        let mut region = Region::new();
        region.insert(4, b"old".to_vec());
        region.insert(4, b"new".to_vec());

        let decoded = Region::decode(&path(), &region.encode()).unwrap();
        assert_eq!(decoded.get(4), Some(b"new".as_slice()));
        assert_eq!(decoded.stored_chunks(), 1);
    }

    #[test]
    fn a_foreign_file_is_refused() {
        assert!(matches!(
            Region::decode(&path(), &[0u8; HEADER_BYTES]),
            Err(RegionError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_future_region_version_is_refused() {
        let mut bytes = Region::new().encode();
        bytes[4..6].copy_from_slice(&(REGION_FORMAT_VERSION + 1).to_le_bytes());

        assert!(matches!(
            Region::decode(&path(), &bytes),
            Err(RegionError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn a_slot_pointing_outside_the_file_is_refused() {
        // The bug this stops is an out-of-range slice, which is a panic.
        let mut region = Region::new();
        region.insert(0, b"payload".to_vec());
        let mut bytes = region.encode();

        // Slot 0's table entry is the first pair after the 8-byte preamble.
        let past_the_end = bytes.len() as u32 - 2;
        bytes[8..12].copy_from_slice(&past_the_end.to_le_bytes());
        bytes[12..16].copy_from_slice(&64u32.to_le_bytes());

        assert!(matches!(
            Region::decode(&path(), &bytes),
            Err(RegionError::SlotOutOfBounds { .. })
        ));
    }

    #[test]
    fn a_slot_offset_that_would_wrap_is_refused() {
        let mut region = Region::new();
        region.insert(0, b"payload".to_vec());
        let mut bytes = region.encode();

        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());

        assert!(Region::decode(&path(), &bytes).is_err());
    }

    #[test]
    fn a_slot_pointing_into_the_header_is_refused() {
        // Otherwise a payload could be made to alias the slot table.
        let mut region = Region::new();
        region.insert(0, b"payload".to_vec());
        let mut bytes = region.encode();

        bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&16u32.to_le_bytes());

        assert!(matches!(
            Region::decode(&path(), &bytes),
            Err(RegionError::SlotOutOfBounds { .. })
        ));
    }

    #[test]
    fn an_absurd_payload_length_is_refused_before_allocating() {
        let mut region = Region::new();
        region.insert(0, b"payload".to_vec());
        let mut bytes = region.encode();

        bytes[12..16].copy_from_slice(&(MAX_PAYLOAD as u32 + 1).to_le_bytes());

        assert!(matches!(
            Region::decode(&path(), &bytes),
            Err(RegionError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn truncation_at_any_point_is_an_error_and_never_a_panic() {
        let mut region = Region::new();
        region.insert(3, b"some payload bytes".to_vec());
        let full = region.encode();

        for cut in 0..full.len() {
            assert!(
                Region::decode(&path(), &full[..cut]).is_err(),
                "a {cut}-byte prefix decoded successfully"
            );
        }
    }

    #[test]
    fn a_missing_file_reads_as_an_empty_region() {
        // First run, or a region nobody has modified anything in.
        let region = Region::load(Path::new("definitely-not-here.vxr")).unwrap();
        assert_eq!(region.stored_chunks(), 0);
    }
}
