//! Character matte and SAM 2 interactive segmentation.

use anyhow::{Context, Result};
use image::{GrayImage, Luma, Rgba, RgbaImage};
use crate::models::{models_dir, ModelId};
use crate::onnx::{image_to_nchw, pooled, unletterbox_mask, LetterboxInfo, OnnxSession};

/// Extracts or generates a character matte (grayscale alpha mask) for `img`.
///
/// If `img` contains meaningful alpha (any pixel has alpha < 250), the image's
/// own alpha channel is extracted directly.
/// Otherwise, ISNet-anime (`ModelId::AnimeSegmentation`) is executed to generate
/// the segmentation matte.
pub fn matte(img: &RgbaImage, session: Option<&mut OnnxSession>) -> Result<GrayImage> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(GrayImage::new(w, h));
    }

    // 1. Check if the image has genuine transparency
    let has_transparency = img.pixels().any(|p| p[3] < 250);
    if has_transparency {
        let mut gray = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                gray.put_pixel(x, y, Luma([img.get_pixel(x, y)[3]]));
            }
        }
        return Ok(drop_small_specks(&gray, SPECK_AREA_FRACTION));
    }

    // 2. Otherwise run ISNet-anime
    let (nchw, info) = image_to_nchw(
        img,
        (1024, 1024),
        [0.5, 0.5, 0.5],
        [1.0, 1.0, 1.0],
        [0, 0, 0, 255],
    );

    let raw_mask = match session {
        Some(s) => run_isnet(s, &nchw)?,
        None => match pooled(ModelId::AnimeSegmentation) {
            Some(pooled_session) => {
                let mut guard = pooled_session.lock().unwrap();
                run_isnet(&mut guard, &nchw)?
            }
            None => {
                let model_path = models_dir().join("isnetis.onnx");
                if !model_path.exists() {
                    anyhow::bail!(
                        "Anime segmentation model is not installed. Download it first via AI Models dialog."
                    );
                }
                let mut local_session = OnnxSession::load(&model_path)?;
                run_isnet(&mut local_session, &nchw)?
            }
        },
    };

    let unletterboxed = unletterbox_mask(&raw_mask, 1024, 1024, &info);
    let mut gray = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let val = (unletterboxed[(y * w + x) as usize] * 255.0).clamp(0.0, 255.0) as u8;
            gray.put_pixel(x, y, Luma([val]));
        }
    }
    trim_paper_halo(img, &mut gray);

    Ok(drop_small_specks(&gray, SPECK_AREA_FRACTION))
}

/// How far (Euclidean RGB distance) a pixel's colour may be from the paper's and
/// still count as paper in [`trim_paper_halo`].
const PAPER_COLOUR_TOLERANCE: f32 = 26.0;

/// **Takes the paper-coloured halo off a matte**, in place. The segmentation
/// model is generous round a thin, light-on-light shape (a hand, a foot, a wisp of
/// hair on a cream page): its matte then holds a band of plain paper beside the
/// drawing. Inside the character that band was traced as a pale shape with an
/// outline round it (white blobs at the fingertips, a cream shadow by the feet),
/// and the old completion hid some of it by flat-painting over it.
///
/// The paper colour is the median colour of the matte-free pixels along the
/// picture's edge. Every matte pixel (`> 128`) of that colour that touches the
/// outside of the matte, or touches another such pixel that does, is zeroed:
/// a flood in from the outside through paper-coloured pixels only. Anything the
/// drawing encloses (an eye white inside its outline, teeth, a white shirt with
/// an outline) is never reached, and nothing that is not paper-coloured is touched;
/// the flood goes no deeper than 2% of the picture's longer side (a halo is thinner
/// than that), so a gap in a light garment's outline cannot let it eat the garment.
/// Does nothing when there is too little matte-free edge to read the paper from.
pub fn trim_paper_halo(img: &RgbaImage, gray: &mut GrayImage) {
    let (w, h) = (img.width(), img.height());
    if w < 8 || h < 8 || gray.width() != w || gray.height() != h {
        return;
    }
    // The paper: median colour of the outer 4-pixel frame, where the matte is empty
    // (at or under 128, the threshold every consumer cuts the matte at; the model's
    // background is a soft ~0.5, not 0).
    let mut samples: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..h {
        for x in 0..w {
            let on_frame = x < 4 || y < 4 || x + 4 >= w || y + 4 >= h;
            if on_frame && gray.get_pixel(x, y)[0] <= 128 {
                let p = img.get_pixel(x, y);
                for c in 0..3 {
                    samples[c].push(p[c]);
                }
            }
        }
    }
    if samples[0].len() < 100 {
        return;
    }
    let mut paper = [0f32; 3];
    for c in 0..3 {
        samples[c].sort_unstable();
        paper[c] = samples[c][samples[c].len() / 2] as f32;
    }
    let is_paper = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        let d = (0..3).map(|c| (p[c] as f32 - paper[c]).powi(2)).sum::<f32>();
        d <= PAPER_COLOUR_TOLERANCE * PAPER_COLOUR_TOLERANCE
    };

    // Flood in from the outside, breadth first and no deeper than a halo can be:
    // a paper-coloured matte pixel next to an empty one, or next to one already
    // removed, goes. The depth limit keeps a gap in a light garment's outline
    // from letting the flood eat far into the garment.
    let max_depth = ((w.max(h) as f32 * 0.02).round() as u32).max(12);
    let (wi, hi) = (w as i64, h as i64);
    let mut queue: std::collections::VecDeque<(u32, u32, u32)> = std::collections::VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            if gray.get_pixel(x, y)[0] <= 128 {
                queue.push_back((x, y, 0));
            }
        }
    }
    while let Some((x, y, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= wi || ny >= hi {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            if gray.get_pixel(nx, ny)[0] > 128 && is_paper(nx, ny) {
                gray.put_pixel(nx, ny, Luma([0]));
                queue.push_back((nx, ny, depth + 1));
            }
        }
    }
}

