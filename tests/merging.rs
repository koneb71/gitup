//! Merge, conflict resolution, cherry-pick, revert, reset, and rebase.

mod common;

use common::Fixture;
use gitup::git::conflict::{self, ConflictKind, Resolution};
use gitup::git::merge::{self, MergeOutcome, ResetKind};
use gitup::git::{graph, rebase};
use gitup::job::Cancel;
use std::path::Path;

fn head_summary(f: &Fixture) -> String {
    f.repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .summary()
        .unwrap()
        .unwrap_or("")
        .to_owned()
}

fn read(f: &Fixture, path: &str) -> String {
    std::fs::read_to_string(f.path().join(path)).expect("read")
}

fn commit_id(f: &Fixture, summary: &str) -> git2::Oid {
    let page = graph::build(&f.repo, 100, &Cancel::default()).expect("graph");
    page.rows
        .iter()
        .find(|r| r.commit.summary == summary)
        .unwrap_or_else(|| panic!("no commit {summary:?}"))
        .commit
        .id
}

#[test]
fn a_fast_forward_merge_moves_head_without_a_merge_commit() {
    let f = Fixture::empty_named("ff");
    f.commit_file("a.txt", "a\n", "Base");
    f.branch("ahead");
    f.checkout("ahead");
    f.commit_file("b.txt", "b\n", "Ahead");
    f.checkout("main");

    let outcome = merge::merge(&f.repo, "ahead").expect("merge");
    assert_eq!(outcome, MergeOutcome::FastForward);
    assert_eq!(head_summary(&f), "Ahead");
    assert_eq!(
        f.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        1,
        "a fast-forward creates no merge commit"
    );
    assert!(f.path().join("b.txt").exists());
}

#[test]
fn merging_an_ancestor_is_a_no_op() {
    let f = Fixture::empty_named("uptodate");
    f.commit_file("a.txt", "a\n", "Base");
    f.branch("behind");
    f.commit_file("b.txt", "b\n", "Ahead of behind");

    assert_eq!(
        merge::merge(&f.repo, "behind").expect("merge"),
        MergeOutcome::UpToDate
    );
}

#[test]
fn a_clean_merge_stages_both_sides_and_leaves_merge_head() {
    let f = Fixture::mergeable();
    let outcome = merge::merge(&f.repo, "side").expect("merge");
    assert_eq!(outcome, MergeOutcome::Merged { conflicts: 0 });

    assert!(f.path().join("ours.txt").exists());
    assert!(f.path().join("theirs.txt").exists());
    assert_eq!(
        f.repo.state(),
        git2::RepositoryState::Merge,
        "the merge is staged but not committed"
    );

    // Committing now records both parents.
    gitup::git::commit::commit(
        &f.repo,
        "Merge side\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit the merge");

    let head = f.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2, "MERGE_HEAD must become a parent");
    assert_eq!(
        f.repo.state(),
        git2::RepositoryState::Clean,
        "committing clears the in-progress state"
    );
}

#[test]
fn a_conflicting_merge_reports_the_conflict_with_all_three_sides() {
    let f = Fixture::conflicting();
    let outcome = merge::merge(&f.repo, "theirs").expect("merge");
    assert_eq!(outcome, MergeOutcome::Merged { conflicts: 1 });

    let conflicts = conflict::list(&f.repo).expect("conflicts");
    assert_eq!(conflicts.files.len(), 1);
    let c = &conflicts.files[0];
    assert_eq!(c.path, "shared.txt");
    assert_eq!(c.kind, ConflictKind::BothModified);

    // All three stages are present — this is what a marker-parsing approach
    // would lose.
    assert!(c.base.present && c.base.content.contains("\nb\n"));
    assert!(c.ours.content.contains("OURS"));
    assert!(c.theirs.content.contains("THEIRS"));
    assert!(
        conflict::has_markers(&c.merged),
        "the working file should carry git's markers"
    );
}

#[test]
fn resolving_with_ours_clears_the_conflict() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");

    conflict::resolve_with(&f.repo, "shared.txt", Resolution::Ours).expect("resolve");

    assert_eq!(read(&f, "shared.txt"), "a\nOURS\nc\n");
    let after = conflict::list(&f.repo).expect("conflicts");
    assert!(after.is_empty(), "the path should no longer be conflicted");

    // And the merge can now be committed.
    gitup::git::commit::commit(
        &f.repo,
        "Take ours\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit");
    assert_eq!(head_summary(&f), "Take ours");
}

