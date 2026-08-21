//! Pulling, from the git layer up through the app.
//!
//! Written after a report of "I can't pull". The repository in question had no
//! remote at all — so git could not pull either — but the app said nothing
//! about why, which is the actual defect these tests pin down.

mod common;

use common::Fixture;
use git2::Repository;
use gitup::app::GitupApp;
use gitup::git::remote::{self, PushMode};
use gitup::git::repo as repo_info;
use gitup::job::Cancel;
use gitup::settings::Settings;
use gitup::ui::command::Command;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn settle(app: &mut GitupApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.tick(ctx);
        if !app.is_busy_for_test() {
            app.tick(ctx);
            if !app.is_busy_for_test() {
                return;
            }
        }
        assert!(Instant::now() < deadline, "jobs never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A bare origin, a publisher pushing to it, and a clone that is behind.
fn clone_that_is_behind() -> (tempfile::TempDir, PathBuf, Fixture) {
    let origin_dir = tempfile::tempdir().expect("tempdir");
    let bare = Repository::init_bare(origin_dir.path()).expect("init bare");
    bare.set_head("refs/heads/main").expect("set head");
    let url = origin_dir.path().to_string_lossy().into_owned();

    let publisher = Fixture::empty_named("publisher");
    publisher.commit_file("a.txt", "one\n", "First");
    remote::add_remote(publisher.path(), "origin", &url, &Cancel::default()).expect("remote");
    remote::push(
        publisher.path(),
        "origin",
        "main",
        true,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .expect("push");

    let target = tempfile::tempdir().expect("tempdir");
    let clone_path =
        remote::clone(target.path(), &url, "clone", &Cancel::default(), |_| {}).expect("clone");

    // Publish something the clone does not have yet.
    publisher.commit_file("b.txt", "two\n", "Second");
    remote::push(
        publisher.path(),
        "origin",
        "main",
        false,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .expect("push again");

    // `target` is returned so the clone outlives this function.
    std::mem::forget(target);
    (origin_dir, clone_path, publisher)
}

#[test]
fn a_repository_with_no_remote_reports_why_it_cannot_pull() {
    // The reported case: nothing to pull from, and the app has to say so.
    let ctx = egui::Context::default();
    let solo = Fixture::simple();
    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(solo.path_buf()));
    settle(&mut app, &ctx);

    let reason = app
        .pull_blocker_for_test()
        .expect("pulling should be reported as unavailable");
    assert!(
        reason.contains("no remotes"),
        "the reason must name the actual problem, got {reason:?}"
    );
}

#[test]
fn a_repository_with_no_commits_says_so_rather_than_offering_a_pull() {
    let ctx = egui::Context::default();
    let empty = Fixture::empty();
    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(empty.path_buf()));
    settle(&mut app, &ctx);

    let reason = app.pull_blocker_for_test().expect("unavailable");
    assert!(reason.contains("no commits"), "got {reason:?}");
}

#[test]
fn a_branch_with_a_remote_but_no_upstream_explains_how_to_set_one() {
    let ctx = egui::Context::default();
    let f = Fixture::simple();
    f.repo
        .remote("origin", "https://example.invalid/repo.git")
        .expect("add remote");

    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(f.path_buf()));
    settle(&mut app, &ctx);

    let reason = app.pull_blocker_for_test().expect("unavailable");
    assert!(
        reason.contains("isn't tracking"),
        "should point at the missing upstream, got {reason:?}"
    );
}

#[test]
fn a_detached_head_says_to_check_out_a_branch() {
    let ctx = egui::Context::default();
    let f = Fixture::simple();
    let head = f.repo.head().unwrap().peel_to_commit().unwrap().id();
    gitup::git::branch::checkout_commit(&f.repo, head).expect("detach");

    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(f.path_buf()));
    settle(&mut app, &ctx);

    let reason = app.pull_blocker_for_test().expect("unavailable");
    assert!(reason.contains("detached"), "got {reason:?}");
}

#[test]
fn a_clone_can_pull() {
    let (_origin, clone_path, _publisher) = clone_that_is_behind();
    let ctx = egui::Context::default();

    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(clone_path.clone()));
    settle(&mut app, &ctx);

    assert_eq!(
        app.pull_blocker_for_test(),
        None,
        "a fresh clone tracks its origin, so pulling is available"
    );

    assert!(!clone_path.join("b.txt").exists(), "behind to begin with");
    app.run_command_for_test(&ctx, Command::Pull);
    settle(&mut app, &ctx);

    assert!(
        clone_path.join("b.txt").exists(),
        "pulling should have brought the new commit down"
    );
}

#[test]
fn tracking_configured_without_a_fetched_ref_still_allows_pulling() {
    // The over-strict case: `branch.main.remote` is set but the local
    // remote-tracking ref does not exist, so resolving the upstream fails.
    // git pulls happily in this state, and so should the app.
    let (_origin, clone_path, _publisher) = clone_that_is_behind();
    let clone = Repository::open(&clone_path).expect("open");

    // Delete the tracking ref, leaving the configuration behind.
    clone
        .find_reference("refs/remotes/origin/main")
        .expect("tracking ref")
        .delete()
        .expect("delete it");

    let key = repo_info::discover(&clone_path).expect("discover");
    let reopened = repo_info::open(&key).expect("open");
    let head = repo_info::head_info(&reopened).expect("head");

    assert!(
        head.upstream.is_none(),
        "the upstream ref really is unresolvable now"
    );
    assert!(
        head.tracking_configured,
        "but the configuration is still there"
    );
    assert!(head.can_pull(), "so pulling is possible");

    // And it genuinely works.
    remote::pull(
        &clone_path,
        remote::PullMode::FastForwardOnly,
        &Cancel::default(),
        |_| {},
    )
    .expect("pull should succeed");
    assert!(clone_path.join("b.txt").exists());
}

#[test]
fn pulling_with_nothing_to_pull_is_not_an_error() {
    let (_origin, clone_path, _publisher) = clone_that_is_behind();
    remote::pull(
        &clone_path,
        remote::PullMode::Merge,
        &Cancel::default(),
        |_| {},
    )
    .expect("first pull");

    // A second pull has nothing to do; that is a success, not a failure.
    let summary = remote::pull(
        &clone_path,
        remote::PullMode::Merge,
        &Cancel::default(),
        |_| {},
    )
    .expect("second pull should succeed");
    assert!(!summary.is_empty());
}

#[test]
fn a_pull_that_git_refuses_surfaces_gits_explanation() {
    // No tracking information: git's own message names the problem and the fix,
    // so it is worth passing through rather than replacing.
    let f = Fixture::simple();
    let error = remote::pull(
        f.path(),
        remote::PullMode::Merge,
        &Cancel::default(),
        |_| {},
    )
    .expect_err("no upstream means no pull");

    let message = error.user_message().to_lowercase();
    assert!(
        message.contains("tracking") || message.contains("no such"),
        "unhelpful message: {message}"
    );
}
