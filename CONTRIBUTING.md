# Contributing to Gitup

Thanks for wanting to help. This document is short because most of what matters
is in [docs/](docs/README.md); this is the part specific to sending a change.

## Getting set up

[docs/building.md](docs/building.md) covers the toolchain and the system
libraries for each platform. In short: Rust 1.95 or newer, the `git` binary, and
on Linux a handful of `-dev` packages.

```sh
git clone https://github.com/koneb71/gitup
cd gitup
cargo test
```

If `cargo test` passes, you are set up.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --all-targets --all-features
cargo test
```

CI runs all three on Linux, macOS, and Windows, and treats clippy warnings as
errors. Running them locally saves a round trip.

## What a good change looks like

**A fix comes with a test that fails without it.** Not for ceremony — this
codebase has a recurring class of bug that only appears with more than one
repository open, and the only reliable way to know a fix works is a test that
reproduced the problem first. [docs/testing.md](docs/testing.md) describes the
three shapes those tests usually take.

**Match the code around you.** Comment density, naming, and structure vary by
module because the modules do different things. `src/git/stage.rs` explains
every rule it applies because each one was got wrong at least once;
`src/ui/icons/` is generated data with no commentary at all. Follow whichever
you are standing in.

**Comments say why, not what.** The interesting comments in this codebase record
a decision or a defect: why two lanes may expect the same parent, why the
reverse patch anchors at the new line numbers, why item spacing has to be zero
in a virtualized list. A comment restating the line below it is noise.

**Keep the UI thread off libgit2.** Every Git operation goes through the job
system. If you find yourself wanting to call `git2` from a draw function, the
answer is a new `Job` variant. [docs/architecture.md](docs/architecture.md)
explains why, and the traps around job topics and supersession.

## Scope

Small, focused changes are easiest to review and easiest to revert. If a change
is large or reshapes something, please open an issue first so the approach can
be agreed before you write it — that is a courtesy to you as much as anyone.

Good first areas:

- **Distribution packaging.** Releases ship a Linux tarball. A `.deb`, `.rpm`,
  Flatpak, AUR, Homebrew, or winget package would each be genuinely useful and
  each is self-contained.
- **Platform bugs.** The maintainer develops on macOS. Windows and Linux get CI
  coverage and container testing, which is not the same as daily use.
- **Anything in the known limits** in
  [docs/platform-notes.md](docs/platform-notes.md).

## UI changes and snapshots

Snapshot tests render the real interface and compare pixels, so they only run on
macOS — text rasterization differs too much between platforms for committed
images to match. See [docs/testing.md](docs/testing.md).

If you are changing the UI from Linux or Windows, make the change and say so in
the pull request. CI renders the snapshots on its macOS leg and uploads any
diffs as an artifact, and a maintainer can regenerate them.

If you are on macOS:

```sh
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshots
```

Then open the images before committing. `git diff` on a PNG tells you nothing.

## Commit messages

A subject line in the imperative, under about seventy characters, and a body
explaining why if it is not obvious. No particular format is enforced.

## Reporting bugs

Use the issue templates. The details that help most are the platform, the Gitup
and git versions, and the shape of the repository — how many commits, whether a
merge or rebase was in progress, what the working tree looked like. You do not
need to share the repository itself.

For anything with security implications, please use
[private reporting](https://github.com/koneb71/gitup/security/advisories/new)
rather than a public issue. [SECURITY.md](SECURITY.md) has the details.

## Licence

Contributions are accepted under the MIT licence, the same as the rest of the
project. By opening a pull request you agree your work can be distributed under
it.
