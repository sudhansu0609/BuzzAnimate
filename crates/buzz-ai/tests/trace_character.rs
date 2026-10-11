use buzz_ai::character::{complete_parts, part_masks, skeleton, trace_character, trace_character_whole};
use buzz_ai::pose::Person;
use buzz_ai::segment::matte;
use buzz_geom::{BezPath, PathEl, Point, Shape};

fn flatten_points(path: &BezPath) -> Vec<Point> {
    let mut out = Vec::new();
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => out.push(p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => out.push(p),
        PathEl::ClosePath => {}
    });
    out
}

#[test]
fn trace_character_enforces_uniform_stroke_width_and_quantized_palette() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();
    let matte = matte(&img, None).expect("matte");

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("complete parts");

    let target_width = 2.5;
    let traced = trace_character(&completed, Some(target_width)).expect("trace character");

    assert_eq!(traced.uniform_width, target_width);
    assert!(
        traced.palette.len() >= 6 && traced.palette.len() <= 12,
        "palette size {} should be in 6..=12",
        traced.palette.len()
    );

    let mut total_strokes = 0;
    let mut total_fills = 0;

    for part in &traced.parts {
        for shape in &part.shapes {
            if let Some(stroke) = &shape.stroke {
                total_strokes += 1;
                assert!(
                    (stroke.width - target_width).abs() < 1e-4,
                    "stroke width {} in part {} does not match uniform_width {}",
                    stroke.width,
                    part.name,
                    target_width
                );
            }
            if let Some(fill) = &shape.fill {
                total_fills += 1;
                let c = fill.color();
                let matches_palette = traced.palette.iter().any(|pc| *pc == c);
                assert!(
                    matches_palette,
                    "fill color {:?} in part {} does not match any palette color",
                    c,
                    part.name
                );
            }
        }
    }

    assert!(total_strokes > 0, "must produce strokes across character parts");
    assert!(total_fills > 0, "must produce fills across character parts");

    // Verify SVG generation produces valid SVG
    let svg = traced.to_svg(completed.width, completed.height);
    assert!(svg.starts_with("<svg"));
    assert!(svg.ends_with("</svg>\n"));
    assert!(svg.contains("stroke-width=\"2.5\""));
}

#[test]
fn traced_strokes_are_smooth_and_close_to_source_lines() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();
    let matte = matte(&img, None).expect("matte");

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced = trace_character(&completed, Some(2.0)).expect("trace character");

    // Check smoothness of traced strokes at 10 px intervals:
    let mut total_10px_segments = 0;
    let mut smooth_segments = 0;

    for part in &traced.parts {
        for shape in &part.shapes {
            if shape.stroke.is_some() {
                let pts = flatten_points(&shape.path);
                if pts.len() < 2 {
                    continue;
                }

                // Compute cumulative arc-length
                let mut cum_len = vec![0.0f64];
                for i in 1..pts.len() {
                    let dx = pts[i].x - pts[i - 1].x;
                    let dy = pts[i].y - pts[i - 1].y;
                    let seg_len = (dx * dx + dy * dy).sqrt();
                    cum_len.push(cum_len.last().unwrap() + seg_len);
                }

                let total_len = *cum_len.last().unwrap();
                if total_len < 20.0 {
                    continue;
                }

                // Resample at 10 px steps
                let step = 10.0;
                let mut sampled = Vec::new();
                let mut curr_dist = 0.0;
                let mut p_idx = 0;

                while curr_dist <= total_len {
                    while p_idx + 1 < cum_len.len() && cum_len[p_idx + 1] < curr_dist {
                        p_idx += 1;
                    }
                    if p_idx + 1 >= cum_len.len() {
                        sampled.push(pts[cum_len.len() - 1]);
                        break;
                    }
                    let seg_start = cum_len[p_idx];
                    let seg_end = cum_len[p_idx + 1];
                    let t = if (seg_end - seg_start).abs() > 1e-6 {
                        (curr_dist - seg_start) / (seg_end - seg_start)
                    } else {
                        0.0
                    };
                    let sx = pts[p_idx].x + t * (pts[p_idx + 1].x - pts[p_idx].x);
                    let sy = pts[p_idx].y + t * (pts[p_idx + 1].y - pts[p_idx].y);
                    sampled.push(Point::new(sx, sy));
                    curr_dist += step;
                }

                if sampled.len() >= 3 {
                    for i in 1..sampled.len() - 1 {
                        let p0 = sampled[i - 1];
                        let p1 = sampled[i];
                        let p2 = sampled[i + 1];

                        let v1x = p1.x - p0.x;
                        let v1y = p1.y - p0.y;
                        let v2x = p2.x - p1.x;
                        let v2y = p2.y - p1.y;

                        let len1 = (v1x * v1x + v1y * v1y).sqrt();
                        let len2 = (v2x * v2x + v2y * v2y).sqrt();

                        if len1 > 1.0 && len2 > 1.0 {
                            let dot = (v1x * v2x + v1y * v2y) / (len1 * len2);
                            let angle_deg = dot.clamp(-1.0, 1.0).acos().to_degrees();
                            total_10px_segments += 1;
                            if angle_deg <= 25.0 {
                                smooth_segments += 1;
                            }
                        }
                    }
                }

                // Check that line points remain within 1.5 px of character boundary or inside matte
                let (w, h) = (matte.width() as usize, matte.height() as usize);
                for p in &pts {
                    let px = (p.x.round() as usize).min(w - 1);
                    let py = (p.y.round() as usize).min(h - 1);
                    // Point must be on or near the character matte
                    let mut near_matte = false;
                    for dy in -2..=2 {
                        for dx in -2..=2 {
                            let nx = (px as isize + dx).clamp(0, w as isize - 1) as u32;
                            let ny = (py as isize + dy).clamp(0, h as isize - 1) as u32;
                            if matte.get_pixel(nx, ny)[0] > 128 {
                                near_matte = true;
                                break;
                            }
                        }
                        if near_matte {
                            break;
                        }
                    }
                    assert!(
                        near_matte,
                        "line point ({:.1}, {:.1}) in part {} is too far outside the matte",
                        p.x, p.y, part.name
                    );
                }
            }
        }
    }

    assert!(total_10px_segments > 0, "must evaluate 10px segments");
    let smooth_ratio = smooth_segments as f64 / total_10px_segments as f64;
    assert!(
        smooth_ratio >= 0.60,
        "smooth segments ratio ({:.1}%, {}/{}) must be >= 60% (non-corners under 25° per 10px)",
        smooth_ratio * 100.0,
        smooth_segments,
        total_10px_segments
    );
}

