//! Platform-independent core of obojeno: palette generation, templates and config.

pub mod color;
pub mod palette;

pub use color::Color;
pub use palette::{Mode, Palette, SchemeVariant};
