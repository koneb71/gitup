# Gitup

[![CI](https://github.com/koneb71/gitup/actions/workflows/ci.yml/badge.svg)](https://github.com/koneb71/gitup/actions/workflows/ci.yml)
[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](docs/platform-notes.md)

A graphical Git client for macOS, Linux, and Windows. Written in Rust, built on
libgit2 and egui — one native binary, no webview, no Electron.

It began as a modernized answer to [GitAhead][gitahead] — the same double-pane
diff and real commit graph, without the parts that made GitAhead frustrating:
credentials that never worked, a search index that went stale, and a UI that
showed you the repository as it was several operations ago.

[gitahead]: https://github.com/gitahead/gitahead

## Installing

Gitup needs the **`git` binary on `PATH`** for anything that touches the
network. That is deliberate: it means your existing credential helpers, SSH
agent, and keychain work in Gitup exactly as they do in your terminal, with
nothing to set up. Everything local — history, diffs, staging, committing,
merging — works without it.

Builds are not code-signed, so each platform asks for confirmation the first
time. [docs/platform-notes.md](docs/platform-notes.md) has the details.

**macOS** — download the `.dmg` from [Releases][releases], drag Gitup to
Applications, then right-click → Open the first time.

**Linux** — download the tarball from [Releases][releases]:

```sh
tar xzf gitup-*-linux-x86_64.tar.gz
./gitup-*-linux-x86_64/install.sh      # installs into ~/.local, no root needed
```

**Windows** — download the zip from [Releases][releases] and unzip it anywhere.
It is portable; run `gitup.exe`. You will also need
[Git for Windows](https://git-scm.com/download/win).

**From source** — a Rust toolchain, then `cargo run --release`. Linux needs a
few `-dev` packages first; [docs/building.md](docs/building.md) lists them per
distribution.

[releases]: https://github.com/koneb71/gitup/releases

## What it does

**Reading history**
- Commit graph with real lane assignment — branches, merges, and where they
  converge, virtualized so a hundred-thousand-commit repository scrolls at full
  frame rate
- Diffs in unified or side-by-side layout, with syntax highlighting and
  word-level intra-line diff so a one-token change doesn't look like a rewrite
- Changed images shown before-and-after at a common scale, rather than
  "binary file, no diff"
- Git LFS files described by the object they point at — size, digest, and
  whether it has been downloaded — instead of the pointer's three lines of text
- Blame with per-line attribution, shaded by age and syntax highlighted, and
  "blame before this commit" to walk a line backwards through history
- File history that follows renames, and search across messages, content,
  authors, and paths

**Changing things**
- Stage and unstage by file, by hunk, or by individual line; discard the same way
- Work through changes without the mouse: arrows move the list that has focus,
  `→`/`←` step between history and the changed files, `Space` stages and moves
  on to the next
- Commit and amend, with subject-length hints and no modal in the way
- Draft a commit message from what is staged — offline, in whatever style your
  history already uses, and never over something you typed
  ([how it works](docs/commit-messages.md))
- Branches, tags, and stashes: create, check out, rename, delete, apply, drop
- Fetch, pull, push, and clone with live progress
- Merge, cherry-pick, revert, and reset, with a three-way conflict editor.
  Merge commits ask which parent to measure against rather than refusing
- Interactive rebase — pick, reword, squash, fixup, drop, and reorder by
  dragging or with the keyboard
- Submodules: initialize, update, add, remove, and open one as its own
  repository

**Getting around**
- Several repositories open at once, in tabs. Each keeps its own selection,
  search, and half-written commit message, and each tab shows a badge for
  uncommitted work, conflicts, or an operation in progress — so a repository
  that went into conflict while you were elsewhere says so
- A command palette (`⌘K`, or `Ctrl+K`) that matches commands, branch names,
  commit hashes, and open tabs with the same keystrokes
- Everything refreshes on its own: a filesystem watcher notices commits you made
  in a terminal — in every tab, not just the visible one
- Drop a folder onto the window to open it
- Tabs are restored on the next launch
- Remappable key bindings with conflict warnings, in Settings
- Your commit identity, set globally or overridden per repository, with the
  address git will actually use stated plainly
  ([details](docs/identity.md))

Shortcuts are shown the way your platform writes them: `⇧⌘P` on macOS,
`Ctrl+Shift+P` on Windows and Linux. Full table in
[docs/shortcuts.md](docs/shortcuts.md).

## How it is built

Three decisions account for most of the architecture.

**The UI thread never calls libgit2.** egui is immediate-mode and redraws every
frame, so a revwalk or a status scan on the UI thread would freeze the window.
All Git work goes through a job system that owns a pool of worker threads, each
holding its own `git2::Repository`. Jobs carry a *topic*, and dispatching on a
topic supersedes anything still in flight for it — which is what stops the diff
pane flickering between commits when you scroll history quickly.

**Network operations shell out to `git`.** libgit2 does not implement
`credential.helper`, does not read `~/.ssh/config`, and knows nothing about the
macOS keychain, Git Credential Manager, or `libsecret`. Running the real binary
inherits every one of those, already configured and already working. This is the
single biggest functional difference from GitAhead.

**Partial staging synthesizes patches.** Git has no "stage these lines"
primitive, so staging a selection means generating a patch containing only the
selected changes and applying it to the index. The rules are not symmetric —
unselected deletions become context, unselected additions vanish, and the whole
thing mirrors when unstaging.

There is no search index. `git log` already searches messages, content, authors,
and paths using data git maintains anyway, so a search is a process spawn rather
than a second copy of the repository that can disagree with the first.

[docs/architecture.md](docs/architecture.md) goes into the rest.

## Documentation

- [Building and packaging](docs/building.md) — toolchains, system libraries,
  troubleshooting
- [Identity](docs/identity.md) — the name and email commits are authored under
- [Commit messages](docs/commit-messages.md) — what the Draft button writes
- [Keyboard shortcuts](docs/shortcuts.md) — both spellings, and where settings
  live
- [Platform notes](docs/platform-notes.md) — differences and known limits
- [Architecture](docs/architecture.md) — how the pieces fit
- [Testing](docs/testing.md) — what the suite covers

## Contributing

Bug reports, fixes, and platform packaging are all welcome.
[CONTRIBUTING.md](CONTRIBUTING.md) covers setup and what a good change looks
like. The short version:

```sh
cargo fmt --all && cargo clippy --all-targets && cargo test
```

Development happens on macOS, so Windows and Linux bug reports are especially
useful — CI coverage is not the same as daily use.

## Licence

MIT — see [LICENSE](LICENSE). Bundled fonts keep their own: Inter and JetBrains
Mono are OFL, Phosphor Icons are MIT. See `assets/fonts/`.
