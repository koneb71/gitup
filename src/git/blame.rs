//! Blame: who last touched each line, and when.

use crate::error::{Error, Result};
use crate::git::graph::CommitSummary;
use crate::job::Cancel;
use git2::{BlameOptions, Oid, Repository};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Blaming a very large file is slow and the result is unreadable anyway.
const MAX_LINES: usize = 50_000;

#[derive(Debug, Clone)]
pub struct BlameLine {
    pub line_no: u32,
    pub content: String,
    pub commit: Oid,
    /// True for the first line of a run attributed to the same commit, so the
    /// view can show the metadata once per group instead of on every line.
    pub starts_group: bool,
    /// Syntax-highlighting runs over `content`.
    pub spans: Vec<super::highlight::Span>,
}

#[derive(Debug, Clone)]
pub struct BlameResult {
    pub path: String,
    pub lines: Vec<BlameLine>,
    /// Metadata for every commit that appears, looked up once.
    pub commits: HashMap<Oid, CommitSummary>,
    /// Stable colour index per commit, assigned in order of first appearance.
    pub colors: HashMap<Oid, usize>,
    /// Oldest and newest author times, for shading lines by age.
    pub time_range: (i64, i64),
    pub truncated: bool,
}

impl BlameResult {
    pub fn commit(&self, oid: Oid) -> Option<&CommitSummary> {
        self.commits.get(&oid)
    }

    /// Where a line's commit falls between the oldest and newest, 0.0 to 1.0.
    ///
    /// Used to shade the age bar: recent changes are what you are usually
    /// looking for, and a gradient finds them faster than reading dates.
    pub fn recency(&self, oid: Oid) -> f32 {
        let (oldest, newest) = self.time_range;
        if newest <= oldest {
            return 1.0;
        }
        let Some(commit) = self.commits.get(&oid) else {
            return 0.0;
        };
        ((commit.time - oldest) as f32 / (newest - oldest) as f32).clamp(0.0, 1.0)
    }
}

/// Blame `path`, as of `at` or the working tree.
pub fn blame(
    repo: &Repository,
    path: &str,
    at: Option<Oid>,
    theme: super::highlight::HighlightTheme,
    cancel: &Cancel,
) -> Result<Arc<BlameResult>> {
    let mut options = BlameOptions::new();
    options
        .track_copies_same_file(true)
        .track_copies_same_commit_moves(true);
    if let Some(oid) = at {
        options.newest_commit(oid);
    }

    let blame = repo
        .blame_file(Path::new(path), Some(&mut options))
        .map_err(|e| Error::refused(format!("Couldn't blame {path}: {}", e.message())))?;

    let content = read_content(repo, path, at)?;
    let mut text_lines: Vec<&str> = content.lines().collect();
    let truncated = text_lines.len() > MAX_LINES;
    if truncated {
        text_lines.truncate(MAX_LINES);
    }

    // Highlighted in one pass over the whole file, which is both faster than
    // per-line work and more accurate than the diff view can manage: here the
    // parser sees the complete text.
    let mut highlights = super::highlight::whole_file(path, text_lines.iter().copied(), theme);

    let mut lines = Vec::with_capacity(text_lines.len());
    let mut commits: HashMap<Oid, CommitSummary> = HashMap::new();
    let mut colors: HashMap<Oid, usize> = HashMap::new();
    let mut previous: Option<Oid> = None;
    let (mut oldest, mut newest) = (i64::MAX, i64::MIN);

    for (index, text) in text_lines.iter().enumerate() {
        if index % 2048 == 0 {
            cancel.check()?;
        }
        let line_no = index as u32 + 1;
        // libgit2 indexes blame hunks by 1-based line number.
        let Some(hunk) = blame.get_line(index + 1) else {
            continue;
        };
        let oid = hunk.final_commit_id();

        if let std::collections::hash_map::Entry::Vacant(slot) = commits.entry(oid) {
            if let Ok(commit) = repo.find_commit(oid) {
                let summary = super::graph::summarize(&commit);
                oldest = oldest.min(summary.time);
                newest = newest.max(summary.time);
                slot.insert(summary);
            }
        }
        let next_color = colors.len();
        colors.entry(oid).or_insert(next_color);

        lines.push(BlameLine {
            line_no,
            content: super::highlight::expand_tabs(text).unwrap_or_else(|| (*text).to_owned()),
            commit: oid,
            starts_group: previous != Some(oid),
            spans: highlights
                .get_mut(index)
                .map(std::mem::take)
                .unwrap_or_default(),
        });
        previous = Some(oid);
    }

    if oldest == i64::MAX {
        oldest = 0;
        newest = 0;
    }

    Ok(Arc::new(BlameResult {
        path: path.to_owned(),
        lines,
        commits,
        colors,
        time_range: (oldest, newest),
        truncated,
    }))
}

/// File content as of a commit, or from the working tree.
fn read_content(repo: &Repository, path: &str, at: Option<Oid>) -> Result<String> {
    match at {
        Some(oid) => {
            let commit = repo.find_commit(oid)?;
            let entry = commit
                .tree()?
                .get_path(Path::new(path))
                .map_err(|_| Error::refused(format!("{path} doesn't exist at that commit")))?;
            let blob = repo.find_blob(entry.id())?;
            if blob.is_binary() {
                return Err(Error::refused("Can't blame a binary file"));
            }
            Ok(String::from_utf8_lossy(blob.content()).into_owned())
        }
        None => {
            let workdir = repo
                .workdir()
                .ok_or_else(|| Error::refused("This repository has no working tree"))?;
            let full = workdir.join(path);
            let bytes = std::fs::read(&full)?;
            if bytes.contains(&0) {
                return Err(Error::refused("Can't blame a binary file"));
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}
