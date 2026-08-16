//! Windowing, input and platform paths.

pub mod input;
pub mod paths;

pub use input::InputState;
pub use paths::{config_dir, data_dir, saves_dir};
