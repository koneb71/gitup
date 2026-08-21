//! Intra-line ("word") diff.
//!
//! A whole-line add/remove tint tells you *that* a line changed. When the change
//! is one token in a long line, that is close to useless — the eye still has to
//! compare the two lines character by character. This module marks the parts
//! that actually differ, so `strict: false` → `strict: true` reads as two words
//! rather than two lines.

use super::diff::{Hunk, LineKind};

/// A run of characters that differs from the paired line, as character indices
/// into the line's displayed text.
///
/// Character indices rather than byte offsets because the renderer positions
/// them by multiplying by the monospace advance width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emphasis {
    pub start: usize,
    pub end: usize,
}

/// Above this fraction of a line being different, marking the differences stops
/// helping: the line was rewritten, and highlighting nearly all of it is just a
/// second, noisier way of saying "this line changed".
const MAX_CHANGED_FRACTION: f32 = 0.55;

/// Split a line into comparison tokens, returning each token with the character
/// index it starts at.
///
/// Identifier-ish runs stay whole so that renaming `count` to `total` marks two
/// words instead of a scatter of letters; everything else is per-character so
/// punctuation changes stay tight.
fn tokenize(text: &str) -> Vec<(usize, String)> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;

    for (index, ch) in text.chars().enumerate() {
        let word_char = ch.is_alphanumeric() || ch == '_';
        if word_char {
            if current.is_empty() {
                start = index;
            }
            current.push(ch);
        } else {
            if !current.is_empty() {
                tokens.push((start, std::mem::take(&mut current)));
            }
            tokens.push((index, ch.to_string()));
        }
    }
    if !current.is_empty() {
        tokens.push((start, current));
    }
    tokens
}

/// Character length of a token list, for computing the changed fraction.
fn char_len(tokens: &[(usize, String)]) -> usize {
    tokens.iter().map(|(_, t)| t.chars().count()).sum()
}

/// Compute emphasis ranges for one pair of lines.
fn compare(old: &str, new: &str) -> (Vec<Emphasis>, Vec<Emphasis>) {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let old_words: Vec<&str> = old_tokens.iter().map(|(_, t)| t.as_str()).collect();
    let new_words: Vec<&str> = new_tokens.iter().map(|(_, t)| t.as_str()).collect();

    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old_words, &new_words);

    let mut old_marks = Vec::new();
    let mut new_marks = Vec::new();
    let (mut old_changed, mut new_changed) = (0usize, 0usize);

    for op in ops {
        use similar::DiffOp;
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                old_changed += push_range(&mut old_marks, &old_tokens, old_index, old_len);
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                new_changed += push_range(&mut new_marks, &new_tokens, new_index, new_len);
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                old_changed += push_range(&mut old_marks, &old_tokens, old_index, old_len);
                new_changed += push_range(&mut new_marks, &new_tokens, new_index, new_len);
            }
        }
    }

    let old_total = char_len(&old_tokens).max(1);
    let new_total = char_len(&new_tokens).max(1);
    let too_different = old_changed as f32 / old_total as f32 > MAX_CHANGED_FRACTION
        && new_changed as f32 / new_total as f32 > MAX_CHANGED_FRACTION;
    if too_different {
        return (Vec::new(), Vec::new());
    }

    (merge_adjacent(old_marks), merge_adjacent(new_marks))
}

/// Record the character span covered by `tokens[index..index + len]`, returning
/// how many characters it covers.
fn push_range(
    out: &mut Vec<Emphasis>,
    tokens: &[(usize, String)],
    index: usize,
    len: usize,
) -> usize {
    if len == 0 || index >= tokens.len() {
        return 0;
    }
    let end_index = (index + len - 1).min(tokens.len() - 1);
    let start = tokens[index].0;
    let end = tokens[end_index].0 + tokens[end_index].1.chars().count();
    out.push(Emphasis { start, end });
    end - start
}

