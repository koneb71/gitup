//! Filesystem watching.
//!
//! GitAhead's most persistent annoyance was showing state that was no longer
//! true: you'd commit in a terminal and the UI would keep displaying the old
//! working tree until you forced a refresh. This module removes that failure
//! mode by watching the worktree and the git directory and telling the app
//! precisely what changed, so it can reload only the affected views.

use crate::error::Result;
use crossbeam_channel::{Receiver, Sender};
use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, Debouncer, RecommendedCache};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Coalescing window. Long enough that a multi-file checkout arrives as one
/// change, short enough to feel immediate.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// What kind of change was observed. A single filesystem burst can set more
/// than one of these — `git commit` touches the index, refs, and logs at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    /// A tracked or untracked file in the working tree changed.
    pub worktree: bool,
    /// HEAD or a ref moved — branch switch, commit, fetch, reset.
    pub refs: bool,
    /// The index changed — something was staged or unstaged.
    pub index: bool,
}

impl Change {
    pub fn any(&self) -> bool {
        self.worktree || self.refs || self.index
    }

    fn merge(&mut self, other: Change) {
        self.worktree |= other.worktree;
        self.refs |= other.refs;
        self.index |= other.index;
    }
}

impl std::fmt::Debug for RepoWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The debouncer has no useful debug output and a great deal of it.
        f.debug_struct("RepoWatcher")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

pub struct RepoWatcher {
    // Held to keep the watcher thread alive; dropping it stops watching.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    rx: Receiver<Change>,
    root: PathBuf,
}

impl RepoWatcher {
    /// Watch a repository. `root` is the worktree, `git_dir` the `.git`
    /// directory — which is not always inside the worktree (worktrees,
    /// submodules, `GIT_DIR`).
    pub fn new(root: &Path, git_dir: &Path, ctx: egui::Context) -> Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Change>();
        let git_dir_owned = git_dir.to_path_buf();
        let ctx_for_handler = ctx.clone();

        let mut debouncer = new_debouncer(
            DEBOUNCE,
            None,
            move |result: notify_debouncer_full::DebounceEventResult| {
                let Ok(events) = result else { return };
                let mut change = Change::default();
                for event in events {
                    for path in &event.paths {
                        change.merge(classify(path, &git_dir_owned));
                    }
                }
                if change.any() {
                    let _ = tx.send(change);
                    // Wake the UI so it notices without waiting for input.
                    ctx_for_handler.request_repaint();
                }
            },
        )
        .map_err(|e| crate::error::Error::refused(format!("Couldn't watch for changes: {e}")))?;

        // The worktree, for file edits.
        debouncer
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| {
                crate::error::Error::refused(format!("Couldn't watch {}: {e}", root.display()))
            })?;

        // The git directory, for refs and the index. Watched separately because
        // it may live outside the worktree, and because a bare repository has
        // no worktree at all.
        if !git_dir.starts_with(root) {
            let _ = debouncer.watch(git_dir, RecursiveMode::Recursive);
        }

        Ok(Self {
            _debouncer: debouncer,
            rx,
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Drain every pending change into one. Called once per frame; returns
    /// `None` when nothing happened.
    pub fn poll(&self) -> Option<Change> {
        let mut merged = Change::default();
        while let Ok(change) = self.rx.try_recv() {
            merged.merge(change);
        }
        merged.any().then_some(merged)
    }
}

/// Decide what a changed path means.
///
/// Object and log writes are deliberately ignored: a fetch writes thousands of
/// files under `.git/objects` and none of them change what the user sees until
/// a ref moves, which is a separate event.
fn classify(path: &Path, git_dir: &Path) -> Change {
    let none = Change::default();

    // Lock files appear and vanish around every git operation and never carry
    // information of their own.
    if path.extension().is_some_and(|e| e == "lock") {
        return none;
    }

    if !path.starts_with(git_dir) {
        // Editors write backup and swap files constantly; they are not content.
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with('~') || name.starts_with(".#") || name.ends_with(".swp") {
            return none;
        }
        return Change {
            worktree: true,
            ..none
        };
    }

    let rel = path.strip_prefix(git_dir).unwrap_or(path);
    let rel_str = rel.to_string_lossy().replace('\\', "/");

    if rel_str.starts_with("objects/") || rel_str.starts_with("logs/") {
        return none;
    }
    if rel_str == "index" {
        return Change {
            index: true,
            ..none
        };
    }
    if rel_str == "HEAD"
        || rel_str.starts_with("refs/")
        || rel_str == "packed-refs"
        || rel_str.starts_with("worktrees/")
    {
        return Change { refs: true, ..none };
    }
    // MERGE_HEAD, REBASE_HEAD, CHERRY_PICK_HEAD and friends change what
    // operation is in progress, which is part of head state.
    if rel_str.ends_with("_HEAD") || rel_str.starts_with("rebase-") || rel_str == "MERGE_MSG" {
        return Change { refs: true, ..none };
    }

    none
}

/// Convenience for tests and for callers that only have a sender.
#[allow(dead_code)]
pub(crate) fn send_change(tx: &Sender<Change>, change: Change) {
    let _ = tx.send(change);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_and_logs_are_ignored() {
        let git = Path::new("/r/.git");
        assert!(!classify(Path::new("/r/.git/objects/ab/cdef"), git).any());
        assert!(!classify(Path::new("/r/.git/logs/HEAD"), git).any());
        assert!(!classify(Path::new("/r/.git/index.lock"), git).any());
    }

    #[test]
    fn refs_and_index_are_distinguished() {
        let git = Path::new("/r/.git");
        assert!(classify(Path::new("/r/.git/index"), git).index);
        assert!(classify(Path::new("/r/.git/HEAD"), git).refs);
        assert!(classify(Path::new("/r/.git/refs/heads/main"), git).refs);
        assert!(classify(Path::new("/r/.git/MERGE_HEAD"), git).refs);
    }

    #[test]
    fn worktree_edits_are_worktree_changes() {
        let git = Path::new("/r/.git");
        assert!(classify(Path::new("/r/src/main.rs"), git).worktree);
        assert!(!classify(Path::new("/r/src/main.rs~"), git).any());
    }
}
