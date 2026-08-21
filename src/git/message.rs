//! Drafting a commit message from what is staged.
//!
//! This reads the staged diff and writes a first draft. It is deliberately a
//! set of rules rather than a model: it runs offline, instantly, with no key to
//! configure and nothing leaving the machine, and — more importantly — its
//! output is predictable enough to correct. A draft you can trust to be *wrong
//! in the same way every time* is easier to fix than one that is plausibly
//! wrong in a new way each time.
//!
//! It cannot know *why* a change was made, so it does not pretend to. It
//! describes what changed and leaves the reasoning to the person, who is
//! already looking at an editable text box.
//!
//! The one thing it does try to get right is *house style*: a repository whose
//! history is `feat(parser): handle empty input` gets a draft in that shape,
//! and one whose history is `Add empty-input handling` gets that instead.
//! Matching the surrounding history is most of what makes a generated message
//! feel like it belongs.

use super::diff::{DiffModel, FileDiff, LineKind};
use super::status::Delta;
use crate::util::words::plural;

/// The subject-line convention a repository already follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    /// `feat(scope): lower case summary`
    Conventional,
    /// `Capitalized imperative summary`
    Plain,
}

impl Convention {
    /// Read the convention off recent subject lines.
    ///
    /// Merge commits are excluded: `Merge branch 'x'` is git's wording, not the
    /// project's, and a busy history has enough of them to swing the vote.
    pub fn detect<'a>(subjects: impl IntoIterator<Item = &'a str>) -> Self {
        let considered: Vec<&str> = subjects
            .into_iter()
            .filter(|s| !s.starts_with("Merge ") && !s.starts_with("Revert "))
            .take(SUBJECTS_SAMPLED)
            .collect();

        if considered.is_empty() {
            return Self::Plain;
        }

        let conventional = considered.iter().filter(|s| is_conventional(s)).count();
        // A simple majority. Repositories drift, and a handful of stray
        // subjects should not outvote a history that is plainly one style.
        if conventional * 2 > considered.len() {
            Self::Conventional
        } else {
            Self::Plain
        }
    }
}

/// How many recent subjects are read to decide the convention.
const SUBJECTS_SAMPLED: usize = 30;

/// `type: summary`, `type(scope): summary`, and the `!` breaking-change marker.
fn is_conventional(subject: &str) -> bool {
    let Some((head, rest)) = subject.split_once(": ") else {
        return false;
    };
    if rest.trim().is_empty() || head.len() > 40 {
        return false;
    }

    let head = head.strip_suffix('!').unwrap_or(head);
    let kind = match head.split_once('(') {
        Some((kind, scope)) => {
            if !scope.ends_with(')') || scope.len() < 2 {
                return false;
            }
            kind
        }
        None => head,
    };

    !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase())
}

/// A drafted message, before it becomes text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub subject: String,
    /// One line per entry. Empty when the change is small enough to speak for
    /// itself.
    pub body: Vec<String>,
}

impl Draft {
    /// The message as it goes into the editor: subject, blank line, body.
    pub fn render(&self) -> String {
        if self.body.is_empty() {
            return self.subject.clone();
        }
        format!("{}\n\n{}", self.subject, self.body.join("\n"))
    }
}

/// Draft a message for `diff`, or `None` when there is nothing staged.
pub fn draft(diff: &DiffModel, convention: Convention) -> Option<Draft> {
    if diff.files.is_empty() {
        return None;
    }

    let subject = match convention {
        // Nothing carries the location, so the summary has to.
        Convention::Plain => capitalize(&summarize(&diff.files, Location::Include)),
        Convention::Conventional => {
            let kind = kind_of(&diff.files);
            match scope_of(&diff.files) {
                // The scope already says where; repeating it would read
                // "fix(ui): update 2 files in ui".
                Some(scope) => {
                    format!(
                        "{kind}({scope}): {}",
                        summarize(&diff.files, Location::Omit)
                    )
                }
                None => format!("{kind}: {}", summarize(&diff.files, Location::Include)),
            }
        }
    };

    Some(Draft {
        subject,
        body: body_for(&diff.files),
    })
}

// ---------------------------------------------------------------------------
// What kind of change this is
// ---------------------------------------------------------------------------

