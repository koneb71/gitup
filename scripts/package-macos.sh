#!/usr/bin/env bash
#
# Build Gitup and wrap it in a macOS .app bundle.
#
# The bundle is not code-signed. Gatekeeper will refuse to open an unsigned app
# downloaded from the internet, but one you built yourself opens after the usual
# right-click → Open. Signing needs a Developer ID certificate, which is a
# decision for whoever ships it, not something a build script should assume.
#
#   scripts/bundle.sh          build the .app
#   scripts/bundle.sh --dmg    also produce a disk image

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Gitup"
BUNDLE_ID="dev.gitup.Gitup"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
APP="$TARGET_DIR/$APP_NAME.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script builds a macOS bundle; on other platforms just use the" >&2
  echo "binary at $TARGET_DIR/release/gitup." >&2
  exit 1
fi

echo "==> Building release binary"
cargo build --release

echo "==> Preparing icon"
if [[ ! -d assets/icon/Gitup.iconset ]]; then
  python3 scripts/make_icon.py assets/icon
fi
iconutil --convert icns assets/icon/Gitup.iconset --output "$TARGET_DIR/$APP_NAME.icns"

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$TARGET_DIR/release/gitup" "$APP/Contents/MacOS/$APP_NAME"
cp "$TARGET_DIR/$APP_NAME.icns" "$APP/Contents/Resources/$APP_NAME.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <!-- Repositories live wherever the user keeps them, which on a modern
         macOS means Documents or Desktop; without these the first open of a
         repository there fails with a permission error rather than a prompt. -->
    <key>NSDocumentsFolderUsageDescription</key>
    <string>Gitup needs access to open repositories stored in your Documents folder.</string>
    <key>NSDesktopFolderUsageDescription</key>
    <string>Gitup needs access to open repositories stored on your Desktop.</string>
    <key>NSDownloadsFolderUsageDescription</key>
    <string>Gitup needs access to open repositories in your Downloads folder.</string>
</dict>
</plist>
PLIST

# Ad-hoc signature: not a Developer ID, but it stops macOS from treating the
# bundle as damaged when it is moved between folders.
codesign --force --deep --sign - "$APP" 2>/dev/null || \
  echo "    (ad-hoc signing skipped)"

echo "==> Built $APP ($(du -sh "$APP" | cut -f1))"

if [[ "${1:-}" == "--dmg" ]]; then
  DMG="$TARGET_DIR/$APP_NAME-$VERSION.dmg"
  echo "==> Creating $DMG"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  echo "==> Built $DMG ($(du -sh "$DMG" | cut -f1))"
fi

echo
echo "Open it with:  open \"$APP\""
