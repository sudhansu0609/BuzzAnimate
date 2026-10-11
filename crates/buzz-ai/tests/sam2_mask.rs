//! Direct validation of the SAM2 encode/decode wiring (transformers.js
//! `sam2.1-hiera-tiny` ONNX layout), independent of `part_masks`' silent
//! fallback. Requires the SAM2 encoder/decoder (+ decoder `.onnx_data`) to be
//! downloaded; `#[ignore]`d like the rest of C1-C4's honest model tests.

use buzz_ai::segment::{sam2_encode, sam2_mask};

#[test]
#[ignore = "requires downloaded SAM2 encoder/decoder models"]
fn sam2_encode_and_decode_a_real_fixture() {
    let png_path = "tests/data/pujariFront_0.png";
    let img = image::open(png_path).expect("open fixture").to_rgba8();

    let embeddings = sam2_encode(&img)
        .expect("sam2_encode should not error")
        .expect("SAM2 encoder should be installed for this test");

    // A point roughly on the character's torso (chest/hips sit around
    // x=180, y=360..500 in this 346x1024 fixture), no box prompt.
    let points = [(180.0f32, 420.0f32, true)];
    let (mask, score) = sam2_mask(&embeddings, &points, None)
        .expect("sam2_mask should not error")
        .expect("SAM2 decoder should be installed for this test");

    assert_eq!(mask.width(), img.width());
    assert_eq!(mask.height(), img.height());
    assert!(score.is_finite(), "iou score should be finite, got {score}");

    let on_count = mask.pixels().filter(|p| p[0] > 128).count();
    let total = (mask.width() * mask.height()) as usize;
    println!("sam2 mask: {on_count}/{total} px on, score={score:.3}");
    assert!(on_count > 0, "SAM2 mask should not be empty");
    assert!(
        on_count < total * 9 / 10,
        "SAM2 mask should not cover almost the whole image (got {on_count}/{total})"
    );

    // A box prompt over the same rough torso region.
    let (mask2, score2) = sam2_mask(&embeddings, &[], Some([110.0, 230.0, 250.0, 520.0]))
        .expect("sam2_mask (box) should not error")
        .expect("SAM2 decoder should be installed for this test");
    let on_count2 = mask2.pixels().filter(|p| p[0] > 128).count();
    println!("sam2 box mask: {on_count2}/{total} px on, score={score2:.3}");
    assert!(on_count2 > 0, "SAM2 box-prompted mask should not be empty");
}
