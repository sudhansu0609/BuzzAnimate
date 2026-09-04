@echo off
rem ===========================================================================
rem  Put a BuzzAnimate shortcut on the desktop.
rem
rem  It points at BuzzAnimate.vbs rather than at the binary, so the shortcut
rem  keeps working after a rebuild, after a `cargo clean`, and after switching
rem  between the release and debug builds.
rem
rem  **At the .vbs, not the .bat.** A shortcut to the batch file opened two
rem  windows - the editor, and the black console the batch runs in, which takes
rem  the foreground on the way past. Windows cannot start a batch file without
rem  a console; the .vbs is the one wrapper that genuinely suppresses it, and
rem  it still shows a build failure, in a dialog. See the notes in that file.
rem
rem  WindowStyle is 1 (normal) rather than 7 (minimised) for the same reason:
rem  7 was there to keep the console out of the way, there is no console now,
rem  and a *minimised* style used to be passed through to the program itself.
rem ===========================================================================

setlocal
cd /d "%~dp0"

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$shell = New-Object -ComObject WScript.Shell;" ^
  "$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Desktop')) 'BuzzAnimate.lnk'));" ^
  "$link.TargetPath = (Join-Path $env:SystemRoot 'System32\wscript.exe');" ^
  "$link.Arguments = ('\"' + (Join-Path '%CD%' 'BuzzAnimate.vbs') + '\"');" ^
  "$link.WorkingDirectory = '%CD%';" ^
  "$link.Description = 'BuzzAnimate';" ^
  "$icon = (Join-Path '%CD%' 'assets\buzzanimate.ico');" ^
  "if (Test-Path $icon) { $link.IconLocation = $icon }" ^
  "$link.WindowStyle = 1;" ^
  "$link.Save();" ^
  "Write-Host ('Shortcut created: ' + (Join-Path ([Environment]::GetFolderPath('Desktop')) 'BuzzAnimate.lnk'))"

if errorlevel 1 (
    echo.
    echo   Could not create the shortcut.
    echo.
)
pause
endlocal
