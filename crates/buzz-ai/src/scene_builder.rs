//! Build a scene from a reference image (Chunk C10).
//!
//! Workflow:
//! 1. The local VLM is asked for `{objects:[{label, bbox:[x0,y0,x1,y1] normalised, kind, facing}], ground_y, camera:"wide|medium|close"}`.
//! 2. Matches detected objects against library assets:
//!    - First by names and tags (C8).
//!    - Then by visual embedding / feature similarity.
//! 3. Placement: scales to the bounding box, sets depth order by ground contact (bbox bottom),
//!    and poses characters using C7 `pose_from_skeleton` on their crop.
//! 4. Produces an editable Scene and a report of matched and unmatched objects.

use serde::{Deserialize, Serialize};

use buzz_geom::{Affine, Rect, Size};
use buzz_scene::{LayerKind, Object, ObjectKind, Scene, SymbolId};

use crate::character::Skeleton;
use crate::llm::{Llm, LlmError};
use crate::motion::{pose_from_skeleton, CharacterPose};

/// Camera framing angle detected from the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CameraFraming {
    Wide,
    Medium,
    Close,
}

impl CameraFraming {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Medium => "medium",
            Self::Close => "close",
        }
    }
}

/// A detected object in the reference scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedSceneObject {
    /// Descriptive label (e.g. 'banyan_tree', 'villager', 'hut', 'river').
    pub label: String,
    /// Normalised bounding box [x0, y0, x1, y1] within 0.0..=1.0.
    pub bbox: [f64; 4],
    /// Category kind (character, prop, background, vehicle, animal, effect, other).
    pub kind: String,
    /// Facing direction (left, right, front, back, na).
    #[serde(default = "default_facing")]
    pub facing: String,
}

fn default_facing() -> String {
    "front".to_string()
}

/// Layout and composition extracted from a reference image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedSceneLayout {
    pub objects: Vec<DetectedSceneObject>,
    /// Normalised horizon or ground level line (0.0..=1.0).
    pub ground_y: f64,
    /// Overall camera framing.
    pub camera: String,
}

/// Detailed report of a successfully placed library asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchedObjectReport {
    pub label: String,
    pub symbol_id: u64,
    pub symbol_name: String,
    pub stage_position: (f64, f64),
    pub scale: f64,
    pub depth_index: usize,
    pub match_score: f64,
    /// Pose estimated from the crop's skeleton (C7), if one was available for this
    /// object. The caller applies this to the placed instance's armature, if any;
    /// we cannot know here whether the matched symbol is rigged.
    pub pose: Option<CharacterPose>,
}

/// Report of an object detected in the image that could not be matched to an existing asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnmatchedObjectReport {
    pub label: String,
    pub bbox: [f64; 4],
    pub kind: String,
    pub reason: String,
}

/// Complete result of reconstructing a scene from a reference image.
pub struct ReconstructedScene {
    pub scene: Scene,
    pub layout: DetectedSceneLayout,
    pub matched: Vec<MatchedObjectReport>,
    pub unmatched: Vec<UnmatchedObjectReport>,
}

/// Strict JSON schema for VLM scene decomposition.
pub fn scene_decomposition_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "objects": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "bbox": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 4,
                            "maxItems": 4
                        },
                        "kind": {
                            "type": "string",
                            "enum": ["character", "prop", "background", "vehicle", "animal", "effect", "other"]
                        },
                        "facing": {
                            "type": "string",
                            "enum": ["left", "right", "front", "back", "na"]
                        }
                    },
                    "required": ["label", "bbox", "kind", "facing"],
                    "additionalProperties": false
                }
            },
            "ground_y": { "type": "number" },
            "camera": {
                "type": "string",
                "enum": ["wide", "medium", "close"]
            }
        },
        "required": ["objects", "ground_y", "camera"],
        "additionalProperties": false
    })
}

