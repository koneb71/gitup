# Testing

```sh
cargo test
```

That runs everything: unit tests inside `src/`, integration tests in `tests/`,
and — on macOS — the rendered UI snapshots.

## What the suite is made of

**Unit tests** live beside the code they cover, and concentrate on the logic
that is easy to get subtly wrong: patch synthesis for line-level staging, commit
graph lane assignment, conflict stage parsing, error message summarizing, and
the tab arithmetic in `state::tab_index`.

**Integration tests** in `tests/` drive real repositories. Fixtures are built
programmatically in temporary directories by `tests/common/mod.rs`, with pinned
commit timestamps and stable names so results are reproducible. There are
fixtures for merges, conflicts, renames, binary files, submodules, detached
HEAD, and a synthetic history large enough to make performance assertions
meaningful.

Network operations are tested against bare repositories on disk — a real remote,
reached through the real `git` binary, with no network access and no
credentials involved.

**Snapshot tests** in `tests/ui_snapshots.rs` render the actual UI headlessly
through `egui_kittest` and compare the result against committed PNGs in
`tests/snapshots/`. A Git client is a visual tool, and an assertion that a
status list contains five entries says nothing about whether it is legible.

## Snapshot tests run on macOS only

The comparison is pixel-by-pixel, and text rasterization differs between
platforms: the same font at the same size lands on different subpixels under
CoreText, FreeType, and DirectWrite. Images committed from one platform can
never match another.

macOS is the reference simply because that is where the committed PNGs came
from. `tests/ui_snapshots.rs` is gated with `#![cfg(target_os = "macos")]`, so
on Linux and Windows the suite compiles and reports zero tests rather than
failing. CI runs it on the macOS leg of the matrix. Every other test runs
everywhere.

If you are contributing a UI change from Linux or Windows, make the change and
say so in the pull request — CI will render the snapshots and upload any diffs
as an artifact, and a maintainer on a Mac can regenerate them.

### Regenerating

```sh
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshots
```

Then look at the images before committing them. `git diff` on a PNG tells you
nothing; opening the file tells you whether the change was the one you meant.

A failing run writes `<name>.new.png` and `<name>.diff.png` beside the expected
image. Both are gitignored.

## Documentation tests

`tests/docs.rs` checks that the reference tables in `docs/` still describe the
code: the shortcut table against the default keymap, and the Debian package list
in [building.md](building.md) against the Dockerfile that CI uses. It also
verifies that relative links between documents resolve.

These exist because reference documentation rots silently. A default binding
changes, nobody greps the docs, and the next reader learns the wrong key.

## Writing a test for a bug

The house rule is that a fix comes with a test that fails without it. For this
codebase that usually means one of three shapes:

- **A repository fixture**, when the bug is about what Git did. Build the
  smallest history that reproduces it in `tests/common/mod.rs`.
- **Driving the app**, when the bug is about state. `tests/tabs.rs` shows the
  pattern: build a `GitupApp` with ephemeral settings, `tick` it until the job
  queue drains, then assert on what the session holds.
- **A pure function**, when the bug is arithmetic. Several modules exist in the
  shape they do — `state::tab_index` especially — because pulling the arithmetic
  out of the app made it testable.

Bugs that only appear with more than one repository open deserve particular
suspicion, and `tests/tabs.rs` is where they go. The recurring defect in this
codebase has been state that should be per-repository but is not: a shared job
topic, an outcome routed to whichever tab is visible, a flag that a superseded
job never clears. All of them look fine with a single tab open.

## Continuous integration

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs formatting and
clippy once on Linux, then the full suite on Linux, macOS, and Windows, then
builds the release packages on all three. The matrix does not fail fast: knowing
whether a break is platform-specific is worth the extra minutes.

Before opening a pull request:

```sh
cargo fmt --all
cargo clippy --all-targets --all-features
cargo test
```