/// A connected component smaller than this fraction of the matte's own
/// largest component is a speck — dust, anti-aliasing noise, a stray
/// mis-thresholded pixel cluster — not real content, and `drop_small_specks`
/// removes it. Real disconnected content (a separately-drawn hand, a held
/// prop with a gap to the body) is almost always a meaningful fraction of
/// the character's own size and survives this; see `character::part_masks`
/// for what keeps a *surviving* disconnected piece from being glued to the
/// wrong bone instead.
const SPECK_AREA_FRACTION: f64 = 0.001;

/// Zeroes every foreground pixel (> 128) belonging to a connected component
/// smaller than `min_fraction` of the matte's own largest component — noise
/// specks, not real content.
///
/// This deliberately does **not** drop every other disconnected component:
/// an earlier version kept only the single largest component outright,
/// which silently deleted real, separately-drawn artwork with a gap to the
/// body (a detached hand, a held prop) along with genuine specks. A
/// disconnected-but-substantial piece is real content and is left in the
/// matte; `character::part_masks` is where it is kept from being glued to
/// whichever bone happens to be nearest, and `character::assemble_character`
/// is where it is kept as its own unbound "Prop" layer instead of being
/// discarded. Values inside every kept component are left exactly as they
/// were (soft/antialiased edges are not touched); this only ever *removes*
/// pixels, never adds or hardens any.
pub fn drop_small_specks(gray: &GrayImage, min_fraction: f64) -> GrayImage {
    let (w, h) = (gray.width(), gray.height());
    let raw: Vec<u8> = gray.pixels().map(|p| p.0[0]).collect();
    let components = connected_components(&raw, w, h);
    let Some(largest) = components.iter().map(|c| c.len()).max() else {
        return gray.clone();
    };
    let min_size = (((largest as f64) * min_fraction).ceil() as usize).max(1);

    let mut drop = vec![false; raw.len()];
    for comp in &components {
        if comp.len() < min_size {
            for &idx in comp {
                drop[idx] = true;
            }
        }
    }

    let mut out = gray.clone();
    for (idx, px) in out.pixels_mut().enumerate() {
        if drop[idx] {
            px.0[0] = 0;
        }
    }
    out
}

/// Every 4-connected foreground (`mask > 128`) component, as lists of flat
/// pixel indices (`y * w + x`) — the general form `largest_component` and
/// `drop_small_specks` are both built from, and what `character::part_masks`
/// uses to find every piece of the matte disconnected from the character's
/// own main silhouette.
pub fn connected_components(mask: &[u8], w: u32, h: u32) -> Vec<Vec<usize>> {
    let (wi, hi) = (w as i32, h as i32);
    let mut visited = vec![false; mask.len()];
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut stack: Vec<(i32, i32)> = Vec::new();
    for y in 0..hi {
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            if mask[idx] <= 128 || visited[idx] {
                continue;
            }
            let mut comp = Vec::new();
            visited[idx] = true;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                let cidx = (cy * wi + cx) as usize;
                comp.push(cidx);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                        let nidx = (ny * wi + nx) as usize;
                        if !visited[nidx] && mask[nidx] > 128 {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            out.push(comp);
        }
    }
    out
}

/// Runs ISNet-anime inference on a pre-letterboxed 1024x1024 NCHW buffer,
/// returning a sigmoid-activated 1024x1024 mask.
fn run_isnet(sess: &mut OnnxSession, nchw: &[f32]) -> Result<Vec<f32>> {
    let input_array = ndarray::Array4::from_shape_vec((1, 3, 1024, 1024), nchw.to_vec())
        .context("Failed to reshape image into (1, 3, 1024, 1024)")?;

    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| anyhow::anyhow!("Failed to create tensor: {e}"))?;

    let outputs = sess.inner_mut().run(ort::inputs![input_tensor])
        .map_err(|e| anyhow::anyhow!("ISNet inference failed: {e}"))?;

    let (_shape, slice) = outputs[0].try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract ISNet output: {e}"))?;

    let mut raw_mask = vec![0.0f32; 1024 * 1024];
    for (i, &v) in slice.iter().take(1024 * 1024).enumerate() {
        raw_mask[i] = 1.0 / (1.0 + (-v).exp());
    }
    Ok(raw_mask)
}