/// Queries the local VLM to decompose a reference image into objects, bboxes, ground line, and camera.
pub fn decompose_scene_image(
    llm: &impl Llm,
    model: &str,
    png_bytes: &[u8],
) -> Result<DetectedSceneLayout, LlmError> {
    let system_prompt = "You are a professional 2D animation art director and layout artist. \
Decompose the input image into distinct animation assets. For every visible character, prop, \
scenery item, and building, return its label, normalized bounding box [x0, y0, x1, y1], \
kind (character, prop, background, vehicle, animal, effect, other), and facing direction. \
Also specify the ground horizon line ground_y (between 0.0 and 1.0) and camera framing (wide, medium, close). \
Respond strictly in JSON according to the schema.";

    let user_prompt = "Analyze this reference animation scene and output the structured JSON layout.";
    let schema = scene_decomposition_schema();

    let mut layout: DetectedSceneLayout = llm.chat_json(
        model,
        system_prompt,
        user_prompt,
        &[png_bytes],
        schema,
    )?;
    if let Ok((w, h)) = image::ImageReader::new(std::io::Cursor::new(png_bytes))
        .with_guessed_format()
        .map_err(|_| ())
        .and_then(|r| r.into_dimensions().map_err(|_| ()))
    {
        normalise_boxes(&mut layout, w, h);
    }

    Ok(layout)
}

/// Candidate asset from the library to match against.
#[derive(Debug, Clone)]
pub struct LibraryAssetCandidate {
    pub id: SymbolId,
    pub name: String,
    pub tags: Vec<String>,
    pub bounds: Rect,
    pub visual_embedding: Option<Vec<f32>>,
}

/// Minimum SigLIP cosine between a detected object's crop and a candidate's
/// thumbnail for the pair to count as a match (F7.1). SigLIP image-image
/// cosines sit high even for unrelated pictures (F6a: 0.79 different vs 0.99
/// same), so the floor is high too; tuned on bhatta2 (Status table, F7.1).
pub const SIGLIP_MATCH_MIN: f64 = 0.80;

/// Minimum name/tag score for a match when there are no embeddings.
pub const NAME_MATCH_MIN: f64 = 0.35;

/// How much the name/tag score adds on top of the SigLIP cosine: enough to
/// break a near-tie between two look-alikes, never enough to carry a match.
pub const NAME_WEIGHT_WITH_SIGLIP: f64 = 0.05;

/// The name/tag half of the score (C8 names and tags against the VLM label).
pub fn name_tag_score(detected: &DetectedSceneObject, candidate: &LibraryAssetCandidate) -> f64 {
    let d_label = detected.label.to_lowercase().replace(['_', '-'], " ");
    let c_name = candidate.name.to_lowercase().replace(['_', '-'], " ");

    let mut score = 0.0;

    // Exact or substring match in name
    if c_name == d_label {
        score += 1.0;
    } else if !c_name.is_empty() && (c_name.contains(&d_label) || d_label.contains(&c_name)) {
        score += 0.7;
    } else {
        // Word token overlap
        let d_words: Vec<&str> = d_label.split_whitespace().collect();
        let c_words: Vec<&str> = c_name.split_whitespace().collect();
        let common = d_words.iter().filter(|w| c_words.contains(w)).count();
        if common > 0 {
            score += 0.4 * (common as f64 / d_words.len().max(1) as f64);
        }
    }

    // Tag matching (C8 tags)
    for tag in &candidate.tags {
        let t_norm = tag.to_lowercase();
        if d_label.split_whitespace().any(|w| w == t_norm) {
            score += 0.4;
        } else if d_label.contains(&t_norm) || detected.kind.eq_ignore_ascii_case(&t_norm) {
            score += 0.25;
        }
    }
    score
}

/// SigLIP cosine between a crop embedding and a candidate's, when both exist.
pub fn visual_cosine(detected_embedding: Option<&[f32]>, candidate: &LibraryAssetCandidate) -> Option<f64> {
    let (d, c) = (detected_embedding?, candidate.visual_embedding.as_deref()?);
    if d.len() != c.len() || d.is_empty() {
        return None;
    }
    let (mut dot, mut nd, mut nc) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..d.len() {
        dot += d[i] * c[i];
        nd += d[i] * d[i];
        nc += c[i] * c[i];
    }
    (nd > 0.0 && nc > 0.0).then(|| (dot / (nd.sqrt() * nc.sqrt())) as f64)
}

