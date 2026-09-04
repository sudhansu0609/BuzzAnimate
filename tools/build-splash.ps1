<#
    The "something is happening" window.

    WHY THIS EXISTS

    The launcher builds before it starts the editor, and it does that with no
    console (see BuzzAnimate.vbs). When the build is warm that is invisible and
    right - it takes under a second. When it is not, it takes minutes, and with
    the console hidden there was nothing at all on screen: no window, no taskbar
    button, no cursor change. Opening the program and having *nothing* happen for
    two minutes is indistinguishable from it being broken, and that is what this
    was reported as.

    HOW IT STAYS OUT OF THE WAY

    It is started for every launch, but it shows nothing for the first stretch:
    it waits, and if the build has already finished by then it exits having
    never drawn. So the warm case - which is nearly every launch - still puts
    the editor on screen and nothing else, and the window appears only when
    there is genuinely something to wait for.

    It closes when the launcher drops the done-file beside the log. That is the
    signal rather than a process handle because the launcher starts this
    hidden and non-blocking, which gives it no handle to hold.
#>

[CmdletBinding()]
param(
    # The build's output, tailed for the line shown under the bar.
    [Parameter(Mandatory = $true)][string] $Log,
    # Created by the launcher when the build is over. Closing signal.
    [Parameter(Mandatory = $true)][string] $Done,
    # The application icon, if it is where it usually is.
    [string] $Icon = '',
    # How long to wait before showing anything, in milliseconds. A warm build
    # finishes well inside this, so nothing is drawn for it.
    [int] $QuietMs = 1200
)

$ErrorActionPreference = 'Stop'