#[test]
fn resolving_with_theirs_takes_the_other_side() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");
    conflict::resolve_with(&f.repo, "shared.txt", Resolution::Theirs).expect("resolve");
    assert_eq!(read(&f, "shared.txt"), "a\nTHEIRS\nc\n");
}

#[test]
fn resolving_with_both_keeps_each_side_in_order() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");
    conflict::resolve_with(&f.repo, "shared.txt", Resolution::Both).expect("resolve");

    let text = read(&f, "shared.txt");
    let ours_at = text.find("OURS").expect("ours present");
    let theirs_at = text.find("THEIRS").expect("theirs present");
    assert!(ours_at < theirs_at, "ours comes first: {text:?}");
}

#[test]
fn hand_edited_content_with_markers_left_in_is_refused() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");

    let error = conflict::resolve_with_content(
        &f.repo,
        "shared.txt",
        "a\n<<<<<<< HEAD\nOURS\n=======\nTHEIRS\n>>>>>>> theirs\nc\n",
    )
    .expect_err("markers should be caught");
    assert!(error.user_message().contains("conflict markers"));

    // Cleaned-up content is accepted.
    conflict::resolve_with_content(&f.repo, "shared.txt", "a\nBOTH\nc\n").expect("resolve");
    assert_eq!(read(&f, "shared.txt"), "a\nBOTH\nc\n");
    assert!(conflict::list(&f.repo).unwrap().is_empty());
}

#[test]
fn aborting_a_merge_puts_everything_back() {
    let f = Fixture::conflicting();
    let before = head_summary(&f);
    merge::merge(&f.repo, "theirs").expect("merge");
    assert_eq!(f.repo.state(), git2::RepositoryState::Merge);

    merge::abort(&f.repo).expect("abort");
    assert_eq!(f.repo.state(), git2::RepositoryState::Clean);
    assert_eq!(head_summary(&f), before);
    assert_eq!(read(&f, "shared.txt"), "a\nOURS\nc\n");
}

#[test]
fn starting_a_second_operation_mid_merge_is_refused() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");
    let error = merge::merge(&f.repo, "theirs").expect_err("should refuse");
    assert!(error.user_message().contains("in progress"));
}

#[test]
fn cherry_pick_applies_one_commit_onto_head() {
    let f = Fixture::empty_named("cherry");
    f.commit_file("base.txt", "base\n", "Base");
    f.branch("side");
    f.checkout("side");
    f.commit_file("feature.txt", "feature\n", "Add feature");
    f.checkout("main");

    let oid = commit_id(&f, "Add feature");
    let conflicts = merge::cherry_pick(&f.repo, oid, 0).expect("cherry-pick");
    assert_eq!(conflicts, 0);
    assert!(f.path().join("feature.txt").exists());

    gitup::git::commit::commit(
        &f.repo,
        "Add feature\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit");
    assert_eq!(head_summary(&f), "Add feature");
}

#[test]
fn revert_undoes_a_commit() {
    let f = Fixture::empty_named("revert");
    f.commit_file("f.txt", "original\n", "First");
    f.commit_file("f.txt", "changed\n", "Change it");

    let oid = commit_id(&f, "Change it");
    merge::revert(&f.repo, oid, 0).expect("revert");
    gitup::git::commit::commit(
        &f.repo,
        "Revert the change\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit");

    assert_eq!(read(&f, "f.txt"), "original\n");
}

#[test]
fn a_merge_commit_needs_a_mainline_and_reports_its_parents() {
    let f = Fixture::merged();
    let merge_oid = commit_id(&f, "Merge feature");

    assert!(merge::needs_mainline(&f.repo, merge_oid).expect("check"));
    let parents = merge::parents_of(&f.repo, merge_oid).expect("parents");
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[0].0, 1, "git numbers parents from one");
    assert_eq!(parents[1].0, 2);
    assert_eq!(parents[0].2, "B", "first parent is the branch we were on");
    assert_eq!(parents[1].2, "F", "second is the branch that came in");

    // Without a mainline it is refused, with an explanation.
    let error = merge::cherry_pick(&f.repo, merge_oid, 0).expect_err("needs a mainline");
    assert!(error.user_message().contains("merge commit"));

    // And a parent that doesn't exist is refused too.
    let error = merge::cherry_pick(&f.repo, merge_oid, 5).expect_err("no such parent");
    assert!(error.user_message().contains("no parent 5"));
}

