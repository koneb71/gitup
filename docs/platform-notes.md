# Platform notes

Gitup is the same application on all three platforms and the same code path
almost everywhere. This page collects the places where it is not, and the
platform-specific things worth knowing.

## All platforms

Gitup needs the **`git` binary on `PATH`** for anything that touches the
network. Local work — history, diffs, staging, committing, branching, merging —
goes through libgit2 and works without it. Fetch, pull, push, and clone report
that git is unavailable rather than failing obscurely.

This is deliberate: it means your existing credential helpers, SSH agent, and
keychain entries work in Gitup exactly as they do in your terminal, with nothing
to configure. See [architecture.md](architecture.md).

## Key bindings

The modifier is **Cmd on macOS and Ctrl on Windows and Linux** — one binding,
spelled according to the platform. `⇧⌘P` and `Ctrl+Shift+P` are the same
shortcut, and the app only ever shows you yours.

Settings files are portable: bindings are stored under the name `cmd` on every
platform, so a config copied from a Mac to a PC keeps working.
[shortcuts.md](shortcuts.md) has the full table and the file locations.

## Windows

- **Git for Windows is required** for network operations. Install it from
  [git-scm.com](https://git-scm.com/download/win) and make sure it is on `PATH`.
- **No console window appears.** The binary is built with the `windows`
  subsystem in release mode, and every `git` subprocess is spawned with
  `CREATE_NO_WINDOW` — otherwise each fetch would flash a black box over the
  interface. Debug builds keep the console, so `GITUP_LOG` output is visible
  while developing.
- **SmartScreen warns on first run**, because the executable is not
  code-signed. "More info" then "Run anyway" gets past it. Signing needs a
  certificate, which is a decision for whoever ships releases.
- **Long paths.** Repositories with deeply nested paths need long path support
  enabled in both Windows and Git (`git config --global core.longpaths true`).
  Gitup inherits whatever git is configured to do.
- **Line endings** are git's business, not Gitup's. Diffs show what git reports,
  so `core.autocrlf` behaves as it does on the command line.

## macOS

- **Gatekeeper.** Release builds are ad-hoc signed but not notarized, so the
  first launch of a downloaded copy needs right-click → Open. A copy you built
  yourself opens normally.
- **Folder permissions.** The `.app` bundle declares usage descriptions for
  Documents, Desktop, and Downloads. Without them, opening a repository in one
  of those folders fails with a permission error instead of prompting. Running
  the bare binary from a terminal inherits the terminal's permissions instead.
- **Snapshot tests only run here.** See [testing.md](testing.md).

## Linux

- **System libraries** are the only real setup step; [building.md](building.md)
  lists them per distribution.
- **Wayland and X11** both work. eframe picks Wayland when it is available and
  falls back to X11.
- **The folder picker uses GTK 3** through `rfd`, which is why `libgtk-3-dev` is
  a build dependency and a GTK 3 runtime is needed. On a desktop that is not
  GTK-based the dialog still works; it just looks like a GTK dialog.
- **Graphics.** Rendering goes through `wgpu`, which prefers Vulkan. If the
  window is black or the app exits at startup, there is no usable driver;
  `WGPU_BACKEND=gl` forces the GL path, and software rendering (`llvmpipe`,
  `lavapipe`) works but is slower.
- **The desktop entry** sets `StartupWMClass=dev.gitup.Gitup` to match the app
  id the window reports, so the running window associates with its launcher
  instead of appearing as a second, unnamed dock item.
- **Distribution packages.** Releases ship a tarball with an `install.sh` rather
  than a `.deb` or `.rpm`, because those bind the result to one distribution's
  policy. Native packages are welcome as contributions.

## Known limits, on every platform

- **Interactive rebase refuses a range containing a merge commit.** Replaying
  merges through a todo list is a different problem, and guessing would produce
  history nobody asked for.
- **Escape and the arrow keys are not remappable.** What they do depends on what
  is open, which a binding table cannot express.
- **LFS objects are not fetched.** Gitup reports whether one has been
  downloaded; `git lfs pull` gets it.
- **Pulling needs a branch that tracks a remote.** When it does not, the Pull
  button says why rather than going grey without explanation — but setting the
  upstream is still a push away, not something Pull does for you.