/// Computes similarity score between a detected object and a library asset candidate.
///
/// **SigLIP first (F7.1).** With both embeddings the score is the crop ↔
/// thumbnail cosine plus a small name/tag term; the VLM's labels are
/// descriptions ("old man with a stick") and library names are not
/// ("OldManSideBody copy 5"), so names alone matched nothing on bhatta2.
/// Without embeddings it falls back to names and tags.
pub fn compute_asset_match_score(
    detected: &DetectedSceneObject,
    candidate: &LibraryAssetCandidate,
    detected_embedding: Option<&[f32]>,
) -> f64 {
    let names = name_tag_score(detected, candidate);
    match visual_cosine(detected_embedding, candidate) {
        Some(cos) => cos + NAME_WEIGHT_WITH_SIGLIP * names.min(1.5),
        None => names,
    }
}

/// Does this candidate pass the match threshold for this object?
fn passes_threshold(
    detected: &DetectedSceneObject,
    candidate: &LibraryAssetCandidate,
    detected_embedding: Option<&[f32]>,
) -> bool {
    match visual_cosine(detected_embedding, candidate) {
        Some(cos) => cos >= SIGLIP_MATCH_MIN,
        None => name_tag_score(detected, candidate) >= NAME_MATCH_MIN,
    }
}

fn collect_instances(obj: &Object, out: &mut std::collections::HashSet<SymbolId>) {
    match &obj.kind {
        ObjectKind::Group(children) => {
            for c in children {
                collect_instances(c, out);
            }
        }
        ObjectKind::Armature(rig) => {
            for p in &rig.parts {
                collect_instances(&p.artwork, out);
            }
        }
        _ => {
            if let Some(inst) = obj.instance() {
                out.insert(inst.symbol);
            }
        }
    }
}

/// **The placeable symbols** (F7.1): what a person would drop on a stage —
/// symbols placed on the stage's own layers anywhere in the timeline, plus
/// library symbols no other symbol uses. Body parts (a palm, a torso) live
/// inside a character and are left out, so they no longer compete with whole
/// characters and props for a detected object.
pub fn placeable_symbols(scene: &Scene) -> Vec<SymbolId> {
    let mut nested = std::collections::HashSet::new();
    for sym in scene.library().iter() {
        for layer in sym.layers.iter() {
            for obj in layer.all_objects() {
                collect_instances(obj, &mut nested);
            }
        }
    }
    let mut on_stage = std::collections::HashSet::new();
    for layer in scene.stage_layers().iter() {
        for obj in layer.all_objects() {
            collect_instances(obj, &mut on_stage);
        }
    }
    scene
        .library()
        .iter()
        .filter(|s| on_stage.contains(&s.id) || !nested.contains(&s.id))
        .filter(|s| s.bounds().is_some_and(|b| b.width() > 1.0 && b.height() > 1.0))
        .map(|s| s.id)
        .collect()
}

/// [`LibraryAssetCandidate`]s for the placeable symbols, without embeddings.
pub fn placeable_candidates(scene: &Scene) -> Vec<LibraryAssetCandidate> {
    placeable_symbols(scene)
        .into_iter()
        .filter_map(|id| scene.library().get(id))
        .map(|sym| LibraryAssetCandidate {
            id: sym.id,
            name: sym.name.clone(),
            tags: sym.tags.clone(),
            bounds: sym.bounds().unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0)),
            visual_embedding: None,
        })
        .collect()
}

/// **The ground truth C10 is scored against** (F7.1): the symbols of the
/// top-level instances actually visible in `frame` — on a painted, visible
/// stage layer, the object itself visible, and at least 2% of the stage's
/// shorter side of it inside the frame the camera shoots.
pub fn visible_top_level_symbols(scene: &Scene, frame: u32) -> std::collections::HashSet<SymbolId> {
    let size = scene.stage().size;
    let stage = Rect::new(0.0, 0.0, size.width, size.height);
    let min_side = size.width.min(size.height) * 0.02;
    let camera = scene.camera_transform(frame);
    let mut out = std::collections::HashSet::new();
    for layer in scene.stage_layers().iter() {
        if !layer.visible || !layer.kind.paints_to_output() {
            continue;
        }
        for obj in layer.objects_at(frame) {
            let Some(inst) = obj.instance() else { continue };
            if !obj.visible {
                continue;
            }
            // Through the frame's camera: a close-up shows a fraction of
            // the stage, and what it crops away is not in the picture.
            let b = buzz_scene::object::transform_rect(camera, scene.resolved_bounds(obj));
            let inter = b.intersect(stage);
            if inter.width() >= min_side && inter.height() >= min_side {
                out.insert(inst.symbol);
            }
        }
    }
    out
}

