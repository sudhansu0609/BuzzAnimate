//! scratch probe — delete after use (TRACE_PICTURE_PLAN T15).
//! zz_bold_sheet <out.png> <file:x,y,w,h:scale>...   crops tiled left to right (nearest), 8 px grey gap.
use image::{Rgba, RgbaImage, imageops};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut tiles: Vec<RgbaImage> = Vec::new();
    for spec in &a[1..] {
        let mut it = spec.rsplitn(3, ':');
        let scale: u32 = it.next().unwrap().parse().unwrap();
        let rect: Vec<u32> = it.next().unwrap().split(',').map(|v| v.parse().unwrap()).collect();
        let file = it.next().unwrap();
        let img = image::open(file).unwrap().to_rgba8();
        let (x, y) = (rect[0].min(img.width() - 1), rect[1].min(img.height() - 1));
        let (w, h) = (rect[2].min(img.width() - x), rect[3].min(img.height() - y));
        let crop = imageops::crop_imm(&img, x, y, w, h).to_image();
        tiles.push(imageops::resize(&crop, w * scale, h * scale, imageops::FilterType::Nearest));
    }
    let total_w: u32 = tiles.iter().map(|t| t.width()).sum::<u32>() + 8 * (tiles.len() as u32 - 1);
    let max_h = tiles.iter().map(|t| t.height()).max().unwrap();
    let mut out = RgbaImage::from_pixel(total_w, max_h, Rgba([128, 128, 128, 255]));
    let mut x = 0;
    for t in &tiles {
        imageops::overlay(&mut out, t, x as i64, 0);
        x += t.width() + 8;
    }
    out.save(&a[0]).unwrap();
}
