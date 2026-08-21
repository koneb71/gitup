//! Renders the real UI headlessly and compares against committed PNGs.
//!
//! These exist because a Git client is a visual tool: an assertion that a
//! status snapshot contains five entries says nothing about whether the list
//! is legible. Rendering the actual widgets catches layout and contrast
//! regressions that unit tests cannot.
//!
//! Regenerate with `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshots`.
//!
//! These run on macOS only. The comparison is pixel-by-pixel, and text
//! rasterization differs between platforms — the same font at the same size
//! lands on different subpixels under CoreText, FreeType and DirectWrite — so
//! images committed from one platform can never match another. macOS is the
//! reference simply because that is where the committed PNGs came from; CI
//! runs this suite on the macOS leg of the matrix and skips it elsewhere.
//! Every other test in the project runs everywhere.
#![cfg(target_os = "macos")]

mod common;

use common::Fixture;
use egui_kittest::Harness;
use gitup::app::GitupApp;
use gitup::settings::Settings;
use gitup::ui::ThemeMode;
use std::path::PathBuf;

/// Build a harness driving the real app against a repository.
///
/// `build_eframe` is used rather than `build_ui` because it constructs the app
/// with a `CreationContext` *before* the first frame — which is where fonts get
/// registered. Building lazily inside the first frame would ask for a font
/// family that egui has not bound yet, since `set_fonts` only takes effect on
/// the following pass.
fn harness(
    repo: Option<PathBuf>,
    theme: ThemeMode,
    size: (f32, f32),
) -> Harness<'static, GitupApp> {
    let settings = Settings {
        theme,
        // Throwaway repositories must not land in the real recent list.
        ..Settings::ephemeral()
    };

    // The harness has its own notion of the theme and applies it after the app
    // is built, so it has to be told as well. Without this the snapshot shows
    // the app's light palette for panels and egui's *dark* style for widgets —
    // which looks like a broken theme and, worse, means the light-mode
    // snapshots were never checking light mode at all.
    let egui_theme = match theme {
        ThemeMode::Light => egui::Theme::Light,
        ThemeMode::Dark => egui::Theme::Dark,
    };

    Harness::builder()
        .with_size(egui::vec2(size.0, size.1))
        .with_theme(egui_theme)
        .wgpu()
        .build_eframe(move |cc| GitupApp::new_with(cc, settings, repo))
}

