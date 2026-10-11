#!/usr/bin/env python3
"""Cut BuzzAnimate's tool icons out of the user's 7x4 icon sheet.

The source is a single PNG, a 7-column by 4-row grid of icons rendered over a
smoothly blurred, continuously-varying backdrop (there is no flat colour key
to chroma against — the "background" drifts across the whole sheet). Each
cell holds one icon in its upper portion and a caption label beneath it.

Grid (row-major, matching the sheet's own labels):

    Brush, Pencil, Eraser, Fill, Eyedropper, Airbrush, Smudge
    Select, Lasso, Magic Select, Crop, Transform, Rotate, Flip
    Shape, Line, Rectangle, Ellipse, Polygon, Text, Gradient
    Onion Skin, Play, Previous Frame, Next Frame, Add Frame, Delete Frame,
        Duplicate Frame

# How the backdrop is knocked out

A flat chroma key cannot work here: the backdrop is not flat, it is a blur
that drifts across the whole sheet, so cell (0, 0)'s background colour is
nothing like cell (3, 6)'s — and every icon also casts its own soft glow into
the blur immediately around it, which a *per-cell* background guess (say,
sampling the cell's own corners) reads as more icon.

Two whole-sheet, low-pass estimates of "what is here if there were no icon"
are built once, up front, and every cell slices out of both:

- A **median filter** (a wide one — see `MEDIAN_SIZE`): robust to a thin
  bright shape sitting in the window, so it does not get fooled by a hollow
  icon's own ring into thinking the hollow *inside* is part of the icon. This
  is what decides, per pixel, transparent-or-not.
- A **Gaussian blur** (see `BLUR_RADIUS`): smooth rather than robust, so it
  tracks the *gradual* brightening around an icon's glow the way the median
  filter's sharper transitions cannot. This is only used to estimate the
  colour a partially-transparent edge pixel would be if the backdrop's own
  contribution were subtracted back out (see "decontaminate" below) — using
  it for the *transparent-or-not* decision itself is what let a large hollow
  icon's ring leak brightness into its own centre and paint a faint false
  smear there instead of a clean hole.

For each icon's crop:

1. Each pixel's distance (Euclidean over R/G/B — both luma and chroma move a
   blurred backdrop) from the **median** estimate at that pixel becomes its
   alpha, ramped smoothly between `LOW_THRESHOLD` (certainly background) and
   `HIGH_THRESHOLD` (certainly icon) rather than a hard cut.
2. Partially-transparent pixels are **decontaminated**: such a pixel is
   itself a blend of icon colour and backdrop colour, and leaving it as-is
   would carry a fringe of the old backdrop onto whatever this icon sits on
   next. Un-blended with the matting equation run in reverse, against the
   **blur** estimate: `fg = bg + (observed - bg) / alpha`.
3. A named handful of icons (`SOLID_INTERIOR`) are meant to be a solid flat
   shape, not a ring — largely uniform in the middle, exactly what a local
   filter mistakes for backdrop no matter which one is used. For those, any
   patch of "background" pixels that a flood fill starting from the crop's
   own border cannot reach is not background at all — it is enclosed by the
   icon, so it is forced fully opaque instead.

Then the icon is trimmed to its own alpha bounding box, padded back out to a
square (transparent margin, so nothing touches the edge), and resampled down
to 64x64 and 32x32 with the alpha premultiplied first — resampling straight
RGB+A separately drags backdrop-coloured fringes back in at the exact edges
this script exists to clean up.

# The icon-versus-caption line

The icon (plus its glow) sits in the upper part of each cell; the caption
sits under it. Both the icon's own size and how close its caption sits
beneath it vary from cell to cell (this sheet was not laid out on a fixed
grid of icon-then-caption offsets), so one fraction of the cell's height does
not separate the two everywhere: measured by hand against one cell per row,
row 1's icons keep drips and sprays running well down the cell while row 3's
simple flat glyphs leave their caption starting much earlier. `ROW_ICON_FRACTION`
records that per row rather than pretending one number fits all 28 cells.

The caption is also not always fully *inside* its own row's nominal band: a
caption sits close enough to the row boundary that the bottom of its own
lettering can cross into the next row down (the sheet's rows are 256 px
apart, but a caption's own text is taller than the gap left for it in at
least one row). Left alone, this shows up as a ghost of the previous row's
caption sitting *above* the next row's own icon, once that icon's real
background is knocked out and the caption's is not. `ROW_TOP_MARGIN` skips
the first few pixels of every row but the first for exactly this reason —
comfortably below where a spillover caption ends, comfortably above where
that row's own icon starts.

# Usage

    python tools/extract_icon_sheet.py
    python tools/extract_icon_sheet.py --sheet path/to/sheet.png --out-dir crates/buzz-ui/assets/icons

Writes `crates/buzz-ui/assets/icons/64/<name>.png` and `.../32/<name>.png`
for every one of the 28 cells, plus two contact sheets (over a dark and a
light background) to `--contact-dir` (default `target/icon-extraction/`) so
the cut can actually be looked at rather than trusted on faith.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

GRID_COLS = 7
GRID_ROWS = 4

# Row-major, left to right, top to bottom — exactly the sheet's own labels,
# lower-cased and underscored.
ICON_NAMES = [
    ["brush", "pencil", "eraser", "fill", "eyedropper", "airbrush", "smudge"],
    ["select", "lasso", "magic_select", "crop", "transform", "rotate", "flip"],
    ["shape", "line", "rectangle", "ellipse", "polygon", "text", "gradient"],
    [
        "onion_skin",
        "play",
        "previous_frame",
        "next_frame",
        "add_frame",
        "delete_frame",
        "duplicate_frame",
    ],
]

# How far down each row's cells the icon (plus its glow) runs before the
# caption starts — see the module docstring's last section. Measured by eye,
# with gridlines, against one cell per row and checked against its neighbours.
ROW_ICON_FRACTION = [0.88, 0.80, 0.74, 0.76]

# Skipped at the top of every row but the first — see the module docstring's
# note on caption spillover. 10 px clears the one spillover actually measured
# (7 px, row 0 into row 1) with room to spare, while staying well short of
# `onion_skin`'s own icon, the closest any row's icon comes to its own top
# edge.
ROW_TOP_MARGIN = 10

# Icons meant to be a solid flat shape rather than a ring or a line drawing —
# see the module docstring, item 3. Named explicitly rather than guessed at:
# nothing about a pixel's own neighbourhood can tell "a hollow icon's own
# hole" apart from "a solid icon's interior mistaken for background", so this
# is the one piece of this script that relies on having looked at the sheet.
SOLID_INTERIOR = {"gradient"}

# A handful of icons are a broad, soft, near-white glow — or, for the whole
# of row 4, a fairly large flat-filled shape — sitting on a patch of sheet
# that is *already* close to that tone. `MEDIAN_SIZE`'s window ends up
# sampling mostly the icon's own fill there and reporting it right back as
# "the background" (row 4's icons came out as hollow outlines of themselves
# under the median-based alpha, the same failure `SOLID_INTERIOR` exists for,
# just too washed-out for even a flood fill to find a wall to stop at). These
# use the wider-reaching Gaussian blur for the transparent-or-not distance as
# well as for decontamination instead — the blur's wider reach still finds
# enough real backdrop past the icon to tell the two apart. Everywhere else
# the blur alone is too easily fooled by a hollow icon's ring (see the
# docstring).
BLUR_ALPHA_ICONS = {
    "onion_skin",
    "smudge",
    "airbrush",
    "play",
    "previous_frame",
    "next_frame",
    "add_frame",
    "delete_frame",
    "duplicate_frame",
}

# A few icons need their own icon/caption split rather than their row's
# default — see the module docstring's last section. Found by measuring each
# one's own row-by-row alpha profile (`tools/`'s own scratch work, not kept)
# rather than eyeballing a gridline over the source image, which is how the
# row defaults above were first picked and which turned out to disagree with
# these often enough to be worth naming individually: `fill`'s drip and
# `airbrush`/`smudge`'s spray fade out far later in the cell than their row's
# other icons, with a clean run of true-zero rows before their caption
# starts. Row 4's own icons have no such clean gap — their caption's own soft
# glow starts fading in well before its lettering is legible to eye, close
# enough behind the icon's own glow that no fraction threads between the two
# cleanly, so these six give up a little of the icon's own trailing glow
# (cut right where its solid fill ends, before the glow tail that turned out
# to already be part of the caption) rather than carry any caption at all.
ICON_FRACTION_OVERRIDES = {
    "fill": 0.92,
    "airbrush": 0.95,
    "smudge": 0.95,
    "onion_skin": 0.58,
    "play": 0.52,
    "previous_frame": 0.52,
    "next_frame": 0.52,
    "add_frame": 0.52,
    "delete_frame": 0.52,
    "duplicate_frame": 0.52,
}

# The whole-sheet low-pass estimates backdrop is built from — see the module
# docstring's second section.
MEDIAN_SIZE = 31
BLUR_RADIUS = 45

# Below LOW_THRESHOLD: certainly backdrop, alpha 0. Above HIGH_THRESHOLD:
# certainly icon, alpha 255. Chosen by looking at the contact sheets this
# script itself writes — see its docstring.
LOW_THRESHOLD = 16.0
HIGH_THRESHOLD = 50.0

# Alpha below this does not count as "content" when trimming to a bounding
# box, or as reachable background for SOLID_INTERIOR's flood fill — otherwise
# a single stray antialiased pixel keeps the whole cell, or blocks the fill.
ALPHA_TRIM_EPSILON = 10
POCKET_ALPHA_THRESHOLD = 0.5

# Transparent margin left around the trimmed icon once it is padded square,
# as a fraction of the square's side, so nothing touches the button's edge.
PAD_FRACTION = 0.08

OUTPUT_SIZES = (64, 32)


def cell_edges(total: int, count: int) -> list[int]:
    """`count + 1` pixel boundaries splitting `total` pixels into `count`
    (nearly) equal cells — nearly, because 1536 / 7 is not an integer."""
    return [round(i * total / count) for i in range(count + 1)]


def matte(cell: np.ndarray, alpha_bg: np.ndarray, blur_bg: np.ndarray) -> np.ndarray:
    """`cell`, `alpha_bg`, `blur_bg`: matching (H, W, 3) float64 crops of the
    sheet, the backdrop estimate the transparent-or-not decision is made
    against (median, ordinarily — see `BLUR_ALPHA_ICONS`), and the blurred
    estimate. Returns RGBA (H, W, 4) float64: the backdrop knocked out to
    transparency with a soft edge, edges decontaminated against the smooth
    (blurred) estimate."""
    distance = np.linalg.norm(cell - alpha_bg, axis=-1)
    alpha = np.clip(
        (distance - LOW_THRESHOLD) / (HIGH_THRESHOLD - LOW_THRESHOLD), 0.0, 1.0
    )

    alpha_safe = np.maximum(alpha, 1e-3)[..., None]
    foreground = blur_bg + (cell - blur_bg) / alpha_safe
    foreground = np.clip(foreground, 0.0, 255.0)

    out = np.empty((*cell.shape[:2], 4), dtype=np.float64)
    out[..., :3] = foreground
    out[..., 3] = alpha * 255.0
    return out


def fill_enclosed_pockets(rgba: np.ndarray, cell: np.ndarray) -> np.ndarray:
    """For `SOLID_INTERIOR` icons: any "background" (low-alpha) pixel that a
    flood fill starting from the crop's own border cannot reach is not
    background — nothing outside the icon is disconnected from the crop's
    edge in a 28-icon sheet with generous margins — so force it opaque,
    coloured from the untouched sheet pixel rather than the (wrong, computed
    against a background it turns out not to be) decontaminated colour."""
    candidate = rgba[..., 3] < POCKET_ALPHA_THRESHOLD * 255.0
    reachable = np.zeros_like(candidate)
    reachable[0, :] = candidate[0, :]
    reachable[-1, :] = candidate[-1, :]
    reachable[:, 0] = candidate[:, 0]
    reachable[:, -1] = candidate[:, -1]

    # Grows one pixel per pass, 4-connected; a pass that adds nothing means
    # every reachable pixel has been found.
    for _ in range(max(rgba.shape[:2])):
        grown = reachable.copy()
        for shift in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            grown |= np.roll(reachable, shift=shift, axis=(0, 1))
        grown &= candidate
        if np.array_equal(grown, reachable):
            break
        reachable = grown

    pocket = candidate & ~reachable
    out = rgba.copy()
    out[pocket, :3] = cell[pocket]
    out[pocket, 3] = 255.0
    return out


def trim_to_content(rgba: np.ndarray) -> np.ndarray:
    """Crop to the bounding box of pixels with real alpha. Falls back to the
    whole image if nothing cleared the threshold (a cell this script's
    thresholds mismatched, rather than a crash)."""
    mask = rgba[..., 3] > ALPHA_TRIM_EPSILON
    if not mask.any():
        return rgba
    rows = np.where(mask.any(axis=1))[0]
    cols = np.where(mask.any(axis=0))[0]
    return rgba[rows[0] : rows[-1] + 1, cols[0] : cols[-1] + 1]


def pad_to_square(rgba: np.ndarray) -> np.ndarray:
    """Centre `rgba` on a transparent square canvas, side = the longer edge
    plus `PAD_FRACTION` of margin all round."""
    h, w = rgba.shape[:2]
    content_side = max(h, w)
    side = int(round(content_side * (1.0 + 2.0 * PAD_FRACTION)))
    canvas = np.zeros((side, side, 4), dtype=np.float64)
    y0 = (side - h) // 2
    x0 = (side - w) // 2
    canvas[y0 : y0 + h, x0 : x0 + w] = rgba
    return canvas


def resize_premultiplied(rgba: np.ndarray, size: int) -> Image.Image:
    """Downsample with the alpha premultiplied in, so a dark or light
    backdrop fringe cannot survive at the new edge — straight RGBA resizing
    blends transparent-but-still-backdrop-tinted pixels straight into the
    result."""
    alpha = rgba[..., 3:4] / 255.0
    premultiplied = rgba[..., :3] * alpha

    premul_img = Image.fromarray(premultiplied.astype(np.uint8))
    alpha_img = Image.fromarray((alpha[..., 0] * 255.0).astype(np.uint8))

    premul_resized = np.asarray(
        premul_img.resize((size, size), Image.LANCZOS), dtype=np.float64
    )
    alpha_resized = np.asarray(
        alpha_img.resize((size, size), Image.LANCZOS), dtype=np.float64
    )

    alpha_safe = np.maximum(alpha_resized, 1.0)
    rgb = np.clip(premul_resized / (alpha_safe[..., None] / 255.0), 0.0, 255.0)

    out = np.empty((size, size, 4), dtype=np.uint8)
    out[..., :3] = rgb.astype(np.uint8)
    out[..., 3] = alpha_resized.astype(np.uint8)
    return Image.fromarray(out)


def extract_all(sheet: Image.Image) -> dict[str, Image.Image]:
    """Every cell, matted and trimmed but not yet resized — keyed by name,
    used both to write the full-resolution outputs and to build the contact
    sheets."""
    rgb = sheet.convert("RGB")
    median_bg = rgb.filter(ImageFilter.MedianFilter(MEDIAN_SIZE))
    blur_bg = rgb.filter(ImageFilter.GaussianBlur(BLUR_RADIUS))

    arr = np.asarray(rgb, dtype=np.float64)
    median_arr = np.asarray(median_bg, dtype=np.float64)
    blur_arr = np.asarray(blur_bg, dtype=np.float64)

    height, width = arr.shape[:2]
    xs = cell_edges(width, GRID_COLS)
    ys = cell_edges(height, GRID_ROWS)

    icons: dict[str, Image.Image] = {}
    for row in range(GRID_ROWS):
        for col in range(GRID_COLS):
            name = ICON_NAMES[row][col]
            fraction = ICON_FRACTION_OVERRIDES.get(name, ROW_ICON_FRACTION[row])

            x0, x1 = xs[col], xs[col + 1]
            y0, y1 = ys[row], ys[row + 1]
            icon_top = y0 if row == 0 else y0 + ROW_TOP_MARGIN
            icon_bottom = y0 + int(round((y1 - y0) * fraction))

            cell = arr[icon_top:icon_bottom, x0:x1]
            blur_crop = blur_arr[icon_top:icon_bottom, x0:x1]
            alpha_bg = (
                blur_crop
                if name in BLUR_ALPHA_ICONS
                else median_arr[icon_top:icon_bottom, x0:x1]
            )
            rgba = matte(cell, alpha_bg, blur_crop)

            if name in SOLID_INTERIOR:
                rgba = fill_enclosed_pockets(rgba, cell)

            rgba = trim_to_content(rgba)
            rgba = pad_to_square(rgba)
            icons[name] = Image.fromarray(rgba.astype(np.uint8))
    return icons


def write_outputs(icons: dict[str, Image.Image], out_dir: Path) -> None:
    for size in OUTPUT_SIZES:
        size_dir = out_dir / str(size)
        size_dir.mkdir(parents=True, exist_ok=True)
        for name, full in icons.items():
            resize_premultiplied(np.asarray(full, dtype=np.float64), size).save(
                size_dir / f"{name}.png"
            )


def contact_sheet(
    icons: dict[str, Image.Image], background: tuple[int, int, int]
) -> Image.Image:
    """Every extracted icon, at a fixed cell size, over a flat `background` —
    the only honest way to check a cutout: look at it on a colour it was
    never blurred against."""
    cell = 140
    cols = GRID_COLS
    rows = GRID_ROWS
    sheet = Image.new("RGB", (cols * cell, rows * cell), background)
    names = [n for row in ICON_NAMES for n in row]
    for i, name in enumerate(names):
        icon = icons[name]
        side = int(cell * 0.8)
        thumb = icon.resize((side, side), Image.LANCZOS)
        x = (i % cols) * cell + (cell - side) // 2
        y = (i // cols) * cell + (cell - side) // 2
        sheet.paste(thumb, (x, y), thumb)
    return sheet


def main() -> None:
    repo_root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sheet",
        type=Path,
        default=repo_root / "ChatGPT Image Sep 26, 2026, 10_06_09 AM.png",
        help="the source icon sheet",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=repo_root / "crates" / "buzz-ui" / "assets" / "icons",
        help="where 64/<name>.png and 32/<name>.png are written",
    )
    parser.add_argument(
        "--contact-dir",
        type=Path,
        default=repo_root / "target" / "icon-extraction",
        help="where the dark/light contact sheets are written",
    )
    args = parser.parse_args()

    sheet = Image.open(args.sheet)
    icons = extract_all(sheet)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    write_outputs(icons, args.out_dir)

    args.contact_dir.mkdir(parents=True, exist_ok=True)
    contact_sheet(icons, (24, 24, 24)).save(args.contact_dir / "contact_dark.png")
    contact_sheet(icons, (240, 240, 240)).save(args.contact_dir / "contact_light.png")

    print(f"wrote {len(icons)} icons x {len(OUTPUT_SIZES)} sizes to {args.out_dir}")
    print(f"wrote contact sheets to {args.contact_dir}")


if __name__ == "__main__":
    main()