/// Computes the Euclidean distance transform of a binary/grayscale matte.
///
/// For each pixel inside the matte (value > 128), computes the distance
/// in pixels to the nearest background pixel (value <= 128).
/// Returns a row-major `Vec<f32>` of size `width * height`.
pub fn distance_transform(matte: &GrayImage) -> Vec<f32> {
    let (w, h) = (matte.width(), matte.height());
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let mut dist = vec![0.0f32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if matte.get_pixel(x, y)[0] <= 128 {
                dist[idx] = 0.0;
            } else {
                dist[idx] = 1e6;
            }
        }
    }

    // Forward pass
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            let mut d = dist[idx];
            if x > 0 {
                d = d.min(dist[idx - 1] + 1.0);
            }
            if y > 0 {
                d = d.min(dist[idx - w as usize] + 1.0);
            }
            if x > 0 && y > 0 {
                d = d.min(dist[idx - w as usize - 1] + 1.4142);
            }
            if x + 1 < w && y > 0 {
                d = d.min(dist[idx - w as usize + 1] + 1.4142);
            }
            dist[idx] = d;
        }
    }

    // Backward pass
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let idx = (y * w + x) as usize;
            let mut d = dist[idx];
            if x + 1 < w {
                d = d.min(dist[idx + 1] + 1.0);
            }
            if y + 1 < h {
                d = d.min(dist[idx + w as usize] + 1.0);
            }
            if x + 1 < w && y + 1 < h {
                d = d.min(dist[idx + w as usize + 1] + 1.4142);
            }
            if x > 0 && y + 1 < h {
                d = d.min(dist[idx + w as usize - 1] + 1.4142);
            }
            dist[idx] = d;
        }
    }

    dist
}

/// Binary dilation of a 0/255 mask by a disc of radius `r` pixels.
/// Restricted to the mask's own bounding box (padded by `r`) for speed.
pub fn dilate(mask: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    morph_disc(mask, w, h, r, true)
}

/// Binary erosion of a 0/255 mask by a disc of radius `r` pixels.
pub fn erode(mask: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    morph_disc(mask, w, h, r, false)
}

fn mask_bbox(mask: &[u8], w: u32, h: u32) -> Option<(i32, i32, i32, i32)> {
    let (wi, hi) = (w as i32, h as i32);
    let (mut minx, mut miny, mut maxx, mut maxy) = (wi, hi, -1i32, -1i32);
    for y in 0..hi {
        for x in 0..wi {
            if mask[(y * wi + x) as usize] > 128 {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    if maxx < 0 { None } else { Some((minx, miny, maxx, maxy)) }
}

fn morph_disc(mask: &[u8], w: u32, h: u32, r: i32, dilating: bool) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    let mut out = vec![0u8; mask.len()];
    if r <= 0 {
        out.copy_from_slice(mask);
        return out;
    }
    let Some((minx, miny, maxx, maxy)) = mask_bbox(mask, w, h) else {
        return out;
    };
    let r2 = r * r;
    let (x0, y0) = ((minx - r).max(0), (miny - r).max(0));
    let (x1, y1) = ((maxx + r).min(wi - 1), (maxy + r).min(hi - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let mut result = !dilating;
            'scan: for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r2 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    let set = nx >= 0 && ny >= 0 && nx < wi && ny < hi && mask[(ny * wi + nx) as usize] > 128;
                    if dilating && set {
                        result = true;
                        break 'scan;
                    }
                    if !dilating && !set {
                        result = false;
                        break 'scan;
                    }
                }
            }
            out[(y * wi + x) as usize] = if result { 255 } else { 0 };
        }
    }
    out
}

/// Morphological close: dilate then erode by `r`, closing small gaps and
/// speckle holes without changing the mask's overall extent.
pub fn morph_close(mask: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    erode(&dilate(mask, w, h, r), w, h, r)
}

/// Morphological open: erode then dilate by `r`, removing small speckle
/// blobs and thin spurs without changing the mask's overall extent.
pub fn morph_open(mask: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    dilate(&erode(mask, w, h, r), w, h, r)
}

/// Keeps only the largest 4-connected component of a 0/255 mask, zeroing
/// every other component. An empty mask is returned unchanged.
pub fn largest_component(mask: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    let mut visited = vec![false; mask.len()];
    let mut best: Vec<usize> = Vec::new();
    let mut stack: Vec<(i32, i32)> = Vec::new();
    for y in 0..hi {
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            if mask[idx] <= 128 || visited[idx] {
                continue;
            }
            let mut comp = Vec::new();
            visited[idx] = true;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                let cidx = (cy * wi + cx) as usize;
                comp.push(cidx);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                        let nidx = (ny * wi + nx) as usize;
                        if !visited[nidx] && mask[nidx] > 128 {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if comp.len() > best.len() {
                best = comp;
            }
        }
    }
    let mut out = vec![0u8; mask.len()];
    for idx in best {
        out[idx] = 255;
    }
    out
}

