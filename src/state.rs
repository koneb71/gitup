//! What the UI reads each frame.
//!
//! Everything here is a snapshot produced by a worker. The UI never computes
//! Git state during a frame; it renders whatever arrived last.

use crate::git::{DiffModel, DiffTarget, GraphPage, HeadInfo, RefTree, RepoKey, StatusSnapshot};
use git2::Oid;
use std::sync::Arc;
use std::time::Instant;

/// A message shown to the user and then dismissed.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub created: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

impl Toast {
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Info)
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Success)
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Error)
    }

    fn new(text: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            text: text.into(),
            kind,
            created: Instant::now(),
        }
    }

    /// Errors stay until dismissed; everything else fades.
    pub fn lifetime_secs(&self) -> Option<f32> {
        match self.kind {
            ToastKind::Error => None,
            _ => Some(4.0),
        }
    }

    pub fn is_expired(&self) -> bool {
        match self.lifetime_secs() {
            Some(secs) => self.created.elapsed().as_secs_f32() > secs,
            None => false,
        }
    }
}

/// What the history list currently has selected.
///
/// Uncommitted work is a row in the same list as the commits rather than a
/// separate mode. The thing you are looking at is always "a set of changes",
/// and the working tree is just the newest one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Workdir,
    Commit(Oid),
}

/// What the centre of the window is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CenterView {
    /// The commit graph.
    #[default]
    History,
    /// Search results, replacing the graph until dismissed.
    Search,
    /// Line-by-line attribution for one file.
    Blame { path: String, at: Option<Oid> },
    /// Commits touching one path.
    FileHistory { path: String },
}

impl CenterView {
    pub fn is_history(&self) -> bool {
        matches!(self, Self::History)
    }

    /// Breadcrumb text for the bar that offers a way back.
    pub fn label(&self) -> Option<String> {
        match self {
            Self::History => None,
            Self::Search => Some("Search results".to_owned()),
            Self::Blame { path, at } => Some(match at {
                Some(oid) => format!("Blame · {path} @ {}", crate::git::repo::short_id(*oid)),
                None => format!("Blame · {path}"),
            }),
            Self::FileHistory { path } => Some(format!("History · {path}")),
        }
    }
}

/// The list the keyboard is working in.
///
/// Without this the arrow keys drove the commit graph whatever the user was
/// doing, so pressing Down while picking through changed files threw them back
/// into history and lost their place. Which list has the keys follows the last
/// one the user actually touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The commit graph.
    #[default]
    History,
    /// The changed-file list beside the diff.
    Files,
}

/// Which half of the working tree the detail pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkdirView {
    #[default]
    Unstaged,
    Staged,
    /// Only reachable while the index has conflicts.
    Conflicts,
}

impl WorkdirView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
            Self::Conflicts => "Conflicts",
        }
    }

    pub fn target(self) -> DiffTarget {
        match self {
            Self::Unstaged | Self::Conflicts => DiffTarget::Unstaged,
            Self::Staged => DiffTarget::Staged,
        }
    }
}

/// How many commits to load on first open.
///
/// Set high deliberately. The revwalk is eagerly resolved by libgit2 (see
/// [`crate::git::graph::build`]), so a smaller limit would not make the first
/// load faster — it would only guarantee a second full traversal as soon as the
/// user scrolled. Twenty-five thousand commits covers almost every repository
/// in one pass and costs a few megabytes.
pub const INITIAL_GRAPH_LIMIT: usize = 25_000;

/// One open repository: its data, and everything about how it is being looked
/// at.
///
/// A tab is a session. Keeping the view state here rather than on the app is
/// what makes switching tabs feel like returning to where you were — the
/// selected commit, the scroll position of the blame you were reading, the
/// half-written commit message all belong to the repository, not to the window.
#[derive(Debug, Default)]
pub struct Session {
    /// Identifies this session for the lifetime of the process.
    ///
    /// A repository is only known by its path once it has been opened, so a
    /// job that is still *finding* it has no key to be routed by. The id gives
    /// the answer a destination from the moment it is asked for — without it,
    /// opening a second repository while the first is still loading delivers
    /// the first one's result into the wrong tab.
    pub id: u64,
    pub repo: RepoState,
    /// The git directory, which is not always inside the worktree.
    pub git_dir: Option<std::path::PathBuf>,
    /// Watches this repository for outside changes. Absent when watching is
    /// off, or when the watcher could not be established.
    pub watcher: Option<crate::watch::RepoWatcher>,

