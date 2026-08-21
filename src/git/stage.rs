//! Staging, unstaging, and discarding — including at hunk and line granularity.
//!
//! Whole-file operations go through the index directly, because that is exact
//! and cannot be confused by content. Partial operations cannot: git has no
//! "stage these lines" primitive. What it has is `apply`, so partial staging
//! works by *synthesizing a patch* containing only the selected changes and
//! applying it to the index.
//!
//! Getting that patch right is the delicate part, and the rules are not
//! symmetric:
//!
//! * A selected `+` line stays `+` — it is being added.
//! * An *unselected* `+` line is **dropped** — it must not exist in the result.
//! * A selected `-` line stays `-` — it is being removed.
//! * An *unselected* `-` line becomes **context** — it must still be there
//!   afterwards, so the patch has to claim it as unchanged.
//!
//! Get the last one wrong and staging one line of a multi-line edit silently
//! deletes the lines you didn't pick.
//!
//! Unstaging and discarding run the *other* way: the file being patched is the
//! diff's new side, not its old one. That is not the same as flipping the signs
//! of a forward patch — the context decisions mirror as well:
//!
//! * A selected `+` becomes `-`; an *unselected* `+` becomes **context**,
//!   because it is present in the file being patched and must stay.
//! * A selected `-` becomes `+`; an *unselected* `-` is **dropped**, because it
//!   is not in that file at all.
//!
//! Building a forward patch and inverting it produces the right signs with the
//! wrong context, and the apply then fails to match — or worse, matches
//! somewhere it shouldn't.

use crate::error::{Error, Result};
use crate::git::diff::{DiffLine, FileDiff, LineKind};
use git2::{ApplyLocation, Diff, IndexAddOption, Repository};
use std::path::Path;

/// Which lines of a hunk to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSelection {
    pub hunk_index: usize,
    /// Indices into the hunk's `lines`. `None` means the entire hunk.
    pub lines: Option<Vec<usize>>,
}

impl HunkSelection {
    pub fn whole(hunk_index: usize) -> Self {
        Self {
            hunk_index,
            lines: None,
        }
    }

    fn includes(&self, line_index: usize) -> bool {
        match &self.lines {
            None => true,
            Some(lines) => lines.contains(&line_index),
        }
    }
}

/// Direction of a partial apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Working tree → index: stage.
    Stage,
    /// Index → working tree: unstage.
    Unstage,
    /// Working tree → discard the change entirely.
    Discard,
}