/// Fills interior background holes (4-connected regions of `mask <= 128`
/// not reachable from the buffer's border) whose area is smaller than
/// `max_frac` of the mask's own foreground pixel count.
pub fn fill_small_holes(mask: &[u8], w: u32, h: u32, max_frac: f32) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    if wi == 0 || hi == 0 {
        return mask.to_vec();
    }
    let fg_area: usize = mask.iter().filter(|&&v| v > 128).count();
    if fg_area == 0 {
        return mask.to_vec();
    }
    let max_hole = ((fg_area as f32) * max_frac).max(1.0) as usize;

    // Flood the background from the border, so anything left unreached is
    // an interior hole.
    let mut reached = vec![false; mask.len()];
    let mut stack: Vec<(i32, i32)> = Vec::new();
    let seed = |x: i32, y: i32, reached: &mut Vec<bool>, stack: &mut Vec<(i32, i32)>| {
        let idx = (y * wi + x) as usize;
        if mask[idx] <= 128 && !reached[idx] {
            reached[idx] = true;
            stack.push((x, y));
        }
    };
    for x in 0..wi {
        seed(x, 0, &mut reached, &mut stack);
        seed(x, hi - 1, &mut reached, &mut stack);
    }
    for y in 0..hi {
        seed(0, y, &mut reached, &mut stack);
        seed(wi - 1, y, &mut reached, &mut stack);
    }
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                let nidx = (ny * wi + nx) as usize;
                if !reached[nidx] && mask[nidx] <= 128 {
                    reached[nidx] = true;
                    stack.push((nx, ny));
                }
            }
        }
    }

    let mut out = mask.to_vec();
    let mut visited = vec![false; mask.len()];
    for y in 0..hi {
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            if mask[idx] > 128 || reached[idx] || visited[idx] {
                continue;
            }
            let mut comp = vec![idx];
            visited[idx] = true;
            let mut st = vec![(x, y)];
            while let Some((cx, cy)) = st.pop() {
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                        let nidx = (ny * wi + nx) as usize;
                        if !visited[nidx] && !reached[nidx] && mask[nidx] <= 128 {
                            visited[nidx] = true;
                            comp.push(nidx);
                            st.push((nx, ny));
                        }
                    }
                }
            }
            if comp.len() <= max_hole {
                for idx in comp {
                    out[idx] = 255;
                }
            }
        }
    }
    out
}

/// Cleans a raw part mask: morphological close (r=3) then open (r=2), keeps
/// the largest connected component, then fills interior holes smaller than
/// 2% of the part's own area. Used to de-speckle fragmented SAM2 masks
/// (LOCAL_AI_PLAN.md "Opus review of the quality run", item 2).
pub fn clean_part_mask(mask: &[u8], w: u32, h: u32) -> Vec<u8> {
    let closed = morph_close(mask, w, h, 3);
    let opened = morph_open(&closed, w, h, 2);
    let largest = largest_component(&opened, w, h);
    fill_small_holes(&largest, w, h, 0.02)
}

/// A multi-label nearest-neighbour assignment ("Voronoi via a distance
/// transform"): every pixel where `matte > 128` and `labels[idx] < 0` is
/// assigned the label of whichever *seeded* pixel (where `labels[idx] >= 0`
/// on entry) is nearest, using the same two-pass chamfer distance transform
/// as [`distance_transform`]. Seeded labels are left unchanged. Pixels
/// outside the matte are left at `-1`.
pub fn voronoi_labels(labels: &mut [i32], matte: &GrayImage, w: u32, h: u32) {
    let (wi, hi) = (w as i32, h as i32);
    let mut dist = vec![f32::MAX; labels.len()];
    for i in 0..labels.len() {
        if labels[i] >= 0 {
            dist[i] = 0.0;
        }
    }
    let step = |idx: usize, cand: usize, add: f32, dist: &mut [f32], labels: &mut [i32]| {
        if labels[cand] < 0 || labels[idx] < -1 {
            return;
        }
        let d = dist[cand] + add;
        if d < dist[idx] {
            dist[idx] = d;
            labels[idx] = labels[cand];
        }
    };
    // Forward pass
    for y in 0..hi {
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            if x > 0 {
                step(idx, idx - 1, 1.0, &mut dist, labels);
            }
            if y > 0 {
                step(idx, idx - wi as usize, 1.0, &mut dist, labels);
            }
            if x > 0 && y > 0 {
                step(idx, idx - wi as usize - 1, 1.4142, &mut dist, labels);
            }
            if x + 1 < wi && y > 0 {
                step(idx, idx - wi as usize + 1, 1.4142, &mut dist, labels);
            }
        }
    }
    // Backward pass
    for y in (0..hi).rev() {
        for x in (0..wi).rev() {
            let idx = (y * wi + x) as usize;
            if x + 1 < wi {
                step(idx, idx + 1, 1.0, &mut dist, labels);
            }
            if y + 1 < hi {
                step(idx, idx + wi as usize, 1.0, &mut dist, labels);
            }
            if x + 1 < wi && y + 1 < hi {
                step(idx, idx + wi as usize + 1, 1.4142, &mut dist, labels);
            }
            if x > 0 && y + 1 < hi {
                step(idx, idx + wi as usize - 1, 1.4142, &mut dist, labels);
            }
        }
    }
    // Clear any pixel outside the matte that got seeded by the passes above
    // (shouldn't happen since we only start from seeded pixels, kept for
    // safety/clarity).
    for y in 0..hi {
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            if matte.get_pixel(x as u32, y as u32)[0] <= 128 {
                labels[idx] = -1;
            }
        }
    }
}