/// Reconstructs a scene from a detected layout and a library of assets.
pub fn build_scene_from_layout(
    layout: &DetectedSceneLayout,
    candidates: &[LibraryAssetCandidate],
    skeletons_for_crops: &[Option<Skeleton>],
    detected_embeddings: &[Option<Vec<f32>>],
    stage_size: Option<Size>,
) -> ReconstructedScene {
    build_scene_with(layout, candidates, skeletons_for_crops, stage_size, |orig_idx, detected| {
        let detected_embedding = detected_embeddings
            .get(orig_idx)
            .and_then(|e| e.as_ref())
            .map(|v| v.as_slice());
        let mut best: Option<(usize, f64)> = None;
        for (ci, candidate) in candidates.iter().enumerate() {
            let score = compute_asset_match_score(detected, candidate, detected_embedding);
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((ci, score));
            }
        }
        // Match threshold: SigLIP cosine >= SIGLIP_MATCH_MIN when both
        // embeddings exist, else a name/tag score >= NAME_MATCH_MIN.
        match best {
            Some((ci, score)) if passes_threshold(detected, &candidates[ci], detected_embedding) => {
                Ok((ci, score))
            }
            other => Err(format!(
                "No asset matched threshold (best score: {:.2})",
                other.map(|(_, s)| s).unwrap_or(0.0)
            )),
        }
    })
}

/// Builds the scene from a choice already made per detected object (index
/// into `candidates`, or `None` for "no match") — what
/// [`confirm_with_vlm`] returns.
pub fn build_scene_from_choices(
    layout: &DetectedSceneLayout,
    candidates: &[LibraryAssetCandidate],
    skeletons_for_crops: &[Option<Skeleton>],
    choices: &[Option<(usize, f64)>],
    stage_size: Option<Size>,
) -> ReconstructedScene {
    build_scene_with(layout, candidates, skeletons_for_crops, stage_size, |orig_idx, _| {
        match choices.get(orig_idx).copied().flatten() {
            Some((ci, score)) if ci < candidates.len() => Ok((ci, score)),
            _ => Err("The vision model confirmed none of the SigLIP shortlist".to_string()),
        }
    })
}

/// Indices of the `k` candidates with the highest SigLIP cosine to this crop,
/// best first, with their cosines.
pub fn siglip_shortlist(
    detected_embedding: Option<&[f32]>,
    candidates: &[LibraryAssetCandidate],
    k: usize,
) -> Vec<(usize, f64)> {
    let mut scored: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| visual_cosine(detected_embedding, c).map(|v| (i, v)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Two candidates that render identically (a symbol and its `copy`) are
    // one choice, not two: keep the first of each near-identical cosine run
    // with the same name stem.
    let mut out: Vec<(usize, f64)> = Vec::new();
    for (i, v) in scored {
        let stem = |n: &str| n.to_lowercase().replace(" copy", "").trim().to_string();
        if out.iter().any(|&(j, w)| (w - v).abs() < 1e-4 && stem(&candidates[j].name) == stem(&candidates[i].name)) {
            continue;
        }
        out.push((i, v));
        if out.len() == k {
            break;
        }
    }
    out
}

/// 3×5 bitmap digits, row-major, for numbering the choice sheet's panels.
const DIGITS: [[u8; 15]; 10] = [
    [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1],
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1],
    [1, 1, 1, 0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1],
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1],
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
];

