//! Conflict inspection and resolution.
//!
//! A conflicted path lives in the index as up to three entries — stage 1 the
//! common ancestor, stage 2 "ours", stage 3 "theirs". Reading those directly is
//! what makes a real three-way view possible; parsing `<<<<<<<` markers out of
//! the working file, which is the tempting shortcut, loses the ancestor
//! entirely and therefore loses the only information that explains *why* the
//! two sides disagree.

use crate::error::{Error, Result};
use crate::git::diff::LineKind;
use git2::{IndexEntry, Repository};
use std::path::Path;
use std::sync::Arc;

/// One side of a conflict.
#[derive(Debug, Clone, Default)]
pub struct ConflictSide {
    pub present: bool,
    pub content: String,
    pub is_binary: bool,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub path: String,
    /// The common ancestor. Absent when both sides added the file.
    pub base: ConflictSide,
    /// The version on the current branch.
    pub ours: ConflictSide,
    /// The version being merged in.
    pub theirs: ConflictSide,
    /// The working-tree file, which git has filled with conflict markers.
    pub merged: String,
    pub kind: ConflictKind,
}

/// The shape of the disagreement, which decides what resolutions make sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides changed the content.
    BothModified,
    /// Both sides created it, differently.
    BothAdded,
    /// One side edited it, the other deleted it.
    ModifiedDeleted,
    /// Neither side has content we can show.
    Binary,
}

impl ConflictKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::BothModified => "Both sides changed this file",
            Self::BothAdded => "Both sides added this file",
            Self::ModifiedDeleted => "One side changed it, the other deleted it",
            Self::Binary => "Binary file — choose a side",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Conflicts {
    pub files: Vec<Conflict>,
}

impl Conflicts {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn find(&self, path: &str) -> Option<&Conflict> {
        self.files.iter().find(|c| c.path == path)
    }
}

/// Which version to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    /// Both, ours first — useful for additive conflicts like import lists.
    Both,
}

pub fn list(repo: &Repository) -> Result<Arc<Conflicts>> {
    let index = repo.index()?;
    let mut files = Vec::new();

    for entry in index.conflicts()? {
        let entry = entry?;
        let path = entry
            .our
            .as_ref()
            .or(entry.their.as_ref())
            .or(entry.ancestor.as_ref())
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }

        let base = side(repo, entry.ancestor.as_ref());
        let ours = side(repo, entry.our.as_ref());
        let theirs = side(repo, entry.their.as_ref());

        let kind = if ours.is_binary || theirs.is_binary {
            ConflictKind::Binary
        } else if !ours.present || !theirs.present {
            ConflictKind::ModifiedDeleted
        } else if !base.present {
            ConflictKind::BothAdded
        } else {
            ConflictKind::BothModified
        };

        let merged = repo
            .workdir()
            .map(|w| w.join(&path))
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();

        files.push(Conflict {
            path,
            base,
            ours,
            theirs,
            merged,
            kind,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Arc::new(Conflicts { files }))
}

fn side(repo: &Repository, entry: Option<&IndexEntry>) -> ConflictSide {
    let Some(entry) = entry else {
        return ConflictSide::default();
    };
    let Ok(blob) = repo.find_blob(entry.id) else {
        return ConflictSide::default();
    };
    if blob.is_binary() {
        return ConflictSide {
            present: true,
            content: String::new(),
            is_binary: true,
        };
    }
    ConflictSide {
        present: true,
        content: String::from_utf8_lossy(blob.content()).into_owned(),
        is_binary: false,
    }
}

/// Resolve a conflict by taking one side wholesale.
pub fn resolve_with(repo: &Repository, path: &str, resolution: Resolution) -> Result<()> {
    let conflicts = list(repo)?;
    let conflict = conflicts
        .find(path)
        .ok_or_else(|| Error::refused(format!("{path} isn't conflicted")))?;

    let content = match resolution {
        Resolution::Ours => conflict.ours.content.clone(),
        Resolution::Theirs => conflict.theirs.content.clone(),
        Resolution::Both => {
            // Ours then theirs, with a newline between if one is missing —
            // otherwise the last line of ours runs into the first of theirs.
            let mut text = conflict.ours.content.clone();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&conflict.theirs.content);
            text
        }
    };

    // Deleting on the chosen side means resolving to "not present".
    let taking_deletion = match resolution {
        Resolution::Ours => !conflict.ours.present,
        Resolution::Theirs => !conflict.theirs.present,
        Resolution::Both => false,
    };

    if taking_deletion {
        return resolve_as_deleted(repo, path);
    }
    write_resolution(repo, path, &content)
}

