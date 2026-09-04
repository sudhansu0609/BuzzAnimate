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
rem      BuzzAnimate.bat --console           watch a release build go past
rem
rem  It builds first when the sources have changed, which is a no-op once the
rem  build is warm: a launcher that quietly ran last week's binary would be a
rem  very confusing thing to own.
rem
rem  THERE IS NO CONSOLE UNLESS YOU ASK FOR ONE. Opening the editor should put
rem  the editor on screen and nothing else, so by default this hands itself to
rem  BuzzAnimate.vbs, which runs it hidden and puts anything that goes wrong in
rem  a dialog box. That holds however it is started: the desktop shortcut, a
rem  double-click on this file, or a terminal.
rem
rem  `--dev` and `--console` keep the window, because that is where the adapter
rem  table, the tracing output, a panic's backtrace and the build itself go -
rem  which is what you want when you are the one changing the sources.
rem ===========================================================================

setlocal EnableDelayedExpansion
cd /d "%~dp0"

rem  This folder, captured before anything else touches the arguments.
rem  `shift` in the parse loop below shifts %0 along with the rest, so after it
rem  `%~dp0` is the directory of whatever argument ended up there - open a
rem  document with this launcher and it became the document's folder, and the
rem  hand-off below went looking for BuzzAnimate.vbs beside the .buzz file.
set "HERE=%~dp0"

set "PROFILE=release"
set "PROFILE_DIR=release"
set "ARGS="

rem --- pull `--dev` and `--console` out; pass everything else through -------
:parse
if "%~1"=="" goto parsed
if /I "%~1"=="--dev" (
    set "PROFILE=dev"
    set "PROFILE_DIR=debug"
) else if /I "%~1"=="--console" (
    set "WANT_CONSOLE=1"
) else (
    set "ARGS=!ARGS! "%~1""
)
shift
goto parse
:parsed

rem --- no console unless one was actually asked for --------------------------
rem
rem  Opening the editor should put the editor on screen and nothing else. But
rem  Windows hands a batch file a console before a single line of it has run,
rem  so double-clicking this one always showed a black window beside the
rem  editor for as long as the build took. It cannot be prevented from in here
rem  - it can only be handed off: BuzzAnimate.vbs runs a fresh copy of this
rem  file with no console at all, so what is left is a flash rather than a
rem  window that sits there. The desktop shortcut goes straight to the .vbs
rem  and does not flash either; this is the safety net for anyone who opens
rem  the folder and double-clicks the launcher instead.
rem
rem  **Asked for, not guessed at.** The obvious trick is to sniff
rem  `%cmdcmdline%` for this file's name and call that a double-click. It is
rem  wrong: PowerShell runs a .bat as `cmd /c "<path>"` too, so running it
rem  from a terminal looked exactly like a double-click and the build output
rem  a developer had just asked for went to a log nobody was watching. So the
rem  console is kept when it is *requested* - `--dev`, which exists for the
rem  adapter table and the backtraces, or `--console` for watching a release
rem  build go past - and not otherwise. BUZZ_SILENT means the .vbs is already
rem  the caller, and handing back to it would be a loop.
if defined BUZZ_SILENT goto :keep_console
if defined WANT_CONSOLE goto :keep_console
if "%PROFILE%"=="dev" goto :keep_console
if not exist "%HERE%BuzzAnimate.vbs" goto :keep_console
start "" wscript.exe "%HERE%BuzzAnimate.vbs" %*
exit /b 0
:keep_console

where cargo >nul 2>&1
if errorlevel 1 (
    echo.
    echo   Rust is not installed, or cargo is not on the PATH.
    echo   Install it from https://rustup.rs and run this again.
    echo.
    goto :halt
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
    goto :halt
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
    goto :halt
)

set "EXE=target\%PROFILE_DIR%\buzzanimate.exe"
if not exist "%EXE%" (
    echo.
    echo   Built, but %EXE% is not there. Has the binary been renamed?
    echo.
    goto :halt
)

echo Starting BuzzAnimate...
echo.
rem --- how the program is started, and why it differs by profile -------------
rem
rem  Release is a Windows GUI binary (see the note at the top of main.rs), so it
rem  has no console of its own. `start` hands it over and this script exits, and
rem  the terminal window this batch file runs in closes with it: one window on
rem  screen, which is the editor. The empty "" is the title argument `start`
rem  requires before a quoted path - without it the path IS taken as the title
rem  and nothing launches.
rem
rem  A previous attempt at `start` opened BuzzAnimate minimised off-screen. The
rem  cause was the desktop shortcut, which used to be created minimised
rem  (WindowStyle 7) so its console would not flash: `start` passed that state
rem  on to a *console* binary. There is no console to hide now - the shortcut
rem  goes through BuzzAnimate.vbs - so it is created normal, and a GUI binary
rem  makes its own window and is unaffected either way.
rem
rem  Debug stays in the console on purpose. `--dev` is how you ask for the
rem  adapter table, the tracing output and a panic's backtrace, and all of that
rem  goes to this window - so this window has to stay.
rem
rem  Unless there is no window. Started hidden through the .vbs, `--dev` would
rem  run the console build with its output going to a log nobody is watching
rem  and the launcher waiting on it forever, which looks exactly like the app
rem  failing to open. So in that one case it is handed over with `start`, which
rem  gives the debug build a console of its own.
if "%PROFILE%"=="release" (
    start "" "%EXE%" %ARGS%
) else if defined BUZZ_SILENT (
    start "" "%EXE%" %ARGS%
) else (
    "%EXE%" %ARGS%
)
endlocal
exit /b 0

rem ===========================================================================
rem  Every failure ends here.
rem
rem  **`pause` is not always available.** The desktop shortcut starts this
rem  through BuzzAnimate.vbs, which runs it with no console at all so the black
rem  window never appears - see the note there. In that state a `pause` waits
rem  for a keypress nobody can give: the launcher would hang forever, invisibly,
rem  and the only sign of it would be that BuzzAnimate never opened. So the
rem  prompt happens only when there is a window to read it in; when there is
rem  not, BuzzAnimate.vbs shows the message in a dialog instead, which is why
rem  everything above is written to the console rather than swallowed.
rem ===========================================================================
:halt
if not defined BUZZ_SILENT pause
endlocal
exit /b 1
