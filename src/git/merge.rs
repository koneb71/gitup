//! Merge, cherry-pick, revert, and reset.
//!
//! These all leave the repository in a state git itself recognizes — an index
//! with conflicts, a `MERGE_HEAD`, a `CHERRY_PICK_HEAD` — rather than inventing
//! private bookkeeping. That means a user can always drop to a terminal and
//! finish or abandon the operation with plain git, which is a property worth
//! protecting: a GUI that strands you in a state only it understands is worse
//! than no GUI.

use crate::error::{Error, Result};
use git2::{build::CheckoutBuilder, AnnotatedCommit, MergeOptions, Oid, Repository, ResetType};

/// What a merge would do, decided before doing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Nothing to do.
    UpToDate,
    /// HEAD simply moves forward; no merge commit.
    FastForward,
    /// A real merge, which may or may not conflict.
    Merged { conflicts: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetKind {
    /// Move the branch; keep the index and working tree.
    Soft,
    /// Move the branch and reset the index; keep the working tree.
    Mixed,
    /// Move everything, discarding uncommitted work.
    Hard,
}

impl ResetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Mixed => "Mixed",
            Self::Hard => "Hard",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Soft => "Move the branch; leave your staged and unstaged work alone",
            Self::Mixed => "Move the branch and unstage everything; keep your files",
            Self::Hard => "Move the branch and discard all uncommitted work",
        }
    }

    fn to_git(self) -> ResetType {
        match self {
            Self::Soft => ResetType::Soft,
            Self::Mixed => ResetType::Mixed,
            Self::Hard => ResetType::Hard,
        }
    }
}

/// Refuse to start an operation on top of another one.
fn require_clean_state(repo: &Repository, what: &str) -> Result<()> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err(Error::refused(format!(
            "Finish or abort the operation in progress before starting a {what}"
        )));
    }
    Ok(())
}

fn annotated<'a>(repo: &'a Repository, rev: &str) -> Result<AnnotatedCommit<'a>> {
    let object = repo
        .revparse_single(rev)
        .map_err(|_| Error::refused(format!("Couldn't resolve ‘{rev}’")))?;
    let commit = object.peel_to_commit()?;
    Ok(repo.find_annotated_commit(commit.id())?)
}

/// Merge `rev` into the current branch.
pub fn merge(repo: &Repository, rev: &str) -> Result<MergeOutcome> {
    require_clean_state(repo, "merge")?;
    let their = annotated(repo, rev)?;
    let (analysis, _) = repo.merge_analysis(&[&their])?;

    if analysis.is_up_to_date() {
        return Ok(MergeOutcome::UpToDate);
    }

    if analysis.is_fast_forward() {
        // Fast-forward: move the branch and check out, without a merge commit.
        let target = repo.find_commit(their.id())?;
        let mut options = CheckoutBuilder::new();
        options.safe();
        repo.checkout_tree(target.as_object(), Some(&mut options))?;
        let mut head = repo.head()?;
        head.set_target(their.id(), &format!("merge {rev}: fast-forward"))?;
        return Ok(MergeOutcome::FastForward);
    }

    if !analysis.is_normal() {
        return Err(Error::refused(format!(
            "‘{rev}’ has no common history with this branch"
        )));
    }

    let mut merge_options = MergeOptions::new();
    // Try harder to resolve automatically before declaring a conflict.
    merge_options.find_renames(true);
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.merge(&[&their], Some(&mut merge_options), Some(&mut checkout))?;

    let conflicts = repo.index()?.conflicts()?.count();
    Ok(MergeOutcome::Merged { conflicts })
}

/// Which parent of a merge commit counts as "the branch this was merged into".
///
/// A merge has no single set of changes — it has one per parent — so applying
/// or undoing one only means something relative to a chosen side. git calls
/// that side the mainline and numbers parents from 1. For an ordinary commit
/// this is irrelevant and must be left at 0.
pub type Mainline = u32;

/// Whether a commit needs a mainline chosen before it can be picked or reverted.
pub fn needs_mainline(repo: &Repository, oid: Oid) -> Result<bool> {
    Ok(repo.find_commit(oid)?.parent_count() > 1)
}

