//! Core types shared by every other crate in the engine.
//!
//! `vx-core` deliberately has no dependency on rendering, windowing or the OS.
//! It holds the vocabulary — blocks, faces, coordinates, events — that the
//! simulation and presentation halves both speak, which is what keeps the
//! networking seam described in the design docs viable.

pub mod block;
pub mod event;
pub mod face;
pub mod pos;

pub use block::{BlockDef, BlockId, BlockRegistry, RegistryError};
pub use event::{Cancellable, Event, EventBus, Priority, PRIORITY_HIGH, PRIORITY_LOW, PRIORITY_NORMAL};
pub use face::Face;
pub use pos::{BlockPos, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE, CHUNK_VOLUME};
