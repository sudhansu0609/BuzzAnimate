# BuzzAnimate's environment, (re)built in place. Run by "BuzzAnimate Setup.exe", or by hand:
#   powershell -ExecutionPolicy Bypass -File installer\setup_env.ps1
# Every step skips what is already in place, so re-running it repairs a half-finished or
# cleaned-up install (e.g. after a disk cleaner removed target\). cargo's own incremental
# build is what makes the build step itself a fast no-op once it is warm.
. (Join-Path $PSScriptRoot "setup_common.ps1")
$root = $script:AppRoot
Set-Steps 2

Step "Checking the tools BuzzAnimate needs (Rust toolchain, cargo)"
$cargo = Require-Command "cargo" "Install Rust from https://rustup.rs, then run Setup again."

Step "Building the release binary (crates\buzz-app -> target\release\buzzanimate.exe)"
# Same command as BuzzAnimate.bat's release path: cargo build --release -p buzz-app.
Invoke-Native "cargo build --release" $cargo @("build", "--release", "-p", "buzz-app") $root
$exe = Join-Path $root "target\release\buzzanimate.exe"
if (-not (Test-Path $exe)) { Fail "build finished but $exe is missing" }

Done "BuzzAnimate's release build is ready."
