//! DWPose whole-body 133-keypoint detection pipeline.

use anyhow::{Context, Result};
use image::{imageops, RgbaImage};
use serde::{Deserialize, Serialize};
use crate::models::{models_dir, ModelId};
use crate::onnx::{image_to_nchw, pooled, OnnxSession};

mod keypoints_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(kps: &[(f32, f32, f32); 133], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        kps.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[(f32, f32, f32); 133], D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<(f32, f32, f32)>::deserialize(deserializer)?;
        if vec.len() != 133 {
            return Err(serde::de::Error::invalid_length(vec.len(), &"133 keypoints"));
        }
        let mut arr = [(0.0, 0.0, 0.0); 133];
        arr.copy_from_slice(&vec);
        Ok(arr)
    }
}

/// A detected person with a bounding box and 133 whole-body keypoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    /// Bounding box in original image pixel coordinates: `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// 133 keypoints formatted as `(x, y, confidence)` in original image space.
    /// COCO-WholeBody convention:
    /// - 0..17: Body keypoints (nose, eyes, ears, shoulders, elbows, wrists, hips, knees, ankles)
    /// - 17..23: Feet keypoints (big toe, small toe, heel for L & R)
    /// - 23..91: Face 68 keypoints
    /// - 91..112: Left hand 21 keypoints
    /// - 112..133: Right hand 21 keypoints
    #[serde(with = "keypoints_serde")]
    pub keypoints: [(f32, f32, f32); 133],
}

impl Person {
    /// Character height computed from bounding box or head-to-ankle span.
    pub fn character_height(&self) -> f32 {
        let bbox_h = (self.bbox[3] - self.bbox[1]).abs();
        if bbox_h > 1.0 {
            bbox_h
        } else {
            // Distance between nose (0) and ankles (15, 16)
            let nose_y = self.keypoints[0].1;
            let ankle_y = (self.keypoints[15].1 + self.keypoints[16].1) / 2.0;
            (ankle_y - nose_y).abs().max(10.0)
        }
    }
}

/// Detects persons and whole-body keypoints in an image using DWPose.
pub fn detect(img: &RgbaImage) -> Result<Vec<Person>> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }

    // Sessions come from the process-wide pool (loaded once, reused across
    // calls) when the model is on disk; otherwise fall back to an ad hoc
    // load so the existing "not found" error still surfaces for the
    // required estimator.
    let detector = pooled(ModelId::DwPoseDetector);
    if let Some(estimator) = pooled(ModelId::DwPoseEstimator) {
        let mut guard = estimator.lock().unwrap();
        return detect_with_sessions(img, w, h, &mut guard, detector);
    }

    let estimator_path = models_dir().join("dw-ll_ucoco_384.onnx");
    if !estimator_path.exists() {
        anyhow::bail!(
            "DWPose estimator model {:?} not found. Please download models via AI Models dialog.",
            estimator_path
        );
    }
    let mut local_estimator = OnnxSession::load(&estimator_path)?;
    detect_with_sessions(img, w, h, &mut local_estimator, detector)
}

/// Runs person-box detection (pooled if available, else an ad hoc load) and
/// whole-body keypoint estimation for every detected box.
fn detect_with_sessions(
    img: &RgbaImage,
    w: u32,
    h: u32,
    estimator_session: &mut OnnxSession,
    pooled_detector: Option<std::sync::Arc<std::sync::Mutex<OnnxSession>>>,
) -> Result<Vec<Person>> {
    let mut bboxes = Vec::new();
    match pooled_detector {
        Some(s) => {
            if let Ok(boxes) = run_detector(img, &mut s.lock().unwrap()) {
                bboxes = boxes;
            }
        }
        None => {
            let detector_path = models_dir().join("yolox_l.onnx");
            if detector_path.exists() {
                if let Ok(mut detector_session) = OnnxSession::load(&detector_path) {
                    if let Ok(boxes) = run_detector(img, &mut detector_session) {
                        bboxes = boxes;
                    }
                }
            }
        }
    }

    if bboxes.is_empty() {
        // Fallback box: full image with a 5% margin
        let margin_x = w as f32 * 0.02;
        let margin_y = h as f32 * 0.02;
        bboxes.push([margin_x, margin_y, w as f32 - margin_x, h as f32 - margin_y]);
    }

    let mut persons = Vec::new();
    for bbox in bboxes {
        if std::env::var_os("BUZZ_AI_DEBUG").is_some() {
            eprintln!("[pose] image {w}x{h}, bbox {bbox:?}");
        }
        if let Ok(person) = estimate_person(img, bbox, estimator_session) {
            persons.push(person);
        }
    }

    Ok(persons)
}

