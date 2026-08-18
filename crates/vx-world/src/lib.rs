//! Chunk storage, terrain generation and world state.
//!
//! This crate is the simulation half of the engine. It has no dependency on
//! rendering or windowing, so it can be driven headlessly — by tests today, by
//! a dedicated server later.

pub mod chunk;
pub mod edit;
pub mod flora;
pub mod gen;
pub mod noise;
pub mod ore;
pub mod physics;
pub mod raycast;
pub mod save;
pub mod storage;
pub mod village;
pub mod world;

pub use chunk::{BlockView, Chunk, SoloChunkView};
pub use edit::{break_block, place_block, BlockBreakEvent, BlockPlaceEvent, EditError};
pub use gen::{TerrainBlocks, TerrainGenerator, SEA_LEVEL};
pub use noise::Fbm;
pub use ore::{deposits_overlapping, ore_at, Deposit, OreKind};
pub use physics::{collides, Aabb, PlayerBody};
pub use raycast::{raycast, raycast_solid, RayHit};
pub use save::{SaveError, WorldSave};
pub use storage::{PalettedStorage, StorageError};
pub use world::World;