/// Thins a 0/255 binary image to a 1px-wide skeleton using the Zhang-Suen
/// algorithm.
pub fn zhang_suen_thin(mask: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    let mut img: Vec<u8> = mask.iter().map(|&v| if v > 128 { 1 } else { 0 }).collect();
    let at = |img: &[u8], x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= wi || y >= hi { 0 } else { img[(y * wi + x) as usize] }
    };
    loop {
        let mut changed = false;
        for sub_iter in 0..2 {
            let mut to_clear = Vec::new();
            for y in 0..hi {
                for x in 0..wi {
                    if at(&img, x, y) == 0 {
                        continue;
                    }
                    let p2 = at(&img, x, y - 1);
                    let p3 = at(&img, x + 1, y - 1);
                    let p4 = at(&img, x + 1, y);
                    let p5 = at(&img, x + 1, y + 1);
                    let p6 = at(&img, x, y + 1);
                    let p7 = at(&img, x - 1, y + 1);
                    let p8 = at(&img, x - 1, y);
                    let p9 = at(&img, x - 1, y - 1);
                    let neighbours = [p2, p3, p4, p5, p6, p7, p8, p9];
                    let b: u32 = neighbours.iter().map(|&v| v as u32).sum();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let seq = [p2, p3, p4, p5, p6, p7, p8, p9, p2];
                    let a = seq.windows(2).filter(|w| w[0] == 0 && w[1] == 1).count();
                    if a != 1 {
                        continue;
                    }
                    let cond = if sub_iter == 0 {
                        p2 * p4 * p6 == 0 && p4 * p6 * p8 == 0
                    } else {
                        p2 * p4 * p8 == 0 && p2 * p6 * p8 == 0
                    };
                    if cond {
                        to_clear.push((x, y));
                    }
                }
            }
            if !to_clear.is_empty() {
                changed = true;
                for (x, y) in to_clear {
                    img[(y * wi + x) as usize] = 0;
                }
            }
        }
        if !changed {
            break;
        }
    }
    img.into_iter().map(|v| if v > 0 { 255 } else { 0 }).collect()
}

/// Image embeddings produced by the SAM 2 vision encoder once per image, so
/// several `sam2_mask` decode calls (one per part) can reuse them.
pub struct Sam2Embeddings {
    e0: Vec<f32>,
    e1: Vec<f32>,
    e2: Vec<f32>,
    e0_shape: [usize; 3],
    e1_shape: [usize; 3],
    e2_shape: [usize; 3],
    info: LetterboxInfo,
}

fn last3(dims: &[i64]) -> [usize; 3] {
    let n = dims.len();
    let at = |k: usize| dims.get(n.wrapping_sub(k)).copied().unwrap_or(0).max(0) as usize;
    [at(3), at(2), at(1)]
}