/// Resolve with content the user edited by hand.
pub fn resolve_with_content(repo: &Repository, path: &str, content: &str) -> Result<()> {
    if has_markers(content) {
        return Err(Error::refused(
            "This still contains conflict markers — remove them before resolving",
        ));
    }
    write_resolution(repo, path, content)
}

fn write_resolution(repo: &Repository, path: &str, content: &str) -> Result<()> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::refused("This repository has no working tree"))?;
    let full = workdir.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&full, content)?;

    // Adding the path replaces all three conflict stages with one stage-0
    // entry, which is what marks it resolved.
    let mut index = repo.index()?;
    index.add_path(Path::new(path))?;
    index.write()?;
    Ok(())
}

fn resolve_as_deleted(repo: &Repository, path: &str) -> Result<()> {
    if let Some(workdir) = repo.workdir() {
        let _ = std::fs::remove_file(workdir.join(path));
    }
    let mut index = repo.index()?;
    index.remove_path(Path::new(path))?;
    index.write()?;
    Ok(())
}

/// Whether text still contains git's conflict markers.
pub fn has_markers(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("<<<<<<<") || line.starts_with("=======") || line.starts_with(">>>>>>>")
    })
}

/// A three-way line view for rendering: base, ours, and theirs side by side.
///
/// Lines are aligned by diffing each side against the base, so unchanged
/// regions line up and the actual disagreement is what stands out.
#[derive(Debug, Clone)]
pub struct AlignedLine {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub ours_kind: LineKind,
    pub theirs_kind: LineKind,
}

/// Align the three sides of a conflict for display.
pub fn align(conflict: &Conflict) -> Vec<AlignedLine> {
    let base: Vec<&str> = conflict.base.content.lines().collect();
    let ours: Vec<&str> = conflict.ours.content.lines().collect();
    let theirs: Vec<&str> = conflict.theirs.content.lines().collect();

    let ours_ops = similar::capture_diff_slices(similar::Algorithm::Myers, &base, &ours);
    let theirs_ops = similar::capture_diff_slices(similar::Algorithm::Myers, &base, &theirs);

    // Map each base line to what each side did with it.
    let mut ours_by_base: Vec<Option<usize>> = vec![None; base.len()];
    let mut theirs_by_base: Vec<Option<usize>> = vec![None; base.len()];
    for op in &ours_ops {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..*len {
                ours_by_base[old_index + offset] = Some(new_index + offset);
            }
        }
    }
    for op in &theirs_ops {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..*len {
                theirs_by_base[old_index + offset] = Some(new_index + offset);
            }
        }
    }

    let mut rows = Vec::new();
    let (mut our_cursor, mut their_cursor) = (0usize, 0usize);

    for (index, line) in base.iter().enumerate() {
        // Emit anything each side inserted before this base line.
        let our_target = ours_by_base[index];
        let their_target = theirs_by_base[index];
        while our_target.is_some_and(|t| our_cursor < t)
            || their_target.is_some_and(|t| their_cursor < t)
        {
            let our_extra = our_target
                .is_some_and(|t| our_cursor < t)
                .then(|| ours[our_cursor].to_owned());
            let their_extra = their_target
                .is_some_and(|t| their_cursor < t)
                .then(|| theirs[their_cursor].to_owned());
            if our_extra.is_some() {
                our_cursor += 1;
            }
            if their_extra.is_some() {
                their_cursor += 1;
            }
            rows.push(AlignedLine {
                base: None,
                ours_kind: kind_for(&our_extra),
                theirs_kind: kind_for(&their_extra),
                ours: our_extra,
                theirs: their_extra,
            });
        }

        let our_line = our_target.map(|t| ours[t].to_owned());
        let their_line = their_target.map(|t| theirs[t].to_owned());
        if our_target.is_some() {
            our_cursor += 1;
        }
        if their_target.is_some() {
            their_cursor += 1;
        }

        rows.push(AlignedLine {
            base: Some((*line).to_owned()),
            ours_kind: if our_line.is_some() {
                LineKind::Context
            } else {
                LineKind::Deletion
            },
            theirs_kind: if their_line.is_some() {
                LineKind::Context
            } else {
                LineKind::Deletion
            },
            ours: our_line,
            theirs: their_line,
        });
    }

    // Trailing insertions past the end of the base.
    while our_cursor < ours.len() || their_cursor < theirs.len() {
        let our_extra = (our_cursor < ours.len()).then(|| ours[our_cursor].to_owned());
        let their_extra = (their_cursor < theirs.len()).then(|| theirs[their_cursor].to_owned());
        if our_extra.is_some() {
            our_cursor += 1;
        }
        if their_extra.is_some() {
            their_cursor += 1;
        }
        rows.push(AlignedLine {
            base: None,
            ours_kind: kind_for(&our_extra),
            theirs_kind: kind_for(&their_extra),
            ours: our_extra,
            theirs: their_extra,
        });
    }

    rows
}

