//! Direct validation of the LaMa inpainting wiring, independent of
//! `complete_parts`' silent fallback. Requires the LaMa model to be
//! downloaded; `#[ignore]`d like the rest of C1-C4's honest model tests.

use image::{GrayImage, Luma, Rgba, RgbaImage};
use buzz_ai::segment::lama_inpaint_512;

#[test]
#[ignore = "requires downloaded LaMa model"]
fn lama_fills_a_synthetic_hole() {
    let mut image = RgbaImage::new(512, 512);
    for y in 0..512u32 {
        for x in 0..512u32 {
            // A simple gradient so there's real structure to reconstruct.
            let v = ((x + y) % 256) as u8;
            image.put_pixel(x, y, Rgba([v, 255 - v, 128, 255]));
        }
    }

    let mut mask = GrayImage::new(512, 512);
    for y in 200..300u32 {
        for x in 200..300u32 {
            mask.put_pixel(x, y, Luma([255]));
        }
    }

    let out = lama_inpaint_512(&image, &mask)
        .expect("lama_inpaint_512 should not error")
        .expect("LaMa model should be installed for this test");

    assert_eq!(out.width(), 512);
    assert_eq!(out.height(), 512);

    // The hole should be filled with plausible (non-black, non-uniform)
    // content, not left as the zeroed-out input.
    let mut nonzero = 0u32;
    let mut sum = [0u64; 3];
    for y in 200..300u32 {
        for x in 200..300u32 {
            let p = out.get_pixel(x, y);
            if p[0] > 5 || p[1] > 5 || p[2] > 5 {
                nonzero += 1;
            }
            sum[0] += p[0] as u64;
            sum[1] += p[1] as u64;
            sum[2] += p[2] as u64;
        }
    }
    let hole_px = 100 * 100;
    println!(
        "lama hole: {nonzero}/{hole_px} nonzero px, mean rgb=({:.1},{:.1},{:.1})",
        sum[0] as f64 / hole_px as f64,
        sum[1] as f64 / hole_px as f64,
        sum[2] as f64 / hole_px as f64
    );
    assert!(
        nonzero as f64 / hole_px as f64 > 0.9,
        "expected the inpainted hole to be mostly non-black, got {nonzero}/{hole_px}"
    );

    // Outside the hole, LaMa should roughly preserve the input (it's given
    // the true pixels there).
    let orig = image.get_pixel(50, 50);
    let filled = out.get_pixel(50, 50);
    let d = (orig[0] as i32 - filled[0] as i32).abs()
        + (orig[1] as i32 - filled[1] as i32).abs()
        + (orig[2] as i32 - filled[2] as i32).abs();
    println!("lama outside-hole preservation delta at (50,50): {d}");
    assert!(d < 150, "expected outside-hole pixels roughly preserved, delta={d}");
}