/// Run frames until the app settles.
///
/// Opening a repository is asynchronous by design — the job system answers on
/// a later frame — so a single frame would snapshot a loading state. Stepping
/// with a short sleep between frames lets the workers land their results.
fn settle(harness: &mut Harness<'static, GitupApp>) {
    for _ in 0..40 {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    // `run` insists the UI reaches a resting state, which some views never do —
    // a visible toast keeps asking for repaints so it can fade. Stepping a
    // fixed number of times is enough for the data to have landed.
    harness.run_ok();
}

#[test]
fn welcome_screen_dark() {
    let mut h = harness(None, ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("welcome_dark");
}

#[test]
fn welcome_screen_light() {
    let mut h = harness(None, ThemeMode::Light, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("welcome_light");
}

#[test]
fn repository_with_changes_dark() {
    let fixture = Fixture::dirty();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("repo_dirty_dark");
}

#[test]
fn repository_with_changes_light() {
    let fixture = Fixture::dirty();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Light, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("repo_dirty_light");
}

#[test]
fn a_drafted_commit_message() {
    // The drafted state is worth a picture: it is the one place the app writes
    // prose on the user's behalf, and a subject that overflows its row or a
    // Commit button that stays grey would both be invisible to a unit test.
    let fixture = Fixture::dirty();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().draft_message_for_test();
    settle(&mut h);
    h.snapshot("commit_drafted_dark");
}

#[test]
fn several_repositories_in_tabs_light() {
    // The tab bar is the strip a user sees before anything else, and light
    // mode is where its fills are closest together — so it is the harder of
    // the two to get right and the one worth a picture.
    let a = Fixture::branchy();
    let b = Fixture::linear(3);
    let c = Fixture::dirty();
    let mut h = harness(Some(a.path_buf()), ThemeMode::Light, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().open_in_new_tab_for_test(b.path_buf());
    settle(&mut h);
    h.state_mut().open_in_new_tab_for_test(c.path_buf());
    settle(&mut h);
    h.snapshot("tabs_light");
}

#[test]
fn branching_history_dark() {
    let fixture = Fixture::branchy();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("graph_branchy_dark");
}

#[test]
fn branching_history_light() {
    let fixture = Fixture::branchy();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Light, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("graph_branchy_light");
}

#[test]
fn source_diff_is_syntax_highlighted() {
    let fixture = Fixture::source();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("diff_source_dark");
}

#[test]
fn source_diff_light() {
    let fixture = Fixture::source();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Light, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("diff_source_light");
}

#[test]
fn blame_view() {
    let fixture = Fixture::source();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().open_blame_for_test("src/parser.rs");
    settle(&mut h);
    h.snapshot("blame_dark");
}

#[test]
fn conflict_resolution_view() {
    let fixture = Fixture::conflicting();
    gitup::git::merge::merge(&fixture.repo, "theirs").expect("merge");

    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("conflicts_dark");
}

#[test]
fn command_palette() {
    let fixture = Fixture::branchy();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().open_palette_for_test("b");
    settle(&mut h);
    h.snapshot("palette_dark");
}

#[test]
fn side_by_side_diff() {
    let fixture = Fixture::source();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    h.state_mut()
        .set_diff_layout_for_test(gitup::ui::diff::DiffLayout::SideBySide);
    settle(&mut h);
    h.snapshot("diff_side_by_side_dark");
}

#[test]
fn settings_sheet() {
    let fixture = Fixture::simple();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 900.0));
    settle(&mut h);
    h.state_mut().open_settings_for_test();
    settle(&mut h);
    h.snapshot("settings_dark");
}

#[test]
fn image_diff_view() {
    let fixture = Fixture::image_change();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("diff_image_dark");
}

#[test]
fn lfs_change_view() {
    let fixture = Fixture::empty_named("lfs_view");
    fixture.write_lfs_pointer("assets/model.bin", &vec![3u8; 5_242_880], false);
    fixture.stage("assets/model.bin");
    fixture.commit("Track the model");
    fixture.write_lfs_pointer("assets/model.bin", &vec![4u8; 8_388_608], true);
    fixture.stage("assets/model.bin");
    fixture.commit("Retrain the model");

    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("diff_lfs_dark");
}

#[test]
fn submodule_sidebar() {
    let (parent, _child) = Fixture::with_submodule();
    // Leave one needing attention, so both states are visible at once.
    common::run_git(
        parent.path(),
        &["submodule", "deinit", "-f", "--", "vendor/lib"],
    );

    let mut h = harness(Some(parent.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("submodules_dark");
}

#[test]
fn several_repositories_in_tabs() {
    let first = Fixture::branchy();
    let second = Fixture::dirty();
    let (third, _child) = Fixture::with_submodule();

    let mut h = harness(Some(first.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().open_in_new_tab_for_test(second.path_buf());
    settle(&mut h);
    h.state_mut().open_in_new_tab_for_test(third.path_buf());
    settle(&mut h);
    // Go back to the first, so the active tab is not simply the last one.
    h.state_mut().activate_for_test(0);
    settle(&mut h);
    h.snapshot("tabs_dark");
}

#[test]
fn a_long_error_message_wraps_in_its_toast() {
    let fixture = Fixture::simple();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.state_mut().push_toast_for_test(
        "Authentication failed — no stored credentials for this remote.\n\
         Set up a credential helper or an SSH key, then try again.",
    );
    settle(&mut h);
    h.snapshot("toast_dark");
}

#[test]
fn clean_repository() {
    let fixture = Fixture::simple();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("repo_clean_dark");
}

#[test]
fn empty_repository_has_unborn_head() {
    let fixture = Fixture::empty();
    let mut h = harness(Some(fixture.path_buf()), ThemeMode::Dark, (1280.0, 820.0));
    settle(&mut h);
    h.snapshot("repo_empty_dark");
}
