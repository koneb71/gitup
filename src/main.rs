//! Window entry point. Everything of substance lives in the library.

// The window should not be shadowed by a console on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gitup::{app::GitupApp, APP_ID, APP_NAME};
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GITUP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,gitup=info")),
        )
        .with_target(false)
        .init();

    let initial = match parse_args() {
        Args::Run(path) => path,
        Args::Version => {
            println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Args::Help => {
            print!("{USAGE}");
            return Ok(());
        }
    };

    let options = eframe::NativeOptions {
        viewport: {
            let mut viewport = egui::ViewportBuilder::default()
                .with_app_id(APP_ID)
                .with_title(APP_NAME)
                .with_inner_size([1280.0, 820.0])
                .with_min_inner_size([880.0, 560.0]);
            if let Some(icon) = load_icon() {
                viewport = viewport.with_icon(icon);
            }
            viewport
        },
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(GitupApp::new(cc, initial)))),
    )
}

/// The window icon, decoded from the PNG built into the binary.
///
/// A failure here is cosmetic — the app runs fine with the platform default —
/// so it degrades to `None` rather than refusing to start.
fn load_icon() -> Option<std::sync::Arc<egui::IconData>> {
    let bytes = include_bytes!("../assets/icon/gitup.png");
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(std::sync::Arc::new(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }))
}

const USAGE: &str = "\
Gitup — a graphical Git client

USAGE:
    gitup [OPTIONS] [PATH]

ARGS:
    <PATH>    Repository to open. With no argument, the working directory is
              used when it is inside one.

OPTIONS:
    -h, --help       Print this message
    -V, --version    Print the version

ENVIRONMENT:
    GITUP_LOG    Log filter, e.g. `gitup=debug`. Defaults to `warn,gitup=info`.
";

/// What the command line asked for.
enum Args {
    /// Open the window, optionally on a repository.
    Run(Option<PathBuf>),
    Version,
    Help,
}

/// `gitup [path]` — an unrecognized flag is ignored rather than fatal, because
/// a mistyped argument should still get you a window.
///
/// With no argument, the working directory is used when it happens to be inside
/// a repository. That is a convenience of launching from a terminal; from
/// Finder the working directory is `/` and this correctly finds nothing.
fn parse_args() -> Args {
    let mut path = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            // Answered before the window opens, so that a terminal asking a
            // question gets an answer rather than an application.
            "-V" | "--version" => return Args::Version,
            "-h" | "--help" => return Args::Help,
            _ if argument.starts_with('-') => continue,
            _ if path.is_none() => path = Some(PathBuf::from(argument)),
            _ => continue,
        }
    }

    if path.is_some() {
        return Args::Run(path);
    }
    Args::Run(
        std::env::current_dir()
            .ok()
            .filter(|d| gitup::git::repo::discover(d).is_ok()),
    )
}