#[test]
fn an_ordinary_commit_rejects_a_mainline() {
    let f = Fixture::simple();
    let oid = commit_id(&f, "Add lib");
    assert!(!merge::needs_mainline(&f.repo, oid).expect("check"));
    let error = merge::cherry_pick(&f.repo, oid, 1).expect_err("not a merge");
    assert!(error.user_message().contains("Only a merge commit"));
}

/// A repository with a *real* content merge, unlike `Fixture::merged`, which
/// only shapes the graph.
fn really_merged() -> Fixture {
    let f = Fixture::mergeable();
    merge::merge(&f.repo, "side").expect("merge");
    gitup::git::commit::commit(
        &f.repo,
        "Merge side\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit the merge");
    f
}

#[test]
fn reverting_a_merge_against_the_first_parent_undoes_the_branch() {
    // Parent 1 is the branch we were on, so reverting against it takes the
    // *other* side's changes back out.
    let f = really_merged();
    let merge_oid = commit_id(&f, "Merge side");
    assert!(
        f.path().join("theirs.txt").exists(),
        "the merge brought it in"
    );
    assert!(f.path().join("ours.txt").exists());

    merge::revert(&f.repo, merge_oid, 1).expect("revert the merge");
    gitup::git::commit::commit(
        &f.repo,
        "Revert the merge\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit");

    assert!(
        !f.path().join("theirs.txt").exists(),
        "the merged-in file should be gone again"
    );
    assert!(
        f.path().join("ours.txt").exists(),
        "our own work is untouched"
    );
}