fn kind_for(line: &Option<String>) -> LineKind {
    if line.is_some() {
        LineKind::Addition
    } else {
        LineKind::Context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_detected() {
        assert!(has_markers(
            "a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> other\n"
        ));
        assert!(!has_markers("a\nb\nc\n"));
        // A line that merely mentions the characters mid-line is not a marker.
        assert!(!has_markers("if x <<<<<<< y\n"));
    }

    fn conflict(base: &str, ours: &str, theirs: &str) -> Conflict {
        Conflict {
            path: "f.txt".to_owned(),
            base: ConflictSide {
                present: true,
                content: base.to_owned(),
                is_binary: false,
            },
            ours: ConflictSide {
                present: true,
                content: ours.to_owned(),
                is_binary: false,
            },
            theirs: ConflictSide {
                present: true,
                content: theirs.to_owned(),
                is_binary: false,
            },
            merged: String::new(),
            kind: ConflictKind::BothModified,
        }
    }

    #[test]
    fn unchanged_lines_align_across_all_three_sides() {
        let rows = align(&conflict("a\nb\nc\n", "a\nB\nc\n", "a\nb\nC\n"));
        // Every base line is represented exactly once.
        let base_lines: Vec<&str> = rows.iter().filter_map(|r| r.base.as_deref()).collect();
        assert_eq!(base_lines, vec!["a", "b", "c"]);

        let row_a = &rows[0];
        assert_eq!(row_a.ours.as_deref(), Some("a"));
        assert_eq!(row_a.theirs.as_deref(), Some("a"));
    }

    #[test]
    fn a_line_each_side_changed_shows_both_versions_gone_from_base() {
        let rows = align(&conflict("a\nb\nc\n", "a\nB\nc\n", "a\nb2\nc\n"));
        let changed = rows
            .iter()
            .find(|r| r.base.as_deref() == Some("b"))
            .expect("the base line");
        assert_eq!(changed.ours, None, "ours no longer contains the base line");
        assert_eq!(changed.theirs, None);
        assert_eq!(changed.ours_kind, LineKind::Deletion);

        // The replacements appear as insertions.
        let inserted: Vec<&str> = rows
            .iter()
            .filter(|r| r.base.is_none())
            .filter_map(|r| r.ours.as_deref())
            .collect();
        assert!(inserted.contains(&"B"), "got {inserted:?}");
    }

    #[test]
    fn an_empty_base_still_produces_rows_for_both_sides() {
        let rows = align(&conflict("", "ours\n", "theirs\n"));
        assert!(!rows.is_empty());
        assert!(rows.iter().any(|r| r.ours.as_deref() == Some("ours")));
        assert!(rows.iter().any(|r| r.theirs.as_deref() == Some("theirs")));
    }

    #[test]
    fn identical_sides_produce_no_deletions() {
        let rows = align(&conflict("a\nb\n", "a\nb\n", "a\nb\n"));
        assert!(rows.iter().all(|r| r.ours_kind != LineKind::Deletion));
        assert!(rows.iter().all(|r| r.theirs_kind != LineKind::Deletion));
    }
}