# Nothing here is worth taking the editor down over: a splash that fails should
# leave a silent build, which is what there was before it, rather than a dialog
# about a splash.
try {
    # --- the quiet stretch ---------------------------------------------------
    $waited = 0
    while ($waited -lt $QuietMs) {
        if (Test-Path -LiteralPath $Done) { exit 0 }
        Start-Sleep -Milliseconds 100
        $waited += 100
    }
    if (Test-Path -LiteralPath $Done) { exit 0 }

    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing

    # **Without this the bar does not move.** A marquee ProgressBar is drawn by
    # the themed common controls; unstyled it falls back to a plain box that
    # sits at zero forever. A frozen progress bar is worse than none — it says
    # the program has hung, which is the exact impression this window exists to
    # correct.
    [System.Windows.Forms.Application]::EnableVisualStyles()

    # The interface's own colours, so this belongs to the same program as the
    # window that follows it. Matches Palette::chrome and the brand band in
    # crates/buzz-ui/src/theme.rs.
    $chrome = [System.Drawing.Color]::FromArgb(0x26, 0x26, 0x26)
    $text = [System.Drawing.Color]::FromArgb(0xD8, 0xD8, 0xD8)
    $dim = [System.Drawing.Color]::FromArgb(0x9A, 0x9A, 0x9A)
    $orange = [System.Drawing.Color]::FromArgb(0xF8, 0x75, 0x1D)
    $grey = [System.Drawing.Color]::FromArgb(0x8A, 0x8F, 0x98)
    $blue = [System.Drawing.Color]::FromArgb(0x1E, 0x7F, 0xD4)

    $form = New-Object System.Windows.Forms.Form
    $form.Text = 'BuzzAnimate'
    $form.FormBorderStyle = 'FixedSingle'
    $form.StartPosition = 'CenterScreen'
    $form.ClientSize = New-Object System.Drawing.Size(460, 150)
    $form.BackColor = $chrome
    $form.TopMost = $true
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ControlBox = $false
    $form.ShowInTaskbar = $true
    if ($Icon -and (Test-Path -LiteralPath $Icon)) {
        try { $form.Icon = New-Object System.Drawing.Icon $Icon } catch { }
    }

    # The brand band across the top, as the editor draws it. Three points, so
    # the orange travels through grey and arrives blue rather than fading
    # straight from one end to the other.
    $band = New-Object System.Windows.Forms.Panel
    $band.Height = 3
    $band.Dock = 'Top'
    $band.Add_Paint({
            param($sender, $e)
            $w = [Math]::Max($sender.Width, 1)
            $half = [Math]::Max([int]($w / 2), 1)
            $left = New-Object System.Drawing.Rectangle 0, 0, $half, 3
            $right = New-Object System.Drawing.Rectangle ($half - 1), 0, ($w - $half + 1), 3
            $b1 = New-Object System.Drawing.Drawing2D.LinearGradientBrush $left, $orange, $grey, 0.0
            $b2 = New-Object System.Drawing.Drawing2D.LinearGradientBrush $right, $grey, $blue, 0.0
            $e.Graphics.FillRectangle($b1, $left)
            $e.Graphics.FillRectangle($b2, $right)
            $b1.Dispose(); $b2.Dispose()
        })

    $title = New-Object System.Windows.Forms.Label
    $title.Text = 'Preparing BuzzAnimate'
    $title.ForeColor = $text
    $title.Font = New-Object System.Drawing.Font 'Segoe UI', 12, ([System.Drawing.FontStyle]::Bold)
    $title.SetBounds(24, 26, 412, 26)

    $note = New-Object System.Windows.Forms.Label
    $note.Text = 'The sources changed, so it is rebuilding. This runs once per change.'
    $note.ForeColor = $dim
    $note.Font = New-Object System.Drawing.Font 'Segoe UI', 8.5
    $note.SetBounds(24, 52, 412, 20)

    $bar = New-Object System.Windows.Forms.ProgressBar
    # Marquee, not a percentage: cargo does not report one, and a bar that
    # invented a number would be lying about how long is left.
    $bar.Style = 'Marquee'
    $bar.MarqueeAnimationSpeed = 25
    $bar.SetBounds(24, 84, 412, 12)

    $status = New-Object System.Windows.Forms.Label
    $status.Text = 'Starting the build...'
    $status.ForeColor = $dim
    $status.Font = New-Object System.Drawing.Font 'Consolas', 8.5
    $status.AutoEllipsis = $true
    $status.SetBounds(24, 106, 412, 20)

    $form.Controls.AddRange(@($title, $note, $bar, $status, $band))

    # What the build is actually doing, read off its log. This is the part that
    # matters: a bar that sweeps proves the *window* is alive, a crate name
    # proves the *build* is.
    #
    # `$script:` throughout, and not for tidiness. A WinForms handler runs in a
    # scope of its own, and a bare `$status` inside it resolved to nothing — so
    # the assignment landed on $null, threw, and was swallowed by the catch. The
    # window came up with a sweeping bar that never said anything, which is
    # half of what it was added to fix.
    $script:log = $Log
    $script:done = $Done
    $script:status = $status
    $script:form = $form

    # The newest line worth showing, or nothing.
    function Get-BuildLine {
        try {
            if (-not (Test-Path -LiteralPath $script:log)) { return $null }
            # Shared read: cargo still has the file open for writing.
            $stream = [System.IO.File]::Open(
                $script:log, [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
            try {
                $reader = New-Object System.IO.StreamReader $stream
                $tail = $reader.ReadToEnd()
            }
            finally { $stream.Dispose() }

            $line = $tail -split "`r?`n" |
            Where-Object { $_ -match '^\s*(Compiling|Building|Finished|Updating|Downloaded)\s' } |
            Select-Object -Last 1
            if ($line) { return $line.Trim() }
        }
        catch { }
        return $null
    }

    # Seeded before the window is shown, so it opens saying something true
    # rather than waiting a tick to catch up.
    $seed = Get-BuildLine
    if ($seed) { $status.Text = $seed }

    $timer = New-Object System.Windows.Forms.Timer
    $timer.Interval = 350
    $timer.Add_Tick({
            if (Test-Path -LiteralPath $script:done) {
                $script:form.Close()
                return
            }
            $line = Get-BuildLine
            if ($line) { $script:status.Text = $line }
        })
    $timer.Start()

    [void]$form.ShowDialog()
    $timer.Stop()
    $form.Dispose()
}
catch {
    exit 0
}