fn luminance(c: [u8; 3]) -> f32 {
    0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
}

/// The area (px²) a filled path encloses, via a shoelace sum over its
/// flattened subpaths — an approximation (holes are not subtracted from
/// outer contours) but enough to tell a small ink accent (a button, a
/// stitch, a wrinkle) from a broad patch.
fn approx_area(path: &BezPath) -> f64 {
    fn shoelace(pts: &[Point]) -> f64 {
        if pts.len() < 3 {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            sum += a.x * b.y - b.x * a.y;
        }
        (sum * 0.5).abs()
    }

    let mut total = 0.0;
    let mut current: Vec<Point> = Vec::new();
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        PathEl::MoveTo(p) => {
            total += shoelace(&current);
            current.clear();
            current.push(p);
        }
        PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => current.push(p),
        PathEl::ClosePath => {
            total += shoelace(&current);
            current.clear();
        }
    });
    total += shoelace(&current);
    total
}

/// The "stray dark filled band across pujariFront's kurta hem" regression:
/// `trace_character_whole`'s whole-character `ColourLines` trace must never
/// leave a large fill near the line colour on the chest/hips (the torso
/// clothing) unless the part's own completed pixels under it really are
/// that dark — see `character::region_is_dark`.
#[test]
fn chest_and_hips_have_no_large_dark_fill_unless_the_source_is_dark() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();
    let matte_img = matte(&img, None).expect("matte");

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte_img, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte_img, &skel, &parts).expect("complete parts");
    let traced =
        trace_character_whole(&completed, &skel, &matte_img, Some(2.0)).expect("trace character whole");

    let line_col = traced
        .palette
        .first()
        .copied()
        .unwrap_or(peniko::Color::from_rgb8(15, 15, 15));
    let line_lum = {
        let c = line_col.to_rgba8();
        luminance([c.r, c.g, c.b])
    };

    const AREA_GUARD_PX: f64 = 150.0;
    const LUM_TOLERANCE: f32 = 18.0;
    const SOURCE_DARK_LUM: f64 = 70.0;

    let mut parts_checked = 0;

    for part in &traced.parts {
        let n = part.name.to_lowercase();
        if !(n.contains("chest") || n.contains("hip")) {
            continue;
        }
        let completed_part = completed.get(&part.name).expect("completed part exists");
        parts_checked += 1;

        for shape in &part.shapes {
            let Some(fill) = &shape.fill else { continue };
            let area = approx_area(&shape.path);
            if area <= AREA_GUARD_PX {
                continue;
            }
            let c = fill.color().to_rgba8();
            if luminance([c.r, c.g, c.b]) > line_lum + LUM_TOLERANCE {
                continue;
            }

            let bbox = shape.path.bounding_box();
            let x0 = bbox.x0.floor().max(0.0) as u32;
            let x1 = (bbox.x1.ceil() as i64).clamp(0, completed_part.width as i64) as u32;
            let y0 = bbox.y0.floor().max(0.0) as u32;
            let y1 = (bbox.y1.ceil() as i64).clamp(0, completed_part.height as i64) as u32;
            // Fills now carry the tracer's exact region colour, so a thin black
            // ink line is a legitimate dark fill however much pale cloth shares
            // its bounding box: measure the source *under the shape's own path*
            // (as `character::region_is_dark` does), not over its box.
            let (mut sum, mut count) = (0.0f64, 0u32);
            for py in y0..y1.min(completed_part.height) {
                for px in x0..x1.min(completed_part.width) {
                    if !shape.path.contains(Point::new(px as f64 + 0.5, py as f64 + 0.5)) {
                        continue;
                    }
                    let idx = ((py * completed_part.width + px) as usize) * 4;
                    if completed_part.rgba[idx + 3] > 128 {
                        sum += luminance([
                            completed_part.rgba[idx],
                            completed_part.rgba[idx + 1],
                            completed_part.rgba[idx + 2],
                        ]) as f64;
                        count += 1;
                    }
                }
            }
            let avg_lum = if count > 0 { sum / count as f64 } else { f64::MAX };
            assert!(
                avg_lum <= SOURCE_DARK_LUM,
                "part {} has a {area:.0}px^2 fill near the line colour {c:?} at {bbox:?}, \
                 but the source pixels there average luminance {avg_lum:.1} (> {SOURCE_DARK_LUM}) \
                 — not dark: this is the 'stray dark filled band across pujariFront's kurta \
                 hem' artefact",
                part.name
            );
        }
    }

    assert!(
        parts_checked >= 2,
        "expected to find both a chest and a hips part to check, found {parts_checked}"
    );
}
