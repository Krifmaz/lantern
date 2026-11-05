#!/usr/bin/env bash
# Lantern's desktop release files, one per system, named as the app's update notice looks for them
# (desktop/src/release.rs `file_for`):
#   Linux    Lantern-<V>-linux-x86_64.tar.gz   the binary in a folder
#   Windows  Lantern-<V>-windows-x86_64.zip    the .exe (signed when WINDOWS_CERT_PFX is set)
#   macOS    Lantern-<V>-macos-universal.dmg   Lantern.app (Apple silicon and Intel joined with lipo), signed
#                                            with MACOS_SIGN_IDENTITY (ad hoc without it), notarized with APPLE_ID,
#                                            APPLE_TEAM_ID and APPLE_APP_PASSWORD when they are set
#
# The kit's kit-desktop.yml runs it on each system from the repository root (its `package` in release.yml):
#   bash scripts/desktop-package.sh --version <V> [--stamp | --release true|false] --features <features> [--universal] --out <dir>
# --stamp (or --release true) builds the app as version <V> (APP_VERSION), so its update notice compares the right
# number; --universal joins both Mac targets.
set -euo pipefail

VERSION=""
STAMP=false
FEATURES=""
UNIVERSAL=false
OUT=out
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --stamp) STAMP=true; shift ;;
    --release) [ "$2" = true ] && STAMP=true; shift 2 ;;
    --features) FEATURES="$2"; shift 2 ;;
    --universal) UNIVERSAL=true; shift ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "--version is required" >&2; exit 2; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
FILE="Lantern"
TITLE="Lantern"
BIN="lantern-desktop"
BUNDLE_ID="dev.krifmaz.lantern.desktop"
if [ "$STAMP" = true ]; then export APP_VERSION="$VERSION"; fi
ARGS=(--release --locked)
if [ -n "$FEATURES" ]; then ARGS+=(--features "$FEATURES"); fi
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cd "$ROOT/desktop"

case "$(uname -s)" in
  Linux)
    cargo build "${ARGS[@]}"
    DIR="$STAGE/$FILE-$VERSION"
    mkdir -p "$DIR"
    cp "target/release/$BIN" README.md "$DIR/"
    tar -C "$STAGE" -czf "$OUT/$FILE-$VERSION-linux-x86_64.tar.gz" "$FILE-$VERSION"
    ;;
  MINGW* | MSYS* | CYGWIN*)
    cargo build "${ARGS[@]}"
    EXE="target/release/$BIN.exe"
    if [ -n "${WINDOWS_CERT_PFX:-}" ]; then
      SIGNTOOL="$(ls "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/signtool.exe | sort -V | tail -1)"
      "$SIGNTOOL" sign /f "$(cygpath -w "$WINDOWS_CERT_PFX")" /p "$WINDOWS_CERT_PASSWORD" /fd sha256 \
        /tr http://timestamp.digicert.com /td sha256 "$(cygpath -w "$EXE")"
    fi
    cp "$EXE" "$STAGE/$FILE.exe"
    (cd "$STAGE" && 7z a -tzip -bd "$OUT/$FILE-$VERSION-windows-x86_64.zip" "$FILE.exe" > /dev/null)
    ;;
  Darwin)
    if [ "$UNIVERSAL" = true ]; then
      for T in aarch64-apple-darwin x86_64-apple-darwin; do cargo build "${ARGS[@]}" --target "$T"; done
      lipo -create -output "$STAGE/$BIN" "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
    else
      cargo build "${ARGS[@]}"
      cp "target/release/$BIN" "$STAGE/$BIN"
    fi
    APP="$STAGE/dmg/$TITLE.app"
    mkdir -p "$APP/Contents/MacOS"
    mv "$STAGE/$BIN" "$APP/Contents/MacOS/$BIN"
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$TITLE</string>
  <key>CFBundleDisplayName</key><string>$TITLE</string>
  <key>CFBundleExecutable</key><string>$BIN</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
    # a real identity signs with a secure timestamp (notarization needs one); without it the app is signed ad hoc
    if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
      codesign --force --deep --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP"
    else
      codesign --force --deep --sign - "$APP"
    fi
    ln -s /Applications "$STAGE/dmg/Applications"
    DMG="$OUT/$FILE-$VERSION-macos-universal.dmg"
    hdiutil create -volname "$TITLE" -srcfolder "$STAGE/dmg" -ov -format UDZO "$DMG"
    if [ -n "${MACOS_SIGN_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
      xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
      xcrun stapler staple "$DMG"
    fi
    ;;
  *)
    echo "No desktop package for $(uname -s)" >&2
    exit 1
    ;;
esac
ls -la "$OUT"
