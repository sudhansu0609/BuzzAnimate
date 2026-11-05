//! E2 gate: zz_e2_gate <library-root> <out-dir> [ROLE,ROLE]
//! An empty 10 s scene, the Cast characters placed the way the Cast panel does
//! (`scene_life::cast_into_shot`), then 8 stills across the 10 s in one strip, plus the pixel
//! change of the chest band (breath) and the eye band (blinks) against frame 0.
use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::Rect;
use buzz_render::GpuPreference;
use serde_json::json;

fn render(ex: &mut Exporter, scene: &buzz_scene::Scene, frame: u32, region: Rect, width: u32) -> anyhow::Result<Frame> {
    let mut s = ExportSettings::for_stage(scene);
    s.width = width;
    s.height = ((width as f64 * region.height() / region.width().max(1.0)).round() as u32).clamp(16, 2000);
    s.transparent = false;
    s.region = Some(region);
    Ok(ex.render(scene, frame, &s)?)
}

fn changed(a: &Frame, b: &Frame, rows: std::ops::Range<u32>) -> usize {
    let mut n = 0;
    for y in rows {
        for x in 0..a.width {
            let i = ((y * a.width + x) * 4) as usize;
            if (0..3).any(|c| (a.pixels[i + c] as i32 - b.pixels[i + c] as i32).abs() > 24) {
                n += 1;
            }
        }
    }
    n
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let library = AssetLibrary::at(std::path::Path::new(&a[0]));
    let out = std::path::PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out)?;
    let roles: Vec<String> = a.get(2).map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_else(|| vec!["DEEPAK".into(), "BHUSHAN".into()]);
    let mut scene = buzz_scene::Scene::default();
    scene.set_frame_count(240);
    let mut placed = Vec::new();
    for role in &roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| &x.name == role && x.folder.ends_with("Khayal3Baje")) else {
            println!("no {role}");
            continue;
        };
        let cast = buzz_app::scene_life::cast_into_shot(&mut scene, &library, &asset, true, 0).map_err(anyhow::Error::msg)?;
        // The layers must reach the scene's end for the strip to show the whole 10 s.
        placed.push(json!({"role": role, "folder": cast.folder.is_some(), "life": format!("{:?}", cast.life)}));
    }
    let full = scene.stage().stage_rect();
    // Close on the characters: the middle 60% of the stage's width, the upper 75% of its height.
    let stage = Rect::new(full.x0 + full.width() * 0.2, full.y0 + full.height() * 0.1, full.x0 + full.width() * 0.8, full.y0 + full.height() * 0.95);
    let mut ex = Exporter::new(&GpuPreference::Automatic)?;
    let frames_at = [0u32, 30, 60, 90, 120, 150, 180, 210];
    let width = 480;
    let imgs: Vec<Frame> = frames_at.iter().map(|f| render(&mut ex, &scene, *f, stage, width)).collect::<anyhow::Result<_>>()?;
    let h = imgs[0].height;
    let cols = 4u32;
    let rows = 2u32;
    let mut sheet = Frame { width: width * cols, height: h * rows, pixels: vec![255; (width * cols * h * rows * 4) as usize] };
    for (c, img) in imgs.iter().enumerate() {
        let (cx, cy) = (c as u32 % cols, c as u32 / cols);
        for y in 0..h {
            for x in 0..width {
                let src = ((y * img.width + x) * 4) as usize;
                let dst = (((cy * h + y) * sheet.width + cx * width + x) * 4) as usize;
                sheet.pixels[dst..dst + 4].copy_from_slice(&img.pixels[src..src + 4]);
            }
        }
    }
    // Heads: frame 0 (eyes open at rest) and the first shut-eye frame of each character, from the lid track.
    let mut heads = Vec::new();
    for ch in buzz_app::scene_life::stage_characters(&scene) {
        let Some(view) = buzz_act::pose::rig_shown(&scene, ch.folder, 0) else { continue };
        let Some(face) = buzz_act::face::face_of(&scene, &view) else { continue };
        let Some((spot, frames)) = face.lid else { continue };
        let track = buzz_act::face::chart_track(&scene, spot, 0, 240);
        let shut = track.iter().find(|(_, c)| *c == frames[2]).map(|(f, _)| *f);
        let Some((_, obj)) = scene.find_object(face.head) else { continue };
        let inherited = scene.layers().inherited_transform(face.head_layer, 0u32);
        let hb = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(obj));
        let region = Rect::new(hb.x0, hb.y0, hb.x1, hb.y1);
        let open = render(&mut ex, &scene, 0, region, 240)?;
        let shut_img = match shut { Some(f) => Some(render(&mut ex, &scene, f, region, 240)?), None => None };
        let eye_rows = (open.height as f64 * 0.2) as u32..(open.height as f64 * 0.5) as u32;
        let eye_px = shut_img.as_ref().map(|s| changed(&open, s, eye_rows.clone()));
        let p1 = out.join(format!("e2_head_{}_open.png", ch.name));
        open.write_png(&p1)?;
        if let Some(s) = &shut_img {
            s.write_png(&out.join(format!("e2_head_{}_shut.png", ch.name)))?;
        }
        heads.push(json!({"name": ch.name, "blinks_keyed": track.len(), "first_shut_frame": shut, "eye_band_px_changed_open_vs_shut": eye_px, "lid_at_frame0_is_open": track.first().map(|(_, c)| *c == frames[0])}));
    }
    let path = out.join("e2_strip.png");
    sheet.write_png(&path)?;
    // Body motion: pixels changed against frame 0 over the middle rows (chest and arms), per still.
    let mid = (h as f64 * 0.35) as u32..(h as f64 * 0.75) as u32;
    let body: Vec<usize> = imgs.iter().map(|i| changed(&imgs[0], i, mid.clone())).collect();
    println!("{}", serde_json::to_string_pretty(&json!({"strip": path.display().to_string(), "placed": placed, "heads": heads, "body_px_vs_frame0": body}))?);
    Ok(())
}