#[test]
fn reverting_against_the_second_parent_undoes_the_other_side() {
    let f = really_merged();
    let merge_oid = commit_id(&f, "Merge side");

    merge::revert(&f.repo, merge_oid, 2).expect("revert against parent 2");
    gitup::git::commit::commit(
        &f.repo,
        "Revert the other way\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect("commit");

    assert!(
        !f.path().join("ours.txt").exists(),
        "measured against the incoming branch, our side is what gets undone"
    );
    assert!(f.path().join("theirs.txt").exists());
}

#[test]
fn cherry_picking_a_merge_applies_one_side() {
    let f = really_merged();
    let merge_oid = commit_id(&f, "Merge side");

    // A fresh branch from the common base, so neither side's work is present.
    gitup::git::branch::create(&f.repo, "fresh", Some("HEAD~1^"), true).expect("branch");
    assert!(!f.path().join("ours.txt").exists());
    assert!(!f.path().join("theirs.txt").exists());

    let conflicts = merge::cherry_pick(&f.repo, merge_oid, 2).expect("cherry-pick the merge");
    assert_eq!(conflicts, 0);
    assert!(
        f.path().join("ours.txt").exists(),
        "picking against parent 2 applies the first parent's side"
    );
}

#[test]
fn reset_modes_differ_in_what_they_keep() {
    for (kind, expect_staged, expect_file) in [
        (ResetKind::Soft, true, "changed\n"),
        (ResetKind::Mixed, false, "changed\n"),
        (ResetKind::Hard, false, "original\n"),
    ] {
        let f = Fixture::empty_named("reset");
        f.commit_file("f.txt", "original\n", "First");
        f.commit_file("f.txt", "changed\n", "Second");
        let first = commit_id(&f, "First");

        merge::reset(&f.repo, first, kind).expect("reset");

        assert_eq!(head_summary(&f), "First", "{kind:?} must move the branch");
        assert_eq!(read(&f, "f.txt"), expect_file, "{kind:?} working tree");

        let status = gitup::git::status::status(&f.repo, false).expect("status");
        let staged = status.staged_count > 0;
        assert_eq!(staged, expect_staged, "{kind:?} staged state");
    }
}

#[test]
fn rebase_replays_commits_onto_another_branch() {
    let f = Fixture::empty_named("rebase");
    f.commit_file("base.txt", "base\n", "Base");
    f.branch("topic");
    f.commit_file("main-only.txt", "main\n", "Main moves on");

    f.checkout("topic");
    f.commit_file("topic.txt", "topic\n", "Topic work");

    rebase::rebase_onto(f.path(), "main", &Cancel::default(), |_| {}).expect("rebase");

    // The topic commit now sits on top of main's, so both files exist.
    assert!(f.path().join("main-only.txt").exists());
    assert!(f.path().join("topic.txt").exists());
    assert_eq!(head_summary(&f), "Topic work");
    let parent = f
        .repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .parent(0)
        .unwrap();
    assert_eq!(parent.summary().unwrap(), Some("Main moves on"));
}

#[test]
fn an_interactive_rebase_can_squash_and_drop() {
    let f = Fixture::linear(4);
    let base = commit_id(&f, "Commit 1");
    let mut plan = rebase::plan_from(&f.repo, base).expect("plan");
    assert_eq!(plan.steps.len(), 3, "commits 2, 3, and 4");
    assert_eq!(plan.steps[0].summary, "Commit 2", "oldest first");

    plan.steps[1].action = rebase::StepAction::Squash; // fold 3 into 2
    plan.steps[2].action = rebase::StepAction::Drop; // remove 4

    rebase::rebase_interactive(f.path(), &plan, &Cancel::default(), |_| {})
        .expect("interactive rebase");

    let page = graph::build(&f.repo, 100, &Cancel::default()).expect("graph");
    let summaries: Vec<&str> = page
        .rows
        .iter()
        .map(|r| r.commit.summary.as_str())
        .collect();
    assert!(!summaries.contains(&"Commit 4"), "dropped: {summaries:?}");
    assert_eq!(summaries.len(), 2, "1 and the squashed 2+3: {summaries:?}");

    // The squashed commit still carries both sets of changes.
    assert!(f.path().join("f2.txt").exists());
    assert!(f.path().join("f3.txt").exists());
    assert!(!f.path().join("f4.txt").exists());
}

#[test]
fn an_interactive_rebase_can_reword_without_an_editor() {
    let f = Fixture::linear(3);
    let base = commit_id(&f, "Commit 1");
    let mut plan = rebase::plan_from(&f.repo, base).expect("plan");

    plan.steps[0].action = rebase::StepAction::Reword;
    plan.steps[0].message = "A much better message".to_owned();

    rebase::rebase_interactive(f.path(), &plan, &Cancel::default(), |_| {})
        .expect("interactive rebase");

    let page = graph::build(&f.repo, 100, &Cancel::default()).expect("graph");
    let summaries: Vec<&str> = page
        .rows
        .iter()
        .map(|r| r.commit.summary.as_str())
        .collect();
    assert!(
        summaries.contains(&"A much better message"),
        "got {summaries:?}"
    );
    assert!(!summaries.contains(&"Commit 2"));
    assert!(summaries.contains(&"Commit 3"), "later commits survive");
}

#[test]
fn an_interactive_rebase_can_reorder() {
    let f = Fixture::linear(3);
    let base = commit_id(&f, "Commit 1");
    let mut plan = rebase::plan_from(&f.repo, base).expect("plan");
    let original: Vec<git2::Oid> = plan.steps.iter().map(|s| s.oid).collect();

    plan.steps.swap(0, 1);
    assert!(rebase::is_reordered(&plan, &original));
    assert!(!plan.is_noop(&original));

    rebase::rebase_interactive(f.path(), &plan, &Cancel::default(), |_| {})
        .expect("interactive rebase");

    let page = graph::build(&f.repo, 100, &Cancel::default()).expect("graph");
    let summaries: Vec<&str> = page
        .rows
        .iter()
        .map(|r| r.commit.summary.as_str())
        .collect();
    // Newest first, so the swapped pair reads 2 then 3.
    assert_eq!(summaries[0], "Commit 2", "got {summaries:?}");
    assert_eq!(summaries[1], "Commit 3");
}

#[test]
fn planning_across_a_merge_is_refused() {
    let f = Fixture::merged();
    let base = commit_id(&f, "A");
    let error = rebase::plan_from(&f.repo, base).expect_err("merge in range");
    assert!(error.user_message().contains("merge commit"));
}

#[test]
fn a_conflicted_index_blocks_committing() {
    let f = Fixture::conflicting();
    merge::merge(&f.repo, "theirs").expect("merge");
    let error = gitup::git::commit::commit(
        &f.repo,
        "Should not work\n",
        gitup::git::commit::CommitMode::Normal,
    )
    .expect_err("conflicts block commits");
    assert!(error.user_message().contains("conflicts"));
    let _ = Path::new("");
}
