//! Branch, tag, and checkout operations. All local; nothing here touches a network.

use crate::error::{Error, Result};
use git2::{build::CheckoutBuilder, BranchType, Repository};

/// Check out a local branch.
///
/// The checkout is "safe": libgit2 refuses rather than overwriting uncommitted
/// work that would conflict. Local changes that *don't* conflict come along,
/// which is what `git checkout` does and what people expect.
pub fn checkout_branch(repo: &Repository, name: &str) -> Result<()> {
    let refname = format!("refs/heads/{name}");
    let object = repo
        .revparse_single(&refname)
        .map_err(|_| Error::refused(format!("No branch named ‘{name}’")))?;

    let mut options = CheckoutBuilder::new();
    options.safe();
    repo.checkout_tree(&object, Some(&mut options))
        .map_err(|e| {
            Error::refused(format!(
                "Couldn't switch to ‘{name}’: {}. Commit or stash your changes first.",
                e.message()
            ))
        })?;
    repo.set_head(&refname)?;
    Ok(())
}

/// Check out a remote-tracking branch by creating a local branch that follows it.
///
/// Checking out `origin/feature` directly would leave HEAD detached, which is
/// almost never what someone double-clicking a remote branch wants.
pub fn checkout_remote(repo: &Repository, remote_ref: &str) -> Result<String> {
    let local_name = remote_ref
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(remote_ref)
        .to_owned();

    // If a local branch of that name already exists, switch to it instead of
    // failing — it is almost certainly the one they meant.
    if repo.find_branch(&local_name, BranchType::Local).is_ok() {
        checkout_branch(repo, &local_name)?;
        return Ok(local_name);
    }

    let remote_branch = repo
        .find_branch(remote_ref, BranchType::Remote)
        .map_err(|_| Error::refused(format!("No remote branch ‘{remote_ref}’")))?;
    let commit = remote_branch.get().peel_to_commit()?;

    let mut branch = repo.branch(&local_name, &commit, false)?;
    // Tracking needs a configured remote. A remote-tracking ref can outlive its
    // remote — after `git remote remove` — and in that case the branch is still
    // worth creating; it just won't track anything.
    if let Err(e) = branch.set_upstream(Some(remote_ref)) {
        tracing::warn!("couldn't track {remote_ref}: {}", e.message());
    }
    checkout_branch(repo, &local_name)?;
    Ok(local_name)
}

/// Detach HEAD at a specific commit.
pub fn checkout_commit(repo: &Repository, oid: git2::Oid) -> Result<()> {
    let object = repo.find_object(oid, None)?;
    let mut options = CheckoutBuilder::new();
    options.safe();
    repo.checkout_tree(&object, Some(&mut options))?;
    repo.set_head_detached(oid)?;
    Ok(())
}

/// Create a branch at `start_point` (a revision string), optionally switching to it.
pub fn create(
    repo: &Repository,
    name: &str,
    start_point: Option<&str>,
    checkout: bool,
) -> Result<()> {
    validate_name(name)?;
    if repo.find_branch(name, BranchType::Local).is_ok() {
        return Err(Error::refused(format!(
            "A branch named ‘{name}’ already exists"
        )));
    }

    let commit = match start_point {
        Some(rev) => repo
            .revparse_single(rev)
            .map_err(|_| Error::refused(format!("Couldn't resolve ‘{rev}’")))?
            .peel_to_commit()?,
        None => repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|_| Error::refused("There are no commits to branch from yet"))?,
    };

    repo.branch(name, &commit, false)?;
    if checkout {
        checkout_branch(repo, name)?;
    }
    Ok(())
}

pub fn delete(repo: &Repository, name: &str, force: bool) -> Result<()> {
    let mut branch = repo
        .find_branch(name, BranchType::Local)
        .map_err(|_| Error::refused(format!("No branch named ‘{name}’")))?;

    if branch.is_head() {
        return Err(Error::refused(
            "You can't delete the branch you're on — switch to another first",
        ));
    }

    // Refuse to drop work that exists nowhere else, unless explicitly forced.
    if !force && !is_merged(repo, &branch)? {
        return Err(Error::refused(format!(
            "‘{name}’ has commits that aren't merged anywhere. Delete anyway to discard them."
        )));
    }

    branch.delete()?;
    Ok(())
}