/// The part of a project a path belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Docs,
    Tests,
    Ci,
    Build,
    Assets,
    Source,
}

fn area(path: &str) -> Area {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let segments: Vec<&str> = lower.split('/').collect();
    let has = |dir: &str| segments.contains(&dir);

    if lower.starts_with(".github/workflows/")
        || matches!(
            name,
            ".gitlab-ci.yml" | ".travis.yml" | "jenkinsfile" | "azure-pipelines.yml"
        )
    {
        return Area::Ci;
    }

    // Tests before docs and source: `tests/fixtures/README.md` is test data,
    // and `src/foo_test.rs` is a test however it is filed.
    if has("tests") || has("test") || has("spec") || has("__tests__") {
        return Area::Tests;
    }
    if name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
    {
        return Area::Tests;
    }

    if matches!(
        name,
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "makefile"
            | "dockerfile"
            | "build.rs"
            | "pyproject.toml"
            | "setup.py"
            | "requirements.txt"
            | "go.mod"
            | "go.sum"
            | "cmakelists.txt"
            | "rustfmt.toml"
            | ".editorconfig"
            | ".gitignore"
    ) || name.ends_with(".gradle")
    {
        return Area::Build;
    }

    if has("docs") || has("doc") {
        return Area::Docs;
    }
    if extension(&lower).is_some_and(|e| matches!(e, "md" | "rst" | "adoc" | "txt")) {
        return Area::Docs;
    }
    if matches!(name, "license" | "licence" | "notice" | "authors") {
        return Area::Docs;
    }

    if extension(&lower).is_some_and(|e| {
        matches!(
            e,
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "webp"
                | "ico"
                | "icns"
                | "ttf"
                | "otf"
                | "woff"
                | "woff2"
                | "mp3"
                | "mp4"
                | "wav"
        )
    }) {
        return Area::Assets;
    }

    Area::Source
}

fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    name.rsplit_once('.').map(|(_, ext)| ext)
}

/// The conventional-commit type.
///
/// `feat` and `fix` are the two this genuinely cannot tell apart — both are
/// edits to existing source — so the tie-break is that adding a file is far
/// more often a new capability, and editing files in place is far more often a
/// correction. It is a guess, it is labelled as one in the docs, and it is two
/// keystrokes to change in the box.
fn kind_of(files: &[FileDiff]) -> &'static str {
    let areas: Vec<Area> = files.iter().map(|f| area(&f.path)).collect();
    let all = |want: Area| areas.iter().all(|a| *a == want);

    if all(Area::Docs) {
        return "docs";
    }
    if all(Area::Tests) {
        return "test";
    }
    if all(Area::Ci) {
        return "ci";
    }
    if all(Area::Build) {
        return "build";
    }
    if all(Area::Assets) {
        return "chore";
    }

    let source = |f: &&FileDiff| area(&f.path) == Area::Source;
    let added = files
        .iter()
        .filter(source)
        .any(|f| matches!(f.status, Delta::Added | Delta::Copied | Delta::Untracked));
    if added {
        return "feat";
    }

    let restructured = files
        .iter()
        .filter(source)
        .all(|f| matches!(f.status, Delta::Deleted | Delta::Renamed));
    if restructured {
        return "refactor";
    }

    "fix"
}

/// The conventional-commit scope: the most specific directory every change
/// shares, minus the container directories every project has.
fn scope_of(files: &[FileDiff]) -> Option<String> {
    let common = common_directory(files.iter().map(|f| f.path.as_str()))?;
    let last = common.split('/').rfind(|s| {
        !s.is_empty() && !matches!(*s, "src" | "lib" | "app" | "source" | "internal" | "pkg")
    })?;

    // A scope has to be shorter than the summary to be worth having.
    (last.len() <= 20).then(|| last.to_owned())
}