/// Builds the (grid_x, grid_y, stride) for each YOLOX anchor, in the same
/// concatenation order as the raw ONNX output (strides 8, 16, 32, each a
/// row-major grid over `input_size / stride` cells).
fn yolox_grid_and_stride(input_size: u32, num_boxes: usize) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::with_capacity(num_boxes);
    for &stride in &[8u32, 16, 32] {
        let fs = input_size / stride;
        for gy in 0..fs {
            for gx in 0..fs {
                out.push((gx as f32, gy as f32, stride as f32));
            }
        }
    }
    out
}

/// Runs person bounding box detection using YOLOX.
fn run_detector(img: &RgbaImage, session: &mut OnnxSession) -> Result<Vec<[f32; 4]>> {
    let target_size = (640, 640);
    let (nchw, info) = image_to_nchw(
        img,
        target_size,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [114, 114, 114, 255],
    );

    // YOLOX expects float 0..255 input
    let mut nchw_255 = nchw;
    for v in &mut nchw_255 {
        *v *= 255.0;
    }

    let input_array = ndarray::Array4::from_shape_vec((1, 3, 640, 640), nchw_255)
        .context("Failed to reshape detector input")?;
    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| anyhow::anyhow!("Failed to create detector tensor: {e}"))?;

    let outputs = session.inner_mut().run(ort::inputs![input_tensor])
        .map_err(|e| anyhow::anyhow!("Detector inference failed: {e}"))?;

    let (shape, slice) = outputs[0].try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract detector output: {e}"))?;

    // Parse boxes: [1, num_boxes, 85] (x, y, w, h, obj_conf, 80 class probs)
    let dims = shape.as_ref();
    if dims.len() < 3 {
        return Ok(Vec::new());
    }
    let num_boxes = dims[1] as usize;
    let num_features = dims[2] as usize;

    // YOLOX's raw ONNX export is un-decoded: outputs are (dx, dy, dw, dh)
    // regressions per grid cell, not absolute pixel coordinates. They must
    // be decoded against each cell's (grid_x, grid_y, stride), matching the
    // official YOLOX ONNXRuntime demo's `demo_postprocess`. For a 640x640
    // input the 3 detection heads (strides 8, 16, 32) produce
    // 80x80 + 40x40 + 20x20 = 8400 anchors, concatenated in that order.
    let grid_and_stride = yolox_grid_and_stride(target_size.0, num_boxes);

    let mut best_box: Option<([f32; 4], f32)> = None;

    for b in 0..num_boxes {
        let offset = b * num_features;
        if offset + 5 >= slice.len() {
            break;
        }
        let (gx, gy, stride) = grid_and_stride.get(b).copied().unwrap_or((0.0, 0.0, 1.0));
        let cx = (slice[offset] + gx) * stride;
        let cy = (slice[offset + 1] + gy) * stride;
        let bw = slice[offset + 2].exp() * stride;
        let bh = slice[offset + 3].exp() * stride;
        let obj_conf = slice[offset + 4];
        let cls_conf = if num_features > 5 { slice[offset + 5] } else { 1.0 }; // class 0 is person
        let score = obj_conf * cls_conf;

        if score > 0.3 {
            let x0 = cx - bw / 2.0;
            let y0 = cy - bh / 2.0;
            let x1 = cx + bw / 2.0;
            let y1 = cy + bh / 2.0;

            let (ux0, uy0) = info.unletterbox_point(x0, y0);
            let (ux1, uy1) = info.unletterbox_point(x1, y1);

            if let Some((_, best_score)) = best_box {
                if score > best_score {
                    best_box = Some(([ux0, uy0, ux1, uy1], score));
                }
            } else {
                best_box = Some(([ux0, uy0, ux1, uy1], score));
            }
        }
    }

    Ok(best_box.map(|(b, _)| vec![b]).unwrap_or_default())
}

