#!/usr/bin/env bash
#
# Build Gitup and lay it out the way Linux expects to find an application.
#
# Produces a relocatable tarball containing the binary, a .desktop entry, and
# hicolor icons, plus an install script that copies them under a prefix. That
# is deliberately plainer than a .deb or an .rpm: those bind the result to one
# distribution's packaging policy, and a tarball works on all of them. Distro
# packages are welcome as separate contributions.
#
#   scripts/package-linux.sh            build the tarball
#   scripts/package-linux.sh --install  build it and install into ~/.local
#
# See docs/building.md for the build dependencies.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Gitup"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
# Made absolute once, here, rather than at each use. These scripts change
# directory — dpkg-shlibdeps insists on being run from its own working tree —
# and a relative path silently stops resolving the moment they do. It is also
# invisible to anyone whose CARGO_TARGET_DIR happens to be absolute, which is
# how it reached CI: every local test set one, and the runner does not.
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT/$TARGET_DIR" ;; esac
ARCH="$(uname -m)"
STAGE_NAME="gitup-$VERSION-linux-$ARCH"
STAGE="$TARGET_DIR/$STAGE_NAME"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This script packages for Linux; you are on $(uname -s)." >&2
  echo "Use scripts/package-macos.sh or scripts/package-windows.ps1." >&2
  exit 1
fi

echo "==> Building release binary"
cargo build --release --locked

echo "==> Assembling $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/applications" "$STAGE/share/icons"
install -m 755 "$TARGET_DIR/release/gitup" "$STAGE/bin/gitup"
cp -R assets/icon/hicolor "$STAGE/share/icons/hicolor"
install -m 644 LICENSE "$STAGE/LICENSE"
install -m 644 README.md "$STAGE/README.md"

# StartupWMClass has to match the app id the window sets, or the running window
# is not associated with its launcher and the dock shows two of it.
cat > "$STAGE/share/applications/gitup.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$APP_NAME
GenericName=Git Client
Comment=A modern graphical Git client
Exec=gitup %f
Icon=gitup
Terminal=false
Categories=Development;RevisionControl;
Keywords=git;version control;vcs;commit;diff;
StartupNotify=true
StartupWMClass=dev.gitup.Gitup
MimeType=inode/directory;
DESKTOP

cat > "$STAGE/install.sh" <<'INSTALL'
#!/usr/bin/env sh
# Install Gitup under a prefix. Defaults to ~/.local, which needs no root and
# is on the XDG search path that desktops already read.
set -eu
PREFIX="${1:-$HOME/.local}"
HERE="$(cd "$(dirname "$0")" && pwd)"

mkdir -p "$PREFIX/bin" "$PREFIX/share/applications" "$PREFIX/share/icons"
cp "$HERE/bin/gitup" "$PREFIX/bin/gitup"
chmod 755 "$PREFIX/bin/gitup"
cp "$HERE/share/applications/gitup.desktop" "$PREFIX/share/applications/"
cp -R "$HERE/share/icons/hicolor" "$PREFIX/share/icons/"

# Desktops cache both of these; without a refresh the entry can take a logout
# to appear. Neither is fatal if missing.
command -v update-desktop-database >/dev/null 2>&1 &&
  update-desktop-database "$PREFIX/share/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true

echo "Installed to $PREFIX/bin/gitup"
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac
INSTALL
chmod 755 "$STAGE/install.sh"

TARBALL="$TARGET_DIR/$STAGE_NAME.tar.gz"
echo "==> Creating $TARBALL"
rm -f "$TARBALL"
tar -czf "$TARBALL" -C "$TARGET_DIR" "$STAGE_NAME"
echo "==> Built $TARBALL ($(du -sh "$TARBALL" | cut -f1))"

# The same staged tree feeds every format, so the three cannot disagree about
# what a Gitup installation contains.
#
# Exit 2 from either means the tool is not installed, which is fine on a
# developer machine. Anything else is a real failure and stops the run: a
# release that silently shipped without its .deb is how this rule was learned.
package_with() {
  local script="$1"
  "$ROOT/scripts/$script" "$STAGE" && return 0
  local status=$?
  if [[ $status -eq 2 ]]; then
    return 0
  fi
  echo "==> $script failed (exit $status)" >&2
  exit $status
}

package_with package-deb.sh
package_with package-appimage.sh

if [[ "${1:-}" == "--install" ]]; then
  "$STAGE/install.sh"
else
  echo
  echo "Install it with:  $STAGE/install.sh"
fi
