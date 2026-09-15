Add-Type -AssemblyName System.Drawing

# ---------------------------------------------------------------------------
# The program's mark.
#
# Two drawings, not one. The large sizes carry the onion-skin ghosts behind the
# play triangle — the frames either side, which is what makes this an animation
# program rather than a media player. Below 64 pixels those ghosts turn to mush,
# so the small sizes get the triangle alone, drawn slightly bolder. Downsampling
# one drawing to sixteen pixels is how icons end up looking smudged.
# ---------------------------------------------------------------------------
$ASSETS = "B:\youtubeProjects\Buzzcaf Media\BuzzAnimate\assets"

$orange = [System.Drawing.Color]::FromArgb(255, 0xF8, 0x6A, 0x14)
$mid    = [System.Drawing.Color]::FromArgb(255, 0x9A, 0x7E, 0x8C)
$blue   = [System.Drawing.Color]::FromArgb(255, 0x17, 0x76, 0xD2)

function Triangle($cx, $cy, $size) {
  $h = $size
  $w = $size * 0.88
  $pts = @(
    (New-Object System.Drawing.PointF([float]($cx - $w / 2), [float]($cy - $h / 2))),
    (New-Object System.Drawing.PointF([float]($cx + $w / 2), [float]$cy)),
    (New-Object System.Drawing.PointF([float]($cx - $w / 2), [float]($cy + $h / 2)))
  )
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $p.AddPolygon($pts)
  $p.CloseFigure()
  return $p
}

function Draw($S, $withGhosts) {
  $bmp = New-Object System.Drawing.Bitmap($S, $S)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.Clear([System.Drawing.Color]::Transparent)

  $radius = [int]($S * 0.22)
  $d = [math]::Max(2, $radius * 2)
  $tile = New-Object System.Drawing.Drawing2D.GraphicsPath
  $tile.AddArc(0, 0, $d, $d, 180, 90)
  $tile.AddArc($S - $d, 0, $d, $d, 270, 90)
  $tile.AddArc($S - $d, $S - $d, $d, $d, 0, 90)
  $tile.AddArc(0, $S - $d, $d, $d, 90, 90)
  $tile.CloseFigure()

  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($S, $S)),
    $orange, $blue)
  $blend = New-Object System.Drawing.Drawing2D.ColorBlend(3)
  # The grey sits late and lightly: at the middle it drained the colour out of
  # the whole tile, which at sixteen pixels is all anybody sees.
  $blend.Colors = @($orange, $mid, $blue)
  $blend.Positions = @(0.0, 0.62, 1.0)
  $brush.InterpolationColors = $blend
  $g.FillPath($brush, $tile)

  $solid = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
  if ($withGhosts) {
    $cx = $S * 0.57; $cy = $S * 0.5; $main = $S * 0.40
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(55, 255,255,255))),
                (Triangle ($cx - $S * 0.21) $cy ($main * 0.85)))
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(105, 255,255,255))),
                (Triangle ($cx - $S * 0.105) $cy ($main * 0.92)))
    $g.FillPath($solid, (Triangle $cx $cy $main))
  } else {
    # Alone, bolder, and centred: at this size the shape is the whole message.
    $g.FillPath($solid, (Triangle ($S * 0.53) ($S * 0.5) ($S * 0.52)))
  }

  $g.Dispose()
  return $bmp
}

$detailed = Draw 1024 $true
$detailed.Save((Join-Path $ASSETS "logo-source.png"), [System.Drawing.Imaging.ImageFormat]::Png)

foreach ($size in @(512, 256, 128, 64)) {
  $out = New-Object System.Drawing.Bitmap($size, $size)
  $g2 = [System.Drawing.Graphics]::FromImage($out)
  $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g2.PixelOffsetMode  = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g2.SmoothingMode    = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
  $g2.Clear([System.Drawing.Color]::Transparent)
  $g2.DrawImage($detailed, 0, 0, $size, $size)
  $g2.Dispose()
  $out.Save((Join-Path $ASSETS "logo-$size.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $out.Dispose()
}
$detailed.Dispose()

# The small sizes are drawn at their own size, not reduced.
foreach ($size in @(48, 32, 16)) {
  $small = Draw ($size * 8) $false
  $out = New-Object System.Drawing.Bitmap($size, $size)
  $g2 = [System.Drawing.Graphics]::FromImage($out)
  $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g2.PixelOffsetMode  = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g2.SmoothingMode    = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
  $g2.Clear([System.Drawing.Color]::Transparent)
  $g2.DrawImage($small, 0, 0, $size, $size)
  $g2.Dispose()
  $out.Save((Join-Path $ASSETS "logo-$size.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $out.Dispose(); $small.Dispose()
}

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
Write-Host "wrote the mark, two drawings"