/// Runs whole-body keypoint estimation on a cropped person bounding box.
fn estimate_person(
    img: &RgbaImage,
    bbox: [f32; 4],
    session: &mut OnnxSession,
) -> Result<Person> {
    let img_w = img.width() as f32;
    let img_h = img.height() as f32;

    // Crop box with padding (aspect ratio 3:4 for 288x384 input)
    let bx0 = bbox[0].clamp(0.0, img_w);
    let by0 = bbox[1].clamp(0.0, img_h);
    let bx1 = bbox[2].clamp(0.0, img_w);
    let by1 = bbox[3].clamp(0.0, img_h);

    let bw = (bx1 - bx0).max(10.0);
    let bh = (by1 - by0).max(10.0);
    let cx = bx0 + bw / 2.0;
    let cy = by0 + bh / 2.0;

    // Expand bounding box to match 3:4 aspect ratio
    let target_ar = 288.0 / 384.0; // 0.75
    let (crop_w, crop_h) = if bw / bh > target_ar {
        (bw * 1.2, (bw * 1.2) / target_ar)
    } else {
        (bh * 1.2 * target_ar, bh * 1.2)
    };

    let cx0 = (cx - crop_w / 2.0).max(0.0).floor() as u32;
    let cy0 = (cy - crop_h / 2.0).max(0.0).floor() as u32;
    let cx1 = (cx + crop_w / 2.0).min(img_w).ceil() as u32;
    let cy1 = (cy + crop_h / 2.0).min(img_h).ceil() as u32;

    let cw = (cx1 - cx0).max(1);
    let ch = (cy1 - cy0).max(1);

    let crop = imageops::crop_imm(img, cx0, cy0, cw, ch).to_image();

    // Input shape for DWPose is width: 288, height: 384
    let (nchw, info) = image_to_nchw(
        &crop,
        (288, 384),
        [0.485, 0.456, 0.406],
        [0.229, 0.224, 0.225],
        [0, 0, 0, 255],
    );

    let input_array = ndarray::Array4::from_shape_vec((1, 3, 384, 288), nchw)
        .context("Failed to reshape keypoint estimator input into (1, 3, 384, 288)")?;
    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| anyhow::anyhow!("Failed to create estimator tensor: {e}"))?;

    let outputs = session.inner_mut().run(ort::inputs![input_tensor])
        .map_err(|e| anyhow::anyhow!("Estimator inference failed: {e}"))?;

    // SimCC outputs: simcc_x and simcc_y
    let (shape_x, slice_x) = outputs[0].try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract simcc_x: {e}"))?;
    let (shape_y, slice_y) = outputs[1].try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract simcc_y: {e}"))?;

    let dims_x = shape_x.as_ref();
    let dims_y = shape_y.as_ref();
    let num_kpts = 133;
    let bins_x = if dims_x.len() >= 3 { dims_x[2] as usize } else { 576 };
    let bins_y = if dims_y.len() >= 3 { dims_y[2] as usize } else { 768 };
    if std::env::var_os("BUZZ_AI_DEBUG").is_some() {
        eprintln!(
            "[pose] crop ({cx0},{cy0})..({cx1},{cy1}) = {cw}x{ch}; dims_x={dims_x:?} dims_y={dims_y:?} bins_x={bins_x} bins_y={bins_y}"
        );
    }

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];

    for k in 0..num_kpts {
        let offset_x = k * bins_x;
        let offset_y = k * bins_y;

        // Argmax on x
        let mut max_x_val = f32::NEG_INFINITY;
        let mut best_x_bin = 0;
        for i in 0..bins_x {
            let v = slice_x[offset_x + i];
            if v > max_x_val {
                max_x_val = v;
                best_x_bin = i;
            }
        }

        // Argmax on y
        let mut max_y_val = f32::NEG_INFINITY;
        let mut best_y_bin = 0;
        for i in 0..bins_y {
            let v = slice_y[offset_y + i];
            if v > max_y_val {
                max_y_val = v;
                best_y_bin = i;
            }
        }

        let lx = (best_x_bin as f32 / bins_x as f32) * 288.0;
        let ly = (best_y_bin as f32 / bins_y as f32) * 384.0;

        let (crop_px, crop_py) = info.unletterbox_point(lx, ly);
        let orig_x = crop_px + cx0 as f32;
        let orig_y = crop_py + cy0 as f32;

        let conf = (1.0 / (1.0 + (-max_x_val).exp()) + 1.0 / (1.0 + (-max_y_val).exp())) / 2.0;
        keypoints[k] = (orig_x, orig_y, conf);
        if std::env::var_os("BUZZ_AI_DEBUG").is_some() && k < 3 {
            eprintln!(
                "[pose] kp[{k}]: bin=({best_x_bin},{best_y_bin}) local=({lx:.1},{ly:.1}) crop_px=({crop_px:.1},{crop_py:.1}) orig=({orig_x:.1},{orig_y:.1}) conf={conf:.3}"
            );
        }
    }

    Ok(Person { bbox, keypoints })
}
