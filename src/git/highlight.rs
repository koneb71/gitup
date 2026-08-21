//! Syntax highlighting for diff content.
//!
//! Runs on a worker as part of building a diff, never on the UI thread: syntect
//! parses text, and parsing a few thousand lines at 60fps is not something a
//! frame can afford.
//!
//! A diff only contains fragments of a file, so highlighting can never be fully
//! correct — a hunk that starts inside a block comment has no way to know that.
//! Each hunk is therefore parsed independently from a clean state, with the old
//! and new sides parsed separately so that a line's colours match the version of
//! the file it belongs to.

use super::diff::{DiffLine, Hunk, LineKind};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// A run of characters sharing one colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Length in bytes, over the line's displayed text.
    pub len: u32,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightTheme {
    Dark,
    Light,
    /// Highlighting disabled.
    Off,
}

/// Loading the syntax and theme sets costs on the order of a hundred
/// milliseconds, so it happens once for the life of the process.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme(kind: HighlightTheme) -> Option<&'static Theme> {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(ThemeSet::load_defaults);
    match kind {
        HighlightTheme::Dark => set.themes.get("base16-ocean.dark"),
        HighlightTheme::Light => set.themes.get("InspiredGitHub"),
        HighlightTheme::Off => None,
    }
}

/// Expand tabs to a four-column stop.
///
/// This lives in the model rather than the renderer because span offsets are
/// byte indices into the text that is actually drawn. Expanding tabs at draw
/// time would shift every offset after the first tab.
pub fn expand_tabs(s: &str) -> Option<String> {
    if !s.contains('\t') {
        return None;
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut column = 0;
    for ch in s.chars() {
        if ch == '\t' {
            let spaces = 4 - (column % 4);
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    Some(out)
}

/// The syntax definition for a path, if one is known.
///
/// Matched by file name first, since that catches `Makefile` and `Dockerfile`
/// as well as extensions.
fn syntax_for(path: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
    let syntax_set = syntaxes();
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .and_then(|(_, ext)| syntax_set.find_syntax_by_extension(ext))
        .or_else(|| syntax_set.find_syntax_by_extension(name))
        .or_else(|| syntax_set.find_syntax_by_name(name))
}

/// Highlight a whole file, one entry per line.
///
/// Unlike a diff, this sees the complete text, so the result is actually
/// correct — a line inside a block comment is coloured as a comment, because
/// the parser reached it the same way the compiler would.
pub fn whole_file<'a>(
    path: &str,
    lines: impl IntoIterator<Item = &'a str>,
    kind: HighlightTheme,
) -> Vec<Vec<Span>> {
    let lines: Vec<&str> = lines.into_iter().collect();
    let (Some(theme), Some(syntax)) = (theme(kind), syntax_for(path)) else {
        return vec![Vec::new(); lines.len()];
    };
    let syntax_set = syntaxes();
    let mut highlighter = HighlightLines::new(syntax, theme);

    lines
        .into_iter()
        .map(|line| {
            let expanded = expand_tabs(line);
            let text = expanded.as_deref().unwrap_or(line);
            highlight_one(&mut highlighter, &format!("{text}\n"), syntax_set)
        })
        .collect()
}

/// Highlight every hunk of a file in place.
pub fn apply(path: &str, hunks: &mut [Hunk], kind: HighlightTheme) {
    let Some(theme) = theme(kind) else { return };
    let syntax_set = syntaxes();
    let Some(syntax) = syntax_for(path) else {
        return;
    };

    for hunk in hunks {
        let mut new_side = HighlightLines::new(syntax, theme);
        let mut old_side = HighlightLines::new(syntax, theme);

        for line in &mut hunk.lines {
            if line.kind == LineKind::NoNewline {
                continue;
            }
            // Context lines exist on both sides and must advance both parsers,
            // or the side that skipped them loses its state.
            let text = display_text(line).to_owned();
            let with_newline = format!("{text}\n");

            let new_spans = (line.kind != LineKind::Deletion)
                .then(|| highlight_one(&mut new_side, &with_newline, syntax_set));
            let old_spans = (line.kind != LineKind::Addition)
                .then(|| highlight_one(&mut old_side, &with_newline, syntax_set));

            line.spans = match line.kind {
                LineKind::Deletion => old_spans.unwrap_or_default(),
                _ => new_spans.unwrap_or_default(),
            };
        }
    }
}

fn highlight_one(
    highlighter: &mut HighlightLines<'_>,
    line: &str,
    syntax_set: &SyntaxSet,
) -> Vec<Span> {
    let Ok(ranges) = highlighter.highlight_line(line, syntax_set) else {
        return Vec::new();
    };
    let mut spans: Vec<Span> = Vec::with_capacity(ranges.len());
    for (style, text) in ranges {
        // Drop the trailing newline we added; it is not part of the display.
        let text = text.strip_suffix('\n').unwrap_or(text);
        if text.is_empty() {
            continue;
        }
        let color = [style.foreground.r, style.foreground.g, style.foreground.b];
        // Merge adjacent runs of the same colour: syntect emits many one-token
        // ranges, and each extra span becomes an extra layout section.
        match spans.last_mut() {
            Some(last) if last.color == color => last.len += text.len() as u32,
            _ => spans.push(Span {
                len: text.len() as u32,
                color,
            }),
        }
    }
    spans
}

/// The text a line actually renders — tab-expanded when it needed it.
pub fn display_text(line: &DiffLine) -> &str {
    line.display.as_deref().unwrap_or(&line.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(expand_tabs("\tx").as_deref(), Some("    x"));
        assert_eq!(expand_tabs("a\tb").as_deref(), Some("a   b"));
        assert_eq!(expand_tabs("abcd\te").as_deref(), Some("abcd    e"));
        assert_eq!(expand_tabs("no tabs"), None, "no allocation when unneeded");
    }

    #[test]
    fn spans_cover_the_whole_line() {
        let mut hunks = vec![Hunk {
            header: "@@".to_owned(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 1,
            lines: vec![DiffLine {
                kind: LineKind::Addition,
                old_lineno: None,
                new_lineno: Some(1),
                content: "let x = 1; // note".to_owned(),
                display: None,
                truncated: false,
                spans: Vec::new(),
                emphasis: Vec::new(),
            }],
        }];
        apply("main.rs", &mut hunks, HighlightTheme::Dark);

        let line = &hunks[0].lines[0];
        assert!(!line.spans.is_empty(), "Rust source should highlight");
        let covered: u32 = line.spans.iter().map(|s| s.len).sum();
        assert_eq!(
            covered as usize,
            line.content.len(),
            "spans must tile the line exactly, or the renderer will drop text"
        );
        assert!(
            line.spans
                .iter()
                .map(|s| s.color)
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1,
            "a keyword and a comment should not be the same colour"
        );
    }

    #[test]
    fn a_whole_file_is_highlighted_with_correct_state() {
        // The third line is inside a block comment. A per-fragment highlighter
        // would miss that; a whole-file pass must not.
        let lines = [
            "fn main() {",
            "    /* a block comment",
            "       still the comment */",
            "    let x = 1;",
            "}",
        ];
        let spans = whole_file("main.rs", lines.iter().copied(), HighlightTheme::Dark);
        assert_eq!(spans.len(), 5);

        let comment_colour = spans[1].last().expect("colour on line 2").color;
        let continued = spans[2].first().expect("colour on line 3").color;
        assert_eq!(
            comment_colour, continued,
            "the comment's colour must carry across the line break"
        );

        for (index, line) in lines.iter().enumerate() {
            let covered: u32 = spans[index].iter().map(|s| s.len).sum();
            let expected = expand_tabs(line).unwrap_or_else(|| (*line).to_owned());
            assert_eq!(covered as usize, expected.len(), "line {index} not tiled");
        }
    }

    #[test]
    fn a_whole_file_of_unknown_type_yields_empty_spans() {
        let spans = whole_file("x.zzzznope", ["a", "b"], HighlightTheme::Dark);
        assert_eq!(spans.len(), 2);
        assert!(spans.iter().all(Vec::is_empty));
    }

    #[test]
    fn unknown_file_types_are_left_alone() {
        let mut hunks = vec![Hunk {
            header: "@@".to_owned(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 1,
            lines: vec![DiffLine {
                kind: LineKind::Addition,
                old_lineno: None,
                new_lineno: Some(1),
                content: "anything".to_owned(),
                display: None,
                truncated: false,
                spans: Vec::new(),
                emphasis: Vec::new(),
            }],
        }];
        apply("file.zzzznotathing", &mut hunks, HighlightTheme::Dark);
        assert!(hunks[0].lines[0].spans.is_empty());
    }
}
