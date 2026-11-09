#!/usr/bin/env bash
# ===========================================================================
#  Builds BuzzAnimate.app and BuzzAnimate.dmg for macOS (Apple Silicon).
#
#      installer/macos/build_app.sh             build the .app and .dmg into dist/
#      installer/macos/build_app.sh --install   also copy the .app into /Applications
#
#  The bundle is self-contained apart from ffmpeg (video/GIF export), which is
#  picked up from Homebrew when present. User data that the source checkout
#  keeps at the repo root (.env) lives in ~/Library/Application Support/BuzzAnimate.
# ===========================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST="$ROOT/dist"
APP="$DIST/BuzzAnimate.app"
DMG="$DIST/BuzzAnimate-$VERSION-macos-arm64.dmg"

echo "== Building release binary"
cargo build --release -p buzz-app

echo "== Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/buzzanimate "$APP/Contents/MacOS/buzzanimate-bin"
cp -R assets "$APP/Contents/Resources/assets"

# Finder starts apps with cwd "/" and a minimal PATH, so the launcher sets up
# what the editor expects: assets relative to cwd, ffmpeg on PATH, a home for .env.
# The binary is named buzzanimate-bin: APFS is case-insensitive, so
# "buzzanimate" would be the same file as this "BuzzAnimate" launcher.
cat > "$APP/Contents/MacOS/BuzzAnimate" <<'EOF'
#!/bin/bash
HERE="$(cd "$(dirname "$0")" && pwd)"
RES="$HERE/../Resources"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
export BUZZ_HOME="${BUZZ_HOME:-$HOME/Library/Application Support/BuzzAnimate}"
mkdir -p "$BUZZ_HOME"
export BUZZ_COMFY_PROFILES="${BUZZ_COMFY_PROFILES:-$RES/assets/comfy_profiles}"
cd "$RES"
exec "$HERE/buzzanimate-bin" "$@"
EOF
chmod +x "$APP/Contents/MacOS/BuzzAnimate"

echo "== Icon"
ICONSET="$DIST/BuzzAnimate.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
    sips -z $s $s assets/logo-512.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d assets/logo-512.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/BuzzAnimate.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>BuzzAnimate</string>
    <key>CFBundleDisplayName</key><string>BuzzAnimate</string>
    <key>CFBundleIdentifier</key><string>com.spilledcoffee.buzzanimate</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key><string>BuzzAnimate</string>
    <key>CFBundleIconFile</key><string>BuzzAnimate</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>BuzzAnimate Document</string>
            <key>CFBundleTypeExtensions</key><array><string>buzz</string></array>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>CFBundleTypeIconFile</key><string>BuzzAnimate</string>
        </dict>
    </array>
</dict>
</plist>
EOF

echo "== Ad-hoc signing"
codesign --force --deep --sign - "$APP"

echo "== Disk image"
STAGE="$DIST/dmg-stage"
rm -rf "$STAGE" "$DMG" && mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "BuzzAnimate" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

if [ "${1:-}" = "--install" ]; then
    echo "== Installing to /Applications"
    rm -rf /Applications/BuzzAnimate.app
    cp -R "$APP" /Applications/
fi

echo "Done:"
echo "  $APP"
echo "  $DMG"