/// Build a unified patch containing only the selected changes.
///
/// Returns `None` when the selection contains no actual change, which happens
/// if a user selects only context lines — applying that would be a no-op patch
/// that git rejects.
pub fn build_patch(file: &FileDiff, selections: &[HunkSelection], reverse: bool) -> Option<String> {
    let mut body = String::new();
    let mut any_change = false;

    for selection in selections {
        let hunk = file.hunks.get(selection.hunk_index)?;

        let mut lines = String::new();
        let (mut old_count, mut new_count) = (0u32, 0u32);
        let mut hunk_has_change = false;

        for (index, line) in hunk.lines.iter().enumerate() {
            let selected = selection.includes(index);
            match (line.kind, reverse) {
                (LineKind::Context, _) => {
                    push(&mut lines, ' ', line);
                    old_count += 1;
                    new_count += 1;
                }

                // Forward: staging.
                (LineKind::Addition, false) => {
                    if selected {
                        push(&mut lines, '+', line);
                        new_count += 1;
                        hunk_has_change = true;
                    }
                    // Unselected additions are simply absent: not in the old
                    // file, and not wanted in the new one.
                }
                (LineKind::Deletion, false) => {
                    if selected {
                        push(&mut lines, '-', line);
                        old_count += 1;
                        hunk_has_change = true;
                    } else {
                        // Not being removed, so it survives — the patch has to
                        // carry it as context on both sides.
                        push(&mut lines, ' ', line);
                        old_count += 1;
                        new_count += 1;
                    }
                }

                // Reverse: unstaging or discarding.
                (LineKind::Addition, true) => {
                    if selected {
                        push(&mut lines, '-', line);
                        old_count += 1;
                        hunk_has_change = true;
                    } else {
                        // Present in the file being patched, and staying.
                        push(&mut lines, ' ', line);
                        old_count += 1;
                        new_count += 1;
                    }
                }
                (LineKind::Deletion, true) => {
                    if selected {
                        push(&mut lines, '+', line);
                        new_count += 1;
                        hunk_has_change = true;
                    }
                    // Unselected deletions are not in the file being patched.
                }

                (LineKind::NoNewline, _) => {
                    lines.push_str("\\ No newline at end of file\n");
                }
            }
        }

        if !hunk_has_change {
            continue;
        }
        any_change = true;

        // The start line refers to the file being patched: the diff's old side
        // going forward, its new side in reverse. Both sides of the emitted
        // header use it, because only this hunk is applied — no earlier hunk
        // has shifted anything.
        let anchor = if reverse {
            hunk.new_start
        } else {
            hunk.old_start
        };
        let old_start = normalize_start(anchor, old_count);
        let new_start = normalize_start(anchor, new_count);
        body.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
        ));
        body.push_str(&lines);
    }

    if !any_change {
        return None;
    }

    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let new_path = &file.path;

    // libgit2's parser cross-checks the `diff --git` header against the `---`
    // and `+++` lines and rejects a mismatch, so reversing a patch means
    // swapping the header paths too — not just the body. A creation reversed is
    // a deletion, and both need an explicit mode line or the /dev/null side is
    // rejected as a "mismatched old path name".
    let creates = matches!(
        file.status,
        crate::git::Delta::Added | crate::git::Delta::Untracked
    );
    let deletes = file.status == crate::git::Delta::Deleted;
    let mode = format!("{:o}", file.mode);

    let dev_null = "/dev/null".to_owned();
    let (a_path, b_path, from, to, mode_line) = match (reverse, creates, deletes) {
        // Reversing a creation deletes the file.
        (true, true, _) => (
            new_path.clone(),
            new_path.clone(),
            format!("a/{new_path}"),
            dev_null.clone(),
            Some(format!("deleted file mode {mode}\n")),
        ),
        // Reversing a deletion recreates it.
        (true, _, true) => (
            old_path.to_owned(),
            old_path.to_owned(),
            dev_null.clone(),
            format!("b/{old_path}"),
            Some(format!("new file mode {mode}\n")),
        ),
        (true, _, _) => (
            new_path.clone(),
            old_path.to_owned(),
            format!("a/{new_path}"),
            format!("b/{old_path}"),
            None,
        ),
        (false, true, _) => (
            new_path.clone(),
            new_path.clone(),
            dev_null.clone(),
            format!("b/{new_path}"),
            Some(format!("new file mode {mode}\n")),
        ),
        (false, _, true) => (
            old_path.to_owned(),
            old_path.to_owned(),
            format!("a/{old_path}"),
            dev_null,
            Some(format!("deleted file mode {mode}\n")),
        ),
        (false, _, _) => (
            old_path.to_owned(),
            new_path.clone(),
            format!("a/{old_path}"),
            format!("b/{new_path}"),
            None,
        ),
    };

    let mode_line = mode_line.unwrap_or_default();

    Some(format!(
        "diff --git a/{a_path} b/{b_path}\n{mode_line}--- {from}\n+++ {to}\n{body}"
    ))
}

/// Git counts an empty side as starting at line 0, not line 1.
fn normalize_start(start: u32, count: u32) -> u32 {
    if count == 0 {
        0
    } else {
        start.max(1)
    }
}

fn push(out: &mut String, sign: char, line: &DiffLine) {
    out.push(sign);
    // The raw content, never the tab-expanded display text: this has to match
    // the file byte for byte or the apply will not find its context.
    out.push_str(&line.content);
    out.push('\n');
}

