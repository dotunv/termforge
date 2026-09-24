//! Design system for TermForge.
//!
//! This crate is deliberately framework-free: it defines tokens (spacing,
//! radii, type scale, motion) and generates full themes from three inputs,
//! in the style of Linear's theme system. The GPUI layer in `tf-app` maps
//! these values onto its own types, so a UI framework change never touches
//! design decisions. See `docs/adr/0001-ui-framework.md`.

pub mod color;
pub mod theme;
pub mod tokens;

pub use color::{Oklch, Rgb};
pub use theme::{Theme, ThemeInput};