/// Whether the branch's tip is reachable from HEAD or from its upstream.
fn is_merged(repo: &Repository, branch: &git2::Branch<'_>) -> Result<bool> {
    let Some(tip) = branch.get().target() else {
        return Ok(true);
    };
    if let Ok(head) = repo.head().and_then(|h| h.peel_to_commit()) {
        if repo.graph_descendant_of(head.id(), tip).unwrap_or(false) || head.id() == tip {
            return Ok(true);
        }
    }
    if let Ok(upstream) = branch.upstream() {
        if let Some(oid) = upstream.get().target() {
            if repo.graph_descendant_of(oid, tip).unwrap_or(false) || oid == tip {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn rename(repo: &Repository, old: &str, new: &str, force: bool) -> Result<()> {
    validate_name(new)?;
    let mut branch = repo
        .find_branch(old, BranchType::Local)
        .map_err(|_| Error::refused(format!("No branch named ‘{old}’")))?;
    branch.rename(new, force)?;
    Ok(())
}

pub fn set_upstream(repo: &Repository, branch: &str, upstream: Option<&str>) -> Result<()> {
    let mut b = repo
        .find_branch(branch, BranchType::Local)
        .map_err(|_| Error::refused(format!("No branch named ‘{branch}’")))?;
    b.set_upstream(upstream)?;
    Ok(())
}

pub fn create_tag(
    repo: &Repository,
    name: &str,
    target: git2::Oid,
    message: Option<&str>,
) -> Result<()> {
    validate_name(name)?;
    let object = repo.find_object(target, None)?;
    match message {
        // An annotated tag records who made it and when; a lightweight one is
        // just a pointer. Both are legitimate, so the caller decides.
        Some(text) if !text.trim().is_empty() => {
            let signature = super::commit::signature(repo)?;
            repo.tag(name, &object, &signature, text, false)?;
        }
        _ => {
            repo.tag_lightweight(name, &object, false)?;
        }
    }
    Ok(())
}

pub fn delete_tag(repo: &Repository, name: &str) -> Result<()> {
    repo.tag_delete(name)
        .map_err(|_| Error::refused(format!("No tag named ‘{name}’")))?;
    Ok(())
}

/// Reject names git itself would reject, with a message that says which rule.
///
/// `git check-ref-format` has a longer list; these are the ones a person
/// actually types by accident.
pub fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::refused("A name is required"));
    }
    if name.starts_with('-') {
        return Err(Error::refused("Names can't start with a dash"));
    }
    if name.starts_with('/') || name.ends_with('/') {
        return Err(Error::refused("Names can't start or end with a slash"));
    }
    if name.ends_with(".lock") {
        return Err(Error::refused("Names can't end with ‘.lock’"));
    }
    if name.ends_with('.') {
        return Err(Error::refused("Names can't end with a dot"));
    }
    if name.contains("..") {
        return Err(Error::refused("Names can't contain ‘..’"));
    }
    if name.contains("//") {
        return Err(Error::refused("Names can't contain an empty path segment"));
    }
    if name.contains("@{") {
        return Err(Error::refused("Names can't contain ‘@{’"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\') || c.is_control())
    {
        return Err(Error::refused(format!(
            "Names can't contain ‘{}’",
            if bad == ' ' {
                "space".chars().next().unwrap()
            } else {
                bad
            }
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn ordinary_names_are_accepted() {
        for name in ["main", "feature/parser", "release-1.2", "user/fix_bug"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn names_git_rejects_are_rejected_here_too() {
        for name in [
            "",
            "-x",
            "/leading",
            "trailing/",
            "with space",
            "a..b",
            "a//b",
            "ends.",
            "x.lock",
            "ca^ret",
            "co:lon",
            "que?st",
            "star*",
            "brack[et",
            "at@{x",
        ] {
            assert!(validate_name(name).is_err(), "{name:?} should be rejected");
        }
    }

    #[test]
    fn rejection_messages_say_which_rule() {
        let message = validate_name("a..b").unwrap_err().user_message();
        assert!(message.contains(".."), "got {message:?}");
    }
}
