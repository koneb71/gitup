//! Phosphor icon codepoints, vendored from egui-phosphor 0.13 (MIT/Apache-2.0).
//!
//! Vendored rather than depended on because egui-phosphor 0.13 targets egui
//! 0.35, which would pull a second copy of egui into the build.

pub mod fill;
pub mod regular;

pub use regular::*;
