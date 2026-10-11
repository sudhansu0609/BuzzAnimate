//! Character pipeline stages (skeleton, masks, completion, trace, rig).
//!
//! Usage:
//!   cargo run --release -p buzz-app --example ai_character -- <image.png> <out_dir> [--stage skeleton]

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};

fn draw_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && px < w && py >= 0 && py < h {
                    img.put_pixel(px as u32, py as u32, Rgba(color));
                }
            }
        }
    }
}

fn draw_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, thickness: i32, color: [u8; 4]) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        draw_circle(img, x, y, thickness, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

fn detect_or_load_skeleton(img: &RgbaImage, input_path: &Path) -> Result<buzz_ai::character::Skeleton> {
    match buzz_ai::pose::detect(img) {
        Ok(persons) if !persons.is_empty() => {
            println!("DWPose detected person with bbox {:?}", persons[0].bbox);
            return Ok(buzz_ai::character::skeleton(&persons[0]));
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("DWPose inference: {e}");
        }
    }

    let stem = input_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let sidecar = input_path.with_file_name(format!("{stem}.joints.json"));
    if sidecar.exists() {
        println!("Loading ground-truth skeleton from sidecar {:?}", sidecar);
        let content = std::fs::read_to_string(&sidecar)?;
        let joints: Vec<(String, (f64, f64))> = serde_json::from_str(&content)?;
        let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
        for (name, (x, y)) in &joints {
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
        let person = buzz_ai::pose::Person {
            bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
            keypoints,
        };
        return Ok(buzz_ai::character::skeleton(&person));
    }

    anyhow::bail!("No person detected and no sidecar .joints.json found for {:?}", input_path);
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let stage = if let Some(pos) = args.iter().position(|a| a == "--stage") {
        if pos + 1 < args.len() {
            let s = args.remove(pos + 1);
            args.remove(pos);
            s
        } else {
            args.remove(pos);
            "skeleton".to_string()
        }
    } else {
        "skeleton".to_string()
    };

    let usage = "usage: ai_character <image.png> <out_dir> [--stage skeleton|parts|complete|trace|assemble]";
    if args.len() < 2 {
        anyhow::bail!("{usage}");
    }

    let input_path = PathBuf::from(&args[0]);
    let out_dir = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&out_dir)?;

    println!("Loading image {:?}...", input_path);
    let img = image::open(&input_path)
        .with_context(|| format!("Failed to open image at {:?}", input_path))?
        .to_rgba8();

    match stage.as_str() {
        "skeleton" => {
            let skel = detect_or_load_skeleton(&img, &input_path)?;
            let json_path = out_dir.join("skeleton.json");
            let json_str = serde_json::to_string_pretty(&skel)?;
            std::fs::write(&json_path, json_str)?;
            println!("Wrote skeleton data -> {:?}", json_path);

            // Draw skeleton on image
            let mut overlay = img.clone();

            // Bone colors (teal / cyan)
            let bone_color = [0, 230, 200, 255];
            for bone in &skel.bones {
                if bone.confidence > 0.2 {
                    draw_line(
                        &mut overlay,
                        bone.head.0.round() as i32,
                        bone.head.1.round() as i32,
                        bone.tail.0.round() as i32,
                        bone.tail.1.round() as i32,
                        2,
                        bone_color,
                    );
                }
            }

            // Joint circles (yellow with border)
            let joint_fill = [255, 220, 0, 255];
            let joint_border = [20, 20, 20, 255];
            for joint in &skel.joints {
                if joint.confidence > 0.2 {
                    let jx = joint.x.round() as i32;
                    let jy = joint.y.round() as i32;
                    draw_circle(&mut overlay, jx, jy, 4, joint_border);
                    draw_circle(&mut overlay, jx, jy, 3, joint_fill);
                }
            }

            let out_png = out_dir.join("skeleton.png");
            overlay.save(&out_png)?;
            println!("Wrote skeleton overlay -> {:?}", out_png);
        }
        "parts" => {
            println!("Generating matte and skeleton for parts extraction...");
            let matte = buzz_ai::segment::matte(&img, None)
                .context("Failed to compute character matte")?;
            let skel = detect_or_load_skeleton(&img, &input_path)?;
            println!("Segmenting character part masks...");
            let part_masks = buzz_ai::character::part_masks(&img, &matte, &skel)?;

            let parts_png_path = out_dir.join("parts.png");
            let palette_img = part_masks.color_palette_image();
            palette_img.save(&parts_png_path)?;
            println!("Wrote color-coded parts visualization -> {:?}", parts_png_path);

            for part in &part_masks.parts {
                if let Some(alpha_img) = part_masks.part_alpha_image(&part.name, &img) {
                    let sanitized = part.name.replace(' ', "_").to_lowercase();
                    let part_file = out_dir.join(format!("{sanitized}.png"));
                    alpha_img.save(&part_file)?;
                    println!("Wrote part alpha PNG -> {:?}", part_file);
                }
            }
            println!("Successfully generated {} part masks.", part_masks.parts.len());
        }
        "complete" => {
            println!("Generating matte and skeleton for completion...");
            let matte = buzz_ai::segment::matte(&img, None)
                .context("Failed to compute character matte")?;
            let skel = detect_or_load_skeleton(&img, &input_path)?;
            println!("Segmenting character part masks...");
            let part_masks = buzz_ai::character::part_masks(&img, &matte, &skel)?;
            println!("Completing hidden regions and joint caps...");
            let completed = buzz_ai::character::complete_parts(&img, &matte, &skel, &part_masks)?;

            for part in &completed.parts {
                let sanitized = part.name.replace(' ', "_").to_lowercase();
                let out_file = out_dir.join(format!("completed_{sanitized}.png"));
                let part_img = part.to_image();
                part_img.save(&out_file)?;
                println!("Wrote completed part PNG -> {:?}", out_file);
            }
            println!("Successfully completed {} character parts.", completed.parts.len());
        }
        "trace" => {
            println!("Generating matte and skeleton for tracing...");
            let matte = buzz_ai::segment::matte(&img, None)
                .context("Failed to compute character matte")?;
            let skel = detect_or_load_skeleton(&img, &input_path)?;
            println!("Segmenting character part masks...");
            let part_masks = buzz_ai::character::part_masks(&img, &matte, &skel)?;
            println!("Completing hidden regions and joint caps...");
            let completed = buzz_ai::character::complete_parts(&img, &matte, &skel, &part_masks)?;
            println!("Tracing character parts with equal-width lines and quantized palette...");
            let traced = buzz_ai::character::trace_character_whole(&completed, &skel, &matte, None)?;

            // Save full character SVG
            let char_svg_path = out_dir.join("character.svg");
            let char_svg = traced.to_svg(completed.width, completed.height);
            std::fs::write(&char_svg_path, char_svg)?;
            println!("Wrote full character SVG -> {:?}", char_svg_path);

            // Save individual part SVGs
            let mut total_shapes = 0;
            let mut total_strokes = 0;
            let mut total_fills = 0;

            for part in &traced.parts {
                total_shapes += part.shapes.len();
                for shape in &part.shapes {
                    if shape.stroke.is_some() {
                        total_strokes += 1;
                    }
                    if shape.fill.is_some() {
                        total_fills += 1;
                    }
                }
                let sanitized = part.name.replace(' ', "_").to_lowercase();
                let part_svg_path = out_dir.join(format!("traced_{sanitized}.svg"));
                let part_svg = part.to_svg(completed.width, completed.height);
                std::fs::write(&part_svg_path, part_svg)?;
                println!("Wrote traced part SVG -> {:?}", part_svg_path);
            }

            // Save palette JSON
            let palette_hex: Vec<String> = traced
                .palette
                .iter()
                .map(|c| {
                    let rgba = c.to_rgba8();
                    format!("#{:02X}{:02X}{:02X}", rgba.r, rgba.g, rgba.b)
                })
                .collect();
            let pal_path = out_dir.join("palette.json");
            std::fs::write(&pal_path, serde_json::to_string_pretty(&palette_hex)?)?;
            println!("Wrote palette JSON ({} colors) -> {:?}", palette_hex.len(), pal_path);

            println!(
                "Tracing complete: {} parts, {} shapes ({} strokes, {} fills), uniform width = {:.1}px",
                traced.parts.len(),
                total_shapes,
                total_strokes,
                total_fills,
                traced.uniform_width
            );
        }
        "assemble" => {
            println!("Detecting skeleton and matte...");
            let matte = buzz_ai::segment::matte(&img, None)?;
            let skel = detect_or_load_skeleton(&img, &input_path)?;
            println!("Segmenting parts...");
            let parts = buzz_ai::character::part_masks(&img, &matte, &skel)?;
            println!("Completing hidden parts...");
            let completed = buzz_ai::character::complete_parts(&img, &matte, &skel, &parts)?;
            println!("Tracing parts into uniform-width vectors...");
            let traced = buzz_ai::character::trace_character_whole(&completed, &skel, &matte, Some(2.0))?;

            let stem = input_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Character");
            println!("Assembling and rigging character '{}'...", stem);
            let assembled = buzz_ai::character::assemble_character(
                &skel,
                &traced,
                stem,
                Some((img.width() as f64, img.height() as f64)),
                None,
            )?;

            // Save .buzz document
            let buzz_path = out_dir.join(format!("{stem}.buzz"));
            buzz_doc::format::save(&assembled.scene, &buzz_path)?;
            println!("Wrote assembled character document -> {:?}", buzz_path);

            // Also save to Cast in an asset library within out_dir
            let cast_dir = out_dir.join("library");
            let mut assets = buzz_doc::AssetLibrary::at(&cast_dir);
            let asset = assembled.save_to_cast(&mut assets)?;
            println!(
                "Saved character to Cast folder: name='{}', folder='{}', path={:?}",
                asset.name, asset.folder, asset.path
            );

            println!(
                "Assembly complete: {} part symbols, {} layers, {} armature bones",
                assembled.part_symbols.len(),
                assembled.character_symbol.layers.len(),
                assembled.armature.armature.len()
            );
        }
        unknown => {
            anyhow::bail!("Unknown stage: {unknown}");
        }
    }

    Ok(())
}
