#!/usr/bin/env bash
#
# Build an AppImage from the tree scripts/package-linux.sh has already staged.
#
# An AppImage runs on distributions whose package manager Gitup has no package
# for, which is most of them. It bundles only the binary: the libraries the app
# actually links are libc, libgcc, libm and libz, all of which any system
# running a desktop already has at a compatible version, and the rest — X11,
# Wayland, the graphics drivers, the desktop portal — are opened at runtime and
# must come from the host anyway, because they are the host's hardware and
# session.
#
# Requires appimagetool. In CI it is fetched; locally it is skipped when absent
# rather than failing the whole packaging run.

set -euo pipefail

STAGE="${1:?usage: package-appimage.sh <staged-directory>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
ARCH="$(uname -m)"
APPDIR="$TARGET_DIR/Gitup.AppDir"

TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
if [[ -z "$TOOL" ]]; then
  echo "==> appimagetool not found; skipping the AppImage" >&2
  exit 1
fi

echo "==> Assembling $APPDIR"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share"
install -m 755 "$STAGE/bin/gitup" "$APPDIR/usr/bin/gitup"
cp -R "$STAGE/share/applications" "$APPDIR/usr/share/applications"
cp -R "$STAGE/share/icons" "$APPDIR/usr/share/icons"

# appimagetool looks for all three of these at the AppDir root, by name.
install -m 644 "$STAGE/share/applications/gitup.desktop" "$APPDIR/gitup.desktop"
install -m 644 assets/icon/hicolor/256x256/apps/gitup.png "$APPDIR/gitup.png"
ln -sf gitup.png "$APPDIR/.DirIcon"

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
# Resolve the payload relative to this file rather than to the working
# directory: an AppImage is mounted at a different path on every run.
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
# So the launcher and the icon are found when the AppImage is integrated.
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
exec "$HERE/usr/bin/gitup" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

OUT="$TARGET_DIR/Gitup-$VERSION-$ARCH.AppImage"
echo "==> Creating $OUT"
rm -f "$OUT"
# No FUSE on a CI runner, and appimagetool is itself an AppImage: without this
# it cannot even unpack itself. ARCH is read from the environment by the tool.
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT" >/dev/null 2>&1 \
  || ARCH="$ARCH" "$TOOL" "$APPDIR" "$OUT"
chmod +x "$OUT"
echo "==> Built $OUT ($(du -sh "$OUT" | cut -f1))"
