//! Network operations, exercised against local remotes.
//!
//! A bare repository on disk is a perfectly good git remote, so clone, fetch,
//! push, and pull can all be tested end to end — through the real `git` binary,
//! with real refspecs and real progress output — without touching a network or
//! depending on credentials.

mod common;

use common::Fixture;
use git2::Repository;
use gitup::git::remote::{self, PullMode, PushMode};
use gitup::job::{Cancel, Progress};
use tempfile::TempDir;

/// A bare repository to act as `origin`.
struct Origin {
    dir: TempDir,
}

impl Origin {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repository::init_bare(dir.path()).expect("init bare");
        // A bare repo's HEAD defaults to whatever `init.defaultBranch` says,
        // which may not be `main`. A clone follows the remote's HEAD, so
        // leaving it pointing at a branch that will never exist makes every
        // clone land with no checked-out branch.
        repo.set_head("refs/heads/main").expect("set head");
        Self { dir }
    }

    fn url(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    fn repo(&self) -> Repository {
        Repository::open_bare(self.dir.path()).expect("open bare")
    }
}

fn collect(progress: &std::sync::Mutex<Vec<Progress>>) -> impl FnMut(Progress) + '_ {
    move |p| progress.lock().unwrap().push(p)
}

#[test]
fn push_then_clone_round_trips_through_a_real_remote() {
    let origin = Origin::new();
    let source = Fixture::simple();

    remote::add_remote(source.path(), "origin", &origin.url(), &Cancel::default())
        .expect("add remote");

    let progress = std::sync::Mutex::new(Vec::new());
    remote::push(
        source.path(),
        "origin",
        "main",
        true,
        PushMode::Normal,
        &Cancel::default(),
        collect(&progress),
    )
    .expect("push");

    // The remote now has the branch.
    let bare = origin.repo();
    let head = bare
        .find_reference("refs/heads/main")
        .expect("main on the remote")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(head.summary().unwrap(), Some("Add lib"));

    // And pushing set up tracking, so the local branch knows where it went.
    let branch = source
        .repo
        .find_branch("main", git2::BranchType::Local)
        .expect("main");
    let upstream = branch.upstream().expect("upstream after --set-upstream");
    assert_eq!(upstream.name().unwrap(), Some("origin/main"));

    // Clone it back out and confirm the history survived the trip.
    let target = tempfile::tempdir().expect("tempdir");
    let path = remote::clone(
        target.path(),
        &origin.url(),
        "clone",
        &Cancel::default(),
        collect(&progress),
    )
    .expect("clone");

    let cloned = Repository::open(&path).expect("open clone");
    let cloned_head = cloned.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(cloned_head.id(), head.id());

    // Progress was actually reported, which is what the status bar renders.
    let seen = progress.lock().unwrap();
    assert!(
        !seen.is_empty(),
        "expected progress output from push and clone"
    );
    assert!(seen.iter().all(|p| !p.label.is_empty()));
}

#[test]
fn fetch_brings_down_new_commits_without_touching_the_working_tree() {
    let origin = Origin::new();

    // Publisher pushes an initial commit.
    let publisher = Fixture::empty_named("publisher");
    publisher.commit_file("a.txt", "one\n", "First");
    remote::add_remote(
        publisher.path(),
        "origin",
        &origin.url(),
        &Cancel::default(),
    )
    .expect("add remote");
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

    // Consumer clones, then the publisher adds another commit.
    let target = tempfile::tempdir().expect("tempdir");
    let consumer_path = remote::clone(
        target.path(),
        &origin.url(),
        "consumer",
        &Cancel::default(),
        |_| {},
    )
    .expect("clone");

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

    remote::fetch(&consumer_path, "origin", true, &Cancel::default(), |_| {}).expect("fetch");

    let consumer = Repository::open(&consumer_path).expect("open");
    // The remote-tracking ref moved...
    let tracking = consumer
        .find_branch("origin/main", git2::BranchType::Remote)
        .expect("origin/main")
        .get()
        .peel_to_commit()
        .expect("commit");
    assert_eq!(tracking.summary().unwrap(), Some("Second"));

    // ...but fetch does not touch the working tree, so the file isn't there yet.
    assert!(
        !consumer_path.join("b.txt").exists(),
        "fetch must not modify the working tree"
    );
    let local = consumer.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(local.summary().unwrap(), Some("First"));
}

