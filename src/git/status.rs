//! Working-tree status: what changed, staged and unstaged, in one snapshot.

use crate::error::Result;
use git2::{Repository, Status, StatusOptions, StatusShow};
use std::sync::Arc;

/// How a single path changed. Mirrors git's delta vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Delta {
    Unmodified,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    Ignored,
    Conflicted,
}

impl Delta {
    /// Single-letter code, matching `git status --short`.
    pub fn code(self) -> char {
        match self {
            Self::Unmodified => ' ',
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChange => 'T',
            Self::Untracked => '?',
            Self::Ignored => '!',
            Self::Conflicted => 'U',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unmodified => "unchanged",
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChange => "type changed",
            Self::Untracked => "untracked",
            Self::Ignored => "ignored",
            Self::Conflicted => "conflicted",
        }
    }

    pub fn is_change(self) -> bool {
        !matches!(self, Self::Unmodified | Self::Ignored)
    }
}

#[derive(Debug, Clone)]
pub struct StatusEntry {
    /// Path relative to the worktree root, always forward-slashed.
    pub path: String,
    /// Previous path, for renames and copies.
    pub orig_path: Option<String>,
    /// Change between HEAD and the index — i.e. what is staged.
    pub staged: Delta,
    /// Change between the index and the working tree — i.e. what is not staged.
    pub unstaged: Delta,
    pub conflicted: bool,
}

impl StatusEntry {
    pub fn has_staged(&self) -> bool {
        self.staged.is_change()
    }

    pub fn has_unstaged(&self) -> bool {
        self.unstaged.is_change()
    }

    /// The name shown in lists, without directories.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn parent_dir(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[..i],
            None => "",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub entries: Vec<StatusEntry>,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub conflict_count: usize,
    pub untracked_count: usize,
}

impl StatusSnapshot {
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.has_staged())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.has_unstaged())
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.conflicted)
    }

    pub fn find(&self, path: &str) -> Option<&StatusEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// Read the full working-tree status.
///
/// Rename detection is enabled for both halves: without it a rename shows up as
/// an unrelated delete plus add, which is exactly the noise that makes reviewing
/// a refactor miserable.
pub fn status(repo: &Repository, include_ignored: bool) -> Result<Arc<StatusSnapshot>> {
    let mut opts = StatusOptions::new();
    opts.show(StatusShow::IndexAndWorkdir)
        .include_untracked(true)
        .include_ignored(include_ignored)
        .include_unmodified(false)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .renames_from_rewrites(true)
        .update_index(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut entries = Vec::with_capacity(statuses.len());
    let (mut staged_count, mut unstaged_count, mut conflict_count, mut untracked_count) =
        (0, 0, 0, 0);

    for entry in statuses.iter() {
        let flags = entry.status();
        let conflicted = flags.contains(Status::CONFLICTED);

        // For renames libgit2 reports the new path in `new_file` and the old in
        // `old_file`; for everything else both sides carry the same path.
        let head_to_index = entry.head_to_index();
        let index_to_workdir = entry.index_to_workdir();

        let path = index_to_workdir
            .as_ref()
            .and_then(|d| d.new_file().path())
            .or_else(|| head_to_index.as_ref().and_then(|d| d.new_file().path()))
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .or_else(|| entry.path().ok().map(|p| p.to_owned()))
            .unwrap_or_default();

        let orig_path = head_to_index
            .as_ref()
            .and_then(|d| d.old_file().path())
            .or_else(|| index_to_workdir.as_ref().and_then(|d| d.old_file().path()))
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| *p != path);

        let staged = staged_delta(flags);
        let unstaged = unstaged_delta(flags);

        if conflicted {
            conflict_count += 1;
        } else {
            if staged.is_change() {
                staged_count += 1;
            }
            if unstaged.is_change() {
                unstaged_count += 1;
            }
        }
        if unstaged == Delta::Untracked {
            untracked_count += 1;
        }

        entries.push(StatusEntry {
            path,
            orig_path,
            staged,
            unstaged,
            conflicted,
        });
    }

    // Path order makes the list stable across refreshes; anything else makes
    // rows jump around under the cursor while the user is clicking.
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Arc::new(StatusSnapshot {
        entries,
        staged_count,
        unstaged_count,
        conflict_count,
        untracked_count,
    }))
}

fn staged_delta(f: Status) -> Delta {
    if f.contains(Status::CONFLICTED) {
        Delta::Conflicted
    } else if f.contains(Status::INDEX_NEW) {
        Delta::Added
    } else if f.contains(Status::INDEX_MODIFIED) {
        Delta::Modified
    } else if f.contains(Status::INDEX_DELETED) {
        Delta::Deleted
    } else if f.contains(Status::INDEX_RENAMED) {
        Delta::Renamed
    } else if f.contains(Status::INDEX_TYPECHANGE) {
        Delta::TypeChange
    } else {
        Delta::Unmodified
    }
}

fn unstaged_delta(f: Status) -> Delta {
    if f.contains(Status::CONFLICTED) {
        Delta::Conflicted
    } else if f.contains(Status::WT_NEW) {
        Delta::Untracked
    } else if f.contains(Status::WT_MODIFIED) {
        Delta::Modified
    } else if f.contains(Status::WT_DELETED) {
        Delta::Deleted
    } else if f.contains(Status::WT_RENAMED) {
        Delta::Renamed
    } else if f.contains(Status::WT_TYPECHANGE) {
        Delta::TypeChange
    } else if f.contains(Status::IGNORED) {
        Delta::Ignored
    } else {
        Delta::Unmodified
    }
}
