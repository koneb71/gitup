//! Working through changes without the mouse.
//!
//! These drive real key events through the real widgets, because the thing
//! being tested is which list a keypress reaches — and that is decided during
//! layout, not by any state the app holds between frames.
//!
//! Unlike `ui_snapshots.rs` these compare no pixels, so they run on every
//! platform.

mod common;

use common::Fixture;
use egui_kittest::Harness;
use gitup::app::GitupApp;
use gitup::settings::Settings;
use std::path::PathBuf;

fn harness(repo: PathBuf) -> Harness<'static, GitupApp> {
    let settings = Settings::ephemeral();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 820.0))
        .wgpu()
        .build_eframe(move |cc| GitupApp::new_with(cc, settings, Some(repo)))
}

fn settle(harness: &mut Harness<'static, GitupApp>) {
    for _ in 0..40 {
        harness.step();
        if !harness.state().is_busy_for_test() {
            harness.step();
            if !harness.state().is_busy_for_test() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A repository with several unstaged files, showing the working tree.
fn staging_harness() -> (Fixture, Harness<'static, GitupApp>) {
    let fixture = Fixture::linear(2);
    for name in ["a.txt", "b.txt", "c.txt"] {
        fixture.write(name, "content\n");
    }

    let mut harness = harness(fixture.path_buf());
    settle(&mut harness);
    // Enter the file list the way the keyboard user does.
    harness.key_press(egui::Key::ArrowRight);
    harness.step();
    (fixture, harness)
}

#[test]
fn the_arrow_keys_move_through_the_changed_files() {
    let (_fixture, mut harness) = staging_harness();

    let first = harness.state().active_file_for_test().map(str::to_owned);
    assert!(first.is_some(), "no file was active to start from");

    harness.key_press(egui::Key::ArrowDown);
    harness.step();
    let second = harness.state().active_file_for_test().map(str::to_owned);

    assert_ne!(first, second, "Down did not move to another file");

    harness.key_press(egui::Key::ArrowUp);
    harness.step();
    assert_eq!(
        harness.state().active_file_for_test().map(str::to_owned),
        first,
        "Up did not come back"
    );
}

#[test]
fn moving_through_files_does_not_disturb_the_commit_selection() {
    // The bug this replaces: the arrow keys drove the commit graph whatever
    // the user was doing, so pressing Down while picking through changed files
    // threw them back into history.
    let (_fixture, mut harness) = staging_harness();
    let before = harness.state().selection_for_test();

    for _ in 0..3 {
        harness.key_press(egui::Key::ArrowDown);
        harness.step();
    }

    assert_eq!(
        harness.state().selection_for_test(),
        before,
        "moving through files changed what was selected in history"
    );
}

#[test]
fn space_stages_the_active_file() {
    let (_fixture, mut harness) = staging_harness();
    let staged_before = harness.state().staged_count_for_test();

    harness.key_press(egui::Key::Space);
    harness.step();
    settle(&mut harness);

    assert_eq!(
        harness.state().staged_count_for_test(),
        staged_before + 1,
        "Space did not stage anything"
    );
}

#[test]
fn staging_moves_on_to_the_next_file() {
    // Landing back at the top after every keystroke would make working down a
    // list of changes impossible, which is the whole point of staging by key.
    let (_fixture, mut harness) = staging_harness();

    let first = harness
        .state()
        .active_file_for_test()
        .map(str::to_owned)
        .expect("a file to stage");

    harness.key_press(egui::Key::Space);
    harness.step();
    settle(&mut harness);

    let now = harness.state().active_file_for_test().map(str::to_owned);
    assert!(now.is_some(), "nothing was active after staging");
    assert_ne!(
        now,
        Some(first),
        "the staged file was still selected after it left the list"
    );
}

#[test]
fn left_and_right_move_between_history_and_the_files() {
    let fixture = Fixture::linear(3);
    fixture.write("a.txt", "content\n");
    let mut harness = harness(fixture.path_buf());
    settle(&mut harness);

    assert!(
        harness.state().history_has_focus_for_test(),
        "history should hold the keys before anything else is touched"
    );

    harness.key_press(egui::Key::ArrowRight);
    harness.step();
    assert!(
        !harness.state().history_has_focus_for_test(),
        "Right did not step into the file list"
    );

    harness.key_press(egui::Key::ArrowLeft);
    harness.step();
    assert!(
        harness.state().history_has_focus_for_test(),
        "Left did not come back to history"
    );
}

#[test]
fn the_arrow_keys_still_move_through_history() {
    // The other half: focus following the file list must not have taken the
    // keys away from the commit graph.
    let fixture = Fixture::linear(4);
    let mut harness = harness(fixture.path_buf());
    settle(&mut harness);

    let before = harness.state().selection_for_test();
    harness.key_press(egui::Key::ArrowDown);
    harness.step();

    assert_ne!(
        harness.state().selection_for_test(),
        before,
        "Down did not move through history"
    );
}
