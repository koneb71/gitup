//! Drafting a commit message, through the real app.
//!
//! The rules themselves are unit-tested in `src/git/message.rs`. What these
//! check is the part that only exists once a repository is open: that the draft
//! describes what is *staged* rather than everything that changed, that it
//! matches the history's own style, and that it can never replace something the
//! user typed.

mod common;

use common::Fixture;
use gitup::app::GitupApp;
use gitup::settings::Settings;
use std::time::{Duration, Instant};

fn app(ctx: &egui::Context, fixture: &Fixture) -> GitupApp {
    GitupApp::new_in(ctx, Settings::ephemeral(), Some(fixture.path_buf()))
}

fn settle(app: &mut GitupApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.tick(ctx);
        if !app.is_busy_for_test() {
            app.tick(ctx);
            if !app.is_busy_for_test() {
                return;
            }
        }
        assert!(Instant::now() < deadline, "jobs never settled");
    }
}

#[test]
fn a_new_file_is_described_by_name() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    fixture.write("src/parser.rs", "pub fn parse() {}\n");
    fixture.stage("src/parser.rs");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);
    app.draft_message_for_test();

    assert_eq!(app.commit_message_for_test(), "Add parser.rs");
}

#[test]
fn only_what_is_staged_is_described() {
    // The whole promise of the button is that the message matches the commit
    // it is about to make. An unstaged file appearing in it would be a lie.
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    fixture.write("staged.txt", "in\n");
    fixture.write("unstaged.txt", "out\n");
    fixture.stage("staged.txt");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);
    app.draft_message_for_test();

    let message = app.commit_message_for_test();
    assert!(message.contains("staged.txt"), "drafted {message:?}");
    assert!(!message.contains("unstaged.txt"), "drafted {message:?}");
}

#[test]
fn the_draft_follows_the_style_the_history_already_uses() {
    let ctx = egui::Context::default();
    let fixture = Fixture::empty();
    fixture.commit_file("a.rs", "a\n", "feat: add a");
    fixture.commit_file("b.rs", "b\n", "fix(core): correct b");
    fixture.commit_file("c.rs", "c\n", "chore: tidy up");
    fixture.write("src/parser.rs", "pub fn parse() {}\n");
    fixture.stage("src/parser.rs");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);
    app.draft_message_for_test();

    let message = app.commit_message_for_test();
    // No scope: `src` is a container every project has, so naming it would
    // say nothing.
    assert_eq!(message, "feat: add parser.rs", "drafted {message:?}");
}

#[test]
fn a_plain_history_gets_a_plain_draft() {
    let ctx = egui::Context::default();
    let fixture = Fixture::empty();
    fixture.commit_file("a.rs", "a\n", "Add the first thing");
    fixture.commit_file("b.rs", "b\n", "Correct the second thing");
    fixture.write("src/parser.rs", "pub fn parse() {}\n");
    fixture.stage("src/parser.rs");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);
    app.draft_message_for_test();

    assert_eq!(app.commit_message_for_test(), "Add parser.rs");
}

#[test]
fn drafting_never_replaces_what_the_user_wrote() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    fixture.write("src/parser.rs", "pub fn parse() {}\n");
    fixture.stage("src/parser.rs");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    app.type_commit_message_for_test("Something I thought about carefully");
    assert!(
        !app.can_draft_message_for_test(),
        "the button was offered over text the user wrote"
    );

    app.draft_message_for_test();
    assert_eq!(
        app.commit_message_for_test(),
        "Something I thought about carefully",
        "drafting destroyed a hand-written message"
    );
}

#[test]
fn a_previous_draft_can_be_replaced_by_a_newer_one() {
    // The other half of the rule: after staging more, drafting again has to
    // work, or the button is single-use per commit.
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    fixture.write("src/parser.rs", "pub fn parse() {}\n");
    fixture.stage("src/parser.rs");

    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);
    app.draft_message_for_test();
    assert_eq!(app.commit_message_for_test(), "Add parser.rs");

    fixture.write("src/lexer.rs", "pub fn lex() {}\n");
    fixture.stage("src/lexer.rs");
    app.refresh_tab_for_test(0);
    settle(&mut app, &ctx);

    assert!(
        app.can_draft_message_for_test(),
        "a draft could not be redrafted after staging more"
    );
    app.draft_message_for_test();
    let message = app.commit_message_for_test();
    assert!(
        message.contains("parser.rs") && message.contains("lexer.rs"),
        "the second draft did not describe both files: {message:?}"
    );
}

#[test]
fn nothing_staged_means_nothing_to_draft() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    assert!(!app.can_draft_message_for_test());
    app.draft_message_for_test();
    assert_eq!(app.commit_message_for_test(), "");
}
