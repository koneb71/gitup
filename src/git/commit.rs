//! Creating commits.

use crate::error::{Error, Result};
use git2::{Oid, Repository, Signature};

/// Clean a commit message the way git does before committing.
///
/// Comment lines are stripped, trailing whitespace on each line is removed, and
/// leading and trailing blank lines go. Doing this here rather than at the
/// input field means what you see stored is what git would have stored.
pub fn clean_message(raw: &str, comment_char: char) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if line.starts_with(comment_char) {
            continue;
        }
        lines.push(line.trim_end());
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// The identity to commit as.
///
/// An unset `user.name` or `user.email` is the single most common first-run
/// failure, and libgit2's own message for it is not actionable, so it is
/// replaced with one that says what to run.
pub fn signature(repo: &Repository) -> Result<Signature<'static>> {
    repo.signature().map_err(|_| {
        Error::refused(
            "Set your name and email before committing. \
             Settings → Identity, or from a terminal:\n\
             git config --global user.name \"Your Name\"\n\
             git config --global user.email \"you@example.com\"",
        )
    })
}

/// Parents an ordinary commit should have, including any merge in progress.
///
/// A commit made during a merge must record `MERGE_HEAD` as a second parent.
/// Missing that produces a commit that silently discards the merge — the merged
/// content is there, but history says the branches never joined.
fn parents(repo: &Repository) -> Result<Vec<git2::Commit<'_>>> {
    let mut out = Vec::new();
    if let Ok(head) = repo.head().and_then(|h| h.peel_to_commit()) {
        out.push(head);
    }
    // `MERGE_HEAD` is read from disk rather than through
    // `Repository::mergehead_foreach`, which needs a mutable handle that this
    // whole call path would otherwise have to carry. The file is one oid per
    // line — several for an octopus merge.
    for oid in merge_heads(repo) {
        if let Ok(commit) = repo.find_commit(oid) {
            out.push(commit);
        }
    }
    Ok(out)
}

/// Commits listed in `MERGE_HEAD`, or empty when no merge is in progress.
pub fn merge_heads(repo: &Repository) -> Vec<Oid> {
    let path = repo.path().join("MERGE_HEAD");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| Oid::from_str(line.trim()).ok())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    Normal,
    /// Replace HEAD instead of adding to it.
    Amend,
}

pub fn commit(repo: &Repository, message: &str, mode: CommitMode) -> Result<Oid> {
    let message = clean_message(message, '#');
    if message.trim().is_empty() {
        return Err(Error::refused("A commit needs a message"));
    }

    let index_has_conflicts = repo.index()?.has_conflicts();
    if index_has_conflicts {
        return Err(Error::refused(
            "There are unresolved conflicts — resolve them before committing",
        ));
    }

    let signature = signature(repo)?;
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let oid = match mode {
        CommitMode::Amend => {
            let head = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|_| Error::refused("There is no commit to amend"))?;
            // Author stays as it was — amending is editing your own commit, not
            // claiming authorship of it.
            head.amend(
                Some("HEAD"),
                None,
                Some(&signature),
                None,
                Some(&message),
                Some(&tree),
            )?
        }
        CommitMode::Normal => {
            let parents = parents(repo)?;
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &signature, &signature, &message, &tree, &refs)?
        }
    };

    // Clear MERGE_HEAD and friends so the repository stops reporting an
    // operation in progress.
    let _ = repo.cleanup_state();
    Ok(oid)
}

/// Whether committing right now would produce an empty commit.
pub fn would_be_empty(repo: &Repository) -> bool {
    let Ok(mut index) = repo.index() else {
        return false;
    };
    let Ok(tree_id) = index.write_tree() else {
        return false;
    };
    match repo.head().and_then(|h| h.peel_to_commit()) {
        Ok(head) => head.tree_id() == tree_id,
        // No HEAD: the first commit is empty only if the tree is.
        Err(_) => repo
            .find_tree(tree_id)
            .map(|t| t.is_empty())
            .unwrap_or(false),
    }
}

/// The message HEAD was committed with, for pre-filling an amend.
pub fn head_message(repo: &Repository) -> Option<String> {
    repo.head()
        .and_then(|h| h.peel_to_commit())
        .ok()
        .and_then(|c| c.message().ok().map(str::to_owned))
}

/// Split a message into its subject line and body.
pub fn split_message(message: &str) -> (&str, &str) {
    match message.split_once("\n\n") {
        Some((subject, body)) => (subject.trim_end(), body),
        None => (message.trim_end(), ""),
    }
}

/// Git's convention: a subject line beyond this reads badly in `git log
/// --oneline` and in most tools. Advisory, never enforced.
pub const SUBJECT_SOFT_LIMIT: usize = 50;
pub const SUBJECT_HARD_LIMIT: usize = 72;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_lines_are_stripped() {
        let raw = "Add feature\n# this is a comment\n\nBody text\n";
        assert_eq!(clean_message(raw, '#'), "Add feature\n\nBody text\n");
    }

    #[test]
    fn surrounding_blank_lines_are_removed() {
        assert_eq!(clean_message("\n\n  Subject  \n\n\n", '#'), "  Subject\n");
    }

    #[test]
    fn trailing_whitespace_goes_but_indentation_stays() {
        assert_eq!(
            clean_message("Subject\n\n    indented body   \n", '#'),
            "Subject\n\n    indented body\n"
        );
    }

    #[test]
    fn an_all_comment_message_is_empty() {
        assert_eq!(clean_message("# nothing\n# here\n", '#'), "");
    }

    #[test]
    fn a_message_without_a_body_splits_cleanly() {
        let (subject, body) = split_message("Just a subject\n");
        assert_eq!(subject, "Just a subject");
        assert_eq!(body, "");
    }

    #[test]
    fn subject_and_body_split_on_the_blank_line() {
        let (subject, body) = split_message("Subject\n\nBody line one\nBody line two\n");
        assert_eq!(subject, "Subject");
        assert_eq!(body, "Body line one\nBody line two\n");
    }
}