    /// What the centre pane is showing.
    pub center: CenterView,
    pub workdir_view: WorkdirView,
    /// Which list the arrow keys move through.
    pub focus: Focus,

    /// A commit the history list should scroll to on the next frame.
    pub pending_scroll: Option<Oid>,

    /// Diff lines selected, as `(hunk index, line index)`.
    pub line_selection: std::collections::BTreeSet<(usize, usize)>,
    /// Anchor for shift-click range selection.
    pub selection_anchor: Option<(usize, usize)>,

    pub commit_message: String,
    /// True while `commit_message` is exactly what the app drafted.
    ///
    /// Cleared the moment the user types. Drafting again is then free to
    /// replace the box, because the only thing it can overwrite is a previous
    /// draft — never something a person wrote.
    pub message_is_draft: bool,
    pub amending: bool,

    pub blame: Option<Arc<crate::git::blame::BlameResult>>,
    pub search_results: Option<Arc<crate::git::search::SearchResults>>,
    pub search_query: String,
    pub search_kind: crate::git::search::SearchKind,

    pub conflicts: Option<Arc<crate::git::conflict::Conflicts>>,
    pub conflict_file: Option<String>,
    pub conflict_buffer: String,
    pub conflict_editing: bool,

    pub submodules: Option<Arc<crate::git::submodule::Submodules>>,
}

impl Session {
    pub fn is_open(&self) -> bool {
        self.repo.is_open()
    }

    /// The name shown on the tab.
    pub fn title(&self) -> String {
        self.repo.name()
    }

    /// A short suffix distinguishing this tab from another of the same name.
    ///
    /// Two checkouts of the same project are common enough — a worktree, a
    /// second clone — that identical tab labels would be a real problem.
    pub fn disambiguator(&self) -> Option<String> {
        let path = self.repo.key.as_ref()?.path();
        path.parent()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }

    /// Whether anything about this repository wants the user's attention.
    pub fn badge(&self) -> Option<SessionBadge> {
        let status = self.repo.status.as_ref()?;
        if status.conflict_count > 0 {
            return Some(SessionBadge::Conflicts(status.conflict_count));
        }
        let pending = self.repo.head.as_ref().map(|h| h.pending);
        if pending.is_some_and(|p| p != crate::git::PendingOp::None) {
            return Some(SessionBadge::InProgress);
        }
        (!status.is_clean()).then(|| SessionBadge::Changes(status.entries.len()))
    }

    /// Choose a selection when the user hasn't made one.
    ///
    /// Runs for background tabs too, so switching to one lands somewhere
    /// sensible rather than on a blank detail pane.
    pub fn ensure_selection(&mut self) {
        if self.repo.selection_pinned {
            return;
        }
        if let Some(preferred) = self.repo.default_selection() {
            self.repo.selection = Some(preferred);
        }
    }

    /// Keep a file selected in the diff pane, so it is never blank when there
    /// is something to show.
    pub fn ensure_active_file(&mut self) {
        let still_present = self
            .repo
            .active_diff(self.workdir_view)
            .zip(self.repo.active_file.as_ref())
            .is_some_and(|(model, path)| model.find(path).is_some());
        if still_present {
            return;
        }
        self.repo.active_file = self
            .repo
            .active_diff(self.workdir_view)
            .and_then(|m| m.files.first())
            .map(|f| f.path.clone());
    }

    /// Forget everything derived, keeping only which repository this is.
    pub fn invalidate(&mut self) {
        self.repo.invalidate();
        self.blame = None;
        self.conflicts = None;
        self.conflict_file = None;
        self.conflict_buffer.clear();
        self.submodules = None;
        self.line_selection.clear();
    }
}

/// What a tab shows beside its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBadge {
    Changes(usize),
    Conflicts(usize),
    InProgress,
}

