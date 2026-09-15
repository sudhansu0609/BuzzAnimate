#!/usr/bin/env bash
# ===========================================================================
#  BuzzAnimate macOS Launcher
#
#  Double-click this file in Finder to build and run BuzzAnimate,
#  or run it from Terminal with optional arguments:
#
#      ./BuzzAnimate.command                      open an empty document
#      ./BuzzAnimate.command "path/to/scene.buzz" open a document
#      ./BuzzAnimate.command --dev                use debug build (faster compile)
#      ./BuzzAnimate.command --gpu "Apple M"      pick graphics adapter by name
#      ./BuzzAnimate.command --script tidy.js     run script at startup
# ===========================================================================

set -e

# Change directory to the repository folder
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

# Ensure standard Cargo and Homebrew paths are loaded even when double-clicked from Finder
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

PROFILE="release"
PROFILE_DIR="release"
CARGO_ARGS=("--release")
PASSTHROUGH_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dev)
            PROFILE="dev"
            PROFILE_DIR="debug"
            CARGO_ARGS=()
            shift
            ;;
        *)
            PASSTHROUGH_ARGS+=("$1")
            shift
            ;;
    esac
done

# Check if Rust / Cargo is installed
if ! command -v cargo >/dev/null 2>&1; then
    echo ""
    echo "  ==========================================================="
    echo "  Rust is not installed, or cargo is not on your PATH."
    echo "  Install it from https://rustup.rs and run this file again."
    echo "  ==========================================================="
    echo ""
    read -p "Press Enter to close..."
    exit 1
fi

# Check for ffmpeg (optional advisory for video / GIF export)
if ! command -v ffmpeg >/dev/null 2>&1; then
    echo ""
    echo "  [Advisory] ffmpeg was not detected. Video/GIF export requires ffmpeg."
    echo "  You can install it easily with: brew install ffmpeg"
    echo ""
fi

# Check if BuzzAnimate is already running
if pgrep -x "buzzanimate" >/dev/null 2>&1; then
    echo ""
    echo "  ==========================================================="
    echo "  BuzzAnimate is already running."
    echo "  Close the running copy and run this again."
    echo "  ==========================================================="
    echo ""
    read -p "Press Enter to close..."
    exit 1
fi

echo "Building BuzzAnimate ($PROFILE)..."
if ! cargo build "${CARGO_ARGS[@]}" -p buzz-app; then
    echo ""
    echo "  Build failed. Please inspect the errors above."
    echo ""
    read -p "Press Enter to close..."
    exit 1
fi

EXE="$HERE/target/$PROFILE_DIR/buzzanimate"
if [ ! -f "$EXE" ]; then
    echo "Error: Built executable not found at $EXE"
    read -p "Press Enter to close..."
    exit 1
fi

echo "Starting BuzzAnimate..."
exec "$EXE" "${PASSTHROUGH_ARGS[@]}"
