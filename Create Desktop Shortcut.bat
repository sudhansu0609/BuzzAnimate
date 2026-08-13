@echo off
rem ===========================================================================
rem  Put a BuzzAnimate shortcut on the desktop.
rem
rem  It points at BuzzAnimate.bat rather than at the binary, so the shortcut
rem  keeps working after a rebuild, after a `cargo clean`, and after switching
rem  between the release and debug builds.
rem ===========================================================================

setlocal
cd /d "%~dp0"

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$shell = New-Object -ComObject WScript.Shell;" ^
  "$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Desktop')) 'BuzzAnimate.lnk'));" ^
  "$link.TargetPath = (Join-Path '%CD%' 'BuzzAnimate.bat');" ^
  "$link.WorkingDirectory = '%CD%';" ^
  "$link.Description = 'BuzzAnimate';" ^
  "$link.WindowStyle = 7;" ^
  "$link.Save();" ^
  "Write-Host ('Shortcut created: ' + (Join-Path ([Environment]::GetFolderPath('Desktop')) 'BuzzAnimate.lnk'))"

if errorlevel 1 (
    echo.
    echo   Could not create the shortcut.
    echo.
)
pause
endlocal
