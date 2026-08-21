#!/usr/bin/env bash
#
# Build a .deb from the tree scripts/package-linux.sh has already staged.
#
# Plain dpkg-deb rather than a cargo plugin: the payload is one binary, a
# desktop entry and some icons, and a build tool that has to be installed first
# is a poor trade for a package this simple.
#
# Dependencies are computed by dpkg-shlibdeps rather than written down, because
# a hand-written version constraint is wrong the moment the toolchain moves.
# Only what the binary actually links appears there — X11, Wayland, the
# graphics drivers and the desktop portal are opened at runtime, so no scanner
# can see them and they are declared as Recommends by hand below.

set -euo pipefail

STAGE="${1:?usage: package-deb.sh <staged-directory>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Exit 2 means "the tool is not here, carry on without me"; any other non-zero
# means something went wrong and the caller must not paper over it. The two
# were indistinguishable before, so a broken .deb build looked exactly like a
# machine that simply had no dpkg.
if ! command -v dpkg-deb >/dev/null 2>&1 || ! command -v dpkg-shlibdeps >/dev/null 2>&1; then
  echo "==> dpkg-deb/dpkg-shlibdeps not found; skipping the .deb" >&2
  exit 2
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
# Made absolute once, here, rather than at each use. These scripts change
# directory — dpkg-shlibdeps insists on being run from its own working tree —
# and a relative path silently stops resolving the moment they do. It is also
# invisible to anyone whose CARGO_TARGET_DIR happens to be absolute, which is
# how it reached CI: every local test set one, and the runner does not.
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT/$TARGET_DIR" ;; esac
# Debian's own architecture names, which are not uname's.
ARCH="$(dpkg --print-architecture)"
ROOTFS="$TARGET_DIR/deb/gitup_${VERSION}_${ARCH}"

echo "==> Assembling $ROOTFS"
rm -rf "$ROOTFS"
mkdir -p "$ROOTFS/DEBIAN" "$ROOTFS/usr/bin" "$ROOTFS/usr/share"
install -m 755 "$STAGE/bin/gitup" "$ROOTFS/usr/bin/gitup"
cp -R "$STAGE/share/applications" "$ROOTFS/usr/share/applications"
cp -R "$STAGE/share/icons" "$ROOTFS/usr/share/icons"

# Debian expects all three of these, at paths derived from the package name.
# lintian treats a missing changelog as an error rather than a nicety: it is
# how a user finds out what changed in the version they were just given.
mkdir -p "$ROOTFS/usr/share/doc/gitup" "$ROOTFS/usr/share/man/man1"
install -m 644 LICENSE "$ROOTFS/usr/share/doc/gitup/copyright"
# A native package — the packaging lives in the upstream tree, which is what
# native means — so this file has to be in Debian's changelog format rather
# than being CHANGELOG.md renamed. It points at the full one instead of
# duplicating it, since the two would drift.
#
# SOURCE_DATE_EPOCH is honoured so the package is reproducible; without it the
# timestamp is the build time and two builds of the same source differ.
STAMP="$(date -R -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" 2>/dev/null \
  || date -R -u -r "${SOURCE_DATE_EPOCH:-$(date +%s)}")"
gzip -9 -n -c > "$ROOTFS/usr/share/doc/gitup/changelog.gz" <<CHANGELOG
gitup ($VERSION) unstable; urgency=medium

  * Release $VERSION. See /usr/share/doc/gitup/copyright for licensing and
    https://github.com/koneb71/gitup/blob/main/CHANGELOG.md for what changed.

 -- Gitup contributors <noreply@github.com>  $STAMP
CHANGELOG
chmod 644 "$ROOTFS/usr/share/doc/gitup/changelog.gz"
gzip -9 -n -c assets/man/gitup.1 > "$ROOTFS/usr/share/man/man1/gitup.1.gz"
chmod 644 "$ROOTFS/usr/share/man/man1/gitup.1.gz"

cat > "$ROOTFS/DEBIAN/control" <<CONTROL
Package: gitup
Version: $VERSION
Section: vcs
Priority: optional
Architecture: $ARCH
Maintainer: Gitup contributors <noreply@github.com>
Homepage: https://github.com/koneb71/gitup
Recommends: git, xdg-desktop-portal
Suggests: xdg-desktop-portal-gtk | xdg-desktop-portal-kde | xdg-desktop-portal-wlr
Description: Modern graphical Git client
 Gitup is a native Git client built on libgit2 and egui: a commit graph with
 real lane assignment, diffs with syntax highlighting and word-level intra-line
 comparison, blame, staging by file, hunk or individual line, and a three-way
 conflict editor.
 .
 Network operations run the real git binary, so existing credential helpers,
 the SSH agent and the system keychain work without configuration. git is
 recommended rather than required because everything local works without it.
CONTROL

# dpkg-shlibdeps computes the Depends line, including the version constraints
# that say which releases the binary will actually run on. Those are the part
# worth having and the part nobody gets right by hand: a package built on
# bookworm that claims plain "libc6" installs happily on an older system and
# then fails to start.
#
# The tool insists on a debian/ directory next to where it is run, so it gets a
# minimal one. A failure here is reported rather than swallowed — falling back
# to an unversioned guess would produce exactly the package described above.
echo "==> Computing dependencies"
DEPWORK="$TARGET_DIR/deb/shlibdeps"
rm -rf "$DEPWORK"
mkdir -p "$DEPWORK/debian"
printf 'Source: gitup\n\nPackage: gitup\nArchitecture: any\n' > "$DEPWORK/debian/control"

# stderr is kept: when this fails, the reason is the only useful thing there
# is, and throwing it away is what made the first CI failure a mystery.
SHLIBDEPS_LOG="$DEPWORK/stderr.log"
DEPENDS="$( (cd "$DEPWORK" && dpkg-shlibdeps -O --ignore-missing-info \
  "$ROOTFS/usr/bin/gitup" 2>"$SHLIBDEPS_LOG") | sed 's/^shlibs:Depends=//')"

if [[ -z "$DEPENDS" ]]; then
  echo "==> dpkg-shlibdeps produced nothing; refusing to guess a Depends line" >&2
  sed 's/^/    /' "$SHLIBDEPS_LOG" >&2 || true
  rm -rf "$DEPWORK"
  exit 1
fi
rm -rf "$DEPWORK"
echo "Depends: $DEPENDS" >> "$ROOTFS/DEBIAN/control"
echo "    $DEPENDS"

# Both caches are consulted by desktops at login; refreshing them is what makes
# the launcher appear without one. Neither is fatal if the tool is absent.
cat > "$ROOTFS/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if [ "$1" = "configure" ]; then
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database -q /usr/share/applications || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q -f -t /usr/share/icons/hicolor || true
fi
POSTINST
cat > "$ROOTFS/DEBIAN/postrm" <<'POSTRM'
#!/bin/sh
set -e
if [ "$1" = "remove" ] || [ "$1" = "purge" ]; then
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database -q /usr/share/applications || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q -f -t /usr/share/icons/hicolor || true
fi
POSTRM
chmod 755 "$ROOTFS/DEBIAN/postinst" "$ROOTFS/DEBIAN/postrm"

DEB="$TARGET_DIR/gitup_${VERSION}_${ARCH}.deb"
echo "==> Creating $DEB"
rm -f "$DEB"
# Ownership matters: files unpacked from a .deb must belong to root, and the
# staged tree belongs to whoever ran the build.
dpkg-deb --root-owner-group --build "$ROOTFS" "$DEB" >/dev/null
echo "==> Built $DEB ($(du -sh "$DEB" | cut -f1))"