/// The deepest directory shared by every path, `""` when they share only the
/// repository root.
fn common_directory<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut shared: Option<Vec<&str>> = None;
    for path in paths {
        let directories: Vec<&str> = path.split('/').collect();
        // The file name itself is not a directory.
        let directories = &directories[..directories.len().saturating_sub(1)];
        shared = Some(match shared {
            None => directories.to_vec(),
            Some(current) => current
                .iter()
                .zip(directories)
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    shared.map(|parts| parts.join("/"))
}

// ---------------------------------------------------------------------------
// What to say about it
// ---------------------------------------------------------------------------

/// The most files a subject line will name one by one before counting instead.
const NAMED_LIMIT: usize = 3;

/// `a`, `a and b`, `a, b and c`.
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Whether the summary should name where the change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Include,
    /// Something else in the subject already says it.
    Omit,
}

/// The subject's summary, lower case and imperative: "add the parser",
/// "update 3 files in src/ui".
fn summarize(files: &[FileDiff], location: Location) -> String {
    if let [only] = files {
        return summarize_one(only);
    }

    let added = files
        .iter()
        .filter(|f| matches!(f.status, Delta::Added | Delta::Untracked))
        .count();
    let deleted = files.iter().filter(|f| f.status == Delta::Deleted).count();
    let renamed = files.iter().filter(|f| f.status == Delta::Renamed).count();
    let n = files.len();

    // Few enough to name: "update README.md and lib.rs" says everything
    // "update 2 files" says, and then says which. Counting is what you fall
    // back to when the list would no longer fit in a subject line.
    if n <= NAMED_LIMIT {
        let verb = if added == n {
            "add"
        } else if deleted == n {
            "remove"
        } else if renamed == n {
            "move"
        } else {
            "update"
        };
        let names: Vec<&str> = files.iter().map(|f| f.file_name()).collect();
        return format!("{verb} {}", join_names(&names));
    }

    // The full shared directory rather than the short scope: with nothing
    // else in the subject to place it, "in src/ui" is more use than "in ui".
    let where_ = match location {
        Location::Omit => String::new(),
        Location::Include => common_directory(files.iter().map(|f| f.path.as_str()))
            .filter(|d| !d.is_empty())
            .map(|d| format!(" in {d}"))
            .unwrap_or_default(),
    };

    if added == n {
        return format!("add {}{where_}", plural(n, "file"));
    }
    if deleted == n {
        return format!("remove {}{where_}", plural(n, "file"));
    }
    if renamed == n {
        return format!("move {}{where_}", plural(n, "file"));
    }
    format!("update {}{where_}", plural(n, "file"))
}

fn summarize_one(file: &FileDiff) -> String {
    let name = file.file_name();
    match file.status {
        Delta::Added | Delta::Copied | Delta::Untracked => format!("add {name}"),
        Delta::Deleted => format!("remove {name}"),
        Delta::Renamed => match &file.old_path {
            Some(old) => {
                let old_name = old.rsplit('/').next().unwrap_or(old);
                format!("move {old_name} to {name}")
            }
            None => format!("move {name}"),
        },
        _ => match added_definitions(file) {
            // Naming what appeared is far more useful than "update foo.rs",
            // and it is the one thing the diff text can be read for without
            // guessing at intent.
            Some(names) => format!("add {names} to {name}"),
            None => format!("update {name}"),
        },
    }
}

/// Named definitions introduced by this diff, when they are the whole story.
///
/// Only for changes that are purely additions: once lines have been removed,
/// something was reworked rather than added, and saying "add x" would be wrong.
fn added_definitions(file: &FileDiff) -> Option<String> {
    if file.deletions > 0 || file.additions == 0 {
        return None;
    }

    let mut names = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            if line.kind != LineKind::Addition {
                continue;
            }
            if let Some(name) = definition_name(&line.content) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            if names.len() > MAX_DEFINITIONS {
                // More than a handful and listing them stops being a summary.
                return None;
            }
        }
    }

    match names.len() {
        0 => None,
        1 => Some(names.remove_first()),
        _ => {
            let last = names.pop()?;
            Some(format!("{} and {last}", names.join(", ")))
        }
    }
}

const MAX_DEFINITIONS: usize = 3;

/// Convenience so the single-name case reads without an index.
trait RemoveFirst {
    fn remove_first(self) -> String;
}

impl RemoveFirst for Vec<String> {
    fn remove_first(mut self) -> String {
        self.remove(0)
    }
}

