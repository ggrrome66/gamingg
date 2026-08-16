//! Encoding one chunk to bytes and back.
//!
//! The palette is written as **namespaced names**, never numeric ids. Ids are
//! assigned in registration order, so adding, removing or reordering blocks
//! renumbers them and a world saved under one block set would decode as a
//! different world under another — stone becoming water, quietly. Names cost a
//! few dozen bytes per chunk and are the only stable identity there is.
//!
//! The packed index data is stored as-is. It is already the compact
//! representation, so there is nothing to gain by unpacking it to write it.

use vx_core::{BlockId, BlockRegistry, ChunkPos, CHUNK_VOLUME};
use vx_world::chunk::Chunk;
use vx_world::storage::{PalettedStorage, StorageError, MAX_PALETTE};

use crate::cursor::{Cursor, CursorError};

/// Tags the start of a chunk payload.
const MAGIC: [u8; 4] = *b"VXCH";
/// Bumped whenever the layout below changes incompatibly.
pub const CHUNK_FORMAT_VERSION: u16 = 1;

/// Longest block name accepted. Generous for `namespace:some_long_block_name`
/// and far short of anything that could be used to bloat a file.
const MAX_NAME: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum ChunkFormatError {
    #[error("not a chunk payload: bad magic")]
    BadMagic,
    #[error("chunk format version {found} is not supported (this build reads {supported})")]
    UnsupportedVersion { found: u16, supported: u16 },
    #[error("malformed chunk payload: {0}")]
    Malformed(#[from] CursorError),
    #[error("chunk payload describes impossible storage: {0}")]
    Storage(#[from] StorageError),
    #[error("chunk claims {0} blocks; every chunk holds {CHUNK_VOLUME}")]
    WrongVolume(usize),
    #[error("{0} trailing bytes after the chunk payload")]
    TrailingBytes(usize),
}

/// A decoded chunk, plus what could not be resolved.
pub struct DecodedChunk {
    pub chunk: Chunk,
    /// Names in the file that this build's registry does not know, with how
    /// many palette slots each accounted for. They decode to air.
    pub unknown_blocks: Vec<String>,
}

/// Serialise `chunk`, resolving palette ids to names through `registry`.
pub fn encode(chunk: &Chunk, registry: &BlockRegistry) -> Vec<u8> {
    let storage = chunk.storage();
    let palette = storage.palette();
    let data = storage.raw_data();

    let mut out = Vec::with_capacity(64 + palette.len() * 24 + data.len() * 8);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&CHUNK_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(storage.len() as u32).to_le_bytes());

    out.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    for id in palette {
        // An id with no definition can only come from a registry that has
        // changed under us. Air is the safe stand-in and round-trips.
        let name = registry.get(*id).map_or("engine:air", |def| def.name.as_str());
        let bytes = name.as_bytes();
        out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(bytes);
    }

    out.extend_from_slice(&storage.bits_per_index().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for word in data {
        out.extend_from_slice(&word.to_le_bytes());
    }

    out
}

/// Rebuild a chunk from `bytes`, mapping names back through `registry`.
pub fn decode(
    pos: ChunkPos,
    bytes: &[u8],
    registry: &BlockRegistry,
) -> Result<DecodedChunk, ChunkFormatError> {
    let mut cursor = Cursor::new(bytes);

    if !cursor.expect_magic(&MAGIC)? {
        return Err(ChunkFormatError::BadMagic);
    }
    let version = cursor.take_u16()?;
    if version != CHUNK_FORMAT_VERSION {
        return Err(ChunkFormatError::UnsupportedVersion {
            found: version,
            supported: CHUNK_FORMAT_VERSION,
        });
    }

    // Every chunk is the same size; the stored length is a consistency check,
    // not a parameter to trust.
    let len = cursor.take_u32()? as usize;
    if len != CHUNK_VOLUME {
        return Err(ChunkFormatError::WrongVolume(len));
    }

    // Smallest a palette entry can be is a two-byte length plus one byte of
    // name, so three.
    let entries = cursor.take_count("palette", MAX_PALETTE, 3)?;
    let mut palette = Vec::with_capacity(entries);
    let mut unknown_blocks = Vec::new();

    for _ in 0..entries {
        let name = cursor.take_string("block name", MAX_NAME)?;
        match registry.id_of(&name) {
            Some(id) => palette.push(id),
            None => {
                // A block this build has never heard of — a removed mod, or a
                // world from a newer version. Decoding to air loses it, but
                // refusing the whole chunk would lose far more.
                if !unknown_blocks.contains(&name) {
                    unknown_blocks.push(name);
                }
                palette.push(BlockId::AIR);
            }
        }
    }

    let bits = cursor.take_u32()?;
    // Words are eight bytes each, which bounds the count against what is left.
    let words = cursor.take_count("data words", CHUNK_VOLUME, 8)?;
    let mut data = Vec::with_capacity(words);
    for _ in 0..words {
        data.push(cursor.take_u64()?);
    }

    if !cursor.is_empty() {
        return Err(ChunkFormatError::TrailingBytes(cursor.remaining()));
    }

    // `from_parts` is the real gate: it re-derives the word count, checks the
    // bit width against the palette, and verifies every packed index is inside
    // the palette before any of this can be read.
    let storage = PalettedStorage::from_parts(palette, bits, data, len)?;

    Ok(DecodedChunk {
        chunk: Chunk::from_storage(pos, storage),
        unknown_blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockDef, LocalPos};

    fn registry() -> BlockRegistry {
        let mut registry = BlockRegistry::new();
        for name in ["engine:stone", "engine:dirt", "engine:grass"] {
            registry.register(BlockDef::uniform(name, 0)).unwrap();
        }
        registry
    }

    fn local(x: i32, y: i32, z: i32) -> LocalPos {
        LocalPos::new(x, y, z).unwrap()
    }

    fn sample_chunk(registry: &BlockRegistry) -> Chunk {
        let mut chunk = Chunk::empty(ChunkPos::new(2, -3));
        let stone = registry.id_of("engine:stone").unwrap();
        let dirt = registry.id_of("engine:dirt").unwrap();
        chunk.fill_column(0, 0, 0, 40, stone);
        chunk.set(local(5, 70, 9), dirt);
        chunk
    }

    #[test]
    fn a_chunk_survives_a_round_trip_unchanged() {
        let registry = registry();
        let original = sample_chunk(&registry);

        let bytes = encode(&original, &registry);
        let decoded = decode(ChunkPos::new(2, -3), &bytes, &registry).unwrap();

        assert!(decoded.unknown_blocks.is_empty());
        for index in 0..CHUNK_VOLUME {
            let at = LocalPos::from_index(index).unwrap();
            assert_eq!(
                decoded.chunk.get(at),
                original.get(at),
                "block {index} changed across the round trip"
            );
        }
    }

    #[test]
    fn an_air_chunk_encodes_to_almost_nothing() {
        // Uniform storage has no data words at all, and that has to survive
        // the round trip or every empty chunk costs 512 KiB.
        let registry = registry();
        let chunk = Chunk::empty(ChunkPos::new(0, 0));

        let bytes = encode(&chunk, &registry);
        assert!(bytes.len() < 64, "an air chunk took {} bytes", bytes.len());

        let decoded = decode(ChunkPos::new(0, 0), &bytes, &registry).unwrap();
        assert!(decoded.chunk.is_empty());
    }

    #[test]
    fn the_decoded_chunk_is_marked_for_meshing_but_not_for_saving() {
        let registry = registry();
        let bytes = encode(&sample_chunk(&registry), &registry);
        let decoded = decode(ChunkPos::new(2, -3), &bytes, &registry).unwrap();

        assert!(decoded.chunk.is_dirty(), "loaded geometry needs a mesh");
        assert!(
            !decoded.chunk.is_modified(),
            "a chunk straight off disk would be written straight back"
        );
        assert_eq!(decoded.chunk.pos(), ChunkPos::new(2, -3));
    }

    #[test]
    fn blocks_are_keyed_on_name_so_renumbering_cannot_corrupt_a_world() {
        // The bug this prevents: ids come from registration order, so a world
        // saved with one block set must not decode through another's numbering.
        let saved_with = registry();
        let chunk = sample_chunk(&saved_with);
        let bytes = encode(&chunk, &saved_with);

        // Same blocks, registered in a different order, so every id differs.
        let mut loaded_with = BlockRegistry::new();
        for name in ["engine:grass", "engine:dirt", "engine:stone"] {
            loaded_with.register(BlockDef::uniform(name, 0)).unwrap();
        }
        assert_ne!(
            saved_with.id_of("engine:stone"),
            loaded_with.id_of("engine:stone"),
            "the registries need different numbering for this test to mean anything"
        );

        let decoded = decode(ChunkPos::new(2, -3), &bytes, &loaded_with).unwrap();

        // Identity is preserved even though the numbers are not.
        let stone_now = loaded_with.id_of("engine:stone").unwrap();
        assert_eq!(decoded.chunk.get(local(0, 0, 0)), stone_now);
        assert_eq!(
            loaded_with.get(decoded.chunk.get(local(5, 70, 9))).unwrap().name,
            "engine:dirt"
        );
    }

    #[test]
    fn blocks_this_build_does_not_know_decode_to_air_and_are_reported() {
        let saved_with = {
            let mut registry = registry();
            registry.register(BlockDef::uniform("somemod:copper", 0)).unwrap();
            registry
        };
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(1, 1, 1), saved_with.id_of("somemod:copper").unwrap());
        let bytes = encode(&chunk, &saved_with);

        // The mod is gone.
        let decoded = decode(ChunkPos::new(0, 0), &bytes, &registry()).unwrap();

        assert_eq!(decoded.unknown_blocks, vec!["somemod:copper".to_string()]);
        assert!(
            decoded.chunk.get(local(1, 1, 1)).is_air(),
            "an unknown block should read as air, not as some other block"
        );
    }

    #[test]
    fn a_foreign_or_empty_payload_is_refused() {
        let registry = registry();
        let pos = ChunkPos::new(0, 0);

        assert!(matches!(
            decode(pos, b"", &registry),
            Err(ChunkFormatError::Malformed(_))
        ));
        assert!(matches!(
            decode(pos, b"NOPE0000000000000000", &registry),
            Err(ChunkFormatError::BadMagic)
        ));
    }

    #[test]
    fn a_future_format_version_is_refused_rather_than_guessed_at() {
        let registry = registry();
        let mut bytes = encode(&sample_chunk(&registry), &registry);
        bytes[4..6].copy_from_slice(&(CHUNK_FORMAT_VERSION + 1).to_le_bytes());

        assert!(matches!(
            decode(ChunkPos::new(0, 0), &bytes, &registry),
            Err(ChunkFormatError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn a_packed_index_outside_the_palette_is_caught() {
        // The memory-safety-adjacent case: `PalettedStorage::get` indexes the
        // palette directly, so this would panic deep inside meshing if it were
        // allowed through.
        let registry = registry();
        let bytes = encode(&sample_chunk(&registry), &registry);

        // Flip the data words to all-ones, so every packed index is the
        // largest the bit width can express — well past the palette.
        let mut corrupted = bytes.clone();
        let data_start = corrupted.len() - 8;
        corrupted[data_start..].copy_from_slice(&u64::MAX.to_le_bytes());

        match decode(ChunkPos::new(0, 0), &corrupted, &registry) {
            Err(ChunkFormatError::Storage(StorageError::IndexOutOfPalette { .. })) => {}
            Err(other) => panic!("rejected, but for the wrong reason: {other}"),
            Ok(decoded) => {
                // If it were accepted, this read is the panic we are guarding.
                let _ = decoded.chunk.get(local(0, 0, 0));
                panic!("an out-of-palette index was accepted");
            }
        }
    }

    #[test]
    fn truncation_at_any_point_is_an_error_and_never_a_panic() {
        // A crash partway through a write leaves exactly this.
        let registry = registry();
        let full = encode(&sample_chunk(&registry), &registry);

        for cut in 0..full.len() {
            let result = decode(ChunkPos::new(0, 0), &full[..cut], &registry);
            assert!(result.is_err(), "a {cut}-byte prefix decoded successfully");
        }
    }

    #[test]
    fn trailing_rubbish_is_refused() {
        // Silently ignoring extra bytes lets a payload smuggle data past any
        // length accounting the container does.
        let registry = registry();
        let mut bytes = encode(&sample_chunk(&registry), &registry);
        bytes.extend_from_slice(b"extra");

        assert!(matches!(
            decode(ChunkPos::new(0, 0), &bytes, &registry),
            Err(ChunkFormatError::TrailingBytes(5))
        ));
    }

    #[test]
    fn every_single_byte_corruption_is_survivable() {
        // Not that each one is detected — some land in block data and decode
        // to a valid but different world — only that none of them panics.
        let registry = registry();
        let full = encode(&sample_chunk(&registry), &registry);

        for at in 0..full.len() {
            let mut corrupted = full.clone();
            corrupted[at] ^= 0xff;
            if let Ok(decoded) = decode(ChunkPos::new(0, 0), &corrupted, &registry) {
                // Whatever came back must still be safe to read everywhere.
                for index in (0..CHUNK_VOLUME).step_by(997) {
                    let _ = decoded.chunk.get(LocalPos::from_index(index).unwrap());
                }
            }
        }
    }
}
