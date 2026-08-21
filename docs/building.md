# Building Gitup

Gitup is a single Rust binary. There is no bundler, no Node, and no code
generation step — `cargo build` is the whole story once the system libraries
are in place.

## What every platform needs

- **A Rust toolchain**, 1.95 or newer. Install from [rustup.rs](https://rustup.rs).
  The floor comes from egui and eframe 0.36, not from anything in this codebase.
- **The `git` binary**, on `PATH`. Gitup uses libgit2 for local work but runs
  real `git` for anything touching the network, so that it inherits your
  credential helpers, SSH agent, and keychain. Without it the app still opens
  repositories; fetch, pull, push, and clone report that git is unavailable.
- **A GPU that can run Vulkan, Metal, or DX12.** Rendering goes through `wgpu`.
  Software rendering works (Mesa's `llvmpipe`, or `lavapipe` for Vulkan) but is
  noticeably slower.

Then:

```sh
cargo run --release
```

`cargo run --release -- /path/to/repo` opens a specific repository. With no
argument, launching from inside a repository opens that one. `--version` and
`--help` answer and exit without opening a window — except on Windows, where a
GUI build has no console to print to.

## Linux

Build dependencies are short, because the binary links almost nothing:

```sh
sudo apt-get install build-essential pkg-config cmake
```

On Fedora: `sudo dnf install gcc gcc-c++ pkgconf-pkg-config cmake`.
On Arch: `sudo pacman -S base-devel pkgconf cmake`.

That is genuinely the whole list. `ldd` on the result names only libc, libgcc,
libm and libz. libgit2 is vendored with its own zlib and crypto, so there is no
OpenSSL dependency; `rfd` talks to the XDG desktop portal over DBus rather than
linking GTK; and winit and wgpu open X11, Wayland and the graphics drivers at
runtime instead of linking them.

The authoritative list is [`scripts/docker/Dockerfile.linux`](../scripts/docker/Dockerfile.linux),
which CI and the container build both use.

### What it needs to *run*

Different list, and the one that matters to someone installing a package:

| Needed for | Provided by |
|---|---|
| the window | X11 or Wayland — any desktop session has one |
| rendering | a Vulkan or GL driver, e.g. Mesa |
| the folder picker | `xdg-desktop-portal` and a backend for your desktop |
| fetch, pull, push, clone | `git` |

The `.deb` declares the first two through the libraries the binary links, and
recommends `git` and `xdg-desktop-portal`. Without a portal backend installed
the file picker cannot open, though everything else works.

### Packaging

```sh
scripts/package-linux.sh
```

Produces three things in `target/`:

- **`gitup_<version>_<arch>.deb`** — for Debian, Ubuntu and derivatives.
  `sudo apt install ./gitup_0.1.0_amd64.deb`.
- **`Gitup-<version>-<arch>.AppImage`** — for everything else. `chmod +x` and
  run it; no installation, no root. Needs `appimagetool` at build time, and is
  skipped with a message when that is absent.
- **`gitup-<version>-linux-<arch>.tar.gz`** — the binary, a `.desktop` entry,
  hicolor icons, and an `install.sh` that copies them under a prefix
  (`~/.local` by default, which needs no root).

The `.deb` is built with plain `dpkg-deb`; its dependencies come from
`dpkg-shlibdeps` rather than being written by hand, so the version constraints
say which releases the binary will actually run on.

### Building on a machine that is not Linux

There is a container image for exactly this:

```sh
docker build -f scripts/docker/Dockerfile.linux -t gitup-linux .
docker run --rm -v "$PWD":/src -e CARGO_TARGET_DIR=/src/target-linux gitup-linux \
  cargo test
```

Tests run headlessly, so no display is needed. Snapshot tests do not run outside
macOS (see [testing.md](testing.md)).

## macOS

Xcode command line tools and nothing else:

```sh
xcode-select --install
```

### Packaging

```sh
scripts/package-macos.sh --dmg
```

Produces `target/Gitup.app` and a disk image beside it. The bundle is ad-hoc
signed, not notarized, so macOS asks for confirmation the first time it opens —
right-click → Open gets past it. Proper signing needs a Developer ID
certificate, which is a decision for whoever ships releases rather than
something a build script should assume.

## Windows

Install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)
with the "Desktop development with C++" workload — libgit2 needs a C compiler —
and [Git for Windows](https://git-scm.com/download/win). Rustup will pick up the
MSVC toolchain automatically.

```powershell
cargo run --release
```

### Packaging

```powershell
.\scripts\package-windows.ps1
```

Produces two things in `target\`:

- **`gitup-<version>-windows-<arch>-setup.exe`** — an Inno Setup installer. It
  installs per-user by default so no administrator prompt is needed, adds a
  Start Menu entry and an optional desktop shortcut, and registers an
  uninstaller in Add/Remove Programs. It also notices when Git for Windows is
  absent and says so, rather than leaving you to find out at the first pull.
- **`gitup-<version>-windows-<arch>.zip`** — the portable alternative. Unzip
  anywhere and run it.

Inno Setup rather than WiX: both are preinstalled on the GitHub Actions Windows
runners, and Inno produces a friendlier wizard for what is a single
self-contained binary. Locally, the installer step is skipped with a message
when `ISCC.exe` is not found, so the zip is still produced.

Neither is code-signed, so SmartScreen warns the first time either is run.

### Cross-compiling to Windows

From macOS or Linux with `mingw-w64` installed:

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu
```

This compiles, including the icon resource that `build.rs` embeds — `windres`
comes with mingw. It is useful for catching platform-specific compile errors
without a Windows machine. It is not how releases are built; those use the MSVC
toolchain on a real Windows runner.

## Icons

`assets/icon/` holds generated files that are committed rather than built:
the macOS `.iconset`, the Windows `.ico` that `build.rs` embeds, the hicolor
tree the Linux packaging copies, and the PNG the binary carries for its window
icon. Committing them keeps Python and Pillow off the dependency list for
everyone who is not changing the artwork.

If you are:

```sh
python3 scripts/make_icon.py assets/icon
```

## Troubleshooting

**`failed to run custom build command for libgit2-sys`** — `cmake` or a C
compiler is missing. Install the build-essential group for your platform.

**`error: linker 'cc' not found`** on Linux — install `build-essential`.

**The window is black, or the app exits immediately on Linux** — no usable
Vulkan or GL driver. `WGPU_BACKEND=gl` forces the GL path; `vulkaninfo` and
`glxinfo` will say what is actually available.

**`icon not embedded` warning during a Windows build** — cosmetic. The build
succeeds; the executable just has the default icon. It means `windres` or the
resource compiler was not found.
