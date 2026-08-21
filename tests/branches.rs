//! Branch, tag, and stash operations.

mod common;

use common::Fixture;
use gitup::git::{branch, stash};

fn head_name(f: &Fixture) -> String {
    f.repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(str::to_owned))
        .unwrap_or_default()
}

#[test]
fn creating_a_branch_can_switch_to_it() {
    let f = Fixture::simple();
    branch::create(&f.repo, "feature", None, true).expect("create");
    assert_eq!(head_name(&f), "feature");

    // And creating without switching leaves you where you were.
    branch::create(&f.repo, "other", None, false).expect("create");
    assert_eq!(head_name(&f), "feature");
    assert!(f.repo.find_branch("other", git2::BranchType::Local).is_ok());
}

#[test]
fn a_duplicate_branch_name_is_refused_with_a_clear_message() {
    let f = Fixture::simple();
    branch::create(&f.repo, "dup", None, false).expect("create");
    let error = branch::create(&f.repo, "dup", None, false).expect_err("duplicate");
    assert!(error.user_message().contains("already exists"));
}

#[test]
fn branching_from_an_explicit_start_point() {
    let f = Fixture::simple();
    let root = f
        .repo
        .revparse_single("HEAD~2")
        .expect("HEAD~2")
        .peel_to_commit()
        .expect("commit");

    branch::create(&f.repo, "from-root", Some("HEAD~2"), false).expect("create");
    let created = f
        .repo
        .find_branch("from-root", git2::BranchType::Local)
        .expect("branch");
    assert_eq!(created.get().target(), Some(root.id()));
}

#[test]
fn switching_branches_updates_the_working_tree() {
    let f = Fixture::empty_named("switch");
    f.commit_file("shared.txt", "shared\n", "Base");
    branch::create(&f.repo, "side", None, true).expect("create");
    f.commit_file("only-on-side.txt", "side\n", "Side work");

    branch::checkout_branch(&f.repo, "main").expect("switch back");
    assert_eq!(head_name(&f), "main");
    assert!(
        !f.path().join("only-on-side.txt").exists(),
        "checkout must remove files that don't exist on the target branch"
    );

    branch::checkout_branch(&f.repo, "side").expect("switch forward");
    assert!(f.path().join("only-on-side.txt").exists());
}

#[test]
fn deleting_the_current_branch_is_refused() {
    let f = Fixture::simple();
    let error = branch::delete(&f.repo, "main", false).expect_err("should refuse");
    assert!(error.user_message().contains("branch you're on"));
}

#[test]
fn deleting_an_unmerged_branch_needs_force() {
    let f = Fixture::empty_named("unmerged");
    f.commit_file("base.txt", "base\n", "Base");
    branch::create(&f.repo, "orphaned", None, true).expect("create");
    f.commit_file("work.txt", "work\n", "Unmerged work");
    branch::checkout_branch(&f.repo, "main").expect("back to main");

    let error = branch::delete(&f.repo, "orphaned", false).expect_err("should warn");
    assert!(
        error.user_message().contains("aren't merged"),
        "got {:?}",
        error.user_message()
    );

    // Forcing goes through, because at that point it is an informed choice.
    branch::delete(&f.repo, "orphaned", true).expect("forced delete");
    assert!(f
        .repo
        .find_branch("orphaned", git2::BranchType::Local)
        .is_err());
}

#[test]
fn a_merged_branch_deletes_without_force() {
    let f = Fixture::merged();
    branch::delete(&f.repo, "feature", false).expect("merged branches are safe to delete");
}

#[test]
fn renaming_a_branch_keeps_its_commit() {
    let f = Fixture::simple();
    branch::create(&f.repo, "old-name", None, false).expect("create");
    let before = f
        .repo
        .find_branch("old-name", git2::BranchType::Local)
        .unwrap()
        .get()
        .target();

    branch::rename(&f.repo, "old-name", "new-name", false).expect("rename");
    let after = f
        .repo
        .find_branch("new-name", git2::BranchType::Local)
        .expect("renamed branch")
        .get()
        .target();

    assert_eq!(before, after);
    assert!(f
        .repo
        .find_branch("old-name", git2::BranchType::Local)
        .is_err());
}

#[test]
fn tags_can_be_annotated_or_lightweight() {
    let f = Fixture::simple();
    let head = f.repo.head().unwrap().peel_to_commit().unwrap().id();

    branch::create_tag(&f.repo, "light", head, None).expect("lightweight");
    branch::create_tag(&f.repo, "heavy", head, Some("Release notes")).expect("annotated");

    let tree =
        gitup::git::refs::build(&mut git2::Repository::open(f.path()).unwrap()).expect("refs");
    let light = tree.tags.iter().find(|t| t.name == "light").expect("light");
    let heavy = tree.tags.iter().find(|t| t.name == "heavy").expect("heavy");
    assert!(!light.annotated);
    assert!(heavy.annotated);
    assert_eq!(light.target, Some(head));
    assert_eq!(
        heavy.target,
        Some(head),
        "annotated tags peel to the commit"
    );

    branch::delete_tag(&f.repo, "light").expect("delete");
    assert!(branch::delete_tag(&f.repo, "light").is_err());
}

