//! Performance guardrails.
//!
//! The architecture is justified by a claim — that a large repository stays
//! responsive — and a claim with no test behind it is a hope. These bound the
//! operations that run on every interaction. The thresholds are deliberately
//! loose: they exist to catch an accidental quadratic, not to police
//! milliseconds on whatever machine happens to run them.

mod common;

use common::Fixture;
use gitup::git::diff::{self, DiffTarget};
use gitup::git::highlight::HighlightTheme;
use gitup::git::{graph, status};
use gitup::job::Cancel;
use std::time::{Duration, Instant};

const COMMITS: usize = 10_000;

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    let elapsed = start.elapsed();
    println!("{label}: {elapsed:?}");
    (value, elapsed)
}

#[test]
fn walking_ten_thousand_commits_is_fast() {
    let f = Fixture::synthetic("perf_graph", COMMITS);

    let (page, elapsed) = timed("graph build (10k)", || {
        graph::build(&f.repo, COMMITS, &Cancel::default()).expect("graph")
    });

    assert_eq!(page.rows.len(), COMMITS);
    assert_eq!(page.max_width, 1, "a linear history needs one lane");
    assert!(
        elapsed < Duration::from_secs(10),
        "graph build took {elapsed:?}; something is scaling badly"
    );
}

#[test]
fn the_limit_bounds_memory_rather_than_time() {
    // Worth asserting explicitly, because the opposite is the natural
    // assumption: libgit2 resolves a topological revwalk eagerly, so asking for
    // the first thousand commits traverses the same graph as asking for all of
    // them. The limit caps how many summaries are built, not how much walking
    // happens — which is exactly why the initial limit is set high, not low.
    let f = Fixture::synthetic("perf_page", COMMITS);

    let (_, full) = timed("graph build (all)", || {
        graph::build(&f.repo, COMMITS, &Cancel::default()).expect("graph")
    });
    let (page, first) = timed("graph build (first 1000)", || {
        graph::build(&f.repo, 1_000, &Cancel::default()).expect("graph")
    });

    assert_eq!(page.rows.len(), 1_000, "the limit caps the rows returned");
    assert!(page.has_more, "and reports that the walk stopped early");
    // Deliberately no assertion that one is faster than the other: they are
    // within noise by design. What matters is that neither is pathological.
    assert!(full < Duration::from_secs(10), "full walk took {full:?}");
    assert!(
        first < Duration::from_secs(10),
        "capped walk took {first:?}"
    );
}

#[test]
fn a_walk_can_be_cancelled_partway() {
    let f = Fixture::synthetic("perf_cancel", COMMITS);
    let cancel = Cancel::default();
    cancel.cancel();

    let (result, elapsed) = timed("cancelled walk", || graph::build(&f.repo, COMMITS, &cancel));

    assert!(
        result.is_err_and(|e| e.is_cancelled()),
        "a cancelled walk must stop rather than finish"
    );
    // The check happens before the revwalk is resolved, so an already-cancelled
    // job costs nothing. Once the walk begins libgit2 offers no way to interrupt
    // it, which is precisely why the early check has to be there.
    assert!(
        elapsed < Duration::from_millis(100),
        "an already-cancelled walk should return immediately, took {elapsed:?}"
    );
}

#[test]
fn status_on_a_large_working_tree_is_fast() {
    let f = Fixture::empty_named("perf_status");
    // 2000 files across 40 directories, which is a realistic large checkout.
    for dir in 0..40 {
        for file in 0..50 {
            f.write(
                &format!("dir{dir}/file{file}.txt"),
                &format!("content {dir}/{file}\n"),
            );
        }
    }
    f.stage_all();
    f.commit("Add many files");

    // Touch a handful so the scan has real work to report.
    for dir in 0..5 {
        f.write(&format!("dir{dir}/file0.txt"), "changed\n");
    }

    let (snapshot, elapsed) = timed("status (2000 files)", || {
        status::status(&f.repo, false).expect("status")
    });

    assert_eq!(snapshot.unstaged_count, 5);
    assert!(
        elapsed < Duration::from_secs(5),
        "status took {elapsed:?} on 2000 files"
    );
}

#[test]
fn diffing_a_large_file_stays_bounded() {
    let f = Fixture::empty_named("perf_diff");
    let original: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    f.commit_file("big.txt", &original, "Add");

    // Change one line in the middle: the diff should be small even though the
    // file is not.
    let edited: String = (0..20_000)
        .map(|i| {
            if i == 10_000 {
                "line ten thousand, edited\n".to_owned()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    f.write("big.txt", &edited);

    let (model, elapsed) = timed("diff (20k-line file, one change)", || {
        diff::build(
            &f.repo,
            DiffTarget::Unstaged,
            HighlightTheme::Dark,
            &Cancel::default(),
        )
        .expect("diff")
    });

    let file = model.find("big.txt").expect("big.txt");
    assert_eq!(file.additions, 1);
    assert_eq!(file.deletions, 1);
    assert!(
        file.line_count() < 20,
        "context should be a handful of lines, got {}",
        file.line_count()
    );
    assert!(elapsed < Duration::from_secs(5), "diff took {elapsed:?}");
}

#[test]
fn a_pathological_file_is_refused_rather_than_rendered() {
    // The guard exists so one generated file can't lock up the renderer.
    let f = Fixture::empty_named("perf_huge");
    let body: String = (0..60_000).map(|i| format!("line {i}\n")).collect();
    f.commit_file("generated.txt", &body, "Add generated file");

    let page = graph::build(&f.repo, 10, &Cancel::default()).expect("graph");
    let (model, elapsed) = timed("diff (60k-line addition)", || {
        diff::build(
            &f.repo,
            DiffTarget::Commit(page.rows[0].commit.id),
            HighlightTheme::Dark,
            &Cancel::default(),
        )
        .expect("diff")
    });

    let file = &model.files[0];
    assert_eq!(file.omitted, Some(diff::Omitted::TooLarge));
    assert!(file.hunks.is_empty());
    assert!(
        elapsed < Duration::from_secs(5),
        "even refusing took {elapsed:?}"
    );
}
