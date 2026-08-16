//! Chunk storage, terrain generation and world state.
//!
//! This crate is the simulation half of the engine. It has no dependency on
//! rendering or windowing, so it can be driven headlessly — by tests today, by
//! a dedicated server later.

pub mod body;
pub mod chunk;
pub mod gen;
pub mod light;
pub mod noise;
pub mod raycast;
pub mod storage;
pub mod tick;
pub mod world;

pub use body::{Body, StepResult};
pub use chunk::{BlockView, Chunk, SoloChunkView};
pub use gen::{TerrainBlocks, TerrainGenerator, SEA_LEVEL};
pub use light::{Channel, LightGrid, LightQueue, MAX_LIGHT, RELIGHT_BUDGET};
pub use noise::Fbm;
pub use raycast::{cast_ray, RayHit};
pub use storage::PalettedStorage;
pub use tick::{Refusal, TickClock, TickLimits, TickScheduler, TICKS_PER_SECOND};
pub use world::{EditError, TickReport, World};