/// Everything known about the currently open repository.
#[derive(Debug, Default)]
pub struct RepoState {
    pub key: Option<RepoKey>,
    pub head: Option<HeadInfo>,
    pub status: Option<Arc<StatusSnapshot>>,
    pub graph: Option<Arc<GraphPage>>,
    pub refs: Option<Arc<RefTree>>,
    /// The limit the current graph was built with.
    pub graph_limit: usize,
    pub selection: Option<Selection>,
    /// True once the user has chosen a row themselves, after which the app
    /// stops revising the selection as data arrives.
    pub selection_pinned: bool,
    /// Diff of the selected commit, keyed by the commit it belongs to so a
    /// stale result can be recognized and ignored.
    pub commit_diff: Option<(Oid, Arc<DiffModel>)>,
    /// Index against HEAD.
    pub staged: Option<Arc<DiffModel>>,
    /// Working tree against the index.
    pub unstaged: Option<Arc<DiffModel>>,
    /// Path of the file whose contents the diff pane is showing.
    pub active_file: Option<String>,
    /// Set while the first load is still running, so the UI can show a
    /// skeleton rather than an empty repository.
    pub loading: bool,
}

impl RepoState {
    pub fn is_open(&self) -> bool {
        self.key.is_some()
    }

    pub fn name(&self) -> String {
        self.key
            .as_ref()
            .map(|k| k.name())
            .unwrap_or_else(|| "No repository".to_owned())
    }

    /// Clear per-repository data without forgetting which repository it is.
    pub fn invalidate(&mut self) {
        self.status = None;
        self.graph = None;
        self.refs = None;
        self.commit_diff = None;
        self.staged = None;
        self.unstaged = None;
    }

    /// The diff the pane should render for the current selection, if loaded.
    pub fn active_diff(&self, view: WorkdirView) -> Option<&Arc<DiffModel>> {
        match self.selection? {
            Selection::Commit(oid) => self
                .commit_diff
                .as_ref()
                .filter(|(id, _)| *id == oid)
                .map(|(_, m)| m),
            Selection::Workdir => match view {
                WorkdirView::Unstaged | WorkdirView::Conflicts => self.unstaged.as_ref(),
                WorkdirView::Staged => self.staged.as_ref(),
            },
        }
    }

    /// Diff targets the current selection needs loaded.
    pub fn required_diffs(&self) -> Vec<DiffTarget> {
        match self.selection {
            Some(Selection::Commit(oid)) => {
                if self.commit_diff.as_ref().is_some_and(|(id, _)| *id == oid) {
                    Vec::new()
                } else {
                    vec![DiffTarget::Commit(oid)]
                }
            }
            Some(Selection::Workdir) => {
                let mut v = Vec::new();
                if self.unstaged.is_none() {
                    v.push(DiffTarget::Unstaged);
                }
                if self.staged.is_none() {
                    v.push(DiffTarget::Staged);
                }
                v
            }
            None => Vec::new(),
        }
    }

    pub fn has_uncommitted(&self) -> bool {
        self.status.as_ref().is_some_and(|s| !s.is_clean())
    }

    /// The commit currently selected, if the selection is a commit.
    pub fn selected_commit(&self) -> Option<Oid> {
        match self.selection {
            Some(Selection::Commit(oid)) => Some(oid),
            _ => None,
        }
    }

    /// Pick a sensible selection when none exists yet: uncommitted work if
    /// there is any, otherwise the newest commit.
    pub fn default_selection(&self) -> Option<Selection> {
        if self.has_uncommitted() {
            return Some(Selection::Workdir);
        }
        self.graph
            .as_ref()
            .and_then(|g| g.rows.first())
            .map(|r| Selection::Commit(r.commit.id))
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }
}

