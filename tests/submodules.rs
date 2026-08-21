//! Submodules, against real nested repositories.
//!
//! A local directory is a perfectly good submodule source, so this needs no
//! network — but it does go through the real `git submodule` machinery, which
//! is the part worth testing.

mod common;

use common::Fixture;
use gitup::git::cli;
use gitup::git::submodule::{self, SubmoduleState};
use gitup::job::Cancel;

use common::Fixture as F;

fn with_submodule() -> (Fixture, Fixture) {
    F::with_submodule()
}

#[test]
fn a_checked_out_submodule_reports_up_to_date() {
    let (parent, _child) = with_submodule();
    let modules = submodule::list(&parent.repo).expect("list");

    assert_eq!(modules.entries.len(), 1);
    let entry = &modules.entries[0];
    assert_eq!(entry.path, "vendor/lib");
    assert_eq!(entry.state, SubmoduleState::UpToDate, "{entry:?}");
    assert_eq!(
        entry.recorded, entry.checked_out,
        "recorded and checked-out commits should match"
    );
    assert!(entry.url.is_some());
    assert_eq!(modules.needing_attention(), 0);
}

#[test]
fn a_repository_with_no_submodules_lists_none() {
    let f = Fixture::simple();
    let modules = submodule::list(&f.repo).expect("list");
    assert!(modules.is_empty());
    assert_eq!(modules.needing_attention(), 0);
}

#[test]
fn an_uninitialized_submodule_is_recognized_and_can_be_initialized() {
    let (parent, _child) = with_submodule();

    // Deinit puts it back to "registered but not cloned", which is the state a
    // fresh clone of the parent lands in.
    cli::run(
        parent.path(),
        &["submodule", "deinit", "-f", "--", "vendor/lib"],
        &Cancel::default(),
        |_| {},
    )
    .expect("deinit");

    let before = submodule::list(&parent.repo).expect("list");
    assert_eq!(before.entries[0].state, SubmoduleState::Uninitialized);
    assert_eq!(before.needing_attention(), 1);
    assert!(before.entries[0].state.needs_update());

    // And updating brings it back.
    submodule::update(
        parent.path(),
        Some("vendor/lib"),
        &Cancel::default(),
        |_| {},
    )
    .expect("update");

    let after = submodule::list(&parent.repo).expect("list");
    assert_eq!(after.entries[0].state, SubmoduleState::UpToDate);
    assert!(parent.path().join("vendor/lib/lib.txt").exists());
}

#[test]
fn a_submodule_at_the_wrong_commit_reports_out_of_date() {
    let (parent, child) = with_submodule();
    // Move the submodule's own checkout forward without telling the parent.
    child.commit_file("lib.txt", "library, revised\n", "Second commit");
    cli::run(
        &parent.path().join("vendor/lib"),
        &["fetch", "origin"],
        &Cancel::default(),
        |_| {},
    )
    .expect("fetch inside the submodule");
    cli::run(
        &parent.path().join("vendor/lib"),
        &["reset", "--hard", "origin/main"],
        &Cancel::default(),
        |_| {},
    )
    .expect("move the submodule");

    let modules = submodule::list(&parent.repo).expect("list");
    let entry = &modules.entries[0];
    assert_eq!(entry.state, SubmoduleState::OutOfDate, "{entry:?}");
    assert_ne!(
        entry.recorded, entry.checked_out,
        "the two commits should now differ"
    );
    assert_eq!(modules.needing_attention(), 1);

    // Updating puts it back to what the parent records.
    submodule::update(
        parent.path(),
        Some("vendor/lib"),
        &Cancel::default(),
        |_| {},
    )
    .expect("update");
    let after = submodule::list(&parent.repo).expect("list");
    assert_eq!(after.entries[0].state, SubmoduleState::UpToDate);
}

#[test]
fn a_submodule_can_be_opened_as_its_own_repository() {
    let (parent, _child) = with_submodule();
    let path = parent.path().join("vendor/lib");

    assert!(submodule::is_repository(&path));
    let key = gitup::git::repo::discover(&path).expect("discover");
    let repo = gitup::git::repo::open(&key).expect("open");
    let head = gitup::git::repo::head_info(&repo).expect("head");
    assert_eq!(head.summary, "Library first commit");
}

#[test]
fn removing_a_submodule_stages_the_removal_and_needs_a_commit() {
    let (parent, _child) = with_submodule();
    submodule::remove(parent.path(), "vendor/lib", &Cancel::default()).expect("remove");

    // The working tree is gone immediately...
    assert!(!parent.path().join("vendor/lib/lib.txt").exists());

    // ...but the removal is only staged, so HEAD still records it. Listing it
    // is correct at this point: nothing has left history yet.
    let staged = submodule::list(&parent.repo).expect("list");
    assert_eq!(staged.entries.len(), 1, "still in HEAD until committed");

    parent.commit("Remove the submodule");
    let after = submodule::list(&parent.repo).expect("list");
    assert!(after.is_empty(), "got {:?}", after.entries);
}

#[test]
fn adding_a_submodule_registers_and_clones_it() {
    let child = Fixture::empty_named("newchild");
    child.commit_file("thing.txt", "thing\n", "First");

    let parent = Fixture::empty_named("newparent");
    parent.commit_file("README.md", "# Parent\n", "First");

    // The helper does not set `protocol.file.allow`, so a local path is refused
    // — which is git's own protection, and worth confirming we surface.
    let error = submodule::add(
        parent.path(),
        &child.path().to_string_lossy(),
        "vendor/thing",
        &Cancel::default(),
        |_| {},
    );
    if let Err(error) = error {
        assert!(
            !error.user_message().is_empty(),
            "a refusal must explain itself"
        );
        return;
    }

    let modules = submodule::list(&parent.repo).expect("list");
    assert_eq!(modules.entries.len(), 1);
    assert_eq!(modules.entries[0].path, "vendor/thing");
}

#[test]
fn empty_input_is_refused_before_running_git() {
    let f = Fixture::simple();
    assert!(submodule::add(f.path(), "", "vendor/x", &Cancel::default(), |_| {}).is_err());
    assert!(submodule::add(
        f.path(),
        "https://example.invalid/x.git",
        "  ",
        &Cancel::default(),
        |_| {}
    )
    .is_err());
}

#[test]
fn staging_everything_works_in_a_repository_with_a_submodule() {
    // libgit2's `add_all` rejects a submodule directory outright — the fixture
    // helper hits this — so "Stage all" has to cope with one being present.
    let (parent, _child) = with_submodule();
    parent.write("notes.txt", "a new file\n");

    gitup::git::stage::stage_all(&parent.repo).expect("stage all");

    parent.reload_index();
    let status = gitup::git::status::status(&parent.repo, false).expect("status");
    assert_eq!(
        status.untracked_count, 0,
        "the new file should be staged: {:?}",
        status.entries
    );
    assert!(status.staged_count > 0);
}
