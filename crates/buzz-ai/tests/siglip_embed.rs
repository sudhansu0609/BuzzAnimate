//! Direct validation of SigLIP embedding quality (C10 asset matching).
//! Requires the SigLIP vision encoder to be downloaded; `#[ignore]`d like
//! the rest of C1-C4's honest model tests.

use buzz_ai::siglip::embed;

#[test]
#[ignore = "requires downloaded SigLIP vision encoder"]
fn same_symbol_at_different_scales_embeds_closer_than_a_different_symbol() {
    let a = image::open("tests/data/pujariFront_0.png")
        .expect("open fixture")
        .to_rgba8();
    // "Two renders of the same symbol at different scales": a half-size
    // render of the very same artwork.
    let a_small = image::imageops::resize(
        &a,
        (a.width() / 2).max(1),
        (a.height() / 2).max(1),
        image::imageops::FilterType::Triangle,
    );
    let b = image::open("tests/data/OldmanfrontWalk_0.png")
        .expect("open fixture")
        .to_rgba8();

    let embeddings = embed(&[a, a_small, b]).expect("embed should not error with the model installed");
    assert_eq!(embeddings.len(), 3);

    // Every embedding is already L2-normalised, so a dot product is the
    // cosine similarity directly.
    let cosine = |x: &[f32], y: &[f32]| -> f32 { x.iter().zip(y).map(|(p, q)| p * q).sum() };

    let same_pair = cosine(&embeddings[0], &embeddings[1]);
    let different_pair = cosine(&embeddings[0], &embeddings[2]);
    println!("same-symbol cosine (different scales): {same_pair:.4}");
    println!("different-symbol cosine: {different_pair:.4}");

    assert!(
        same_pair > 0.9,
        "the same symbol at two scales should embed near-identically, got {same_pair}"
    );
    assert!(
        different_pair < same_pair,
        "a different-looking symbol should be less similar than the same symbol was to itself, \
         got {different_pair} (different) vs {same_pair} (same)"
    );
}
