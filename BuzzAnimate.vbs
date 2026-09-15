' ===========================================================================
'  BuzzAnimate launcher, with no console window.
'
'  WHY THIS FILE EXISTS
'
'  Starting the editor used to put two windows on screen: BuzzAnimate itself,
'  and the black console the .bat runs in. The console is not decoration - it
'  is where `cargo build` reports, and the launcher genuinely needs somewhere
'  to build - but it is not something anyone opening a drawing program should
'  have to look at, and it steals focus from the editor as it goes.
'
'  Windows has no way to start a batch file without a console; `start /min`
'  and a minimised shortcut only move it, and it still flashes up and takes the
'  foreground. WScript.Shell.Run with a window style of 0 is the one thing that
'  actually suppresses it, and running a .vbs needs no console of its own.
'
'  So: this runs the .bat hidden, waits for it, and if it fails, shows what it
'  said in a dialog. Nothing is swallowed - a build error is still a build
'  error and still tells you which one; it just arrives in a box rather than in
'  a window that closed before you could read it.
'
'  BuzzAnimate.bat is unchanged for people who want the console: double-click
'  it, or run it from a terminal, and the build scrolls past as it always did.
'  Only the desktop shortcut comes through here.
' ===========================================================================

Option Explicit

Dim shell, fso, here, bat, logFile, doneFile, splash, icon, cmd, args, i, code, message

Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

here = fso.GetParentFolderName(WScript.ScriptFullName)
bat = fso.BuildPath(here, "BuzzAnimate.bat")

If Not fso.FileExists(bat) Then
    MsgBox "BuzzAnimate.bat is not next to this launcher." & vbCrLf & vbCrLf & _
           "Expected it at:" & vbCrLf & bat, vbExclamation, "BuzzAnimate"
    WScript.Quit 1
End If

' Somewhere to catch what the hidden console says, so a failure can be shown
' rather than lost. The temp folder, not the project: this runs from wherever
' BuzzAnimate is installed, and that may be read-only.
logFile = fso.BuildPath(fso.GetSpecialFolder(2), "buzzanimate-launch.log")

' Anything dropped on the shortcut - a .buzz file, --gpu, --script - is passed
' straight through, quoted, so paths with spaces survive.
args = ""
For i = 0 To WScript.Arguments.Count - 1
    args = args & " " & """" & WScript.Arguments(i) & """"
Next

' Tells the batch file not to `pause` on failure. There is no console to press
' a key in, so a prompt would hang the launcher forever with nothing on screen.
shell.Environment("PROCESS")("BUZZ_SILENT") = "1"

' --- something on screen while the build runs -----------------------------
'
'  Hiding the console fixed the black window nobody wanted and created a worse
'  problem: when the sources have changed, the build takes minutes, and with
'  nothing on screen at all - no window, no taskbar button - opening the
'  program looks exactly like opening a program that is broken. That is what
'  it was reported as.
'
'  tools\build-splash.ps1 is the answer. It shows a small window with a sweeping
'  bar and the crate cargo is on, read from the same log the failure dialog
'  below uses. It is started for *every* launch and draws nothing for the first
'  stretch, so the warm case - which is nearly every launch - still puts the
'  editor on screen and nothing else.
'
'  The done-file is how it is told to close. The splash is started hidden and
'  non-blocking, which leaves no handle to terminate, and a file both sides can
'  see needs no handle.
doneFile = logFile & ".done"
On Error Resume Next
If fso.FileExists(doneFile) Then fso.DeleteFile doneFile, True
On Error GoTo 0

splash = fso.BuildPath(here, "tools\build-splash.ps1")
icon = fso.BuildPath(here, "assets\buzzanimate.ico")
If fso.FileExists(splash) Then
    ' -WindowStyle Hidden and a window style of 0 between them keep
    ' PowerShell's own console off the screen; the form it draws is the only
    ' thing that appears. False = do not wait, or the build would never start.
    shell.Run "powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden" & _
              " -File """ & splash & """ -Log """ & logFile & """" & _
              " -Done """ & doneFile & """ -Icon """ & icon & """", 0, False
End If

' cmd /c "" ... "" is the quoting cmd.exe wants when the command itself is
' quoted: the outer pair is stripped, the inner ones do the work.
cmd = "cmd /c """"" & bat & """" & args & " > """ & logFile & """ 2>&1"""

' 0 = no window, True = wait. The wait is short once the build is warm: the
' batch hands the editor over with `start` and returns immediately.
code = shell.Run(cmd, 0, True)

' The build is over, whether it worked or not. Told before the failure dialog
' below, so the splash is never left sweeping behind a message box.
On Error Resume Next
fso.CreateTextFile(doneFile, True).WriteLine "done"
On Error GoTo 0

If code <> 0 Then
    message = ""
    On Error Resume Next
    If fso.FileExists(logFile) Then
        message = fso.OpenTextFile(logFile, 1).ReadAll()
    End If
    On Error GoTo 0
    If Trim(message) = "" Then
        message = "The launcher exited with code " & code & " and said nothing."
    End If
    MsgBox "BuzzAnimate could not start." & vbCrLf & vbCrLf & message & vbCrLf & _
           "Full output: " & logFile, vbExclamation, "BuzzAnimate"
    WScript.Quit code
End If
