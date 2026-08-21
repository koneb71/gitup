//! The job system end to end.
//!
//! These go through the real worker pool rather than calling the git layer
//! directly, so they cover the parts that only exist at that boundary:
//! serialization of writes, supersession of stale reads, and cancellation.

mod common;

use common::Fixture;
use gitup::git::commit::CommitMode;
use gitup::git::highlight::HighlightTheme;
use gitup::git::{repo, DiffTarget};
use gitup::job::{Job, JobSystem, Message, Mutation, Outcome};
use std::path::Path;
use std::time::{Duration, Instant};

/// Run the job system until `predicate` is satisfied, or fail.
///
/// The context is a bare `egui::Context`; workers only use it to request a
/// repaint, which is a no-op with no window attached.
fn pump<F>(jobs: &mut JobSystem, mut predicate: F) -> Vec<Outcome>
where
    F: FnMut(&[Outcome]) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut outcomes = Vec::new();
    let mut errors = Vec::new();

    while Instant::now() < deadline {
        for message in jobs.poll() {
            match message {
                Message::Done { outcome, .. } => outcomes.push(outcome),
                Message::Failed { error, .. } => errors.push(error.user_message()),
                Message::Progress { .. } => {}
            }
        }
        if !errors.is_empty() {
            panic!("job failed: {errors:?}");
        }
        if predicate(&outcomes) {
            return outcomes;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out; got {} outcomes, errors {errors:?}",
        outcomes.len()
    );
}

fn system() -> JobSystem {
    JobSystem::new(egui::Context::default())
}

fn index_content(repo: &git2::Repository, path: &str) -> String {
    let mut index = repo.index().expect("index");
    // This handle is not the one the worker wrote through, so its cached index
    // has to be reloaded before it can see the change.
    index.read(false).expect("reload index");
    let entry = index
        .get_path(Path::new(path), 0)
        .unwrap_or_else(|| panic!("{path} is not in the index"));
    String::from_utf8_lossy(repo.find_blob(entry.id).expect("blob").content()).into_owned()
}

#[test]
fn opening_a_repository_reports_its_head() {
    let f = Fixture::simple();
    let mut jobs = system();
    jobs.dispatch(Job::OpenRepo {
        path: f.path_buf(),
        token: 7,
    });

    let outcomes = pump(&mut jobs, |o| !o.is_empty());
    match &outcomes[0] {
        Outcome::RepoOpened {
            token,
            key,
            head,
            git_dir,
        } => {
            assert_eq!(*token, 7, "the answer names who asked");
            assert_eq!(key.path(), f.path().canonicalize().unwrap());
            assert_eq!(head.summary, "Add lib");
            assert_eq!(head.display_name(), "main");
            assert!(git_dir.ends_with(".git/"), "got {git_dir:?}");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn concurrent_opens_are_distinguishable_by_token() {
    // Two repositories opened at once must be tellable apart, or the answers
    // land in the wrong tabs.
    let one = Fixture::empty_named("one");
    one.commit_file("a.txt", "a\n", "One");
    let two = Fixture::empty_named("two");
    two.commit_file("b.txt", "b\n", "Two");

    let mut jobs = system();
    jobs.dispatch(Job::OpenRepo {
        path: one.path_buf(),
        token: 1,
    });
    jobs.dispatch(Job::OpenRepo {
        path: two.path_buf(),
        token: 2,
    });

    let outcomes = pump(&mut jobs, |o| {
        o.iter()
            .filter(|x| matches!(x, Outcome::RepoOpened { .. }))
            .count()
            == 2
    });

    for outcome in &outcomes {
        if let Outcome::RepoOpened { token, head, .. } = outcome {
            let expected = if *token == 1 { "One" } else { "Two" };
            assert_eq!(head.summary, expected, "token {token} got the wrong repo");
        }
    }
}

#[test]
fn staging_through_the_job_system_updates_the_index() {
    let f = Fixture::empty_named("jobstage");
    f.commit_file("f.txt", "one\n", "Add");
    f.write("f.txt", "one\ntwo\n");
    let key = repo::discover(f.path()).expect("discover");

    let mut jobs = system();
    jobs.dispatch(Job::Mutate {
        repo: key.clone(),
        action: Mutation::StageFiles(vec!["f.txt".to_owned()]),
    });
    pump(&mut jobs, |o| {
        o.iter().any(|x| matches!(x, Outcome::Mutated { .. }))
    });

    assert_eq!(index_content(&f.repo, "f.txt"), "one\ntwo\n");
}

#[test]
fn committing_through_the_job_system_moves_head() {
    let f = Fixture::empty_named("jobcommit");
    f.commit_file("f.txt", "one\n", "First");
    f.write("f.txt", "one\ntwo\n");
    let key = repo::discover(f.path()).expect("discover");

    let mut jobs = system();
    jobs.dispatch(Job::Mutate {
        repo: key.clone(),
        action: Mutation::StageAll,
    });
    jobs.dispatch(Job::Mutate {
        repo: key.clone(),
        action: Mutation::Commit {
            message: "Second commit\n".to_owned(),
            mode: CommitMode::Normal,
        },
    });
    let outcomes = pump(&mut jobs, |o| {
        o.iter()
            .filter(|x| matches!(x, Outcome::Mutated { .. }))
            .count()
            == 2
    });

    assert!(outcomes.iter().any(|o| matches!(
        o,
        Outcome::Mutated {
            moved_head: true,
            ..
        }
    )));

    let head = f.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap(), Some("Second commit"));
    assert_eq!(head.parent_count(), 1);
}

#[test]
fn mutations_are_serialized_even_when_dispatched_together() {
    // Two stages of different files, queued back to back. Both must land: if
    // they raced on the index, one would overwrite the other's write.
    let f = Fixture::empty_named("serial");
    f.commit_file("seed.txt", "seed\n", "Seed");
    for i in 0..8 {
        f.write(&format!("f{i}.txt"), &format!("content {i}\n"));
    }
    let key = repo::discover(f.path()).expect("discover");

    let mut jobs = system();
    for i in 0..8 {
        jobs.dispatch(Job::Mutate {
            repo: key.clone(),
            action: Mutation::StageFiles(vec![format!("f{i}.txt")]),
        });
    }
    pump(&mut jobs, |o| {
        o.iter()
            .filter(|x| matches!(x, Outcome::Mutated { .. }))
            .count()
            == 8
    });

    let mut index = f.repo.index().expect("index");
    index.read(false).expect("reload index");
    for i in 0..8 {
        assert!(
            index.get_path(Path::new(&format!("f{i}.txt")), 0).is_some(),
            "f{i}.txt was lost to a race"
        );
    }
}

#[test]
fn a_superseded_read_is_dropped_rather_than_delivered() {
    let f = Fixture::simple();
    let key = repo::discover(f.path()).expect("discover");
    let mut jobs = system();

    // Three status reads in a row. Only the newest answers a question anyone
    // is still asking, so the older two must not surface.
    for _ in 0..3 {
        jobs.dispatch(Job::ReadStatus {
            repo: key.clone(),
            include_ignored: false,
        });
    }

    let outcomes = pump(&mut jobs, |o| {
        // Give every worker a chance to finish before judging.
        !o.is_empty() && settled()
    });
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Status { .. }))
            .count(),
        1,
        "superseded status reads should be discarded"
    );
}

