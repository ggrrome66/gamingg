//! The on-disk world format.
//!
//! Keeps file I/O out of `vx-world`, which stays a pure simulation crate: this
//! crate depends on it, never the other way round. The app owns the wiring,
//! deciding when a chunk is loaded from disk rather than generated and when
//! modified chunks are written back.
//!
//! Two properties the rest of the engine relies on:
//!
//! **Blocks are stored by namespaced name, never by numeric id.** Ids are
//! assigned in registration order and shift whenever the block set changes, so
//! a world saved under one set would decode as a different world under
//! another.
//!
//! **Only modified chunks are stored.** Worldgen is a pure function of
//! `(seed, position)`, so untouched terrain is recreated exactly on demand and
//! never costs a byte. A world is the diff against what generation would
//! produce.
//!
//! # Reading is a trust boundary
//!
//! Everything decoded here comes from a file that may be truncated by a crash,
//! corrupted on disk, or crafted deliberately. Nothing read is assumed to be
//! consistent: lengths are checked against the bytes actually present before
//! anything is allocated, offsets are range-checked with overflow-safe
//! arithmetic, packed block indices are verified to lie inside their palette,
//! and world names are matched against an allowed set rather than filtered for
//! dangerous sequences. Malformed input produces an error; it never panics.

pub mod chunk_format;
pub mod cursor;
pub mod player_format;
pub mod region;
pub mod world_store;

pub use chunk_format::{decode, encode, ChunkFormatError, DecodedChunk, CHUNK_FORMAT_VERSION};
pub use cursor::{Cursor, CursorError};
pub use player_format::{decode_player, encode_player, PlayerFormatError, PlayerRecord, PLAYER_FORMAT_VERSION};
pub use region::{Region, RegionError, REGION_SIZE};
pub use world_store::{is_safe_world_name, SaveError, WorldMeta, WorldStore, LEVEL_FORMAT_VERSION};