/// Index arithmetic for the tab bar.
///
/// The current session is held apart from the others so that borrowing it never
/// conflicts with borrowing the app. The cost is that "which tab is where" has
/// to be computed rather than read off a list — and getting it wrong reorders
/// the user's tabs behind their back, which is the kind of bug that is obvious
/// in a screenshot and invisible in a type signature. Hence: pure functions,
/// and tests.
pub mod tab_index {
    /// Where the tab at `position` lives in the background list.
    ///
    /// `None` when `position` is the current tab, which is not in that list.
    pub fn background_of(position: usize, current_slot: usize) -> Option<usize> {
        match position.cmp(&current_slot) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(position),
            std::cmp::Ordering::Greater => Some(position - 1),
        }
    }

    /// Where the outgoing current session is reinserted when `position` becomes
    /// current.
    ///
    /// Computed against the list *after* the incoming session has been removed
    /// from it, which is why it is not simply `current_slot`.
    pub fn reinsert_at(current_slot: usize, position: usize) -> usize {
        if current_slot < position {
            current_slot
        } else {
            current_slot.saturating_sub(1)
        }
    }

    /// Which background entry takes over when the current tab at `position`
    /// closes, given how many background tabs remain.
    pub fn successor(position: usize, background_len: usize) -> Option<usize> {
        if background_len == 0 {
            return None;
        }
        // The tab that was just after this one, or the last if there was none.
        Some(position.min(background_len - 1))
    }
}

#[cfg(test)]
mod tab_index_tests {
    use super::tab_index::*;

    /// Rebuild the visible order from the split representation, the same way
    /// the app does, so the tests check the thing that actually matters.
    fn order(current_slot: usize, background: &[&str], current: &str) -> Vec<String> {
        let total = background.len() + 1;
        let mut out = Vec::with_capacity(total);
        let mut rest = background.iter();
        for slot in 0..total {
            if slot == current_slot {
                out.push(current.to_owned());
            } else if let Some(name) = rest.next() {
                out.push((*name).to_owned());
            }
        }
        out
    }

    /// Apply an activation and return the new (current, background, slot).
    fn activate(
        current: &str,
        background: Vec<&str>,
        current_slot: usize,
        position: usize,
    ) -> (String, Vec<String>, usize) {
        let mut background: Vec<String> = background.into_iter().map(str::to_owned).collect();
        let Some(index) = background_of(position, current_slot) else {
            return (current.to_owned(), background, current_slot);
        };
        let incoming = background.remove(index);
        let outgoing = current.to_owned();
        let at = reinsert_at(current_slot, position);
        background.insert(at.min(background.len()), outgoing);
        (incoming, background, position)
    }

    #[test]
    fn positions_map_to_background_indices() {
        // Current at slot 1 of three tabs: background holds slots 0 and 2.
        assert_eq!(background_of(0, 1), Some(0));
        assert_eq!(background_of(1, 1), None);
        assert_eq!(background_of(2, 1), Some(1));
    }

    #[test]
    fn switching_tabs_leaves_the_order_alone() {
        // Three tabs, a b c, with c showing.
        let (current, background, slot) = activate("c", vec!["a", "b"], 2, 0);
        assert_eq!(current, "a");
        assert_eq!(slot, 0);
        assert_eq!(
            order(
                slot,
                &background.iter().map(String::as_str).collect::<Vec<_>>(),
                &current
            ),
            vec!["a", "b", "c"],
            "switching must not reorder anything"
        );
    }

    #[test]
    fn switching_forward_also_leaves_the_order_alone() {
        let (current, background, slot) = activate("a", vec!["b", "c"], 0, 2);
        assert_eq!(current, "c");
        assert_eq!(slot, 2);
        assert_eq!(
            order(
                slot,
                &background.iter().map(String::as_str).collect::<Vec<_>>(),
                &current
            ),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn every_switch_from_every_position_preserves_the_order() {
        let names = ["a", "b", "c", "d"];
        for start in 0..names.len() {
            for target in 0..names.len() {
                let current = names[start];
                let background: Vec<&str> =
                    names.iter().copied().filter(|n| *n != current).collect();
                let (new_current, new_background, slot) =
                    activate(current, background, start, target);
                let seen = order(
                    slot,
                    &new_background
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    &new_current,
                );
                assert_eq!(seen, names, "switching {start} -> {target} reordered tabs");
                assert_eq!(
                    new_current, names[target],
                    "switching {start} -> {target} showed the wrong tab"
                );
            }
        }
    }

    #[test]
    fn closing_promotes_the_next_tab() {
        // Closing the middle of three promotes what was after it.
        assert_eq!(successor(1, 2), Some(1));
        // Closing the last promotes what was before it.
        assert_eq!(successor(2, 2), Some(1));
        // Closing the first promotes what was after it.
        assert_eq!(successor(0, 2), Some(0));
        // Nothing left.
        assert_eq!(successor(0, 0), None);
    }
}