/// The parents of a commit, as `(number, short id, summary)`.
///
/// The number is git's own 1-based mainline index, so it can be handed straight
/// back to [`cherry_pick`] or [`revert`].
pub fn parents_of(repo: &Repository, oid: Oid) -> Result<Vec<(Mainline, String, String)>> {
    let commit = repo.find_commit(oid)?;
    Ok(commit
        .parents()
        .enumerate()
        .map(|(index, parent)| {
            (
                index as Mainline + 1,
                super::repo::short_id(parent.id()),
                parent.summary().ok().flatten().unwrap_or("").to_owned(),
            )
        })
        .collect())
}

fn check_mainline(repo: &Repository, oid: Oid, mainline: Mainline, verb: &str) -> Result<()> {
    let parents = repo.find_commit(oid)?.parent_count() as u32;
    if parents > 1 && mainline == 0 {
        return Err(Error::refused(format!(
            "This is a merge commit — choose which parent to {verb} against"
        )));
    }
    if parents <= 1 && mainline != 0 {
        return Err(Error::refused(
            "Only a merge commit has a mainline to choose",
        ));
    }
    if mainline > parents {
        return Err(Error::refused(format!(
            "This commit has {parents} parents, so there is no parent {mainline}"
        )));
    }
    Ok(())
}

/// Apply a commit's changes on top of HEAD.
///
/// `mainline` is 0 for an ordinary commit and the 1-based parent number for a
/// merge; see [`Mainline`].
pub fn cherry_pick(repo: &Repository, oid: Oid, mainline: Mainline) -> Result<usize> {
    require_clean_state(repo, "cherry-pick")?;
    check_mainline(repo, oid, mainline, "cherry-pick")?;

    let commit = repo.find_commit(oid)?;
    let mut options = git2::CherrypickOptions::new();
    options.mainline(mainline);
    repo.cherrypick(&commit, Some(&mut options))?;
    Ok(repo.index()?.conflicts()?.count())
}

/// Create a commit that undoes another.
pub fn revert(repo: &Repository, oid: Oid, mainline: Mainline) -> Result<usize> {
    require_clean_state(repo, "revert")?;
    check_mainline(repo, oid, mainline, "revert")?;

    let commit = repo.find_commit(oid)?;
    let mut options = git2::RevertOptions::new();
    options.mainline(mainline);
    repo.revert(&commit, Some(&mut options))?;
    Ok(repo.index()?.conflicts()?.count())
}

/// Move the current branch to `oid`.
pub fn reset(repo: &Repository, oid: Oid, kind: ResetKind) -> Result<()> {
    let object = repo.find_object(oid, None)?;
    let mut checkout = CheckoutBuilder::new();
    if kind == ResetKind::Hard {
        checkout.force();
    }
    repo.reset(&object, kind.to_git(), Some(&mut checkout))?;
    Ok(())
}

/// Abandon whatever operation is in progress and return to HEAD.
///
/// This is deliberately blunt: `git merge --abort` and friends all amount to
/// "put everything back the way it was", and a half-aborted merge is worse than
/// either outcome.
pub fn abort(repo: &Repository) -> Result<()> {
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|_| Error::refused("There's no HEAD to return to"))?;

    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.reset(head.as_object(), ResetType::Hard, Some(&mut checkout))?;
    repo.cleanup_state()?;
    Ok(())
}

/// A one-line description of what committing now would finish.
pub fn in_progress_message(repo: &Repository) -> Option<String> {
    use git2::RepositoryState::*;
    match repo.state() {
        Clean => None,
        Merge => Some(read_message(repo).unwrap_or_else(|| "Merge".to_owned())),
        Revert | RevertSequence => Some("Revert".to_owned()),
        CherryPick | CherryPickSequence => {
            Some(read_message(repo).unwrap_or_else(|| "Cherry-pick".to_owned()))
        }
        _ => None,
    }
}

/// The message git prepared for the operation in progress, if any.
fn read_message(repo: &Repository) -> Option<String> {
    std::fs::read_to_string(repo.path().join("MERGE_MSG"))
        .ok()
        .map(|text| {
            text.lines()
                .filter(|l| !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned()
        })
        .filter(|t| !t.is_empty())
}
