//! Platform-independent core of obojeno: palette generation, templates and config.

pub mod color;
pub mod config;
pub mod output;
pub mod palette;
pub mod targets;
pub mod template;

pub use color::Color;
pub use palette::{Mode, Palette, SchemeVariant};