#[test]
fn pull_fast_forwards_the_working_tree() {
    let origin = Origin::new();
    let publisher = Fixture::empty_named("pub2");
    publisher.commit_file("a.txt", "one\n", "First");
    remote::add_remote(
        publisher.path(),
        "origin",
        &origin.url(),
        &Cancel::default(),
    )
    .unwrap();
    remote::push(
        publisher.path(),
        "origin",
        "main",
        true,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .unwrap();

    let target = tempfile::tempdir().unwrap();
    let consumer_path = remote::clone(
        target.path(),
        &origin.url(),
        "c",
        &Cancel::default(),
        |_| {},
    )
    .unwrap();

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
    .unwrap();

    remote::pull(
        &consumer_path,
        PullMode::FastForwardOnly,
        &Cancel::default(),
        |_| {},
    )
    .expect("pull");

    assert!(
        consumer_path.join("b.txt").exists(),
        "pull updates the tree"
    );
    let consumer = Repository::open(&consumer_path).unwrap();
    let head = consumer.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap(), Some("Second"));
}

#[test]
fn a_rejected_push_reports_why() {
    let origin = Origin::new();

    let a = Fixture::empty_named("a");
    a.commit_file("f.txt", "a\n", "From A");
    remote::add_remote(a.path(), "origin", &origin.url(), &Cancel::default()).unwrap();
    remote::push(
        a.path(),
        "origin",
        "main",
        true,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .unwrap();

    // B has unrelated history and tries to push over it.
    let b = Fixture::empty_named("b");
    b.commit_file("f.txt", "b\n", "From B");
    remote::add_remote(b.path(), "origin", &origin.url(), &Cancel::default()).unwrap();

    let error = remote::push(
        b.path(),
        "origin",
        "main",
        false,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .expect_err("a non-fast-forward push must be rejected");

    let message = error.user_message().to_lowercase();
    assert!(
        message.contains("reject") || message.contains("fetch first") || message.contains("failed"),
        "unhelpful message: {message}"
    );
}

#[test]
fn force_with_lease_succeeds_when_the_remote_is_where_we_last_saw_it() {
    let origin = Origin::new();
    let a = Fixture::empty_named("lease");
    a.commit_file("f.txt", "one\n", "First");
    remote::add_remote(a.path(), "origin", &origin.url(), &Cancel::default()).unwrap();
    remote::push(
        a.path(),
        "origin",
        "main",
        true,
        PushMode::Normal,
        &Cancel::default(),
        |_| {},
    )
    .unwrap();

    // Rewrite history locally, then force-push over it.
    a.write("f.txt", "rewritten\n");
    a.stage("f.txt");
    let sig = git2::Signature::new(
        "Fixture",
        "fixture@example.com",
        &git2::Time::new(1_704_070_800, 0),
    )
    .unwrap();
    let tree_id = a.repo.index().unwrap().write_tree().unwrap();
    let tree = a.repo.find_tree(tree_id).unwrap();
    // Amend rather than commit: this is a rewrite of the pushed history, which
    // is exactly the situation force-with-lease exists for.
    let head = a.repo.head().unwrap().peel_to_commit().unwrap();
    head.amend(
        Some("HEAD"),
        None,
        Some(&sig),
        None,
        Some("Rewritten"),
        Some(&tree),
    )
    .unwrap();

    remote::push(
        a.path(),
        "origin",
        "main",
        false,
        PushMode::ForceWithLease,
        &Cancel::default(),
        |_| {},
    )
    .expect("force-with-lease should be accepted");

    let bare = origin.repo();
    let head = bare
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert_eq!(head.summary().unwrap(), Some("Rewritten"));
}

#[test]
fn a_cancelled_fetch_stops_and_reports_cancellation() {
    let origin = Origin::new();
    let f = Fixture::simple();
    remote::add_remote(f.path(), "origin", &origin.url(), &Cancel::default()).unwrap();

    let cancel = Cancel::default();
    cancel.cancel();

    let error = remote::fetch(f.path(), "origin", false, &cancel, |_| {})
        .expect_err("a cancelled fetch must not report success");
    assert!(error.is_cancelled(), "got {error:?}");
}
