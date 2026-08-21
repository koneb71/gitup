//! Presentation layer. Reads state, draws, and reports intent back to the app.

pub mod blame;
pub mod command;
pub mod conflict;
pub mod dialog;
pub mod diff;
pub mod graph;
pub mod icons;
pub mod keymap;
pub mod layout;
pub mod settings_panel;
pub mod sidebar;
pub mod tabs;
pub mod text;
pub mod theme;
pub mod widgets;

pub use theme::{metrics, radius, space, Palette, ThemeMode};