/// The name a line declares, for the languages whose declarations are
/// recognizable from one line.
///
/// Anything ambiguous returns `None`: a wrong name in the subject is worse than
/// no name, because it reads as deliberate.
fn definition_name(line: &str) -> Option<String> {
    let text = line.trim_start();
    // Indented declarations are members, not the subject of a commit.
    if line.len() - text.len() > 4 {
        return None;
    }

    const KEYWORDS: &[&str] = &[
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "type ",
        "def ",
        "class ",
        "func ",
        "function ",
        "interface ",
    ];
    // Prefixes that sit in front of the keyword rather than replacing it.
    const MODIFIERS: &[&str] = &[
        "pub ",
        "pub(crate) ",
        "pub(super) ",
        "async ",
        "unsafe ",
        "const ",
        "export ",
        "default ",
        "static ",
        "extern ",
    ];

    let mut rest = text;
    for _ in 0..4 {
        match MODIFIERS.iter().find_map(|m| rest.strip_prefix(m)) {
            Some(stripped) => rest = stripped,
            None => break,
        }
    }

    let keyword = KEYWORDS.iter().find(|k| rest.starts_with(**k))?;
    let name: String = rest[keyword.len()..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();

    if name.is_empty() || name.len() > 40 {
        return None;
    }
    // A bare `fn` inside a type annotation, and similar false positives.
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }

    Some(match *keyword {
        "fn " | "def " | "func " | "function " => format!("{name}()"),
        _ => name,
    })
}

/// The body: one line per file, so a reviewer can see the shape of the change
/// without opening it. Omitted for a single file, where it would only repeat
/// the subject.
fn body_for(files: &[FileDiff]) -> Vec<String> {
    // The body exists to expand a subject that could only give a number. When
    // the subject named the files, listing them again says nothing.
    if files.len() <= NAMED_LIMIT {
        return Vec::new();
    }

    let mut lines: Vec<String> = files
        .iter()
        .take(MAX_BODY_LINES)
        .map(|file| {
            let verb = match file.status {
                Delta::Added | Delta::Copied | Delta::Untracked => "Add",
                Delta::Deleted => "Remove",
                Delta::Renamed => "Move",
                _ => "Update",
            };
            format!("- {verb} {}", file.path)
        })
        .collect();

    if files.len() > MAX_BODY_LINES {
        lines.push(format!(
            "- …and {}",
            plural(files.len() - MAX_BODY_LINES, "more file")
        ));
    }
    lines
}

