//! Submodules.
//!
//! A submodule is a pinned commit of another repository. Three facts explain
//! almost every question anyone has about one: which commit the parent expects,
//! which commit is actually checked out, and whether the thing has been cloned
//! at all. This module surfaces those directly rather than reporting a bare
//! "modified" the way a plain status does.

use crate::error::{Error, Result};
use crate::job::Cancel;
use git2::{Oid, Repository, SubmoduleIgnore};
use std::path::Path;
use std::sync::Arc;

/// What state a submodule is in, in the order a user would want to act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleState {
    /// Registered but never cloned.
    Uninitialized,
    /// Checked out at a different commit than the parent records.
    OutOfDate,
    /// Has uncommitted changes of its own.
    Dirty,
    /// Checked out exactly where the parent expects.
    UpToDate,
}

impl SubmoduleState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Uninitialized => "not initialized",
            Self::OutOfDate => "out of date",
            Self::Dirty => "modified",
            Self::UpToDate => "up to date",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Uninitialized => "Registered here, but never cloned",
            Self::OutOfDate => "Checked out at a different commit than this repository records",
            Self::Dirty => "Has uncommitted changes of its own",
            Self::UpToDate => "Exactly where this repository expects it",
        }
    }

    /// Whether updating would change anything.
    pub fn needs_update(self) -> bool {
        matches!(self, Self::Uninitialized | Self::OutOfDate)
    }
}

#[derive(Debug, Clone)]
pub struct SubmoduleEntry {
    pub name: String,
    /// Path relative to the parent's worktree.
    pub path: String,
    pub url: Option<String>,
    /// Commit the parent repository records.
    pub recorded: Option<Oid>,
    /// Commit actually checked out, when it has been cloned.
    pub checked_out: Option<Oid>,
    pub state: SubmoduleState,
}

impl SubmoduleEntry {
    pub fn short_recorded(&self) -> Option<String> {
        self.recorded.map(super::repo::short_id)
    }

    pub fn short_checked_out(&self) -> Option<String> {
        self.checked_out.map(super::repo::short_id)
    }

    /// Absolute path to the submodule's working tree, if the parent has one.
    pub fn absolute_path(&self, parent: &Path) -> std::path::PathBuf {
        parent.join(&self.path)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Submodules {
    pub entries: Vec<SubmoduleEntry>,
}

impl Submodules {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<&SubmoduleEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// How many are not where the parent expects them.
    pub fn needing_attention(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state.needs_update())
            .count()
    }
}

/// List the repository's submodules and work out what state each is in.
///
/// A fresh `Repository` handle is opened rather than reusing the caller's.
/// libgit2 caches submodule configuration inside the handle, and git2 exposes
/// no equivalent of `git_submodule_reload_all` — so a worker's long-lived
/// handle can go on reporting configuration that has since changed. Reopening
/// costs about a millisecond and only happens on a refresh.
pub fn list(repo: &Repository) -> Result<Arc<Submodules>> {
    let repo = &Repository::open(repo.path())
        .map_err(|_| Error::NotARepository(repo.path().to_path_buf()))?;
    let mut entries = Vec::new();

    for submodule in repo.submodules()? {
        let Ok(name) = submodule.name() else { continue };
        let name = name.to_owned();
        let path = submodule.path().to_string_lossy().replace('\\', "/");
        let recorded = submodule.head_id().or_else(|| submodule.index_id());
        let checked_out = submodule.workdir_id();

        // `submodule_status` is the authority on initialization; `workdir_id`
        // alone can't distinguish "not cloned" from "cloned but empty".
        let status = repo
            .submodule_status(&name, SubmoduleIgnore::None)
            .unwrap_or_else(|_| git2::SubmoduleStatus::empty());

        let state = if status.is_wd_uninitialized() || !status.is_in_wd() {
            SubmoduleState::Uninitialized
        } else if status.is_wd_modified() || recorded != checked_out {
            SubmoduleState::OutOfDate
        } else if status.is_wd_wd_modified() || status.is_wd_untracked() {
            SubmoduleState::Dirty
        } else {
            SubmoduleState::UpToDate
        };

        entries.push(SubmoduleEntry {
            name,
            path,
            url: submodule.url().ok().flatten().map(str::to_owned),
            recorded,
            checked_out,
            state,
        });
    }

    entries.sort_by(|a, b| super::refs::natural_cmp(&a.path, &b.path));
    Ok(Arc::new(Submodules { entries }))
}

/// Clone and check out a submodule, or bring it back to the recorded commit.
///
/// Run through the `git` binary rather than libgit2's `Submodule::update`: a
/// submodule clone is a network operation, so it needs the same credential
/// handling as every other one, and `--recursive` handles nesting that libgit2
/// would leave to the caller.
pub fn update(
    workdir: &Path,
    path: Option<&str>,
    cancel: &Cancel,
    on_progress: impl FnMut(crate::job::Progress),
) -> Result<String> {
    let mut args = vec!["submodule", "update", "--init", "--recursive", "--progress"];
    if let Some(path) = path {
        args.push("--");
        args.push(path);
    }
    let output = super::cli::run(workdir, &args, cancel, on_progress)?;

    let summary = output
        .stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.contains('%'))
        .map(str::to_owned)
        .unwrap_or_else(|| match path {
            Some(path) => format!("Updated {path}"),
            None => "Submodules updated".to_owned(),
        });
    Ok(summary)
}

/// Register a new submodule and clone it.
pub fn add(
    workdir: &Path,
    url: &str,
    path: &str,
    cancel: &Cancel,
    on_progress: impl FnMut(crate::job::Progress),
) -> Result<String> {
    if path.trim().is_empty() {
        return Err(Error::refused("A path is required"));
    }
    if url.trim().is_empty() {
        return Err(Error::refused("A URL is required"));
    }
    let args = vec!["submodule", "add", "--progress", url, path];
    super::cli::run(workdir, &args, cancel, on_progress)?;
    Ok(format!("Added submodule at {path}"))
}

/// Stop tracking a submodule.
///
/// `deinit` first, so git removes the working tree it created rather than
/// leaving an orphaned checkout behind.
///
/// The removal is *staged*, not committed — same as `git rm`. Until it is
/// committed the submodule still exists in `HEAD`, and will still be listed,
/// which is correct rather than a bug: nothing has been removed from history
/// yet.
pub fn remove(workdir: &Path, path: &str, cancel: &Cancel) -> Result<String> {
    super::cli::run(
        workdir,
        &["submodule", "deinit", "-f", "--", path],
        cancel,
        |_| {},
    )?;
    super::cli::run(workdir, &["rm", "-f", "--", path], cancel, |_| {})?;
    Ok(format!("Removed {path} — commit to finish"))
}

/// Whether a path inside the parent is a submodule worth opening.
pub fn is_repository(path: &Path) -> bool {
    path.join(".git").exists()
}
