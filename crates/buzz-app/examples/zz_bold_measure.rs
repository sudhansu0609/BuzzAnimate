//! scratch probe — delete after use (TRACE_PICTURE_PLAN T15).
//! zz_bold_measure <png>...   outline weight and colour measured on pixels, the same way for FLA frames and traces.
//! ink = pixel darker than 0.55 x its 21 px neighbourhood mean and luma < 90; weight = min(horizontal run, vertical run)
//! of ink at the pixel (runs over 14 px are masses, not lines); length-weighted (each pixel counts 1/weight).
use image::RgbaImage;

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

pub fn ink_mask(img: &RgbaImage) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let l: Vec<f32> = img.pixels().map(luma).collect();
    let mut integral = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += l[y * w + x] as f64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let r = 10usize;
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1, y0, y1) = (x.saturating_sub(r), (x + r + 1).min(w), y.saturating_sub(r), (y + r + 1).min(h));
            let sum = integral[y1 * (w + 1) + x1] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0] + integral[y0 * (w + 1) + x0];
            let mean = sum / ((x1 - x0) * (y1 - y0)) as f64;
            l[i] < 90.0 && (l[i] as f64) < 0.55 * mean
        })
        .collect()
}

fn main() {
    for path in std::env::args().skip(1) {
        let img = image::open(&path).unwrap().to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let ink = ink_mask(&img);
        let mut hrun = vec![0u16; w * h];
        let mut vrun = vec![0u16; w * h];
        for y in 0..h {
            let mut x = 0;
            while x < w {
                if !ink[y * w + x] {
                    x += 1;
                    continue;
                }
                let s = x;
                while x < w && ink[y * w + x] {
                    x += 1;
                }
                for k in s..x {
                    hrun[y * w + k] = (x - s) as u16;
                }
            }
        }
        for x in 0..w {
            let mut y = 0;
            while y < h {
                if !ink[y * w + x] {
                    y += 1;
                    continue;
                }
                let s = y;
                while y < h && ink[y * w + x] {
                    y += 1;
                }
                for k in s..y {
                    vrun[k * w + x] = (y - s) as u16;
                }
            }
        }
        let mut ws: Vec<(f64, f64)> = Vec::new();
        let mut cols: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for i in 0..w * h {
            if !ink[i] {
                continue;
            }
            let m = hrun[i].min(vrun[i]) as f64;
            if m > 14.0 || m < 1.0 {
                continue;
            }
            ws.push((m, 1.0 / m));
            let p = img.get_pixel((i % w) as u32, (i / w) as u32);
            for c in 0..3 {
                cols[c].push(p[c]);
            }
        }
        ws.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let total: f64 = ws.iter().map(|x| x.1).sum();
        let pct = |q: f64| {
            let mut acc = 0.0;
            for (m, wt) in &ws {
                acc += wt;
                if acc >= total * q {
                    return *m;
                }
            }
            0.0
        };
        let med = |v: &mut Vec<u8>| {
            v.sort_unstable();
            v.get(v.len() / 2).copied().unwrap_or(0)
        };
        let c = [med(&mut cols[0]), med(&mut cols[1]), med(&mut cols[2])];
        println!(
            "{path}: ink length {:.0} px, weight p10/med/p90 {:.1}/{:.1}/{:.1}, colour #{:02x}{:02x}{:02x}",
            total,
            pct(0.1),
            pct(0.5),
            pct(0.9),
            c[0],
            c[1],
            c[2]
        );
    }
}