const MAX_BODY_LINES: usize = 12;

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::{DiffLine, Hunk};

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

    fn file(path: &str, status: Delta) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            status,
            hunks: Vec::new(),
            additions: if status == Delta::Deleted { 0 } else { 1 },
            deletions: if status == Delta::Added { 0 } else { 1 },
            omitted: None,
            mode: 0o100644,
            image: None,
            lfs: None,
        }
    }

    fn model(files: Vec<FileDiff>) -> DiffModel {
        DiffModel {
            additions: files.iter().map(|f| f.additions).sum(),
            deletions: files.iter().map(|f| f.deletions).sum(),
            files,
            is_merge_first_parent: false,
        }
    }

    fn subject(files: Vec<FileDiff>, convention: Convention) -> String {
        draft(&model(files), convention).expect("a draft").subject
    }

    #[test]
    fn nothing_staged_drafts_nothing() {
        assert_eq!(draft(&model(Vec::new()), Convention::Plain), None);
    }

    #[test]
    fn a_single_file_is_described_by_name() {
        assert_eq!(
            subject(vec![file("src/parser.rs", Delta::Added)], Convention::Plain),
            "Add parser.rs"
        );
        assert_eq!(
            subject(
                vec![file("src/parser.rs", Delta::Deleted)],
                Convention::Plain
            ),
            "Remove parser.rs"
        );
        assert_eq!(
            subject(
                vec![file("src/parser.rs", Delta::Modified)],
                Convention::Plain
            ),
            "Update parser.rs"
        );
    }

    #[test]
    fn a_rename_names_both_ends() {
        let mut renamed = file("src/lexer.rs", Delta::Renamed);
        renamed.old_path = Some("src/tokenizer.rs".to_owned());
        assert_eq!(
            subject(vec![renamed], Convention::Plain),
            "Move tokenizer.rs to lexer.rs"
        );
    }

    #[test]
    fn a_handful_of_files_are_named_rather_than_counted() {
        let files = vec![
            file("src/ui/diff.rs", Delta::Modified),
            file("src/ui/graph.rs", Delta::Modified),
            file("src/ui/blame.rs", Delta::Modified),
        ];
        assert_eq!(
            subject(files, Convention::Plain),
            "Update diff.rs, graph.rs and blame.rs"
        );
    }

    #[test]
    fn too_many_to_name_are_counted_and_placed() {
        let files: Vec<FileDiff> = ["diff", "graph", "blame", "sidebar"]
            .iter()
            .map(|n| file(&format!("src/ui/{n}.rs"), Delta::Modified))
            .collect();
        assert_eq!(
            subject(files, Convention::Plain),
            "Update 4 files in src/ui"
        );
    }

    #[test]
    fn the_verb_follows_what_happened_to_all_of_them() {
        let added = vec![
            file("src/a.rs", Delta::Added),
            file("src/b.rs", Delta::Added),
        ];
        assert_eq!(subject(added, Convention::Plain), "Add a.rs and b.rs");

        let removed = vec![
            file("src/a.rs", Delta::Deleted),
            file("src/b.rs", Delta::Deleted),
        ];
        assert_eq!(subject(removed, Convention::Plain), "Remove a.rs and b.rs");

        // Mixed actions have no single verb, so the neutral one is correct.
        let mixed = vec![
            file("src/a.rs", Delta::Added),
            file("src/b.rs", Delta::Deleted),
        ];
        assert_eq!(subject(mixed, Convention::Plain), "Update a.rs and b.rs");
    }

    #[test]
    fn the_conventional_form_carries_a_type_and_scope() {
        // `src` is a container every project has, so the scope is the part that
        // actually says where: `ui`.
        let files = vec![
            file("src/ui/diff.rs", Delta::Modified),
            file("src/ui/graph.rs", Delta::Modified),
        ];
        assert_eq!(
            subject(files, Convention::Conventional),
            "fix(ui): update diff.rs and graph.rs"
        );
    }

    #[test]
    fn a_new_source_file_is_a_feature_and_an_edit_is_a_fix() {
        assert!(subject(
            vec![file("src/parser.rs", Delta::Added)],
            Convention::Conventional
        )
        .starts_with("feat"));
        assert!(subject(
            vec![file("src/parser.rs", Delta::Modified)],
            Convention::Conventional
        )
        .starts_with("fix"));
    }

    #[test]
    fn changes_confined_to_one_area_are_typed_by_it() {
        let cases = [
            ("README.md", "docs"),
            ("docs/building.md", "docs"),
            ("tests/tabs.rs", "test"),
            (".github/workflows/ci.yml", "ci"),
            ("Cargo.toml", "build"),
            ("assets/icon/gitup.png", "chore"),
        ];
        for (path, expected) in cases {
            let drafted = subject(vec![file(path, Delta::Modified)], Convention::Conventional);
            assert!(
                drafted.starts_with(expected),
                "{path} drafted {drafted:?}, expected a {expected} commit"
            );
        }
    }

    #[test]
    fn a_test_fixture_that_is_markdown_is_still_a_test() {
        // Classification order matters: extension alone would call this docs.
        let drafted = subject(
            vec![file("tests/fixtures/README.md", Delta::Added)],
            Convention::Conventional,
        );
        assert!(drafted.starts_with("test"), "drafted {drafted:?}");
    }

    #[test]
    fn a_pure_addition_names_what_appeared() {
        let mut added = file("src/parser.rs", Delta::Modified);
        added.deletions = 0;
        added.additions = 3;
        added.hunks = vec![Hunk {
            header: "@@".to_owned(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                line(LineKind::Context, "// nearby"),
                line(LineKind::Addition, "pub fn parse_header() {"),
                line(LineKind::Addition, "    let x = 1;"),
                line(LineKind::Addition, "struct Header {"),
            ],
        }];
        assert_eq!(
            subject(vec![added], Convention::Plain),
            "Add parse_header() and Header to parser.rs"
        );
    }

    #[test]
    fn a_reworked_file_is_not_described_as_an_addition() {
        // The same added lines, but something was removed too — so "add" would
        // be a claim about a change that also took things away.
        let mut reworked = file("src/parser.rs", Delta::Modified);
        reworked.additions = 1;
        reworked.deletions = 1;
        reworked.hunks = vec![Hunk {
            header: "@@".to_owned(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                line(LineKind::Deletion, "pub fn old_name() {"),
                line(LineKind::Addition, "pub fn parse_header() {"),
            ],
        }];
        assert_eq!(
            subject(vec![reworked], Convention::Plain),
            "Update parser.rs"
        );
    }

    #[test]
    fn indented_declarations_are_not_the_subject_of_the_commit() {
        assert_eq!(definition_name("pub fn parse() {"), Some("parse()".into()));
        assert_eq!(definition_name("        fn helper() {"), None);
        assert_eq!(definition_name("    let x = 1;"), None);
        assert_eq!(definition_name("struct Header {"), Some("Header".into()));
        assert_eq!(definition_name("async fn run() {"), Some("run()".into()));
    }

    #[test]
    fn a_named_subject_gets_no_body() {
        // The body expands a count. With the files already named in the
        // subject, repeating them would be the same sentence twice.
        let drafted = draft(
            &model(vec![
                file("src/a.rs", Delta::Modified),
                file("src/b.rs", Delta::Modified),
            ]),
            Convention::Plain,
        )
        .expect("a draft");
        assert_eq!(drafted.render(), "Update a.rs and b.rs");
    }

    #[test]
    fn the_body_lists_the_files_and_stops() {
        let files: Vec<FileDiff> = (0..20)
            .map(|i| file(&format!("src/file{i}.rs"), Delta::Modified))
            .collect();
        let drafted = draft(&model(files), Convention::Plain).expect("a draft");
        assert_eq!(drafted.body.len(), MAX_BODY_LINES + 1);
        assert_eq!(drafted.body.last().unwrap(), "- …and 8 more files");
    }

    #[test]
    fn one_file_gets_no_body() {
        let drafted = draft(
            &model(vec![file("src/parser.rs", Delta::Modified)]),
            Convention::Plain,
        )
        .expect("a draft");
        assert!(drafted.body.is_empty());
        assert_eq!(drafted.render(), "Update parser.rs");
    }

    #[test]
    fn rendering_separates_subject_and_body_with_a_blank_line() {
        let drafted = Draft {
            subject: "Update 2 files".to_owned(),
            body: vec!["- Update a".to_owned(), "- Update b".to_owned()],
        };
        assert_eq!(drafted.render(), "Update 2 files\n\n- Update a\n- Update b");
    }

    #[test]
    fn the_convention_is_read_from_recent_history() {
        assert_eq!(
            Convention::detect(["feat: add a thing", "fix(ui): stop flickering"]),
            Convention::Conventional
        );
        assert_eq!(
            Convention::detect(["Add a thing", "Stop the flickering"]),
            Convention::Plain
        );
        // A history with no commits at all cannot vote.
        assert_eq!(Convention::detect([]), Convention::Plain);
    }

    #[test]
    fn merges_do_not_vote_on_the_convention() {
        // git writes these, not the project, and a busy branch has enough of
        // them to drown out the subjects a person actually chose.
        let subjects = [
            "Merge branch 'main' into feature",
            "Merge pull request #12 from x/y",
            "Merge branch 'release'",
            "feat: add a thing",
            "fix: correct the other thing",
        ];
        assert_eq!(Convention::detect(subjects), Convention::Conventional);
    }

    #[test]
    fn near_misses_are_not_conventional() {
        for subject in [
            "Fix: capitalized type",
            "no colon here",
            "feat:missing space",
            "feat(): empty scope",
            "feat(unclosed: bad",
            "this is a long sentence: with a colon in the middle of it",
        ] {
            assert!(!is_conventional(subject), "{subject:?} should not count");
        }
        for subject in [
            "feat: a thing",
            "fix(ui): a thing",
            "feat!: breaking",
            "feat(api)!: breaking",
        ] {
            assert!(is_conventional(subject), "{subject:?} should count");
        }
    }
}
