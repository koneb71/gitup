//! Lane assignment. Every assertion here describes a shape the renderer will
//! draw, so a regression shows up as a wrong-looking graph, not just a wrong
//! number.

mod common;

use common::Fixture;
use gitup::git::graph::{self, GraphPage, GraphRow};
use gitup::job::Cancel;
use std::sync::Arc;

fn build(fixture: &Fixture) -> Arc<GraphPage> {
    graph::build(&fixture.repo, 1000, &Cancel::default()).expect("graph")
}

fn row<'a>(page: &'a GraphPage, summary: &str) -> &'a GraphRow {
    page.rows
        .iter()
        .find(|r| r.commit.summary == summary)
        .unwrap_or_else(|| {
            panic!(
                "no row {summary:?}; have {:?}",
                page.rows
                    .iter()
                    .map(|r| &r.commit.summary)
                    .collect::<Vec<_>>()
            )
        })
}

fn lanes(segments: &[graph::Segment]) -> Vec<usize> {
    let mut v: Vec<usize> = segments.iter().map(|s| s.lane).collect();
    v.sort_unstable();
    v
}

#[test]
fn empty_repository_yields_no_rows() {
    let f = Fixture::empty();
    let page = build(&f);
    assert!(page.rows.is_empty());
    assert!(!page.has_more);
}

#[test]
fn linear_history_stays_in_one_lane() {
    let f = Fixture::simple();
    let page = build(&f);

    assert_eq!(page.rows.len(), 3);
    assert_eq!(
        page.max_width, 1,
        "a straight history needs exactly one lane"
    );
    assert!(page.rows.iter().all(|r| r.lane == 0));
    assert!(
        page.rows.iter().all(|r| r.passthrough.is_empty()),
        "nothing crosses a single-lane graph"
    );

    // Newest first.
    assert_eq!(page.rows[0].commit.summary, "Add lib");
    assert_eq!(page.rows[2].commit.summary, "Add README");

    // The tip has nothing arriving from above; the root has nothing leaving.
    assert!(page.rows[0].incoming.is_empty());
    assert_eq!(lanes(&page.rows[0].outgoing), vec![0]);
    assert_eq!(lanes(&page.rows[2].incoming), vec![0]);
    assert!(page.rows[2].outgoing.is_empty());
}

#[test]
fn a_merge_opens_a_second_lane_and_closes_it_again() {
    let f = Fixture::merged();
    let page = build(&f);

    assert_eq!(page.rows.len(), 4, "A, B, F, merge");
    assert_eq!(page.max_width, 2, "one side branch means two lanes");

    let merge = row(&page, "Merge feature");
    assert!(merge.commit.is_merge());
    assert_eq!(merge.lane, 0);
    assert_eq!(
        merge.outgoing.len(),
        2,
        "a merge sends a line to each parent"
    );
    assert_eq!(
        lanes(&merge.outgoing),
        vec![0, 1],
        "first parent continues in the merge's lane, second branches right"
    );

    // The side branch keeps descending in its own lane rather than bending
    // early; the two lines meet at the commit they share.
    let side = row(&page, "F");
    assert_eq!(side.lane, 1);
    assert_eq!(lanes(&side.outgoing), vec![1]);

    // The shared root is where they converge — both lanes arrive, and the root
    // claims the leftmost, releasing the other.
    let root = row(&page, "A");
    assert_eq!(root.lane, 0, "the trunk stays in the leftmost lane");
    assert_eq!(
        lanes(&root.incoming),
        vec![0, 1],
        "both branches converge here"
    );
    assert!(root.outgoing.is_empty(), "the root has no parents");
    assert_eq!(root.width, 1, "lanes are released once the branch is done");
}

#[test]
fn overlapping_unrelated_roots_get_their_own_lanes() {
    let f = Fixture::empty_named("roots");
    f.commit_file("a.txt", "a\n", "A");

    // An orphan branch shares no history with `main`.
    f.repo.set_head("refs/heads/orphan").expect("set_head");
    f.repo.index().expect("index").clear().expect("clear");
    f.commit_file("z.txt", "z\n", "Z");

    // Committing on `main` again interleaves the two histories in time, so
    // lane 0 is still descending toward `A` when `Z` is reached and `Z` cannot
    // reuse it. Two roots that *don't* overlap would correctly share one lane.
    f.checkout("main");
    f.commit_file("b.txt", "b\n", "B");

    let page = build(&f);
    assert_eq!(page.rows.len(), 3);
    assert_eq!(page.max_width, 2, "interleaved histories need two lanes");
    assert_ne!(row(&page, "A").lane, row(&page, "Z").lane);
    assert!(row(&page, "Z").outgoing.is_empty(), "Z is a root");
}

#[test]
fn limit_is_respected_and_reported() {
    let f = Fixture::simple();
    let page = graph::build(&f.repo, 2, &Cancel::default()).expect("graph");
    assert_eq!(page.rows.len(), 2);
    assert!(
        page.has_more,
        "stopping at the limit must be visible to the UI"
    );

    let full = build(&f);
    assert!(!full.has_more);
}

#[test]
fn refs_are_attached_to_the_commits_they_point_at() {
    let f = Fixture::merged();
    f.tag("v1.0");
    let page = build(&f);

    let merge = row(&page, "Merge feature");
    let names: Vec<&str> = merge.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"main"), "got {names:?}");
    assert!(names.contains(&"v1.0"), "got {names:?}");
    assert!(
        merge.refs.iter().any(|r| r.is_head),
        "the checked-out branch must be marked"
    );
    // The checked-out branch sorts first so it reads as the primary label.
    assert!(merge.refs[0].is_head);

    let feature = row(&page, "F");
    assert_eq!(
        feature
            .refs
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["feature"]
    );
}

#[test]
fn commit_metadata_survives_the_walk() {
    let f = Fixture::simple();
    let page = build(&f);
    let top = &page.rows[0].commit;

    assert_eq!(top.author_name, "Fixture");
    assert_eq!(top.author_email, "fixture@example.com");
    assert_eq!(top.short_id.len(), 7);
    assert!(top.id.to_string().starts_with(&top.short_id));
    assert_eq!(top.parents.len(), 1);
}
