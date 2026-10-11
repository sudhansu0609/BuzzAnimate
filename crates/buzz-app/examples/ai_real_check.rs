//! Chunks F6/F7: a measurement harness against **real** models and real
//! data — no mocks. Exercises C8 (Identify & Rename), C9 (Auto-Rig), C10
//! (Scene from Image, with SigLIP) and C11 (Direct Story) through the same
//! public `buzz_ai` entry points the app itself calls (mirroring the private
//! helpers in `buzz-app/src/app.rs` where those are not `pub`), on
//! `bhatta2.fla`, against either backend:
//!
//! * `--backend local` (default): LM Studio at `http://127.0.0.1:1234/v1`
//!   (vision `google/gemma-4-12b`, text `qwen/qwen3.8-27b`, low reasoning).
//! * `--backend claude`: the Claude API server at `127.0.0.1:8787/v1`
//!   (`sonnet` for both); the key comes from `%APPDATA%/BuzzAnimate/ai.json`
//!   or `BUZZ_CLAUDE_API_KEY`, never from here. C9 is ONNX-only, so it runs
//!   in the local pass.
//!
//! Output: `BUZZ_AI_OUT` (default: the system temp dir `buzz_ai_real/<backend>`).
//! `--vision-model`/`--text-model` override the models.
//!
//! cargo run --release -p buzz-app --example ai_real_check -- --backend claude

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use buzz_ai::llm::Backend;
use buzz_geom::{Affine, Point, Rect, Shape as _, Size, Vec2};
use buzz_scene::{LayerKind, Object, ObjectKind, Scene, ShapeData, Symbol, SymbolId};

const FLA_PATH: &str = "B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla";
static OUT_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static VISION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static IS_CLAUDE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn vision_model() -> &'static str {
    VISION.get().map(String::as_str).unwrap_or("google/gemma-4-12b")
}

fn text_model() -> &'static str {
    TEXT.get().map(String::as_str).unwrap_or("qwen/qwen3.8-27b")
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn backend() -> Backend {
    if *IS_CLAUDE.get().unwrap_or(&false) {
        Backend::default_claude_api()
    } else {
        Backend::OpenAiCompatible {
            base_url: "http://127.0.0.1:1234/v1".into(),
        }
    }
}

fn out(name: &str) -> PathBuf {
    OUT_DIR.get().expect("out dir set in main").join(name)
}

thread_local! {
    static SHARED_EXPORTER: &'static std::cell::RefCell<Option<buzz_export::Exporter>> =
        Box::leak(Box::new(std::cell::RefCell::new(None)));
}

/// **One exporter for the whole run** (F7.5): every render — stage frames,
/// 256² thumbnails, the reconstructed scene, lone rigs — goes through the
/// same `Exporter`, at whatever size it needs.
fn shared_exporter() -> Result<std::cell::RefMut<'static, buzz_export::Exporter>> {
    let cell = SHARED_EXPORTER.with(|c| *c);
    let mut guard = cell.borrow_mut();
    if guard.is_none() {
        *guard = Some(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?);
    }
    Ok(std::cell::RefMut::map(guard, |o| o.as_mut().expect("just set")))
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

