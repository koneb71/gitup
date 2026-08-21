//! Opening repositories and reading their top-level state.

use crate::error::{Error, Result};
use git2::{Repository, RepositoryState};
use std::path::{Path, PathBuf};

/// Where HEAD points right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadKind {
    /// On a branch with at least one commit.
    Branch(String),
    /// On a branch that has no commits yet (fresh `git init`).
    Unborn(String),
    /// Detached at a specific commit.
    Detached,
}

#[derive(Debug, Clone, Default)]
pub struct UpstreamInfo {
    pub name: String,
    pub ahead: usize,
    pub behind: usize,
}

/// What the repository is in the middle of, if anything. Drives the banner
/// that tells the user "you are mid-rebase" instead of leaving them guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOp {
    None,
    Merge,
    Revert,
    CherryPick,
    Bisect,
    Rebase,
    ApplyMailbox,
}

impl PendingOp {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Merge => Some("Merging"),
            Self::Revert => Some("Reverting"),
            Self::CherryPick => Some("Cherry-picking"),
            Self::Bisect => Some("Bisecting"),
            Self::Rebase => Some("Rebasing"),
            Self::ApplyMailbox => Some("Applying patches"),
        }
    }

    fn from_state(state: RepositoryState) -> Self {
        match state {
            RepositoryState::Clean => Self::None,
            RepositoryState::Merge => Self::Merge,
            RepositoryState::Revert | RepositoryState::RevertSequence => Self::Revert,
            RepositoryState::CherryPick | RepositoryState::CherryPickSequence => Self::CherryPick,
            RepositoryState::Bisect => Self::Bisect,
            RepositoryState::Rebase
            | RepositoryState::RebaseInteractive
            | RepositoryState::RebaseMerge => Self::Rebase,
            RepositoryState::ApplyMailbox | RepositoryState::ApplyMailboxOrRebase => {
                Self::ApplyMailbox
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub kind: HeadKind,
    pub oid: Option<String>,
    pub short_id: String,
    pub summary: String,
    /// The upstream branch, when its remote-tracking ref can be resolved.
    ///
    /// `None` does not mean the branch has no upstream: a branch cloned but
    /// never fetched has `branch.<name>.remote` set with no local tracking ref
    /// yet, and `git pull` works fine in that state. Use
    /// [`Self::can_pull`] to decide whether pulling is possible.
    pub upstream: Option<UpstreamInfo>,
    /// Whether `branch.<name>.remote` is configured.
    pub tracking_configured: bool,
    pub pending: PendingOp,
    /// True when the worktree has no commits at all.
    pub is_empty: bool,
}

impl HeadInfo {
    /// Whether `git pull` has somewhere to pull from.
    ///
    /// Deliberately looser than `upstream.is_some()`: the tracking *ref* may be
    /// missing while the tracking *config* is present, and git resolves that
    /// itself by fetching.
    pub fn can_pull(&self) -> bool {
        self.tracking_configured || self.upstream.is_some()
    }

    /// The branch name, when HEAD is on one.
    pub fn branch_name(&self) -> Option<&str> {
        match &self.kind {
            HeadKind::Branch(name) => Some(name),
            _ => None,
        }
    }

    /// What to show in the branch chip.
    pub fn display_name(&self) -> String {
        match &self.kind {
            HeadKind::Branch(n) | HeadKind::Unborn(n) => n.clone(),
            HeadKind::Detached => format!("detached @ {}", self.short_id),
        }
    }
}

/// Identifies an open repository. The canonical path doubles as the key in
/// every per-repository cache, so it must be canonicalized exactly once, here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoKey(pub PathBuf);

impl RepoKey {
    pub fn name(&self) -> String {
        self.0
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.0.to_string_lossy().into_owned())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Find the repository containing `path` and return a stable key for it.
///
/// `Repository::discover` walks upward, so opening any file inside a checkout
/// finds the right repository — matching what `git` itself does.
pub fn discover(path: &Path) -> Result<RepoKey> {
    let repo = Repository::discover(path).map_err(|_| Error::NotARepository(path.to_path_buf()))?;
    Ok(key_for(&repo))
}

/// The key for an already-open repository: its worktree root, or the git dir
/// itself for a bare repository.
pub fn key_for(repo: &Repository) -> RepoKey {
    let raw = repo.workdir().unwrap_or_else(|| repo.path());
    let canonical = raw.canonicalize().unwrap_or_else(|_| raw.to_path_buf());
    RepoKey(canonical)
}

pub fn open(key: &RepoKey) -> Result<Repository> {
    Repository::discover(&key.0).map_err(|_| Error::NotARepository(key.0.clone()))
}

pub fn head_info(repo: &Repository) -> Result<HeadInfo> {
    let pending = PendingOp::from_state(repo.state());

    // An unborn HEAD is normal in a fresh repository, not an error worth
    // surfacing, so it gets its own branch rather than propagating.
    let head = match repo.head() {
        Ok(h) => h,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            let name = unborn_branch_name(repo);
            return Ok(HeadInfo {
                kind: HeadKind::Unborn(name),
                oid: None,
                short_id: String::new(),
                summary: "No commits yet".to_owned(),
                upstream: None,
                tracking_configured: false,
                pending,
                is_empty: true,
            });
        }
        Err(e) => return Err(e.into()),
    };

    let commit = head.peel_to_commit()?;
    let oid = commit.id();
    let summary = commit.summary().ok().flatten().unwrap_or("").to_owned();

    let kind = if repo.head_detached()? {
        HeadKind::Detached
    } else {
        HeadKind::Branch(head.shorthand().unwrap_or("HEAD").to_owned())
    };

    let upstream = match &kind {
        HeadKind::Branch(name) => upstream_info(repo, name, oid),
        _ => None,
    };
    let tracking_configured = match &kind {
        HeadKind::Branch(name) => has_tracking_config(repo, name),
        _ => false,
    };

    Ok(HeadInfo {
        kind,
        oid: Some(oid.to_string()),
        short_id: short_id(oid),
        summary,
        upstream,
        tracking_configured,
        pending,
        is_empty: false,
    })
}

/// Ahead/behind against the branch's configured upstream. Absent upstream is
/// the common case for local-only branches, so failures here are silent.
fn upstream_info(repo: &Repository, branch: &str, local_oid: git2::Oid) -> Option<UpstreamInfo> {
    let local = repo.find_branch(branch, git2::BranchType::Local).ok()?;
    let upstream = local.upstream().ok()?;
    let name = upstream.name().ok().flatten()?.to_owned();
    let upstream_oid = upstream.get().target()?;
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid).ok()?;
    Some(UpstreamInfo {
        name,
        ahead,
        behind,
    })
}

/// Whether the branch has a remote configured to pull from.
///
/// Read from config rather than by resolving the upstream ref, because the ref
/// is absent for a branch that has been configured but never fetched — and git
/// can pull in exactly that state.
fn has_tracking_config(repo: &Repository, branch: &str) -> bool {
    let Ok(config) = repo.config() else {
        return false;
    };
    config
        .get_string(&format!("branch.{branch}.remote"))
        .is_ok_and(|remote| !remote.trim().is_empty())
}

/// The branch an unborn HEAD *would* create — read from the HEAD symref.
fn unborn_branch_name(repo: &Repository) -> String {
    repo.find_reference("HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(|s| s.to_owned()))
        .and_then(|t| t.strip_prefix("refs/heads/").map(|s| s.to_owned()))
        .unwrap_or_else(|| "main".to_owned())
}

/// Git's own abbreviation length is 7 by default; matching it keeps hashes
/// recognizable against what the user sees in a terminal.
pub fn short_id(oid: git2::Oid) -> String {
    let s = oid.to_string();
    s.chars().take(7).collect()
}
