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

The system libraries are the only fiddly part. On Debian or Ubuntu:

```sh
sudo apt-get install build-essential pkg-config cmake libgtk-3-dev libssl-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libxkbcommon-dev \
  libwayland-dev libgl1-mesa-dev
```

On Fedora:

```sh
sudo dnf install gcc gcc-c++ pkgconf-pkg-config cmake gtk3-devel openssl-devel \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel libxkbcommon-devel \
  wayland-devel mesa-libGL-devel
```

On Arch:

```sh
sudo pacman -S base-devel pkgconf cmake gtk3 openssl libx11 libxcursor \
  libxrandr libxi libxkbcommon wayland mesa
```

What each group is for:

| Package group | Needed by |
|---|---|
| `build-essential`, `pkg-config`, `cmake` | compiling the vendored libgit2 |
| `libgtk-3-dev` | `rfd`, which draws the native folder picker |
| `libssl-dev` | libgit2's HTTPS support |
| X11 and `libxkbcommon` | window creation and keyboard layout handling |
| `libwayland-dev` | the Wayland backend |
| `libgl1-mesa-dev` | `wgpu`'s GL fallback, used when Vulkan is unavailable |

The authoritative list is [`scripts/docker/Dockerfile.linux`](../scripts/docker/Dockerfile.linux),
which CI and the container build both use — if the table above drifts, trust the
Dockerfile.

### Packaging

```sh
scripts/package-linux.sh
```

Produces `target/gitup-<version>-linux-<arch>.tar.gz` containing the binary, a
`.desktop` entry, hicolor icons, and an `install.sh` that copies them under a
prefix (`~/.local` by default, which needs no root):

```sh
tar xzf gitup-0.1.0-linux-x86_64.tar.gz
./gitup-0.1.0-linux-x86_64/install.sh
```

A tarball rather than a `.deb` or `.rpm` on purpose: those bind the result to
one distribution's packaging policy, and a tarball works everywhere. Distro
packages are welcome as separate contributions.

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

Produces `target\gitup-<version>-windows-<arch>.zip` containing `gitup.exe` and
the licence. It is portable: unzip anywhere and run it.

There is no MSI. An installer is worth having when it is code-signed, and an
unsigned one trips SmartScreen exactly like a bare `.exe` does while adding a
build dependency on WiX.

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
