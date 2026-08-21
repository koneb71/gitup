# Changelog

Notable changes to Gitup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html) — with the usual
pre-1.0 caveat that the interface may still change between minor versions.

## [0.1.1]

Installers, where 0.1.0 shipped only archives.

### Added

- **A Windows installer**, `gitup-<version>-windows-x86_64-setup.exe`. Installs
  per-user by default so no administrator prompt is needed, adds a Start Menu
  entry and an optional desktop shortcut, and registers an uninstaller in
  Add/Remove Programs. It notices when Git for Windows is absent and says so,
  rather than leaving you to discover it at the first pull. A portable `.zip`
  is still published alongside it.
- **A Debian package**, `gitup_<version>_<arch>.deb`, for Debian, Ubuntu and
  derivatives. Its dependencies come from `dpkg-shlibdeps` rather than being
  written by hand, so the version constraints say which releases the binary
  will actually run on. Carries a man page and a changelog, and is lintian
  clean.
- **An AppImage**, for the distributions that have no package: one executable
  file, no root, no package manager.

The macOS `.dmg` was already the platform's installer and is unchanged.

### Fixed

- **The documented Linux build dependencies were wrong.** Seven `-dev` packages
  were listed and installed — GTK, X11, Wayland, OpenSSL, Mesa — and none are
  needed. libgit2 is vendored with its own zlib and crypto, `rfd` talks to the
  XDG desktop portal over DBus rather than linking GTK, and winit and wgpu open
  X11, Wayland and the graphics drivers at runtime. `ldd` on the binary names
  only libc, libgcc, libm and libz, and the full suite passes in an image
  carrying just `build-essential`, `pkg-config`, `cmake` and `git`.

  The test that guarded this compared the documented list against the
  Dockerfile, so both being wrong together passed it. Build and run are
  different lists, and [docs/building.md](docs/building.md) now gives both —
  the runtime one being what a package has to declare.
- A release created by the workflow is now held as a draft. `draft: true` only
  applies when the action *creates* the release; an existing one is updated in
  place with its draft state untouched, which is how 0.1.0 went out published
  when it was meant to be reviewed first.
- Test fixtures pin `core.autocrlf`. Git for Windows defaults it to true, so a
  fixture file written as `"a\n"` came back from a checkout as `"a\r\n"` and
  any test comparing file contents byte for byte failed there and only there.

## [0.1.0] — first public release

A complete Git client: commit graph with lane assignment, diff viewer with
syntax highlighting and word-level intra-line diff, blame, file history and
search, staging by file, hunk, or line, commit and amend, branches, tags and
stashes, fetch/pull/push/clone, merge, cherry-pick, revert and reset, a
three-way conflict editor, interactive rebase, submodules, Git LFS awareness,
multiple repositories in tabs, a command palette, and remappable key bindings.

### Notable

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

### Decisions worth knowing

Arrived at by measurement, and recorded because the reasoning is the part that
gets lost.

- **The centre split is a share, not a height.** The detail pane pays for two
  header bands and the commit box out of its own allowance, so a plain 66% of
  the centre leaves the diff 301px at 1280x820 but only 129px at the 880x560
  minimum. The share is therefore taken of the *usable* centre, past that fixed
  furniture, and the diff grows with the window rather than handing every extra
  pixel to the commit graph. `src/ui/layout.rs` holds the arithmetic and its
  tests.
- **Dragged layout sizes are written to settings.** egui keeps panel sizes in
  memory only and eframe's persistence feature is off, so nothing survives a
  restart on its own. Sizes are flushed when the drag ends, not during it.
- **Tabs are sized to their labels.** An equal share of the bar is what a
  spreadsheet's column headers look like — `cap3` would take exactly as much
  room as `DevCapBackend`.
- **Arrow keys move whichever list has focus.** Driving the commit graph
  unconditionally means Down throws you out of the file list you are working
  in. The list holding the keys shows a bright marker while the other dims.
- **Every advertised shortcut comes from the keymap.** Written as literals they
  are both macOS-only and free to drift from the binding they name.
- **Counts and button heights go through single definitions.** Seven call sites
  spelling their own plurals is seven chances to render "1 files"; five button
  heights for three roles is drift nobody can see in any one file.
- **Disabled controls say why.** A greyed-out button with no explanation is
  indistinguishable from a broken one, which is how it gets reported.