/// Stage or unstage entire files.
///
/// This goes through the index rather than a patch because it is exact:
/// `add_path` records the file as it is on disk, with no context matching that
/// could fail.
pub fn stage_files(repo: &Repository, paths: &[String]) -> Result<()> {
    let mut index = repo.index()?;
    for path in paths {
        let p = Path::new(path);
        if repo.workdir().is_some_and(|w| w.join(p).exists()) {
            index.add_path(p)?;
        } else {
            // The file is gone from the worktree, so staging it means staging
            // the deletion.
            index.remove_path(p)?;
        }
    }
    index.write()?;
    Ok(())
}

/// Unstage entire files: reset their index entry back to HEAD.
pub fn unstage_files(repo: &Repository, paths: &[String]) -> Result<()> {
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let mut index = repo.index()?;

    match head {
        Some(commit) => {
            let tree = commit.tree()?;
            for path in paths {
                let p = Path::new(path);
                match tree.get_path(p) {
                    Ok(entry) => {
                        // Restore the HEAD version of this path into the index.
                        let mut index_entry = index_entry_for(&entry, path)?;
                        index_entry.path = path.as_bytes().to_vec();
                        index.add(&index_entry)?;
                    }
                    // Not in HEAD, so unstaging means removing it entirely,
                    // which returns the file to being untracked.
                    Err(_) => {
                        let _ = index.remove_path(p);
                    }
                }
            }
        }
        None => {
            // No commits yet: everything staged came from nothing.
            for path in paths {
                let _ = index.remove_path(Path::new(path));
            }
        }
    }

    index.write()?;
    Ok(())
}

fn index_entry_for(entry: &git2::TreeEntry<'_>, path: &str) -> Result<git2::IndexEntry> {
    Ok(git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: entry.filemode() as u32,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: entry.id(),
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    })
}

/// Apply a synthesized patch to the index or the working tree.
pub fn apply_patch(repo: &Repository, patch: &str, location: ApplyLocation) -> Result<()> {
    let diff = Diff::from_buffer(patch.as_bytes()).map_err(|e| {
        Error::refused(format!(
            "Couldn't parse the generated patch: {}",
            e.message()
        ))
    })?;
    repo.apply(&diff, location, None).map_err(|e| {
        // A failed apply almost always means the file changed underneath us.
        Error::refused(format!(
            "Couldn't apply the change: {}. The file may have changed since the diff was computed.",
            e.message()
        ))
    })
}

/// Stage part of a file.
pub fn stage_partial(
    repo: &Repository,
    file: &FileDiff,
    selections: &[HunkSelection],
) -> Result<()> {
    let Some(patch) = build_patch(file, selections, false) else {
        return Err(Error::refused("Nothing to stage in that selection"));
    };
    apply_patch(repo, &patch, ApplyLocation::Index)
}

/// Unstage part of a file, by reverse-applying its staged diff to the index.
pub fn unstage_partial(
    repo: &Repository,
    file: &FileDiff,
    selections: &[HunkSelection],
) -> Result<()> {
    let Some(patch) = build_patch(file, selections, true) else {
        return Err(Error::refused("Nothing to unstage in that selection"));
    };
    apply_patch(repo, &patch, ApplyLocation::Index)
}

/// Throw away part of a file's unstaged changes.
pub fn discard_partial(
    repo: &Repository,
    file: &FileDiff,
    selections: &[HunkSelection],
) -> Result<()> {
    let Some(patch) = build_patch(file, selections, true) else {
        return Err(Error::refused("Nothing to discard in that selection"));
    };
    apply_patch(repo, &patch, ApplyLocation::WorkDir)
}

/// Throw away all unstaged changes to these paths, restoring them from the index.
pub fn discard_files(repo: &Repository, paths: &[String]) -> Result<()> {
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().remove_untracked(false);
    for path in paths {
        checkout.path(path);
    }
    repo.checkout_index(None, Some(&mut checkout))?;
    Ok(())
}

