//! Gitup — a graphical Git client.
//!
//! Architecture in one paragraph: the UI thread runs egui and never touches
//! libgit2. All Git work goes through [`job::JobSystem`], which owns a pool of
//! worker threads holding their own `git2::Repository` handles and returns
//! immutable snapshots. See [`job`] for why supersession matters.
//!
//! The crate is a library so that the UI can be driven headlessly by tests;
//! `main.rs` is a thin wrapper that opens a window around it.

pub mod app;
pub mod error;
pub mod git;
pub mod job;
pub mod settings;
pub mod state;
pub mod ui;
pub mod util;
pub mod watch;

pub const APP_ID: &str = "dev.gitup.Gitup";
pub const APP_NAME: &str = "Gitup";
