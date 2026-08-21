//! Blame, file history, and search.

mod common;

use common::Fixture;
use gitup::git::highlight::HighlightTheme;
use gitup::git::search::{self, SearchKind};
use gitup::git::{blame, graph};
use gitup::job::Cancel;

#[test]
fn blame_attributes_each_line_to_the_commit_that_introduced_it() {
    let f = Fixture::empty_named("blame");
    f.commit_file("f.txt", "alpha\nbeta\n", "Add alpha and beta");
    f.commit_file("f.txt", "alpha\nbeta\ngamma\n", "Add gamma");
    f.commit_file("f.txt", "ALPHA\nbeta\ngamma\n", "Change alpha");

    let result = blame::blame(
        &f.repo,
        "f.txt",
        None,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("blame");
    assert_eq!(result.lines.len(), 3);

    let summary_of = |index: usize| {
        result
            .commit(result.lines[index].commit)
            .expect("commit metadata")
            .summary
            .clone()
    };
    assert_eq!(summary_of(0), "Change alpha");
    assert_eq!(
        summary_of(1),
        "Add alpha and beta",
        "beta was never touched"
    );
    assert_eq!(summary_of(2), "Add gamma");

    assert_eq!(result.lines[0].content, "ALPHA");
    assert_eq!(result.lines[0].line_no, 1);
}

#[test]
fn consecutive_lines_from_one_commit_form_a_group() {
    let f = Fixture::empty_named("groups");
    f.commit_file("f.txt", "one\ntwo\nthree\n", "All at once");
    f.commit_file("f.txt", "one\ntwo\nthree\nfour\n", "Add four");

    let result = blame::blame(
        &f.repo,
        "f.txt",
        None,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("blame");
    // The first three lines share a commit, so only the first starts a group.
    assert!(result.lines[0].starts_group);
    assert!(!result.lines[1].starts_group);
    assert!(!result.lines[2].starts_group);
    assert!(result.lines[3].starts_group, "a new commit starts a group");
}

#[test]
fn blame_can_look_at_an_earlier_commit() {
    let f = Fixture::empty_named("blame_at");
    f.commit_file("f.txt", "original\n", "First");
    f.commit_file("f.txt", "rewritten\n", "Second");

    let page = graph::build(&f.repo, 10, &Cancel::default()).expect("graph");
    let first = page
        .rows
        .iter()
        .find(|r| r.commit.summary == "First")
        .expect("first commit")
        .commit
        .id;

    let result = blame::blame(
        &f.repo,
        "f.txt",
        Some(first),
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("blame at");
    assert_eq!(result.lines.len(), 1);
    assert_eq!(
        result.lines[0].content, "original",
        "blaming at a commit must read the file as it was then"
    );
}

#[test]
fn recency_shades_between_oldest_and_newest() {
    let f = Fixture::empty_named("recency");
    f.commit_file("f.txt", "old\n", "Old");
    f.commit_file("f.txt", "old\nnew\n", "New");

    let result = blame::blame(
        &f.repo,
        "f.txt",
        None,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("blame");
    let old = result.recency(result.lines[0].commit);
    let new = result.recency(result.lines[1].commit);
    assert!(old < new, "older lines should shade lower: {old} vs {new}");
    assert!((0.0..=1.0).contains(&old) && (0.0..=1.0).contains(&new));
}

#[test]
fn blaming_a_binary_file_is_refused_rather_than_garbled() {
    let f = Fixture::empty_named("binblame");
    let bytes: Vec<u8> = (0u8..=255).cycle().take(2048).collect();
    std::fs::write(f.path().join("blob.bin"), &bytes).expect("write");
    f.stage("blob.bin");
    f.commit("Add binary");

    let error = blame::blame(
        &f.repo,
        "blob.bin",
        None,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect_err("should refuse");
    assert!(error.user_message().contains("binary"));
}

#[test]
fn blame_lines_are_syntax_highlighted() {
    let f = Fixture::empty_named("blame_syntax");
    f.commit_file(
        "src/lib.rs",
        "pub fn add(a: i32) -> i32 {\n    /* a block\n       comment */\n    a + 1\n}\n",
        "Add code",
    );

    let result = blame::blame(
        &f.repo,
        "src/lib.rs",
        None,
        HighlightTheme::Dark,
        &Cancel::default(),
    )
    .expect("blame");

    assert!(
        result.lines.iter().all(|l| !l.spans.is_empty()),
        "every line of a known file type should be coloured"
    );
    // Whole-file highlighting means the comment's colour carries across lines,
    // which a per-fragment pass could not manage.
    let second = result.lines[1].spans.last().expect("colour").color;
    let third = result.lines[2].spans.first().expect("colour").color;
    assert_eq!(second, third, "the block comment should stay one colour");

    for line in &result.lines {
        let covered: u32 = line.spans.iter().map(|s| s.len).sum();
        assert_eq!(
            covered as usize,
            line.content.len(),
            "spans must tile {line:?}"
        );
    }

    // With highlighting off, the same blame carries no colour at all.
    let plain = blame::blame(
        &f.repo,
        "src/lib.rs",
        None,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("blame");
    assert!(plain.lines.iter().all(|l| l.spans.is_empty()));
}

#[test]
fn searching_messages_finds_the_right_commits() {
    let f = Fixture::empty_named("searchmsg");
    f.commit_file("a.txt", "a\n", "Add the parser");
    f.commit_file("b.txt", "b\n", "Fix the lexer");
    f.commit_file("c.txt", "c\n", "Refactor the parser again");

    let results = search::search(
        &f.repo,
        SearchKind::Message,
        "parser",
        50,
        &Cancel::default(),
    )
    .expect("search");
    let summaries: Vec<&str> = results.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries.len(), 2, "got {summaries:?}");
    assert!(summaries.contains(&"Add the parser"));
    assert!(summaries.contains(&"Refactor the parser again"));
    assert!(!results.truncated);
}

#[test]
fn message_search_is_case_insensitive_and_literal() {
    let f = Fixture::empty_named("searchcase");
    f.commit_file("a.txt", "a\n", "Handle UTF-8 input");
    f.commit_file("b.txt", "b\n", "Unrelated");

    let hit = search::search(
        &f.repo,
        SearchKind::Message,
        "utf-8",
        50,
        &Cancel::default(),
    )
    .expect("search");
    assert_eq!(hit.commits.len(), 1);

    // A regex metacharacter is matched literally, so searching for "a.b"
    // doesn't match "axb".
    let f2 = Fixture::empty_named("searchliteral");
    f2.commit_file("a.txt", "a\n", "axb");
    let miss = search::search(&f2.repo, SearchKind::Message, "a.b", 50, &Cancel::default())
        .expect("search");
    assert!(miss.commits.is_empty(), "the dot should not be a wildcard");
}

#[test]
fn content_search_finds_where_a_string_appeared() {
    let f = Fixture::empty_named("searchcontent");
    f.commit_file("a.rs", "fn nothing() {}\n", "First");
    f.commit_file(
        "a.rs",
        "fn nothing() {}\nfn marker_function() {}\n",
        "Second",
    );
    f.commit_file("a.rs", "fn nothing() {}\n", "Third removes it");

    let results = search::search(
        &f.repo,
        SearchKind::Content,
        "marker_function",
        50,
        &Cancel::default(),
    )
    .expect("search");

    let summaries: Vec<&str> = results.commits.iter().map(|c| c.summary.as_str()).collect();
    assert!(summaries.contains(&"Second"), "got {summaries:?}");
    assert!(
        summaries.contains(&"Third removes it"),
        "removal changes the count too: {summaries:?}"
    );
    assert!(!summaries.contains(&"First"));
}

#[test]
fn author_search_matches_name_or_email() {
    let f = Fixture::simple();
    let by_name = search::search(
        &f.repo,
        SearchKind::Author,
        "Fixture",
        50,
        &Cancel::default(),
    )
    .expect("search");
    assert_eq!(by_name.commits.len(), 3);

    let by_email = search::search(
        &f.repo,
        SearchKind::Author,
        "fixture@example.com",
        50,
        &Cancel::default(),
    )
    .expect("search");
    assert_eq!(by_email.commits.len(), 3);

    let miss = search::search(
        &f.repo,
        SearchKind::Author,
        "nobody",
        50,
        &Cancel::default(),
    )
    .expect("search");
    assert!(miss.commits.is_empty());
}

#[test]
fn an_empty_query_returns_nothing_rather_than_everything() {
    let f = Fixture::simple();
    for kind in SearchKind::all() {
        let results = search::search(&f.repo, kind, "   ", 50, &Cancel::default()).expect("search");
        assert!(
            results.commits.is_empty(),
            "{kind:?} returned results for an empty query"
        );
    }
}

#[test]
fn results_report_when_they_were_truncated() {
    let f = Fixture::empty_named("trunc");
    for i in 0..6 {
        f.commit_file(&format!("f{i}.txt"), "x\n", &format!("Commit {i}"));
    }
    let results = search::search(
        &f.repo,
        SearchKind::Message,
        "Commit",
        3,
        &Cancel::default(),
    )
    .expect("search");
    assert_eq!(results.commits.len(), 3);
    assert!(results.truncated);
}

#[test]
fn file_history_follows_a_rename() {
    let f = Fixture::empty_named("history");
    let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
    f.commit_file("original.rs", &body, "Create the file");
    f.commit_file("original.rs", &format!("{body}extra\n"), "Edit it");

    std::fs::rename(f.path().join("original.rs"), f.path().join("renamed.rs")).expect("rename");
    f.stage_all();
    let mut index = f.repo.index().expect("index");
    index
        .remove_path(std::path::Path::new("original.rs"))
        .expect("remove");
    index.write().expect("write");
    f.commit("Rename it");

    let history =
        search::file_history(&f.repo, "renamed.rs", 50, &Cancel::default()).expect("history");
    let summaries: Vec<&str> = history.commits.iter().map(|c| c.summary.as_str()).collect();

    assert!(summaries.contains(&"Rename it"));
    assert!(
        summaries.contains(&"Create the file"),
        "history must follow through the rename: {summaries:?}"
    );
}

#[test]
fn file_history_of_an_unknown_path_is_empty_not_an_error() {
    let f = Fixture::simple();
    let history =
        search::file_history(&f.repo, "no/such/file.txt", 50, &Cancel::default()).expect("history");
    assert!(history.commits.is_empty());
}