/// Delete untracked files outright. Separate from [`discard_files`] because
/// there is nothing in the index to restore them from, and because deleting a
/// file the user has never committed is not reversible.
pub fn delete_untracked(repo: &Repository, paths: &[String]) -> Result<()> {
    let Some(workdir) = repo.workdir() else {
        return Err(Error::refused("This repository has no working tree"));
    };
    for path in paths {
        let full = workdir.join(path);
        if full.is_file() {
            std::fs::remove_file(&full)?;
        }
    }
    Ok(())
}

/// Stage every change in the working tree.
pub fn stage_all(repo: &Repository) -> Result<()> {
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    // `add_all` does not record deletions of files removed from disk.
    index.update_all(["*"].iter(), None)?;
    index.write()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::Hunk;
    use crate::git::Delta;

    fn line(kind: LineKind, content: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_lineno: old,
            new_lineno: new,
            content: content.to_owned(),
            display: None,
            truncated: false,
            spans: Vec::new(),
            emphasis: Vec::new(),
        }
    }

    /// A hunk replacing lines 2 and 3 of a five-line file.
    fn sample() -> FileDiff {
        FileDiff {
            path: "f.txt".to_owned(),
            old_path: None,
            status: Delta::Modified,
            additions: 2,
            deletions: 2,
            omitted: None,
            mode: 0o100_644,
            image: None,
            lfs: None,
            hunks: vec![Hunk {
                header: "@@ -1,5 +1,5 @@".to_owned(),
                old_start: 1,
                old_lines: 5,
                new_start: 1,
                new_lines: 5,
                lines: vec![
                    line(LineKind::Context, "one", Some(1), Some(1)),
                    line(LineKind::Deletion, "two", Some(2), None),
                    line(LineKind::Deletion, "three", Some(3), None),
                    line(LineKind::Addition, "TWO", None, Some(2)),
                    line(LineKind::Addition, "THREE", None, Some(3)),
                    line(LineKind::Context, "four", Some(4), Some(4)),
                ],
            }],
        }
    }

    #[test]
    fn a_whole_hunk_round_trips_unchanged() {
        let file = sample();
        let patch = build_patch(&file, &[HunkSelection::whole(0)], false).expect("patch");
        assert!(patch.contains("@@ -1,4 +1,4 @@"));
        assert!(patch.contains("-two\n"));
        assert!(patch.contains("+TWO\n"));
        assert!(patch.contains(" one\n") && patch.contains(" four\n"));
    }

    #[test]
    fn unselected_deletions_become_context_not_removals() {
        // This is the rule that matters most: staging only the removal of
        // "two" must leave "three" in the file, so the patch has to claim it
        // as context on both sides.
        let file = sample();
        let selection = HunkSelection {
            hunk_index: 0,
            lines: Some(vec![1]), // just the "-two" line
        };
        let patch = build_patch(&file, &[selection], false).expect("patch");

        assert!(patch.contains("-two\n"), "the selected removal");
        assert!(
            patch.contains(" three\n"),
            "the unselected removal must survive as context, got:\n{patch}"
        );
        assert!(!patch.contains("-three\n"), "it must not be removed");
        assert!(
            !patch.contains("+TWO") && !patch.contains("+THREE"),
            "unselected additions must be absent entirely"
        );
        // old side: one, two, three, four = 4; new side: one, three, four = 3.
        assert!(patch.contains("@@ -1,4 +1,3 @@"), "got:\n{patch}");
    }

    #[test]
    fn unselected_additions_are_dropped_entirely() {
        let file = sample();
        let selection = HunkSelection {
            hunk_index: 0,
            lines: Some(vec![3]), // just the "+TWO" line
        };
        let patch = build_patch(&file, &[selection], false).expect("patch");

        assert!(patch.contains("+TWO\n"));
        assert!(!patch.contains("THREE"), "got:\n{patch}");
        // Both deletions are unselected, so both become context.
        assert!(patch.contains(" two\n") && patch.contains(" three\n"));
        assert!(patch.contains("@@ -1,4 +1,5 @@"), "got:\n{patch}");
    }

    #[test]
    fn a_context_only_selection_produces_no_patch() {
        let file = sample();
        let selection = HunkSelection {
            hunk_index: 0,
            lines: Some(vec![0, 5]), // both context lines
        };
        assert!(
            build_patch(&file, &[selection], false).is_none(),
            "a patch with no change would be rejected by git"
        );
    }

    #[test]
    fn reversing_swaps_signs_and_hunk_sides() {
        let file = sample();
        let forward = build_patch(&file, &[HunkSelection::whole(0)], false).expect("forward");
        let reverse = build_patch(&file, &[HunkSelection::whole(0)], true).expect("reverse");

        assert!(forward.contains("-two\n") && forward.contains("+TWO\n"));
        assert!(
            reverse.contains("+two\n") && reverse.contains("-TWO\n"),
            "got:\n{reverse}"
        );
        assert!(forward.contains("@@ -1,4 +1,4 @@"));
        assert!(reverse.contains("@@ -1,4 +1,4 @@"));
        // Context lines keep their leading space in both directions.
        assert!(reverse.contains(" one\n") && reverse.contains(" four\n"));
    }

    #[test]
    fn asymmetric_counts_survive_reversal() {
        let file = FileDiff {
            path: "f.txt".to_owned(),
            old_path: None,
            status: Delta::Modified,
            additions: 2,
            deletions: 0,
            omitted: None,
            mode: 0o100_644,
            image: None,
            lfs: None,
            hunks: vec![Hunk {
                header: "@@".to_owned(),
                old_start: 10,
                old_lines: 1,
                new_start: 10,
                new_lines: 3,
                lines: vec![
                    line(LineKind::Context, "keep", Some(10), Some(10)),
                    line(LineKind::Addition, "new one", None, Some(11)),
                    line(LineKind::Addition, "new two", None, Some(12)),
                ],
            }],
        };
        let reverse = build_patch(&file, &[HunkSelection::whole(0)], true).expect("reverse");
        // Forward is -10,1 +10,3; reversed must be -10,3 +10,1.
        assert!(reverse.contains("@@ -10,3 +10,1 @@"), "got:\n{reverse}");
    }

    #[test]
    fn a_new_file_names_dev_null_on_the_old_side() {
        let file = FileDiff {
            path: "new.txt".to_owned(),
            old_path: None,
            status: Delta::Untracked,
            additions: 1,
            deletions: 0,
            omitted: None,
            mode: 0o100_644,
            image: None,
            lfs: None,
            hunks: vec![Hunk {
                header: "@@".to_owned(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                lines: vec![line(LineKind::Addition, "hello", None, Some(1))],
            }],
        };
        let patch = build_patch(&file, &[HunkSelection::whole(0)], false).expect("patch");
        assert!(patch.contains("--- /dev/null"), "got:\n{patch}");
        assert!(patch.contains("+++ b/new.txt"));
        assert!(patch.contains("@@ -0,0 +1,1 @@"), "got:\n{patch}");
    }

    #[test]
    fn a_missing_trailing_newline_is_preserved() {
        let mut file = sample();
        file.hunks[0]
            .lines
            .push(line(LineKind::NoNewline, "", None, None));
        let patch = build_patch(&file, &[HunkSelection::whole(0)], false).expect("patch");
        assert!(patch.contains("\\ No newline at end of file\n"));
    }

    #[test]
    fn renames_keep_both_paths_in_the_header() {
        let mut file = sample();
        file.path = "new_name.txt".to_owned();
        file.old_path = Some("old_name.txt".to_owned());
        file.status = Delta::Renamed;
        let patch = build_patch(&file, &[HunkSelection::whole(0)], false).expect("patch");
        assert!(patch.starts_with("diff --git a/old_name.txt b/new_name.txt\n"));
        assert!(patch.contains("--- a/old_name.txt"));
        assert!(patch.contains("+++ b/new_name.txt"));
    }
}
