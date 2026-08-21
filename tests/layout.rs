//! How much of the window the diff actually gets.
//!
//! The arithmetic is unit-tested in `src/ui/layout.rs`. What these check is the
//! number that comes out of the real widget tree, because the failure being
//! guarded against is invisible to every other kind of test: all the widgets
//! are present and correct, there is simply nowhere to read the diff.
//!
//! The layout this replaces gave the diff body 172px — ten lines — in the
//! default 1280x820 window, out of 722px of centre.

mod common;

use common::Fixture;
use egui_kittest::Harness;
use gitup::app::GitupApp;
use gitup::settings::Settings;

/// A diff line, from `src/ui/diff.rs`.
const LINE: f32 = 17.0;

/// Render a repository with changes and report the diff body's height.
fn diff_body(size: (f32, f32), settings: Settings) -> f32 {
    let fixture = Fixture::linear(3);
    for name in ["a.txt", "b.txt", "c.txt"] {
        fixture.write(name, "one\ntwo\nthree\n");
    }
    let repo = fixture.path_buf();

    let mut harness = Harness::builder()
        .with_size(egui::vec2(size.0, size.1))
        .wgpu()
        .build_eframe(move |cc| GitupApp::new_with(cc, settings, Some(repo)));

    for _ in 0..40 {
        harness.step();
        if harness.state().diff_body_height_for_test() > 0.0 && !harness.state().is_busy_for_test()
        {
            harness.step();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    harness.state().diff_body_height_for_test()
}

#[test]
fn the_default_window_leaves_room_to_read_a_diff() {
    let px = diff_body((1280.0, 820.0), Settings::ephemeral());
    let lines = px / LINE;
    assert!(
        lines > 16.0,
        "the diff body got {px:.0}px — {lines:.1} lines; it was ten before this \
         was fixed and should not be creeping back"
    );
}

#[test]
fn the_smallest_window_still_shows_a_diff() {
    // 880x560 is the minimum the window can be dragged to. Both halves compete
    // for the same 462px here, so the bar is lower — but a diff you cannot
    // read at all would make the minimum size pointless.
    let px = diff_body((880.0, 560.0), Settings::ephemeral());
    let lines = px / LINE;
    assert!(
        lines > 8.0,
        "the diff body got {px:.0}px — {lines:.1} lines"
    );
}

#[test]
fn a_bigger_window_gives_the_extra_space_to_the_diff() {
    // The reason the split is stored as a share and not a height. A fixed
    // height would hand every extra pixel of a large screen to the commit
    // graph, which is navigation rather than the thing being read.
    let small = diff_body((1280.0, 820.0), Settings::ephemeral());
    let large = diff_body((2560.0, 1440.0), Settings::ephemeral());
    assert!(
        large > small * 1.8,
        "{small:.0}px at 820 tall but only {large:.0}px at 1440"
    );
}

#[test]
fn a_remembered_split_is_honoured() {
    // The half that makes the split worth having: a resize is written to the
    // settings file, and a later launch has to actually use it. egui keeps
    // panel sizes in memory only, so without this the window reopened cramped
    // however carefully it had been adjusted.
    let generous = Settings {
        detail_share: 0.8,
        ..Settings::ephemeral()
    };
    let mean = Settings {
        detail_share: 0.3,
        ..Settings::ephemeral()
    };

    let tall = diff_body((1280.0, 820.0), generous);
    let short = diff_body((1280.0, 820.0), mean);
    assert!(
        tall > short + 100.0,
        "the stored share was ignored: {tall:.0}px vs {short:.0}px"
    );
}