/// Join ranges that touch, so one highlight is drawn instead of several.
fn merge_adjacent(mut marks: Vec<Emphasis>) -> Vec<Emphasis> {
    marks.sort_by_key(|m| m.start);
    let mut merged: Vec<Emphasis> = Vec::with_capacity(marks.len());
    for mark in marks {
        match merged.last_mut() {
            Some(last) if mark.start <= last.end => last.end = last.end.max(mark.end),
            _ => merged.push(mark),
        }
    }
    merged
}

/// Annotate every hunk's changed lines in place.
///
/// Deletions and the additions that immediately follow them are paired by
/// position. That is the shape of almost every real edit — n lines replaced by
/// n lines — and when the counts don't match, only the overlapping prefix is
/// paired rather than inventing correspondences.
pub fn apply(hunks: &mut [Hunk]) {
    for hunk in hunks {
        let mut index = 0;
        while index < hunk.lines.len() {
            if hunk.lines[index].kind != LineKind::Deletion {
                index += 1;
                continue;
            }

            let del_start = index;
            while index < hunk.lines.len() && hunk.lines[index].kind == LineKind::Deletion {
                index += 1;
            }
            let del_end = index;

            let add_start = index;
            while index < hunk.lines.len() && hunk.lines[index].kind == LineKind::Addition {
                index += 1;
            }
            let add_end = index;

            let pairs = (del_end - del_start).min(add_end - add_start);
            for offset in 0..pairs {
                let old_text = hunk.lines[del_start + offset].display_text().to_owned();
                let new_text = hunk.lines[add_start + offset].display_text().to_owned();
                let (old_marks, new_marks) = compare(&old_text, &new_text);
                hunk.lines[del_start + offset].emphasis = old_marks;
                hunk.lines[add_start + offset].emphasis = new_marks;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Ranges = Vec<(usize, usize)>;

    fn marks(old: &str, new: &str) -> (Ranges, Ranges) {
        let (o, n) = compare(old, new);
        (
            o.iter().map(|m| (m.start, m.end)).collect(),
            n.iter().map(|m| (m.start, m.end)).collect(),
        )
    }

    #[test]
    fn a_single_word_change_marks_only_that_word() {
        let old = "    strict: false,";
        let new = "    strict: true,";
        let (o, n) = marks(old, new);
        assert_eq!(o.len(), 1);
        assert_eq!(n.len(), 1);
        assert_eq!(&old[o[0].0..o[0].1], "false");
        assert_eq!(&new[n[0].0..n[0].1], "true");
    }

    #[test]
    fn identical_lines_have_nothing_to_mark() {
        let (o, n) = marks("same text", "same text");
        assert!(o.is_empty() && n.is_empty());
    }

    #[test]
    fn an_insertion_marks_only_the_new_side() {
        let old = "let x = 1;";
        let new = "let mut x = 1;";
        let (o, n) = marks(old, new);
        assert!(o.is_empty(), "nothing was removed");
        assert_eq!(n.len(), 1);
        assert!(new[n[0].0..n[0].1].contains("mut"));
    }

    #[test]
    fn wholly_rewritten_lines_are_left_unmarked() {
        // Marking ~everything is just a noisier way of saying "this changed".
        let (o, n) = marks("alpha beta gamma", "one two three four");
        assert!(o.is_empty() && n.is_empty());
    }

    #[test]
    fn adjacent_changes_merge_into_one_run() {
        let (_, n) = marks("a = 1", "a = 22");
        assert_eq!(n.len(), 1, "touching ranges should not be drawn separately");
    }

    #[test]
    fn tokenizer_keeps_identifiers_whole() {
        let tokens = tokenize("foo_bar(baz)");
        let words: Vec<&str> = tokens.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(words, vec!["foo_bar", "(", "baz", ")"]);
        assert_eq!(tokens[2].0, 8, "character index of `baz`");
    }

    #[test]
    fn multibyte_text_uses_character_indices() {
        let old = "let café = 1;";
        let new = "let café = 2;";
        let (_, n) = marks(old, new);
        assert_eq!(n.len(), 1);
        // Character indices, so `é` counts as one — a byte offset would be off.
        assert_eq!(new.chars().skip(n[0].0).take(1).next(), Some('2'));
    }
}
