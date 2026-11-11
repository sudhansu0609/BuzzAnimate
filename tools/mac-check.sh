#!/usr/bin/env bash
# Type-checks the whole workspace for macOS (aarch64-apple-darwin) from a
# Windows machine, so cfg(target_os = "macos") code is compiled before it
# reaches a Mac. Run from Git Bash; run it before any change that touches cfg code.
#
# Two dependencies have C build scripts that cannot run here, so this script
# stands in for them (cargo check never links, so the stand-ins are never used
# to produce real code):
#   * rquickjs-sys needs a C compiler  -> a fake cc/ar built with rustc.
#   * coreaudio-sys needs Apple's SDK  -> a scratch copy of cpal whose macOS
#     host is its null host, passed with --config patch.crates-io.cpal.path.
# Cargo.lock is backed up and always restored on exit.
# The real CoreAudio path is unchanged by this and only a real Mac build checks it.
#
# Needs: rustup target add aarch64-apple-darwin
# Result: prints the error lines and exits 1, or prints "mac check: OK".
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 1
WORK="$ROOT/target/mac-check"
mkdir -p "$WORK"

cp Cargo.lock "$WORK/Cargo.lock.bak"
restore_lock() { cp "$WORK/Cargo.lock.bak" "$ROOT/Cargo.lock"; }
trap restore_lock EXIT

# --- stand-in C compiler and archiver ---------------------------------------
cat > "$WORK/fakecc.rs" <<'EOF'
fn main(){let a:Vec<String>=std::env::args().skip(1).collect(); if a.iter().any(|x|x=="-E"){println!("clang");} if a.iter().any(|x|x=="--version"){println!("clang version 18.0.0");} let mut i=0; while i<a.len(){ if a[i]=="-o"&&i+1<a.len(){let _=std::fs::write(&a[i+1],b"");i+=1;} else if let Some(p)=a[i].strip_prefix("-o"){ if !p.is_empty(){let _=std::fs::write(p,b"");}} i+=1;}}
EOF
cat > "$WORK/fakear.rs" <<'EOF'
fn main(){let a:Vec<String>=std::env::args().skip(1).collect(); if let Some(l)=a.iter().find(|x|x.ends_with(".a")){let _=std::fs::write(l,b"!<arch>\n");}}
EOF
rustc -O -o "$WORK/fakecc.exe" "$WORK/fakecc.rs" || { echo "error: could not build fakecc"; exit 1; }
rustc -O -o "$WORK/fakear.exe" "$WORK/fakear.rs" || { echo "error: could not build fakear"; exit 1; }

# --- scratch cpal with a null macOS host ------------------------------------
CPAL_SRC=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/index.crates.io-*/cpal-0.15.3 2>/dev/null | head -1)
if [ -z "$CPAL_SRC" ]; then
  echo "error: cpal-0.15.3 not in the cargo registry (run cargo fetch first)"
  exit 1
fi
rm -rf "$WORK/cpal"
cp -r "$CPAL_SRC" "$WORK/cpal"
grep -rl 'any(target_os = "macos", target_os = "ios")' "$WORK/cpal/src" \
  | xargs sed -i 's/any(target_os = "macos", target_os = "ios")/any()/g'
sed -i -e '/^    target_os = "macos",$/d' -e '/^    target_os = "ios",$/d' "$WORK/cpal/src/platform/mod.rs"
# drop the [target...] sections that pull in coreaudio-rs (a section runs to the next line starting with "[")
awk '
  /^\[/ { skip = ($0 ~ /coreaudio-rs/ && ($0 ~ /macos/ || $0 ~ /ios/)) }
  !skip { print }
' "$CPAL_SRC/Cargo.toml" > "$WORK/cpal/Cargo.toml"

# --- the check --------------------------------------------------------------
win() { cygpath -m "$1"; }
export CC_aarch64_apple_darwin="$(win "$WORK/fakecc.exe")"
export CXX_aarch64_apple_darwin="$(win "$WORK/fakecc.exe")"
export AR_aarch64_apple_darwin="$(win "$WORK/fakear.exe")"
CPAL_PATH="$(win "$WORK/cpal")"

cargo check --workspace --all-targets --target aarch64-apple-darwin --keep-going \
  --message-format short --config "patch.crates-io.cpal.path=\"$CPAL_PATH\"" \
  > "$WORK/check.log" 2>&1
STATUS=$?

# "error: could not compile ..." lines repeat the real errors but still count
if grep -E '(^|: )error(\[|:)' "$WORK/check.log" | head -40 | grep -q .; then
  grep -E '(^|: )error(\[|:)' "$WORK/check.log" | head -40
  exit 1
fi
if [ "$STATUS" -ne 0 ]; then
  echo "error: cargo check failed (see $WORK/check.log)"
  tail -5 "$WORK/check.log"
  exit 1
fi
echo "mac check: OK"
