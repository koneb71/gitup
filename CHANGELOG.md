# Changelog

Notable changes to Gitup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html) — with the usual
pre-1.0 caveat that the interface may still change between minor versions.

## [Unreleased]

### Added

- **Staging from the keyboard.** Arrow keys move through whichever list has
  focus, `→`/`←` step between history and the changed files, and `Space` stages
  the selected file and moves on to the next. The list holding the keys shows a
  bright marker while the other dims.
- **Drag a folder onto the window** to open it, with an overlay saying the drop
  will be caught. Several at once open in their own tabs.
- **Clone from the welcome screen.** It was previously reachable only through
  the command palette, which you had to already know about.
- **Settings → Identity**, for the `user.name` and `user.email` commits are
  authored under — globally, or overridden for one repository, as GitAhead had.
  The identity git will actually use is stated separately from the level being
  edited, and `GIT_CONFIG_GLOBAL` is honoured as git honours it. See
  [docs/identity.md](docs/identity.md).
- A **Draft** button on the commit box that writes a message from the staged
  changes. It runs offline with no configuration, matches whichever subject
  style the repository's history already uses, and refuses to replace anything
  you typed yourself. See [docs/commit-messages.md](docs/commit-messages.md).
- Windows and Linux support alongside macOS, with packaging for each: a
  portable zip, a tarball with a `.desktop` entry and hicolor icons, and the
  existing `.app` and disk image.
- Shortcut labels follow the platform. The same binding reads `⇧⌘P` on macOS
  and `Ctrl+Shift+P` on Windows and Linux, and settings files stay portable
  between them.
- The Windows executable carries its icon and version metadata, and neither the
  app nor its `git` subprocesses open a console window.
- Continuous integration across all three platforms, and a release workflow that
  drafts a release with checksummed artifacts.
- Documentation in [docs/](docs/README.md), with tests that check the reference
  tables still match the code.

### Fixed

- **The tab bar looked like a spreadsheet.** Every tab was given an equal share
  of the bar and separated by a full-height rule, so a repository called `cap3`
  took exactly as much room as `DevCapBackend`. Tabs are sized to their labels
  now, the rules are gone, and the active tab is rounded into the content it is
  showing.
- **The search box was two controls.** Choosing what to search and typing what
  to search for sat side by side as separate rounded pills; they are one field
  now, with the selector inside it.
- **The staging diff pane was cramped.** In the default 1280x820 window the
  diff body got 172px — ten lines — because the detail pane paid for two header
  bands and the commit box out of its own share rather than the window's. It
  now gets 294px, seventeen lines, and grows with the window instead of handing
  every extra pixel to the commit graph.
- Panel sizes were never written anywhere, and egui keeps them in memory only,
  so a resized split reverted on every launch — the sidebar width had the same
  problem, being written to settings on every frame but only flushed by
  unrelated saves. Dragged sizes are now stored when the drag ends.
- The arrow keys drove the commit graph whatever the user was doing, so
  pressing Down while picking through changed files threw them back into
  history and lost their place.
- Empty states asked for the impossible: a repository with no commits showed
  "Select a commit" in a pane with nothing to select, and the status bar
  reported "Idle" — a description of the application to itself.
- Shortcut labels came from hardcoded strings and had drifted: the theme button
  advertised Pull's chord. Every label now comes from the keymap, so it follows
  remapping too.
- Cancelling the folder picker left the app believing a dialog was still open,
  silently ignoring every later attempt to open a repository.
- A repository that failed to open cleared the loading state of whichever tab
  was visible, leaving the tab that actually failed skeletal and empty.
- The settings sheet had no scroll area, so it was clipped at the top and bottom
  once its contents outgrew the window — with no way to reach what was cut off.
- Counts spelled their own plurals at seven separate call sites, which is seven
  chances to render "1 files". They now go through one helper.
- Button heights had drifted to five values for three roles — a 20px Reset
  beside a 22px Stage all, a 26px Commit beside a 28px Done. The roles are named
  tokens now.
- Refreshing a repository kept the old staged and unstaged diffs unless the
  caller remembered to drop them, so a refresh could show a diff of a state the
  repository was no longer in. Dropping them is part of refreshing now.
- Graph growth could stop permanently: a superseded or failed walk never cleared
  the flag that guarded it, so scrolling past the end of history stopped loading
  more.

## [0.1.0]

First release. A complete Git client: commit graph with lane assignment, diff
viewer with syntax highlighting and word-level intra-line diff, blame, file
history and search, staging by file, hunk, or line, commit and amend, branches,
tags and stashes, fetch/pull/push/clone, merge, cherry-pick, revert and reset, a
three-way conflict editor, interactive rebase, submodules, Git LFS awareness,
multiple repositories in tabs, a command palette, and remappable key bindings.
