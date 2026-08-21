//! Diff computation.
//!
//! Produces a flat, self-describing model: files, hunks, and lines with their
//! old and new line numbers already resolved. The renderer never has to consult
//! libgit2, which is what allows the diff view to virtualize as aggressively as
//! the history list.

use crate::error::Result;
use crate::job::Cancel;
use git2::{Delta as GitDelta, DiffOptions, Oid, Patch, Repository};
use std::sync::Arc;

use super::status::Delta;

/// Lines longer than this are truncated. A minified bundle on one line would
/// otherwise force the layout engine to shape a megabyte of text for one row.
const MAX_LINE_LEN: usize = 4_000;
/// Files with more diff lines than this are summarized instead of rendered.
const MAX_FILE_LINES: usize = 30_000;

/// What to diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget {
    /// A commit against its first parent.
    Commit(Oid),
    /// Index against HEAD — what is staged.
    Staged,
    /// Working tree against the index — what is not staged.
    Unstaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
    /// `\ No newline at end of file`.
    NoNewline,
}

impl LineKind {
    pub fn sign(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Addition => '+',
            Self::Deletion => '-',
            Self::NoNewline => '\\',
        }
    }

    fn from_origin(origin: char) -> Option<Self> {
        match origin {
            ' ' | '=' => Some(Self::Context),
            '+' | '>' => Some(Self::Addition),
            '-' | '<' => Some(Self::Deletion),
            '\\' => Some(Self::NoNewline),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old file, when the line exists there.
    pub old_lineno: Option<u32>,
    /// Line number in the new file, when the line exists there.
    pub new_lineno: Option<u32>,
    /// Content with the trailing newline stripped, exactly as git has it.
    /// Staging reads this, so it must not be prettified.
    pub content: String,
    /// Tab-expanded text for display, when it differs from `content`.
    pub display: Option<String>,
    /// True when [`MAX_LINE_LEN`] cut this line short.
    pub truncated: bool,
    /// Syntax-highlighting runs over the displayed text. Empty when the file
    /// type is unknown or highlighting is off.
    pub spans: Vec<super::highlight::Span>,
    /// Character ranges that differ from the paired line on the other side.
    pub emphasis: Vec<super::inline::Emphasis>,
}

impl DiffLine {
    /// The text that is actually rendered.
    pub fn display_text(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.content)
    }
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@` line, including any trailing function context.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// One row of a side-by-side rendering: the old line, the new line, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// Index into the hunk's lines for the left (old) column.
    pub left: Option<usize>,
    /// Index into the hunk's lines for the right (new) column.
    pub right: Option<usize>,
}

impl Hunk {
    /// Pair the hunk's lines into side-by-side rows.
    ///
    /// Context lines occupy both columns. A run of deletions followed by a run
    /// of additions is paired positionally, so a two-line edit reads as two
    /// rows of before-and-after rather than four stacked lines; whichever run
    /// is longer leaves blanks opposite its surplus.
    pub fn pair_lines(&self) -> Vec<Pair> {
        let mut pairs = Vec::with_capacity(self.lines.len());
        let mut index = 0;

        while index < self.lines.len() {
            match self.lines[index].kind {
                LineKind::Context | LineKind::NoNewline => {
                    pairs.push(Pair {
                        left: Some(index),
                        right: Some(index),
                    });
                    index += 1;
                }
                _ => {
                    let deletions_start = index;
                    while index < self.lines.len() && self.lines[index].kind == LineKind::Deletion {
                        index += 1;
                    }
                    let deletions_end = index;

                    let additions_start = index;
                    while index < self.lines.len() && self.lines[index].kind == LineKind::Addition {
                        index += 1;
                    }
                    let additions_end = index;

                    let deletions = deletions_end - deletions_start;
                    let additions = additions_end - additions_start;
                    for offset in 0..deletions.max(additions) {
                        pairs.push(Pair {
                            left: (offset < deletions).then_some(deletions_start + offset),
                            right: (offset < additions).then_some(additions_start + offset),
                        });
                    }

                    // A lone addition run with no preceding deletions still has
                    // to advance, or this loops forever.
                    if deletions == 0 && additions == 0 {
                        index += 1;
                    }
                }
            }
        }

        pairs
    }

    /// How many rows this hunk occupies side by side.
    pub fn paired_len(&self) -> usize {
        self.pair_lines().len()
    }

    pub fn additions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Addition)
            .count()
    }

    pub fn deletions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Deletion)
            .count()
    }
}

/// One side of an image comparison.
#[derive(Debug, Clone)]
pub struct ImageSide {
    pub bytes: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

/// Both versions of a changed image.
///
/// Only the bytes are carried; decoding happens in the renderer, which already
/// has an image cache and knows what size it needs. Dimensions are read from
/// the header here because they belong beside the file's other metadata and
/// cost a few bytes to obtain.
#[derive(Debug, Clone)]
pub struct ImagePreview {
    pub old: Option<ImageSide>,
    pub new: Option<ImageSide>,
}

/// Images larger than this are treated as ordinary binary files. Decoding a
/// hundred-megabyte texture to show a thumbnail helps nobody.
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Whether a path looks like an image the renderer can display.
fn is_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".ico"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Read one side's bytes and header dimensions.
fn image_side(bytes: Vec<u8>) -> Option<ImageSide> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    // `into_dimensions` reads the header only, not the pixels.
    let dimensions = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    Some(ImageSide {
        bytes: Arc::new(bytes),
        width: dimensions.0,
        height: dimensions.1,
    })
}

/// Collect both versions of a changed image, if it is one.
fn image_preview(
    repo: &Repository,
    delta: &git2::DiffDelta<'_>,
    path: &str,
    from_workdir: bool,
) -> Option<ImagePreview> {
    if !is_image_path(path) {
        return None;
    }

    let old = (!delta.old_file().id().is_zero())
        .then(|| repo.find_blob(delta.old_file().id()).ok())
        .flatten()
        .and_then(|blob| image_side(blob.content().to_vec()));

    let new = if from_workdir {
        // A working-tree diff has no blob for the new side; the file on disk
        // *is* the new side.
        repo.workdir()
            .map(|w| w.join(path))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(image_side)
    } else {
        (!delta.new_file().id().is_zero())
            .then(|| repo.find_blob(delta.new_file().id()).ok())
            .flatten()
            .and_then(|blob| image_side(blob.content().to_vec()))
    };

    (old.is_some() || new.is_some()).then_some(ImagePreview { old, new })
}

/// Why a file has no rendered content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Omitted {
    Binary,
    TooLarge,
    /// Submodule pointer change.
    Submodule,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    /// Present for renames and copies.
    pub old_path: Option<String>,
    pub status: Delta,
    pub hunks: Vec<Hunk>,
    pub additions: usize,
    pub deletions: usize,
    /// Set when content was deliberately not rendered.
    pub omitted: Option<Omitted>,
    /// The new-side file mode, e.g. `0o100644`. Synthesized patches need it:
    /// a creation or deletion header without a mode line is rejected.
    pub mode: u32,
    /// Both versions, when the file is an image small enough to show.
    pub image: Option<ImagePreview>,
    /// Set when the file is stored in Git LFS, so the view can describe the
    /// real object instead of the pointer that stands in for it.
    pub lfs: Option<super::lfs::LfsChange>,
}

impl FileDiff {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn parent_dir(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[..i],
            None => "",
        }
    }

    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiffModel {
    pub files: Vec<FileDiff>,
    pub additions: usize,
    pub deletions: usize,
    /// True when the diffed commit has more than one parent, so the view can
    /// say it is showing first-parent changes only.
    pub is_merge_first_parent: bool,
}

impl DiffModel {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn find(&self, path: &str) -> Option<&FileDiff> {
        self.files.iter().find(|f| f.path == path)
    }
}

fn options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.context_lines(3)
        .interhunk_lines(1)
        .include_typechange(true)
        .include_typechange_trees(true)
        // Untracked files are shown as full additions rather than a bare
        // "untracked" marker; seeing the content is the point.
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    opts
}

/// Enable rename and copy detection.
///
/// Without this a moved file appears as an unrelated delete plus add, which is
/// precisely the noise that makes reviewing a refactor miserable.
fn detect_renames(diff: &mut git2::Diff<'_>) -> Result<()> {
    let mut find = git2::DiffFindOptions::new();
    find.renames(true)
        .copies(true)
        .rename_limit(1_000)
        .renames_from_rewrites(true);
    diff.find_similar(Some(&mut find))?;
    Ok(())
}

pub fn build(
    repo: &Repository,
    target: DiffTarget,
    theme: super::highlight::HighlightTheme,
    cancel: &Cancel,
) -> Result<Arc<DiffModel>> {
    let mut opts = options();
    let mut is_merge_first_parent = false;

    let mut diff = match target {
        DiffTarget::Commit(oid) => {
            let commit = repo.find_commit(oid)?;
            is_merge_first_parent = commit.parent_count() > 1;
            let new_tree = commit.tree()?;
            // A root commit has no parent tree; diffing against `None` shows the
            // whole tree as additions, which is what a first commit *is*.
            let old_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))?
        }
        DiffTarget::Staged => {
            let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
        }
        DiffTarget::Unstaged => repo.diff_index_to_workdir(None, Some(&mut opts))?,
    };

    detect_renames(&mut diff)?;

    // Working-tree diffs have no blob for the new side, so image previews have
    // to read from disk instead.
    let from_workdir = target == DiffTarget::Unstaged;

    let mut files = Vec::new();
    let (mut additions, mut deletions) = (0usize, 0usize);

    for index in 0..diff.deltas().len() {
        if index % 32 == 0 {
            cancel.check()?;
        }
        let Some(delta) = diff.get_delta(index) else {
            continue;
        };

        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| *p != path);

        let status = map_delta(delta.status());

        if delta.status() == GitDelta::Unmodified {
            continue;
        }

        let mut file = FileDiff {
            path,
            old_path,
            status,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            omitted: None,
            mode: file_mode(&delta),
            image: None,
            lfs: None,
        };

        if matches!(delta.new_file().mode(), git2::FileMode::Commit) {
            file.omitted = Some(Omitted::Submodule);
            files.push(file);
            continue;
        }

        // An LFS pointer is a three-line text file, so it would otherwise
        // diff as ordinary text — accurate about the pointer and silent about
        // the object, which is the only part anyone cares about.
        file.lfs = lfs_change(repo, &delta, &file.path, from_workdir);
        if let Some(lfs) = &file.lfs {
            file.image = lfs_image_preview(repo, &file.path, lfs);
            files.push(file);
            continue;
        }

        // `Patch::from_diff` returns None for deltas with no textual content,
        // such as a pure mode change.
        let patch = match Patch::from_diff(&diff, index)? {
            Some(p) => p,
            None => {
                if is_binary(repo, &delta, None) {
                    file.omitted = Some(Omitted::Binary);
                    file.image = image_preview(repo, &delta, &file.path, from_workdir);
                }
                files.push(file);
                continue;
            }
        };

        // The binary flag on the delta is only populated once libgit2 has
        // actually looked at the content, which happens during patch
        // generation — so this check has to come *after* `from_diff`, not
        // before it.
        if is_binary(repo, &delta, Some(&patch)) {
            file.omitted = Some(Omitted::Binary);
            // A changed image is binary, but "binary file, no diff" is a
            // useless thing to say about a picture.
            file.image = image_preview(repo, &delta, &file.path, from_workdir);
            files.push(file);
            continue;
        }

        let mut total_lines = 0usize;
        for h in 0..patch.num_hunks() {
            let (hunk, line_count) = patch.hunk(h)?;
            total_lines += line_count;
            if total_lines > MAX_FILE_LINES {
                file.hunks.clear();
                file.omitted = Some(Omitted::TooLarge);
                break;
            }

            let mut lines = Vec::with_capacity(line_count);
            for l in 0..line_count {
                let line = patch.line_in_hunk(h, l)?;
                let Some(kind) = LineKind::from_origin(line.origin()) else {
                    continue;
                };
                let (content, truncated) = decode(line.content());
                match kind {
                    LineKind::Addition => file.additions += 1,
                    LineKind::Deletion => file.deletions += 1,
                    _ => {}
                }
                let display = super::highlight::expand_tabs(&content);
                lines.push(DiffLine {
                    kind,
                    old_lineno: line.old_lineno(),
                    new_lineno: line.new_lineno(),
                    content,
                    display,
                    truncated,
                    spans: Vec::new(),
                    emphasis: Vec::new(),
                });
            }

            file.hunks.push(Hunk {
                header: decode(hunk.header()).0,
                old_start: hunk.old_start(),
                old_lines: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_lines: hunk.new_lines(),
                lines,
            });
        }

        super::inline::apply(&mut file.hunks);
        super::highlight::apply(&file.path, &mut file.hunks, theme);

        additions += file.additions;
        deletions += file.deletions;
        files.push(file);
    }

    // Path order, so the file list doesn't reshuffle between refreshes.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Arc::new(DiffModel {
        files,
        additions,
        deletions,
        is_merge_first_parent,
    }))
}

/// Decode diff content, which git treats as bytes with no declared encoding.
///
/// Invalid UTF-8 is replaced rather than rejected: a file with one bad byte is
/// still worth showing.
fn decode(bytes: &[u8]) -> (String, bool) {
    let trimmed = bytes
        .strip_suffix(b"\n")
        .map(|b| b.strip_suffix(b"\r").unwrap_or(b))
        .unwrap_or(bytes);

    if trimmed.len() > MAX_LINE_LEN {
        let cut = floor_char_boundary(trimmed, MAX_LINE_LEN);
        (String::from_utf8_lossy(&trimmed[..cut]).into_owned(), true)
    } else {
        (String::from_utf8_lossy(trimmed).into_owned(), false)
    }
}

/// Largest index `<= max` that does not split a UTF-8 sequence.
fn floor_char_boundary(bytes: &[u8], max: usize) -> usize {
    let mut i = max.min(bytes.len());
    while i > 0 && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

/// Read both sides as LFS pointers, if that is what they are.
fn lfs_change(
    repo: &Repository,
    delta: &git2::DiffDelta<'_>,
    path: &str,
    from_workdir: bool,
) -> Option<super::lfs::LfsChange> {
    let blob_bytes = |id: git2::Oid| -> Option<Vec<u8>> {
        (!id.is_zero())
            .then(|| repo.find_blob(id).ok())
            .flatten()
            .map(|b| b.content().to_vec())
    };

    let old = blob_bytes(delta.old_file().id());
    let new = if from_workdir {
        // A working-tree diff has no blob for the new side.
        repo.workdir()
            .map(|w| w.join(path))
            .and_then(|p| std::fs::read(p).ok())
    } else {
        blob_bytes(delta.new_file().id())
    };

    super::lfs::change(old.as_deref(), new.as_deref(), repo.path())
}

/// If an LFS-tracked image has been downloaded, show it like any other image.
///
/// Without this, an LFS-tracked PNG is strictly worse to review than a plain
/// one, which defeats the point of tracking it.
fn lfs_image_preview(
    repo: &Repository,
    path: &str,
    lfs: &super::lfs::LfsChange,
) -> Option<ImagePreview> {
    if !is_image_path(path) {
        return None;
    }
    let git_dir = repo.path();
    let side = |pointer: Option<&super::lfs::Pointer>| -> Option<ImageSide> {
        let pointer = pointer?;
        let object = pointer.object_path(git_dir)?;
        let bytes = std::fs::read(object).ok()?;
        image_side(bytes)
    };

    let old = side(lfs.old.as_ref());
    let new = side(lfs.new.as_ref());
    (old.is_some() || new.is_some()).then_some(ImagePreview { old, new })
}

/// Whether a delta's content is binary.
///
/// Three sources, because no single one is reliable across diff kinds: the
/// delta flag (set only after content inspection), the patch's own delta (the
/// same flag, but populated), and finally the blob itself, which is what
/// catches tree-to-tree diffs where nothing forced an inspection.
fn is_binary(repo: &Repository, delta: &git2::DiffDelta<'_>, patch: Option<&Patch<'_>>) -> bool {
    if delta.flags().is_binary() {
        return true;
    }
    if patch.is_some_and(|p| p.delta().flags().is_binary()) {
        return true;
    }
    for file in [delta.new_file(), delta.old_file()] {
        if file.id().is_zero() {
            continue;
        }
        if repo.find_blob(file.id()).map(|b| b.is_binary()) == Ok(true) {
            return true;
        }
    }
    false
}

/// The file's mode, preferring the new side and falling back to the old for
/// deletions. Defaults to a regular file when git reports something unusable.
fn file_mode(delta: &git2::DiffDelta<'_>) -> u32 {
    for file in [delta.new_file(), delta.old_file()] {
        let mode = file.mode();
        if matches!(
            mode,
            git2::FileMode::Blob | git2::FileMode::BlobExecutable | git2::FileMode::Link
        ) {
            return mode_bits(mode);
        }
    }
    0o100_644
}

fn mode_bits(mode: git2::FileMode) -> u32 {
    match mode {
        git2::FileMode::BlobExecutable => 0o100_755,
        git2::FileMode::Link => 0o120_000,
        _ => 0o100_644,
    }
}

fn map_delta(status: GitDelta) -> Delta {
    match status {
        GitDelta::Added => Delta::Added,
        GitDelta::Deleted => Delta::Deleted,
        GitDelta::Modified => Delta::Modified,
        GitDelta::Renamed => Delta::Renamed,
        GitDelta::Copied => Delta::Copied,
        GitDelta::Typechange => Delta::TypeChange,
        GitDelta::Untracked => Delta::Untracked,
        GitDelta::Ignored => Delta::Ignored,
        GitDelta::Conflicted => Delta::Conflicted,
        GitDelta::Unmodified | GitDelta::Unreadable => Delta::Unmodified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_strips_line_endings() {
        assert_eq!(decode(b"hello\n").0, "hello");
        assert_eq!(decode(b"hello\r\n").0, "hello");
        assert_eq!(decode(b"hello").0, "hello");
    }

    #[test]
    fn decode_truncates_without_splitting_utf8() {
        let long = "é".repeat(MAX_LINE_LEN);
        let (text, truncated) = decode(long.as_bytes());
        assert!(truncated);
        // Valid UTF-8 out, never a replacement character from a split sequence.
        assert!(!text.contains('\u{FFFD}'));
        assert!(text.len() <= MAX_LINE_LEN);
    }

    #[test]
    fn decode_replaces_invalid_bytes() {
        let (text, _) = decode(&[b'a', 0xFF, b'b']);
        assert!(text.starts_with('a') && text.ends_with('b'));
    }
}

#[cfg(test)]
mod pairing_tests {
    use super::*;

    fn line(kind: LineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            old_lineno: None,
            new_lineno: None,
            content: content.to_owned(),
            display: None,
            truncated: false,
            spans: Vec::new(),
            emphasis: Vec::new(),
        }
    }

    fn hunk(lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            header: "@@".to_owned(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 0,
            lines,
        }
    }

    #[test]
    fn context_lines_occupy_both_columns() {
        let h = hunk(vec![
            line(LineKind::Context, "a"),
            line(LineKind::Context, "b"),
        ]);
        let pairs = h.pair_lines();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|p| p.left == p.right && p.left.is_some()));
    }

    #[test]
    fn equal_runs_pair_one_to_one() {
        let h = hunk(vec![
            line(LineKind::Deletion, "old1"),
            line(LineKind::Deletion, "old2"),
            line(LineKind::Addition, "new1"),
            line(LineKind::Addition, "new2"),
        ]);
        let pairs = h.pair_lines();
        assert_eq!(pairs.len(), 2, "two rows, not four");
        assert_eq!(
            pairs[0],
            Pair {
                left: Some(0),
                right: Some(2)
            }
        );
        assert_eq!(
            pairs[1],
            Pair {
                left: Some(1),
                right: Some(3)
            }
        );
    }

    #[test]
    fn a_longer_run_leaves_blanks_opposite_the_surplus() {
        let h = hunk(vec![
            line(LineKind::Deletion, "old"),
            line(LineKind::Addition, "new1"),
            line(LineKind::Addition, "new2"),
        ]);
        let pairs = h.pair_lines();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].left, Some(0));
        assert_eq!(pairs[1].left, None, "nothing on the old side to show");
        assert_eq!(pairs[1].right, Some(2));
    }

    #[test]
    fn additions_with_no_deletions_only_fill_the_right() {
        let h = hunk(vec![
            line(LineKind::Context, "keep"),
            line(LineKind::Addition, "added"),
        ]);
        let pairs = h.pair_lines();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1].left, None);
        assert_eq!(pairs[1].right, Some(1));
    }

    #[test]
    fn deletions_with_no_additions_only_fill_the_left() {
        let h = hunk(vec![
            line(LineKind::Deletion, "gone"),
            line(LineKind::Context, "keep"),
        ]);
        let pairs = h.pair_lines();
        assert_eq!(pairs[0].right, None);
        assert_eq!(pairs[0].left, Some(0));
    }

    #[test]
    fn every_line_appears_exactly_once() {
        let h = hunk(vec![
            line(LineKind::Context, "a"),
            line(LineKind::Deletion, "b"),
            line(LineKind::Deletion, "c"),
            line(LineKind::Addition, "B"),
            line(LineKind::Context, "d"),
            line(LineKind::Addition, "e"),
        ]);
        let pairs = h.pair_lines();
        let mut seen: Vec<usize> = pairs
            .iter()
            .flat_map(|p| {
                // Context contributes the same index to both columns.
                match (p.left, p.right) {
                    (Some(l), Some(r)) if l == r => vec![l],
                    (l, r) => l.into_iter().chain(r).collect(),
                }
            })
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5], "no line lost or duplicated");
    }

    #[test]
    fn an_empty_hunk_produces_no_rows() {
        assert!(hunk(Vec::new()).pair_lines().is_empty());
    }
}
