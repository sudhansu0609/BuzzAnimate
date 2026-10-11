//! Downloads (verifying SHA-256 via `ensure()`) and loads every C1-C4 ONNX
//! model, then prints its real input/output names, shapes, and the
//! execution provider (DirectML/CPU) actually used.
//!
//! Usage:
//!   cargo run --release -p buzz-app --example ai_models_io

use anyhow::Result;
use buzz_ai::models::{ensure, models_dir, ModelId};
use buzz_ai::onnx::OnnxSession;

/// Models needed for C1-C4 (excludes SigLip, which is C10, later).
const C1_C4_MODELS: &[ModelId] = &[
    ModelId::AnimeSegmentation,
    ModelId::DwPoseDetector,
    ModelId::DwPoseEstimator,
    ModelId::Sam2Encoder,
    ModelId::Sam2Decoder,
    ModelId::LaMa,
];

fn main() -> Result<()> {
    println!("Models directory: {:?}", models_dir());

    for &id in C1_C4_MODELS {
        println!("\n=== {} ({:?}) ===", id.name(), id);
        let t0 = std::time::Instant::now();
        let path = match ensure(id, |downloaded, total| {
            if total > 0 && downloaded % (32 * 1024 * 1024) < 65536 {
                println!("  downloading... {downloaded}/{total} bytes");
            }
        }, || false) {
            Ok(p) => p,
            Err(e) => {
                println!("  DOWNLOAD FAILED: {e}");
                continue;
            }
        };
        let download_ms = t0.elapsed().as_millis();
        println!("  downloaded+verified -> {:?} ({download_ms} ms incl. any download)", path);

        let t1 = std::time::Instant::now();
        match OnnxSession::load(&path) {
            Ok(session) => {
                let load_ms = t1.elapsed().as_millis();
                println!("  execution provider: {} (load {load_ms} ms)", session.ep_name());
                println!("  inputs:");
                for inp in session.inner().inputs() {
                    println!("    {} : {}", inp.name(), inp.dtype());
                }
                println!("  outputs:");
                for out in session.inner().outputs() {
                    println!("    {} : {}", out.name(), out.dtype());
                }
            }
            Err(e) => {
                println!("  SESSION LOAD FAILED: {e:#}");
            }
        }
    }

    Ok(())
}