/// Encodes `img` with the SAM 2.1 hiera-tiny vision encoder (the
/// transformers.js ONNX layout: input `pixel_values` [1,3,1024,1024],
/// outputs `image_embeddings.{0,1,2}`). Returns `None` when the encoder
/// model isn't installed, so callers can fall back to a non-SAM path.
pub fn sam2_encode(img: &RgbaImage) -> Result<Option<Sam2Embeddings>> {
    let Some(session) = pooled(ModelId::Sam2Encoder) else {
        return Ok(None);
    };
    let mut sess = session.lock().unwrap();

    let (nchw, info) = image_to_nchw(
        img,
        (1024, 1024),
        [0.485, 0.456, 0.406],
        [0.229, 0.224, 0.225],
        [0, 0, 0, 255],
    );
    let input_array = ndarray::Array4::from_shape_vec((1, 3, 1024, 1024), nchw)
        .context("Failed to reshape image for SAM2 encoder")?;
    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 encoder tensor: {e}"))?;

    let outputs = sess
        .inner_mut()
        .run(ort::inputs!["pixel_values" => input_tensor])
        .map_err(|e| anyhow::anyhow!("SAM2 encoder inference failed: {e}"))?;

    let (shape0, slice0) = outputs["image_embeddings.0"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract image_embeddings.0: {e}"))?;
    let e0_shape = last3(shape0.as_ref());
    let e0 = slice0.to_vec();

    let (shape1, slice1) = outputs["image_embeddings.1"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract image_embeddings.1: {e}"))?;
    let e1_shape = last3(shape1.as_ref());
    let e1 = slice1.to_vec();

    let (shape2, slice2) = outputs["image_embeddings.2"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract image_embeddings.2: {e}"))?;
    let e2_shape = last3(shape2.as_ref());
    let e2 = slice2.to_vec();

    Ok(Some(Sam2Embeddings {
        e0,
        e1,
        e2,
        e0_shape,
        e1_shape,
        e2_shape,
        info,
    }))
}

/// A single SAM2 point prompt in original-image pixel coordinates:
/// `(x, y, is_positive)`.
pub type Sam2Point = (f32, f32, bool);

/// Decodes a SAM 2 mask from previously-computed `embeddings`, given point
/// and (optional) box prompts in original-image pixel coordinates.
///
/// Returns the highest-IoU-scoring mask as a full original-resolution
/// probability mask (0..255) plus its predicted IoU score, or `None` when
/// the decoder model isn't installed.
pub fn sam2_mask(
    embeddings: &Sam2Embeddings,
    points: &[Sam2Point],
    box_prompt: Option<[f32; 4]>,
) -> Result<Option<(GrayImage, f32)>> {
    let Some(session) = pooled(ModelId::Sam2Decoder) else {
        return Ok(None);
    };
    let mut sess = session.lock().unwrap();

    // -1 is SAM's "padding point, ignore me" label, used when there are no
    // point prompts and only a box is given (the tensor may never be empty).
    let n = points.len().max(1);
    let mut pts = vec![0.0f32; n * 2];
    let mut labels = vec![-1i64; n];
    for (i, &(x, y, positive)) in points.iter().enumerate() {
        let (lx, ly) = embeddings.info.letterbox_point(x, y);
        pts[i * 2] = lx;
        pts[i * 2 + 1] = ly;
        labels[i] = if positive { 1 } else { 0 };
    }

    let points_array = ndarray::Array4::from_shape_vec((1, 1, n, 2), pts)
        .context("Failed to reshape SAM2 points")?;
    let labels_array = ndarray::Array3::from_shape_vec((1, 1, n), labels)
        .context("Failed to reshape SAM2 labels")?;

    let (num_boxes, boxes_vec) = match box_prompt {
        Some([x0, y0, x1, y1]) => {
            let (lx0, ly0) = embeddings.info.letterbox_point(x0, y0);
            let (lx1, ly1) = embeddings.info.letterbox_point(x1, y1);
            (1usize, vec![lx0, ly0, lx1, ly1])
        }
        None => (0usize, Vec::new()),
    };
    let boxes_array = ndarray::Array3::from_shape_vec((1, num_boxes, 4), boxes_vec)
        .context("Failed to reshape SAM2 boxes")?;

    let e0 = ndarray::Array4::from_shape_vec(
        (1, embeddings.e0_shape[0], embeddings.e0_shape[1], embeddings.e0_shape[2]),
        embeddings.e0.clone(),
    )
    .context("Failed to reshape SAM2 embeddings.0")?;
    let e1 = ndarray::Array4::from_shape_vec(
        (1, embeddings.e1_shape[0], embeddings.e1_shape[1], embeddings.e1_shape[2]),
        embeddings.e1.clone(),
    )
    .context("Failed to reshape SAM2 embeddings.1")?;
    let e2 = ndarray::Array4::from_shape_vec(
        (1, embeddings.e2_shape[0], embeddings.e2_shape[1], embeddings.e2_shape[2]),
        embeddings.e2.clone(),
    )
    .context("Failed to reshape SAM2 embeddings.2")?;

    let points_tensor = ort::value::Tensor::from_array(points_array)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 points tensor: {e}"))?;
    let labels_tensor = ort::value::Tensor::from_array(labels_array)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 labels tensor: {e}"))?;
    let boxes_tensor = ort::value::Tensor::from_array(boxes_array)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 boxes tensor: {e}"))?;
    let e0_tensor = ort::value::Tensor::from_array(e0)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 embeddings.0 tensor: {e}"))?;
    let e1_tensor = ort::value::Tensor::from_array(e1)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 embeddings.1 tensor: {e}"))?;
    let e2_tensor = ort::value::Tensor::from_array(e2)
        .map_err(|e| anyhow::anyhow!("Failed to create SAM2 embeddings.2 tensor: {e}"))?;

    let outputs = sess
        .inner_mut()
        .run(ort::inputs![
            "input_points" => points_tensor,
            "input_labels" => labels_tensor,
            "input_boxes" => boxes_tensor,
            "image_embeddings.0" => e0_tensor,
            "image_embeddings.1" => e1_tensor,
            "image_embeddings.2" => e2_tensor,
        ])
        .map_err(|e| anyhow::anyhow!("SAM2 decoder inference failed: {e}"))?;

    let (iou_shape, iou_slice) = outputs["iou_scores"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract iou_scores: {e}"))?;
    let (mask_shape, mask_slice) = outputs["pred_masks"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract pred_masks: {e}"))?;

    let dims: Vec<usize> = mask_shape.as_ref().iter().map(|&d| d.max(0) as usize).collect();
    if dims.len() < 5 {
        anyhow::bail!("unexpected SAM2 pred_masks shape {:?}", dims);
    }
    let (num_masks, mh, mw) = (dims[2], dims[3], dims[4]);
    let iou_dims: Vec<usize> = iou_shape.as_ref().iter().map(|&d| d.max(0) as usize).collect();
    let iou_masks_per_prompt = iou_dims.get(2).copied().unwrap_or(num_masks).max(1);

    // Pick the best-scoring mask for prompt 0 (our single point/box group).
    let mut best_mask_idx = 0usize;
    let mut best_score = f32::NEG_INFINITY;
    for m in 0..num_masks.min(iou_masks_per_prompt) {
        let score = iou_slice[m];
        if score > best_score {
            best_score = score;
            best_mask_idx = m;
        }
    }

    let mask_stride = mh * mw;
    let mask_offset = best_mask_idx * mask_stride;
    let logits = &mask_slice[mask_offset..mask_offset + mask_stride];
    let mut probs = vec![0.0f32; mask_stride];
    for (i, &v) in logits.iter().enumerate() {
        probs[i] = 1.0 / (1.0 + (-v).exp());
    }

    // `pred_masks` sits at the encoder's 1024x1024 letterboxed scale,
    // downsampled by mw/1024 (typically 4x, to 256x256). Scale a copy of the
    // image's LetterboxInfo down to that grid, then reuse `unletterbox_mask`.
    let sx = mw as f32 / embeddings.info.target_width.max(1) as f32;
    let sy = mh as f32 / embeddings.info.target_height.max(1) as f32;
    let mask_info = LetterboxInfo {
        orig_width: embeddings.info.orig_width,
        orig_height: embeddings.info.orig_height,
        target_width: mw as u32,
        target_height: mh as u32,
        scale: embeddings.info.scale * sx,
        pad_x: embeddings.info.pad_x * sx,
        pad_y: embeddings.info.pad_y * sy,
    };
    let unletterboxed = unletterbox_mask(&probs, mw as u32, mh as u32, &mask_info);

    let (ow, oh) = (embeddings.info.orig_width, embeddings.info.orig_height);
    let mut out = GrayImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let v = (unletterboxed[(y * ow + x) as usize] * 255.0).clamp(0.0, 255.0) as u8;
            out.put_pixel(x, y, Luma([v]));
        }
    }

    Ok(Some((out, best_score)))
}

/// Runs LaMa inpainting on an already-512x512 image/mask pair (the model's
/// fixed I/O size). `mask` marks the region to inpaint: pixels `> 128` are
/// holes. Masked pixels are zeroed in the image input before inference (the
/// LaMa reference `image * (1 - mask)` convention). Returns `None` when the
/// model isn't installed.
pub fn lama_inpaint_512(image: &RgbaImage, mask: &GrayImage) -> Result<Option<RgbaImage>> {
    if image.width() != 512 || image.height() != 512 || mask.width() != 512 || mask.height() != 512 {
        anyhow::bail!(
            "lama_inpaint_512 requires 512x512 inputs, got image {}x{} mask {}x{}",
            image.width(),
            image.height(),
            mask.width(),
            mask.height()
        );
    }
    let Some(session) = pooled(ModelId::LaMa) else {
        return Ok(None);
    };

    let n = 512usize * 512;
    let mut img_data = vec![0.0f32; 3 * n];
    let mut mask_data = vec![0.0f32; n];
    for y in 0..512u32 {
        for x in 0..512u32 {
            let idx = (y * 512 + x) as usize;
            let m = if mask.get_pixel(x, y)[0] > 128 { 1.0f32 } else { 0.0 };
            mask_data[idx] = m;
            let p = image.get_pixel(x, y);
            let keep = 1.0 - m;
            img_data[idx] = (p[0] as f32 / 255.0) * keep;
            img_data[n + idx] = (p[1] as f32 / 255.0) * keep;
            img_data[2 * n + idx] = (p[2] as f32 / 255.0) * keep;
        }
    }

    let run_result = {
        let mut sess = session.lock().unwrap();
        run_lama(&mut sess, &img_data, &mask_data)
    };

    let slice = match run_result {
        Ok(v) => v,
        Err(e) => {
            // Some execution providers (DirectML, observed on this machine)
            // commit LaMa's FFC/spectral-transform blocks fine but fail
            // during `run()`. That is a runtime incompatibility `load()`'s
            // commit-time fallback can't catch, so retry once on a
            // CPU-only session and, if it works, replace the pooled one so
            // later calls skip the failing provider.
            tracing::warn!("LaMa inference failed, retrying on CPU: {e}");
            let entry = crate::models::get_entry(ModelId::LaMa);
            let path = models_dir().join(entry.filename);
            let mut cpu_session = OnnxSession::load_cpu(&path)?;
            let slice = run_lama(&mut cpu_session, &img_data, &mask_data)?;
            crate::onnx::replace_pooled(ModelId::LaMa, cpu_session);
            slice
        }
    };

    // The exported range varies by conversion (0..1 vs 0..255); detect it
    // from the actual output rather than assuming.
    let max_v = slice.iter().cloned().fold(f32::MIN, f32::max);
    let scale = if max_v > 1.5 { 1.0 } else { 255.0 };

    let mut out = RgbaImage::new(512, 512);
    for y in 0..512u32 {
        for x in 0..512u32 {
            let idx = (y * 512 + x) as usize;
            let r = (slice[idx] * scale).clamp(0.0, 255.0) as u8;
            let g = (slice[n + idx] * scale).clamp(0.0, 255.0) as u8;
            let b = (slice[2 * n + idx] * scale).clamp(0.0, 255.0) as u8;
            out.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }

    Ok(Some(out))
}

/// Runs a single LaMa inference given pre-built NCHW image/mask buffers,
/// returning the flat `output` tensor (3*512*512, CHW).
fn run_lama(sess: &mut OnnxSession, img_data: &[f32], mask_data: &[f32]) -> Result<Vec<f32>> {
    let img_array = ndarray::Array4::from_shape_vec((1, 3, 512, 512), img_data.to_vec())
        .context("Failed to reshape LaMa image input")?;
    let mask_array = ndarray::Array4::from_shape_vec((1, 1, 512, 512), mask_data.to_vec())
        .context("Failed to reshape LaMa mask input")?;
    let img_tensor = ort::value::Tensor::from_array(img_array)
        .map_err(|e| anyhow::anyhow!("Failed to create LaMa image tensor: {e}"))?;
    let mask_tensor = ort::value::Tensor::from_array(mask_array)
        .map_err(|e| anyhow::anyhow!("Failed to create LaMa mask tensor: {e}"))?;

    let outputs = sess
        .inner_mut()
        .run(ort::inputs!["image" => img_tensor, "mask" => mask_tensor])
        .map_err(|e| anyhow::anyhow!("LaMa inference failed: {e}"))?;

    let (_shape, slice) = outputs["output"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract LaMa output: {e}"))?;
    Ok(slice.to_vec())
}

#[cfg(test)]
mod paper_halo_tests {
    use super::*;

    /// A cream page, a skin-coloured rectangle with a dark outline, a paper-white
    /// patch enclosed inside it (an eye white), and a matte that is generous: the
    /// rectangle plus a 12 px band of plain paper all round.
    fn drawing() -> (RgbaImage, GrayImage) {
        let (w, h) = (240u32, 200u32);
        let paper = Rgba([248, 245, 235, 255]);
        let mut img = RgbaImage::from_pixel(w, h, paper);
        let mut matte = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // The rectangle spans 80..160 x 70..130; the matte reaches 12 px past it.
                if (68..172).contains(&x) && (58..142).contains(&y) {
                    matte.put_pixel(x, y, Luma([255]));
                }
                if (80..160).contains(&x) && (70..130).contains(&y) {
                    let edge = x < 82 || x >= 158 || y < 72 || y >= 128;
                    img.put_pixel(x, y, if edge { Rgba([30, 20, 15, 255]) } else { Rgba([220, 160, 110, 255]) });
                }
                // The enclosed paper-white patch.
                if (110..130).contains(&x) && (90..105).contains(&y) {
                    img.put_pixel(x, y, paper);
                }
            }
        }
        (img, matte)
    }

    /// The paper-coloured band between the drawing and the matte's edge is taken
    /// off; the drawing, its outline and a paper-white patch it encloses are not.
    #[test]
    fn the_paper_coloured_halo_goes_and_the_drawing_and_what_it_encloses_stay() {
        let (img, mut matte) = drawing();
        trim_paper_halo(&img, &mut matte);
        for (x, y) in [(70, 100), (100, 60), (165, 135), (150, 140)] {
            assert_eq!(matte.get_pixel(x, y)[0], 0, "the halo pixel ({x},{y}) must be taken off");
        }
        for (x, y) in [(81, 100), (100, 71), (120, 80), (157, 100)] {
            assert_eq!(matte.get_pixel(x, y)[0], 255, "the drawing at ({x},{y}) must stay");
        }
        assert_eq!(matte.get_pixel(120, 97)[0], 255, "paper-white enclosed by the outline must stay");
    }

    /// The flood is limited in depth: a light garment with a gap in its outline
    /// is not eaten far in.
    #[test]
    fn the_flood_does_not_eat_deep_into_a_paper_coloured_region() {
        let (w, h) = (400u32, 100u32);
        let paper = Rgba([248, 245, 235, 255]);
        let img = RgbaImage::from_pixel(w, h, paper);
        let mut matte = GrayImage::new(w, h);
        for y in 0..h {
            for x in 100..300 {
                matte.put_pixel(x, y, Luma([255]));
            }
        }
        trim_paper_halo(&img, &mut matte);
        // 2% of 400 = 8, so the minimum 12 px: the matte keeps its core.
        assert_eq!(matte.get_pixel(105, 50)[0], 0, "within the depth limit it goes");
        assert_eq!(matte.get_pixel(130, 50)[0], 255, "past the depth limit it stays");
        assert_eq!(matte.get_pixel(200, 50)[0], 255, "deep inside it stays");
    }

    /// With no matte-free edge to read the paper from, nothing is touched.
    #[test]
    fn nothing_is_trimmed_without_a_paper_to_read() {
        let img = RgbaImage::from_pixel(100, 100, Rgba([248, 245, 235, 255]));
        let mut matte = GrayImage::from_pixel(100, 100, Luma([255]));
        trim_paper_halo(&img, &mut matte);
        assert!(matte.pixels().all(|p| p[0] == 255));
    }
}
