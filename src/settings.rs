//! Persisted preferences and the recent-repository list.

use crate::ui::ThemeMode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeMode,
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
    /// Repositories open in tabs, in tab order, restored on the next launch.
    #[serde(default)]
    pub open_tabs: Vec<PathBuf>,
    /// Which tab was showing.
    #[serde(default)]
    pub active_tab: usize,
    pub show_ignored: bool,
    pub sidebar_width: f32,
    /// The detail pane's share of the centre, 0..1.
    ///
    /// A share rather than a height so that the diff grows with the window.
    /// Absent from an older settings file, in which case the default applies.
    #[serde(default = "default_detail_share")]
    pub detail_share: f32,
    /// Height of the commit message box, in pixels.
    #[serde(default = "default_commit_box")]
    pub commit_box_height: f32,
    /// Refresh automatically when the filesystem changes.
    pub auto_refresh: bool,
    pub syntax_highlighting: bool,
    pub diff_layout: crate::ui::diff::DiffLayout,
    #[serde(default)]
    pub keymap: crate::ui::keymap::Keymap,

    /// Whether [`Self::save`] actually writes to disk.
    ///
    /// Tests open throwaway repositories, and those must not end up in the
    /// developer's real recent-repository list.
    #[serde(skip, default = "enabled")]
    pub persist: bool,
}

fn enabled() -> bool {
    true
}

fn default_detail_share() -> f32 {
    crate::ui::layout::DETAIL_SHARE
}

fn default_commit_box() -> f32 {
    crate::ui::metrics::COMMIT_BOX
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            recent: Vec::new(),
            open_tabs: Vec::new(),
            active_tab: 0,
            show_ignored: false,
            sidebar_width: crate::ui::metrics::SIDEBAR_DEFAULT,
            detail_share: default_detail_share(),
            commit_box_height: default_commit_box(),
            auto_refresh: true,
            syntax_highlighting: true,
            diff_layout: crate::ui::diff::DiffLayout::default(),
            keymap: crate::ui::keymap::Keymap::default(),
            persist: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = config_file() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                // A malformed config should not stop the app from starting.
                tracing::warn!("ignoring unreadable settings at {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Settings that never touch disk. For tests.
    pub fn ephemeral() -> Self {
        Self {
            persist: false,
            ..Self::default()
        }
    }

    pub fn save(&self) {
        if !self.persist {
            return;
        }
        let Some(path) = config_file() else { return };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("couldn't create {}: {e}", dir.display());
                return;
            }
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::warn!("couldn't write settings: {e}");
                }
            }
            Err(e) => tracing::warn!("couldn't serialize settings: {e}"),
        }
    }

    /// Move `path` to the front of the recent list, de-duplicating.
    pub fn touch_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn forget_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
    }

    /// Recent entries that still exist, so the list doesn't fill with paths
    /// pointing at deleted directories.
    pub fn existing_recent(&self) -> Vec<PathBuf> {
        self.recent.iter().filter(|p| p.exists()).cloned().collect()
    }

    /// Tabs worth restoring: those whose directory is still there.
    ///
    /// A repository moved or deleted between sessions should not produce an
    /// error on every launch; it just stops being a tab.
    pub fn restorable_tabs(&self) -> Vec<PathBuf> {
        self.open_tabs
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect()
    }
}

fn config_file() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "gitup", "Gitup")
        .map(|d| d.config_dir().join("settings.toml"))
}
