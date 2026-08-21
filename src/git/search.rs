//! Searching history.
//!
//! GitAhead maintained a Lucene index for this, which is where a good deal of
//! its bug reports came from: the index went stale after operations it didn't
//! observe, and rebuilding it was slow enough that people avoided it. There is
//! no index here. `git log` already searches messages, content, authors, and
//! paths, using data structures git maintains anyway — so a search is a process
//! spawn, not a second copy of the repository that can disagree with the first.

use crate::error::Result;
use crate::git::graph::CommitSummary;
use crate::job::Cancel;
use git2::{Oid, Repository};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchKind {
    /// Commit messages.
    #[default]
    Message,
    /// Commits where the number of occurrences of a string changed (`-S`).
    Content,
    /// Commits whose diff contains a match for a regex (`-G`).
    Diff,
    Author,
    /// Commits touching a path.
    Path,
}

impl SearchKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Message => "Message",
            Self::Content => "Content",
            Self::Diff => "Diff",
            Self::Author => "Author",
            Self::Path => "Path",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Message => "Search commit messages",
            Self::Content => "Find commits that added or removed this text",
            Self::Diff => "Find commits whose diff matches this pattern",
            Self::Author => "Search by author name or email",
            Self::Path => "Find commits touching this path",
        }
    }

    pub fn all() -> [Self; 5] {
        [
            Self::Message,
            Self::Content,
            Self::Author,
            Self::Path,
            Self::Diff,
        ]
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub commits: Vec<CommitSummary>,
    pub query: String,
    /// True when the result was cut off at the limit.
    pub truncated: bool,
}

/// Run a search, returning matching commits newest first.
pub fn search(
    repo: &Repository,
    kind: SearchKind,
    query: &str,
    limit: usize,
    cancel: &Cancel,
) -> Result<Arc<SearchResults>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Arc::new(SearchResults::default()));
    }

    let workdir = repo.workdir().unwrap_or_else(|| repo.path());
    // One more than the limit, so a full page can be distinguished from a page
    // that happens to end exactly at the limit.
    let count = (limit + 1).to_string();

    let mut args: Vec<String> = vec![
        "log".to_owned(),
        "--all".to_owned(),
        "--format=%H".to_owned(),
        "-n".to_owned(),
        count,
    ];
    match kind {
        SearchKind::Message => {
            args.push("--regexp-ignore-case".to_owned());
            args.push("--fixed-strings".to_owned());
            args.push(format!("--grep={query}"));
        }
        SearchKind::Content => {
            args.push("--pickaxe-regex".to_owned());
            args.push(format!("-S{query}"));
        }
        SearchKind::Diff => {
            args.push(format!("-G{query}"));
        }
        SearchKind::Author => {
            args.push("--regexp-ignore-case".to_owned());
            args.push(format!("--author={query}"));
        }
        SearchKind::Path => {
            args.push("--".to_owned());
            args.push(query.to_owned());
        }
    }

    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = super::cli::run(workdir, &borrowed, cancel, |_| {})?;

    let mut commits = Vec::new();
    let mut truncated = false;
    for (index, line) in output.stdout.lines().enumerate() {
        if index >= limit {
            truncated = true;
            break;
        }
        if index % 256 == 0 {
            cancel.check()?;
        }
        let Ok(oid) = Oid::from_str(line.trim()) else {
            continue;
        };
        if let Ok(commit) = repo.find_commit(oid) {
            commits.push(super::graph::summarize(&commit));
        }
    }

    Ok(Arc::new(SearchResults {
        commits,
        query: query.to_owned(),
        truncated,
    }))
}

/// Commits that touched a path, following it across renames.
///
/// `--follow` is a git feature with no libgit2 equivalent, and following
/// renames is the entire point of asking for a file's history.
pub fn file_history(
    repo: &Repository,
    path: &str,
    limit: usize,
    cancel: &Cancel,
) -> Result<Arc<SearchResults>> {
    let workdir = repo.workdir().unwrap_or_else(|| repo.path());
    let count = (limit + 1).to_string();
    let args = vec![
        "log",
        "--follow",
        "--format=%H",
        "-n",
        count.as_str(),
        "--",
        path,
    ];
    let output = super::cli::run(workdir, &args, cancel, |_| {})?;

    let mut commits = Vec::new();
    let mut truncated = false;
    for (index, line) in output.stdout.lines().enumerate() {
        if index >= limit {
            truncated = true;
            break;
        }
        let Ok(oid) = Oid::from_str(line.trim()) else {
            continue;
        };
        if let Ok(commit) = repo.find_commit(oid) {
            commits.push(super::graph::summarize(&commit));
        }
    }
    cancel.check()?;

    Ok(Arc::new(SearchResults {
        commits,
        query: path.to_owned(),
        truncated,
    }))
}

/// Whether a path exists in the working tree, so the UI can decide whether
/// blame is meaningful.
pub fn path_exists(repo: &Repository, path: &str) -> bool {
    repo.workdir()
        .map(|w| w.join(Path::new(path)).is_file())
        .unwrap_or(false)
}
