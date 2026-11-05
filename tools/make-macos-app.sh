#!/usr/bin/env bash
# Makes BuzzAnimate.app (in the repo root, git-ignored) so BuzzAnimate gets its
# own Dock icon and its own microphone permission (lip-sync recording) instead
# of borrowing Terminal's. macOS only, Apple Silicon.
#
#   ./tools/make-macos-app.sh
#
# The program itself is copied into the bundle (a wrapper script that execs
# target/release/buzzanimate would lose the bundle's name, icon and microphone
# text once the exec happens). Its Info.plist sets BUZZ_HOME to THIS folder, so
# the app finds assets/ and .env wherever the .app is moved. After pulling
# changes, run this script again: it rebuilds and re-copies.
set -e

if [ "$(uname)" != "Darwin" ]; then
    echo "make-macos-app.sh builds a macOS app bundle and only runs on a Mac."
    exit 1
fi

# Same PATH set-up as BuzzAnimate.command, so cargo is found from any shell
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

cd "$(dirname "${BASH_SOURCE[0]}")/.."
REPO="$(pwd)"

cargo build --release -p buzz-app

APP="BuzzAnimate.app"
VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# The program, plus any dylib the build put beside it
cp target/release/buzzanimate "$APP/Contents/MacOS/BuzzAnimate"
cp target/release/*.dylib "$APP/Contents/MacOS/" 2>/dev/null || true
chmod +x "$APP/Contents/MacOS/BuzzAnimate"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>BuzzAnimate</string>
    <key>CFBundleDisplayName</key>
    <string>BuzzAnimate</string>
    <key>CFBundleIdentifier</key>
    <string>com.buzzcaf.buzzanimate</string>
    <key>CFBundleExecutable</key>
    <string>BuzzAnimate</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>BuzzAnimate records your voice for lip sync.</string>
    <key>LSEnvironment</key>
    <dict>
        <key>BUZZ_HOME</key>
        <string>$REPO</string>
    </dict>
</dict>
</plist>
EOF

# The icon: the standard iconset sizes from assets/logo-*.png, then .icns
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
mkicon() { # <file name> <pixels>
    sips -z "$2" "$2" "assets/logo-$2.png" --out "$ICONSET/$1" >/dev/null
}
mkicon icon_16x16.png 16
mkicon icon_16x16@2x.png 32
mkicon icon_32x32.png 32
mkicon icon_32x32@2x.png 64
mkicon icon_128x128.png 128
mkicon icon_128x128@2x.png 256
mkicon icon_256x256.png 256
mkicon icon_256x256@2x.png 512
mkicon icon_512x512.png 512
iconutil -c icns -o "$APP/Contents/Resources/AppIcon.icns" "$ICONSET"
rm -rf "$(dirname "$ICONSET")"

# Ad-hoc signature, so Gatekeeper treats the bundle as one piece
codesign -s - --force --deep "$APP" || true
# Re-read Info.plist (LSEnvironment is cached when the app is first seen)
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" >/dev/null 2>&1 || true

echo ""
echo "  Made $REPO/$APP"
echo "  Drag it to the Dock or /Applications. It uses this folder's assets,"
echo "  so keep this folder, and run this script again after pulling changes."
echo ""