fn draw_number(img: &mut image::RgbaImage, n: usize, x: u32, y: u32, scale: u32) {
    let text = n.to_string();
    let (bw, bh) = ((text.len() as u32) * 4 * scale + scale, 7 * scale);
    for yy in y..(y + bh).min(img.height()) {
        for xx in x..(x + bw).min(img.width()) {
            img.put_pixel(xx, yy, image::Rgba([255, 255, 255, 255]));
        }
    }
    for (k, ch) in text.chars().enumerate() {
        let d = ch.to_digit(10).unwrap_or(0) as usize;
        for r in 0..5u32 {
            for c in 0..3u32 {
                if DIGITS[d][(r * 3 + c) as usize] == 1 {
                    let ox = x + scale + (k as u32) * 4 * scale + c * scale;
                    let oy = y + scale + r * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            if ox + dx < img.width() && oy + dy < img.height() {
                                img.put_pixel(ox + dx, oy + dy, image::Rgba([200, 0, 0, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One picture for the vision model: the object's crop on the left (a blue
/// frame), then each candidate thumbnail in a numbered panel (1, 2, …).
/// Numbers drawn *on* the panels are far more reliable than "the third image
/// after the first", which the models mixed up.
pub fn choice_sheet(crop: &image::RgbaImage, thumbnails: &[image::RgbaImage]) -> image::RgbaImage {
    const CELL: u32 = 256;
    const GAP: u32 = 12;
    let cols = thumbnails.len() as u32 + 1;
    let mut sheet = image::RgbaImage::from_pixel(cols * CELL + (cols - 1) * GAP, CELL, image::Rgba([128, 128, 128, 255]));
    let fit = |img: &image::RgbaImage| {
        let (w, h) = (img.width().max(1) as f64, img.height().max(1) as f64);
        let s = (CELL as f64 / w).min(CELL as f64 / h);
        image::imageops::resize(
            img,
            ((w * s).round() as u32).max(1),
            ((h * s).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    };
    let place = |sheet: &mut image::RgbaImage, img: &image::RgbaImage, col: u32| {
        let f = fit(img);
        let x = col * (CELL + GAP) + (CELL - f.width()) / 2;
        let y = (CELL - f.height()) / 2;
        let bg = image::RgbaImage::from_pixel(CELL, CELL, image::Rgba([0, 0, 0, 255]));
        image::imageops::overlay(sheet, &bg, (col * (CELL + GAP)) as i64, 0);
        image::imageops::overlay(sheet, &f, x as i64, y as i64);
    };
    place(&mut sheet, crop, 0);
    for yy in 0..CELL {
        for t in 0..4 {
            sheet.put_pixel(t, yy, image::Rgba([40, 90, 255, 255]));
            sheet.put_pixel(CELL - 1 - t, yy, image::Rgba([40, 90, 255, 255]));
        }
    }
    for (i, t) in thumbnails.iter().enumerate() {
        let col = i as u32 + 1;
        place(&mut sheet, t, col);
        draw_number(&mut sheet, i + 1, col * (CELL + GAP) + 4, 4, 6);
    }
    sheet
}

/// **The vision model confirms SigLIP's shortlist** (F7.1). SigLIP ranks
/// every placeable symbol against the object's crop; the model then sees one
/// [`choice_sheet`] — the crop, then the top candidates numbered 1..k — and
/// picks the number of the same asset, or 0 for none. One small call per
/// object.
///
/// Returns the chosen shortlist position.
pub fn confirm_with_vlm(
    llm: &impl Llm,
    model: &str,
    label: &str,
    crop: &image::RgbaImage,
    shortlist: &[image::RgbaImage],
) -> Result<Option<usize>, LlmError> {
    if shortlist.is_empty() {
        return Ok(None);
    }
    let sheet = choice_sheet(crop, shortlist);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(sheet)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| LlmError::Other(e.to_string()))?;
    let system = "You match objects in an animation frame to library assets. You get one picture: \
the leftmost panel (blue border) is a crop of one object from a frame; each other panel is one \
library asset rendered alone on black, numbered in its top-left corner. Pick the number of the \
panel that is the same drawn asset — the same character (face, clothes, colours) or the same prop \
or scenery — allowing a different pose, scale, crop or background. If none is, answer 0.";
    let user = format!(
        "The object was described as \"{label}\". Candidates are numbered 1 to {}. Return {{\"choice\": n}}.",
        shortlist.len()
    );
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "choice": { "type": "integer", "minimum": 0 } },
        "required": ["choice"],
        "additionalProperties": false
    });
    #[derive(Deserialize)]
    struct Choice {
        choice: i64,
    }
    let c: Choice = llm.chat_json(model, system, &user, &[png.as_slice()], schema)?;
    Ok((c.choice >= 1 && (c.choice as usize) <= shortlist.len()).then(|| c.choice as usize - 1))
}

fn build_scene_with(
    layout: &DetectedSceneLayout,
    candidates: &[LibraryAssetCandidate],
    skeletons_for_crops: &[Option<Skeleton>],
    stage_size: Option<Size>,
    mut pick: impl FnMut(usize, &DetectedSceneObject) -> Result<(usize, f64), String>,
) -> ReconstructedScene {
    let stage = stage_size.unwrap_or(Size::new(1920.0, 1080.0));
    let mut scene = Scene::default();
    scene.stage_mut().size = stage;

    let mut matched_reports = Vec::new();
    let mut unmatched_reports = Vec::new();

    // Sort detected objects by ground contact (bbox[3] bottom Y) ascending
    // so background/distant objects are placed first, and closer objects naturally appear in front
    let mut indexed_objects: Vec<(usize, &DetectedSceneObject)> =
        layout.objects.iter().enumerate().collect();
    indexed_objects.sort_by(|a, b| a.1.bbox[3].partial_cmp(&b.1.bbox[3]).unwrap_or(std::cmp::Ordering::Equal));

    let mut placed_depth = 0usize;
    for (orig_idx, detected) in indexed_objects {
        let (cand, best_score) = match pick(orig_idx, detected) {
            Ok((ci, score)) => (&candidates[ci], score),
            Err(reason) => {
                unmatched_reports.push(UnmatchedObjectReport {
                    label: detected.label.clone(),
                    bbox: detected.bbox,
                    kind: detected.kind.clone(),
                    reason,
                });
                continue;
            }
        };
        let target_x0 = detected.bbox[0] * stage.width;
        let target_y0 = detected.bbox[1] * stage.height;
        let target_x1 = detected.bbox[2] * stage.width;
        let target_y1 = detected.bbox[3] * stage.height;

        let target_w = (target_x1 - target_x0).abs().max(10.0);
        let target_h = (target_y1 - target_y0).abs().max(10.0);

        let cand_w = cand.bounds.width().max(10.0);
        let cand_h = cand.bounds.height().max(10.0);

        let scale_x = target_w / cand_w;
        let scale_y = target_h / cand_h;
        let uniform_scale = scale_x.min(scale_y);

        // Ground contact placement: map the bounds box (which may live anywhere,
        // not just at the origin) so its bottom-centre lands on the target bbox's
        // bottom-centre.
        let bounds_center_x = (cand.bounds.x0 + cand.bounds.x1) * 0.5;
        let bounds_bottom_y = cand.bounds.y1;
        let stage_x = target_x0 + target_w * 0.5 - uniform_scale * bounds_center_x;
        let stage_y = target_y1 - uniform_scale * bounds_bottom_y;

        // Add layer on stage in depth order
        let layer_name = format!("{}_{}", cand.name, placed_depth + 1);
        let layer_id = scene.add_layer(&layer_name, LayerKind::Normal);

        let id = scene.next_object_id();
        let mut instance = Object::instance_of(id, cand.id);

        let mut transform = Affine::translate((stage_x, stage_y)) * Affine::scale(uniform_scale);
        if detected.facing == "left" {
            // Mirror horizontally about the bounds' own centre (not the symbol
            // origin), so the mirrored object still occupies exactly the same box.
            transform = transform
                * Affine::translate((2.0 * bounds_center_x, 0.0))
                * Affine::scale_non_uniform(-1.0, 1.0);
        }
        instance.transform = transform;

        // If this is a character with a crop skeleton, estimate its pose so the
        // caller can apply it to the placed instance's armature (we can't know
        // here whether the matched symbol is rigged).
        let pose = match skeletons_for_crops.get(orig_idx) {
            Some(Some(skel)) => Some(pose_from_skeleton(skel, &format!("{}_pose", detected.label))),
            _ => None,
        };

        scene.add_object(layer_id, instance);

        matched_reports.push(MatchedObjectReport {
            label: detected.label.clone(),
            symbol_id: cand.id.0,
            symbol_name: cand.name.clone(),
            stage_position: (stage_x, stage_y),
            scale: uniform_scale,
            depth_index: placed_depth,
            match_score: best_score,
            pose,
        });

        placed_depth += 1;
    }

    ReconstructedScene {
        scene,
        layout: layout.clone(),
        matched: matched_reports,
        unmatched: unmatched_reports,
    }
}

/// The object's crop of the reference frame, from its normalised bbox — what
/// SigLIP compares against each candidate's thumbnail (F7.1). Tried and
/// dropped: matting the crop with ISNet first. On a finished cartoon frame
/// ISNet keeps 70% of the crop (the scenery reads as "figure" too), and the
/// cosines did not move.
pub fn crop_for_matching(reference: &image::RgbaImage, object: &DetectedSceneObject) -> image::RgbaImage {
    let (rw, rh) = (reference.width() as f64, reference.height() as f64);
    let x0 = (object.bbox[0] * rw).clamp(0.0, rw) as u32;
    let y0 = (object.bbox[1] * rh).clamp(0.0, rh) as u32;
    let x1 = (object.bbox[2] * rw).clamp(0.0, rw) as u32;
    let y1 = (object.bbox[3] * rh).clamp(0.0, rh) as u32;
    let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    image::imageops::crop_imm(reference, x0, y0, w, h).to_image()
}

/// True for a picture with nothing to compare: under 8 px on a side, or one
/// flat colour (a symbol that rendered empty, a crop of a blank strip).
pub fn is_blank(img: &image::RgbaImage) -> bool {
    if img.width() < 8 || img.height() < 8 {
        return true;
    }
    let first = img.get_pixel(0, 0);
    img.pixels().all(|p| {
        (0..4).all(|i| (p[i] as i32 - first[i] as i32).abs() <= 6)
    })
}

/// Bring a layout's boxes to 0..=1. Vision models asked for normalised boxes
/// often answer on a 0..1000 grid (Gemma, Qwen-VL) or in pixels; either way
/// every crop clamped to one edge, a sliver of nothing (F7.10 run).
pub fn normalise_boxes(layout: &mut DetectedSceneLayout, image_w: u32, image_h: u32) {
    let max = layout
        .objects
        .iter()
        .flat_map(|o| o.bbox)
        .fold(0.0f64, f64::max);
    if max <= 1.5 {
        return;
    }
    // Up to 1000: a 0..1000 grid. Beyond: pixels of the image sent.
    let (sx, sy) = if max <= 1000.0 {
        (1000.0, 1000.0)
    } else {
        (image_w.max(1) as f64, image_h.max(1) as f64)
    };
    for o in &mut layout.objects {
        o.bbox = [
            (o.bbox[0] / sx).clamp(0.0, 1.0),
            (o.bbox[1] / sy).clamp(0.0, 1.0),
            (o.bbox[2] / sx).clamp(0.0, 1.0),
            (o.bbox[3] / sy).clamp(0.0, 1.0),
        ];
    }
    if layout.ground_y > 1.5 {
        layout.ground_y = (layout.ground_y / sy).clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod box_tests {
    use super::*;

    #[test]
    fn boxes_on_a_1000_grid_or_in_pixels_become_normalised() {
        let obj = |b: [f64; 4]| DetectedSceneObject { label: "x".into(), bbox: b, kind: "prop".into(), facing: "na".into() };
        let mut l = DetectedSceneLayout { objects: vec![obj([310.0, 190.0, 650.0, 1000.0])], ground_y: 0.8, camera: "wide".into() };
        normalise_boxes(&mut l, 960, 540);
        assert_eq!(l.objects[0].bbox, [0.31, 0.19, 0.65, 1.0]);
        let mut l = DetectedSceneLayout { objects: vec![obj([96.0, 54.0, 1920.0, 1080.0])], ground_y: 0.8, camera: "wide".into() };
        normalise_boxes(&mut l, 1920, 1080);
        assert_eq!(l.objects[0].bbox, [0.05, 0.05, 1.0, 1.0]);
        let mut l = DetectedSceneLayout { objects: vec![obj([0.1, 0.2, 0.3, 0.4])], ground_y: 0.8, camera: "wide".into() };
        normalise_boxes(&mut l, 960, 540);
        assert_eq!(l.objects[0].bbox, [0.1, 0.2, 0.3, 0.4]);
        assert!(is_blank(&image::RgbaImage::new(4, 40)));
        assert!(is_blank(&image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 255]))));
    }
}