/// Wait long enough for any still-running worker to deliver, then report that
/// nothing more is coming. `pump` holds `&mut JobSystem`, so `is_busy` is not
/// reachable from inside the predicate.
fn settled() -> bool {
    std::thread::sleep(Duration::from_millis(60));
    true
}

#[test]
fn diffs_for_different_slots_do_not_cancel_each_other() {
    // Staged and unstaged are separate topics precisely so the working-tree
    // view can hold both at once.
    let f = Fixture::dirty();
    let key = repo::discover(f.path()).expect("discover");
    let mut jobs = system();

    jobs.dispatch(Job::LoadDiff {
        repo: key.clone(),
        target: DiffTarget::Staged,
        theme: HighlightTheme::Off,
    });
    jobs.dispatch(Job::LoadDiff {
        repo: key.clone(),
        target: DiffTarget::Unstaged,
        theme: HighlightTheme::Off,
    });

    let outcomes = pump(&mut jobs, |o| {
        o.iter()
            .filter(|x| matches!(x, Outcome::Diff { .. }))
            .count()
            == 2
    });

    let targets: Vec<DiffTarget> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Diff { target, .. } => Some(*target),
            _ => None,
        })
        .collect();
    assert!(targets.contains(&DiffTarget::Staged));
    assert!(targets.contains(&DiffTarget::Unstaged));
}

#[test]
fn a_failed_mutation_reports_an_error_rather_than_panicking() {
    let f = Fixture::simple();
    let key = repo::discover(f.path()).expect("discover");
    let mut jobs = system();

    jobs.dispatch(Job::Mutate {
        repo: key,
        action: Mutation::Commit {
            message: "   \n".to_owned(),
            mode: CommitMode::Normal,
        },
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_error = false;
    while Instant::now() < deadline && !saw_error {
        for message in jobs.poll() {
            if let Message::Failed { error, .. } = message {
                assert!(
                    error.user_message().contains("message"),
                    "got {:?}",
                    error.user_message()
                );
                saw_error = true;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saw_error, "an empty commit message should be refused");
}
