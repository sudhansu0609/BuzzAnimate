@echo off
rem ===========================================================================
rem  BuzzAnimate launcher.
rem
rem  Double-click it, or run it from a terminal with arguments:
rem
rem      BuzzAnimate.bat                     open an empty document
rem      BuzzAnimate.bat "C:\work\Scene.buzz"  open a document
rem      BuzzAnimate.bat --gpu NVIDIA        pick a graphics adapter by name
rem      BuzzAnimate.bat --script tidy.js    run a script at startup
rem      BuzzAnimate.bat --dev               use the debug build (faster to
rem                                          compile, slower to draw)
rem
rem  It builds first when the sources have changed, which is a no-op once the
rem  build is warm: a launcher that quietly ran last week's binary would be a
rem  very confusing thing to own.
rem ===========================================================================

setlocal EnableDelayedExpansion
cd /d "%~dp0"

set "PROFILE=release"
set "PROFILE_DIR=release"
set "ARGS="

rem --- pull `--dev` out of the arguments; pass everything else through ------
:parse
if "%~1"=="" goto parsed
if /I "%~1"=="--dev" (
    set "PROFILE=dev"
    set "PROFILE_DIR=debug"
) else (
    set "ARGS=!ARGS! "%~1""
)
shift
goto parse
:parsed

where cargo >nul 2>&1
if errorlevel 1 (
    echo.
    echo   Rust is not installed, or cargo is not on the PATH.
    echo   Install it from https://rustup.rs and run this again.
    echo.
    pause
    exit /b 1
)

rem --- refuse to build over a copy that is already running -------------------
rem
rem  Windows will not let cargo replace a running .exe, and cargo's message for
rem  it is "failed to remove file ... Access is denied (os error 5)", which
rem  says nothing about the cause. Worse, the old binary is still sitting there
rem  afterwards, so anything that skips the error runs last build's program and
rem  every fix looks as though it did nothing. Ask first, and say why.
tasklist /FI "IMAGENAME eq buzzanimate.exe" 2>nul | find /I "buzzanimate.exe" >nul
if not errorlevel 1 (
    echo.
    echo   BuzzAnimate is already running.
    echo.
    echo   Windows will not let a running program be replaced, so the build
    echo   would fail and this would start the OLD copy again - which looks
    echo   exactly like a fix that did not work.
    echo.
    echo   Close BuzzAnimate and run this again.
    echo.
    pause
    exit /b 1
)

echo Building BuzzAnimate ^(%PROFILE%^)...
if "%PROFILE%"=="release" (
    cargo build --release -p buzz-app
) else (
    cargo build -p buzz-app
)
if errorlevel 1 (
    echo.
    echo   The build failed. Nothing was started - the binary in
    echo   target\%PROFILE_DIR%\ is whatever was there before, and is NOT what
    echo   the sources say. Fix the build before running it.
    echo.
    pause
    exit /b 1
)

set "EXE=target\%PROFILE_DIR%\buzzanimate.exe"
if not exist "%EXE%" (
    echo.
    echo   Built, but %EXE% is not there. Has the binary been renamed?
    echo.
    pause
    exit /b 1
)

echo Starting BuzzAnimate...
echo.
rem Run it here rather than through `start`: the binary keeps a console for its
rem adapter table and its crash message, and `start` hands the child whatever
rem window state it was itself given — which, launched from a script with no
rem console of its own, opened BuzzAnimate minimised off-screen. Found by
rem running this file and looking for the window.
"%EXE%" %ARGS%
endlocal