// ---------------------------------------------------------------------------
// Report shapes
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default, Clone)]
struct RenameRow {
    id: String,
    old_name: String,
    new_name: String,
    category: String,
    view: String,
    tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct C8Report {
    secs: f64,
    batch_secs: Vec<f64>,
    parse_failures: usize,
    thumbnails: usize,
    sheet: String,
    renames: Vec<RenameRow>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct FrameMetrics {
    secs: f64,
    ground_truth: Vec<String>,
    detected_labels: Vec<String>,
    matched_names: Vec<String>,
    matched_names_siglip: Vec<String>,
    recall: f64,
    precision: f64,
    recall_siglip: f64,
    precision_siglip: f64,
    /// Placeable candidates considered (F7.1).
    candidates: usize,
    /// Per detected object: its label, the top SigLIP candidates with their
    /// cosines, and whether the top one is in the ground truth.
    siglip_top: Vec<String>,
    reference_png: String,
    reconstructed_png: String,
    /// SigLIP shortlist (top 5) confirmed by the vision model (F7.1).
    recall_confirmed: f64,
    precision_confirmed: f64,
    matched_names_confirmed: Vec<String>,
    confirm_secs: f64,
    /// Confirmed pick, else SigLIP's best above the threshold (the app's rule).
    recall_combined: f64,
    precision_combined: f64,
    matched_names_combined: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct C10Report {
    secs: f64,
    frame0: FrameMetrics,
    midframe: FrameMetrics,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct C11Report {
    brief: String,
    secs: f64,
    beats: usize,
    verbs_match_brief: bool,
    cues: usize,
    frames: u32,
    script_json: String,
    cue_list: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SlotRow {
    layer: String,
    slot: String,
    overlap_score: f64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct C9Report {
    symbol: String,
    selection_reason: String,
    candidates_considered: usize,
    joints_png: String,
    rest_png: String,
    walk_png: String,
    wave_png: String,
    slot_mapping: Vec<SlotRow>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Report {
    backend: String,
    vision_model: String,
    text_model: String,
    siglip_ready: bool,
    c8: C8Report,
    c10: C10Report,
    c11: C11Report,
    c9: C9Report,
    bugs_found: Vec<String>,
    notes: Vec<String>,
}

/// Writes `report.json` so far — called after every section, not just at the
/// end, so a run that gets cut short (a real local reasoning model can take
/// a while) still leaves the sections that *did* finish on disk instead of
/// nothing at all.
fn checkpoint(report: &Report) {
    if let Ok(json) = serde_json::to_string_pretty(report) {
        let _ = std::fs::write(out("report.json"), &json);
    }
}

// ---------------------------------------------------------------------------
// SigLIP model download (check 1)
// ---------------------------------------------------------------------------

fn ensure_siglip(bugs: &mut Vec<String>) -> bool {
    use buzz_ai::models::{ensure, ModelId};
    let t0 = Instant::now();
    match ensure(ModelId::SigLip, |_downloaded, _total| {}, || false) {
        Ok(path) => {
            println!("SigLIP ready at {path:?} ({:.1}s)", t0.elapsed().as_secs_f64());
            true
        }
        Err(e) => {
            bugs.push(format!("crates/buzz-ai/src/models.rs ensure(ModelId::SigLip) — {e}"));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// C8: Identify & Rename (check 2)
// ---------------------------------------------------------------------------

fn save_contact_sheet(thumbs: &[RgbaImage], path: &Path) {
    let cell = 200u32;
    let cols = 4u32;
    let rows = ((thumbs.len() as u32).max(1) + cols - 1) / cols;
    let mut sheet = RgbaImage::from_pixel(cell * cols, cell * rows.max(1), Rgba([24, 24, 24, 255]));
    for (i, t) in thumbs.iter().enumerate() {
        let resized = image::imageops::resize(t, cell, cell, image::imageops::FilterType::Triangle);
        let x = (i as u32 % cols) * cell;
        let y = (i as u32 / cols) * cell;
        image::imageops::overlay(&mut sheet, &resized, x as i64, y as i64);
    }
    let _ = sheet.save(path);
}

fn run_c8(scene: &Scene, backend: &Backend, bugs: &mut Vec<String>) -> C8Report {
    use buzz_ai::naming::{identify_batch, AssetThumbnailItem};

    let mut report = C8Report::default();

    let Ok(mut exporter) = shared_exporter() else {
        bugs.push("buzz-export Exporter::new — no GPU exporter available for C8 thumbnails".into());
        return report;
    };

    let mut items: Vec<AssetThumbnailItem> = Vec::new();
    let mut thumbs: Vec<RgbaImage> = Vec::new();
    for sym in scene.library().iter() {
        if items.len() >= 16 {
            break;
        }
        if sym.bounds().is_none() {
            continue;
        }
        let png = buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut exporter, scene, sym);
        if let Ok(img) = image::load_from_memory(&png) {
            let img = img.to_rgba8();
            let lit = img.pixels().filter(|p| p[0] > 20 || p[1] > 20 || p[2] > 20).count();
            if lit == 0 {
                bugs.push(format!("C8 thumbnail of '{}' is blank", sym.name));
            }
            thumbs.push(img);
        }
        items.push(AssetThumbnailItem {
            id: sym.id.0.to_string(),
            old_name: sym.name.clone(),
            png_bytes: png,
        });
    }
    report.thumbnails = items.len();

    let sheet_path = out("c8_contact_sheet.png");
    save_contact_sheet(&thumbs, &sheet_path);
    report.sheet = sheet_path.display().to_string();

    let t0 = Instant::now();
    for chunk in items.chunks(8) {
        let bt0 = Instant::now();
        match identify_batch(backend, vision_model(), chunk) {
            Ok(identified) => {
                for ident in identified {
                    let old_name = chunk
                        .iter()
                        .find(|i| i.id == ident.id)
                        .map(|i| i.old_name.clone())
                        .unwrap_or_default();
                    report.renames.push(RenameRow {
                        id: ident.id,
                        old_name,
                        new_name: ident.name,
                        category: ident.category.as_str().to_string(),
                        view: ident.view.as_str().to_string(),
                        tags: ident.tags,
                    });
                }
            }
            Err(e) => {
                if matches!(e, buzz_ai::llm::LlmError::InvalidJson { .. }) {
                    report.parse_failures += 1;
                }
                bugs.push(format!("crates/buzz-ai/src/naming.rs identify_batch — {e}"));
            }
        }
        report.batch_secs.push(bt0.elapsed().as_secs_f64());
    }
    report.secs = t0.elapsed().as_secs_f64();
    report
}

// ---------------------------------------------------------------------------
// C10: Scene from Image, with SigLIP (check 3)
// ---------------------------------------------------------------------------

/// F7.1: scored against the **visible top-level instances** of the frame.
fn ground_truth_symbols(scene: &Scene, frame: u32) -> HashSet<SymbolId> {
    buzz_ai::scene_builder::visible_top_level_symbols(scene, frame)
}

/// F7.1: candidates are the **placeable** symbols only (on stage somewhere,
/// or nested in nothing) — no body parts.
fn build_candidates(scene: &Scene) -> Vec<buzz_ai::scene_builder::LibraryAssetCandidate> {
    buzz_ai::scene_builder::placeable_candidates(scene)
}

fn compute_metrics(
    matched: &[buzz_ai::scene_builder::MatchedObjectReport],
    ground_truth: &HashSet<SymbolId>,
) -> (f64, f64, Vec<String>) {
    let matched_ids: Vec<SymbolId> = matched.iter().map(|m| SymbolId(m.symbol_id)).collect();
    let tp = matched_ids.iter().filter(|id| ground_truth.contains(id)).count();
    let recall = if ground_truth.is_empty() { 0.0 } else { tp as f64 / ground_truth.len() as f64 };
    let precision = if matched_ids.is_empty() { 0.0 } else { tp as f64 / matched_ids.len() as f64 };
    let names = matched.iter().map(|m| m.symbol_name.clone()).collect();
    (recall, precision, names)
}

/// Mirrors `siglip_embeddings_for_scene_from_image` in `buzz-app/src/app.rs`
/// (private to that crate module): each detected object's own crop of the
/// reference image, and a render of each library candidate, both embedded
/// with SigLIP so `build_scene_from_layout` can match on what things look
/// like, not just their name/tags.
fn siglip_embeddings(
    reference: &RgbaImage,
    layout: &buzz_ai::scene_builder::DetectedSceneLayout,
    scene: &Scene,
    candidates: &[buzz_ai::scene_builder::LibraryAssetCandidate],
) -> Result<SiglipInputs> {
    let (rw, rh) = (reference.width() as f64, reference.height() as f64);
    let _ = (rw, rh);
    let mut crops = Vec::with_capacity(layout.objects.len());
    for obj in &layout.objects {
        crops.push(buzz_ai::scene_builder::crop_for_matching(reference, obj));
    }
    static CALL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    save_contact_sheet(&crops, &out(&format!("c10_crops_{n}.png")));

    let mut thumb_exporter = shared_exporter()?;
    let mut thumbnails = Vec::with_capacity(candidates.len());
    for cand in candidates {
        let png = match scene.library().get(cand.id) {
            Some(sym) => buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut thumb_exporter, scene, sym),
            None => Vec::new(),
        };
        let thumb = image::load_from_memory(&png)
            .map(|d| d.to_rgba8())
            .unwrap_or_else(|_| RgbaImage::new(1, 1));
        thumbnails.push(thumb);
    }

    if n == 0 {
        save_contact_sheet(&thumbnails, &out("c10_candidate_thumbnails.png"));
    }
    let detected: Vec<Option<Vec<f32>>> = match buzz_ai::siglip::embed(&crops) {
        Ok(v) => v.into_iter().map(Some).collect(),
        Err(_) => vec![None; crops.len()],
    };
    let candidate_embeddings: Vec<Option<Vec<f32>>> = match buzz_ai::siglip::embed(&thumbnails) {
        Ok(v) => v.into_iter().map(Some).collect(),
        Err(_) => vec![None; thumbnails.len()],
    };

    Ok(SiglipInputs { detected, candidate_embeddings, crops, thumbnails })
}

struct SiglipInputs {
    detected: Vec<Option<Vec<f32>>>,
    candidate_embeddings: Vec<Option<Vec<f32>>>,
    crops: Vec<RgbaImage>,
    thumbnails: Vec<RgbaImage>,
}

fn png_of(img: &RgbaImage) -> Vec<u8> {
    let mut v = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png);
    v
}

fn run_c10_frame(
    scene: &Scene,
    frame: u32,
    backend: &Backend,
    bugs: &mut Vec<String>,
    label: &str,
) -> FrameMetrics {
    let mut metrics = FrameMetrics::default();
    let t0 = Instant::now();

    // Its own exporter, not shared with the thumbnail/reconstructed-scene
    // renders below — see the comment on `siglip_embeddings`.
    let mut ref_exporter = match shared_exporter() {
        Ok(e) => e,
        Err(e) => {
            bugs.push(format!("buzz-export Exporter::new — no GPU exporter for C10 {label}: {e}"));
            return metrics;
        }
    };
    let settings = buzz_export::ExportSettings::scaled(scene, 0.5);
    let rendered = match ref_exporter.render(scene, frame, &settings) {
        Ok(f) => f,
        Err(e) => {
            bugs.push(format!("buzz-export Exporter::render — C10 {label} reference render failed: {e}"));
            return metrics;
        }
    };
    drop(ref_exporter);
    let _ = rendered.write_png(&out(&format!("c10_{label}_reference.png")));
    metrics.reference_png = out(&format!("c10_{label}_reference.png")).display().to_string();
    let png_bytes = match rendered.to_png() {
        Ok(b) => b,
        Err(e) => {
            bugs.push(format!("buzz-export Frame::to_png — C10 {label} — {e}"));
            return metrics;
        }
    };
    let reference_img = match RgbaImage::from_raw(rendered.width, rendered.height, rendered.pixels.clone()) {
        Some(i) => i,
        None => {
            bugs.push(format!("C10 {label} — rendered buffer did not match its own dimensions"));
            return metrics;
        }
    };

    let ground_truth = ground_truth_symbols(scene, frame);
    metrics.ground_truth = ground_truth
        .iter()
        .filter_map(|id| scene.library().get(*id).map(|s| s.name.clone()))
        .collect();

    let layout = match buzz_ai::scene_builder::decompose_scene_image(backend, vision_model(), &png_bytes) {
        Ok(l) => l,
        Err(e) => {
            bugs.push(format!(
                "crates/buzz-ai/src/scene_builder.rs decompose_scene_image — C10 {label} — {e}"
            ));
            return metrics;
        }
    };
    metrics.detected_labels = layout.objects.iter().map(|o| o.label.clone()).collect();

    let candidates_base = build_candidates(scene);
    metrics.candidates = candidates_base.len();
    let stage_size: Size = scene.stage().size;
    let crop_skeletons: Vec<Option<buzz_ai::character::Skeleton>> = vec![None; layout.objects.len()];

    // -- without SigLIP --------------------------------------------------
    let no_siglip_embeddings: Vec<Option<Vec<f32>>> = vec![None; layout.objects.len()];
    let result_no_siglip = buzz_ai::scene_builder::build_scene_from_layout(
        &layout,
        &candidates_base,
        &crop_skeletons,
        &no_siglip_embeddings,
        Some(stage_size),
    );
    let (recall, precision, matched_names) = compute_metrics(&result_no_siglip.matched, &ground_truth);
    metrics.recall = recall;
    metrics.precision = precision;
    metrics.matched_names = matched_names;

    // -- with SigLIP -------------------------------------------------------
    match siglip_embeddings(&reference_img, &layout, scene, &candidates_base) {
        Ok(SiglipInputs { detected, candidate_embeddings, crops, thumbnails }) => {
            let mut candidates_siglip = candidates_base;
            for (c, emb) in candidates_siglip.iter_mut().zip(candidate_embeddings) {
                c.visual_embedding = emb;
            }
            let result_siglip = buzz_ai::scene_builder::build_scene_from_layout(
                &layout,
                &candidates_siglip,
                &crop_skeletons,
                &detected,
                Some(stage_size),
            );
            let (r2, p2, names2) = compute_metrics(&result_siglip.matched, &ground_truth);
            metrics.recall_siglip = r2;
            metrics.precision_siglip = p2;
            metrics.matched_names_siglip = names2;
            for (i, obj) in layout.objects.iter().enumerate() {
                let emb = detected.get(i).and_then(|e| e.as_deref());
                let mut scored: Vec<(f64, &buzz_ai::scene_builder::LibraryAssetCandidate)> = candidates_siglip
                    .iter()
                    .filter_map(|c| buzz_ai::scene_builder::visual_cosine(emb, c).map(|v| (v, c)))
                    .collect();
                scored.sort_by(|a, b| b.0.total_cmp(&a.0));
                let top: Vec<String> = scored
                    .iter()
                    .take(3)
                    .map(|(v, c)| format!("{}{} {:.3}", c.name, if ground_truth.contains(&c.id) { "*" } else { "" }, v))
                    .collect();
                metrics.siglip_top.push(format!(
                    "{} [{:.2},{:.2},{:.2},{:.2}] -> {}",
                    obj.label, obj.bbox[0], obj.bbox[1], obj.bbox[2], obj.bbox[3],
                    top.join(", ")
                ));
            }

            // SigLIP shortlist → the vision model confirms one (or none).
            let ct0 = Instant::now();
            let mut choices: Vec<Option<(usize, f64)>> = Vec::with_capacity(layout.objects.len());
            for (i, obj) in layout.objects.iter().enumerate() {
                let short = buzz_ai::scene_builder::siglip_shortlist(
                    detected.get(i).and_then(|e| e.as_deref()),
                    &candidates_siglip,
                    std::env::var("C10_K").ok().and_then(|v| v.parse().ok()).unwrap_or(6),
                );
                let imgs: Vec<RgbaImage> = short.iter().map(|(ci, _)| thumbnails[*ci].clone()).collect();
                if i < 3 {
                    let _ = buzz_ai::scene_builder::choice_sheet(&crops[i], &imgs)
                        .save(out(&format!("c10_{label}_choice_{i}.png")));
                }
                match buzz_ai::scene_builder::confirm_with_vlm(backend, vision_model(), &obj.label, &crops[i], &imgs) {
                    Ok(Some(pos)) => choices.push(Some(short[pos])),
                    Ok(None) => choices.push(None),
                    Err(e) => {
                        bugs.push(format!("C10 {label} confirm_with_vlm({}) — {e}", obj.label));
                        choices.push(None);
                    }
                }
            }
            metrics.confirm_secs = ct0.elapsed().as_secs_f64();
            let result_confirmed = buzz_ai::scene_builder::build_scene_from_choices(
                &layout,
                &candidates_siglip,
                &crop_skeletons,
                &choices,
                Some(stage_size),
            );
            let (r3, p3, names3) = compute_metrics(&result_confirmed.matched, &ground_truth);
            metrics.recall_confirmed = r3;
            metrics.precision_confirmed = p3;
            metrics.matched_names_confirmed = names3;
            // What the app does (F7.1): the model's confirmed pick, else
            // SigLIP's best when it clears `SIGLIP_MATCH_MIN`.
            let combined: Vec<Option<(usize, f64)>> = choices
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    c.or_else(|| {
                        buzz_ai::scene_builder::siglip_shortlist(detected.get(i).and_then(|e| e.as_deref()), &candidates_siglip, 1)
                            .first()
                            .copied()
                            .filter(|(_, v)| *v >= buzz_ai::scene_builder::SIGLIP_MATCH_MIN)
                    })
                })
                .collect();
            let result_combined = buzz_ai::scene_builder::build_scene_from_choices(
                &layout,
                &candidates_siglip,
                &crop_skeletons,
                &combined,
                Some(stage_size),
            );
            let (r4, p4, names4) = compute_metrics(&result_combined.matched, &ground_truth);
            metrics.recall_combined = r4;
            metrics.precision_combined = p4;
            metrics.matched_names_combined = names4;
            let _ = result_confirmed;
            let result_siglip = result_combined;

            // Its own exporter again — the reconstructed scene's stage size
            // generally differs from both the reference frame and the fixed
            // thumbnail size above.
            match shared_exporter() {
                Ok(mut recon_exporter) => {
                    // The reconstructed scene places instances of the source
                    // library's symbols; give it that library so they draw
                    // (the app merges into the open document instead).
                    let mut recon = result_siglip.scene.clone();
                    for sym in scene.library().iter() {
                        recon.library_mut().insert((**sym).clone());
                    }
                    recon.stage_mut().background = scene.stage().background;
                    let recon_settings = buzz_export::ExportSettings::scaled(&recon, 0.5);
                    if let Ok(recon_frame) = recon_exporter.render(&recon, 0, &recon_settings) {
                        let p = out(&format!("c10_{label}_reconstructed.png"));
                        let _ = recon_frame.write_png(&p);
                        metrics.reconstructed_png = p.display().to_string();
                    }
                }
                Err(e) => bugs.push(format!(
                    "buzz-export Exporter::new — no GPU exporter for C10 {label} reconstructed scene: {e}"
                )),
            }
        }
        Err(e) => {
            bugs.push(format!("C10 {label} — siglip_embeddings failed: {e}"));
        }
    }

    metrics.secs = t0.elapsed().as_secs_f64();
    metrics
}

// ---------------------------------------------------------------------------
// C11: Direct Story (check 4)
// ---------------------------------------------------------------------------

fn run_c11(backend: &Backend, bugs: &mut Vec<String>) -> C11Report {
    use buzz_ai::director::{dialogue_cues, execute_director_script, parse_story_to_script, DirectorVerb};

    let mut report = C11Report::default();
    let brief = "Morning. Village. The Sadhu walks in from the left and waves. The old man turns and talks to him: \"Namaste, Baba. Will you stay for tea?\" The Sadhu listens and says, \"Only if it rains.\" Then the Sadhu walks off.";
    report.brief = brief.to_string();

    let t0 = Instant::now();
    let script = match parse_story_to_script(backend, text_model(), brief) {
        Ok(s) => s,
        Err(e) => {
            bugs.push(format!("crates/buzz-ai/src/director.rs parse_story_to_script — {e}"));
            return report;
        }
    };
    report.secs = t0.elapsed().as_secs_f64();
    report.beats = script.beats.len();
    report.script_json = serde_json::to_string_pretty(&script).unwrap_or_default();
    println!("C11 parsed script:\n{}", report.script_json);

    let has_enter = script.beats.iter().any(|b| b.verb == DirectorVerb::EnterLeft);
    let has_wave = script.beats.iter().any(|b| b.verb == DirectorVerb::Wave);
    let has_turn = script.beats.iter().any(|b| b.verb == DirectorVerb::Turn);
    let has_talk = script.beats.iter().any(|b| {
        b.verb == DirectorVerb::Talk
            && b.dialogue
                .as_deref()
                .map(|d| d.to_lowercase().contains("namaste"))
                .unwrap_or(false)
    });
    let has_listen = script.beats.iter().any(|b| b.verb == DirectorVerb::Listen);
    let has_walk_away = script.beats.iter().any(|b| b.verb == DirectorVerb::WalkAway);
    report.verbs_match_brief = has_enter && has_wave && has_turn && has_talk && has_listen && has_walk_away;

    let mut scene = Scene::default();
    let directed = match execute_director_script(&mut scene, &script) {
        Ok(d) => d,
        Err(e) => {
            bugs.push(format!("crates/buzz-ai/src/director.rs execute_director_script — {e}"));
            return report;
        }
    };
    report.frames = directed.frames;

    let cues = dialogue_cues(&script, &directed, 24.0);
    report.cues = cues.len();
    report.cue_list = cues
        .iter()
        .map(|c| format!("{} [{}-{}]: {}", c.speaker, c.start_frame, c.end_frame, c.text))
        .collect();

    let mid = directed.frames / 2;
    let last = directed.frames.saturating_sub(1);
    match shared_exporter() {
        Ok(mut exporter) => {
            let settings = buzz_export::ExportSettings::for_stage(&scene);
            for (label, frame) in [("frame0", 0u32), ("mid", mid), ("last", last)] {
                match exporter.render(&scene, frame, &settings) {
                    Ok(f) => {
                        let _ = f.write_png(&out(&format!("c11_{label}.png")));
                    }
                    Err(e) => bugs.push(format!("C11 render frame {label} — {e}")),
                }
            }
        }
        Err(e) => bugs.push(format!("buzz-export Exporter::new — no GPU exporter for C11: {e}")),
    }

    report
}

// ---------------------------------------------------------------------------
// C9: Auto-Rig (check 5) — mirrors the private helpers in
// `buzz-app/src/app.rs` (`rig_vector_character_from_detected_skeleton`,
// `rig_parts_for_symbol`, `render_symbol_alone`, `SymbolRenderMap`), which
// are not `pub` outside that crate.
// ---------------------------------------------------------------------------

struct SymbolRenderMap {
    region_center: Point,
    cam_zoom: f64,
    out_width: f64,
    out_height: f64,
    placed_at: Point,
}

impl SymbolRenderMap {
    fn to_symbol_local(&self, px: f32, py: f32) -> (f32, f32) {
        let world_x = (px as f64 - self.out_width * 0.5) / self.cam_zoom + self.region_center.x;
        let world_y = (py as f64 - self.out_height * 0.5) / self.cam_zoom + self.region_center.y;
        ((world_x - self.placed_at.x) as f32, (world_y - self.placed_at.y) as f32)
    }
}

fn render_symbol_alone(scene: &mut Scene, symbol: &Symbol) -> Result<(RgbaImage, SymbolRenderMap)> {
    let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in ids {
        scene.edit_layers().remove(id);
    }
    scene.camera_mut().enabled = false;
    let probe = scene.add_layer("__ai_rig_probe", LayerKind::Normal);
    let centre = scene.stage().stage_rect().center();
    scene
        .add_instance_at(probe, 0, symbol.id, Affine::translate(centre.to_vec2()))
        .ok_or_else(|| anyhow::anyhow!("could not place '{}' to render it", symbol.name))?;

    let local_bounds = symbol
        .bounds()
        .ok_or_else(|| anyhow::anyhow!("'{}' has no visible artwork", symbol.name))?;
    let world = Rect::new(
        local_bounds.x0 + centre.x,
        local_bounds.y0 + centre.y,
        local_bounds.x1 + centre.x,
        local_bounds.y1 + centre.y,
    );
    let pad_x = world.width() * 0.04;
    let pad_y = world.height() * 0.04;
    let region = Rect::new(world.x0 - pad_x, world.y0 - pad_y, world.x1 + pad_x, world.y1 + pad_y);
    let out_h = 1024u32;
    let out_w = ((out_h as f64) * region.width() / region.height().max(1e-6)).round().max(1.0) as u32;

    let settings = buzz_export::ExportSettings {
        width: out_w,
        height: out_h,
        transparent: true,
        region: Some(region),
    };

    let mut exporter = shared_exporter()?;
    let frame = exporter.render(scene, 0, &settings)?;
    let image = RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow::anyhow!("the rendered buffer did not match its own dimensions"))?;

    let map = SymbolRenderMap {
        region_center: region.center(),
        cam_zoom: settings.width as f64 / region.width().max(1e-6),
        out_width: settings.width as f64,
        out_height: settings.height as f64,
        placed_at: centre,
    };
    Ok((image, map))
}

fn rig_parts_for_symbol(scene: &mut Scene, symbol: &Symbol) -> Vec<buzz_ai::rerig::VectorPartCandidate> {
    let mut parts = Vec::new();
    for layer in symbol.layers.iter() {
        let objects = layer.objects_at(0);
        if objects.is_empty() {
            continue;
        }
        let object: Arc<Object> = if objects.len() == 1 {
            objects[0].clone()
        } else {
            let id = scene.next_object_id();
            Arc::new(Object::group(id, objects.iter().cloned().collect()))
        };
        let bounds = layer.bounds().unwrap_or_else(|| object.bounds());
        parts.push(buzz_ai::rerig::VectorPartCandidate {
            name: layer.name.clone(),
            bounds,
            mask: None,
            object,
        });
    }
    parts
}

fn draw_joints(img: &RgbaImage, person: &buzz_ai::pose::Person) -> RgbaImage {
    let mut out_img = img.clone();
    for (x, y, conf) in person.keypoints.iter().take(17) {
        if *conf < 0.2 {
            continue;
        }
        let (cx, cy) = (*x as i64, *y as i64);
        for dy in -3..=3i64 {
            for dx in -3..=3i64 {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < out_img.width() && (py as u32) < out_img.height() {
                    out_img.put_pixel(px as u32, py as u32, Rgba([255, 0, 0, 255]));
                }
            }
        }
    }
    out_img
}

/// Renders a bare `ArmatureData` (rest, or posed) alone on its own stage,
/// scaled so it is at least 1024px tall.
///
/// Cloned from `library_scene` (bhatta2's own scene) rather than
/// `Scene::default()`: several of this rig's parts are themselves instances
/// of other library symbols (the character's clothing pieces, drawn once and
/// reused), which only resolve against the library they actually came from.
///
/// F7.6: this used to render blank — the cloned document's camera reframed
/// it, and `Scene::resolved_bounds` measured the rig by its parts'
/// placeholder instance bounds. Both are fixed.
fn render_armature(
    library_scene: &Scene,
    armature: &buzz_scene::ArmatureData,
    path: &Path,
    bugs: &mut Vec<String>,
    tag: &str,
) {
    let mut scene = library_scene.clone();
    scene.edit_document();
    // F7.6: the document's own camera (bhatta2 has one) reframed this
    // render off the rig — the "blank" armature. `render_symbol_alone`
    // already turned it off.
    scene.camera_mut().enabled = false;
    for id in scene.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        scene.remove_layer(id);
    }
    let layer = scene.add_layer("Armature", LayerKind::Normal);
    let id = scene.next_object_id();
    let mut obj = Object::shape(
        id,
        ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), peniko::Color::TRANSPARENT),
    );
    obj.kind = ObjectKind::Armature(armature.clone());

    // `scene.resolved_bounds`, not the parts' own scene-less `.bounds()`:
    // several parts are themselves instances of other library symbols, whose
    // size `Object::bounds()` cannot know without the scene/library to look
    // the target symbol's own bounds up in.
    let raw_bounds = scene.resolved_bounds(&obj);
    let target_h = 1024.0f64;
    let scale = if raw_bounds.height() > 0.0 { target_h / raw_bounds.height() } else { 1.0 };
    let stage_w = (raw_bounds.width() * scale + 200.0).max(200.0);
    let stage_h = target_h + 200.0;
    scene.stage_mut().size = Size::new(stage_w, stage_h);
    let center = raw_bounds.center();
    obj.transform = Affine::translate(Vec2::new(stage_w / 2.0, stage_h / 2.0))
        * Affine::scale(scale)
        * Affine::translate(Vec2::new(-center.x, -center.y));
    scene.add_object(layer, obj);

    match shared_exporter() {
        Ok(mut exporter) => {
            let settings = buzz_export::ExportSettings {
                width: stage_w.round() as u32,
                height: stage_h.round() as u32,
                transparent: true,
                region: None,
            };
            match exporter.render(&scene, 0, &settings) {
                Ok(frame) => {
                    let _ = frame.write_png(path);
                }
                Err(e) => bugs.push(format!("C9 render_armature({tag}) — {e}")),
            }
        }
        Err(e) => bugs.push(format!("buzz-export Exporter::new — no GPU exporter for C9 {tag}: {e}")),
    }
}

/// Mean confidence over the 17 COCO body keypoints — a proxy for "DWPose
/// found a real, legible figure here" rather than a sliver of a limb.
fn mean_body_confidence(person: &buzz_ai::pose::Person) -> f32 {
    let sum: f32 = person.keypoints.iter().take(17).map(|(_, _, c)| *c).sum();
    sum / 17.0
}

fn run_c9(scene: &Scene, bugs: &mut Vec<String>) -> C9Report {
    let mut report = C9Report::default();

    if !buzz_ai::models::status(buzz_ai::models::ModelId::DwPoseEstimator).is_ready() {
        bugs.push("buzz_ai::models::ModelId::DwPoseEstimator — not installed, C9 cannot run pose detection".into());
        return report;
    }

    // Candidate pool: multi-layer symbols (a jointed figure, not a single
    // prop — bhatta2's library has sub-parts like a moustache whose name
    // also contains "sadhu") whose name suggests a walk cycle, ranked
    // tallest-aspect-ratio first (the shape a standing character render
    // should have), then narrowed to whichever of the top 10 DWPose
    // actually detects a person in with the highest mean keypoint
    // confidence.
    let mut candidates: Vec<Arc<Symbol>> = scene
        .library()
        .iter()
        .filter(|s| {
            let n = s.name.to_lowercase();
            (n.contains("oldman") || n.contains("sadhu"))
                && s.layers.iter().count() >= 2
                && s.bounds().map(|b| b.width() > 0.0 && b.height() > 0.0).unwrap_or(false)
        })
        .cloned()
        .collect();
    candidates.sort_by(|a, b| {
        let aspect = |s: &Symbol| {
            let bounds = s.bounds().expect("filtered to Some above");
            bounds.height() / bounds.width().max(1.0)
        };
        aspect(b).partial_cmp(&aspect(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(10);
    report.candidates_considered = candidates.len();

    if candidates.is_empty() {
        bugs.push("bhatta2.fla library — no multi-layer symbol named containing 'Oldman' or 'Sadhu' was found".into());
        return report;
    }

    let mut best: Option<(Arc<Symbol>, f32)> = None;
    for cand in &candidates {
        let mut trial_scene = scene.clone();
        let (image, _map) = match render_symbol_alone(&mut trial_scene, cand) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Ok(persons) = buzz_ai::pose::detect(&image) else {
            continue;
        };
        let Some(person) = persons.into_iter().next() else {
            continue;
        };
        let confidence = mean_body_confidence(&person);
        if best.as_ref().map(|(_, c)| confidence > *c).unwrap_or(true) {
            best = Some((Arc::clone(cand), confidence));
        }
    }

    let Some((symbol_owned, confidence)) = best else {
        bugs.push(format!(
            "buzz_ai::pose::detect — no figure detected in any of the top {} tallest multi-layer 'Oldman'/'Sadhu' candidates",
            candidates.len()
        ));
        return report;
    };
    report.symbol = symbol_owned.name.clone();
    report.selection_reason = format!(
        "highest mean DWPose body-keypoint confidence ({confidence:.3}) among the {} tallest multi-layer symbols named 'Oldman'/'Sadhu'",
        candidates.len()
    );
    let tag = safe_name(&symbol_owned.name);

    let mut work_scene = scene.clone();

    // Order matches `rig_vector_character_from_detected_skeleton` in
    // buzz-app/src/app.rs: parts first (reads the symbol's own layers),
    // then the alone-render (which strips the scene's stage layers).
    let parts = rig_parts_for_symbol(&mut work_scene, &symbol_owned);
    if parts.is_empty() {
        bugs.push(format!(
            "crates/buzz-app/src/app.rs rig_parts_for_symbol (mirrored) — no vector artwork found in '{}'",
            symbol_owned.name
        ));
        return report;
    }

    let (image, map) = match render_symbol_alone(&mut work_scene, &symbol_owned) {
        Ok(v) => v,
        Err(e) => {
            bugs.push(format!(
                "crates/buzz-app/src/app.rs render_symbol_alone (mirrored) — {e}"
            ));
            return report;
        }
    };

    let persons = match buzz_ai::pose::detect(&image) {
        Ok(p) => p,
        Err(e) => {
            bugs.push(format!("crates/buzz-ai/src/pose.rs detect — {e}"));
            return report;
        }
    };
    let Some(mut person) = persons.into_iter().next() else {
        bugs.push(format!(
            "buzz_ai::pose::detect — no figure detected in '{}'",
            symbol_owned.name
        ));
        return report;
    };

    let joints_img = draw_joints(&image, &person);
    let joints_path = out(&format!("c9_{tag}_joints.png"));
    let _ = joints_img.save(&joints_path);
    report.joints_png = joints_path.display().to_string();

    for kp in person.keypoints.iter_mut() {
        let (x, y) = map.to_symbol_local(kp.0, kp.1);
        kp.0 = x;
        kp.1 = y;
    }
    let (bx0, by0) = map.to_symbol_local(person.bbox[0], person.bbox[1]);
    let (bx1, by1) = map.to_symbol_local(person.bbox[2], person.bbox[3]);
    person.bbox = [bx0, by0, bx1, by1];

    let skel = buzz_ai::character::skeleton(&person);
    let rerigged = match buzz_ai::rerig::rerig_vector_character(&skel, &parts, &symbol_owned.name, None) {
        Ok(r) => r,
        Err(e) => {
            bugs.push(format!("crates/buzz-ai/src/rerig.rs rerig_vector_character — {e}"));
            return report;
        }
    };

    report.slot_mapping = rerigged
        .mappings
        .iter()
        .map(|m| SlotRow {
            layer: m.part_name.clone(),
            slot: m.slot_name.clone(),
            overlap_score: m.overlap_score,
        })
        .collect();

    let rest_path = out(&format!("c9_{tag}_rest.png"));
    render_armature(scene, &rerigged.armature, &rest_path, bugs, "rest");
    report.rest_png = rest_path.display().to_string();

    let mut walk_armature = rerigged.armature.clone();
    if !buzz_ai::motion::PoseLibrary::standard().apply_to_armature("walk", &mut walk_armature) {
        bugs.push("buzz_ai::motion::PoseLibrary::standard — 'walk' pose missing".into());
    }
    let walk_path = out(&format!("c9_{tag}_walk.png"));
    render_armature(scene, &walk_armature, &walk_path, bugs, "walk");
    report.walk_png = walk_path.display().to_string();

    let mut wave_armature = rerigged.armature.clone();
    if !buzz_ai::motion::PoseLibrary::standard().apply_to_armature("wave", &mut wave_armature) {
        bugs.push("buzz_ai::motion::PoseLibrary::standard — 'wave' pose missing".into());
    }
    let wave_path = out(&format!("c9_{tag}_wave.png"));
    render_armature(scene, &wave_armature, &wave_path, bugs, "wave");
    report.wave_png = wave_path.display().to_string();

    report
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let which = arg_value("--backend").unwrap_or_else(|| "local".into());
    let is_claude = which == "claude";
    IS_CLAUDE.set(is_claude).ok();
    let default_model = |local: &str| if is_claude { "sonnet".to_string() } else { local.to_string() };
    VISION.set(arg_value("--vision-model").unwrap_or_else(|| default_model("google/gemma-4-12b"))).ok();
    TEXT.set(arg_value("--text-model").unwrap_or_else(|| default_model("qwen/qwen3.8-27b"))).ok();
    let dir = std::env::var_os("BUZZ_AI_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("buzz_ai_real"))
        .join(&which);
    std::fs::create_dir_all(&dir).ok();
    OUT_DIR.set(dir).ok();
    // F7.3: ask reasoning models for little hidden reasoning.
    buzz_ai::llm::set_low_reasoning(!std::env::args().any(|a| a == "--full-reasoning"));
    let backend = backend();
    println!(
        "backend {} ({}), vision {}, text {}, out {:?}",
        which,
        backend.display_name(),
        vision_model(),
        text_model(),
        out("")
    );

    // Recomputes only C11 against an existing report.json checkpoint (C8/C10
    // are ~15-20 minutes of real inference and don't need redoing just
    // because LM Studio's own idle TTL unloaded the text model between
    // sections of a previous run).
    if std::env::args().any(|a| a == "--only-c8") {
        let imported = buzz_app::import::read(Path::new(FLA_PATH)).map_err(anyhow::Error::msg)?;
        let mut bugs = Vec::new();
        let c8 = run_c8(&imported.scene, &backend, &mut bugs);
        println!("C8 {:.1}s, {} renames, bugs {bugs:?}", c8.secs, c8.renames.len());
        return Ok(());
    }
    if let Some(names) = arg_value("--thumbs") {
        let imported = buzz_app::import::read(Path::new(FLA_PATH)).map_err(anyhow::Error::msg)?;
        let scene = imported.scene;
        let mut imgs = Vec::new();
        for n in names.split(',') {
            if let Some(sym) = scene.library().iter().find(|s| s.name == n) {
                let mut ex = shared_exporter()?;
                let png = buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut ex, &scene, sym);
                imgs.push(image::load_from_memory(&png)?.to_rgba8());
            }
        }
        save_contact_sheet(&imgs, &out("thumbs_named.png"));
        return Ok(());
    }
    if std::env::args().any(|a| a == "--only-c10") {
        let imported = buzz_app::import::read(Path::new(FLA_PATH)).map_err(anyhow::Error::msg)?;
        let scene = imported.scene;
        let _ = scene.cached_bounds_table();
        let mut bugs = Vec::new();
        let t0 = Instant::now();
        let mut c10 = C10Report::default();
        c10.frame0 = run_c10_frame(&scene, 0, &backend, &mut bugs, "frame0");
        c10.midframe = run_c10_frame(&scene, scene.frame_count() / 2, &backend, &mut bugs, "midframe");
        c10.secs = t0.elapsed().as_secs_f64();
        std::fs::write(out("c10_only.json"), serde_json::to_string_pretty(&c10)?)?;
        println!("C10 done in {:.1}s, bugs {bugs:?}", c10.secs);
        return Ok(());
    }
    if std::env::args().any(|a| a == "--only-c11") {
        let existing = std::fs::read_to_string(out("report.json")).unwrap_or_default();
        let mut report: Report = serde_json::from_str(&existing).unwrap_or_default();
        let mut bugs = report.bugs_found.clone();
        println!("== C11: Direct Story (--only-c11) ==");
        report.c11 = run_c11(&backend, &mut bugs);
        report.bugs_found = bugs;
        checkpoint(&report);
        let json = serde_json::to_string_pretty(&report)?;
        println!("\n=== report.json ===\n{json}");
        return Ok(());
    }

    // Recomputes only C9 (no LLM calls, so cheap to redo alone after a
    // harness-side rendering fix) against an existing report.json checkpoint.
    if std::env::args().any(|a| a == "--only-c9") {
        let existing = std::fs::read_to_string(out("report.json")).unwrap_or_default();
        let mut report: Report = serde_json::from_str(&existing).unwrap_or_default();
        let mut bugs = report.bugs_found.clone();
        println!("== C9: Auto-Rig (--only-c9) ==");
        match buzz_app::import::read(Path::new(FLA_PATH)) {
            Ok(imported) => {
                report.c9 = run_c9(&imported.scene, &mut bugs);
            }
            Err(e) => bugs.push(format!("buzz_app::import::read({FLA_PATH}) — {e}")),
        }
        report.bugs_found = bugs;
        checkpoint(&report);
        let json = serde_json::to_string_pretty(&report)?;
        println!("\n=== report.json ===\n{json}");
        return Ok(());
    }

    let mut bugs: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    println!("== SigLIP model download ==");
    let siglip_ready = ensure_siglip(&mut bugs);

    println!("== Import bhatta2.fla ==");
    let import_t0 = Instant::now();
    let scene_opt: Option<Scene> = match buzz_app::import::read(Path::new(FLA_PATH)) {
        Ok(imported) => {
            println!(
                "Import: {:.1}s — {}",
                import_t0.elapsed().as_secs_f64(),
                imported.summary
            );
            for warning in &imported.unsupported {
                println!("Not imported: {warning}");
            }
            let _ = imported.scene.cached_bounds_table();
            Some(imported.scene)
        }
        Err(e) => {
            bugs.push(format!("buzz_app::import::read({FLA_PATH}) — {e}"));
            None
        }
    };

    let mut report = Report {
        backend: which.clone(),
        vision_model: vision_model().to_string(),
        text_model: text_model().to_string(),
        siglip_ready,
        ..Report::default()
    };
    checkpoint(&report);

    if let Some(scene) = &scene_opt {
        if !std::env::args().any(|a| a == "--skip-c8") {
            println!("== C8: Identify & Rename ==");
            report.c8 = run_c8(scene, &backend, &mut bugs);
        }
        report.bugs_found = bugs.clone();
        checkpoint(&report);

        println!("== C10: Scene from Image (with/without SigLIP) ==");
        let c10_t0 = Instant::now();
        report.c10.frame0 = run_c10_frame(scene, 0, &backend, &mut bugs, "frame0");
        report.bugs_found = bugs.clone();
        checkpoint(&report);

        let mid_frame = scene.frame_count() / 2;
        report.c10.midframe = run_c10_frame(scene, mid_frame, &backend, &mut bugs, "midframe");
        report.c10.secs = c10_t0.elapsed().as_secs_f64();
        report.bugs_found = bugs.clone();
        checkpoint(&report);

        if is_claude {
            notes.push("C9 (Auto-Rig) is ONNX-only (DWPose), not an LLM call: run in the local pass.".into());
        } else if !std::env::args().any(|a| a == "--skip-c9") {
            println!("== C9: Auto-Rig ==");
            report.c9 = run_c9(scene, &mut bugs);
            report.bugs_found = bugs.clone();
            checkpoint(&report);
        }
    } else {
        notes.push("bhatta2.fla failed to import — C8, C10 and C9 were skipped.".into());
    }

    println!("== C11: Direct Story ==");
    report.c11 = run_c11(&backend, &mut bugs);
    report.bugs_found = bugs;
    report.notes = notes;
    checkpoint(&report);
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(out("report.json"), &json)?;
    println!("\n=== report.json ===\n{json}");
    Ok(())
}
