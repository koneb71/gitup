//! Several repositories open at once.
//!
//! These drive the real app rather than the tab arithmetic in isolation: the
//! part worth checking is that a session keeps its own state and keeps
//! receiving job results while another tab is showing.

mod common;

use common::Fixture;
use gitup::app::GitupApp;
use gitup::settings::Settings;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Build an app with `first` open, using settings that never touch disk.
fn app(ctx: &egui::Context, first: Option<PathBuf>) -> GitupApp {
    GitupApp::new_in(ctx, Settings::ephemeral(), first)
}

/// Run the app until every dispatched job has landed.
fn settle(app: &mut GitupApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.tick(ctx);
        if !app.is_busy_for_test() {
            // One more pass, so results that arrived on this tick are applied.
            app.tick(ctx);
            if !app.is_busy_for_test() {
                return;
            }
        }
        assert!(Instant::now() < deadline, "jobs never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn opening_several_repositories_gives_each_its_own_tab() {
    let ctx = egui::Context::default();
    let one = Fixture::simple();
    let two = Fixture::branchy();

    let mut app = app(&ctx, Some(one.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(two.path_buf());
    settle(&mut app, &ctx);

    assert_eq!(app.tab_titles_for_test(), vec!["fixture", "branchy"]);
    assert_eq!(
        app.active_tab_for_test(),
        1,
        "the new tab is the one showing"
    );
    assert_eq!(app.tab_head_for_test(0).as_deref(), Some("Add lib"));
    assert_eq!(
        app.tab_head_for_test(1).as_deref(),
        // `branchy` finishes on `main`, whose tip is the merge.
        Some("Merge feature-a"),
        "each tab loaded its own repository"
    );
}

#[test]
fn opening_several_at_once_loads_all_of_them() {
    // The restore-on-launch path opens every remembered tab in one go, so all
    // their loads are in flight together. Each has to survive: work for one
    // repository must not cancel the identical work for another.
    let ctx = egui::Context::default();
    let one = Fixture::simple();
    let two = Fixture::branchy();
    let three = Fixture::merged();

    let mut app = app(&ctx, Some(one.path_buf()));
    // Deliberately no settling between opens.
    app.open_in_new_tab_for_test(two.path_buf());
    app.open_in_new_tab_for_test(three.path_buf());
    settle(&mut app, &ctx);

    assert_eq!(app.tab_titles_for_test().len(), 3);
    for position in 0..3 {
        assert!(
            app.tab_has_graph_for_test(position),
            "tab {position} never got its history"
        );
        assert!(
            app.tab_has_refs_for_test(position),
            "tab {position} never got its branches"
        );
        assert!(
            app.tab_has_status_for_test(position),
            "tab {position} never got its status"
        );
    }
}

#[test]
fn opening_a_repository_that_is_already_open_switches_to_its_tab() {
    let ctx = egui::Context::default();
    let one = Fixture::simple();
    let two = Fixture::branchy();

    let mut app = app(&ctx, Some(one.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(two.path_buf());
    settle(&mut app, &ctx);

    app.open_in_new_tab_for_test(one.path_buf());
    settle(&mut app, &ctx);

    assert_eq!(
        app.tab_titles_for_test().len(),
        2,
        "no duplicate tab for the same repository"
    );
    assert_eq!(app.active_tab_for_test(), 0, "switched to the existing tab");
}

#[test]
fn switching_tabs_preserves_their_order() {
    let ctx = egui::Context::default();
    let a = Fixture::empty_named("alpha");
    a.commit_file("a.txt", "a\n", "Alpha commit");
    let b = Fixture::empty_named("beta");
    b.commit_file("b.txt", "b\n", "Beta commit");
    let c = Fixture::empty_named("gamma");
    c.commit_file("c.txt", "c\n", "Gamma commit");

    let mut app = app(&ctx, Some(a.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(b.path_buf());
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(c.path_buf());
    settle(&mut app, &ctx);

    let order = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    assert_eq!(app.tab_titles_for_test(), order);

    // Switch around; the bar must not shuffle.
    for position in [0usize, 2, 1, 0, 1, 2] {
        app.activate_for_test(position);
        assert_eq!(
            app.tab_titles_for_test(),
            order,
            "activating {position} reordered the tabs"
        );
        assert_eq!(app.active_tab_for_test(), position);
    }
}

#[test]
fn each_tab_keeps_its_own_selection() {
    let ctx = egui::Context::default();
    let a = Fixture::simple();
    let b = Fixture::branchy();

    let mut app = app(&ctx, Some(a.path_buf()));
    settle(&mut app, &ctx);

    // Pick a specific commit in the first tab.
    let target = gitup::git::graph::build(&a.repo, 100, &gitup::job::Cancel::default())
        .expect("graph")
        .rows
        .last()
        .expect("a commit")
        .commit
        .id;
    app.select_commit_for_test(target);
    assert_eq!(app.tab_selection_for_test(0), Some(target));

    app.open_in_new_tab_for_test(b.path_buf());
    settle(&mut app, &ctx);
    assert_ne!(
        app.tab_selection_for_test(1),
        Some(target),
        "the new tab has its own selection"
    );

    app.activate_for_test(0);
    assert_eq!(
        app.tab_selection_for_test(0),
        Some(target),
        "going back should return you where you were"
    );
}

#[test]
fn a_background_tab_still_receives_results() {
    // The point of keeping a tab open is glancing at it. A hidden tab that
    // stopped updating would report whatever was true when you left it.
    let ctx = egui::Context::default();
    let watched = Fixture::simple();
    let other = Fixture::branchy();

    let mut app = app(&ctx, Some(watched.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(other.path_buf());
    settle(&mut app, &ctx);
    assert_eq!(app.active_tab_for_test(), 1);
    assert_eq!(app.tab_change_count_for_test(0), Some(0), "starts clean");

    // Change the *background* repository, then ask the app to refresh it.
    watched.write("new-file.txt", "appeared while hidden\n");
    app.refresh_tab_for_test(0);
    settle(&mut app, &ctx);

    assert_eq!(
        app.tab_change_count_for_test(0),
        Some(1),
        "the hidden tab picked up the change"
    );
    assert_eq!(app.active_tab_for_test(), 1, "and stayed hidden");
}

#[test]
fn work_in_one_tab_does_not_block_another() {
    // Progress used to be a single slot, so a fetch running anywhere greyed out
    // the remote buttons everywhere and drew its progress bar in whichever tab
    // happened to be showing.
    let ctx = egui::Context::default();
    let busy = Fixture::simple();
    let other = Fixture::branchy();

    let mut app = app(&ctx, Some(busy.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(other.path_buf());
    settle(&mut app, &ctx);

    // Pretend the first repository has an operation under way.
    app.set_progress_for_test(0, "Receiving objects");

    assert!(
        app.tab_is_busy_for_test(0),
        "the tab doing the work is marked busy"
    );
    assert!(!app.tab_is_busy_for_test(1), "the other one is not");
    assert!(
        !app.current_is_busy_for_test(),
        "and the tab being shown is free to act"
    );

    // Switching to the busy tab does report it as busy.
    app.activate_for_test(0);
    assert!(app.current_is_busy_for_test());
}

#[test]
fn closing_a_tab_promotes_the_next_one() {
    let ctx = egui::Context::default();
    let a = Fixture::empty_named("alpha");
    a.commit_file("a.txt", "a\n", "Alpha");
    let b = Fixture::empty_named("beta");
    b.commit_file("b.txt", "b\n", "Beta");
    let c = Fixture::empty_named("gamma");
    c.commit_file("c.txt", "c\n", "Gamma");

    let mut app = app(&ctx, Some(a.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(b.path_buf());
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(c.path_buf());
    settle(&mut app, &ctx);

    // Close the middle one while it is not showing.
    app.close_tab_for_test(1);
    assert_eq!(app.tab_titles_for_test(), vec!["alpha", "gamma"]);
    assert_eq!(
        app.active_tab_for_test(),
        1,
        "gamma is still the one showing"
    );

    // Close the visible one; the remaining tab takes over.
    app.close_tab_for_test(1);
    assert_eq!(app.tab_titles_for_test(), vec!["alpha"]);
    assert_eq!(app.active_tab_for_test(), 0);
    assert_eq!(app.tab_head_for_test(0).as_deref(), Some("Alpha"));
}

#[test]
fn closing_the_last_tab_returns_to_the_empty_state() {
    let ctx = egui::Context::default();
    let only = Fixture::simple();

    let mut app = app(&ctx, Some(only.path_buf()));
    settle(&mut app, &ctx);
    assert_eq!(app.tab_titles_for_test().len(), 1);

    app.close_tab_for_test(0);
    assert!(
        app.tab_titles_for_test().is_empty(),
        "nothing open means nothing in the bar"
    );
}

#[test]
fn tabs_are_remembered_for_the_next_launch() {
    let ctx = egui::Context::default();
    let a = Fixture::empty_named("first");
    a.commit_file("a.txt", "a\n", "First");
    let b = Fixture::empty_named("second");
    b.commit_file("b.txt", "b\n", "Second");

    // A settings object that records, but still never writes to disk.
    let mut app = GitupApp::new_in(&ctx, Settings::ephemeral(), Some(a.path_buf()));
    settle(&mut app, &ctx);
    app.open_in_new_tab_for_test(b.path_buf());
    settle(&mut app, &ctx);

    let remembered = app.settings_for_test().open_tabs.clone();
    assert_eq!(remembered.len(), 2, "both tabs recorded");
    assert!(remembered[0].ends_with("first"));
    assert!(remembered[1].ends_with("second"));

    // Reopening with those settings restores both, in order.
    let mut restored_settings = Settings::ephemeral();
    restored_settings.open_tabs = remembered;
    let mut restored = GitupApp::new_in(&ctx, restored_settings, None);
    settle(&mut restored, &ctx);
    assert_eq!(restored.tab_titles_for_test(), vec!["first", "second"]);

    // Restoring opens every tab at once, so this is where cross-repository
    // interference shows up: each must end up fully loaded, not just present.
    for position in 0..2 {
        assert!(
            restored.tab_has_graph_for_test(position),
            "restored tab {position} has no history"
        );
        assert!(
            restored.tab_has_refs_for_test(position),
            "restored tab {position} has no branches"
        );
        assert!(
            restored.tab_has_status_for_test(position),
            "restored tab {position} has no status"
        );
    }
}

#[test]
fn a_repository_that_moved_away_is_dropped_rather_than_erroring() {
    let ctx = egui::Context::default();
    let present = Fixture::empty_named("present");
    present.commit_file("a.txt", "a\n", "Present");

    let mut settings = Settings::ephemeral();
    settings.open_tabs = vec![
        present.path_buf(),
        PathBuf::from("/nowhere/that/exists/anymore"),
    ];

    let mut app = GitupApp::new_in(&ctx, settings, None);
    settle(&mut app, &ctx);
    assert_eq!(
        app.tab_titles_for_test(),
        vec!["present"],
        "the missing one is simply not restored"
    );
}

#[test]
fn a_superseded_diff_request_does_not_block_asking_again() {
    // Selecting one commit and then another before the first diff arrives
    // supersedes the first job — correctly. But its request stayed recorded as
    // outstanding, and coming back to that commit found it "already asked for"
    // and never asked again: the pane sat on "Computing diff…" forever.
    //
    // Nothing is recorded caller-side any more: the job system is asked
    // whether identical work is already in flight, so a cancelled request is
    // simply no longer pending.
    let ctx = egui::Context::default();
    let f = Fixture::simple();
    let mut app = app(&ctx, Some(f.path_buf()));
    settle(&mut app, &ctx);

    let page =
        gitup::git::graph::build(&f.repo, 100, &gitup::job::Cancel::default()).expect("graph");
    let target = page.rows[1].commit.id;

    app.select_commit_for_test(target);
    settle(&mut app, &ctx);

    assert_eq!(
        app.loaded_commit_diff_for_test(),
        Some(target),
        "a stranded request must not stop the diff being fetched"
    );

    // Selecting away and back must load each one in turn, however fast the
    // switching: nothing is remembered that could go stale.
    let other = page.rows[0].commit.id;
    for _ in 0..3 {
        app.select_commit_for_test(other);
        app.tick_for_test(&ctx);
        app.select_commit_for_test(target);
        settle(&mut app, &ctx);
        assert_eq!(app.loaded_commit_diff_for_test(), Some(target));
    }
}

#[test]
fn revisiting_a_commit_after_a_quick_switch_still_loads_its_diff() {
    // Selecting A then B before A's diff arrives supersedes A's job — correctly.
    // But the request was still recorded as outstanding, so coming back to A
    // found it "already requested" and never asked again: the detail pane sat
    // on "Computing diff…" forever.
    let ctx = egui::Context::default();
    let f = Fixture::simple();
    let mut app = app(&ctx, Some(f.path_buf()));
    settle(&mut app, &ctx);

    let page =
        gitup::git::graph::build(&f.repo, 100, &gitup::job::Cancel::default()).expect("graph");
    let first = page.rows[0].commit.id;
    let second = page.rows[1].commit.id;

    // Select one, let the request go out, then switch before it lands.
    app.select_commit_for_test(first);
    app.tick_for_test(&ctx);
    app.select_commit_for_test(second);
    settle(&mut app, &ctx);
    assert_eq!(app.loaded_commit_diff_for_test(), Some(second));

    // Back to the first: its diff has to be fetched again.
    app.select_commit_for_test(first);
    settle(&mut app, &ctx);
    assert_eq!(
        app.loaded_commit_diff_for_test(),
        Some(first),
        "the diff for the revisited commit never loaded"
    );
}

#[test]
fn cancelling_the_folder_picker_lets_you_try_again() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, None);

    // A cancelled picker sends no path, but it still has to report back.
    // Without that, the app believes a dialog is still open and every later
    // attempt to open a repository is dropped on the floor.
    app.simulate_pick_for_test(None);
    assert!(
        !app.picker_is_open_for_test(),
        "cancelling left the app believing a picker was still open"
    );

    let fixture = Fixture::linear(2);
    app.simulate_pick_for_test(Some(fixture.path().to_path_buf()));
    settle(&mut app, &ctx);
    assert_eq!(
        app.tab_titles_for_test().len(),
        1,
        "the repository chosen after a cancel never opened"
    );
    assert!(app.tab_has_graph_for_test(0));
}

#[test]
fn an_open_that_fails_does_not_strand_the_tab_beside_it() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(3);
    let mut app = app(&ctx, Some(fixture.path().to_path_buf()));
    settle(&mut app, &ctx);
    assert!(app.tab_has_graph_for_test(0));

    // Opening parks the good tab and starts a new one. Switching back before
    // the answer arrives is the case that matters: the failure has to be
    // blamed on the tab that asked, not on whichever one is visible when it
    // lands.
    app.open_in_new_tab_for_test(fixture.path().join("not-a-repository"));
    app.activate_for_test(0);
    settle(&mut app, &ctx);

    assert_eq!(
        app.tab_titles_for_test().len(),
        1,
        "the tab that failed to open was left behind as an empty shell"
    );
    assert!(
        app.tab_has_graph_for_test(0),
        "the surviving tab lost the history it had already loaded"
    );
    assert!(!app.tab_is_busy_for_test(0));
}
