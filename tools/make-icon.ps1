Add-Type -AssemblyName System.Drawing

# ---------------------------------------------------------------------------
# The icon: the character's head, with BA lettered under it.
#
# Composed at every output size rather than drawn once and reduced. The letters
# are the reason: two characters shrunk from 512 pixels to 32 turn to grey
# smudge, while the same two drawn *at* 32 keep their stems. The head is scaled
# from the artwork either way, since it is a photograph of a drawing and has no
# small version of itself.
# ---------------------------------------------------------------------------
$REPO   = "B:\youtubeProjects\Buzzcaf Media\BuzzAnimate"
$ASSETS = Join-Path $REPO "assets"
$SRC    = "B:\youtubeProjects\Khayal3Baje\Effects\THUMB1.png"

$src = [System.Drawing.Bitmap]::FromFile($SRC)
$bg  = $src.GetPixel(8, 8)

# The head, and no lettering: see make_brand4 for why this crop.
$hx = 535; $hy = 140; $hw = 170; $hh = 235

# A darker cut of the show's orange for the band, so white letters have
# something to sit against.
$band = [System.Drawing.Color]::FromArgb(255,
  [int]($bg.R * 0.62), [int]($bg.G * 0.55), [int]($bg.B * 0.45))

$family = "Arial Black"
try { $test = New-Object System.Drawing.FontFamily($family) ; $test.Dispose() }
catch { $family = "Arial" }

function Compose($S) {
  $bmp = New-Object System.Drawing.Bitmap($S, $S)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g.PixelOffsetMode   = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
  $g.Clear($bg)

  # At sixteen pixels there is room for one thing, and it is the letters: a
  # head and a caption both shrunk to eight pixels each are two smudges, while
  # BA alone still reads. The title bar and file lists draw this size.
  if ($S -le 16) {
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = [System.Drawing.StringAlignment]::Center
    $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
    # Arial rather than Arial Black here, and fitted by measurement: the heavy
    # face is wide enough that "BA" ran off the tile and the A was lost.
    $pt = [float]($S * 0.80)
    for ($i = 0; $i -lt 16; $i++) {
      $f = New-Object System.Drawing.Font("Arial", $pt,
            [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
      if ($g.MeasureString("BA", $f).Width -le $S * 1.02) { break }
      $f.Dispose(); $pt = $pt * 0.9
    }
    $g.DrawString("BA", $f, [System.Drawing.Brushes]::White,
      (New-Object System.Drawing.RectangleF(0, 0, $S, $S)), $fmt)
    $f.Dispose(); $fmt.Dispose(); $g.Dispose()
    return $bmp
  }

  # The head, sitting in the top three-quarters.
  $bandH = [math]::Max(5, [int]($S * 0.30))
  $room  = $S - $bandH
  $scale = [math]::Min(($S * 0.94) / $hw, ($room * 1.02) / $hh)
  $dw = [int]($hw * $scale); $dh = [int]($hh * $scale)
  $dx = [int](($S - $dw) / 2); $dy = [int]($room * 0.5 - $dh * 0.5)
  $g.DrawImage($src, (New-Object System.Drawing.Rectangle($dx, $dy, $dw, $dh)),
               (New-Object System.Drawing.Rectangle($hx, $hy, $hw, $hh)),
               [System.Drawing.GraphicsUnit]::Pixel)

  # The band, and BA across it.
  $g.FillRectangle((New-Object System.Drawing.SolidBrush $band),
                   0, ($S - $bandH), $S, $bandH)

  # Sized by measurement, not by guess: "BA" in a heavy face is wider than it
  # is tall, so fitting it by height alone runs it off both edges.
  $text = "BA"
  $size = [float]($bandH * 0.86)
  for ($i = 0; $i -lt 12; $i++) {
    $f = New-Object System.Drawing.Font($family, $size, [System.Drawing.FontStyle]::Bold,
                                        [System.Drawing.GraphicsUnit]::Pixel)
    $m = $g.MeasureString($text, $f)
    if ($m.Width -le $S * 0.80 -and $m.Height -le $bandH * 1.18) { break }
    $f.Dispose()
    $size = $size * 0.92
  }
  $fmt = New-Object System.Drawing.StringFormat
  $fmt.Alignment = [System.Drawing.StringAlignment]::Center
  $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
  $rect = New-Object System.Drawing.RectangleF(0, ($S - $bandH), $S, $bandH)
  $g.DrawString($text, $f, [System.Drawing.Brushes]::White, $rect, $fmt)
  $f.Dispose(); $fmt.Dispose()

  $g.Dispose()
  return $bmp
}

$master = Compose 512
$master.Save((Join-Path $ASSETS "logo-source.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$master.Save((Join-Path $ASSETS "logo-512.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$master.Dispose()

foreach ($s in @(256, 128, 64, 48, 32, 16)) {
  $bmp = Compose $s
  $bmp.Save((Join-Path $ASSETS "logo-$s.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}
$src.Dispose()

$icoSizes = @(256, 128, 64, 48, 32, 16)
$pngs = @()
foreach ($s in $icoSizes) { $pngs += ,([System.IO.File]::ReadAllBytes((Join-Path $ASSETS "logo-$s.png"))) }
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$icoSizes.Count)
$offset = 6 + 16 * $icoSizes.Count
for ($i = 0; $i -lt $icoSizes.Count; $i++) {
  $s = $icoSizes[$i]
  $bw.Write([Byte]($(if ($s -ge 256) { 0 } else { $s })))
  $bw.Write([Byte]($(if ($s -ge 256) { 0 } else { $s })))
  $bw.Write([Byte]0); $bw.Write([Byte]0)
  $bw.Write([UInt16]1); $bw.Write([UInt16]32)
  $bw.Write([UInt32]$pngs[$i].Length); $bw.Write([UInt32]$offset)
  $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $bw.Write($p) }
$bw.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $ASSETS "buzzanimate.ico"), $ms.ToArray())
$bw.Dispose(); $ms.Dispose()
Write-Host "wrote the lettered icon set, face: $family"
