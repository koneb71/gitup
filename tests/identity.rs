//! Setting `user.name` and `user.email`, through the real app.
//!
//! # A warning about the global config
//!
//! These tests write to a *global* config, and a careless version of them would
//! rewrite the identity of whoever ran `cargo test`. They are safe because
//! `GIT_CONFIG_GLOBAL` is pointed at a temporary file first, which is what
//! `src/git/identity.rs` resolves the global level through.
//!
//! Anything added here must do the same. Never call the global scope without
//! redirecting it.

mod common;

use common::Fixture;
use gitup::app::GitupApp;
use gitup::git::identity::Scope;
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

/// The identity recorded in a repository's own config, read independently of
/// the app so the assertion is about the file rather than about what the app
/// believes.
fn on_disk(fixture: &Fixture, key: &str) -> Option<String> {
    let repo = git2::Repository::open(fixture.path()).expect("open");
    let mut config = repo
        .config()
        .expect("config")
        .open_level(git2::ConfigLevel::Local)
        .expect("local level");
    config.snapshot().expect("snapshot").get_string(key).ok()
}

#[test]
fn a_repository_identity_is_written_to_its_own_config() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    app.load_identity_for_test();
    settle(&mut app, &ctx);

    app.set_identity_for_test(Scope::Repository, "Ada Lovelace", "ada@example.com");
    settle(&mut app, &ctx);

    assert_eq!(
        on_disk(&fixture, "user.name").as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        on_disk(&fixture, "user.email").as_deref(),
        Some("ada@example.com")
    );

    let identities = app.identity_for_test().expect("read back");
    assert_eq!(identities.repository.name, "Ada Lovelace");
    assert!(identities.is_overridden());
    // The repository level wins over whatever the machine's global config says.
    assert_eq!(identities.effective.email, "ada@example.com");
    assert!(identities.can_commit());
}

#[test]
fn clearing_a_repository_identity_removes_the_keys() {
    // Not "sets them to empty": an empty `user.email` in a repository shadows
    // the global address with nothing, and commits then fail there and only
    // there. Clearing has to mean inheriting again.
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    app.load_identity_for_test();
    settle(&mut app, &ctx);
    app.set_identity_for_test(Scope::Repository, "Ada Lovelace", "ada@example.com");
    settle(&mut app, &ctx);
    assert!(on_disk(&fixture, "user.name").is_some());

    app.set_identity_for_test(Scope::Repository, "", "");
    settle(&mut app, &ctx);

    assert_eq!(on_disk(&fixture, "user.name"), None);
    assert_eq!(on_disk(&fixture, "user.email"), None);
    assert!(!app.identity_for_test().expect("read back").is_overridden());
}

#[test]
fn the_fields_are_only_dirty_when_they_differ() {
    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    app.load_identity_for_test();
    settle(&mut app, &ctx);
    app.set_identity_for_test(Scope::Repository, "Ada Lovelace", "ada@example.com");
    settle(&mut app, &ctx);

    // Saved, and the fields still hold what was saved: nothing to save again.
    assert!(!app.identity_is_dirty_for_test());
}

#[test]
fn a_global_identity_is_written_to_the_global_config() {
    // `GIT_CONFIG_GLOBAL` is what keeps this off the real ~/.gitconfig. It is
    // process-wide, so this is the only test that sets it, and it holds a
    // repository open at the same time to prove the two levels stay apart.
    let home = tempfile::tempdir().expect("temp dir");
    let global = home.path().join("gitconfig");
    // SAFETY: single-threaded setup, before any repository work in this test.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };

    let ctx = egui::Context::default();
    let fixture = Fixture::linear(2);
    let mut app = app(&ctx, &fixture);
    settle(&mut app, &ctx);

    app.load_identity_for_test();
    settle(&mut app, &ctx);
    app.set_identity_for_test(Scope::Global, "Ada Lovelace", "ada@example.com");
    settle(&mut app, &ctx);

    let mut written = git2::Config::open(&global).expect("open the redirected global");
    let snapshot = written.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.get_string("user.name").ok().as_deref(),
        Some("Ada Lovelace")
    );

    let identities = app.identity_for_test().expect("read back");
    assert_eq!(identities.global.name, "Ada Lovelace");

    // The levels have to stay apart. Fixtures set their own identity so they
    // can commit, so this repository overrides the global one — and a global
    // write must leave that override alone rather than flattening it.
    assert_eq!(identities.repository.name, "Fixture");
    assert_eq!(identities.effective.name, "Fixture");
    assert!(identities.is_overridden());

    unsafe { std::env::remove_var("GIT_CONFIG_GLOBAL") };
}
