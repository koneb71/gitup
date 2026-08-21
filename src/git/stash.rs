//! The stash.
//!
//! libgit2's stash API needs a mutable repository handle, which is why these
//! take `&mut Repository` while the rest of the read path does not.

use crate::error::{Error, Result};
use git2::{Repository, StashApplyOptions, StashFlags};

/// Save the working tree to the stash.
pub fn save(
    repo: &mut Repository,
    message: Option<&str>,
    include_untracked: bool,
    keep_index: bool,
) -> Result<git2::Oid> {
    let signature = super::commit::signature(repo)?;
    let mut flags = StashFlags::DEFAULT;
    if include_untracked {
        flags |= StashFlags::INCLUDE_UNTRACKED;
    }
    if keep_index {
        flags |= StashFlags::KEEP_INDEX;
    }

    repo.stash_save2(&signature, message, Some(flags))
        .map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                Error::refused("There's nothing to stash")
            } else {
                Error::Git(e)
            }
        })
}

/// Apply a stash entry, leaving it in the stash list.
pub fn apply(repo: &mut Repository, index: usize) -> Result<()> {
    let mut options = StashApplyOptions::new();
    repo.stash_apply(index, Some(&mut options))
        .map_err(conflict_hint)
}

/// Apply a stash entry and remove it.
pub fn pop(repo: &mut Repository, index: usize) -> Result<()> {
    let mut options = StashApplyOptions::new();
    repo.stash_pop(index, Some(&mut options))
        .map_err(conflict_hint)
}

/// Delete a stash entry without applying it.
pub fn drop(repo: &mut Repository, index: usize) -> Result<()> {
    repo.stash_drop(index)?;
    Ok(())
}

/// Applying a stash onto a changed tree can conflict, and libgit2's message for
/// it doesn't explain what to do next.
fn conflict_hint(e: git2::Error) -> Error {
    if e.code() == git2::ErrorCode::MergeConflict {
        Error::refused(
            "Applying the stash ran into conflicts. \
             Resolve them, or commit your current changes and try again.",
        )
    } else if e.code() == git2::ErrorCode::NotFound {
        Error::refused("That stash entry no longer exists")
    } else {
        Error::Git(e)
    }
}