#[test]
fn checking_out_a_remote_branch_creates_a_tracking_branch() {
    // Simulate a fetched remote without a network: configure the remote, then
    // write the tracking ref it would have created.
    let f = Fixture::simple();
    f.repo
        .remote("origin", "https://example.invalid/repo.git")
        .expect("configure remote");
    let head = f.repo.head().unwrap().peel_to_commit().unwrap();
    f.repo
        .reference("refs/remotes/origin/feature", head.id(), true, "test setup")
        .expect("create remote ref");

    let created = branch::checkout_remote(&f.repo, "origin/feature").expect("checkout");
    assert_eq!(created, "feature");
    assert_eq!(head_name(&f), "feature");

    let local = f
        .repo
        .find_branch("feature", git2::BranchType::Local)
        .expect("local branch");
    assert_eq!(
        local.upstream().expect("upstream").name().unwrap(),
        Some("origin/feature"),
        "the new branch should track the remote it came from"
    );
}

#[test]
fn checking_out_a_remote_branch_whose_remote_is_gone_still_works() {
    // No `origin` in the config, only the leftover tracking ref.
    let f = Fixture::simple();
    let head = f.repo.head().unwrap().peel_to_commit().unwrap();
    f.repo
        .reference("refs/remotes/origin/orphan", head.id(), true, "test setup")
        .expect("create remote ref");

    let created = branch::checkout_remote(&f.repo, "origin/orphan").expect("checkout");
    assert_eq!(created, "orphan");
    assert_eq!(head_name(&f), "orphan");
    let local = f
        .repo
        .find_branch("orphan", git2::BranchType::Local)
        .expect("local branch");
    assert!(
        local.upstream().is_err(),
        "there is no remote to track, and that is not a failure"
    );
}

#[test]
fn checking_out_a_remote_branch_reuses_an_existing_local_one() {
    let f = Fixture::simple();
    let head = f.repo.head().unwrap().peel_to_commit().unwrap();
    f.repo
        .reference("refs/remotes/origin/main", head.id(), true, "test setup")
        .expect("create remote ref");

    // `main` already exists locally, so this should switch to it rather than
    // failing on a duplicate name.
    let created = branch::checkout_remote(&f.repo, "origin/main").expect("checkout");
    assert_eq!(created, "main");
    assert_eq!(head_name(&f), "main");
}

#[test]
fn stashing_clears_the_working_tree_and_popping_restores_it() {
    let f = Fixture::empty_named("stash");
    f.commit_file("f.txt", "committed\n", "Base");
    f.write("f.txt", "modified\n");
    f.write("new.txt", "untracked\n");

    let mut repo = git2::Repository::open(f.path()).expect("open");
    stash::save(&mut repo, Some("wip"), true, false).expect("stash");

    assert_eq!(
        std::fs::read_to_string(f.path().join("f.txt")).unwrap(),
        "committed\n",
        "stashing reverts tracked changes"
    );
    assert!(
        !f.path().join("new.txt").exists(),
        "untracked files were included, so they should be gone too"
    );

    stash::pop(&mut repo, 0).expect("pop");
    assert_eq!(
        std::fs::read_to_string(f.path().join("f.txt")).unwrap(),
        "modified\n"
    );
    assert!(f.path().join("new.txt").exists());
}

#[test]
fn stashing_a_clean_tree_says_there_is_nothing_to_stash() {
    let f = Fixture::simple();
    let mut repo = git2::Repository::open(f.path()).expect("open");
    let error = stash::save(&mut repo, None, false, false).expect_err("nothing to stash");
    assert!(error.user_message().contains("nothing to stash"));
}

#[test]
fn dropping_a_stash_removes_it_without_applying() {
    let f = Fixture::empty_named("stashdrop");
    f.commit_file("f.txt", "committed\n", "Base");
    f.write("f.txt", "modified\n");

    let mut repo = git2::Repository::open(f.path()).expect("open");
    stash::save(&mut repo, Some("discard me"), false, false).expect("stash");
    stash::drop(&mut repo, 0).expect("drop");

    let tree = gitup::git::refs::build(&mut repo).expect("refs");
    assert!(tree.stashes.is_empty());
    assert_eq!(
        std::fs::read_to_string(f.path().join("f.txt")).unwrap(),
        "committed\n",
        "dropping must not apply the stash"
    );
}
