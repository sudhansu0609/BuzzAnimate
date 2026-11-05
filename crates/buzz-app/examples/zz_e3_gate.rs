//! E3 gate: zz_e3_gate <library-root> <out-dir> <audio> <script.md> <from_s> <secs> <ROLE,ROLE> [first_scene,last_scene]
//! A scratch scene: the slice `from_s .. from_s + secs` of the recording as master audio, the speakers
//! placed the way the Cast panel does (`scene_life::cast_into_shot`), then what Sequence > Lip Sync All
//! Characters does (`scene_life::hear_dialogue` + `Editor::lip_sync_all_apply`), then stills: for every
//! spoken line of each speaker one still of each character at the middle of the line, plus the mouth
//! and body measurements. `HEAR_ONLY=1` stops after printing the lines.
use buzz_act::audio_align;
use buzz_app::editor::Editor;
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

fn paste(sheet: &mut Frame, img: &Frame, col: u32, row: u32) {
    for y in 0..img.height {
        for x in 0..img.width {
            let src = ((y * img.width + x) * 4) as usize;
            let dst = (((row * img.height + y) * sheet.width + col * img.width + x) * 4) as usize;
            sheet.pixels[dst..dst + 4].copy_from_slice(&img.pixels[src..src + 4]);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let library = AssetLibrary::at(std::path::Path::new(&a[0]));
    let out = std::path::PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out)?;
    let whole = buzz_audio::Clip::open(std::path::Path::new(&a[2]))?;
    let script_text = std::fs::read_to_string(&a[3])?;
    let (from_s, secs): (f64, f64) = (a[4].parse()?, a[5].parse()?);
    let roles: Vec<String> = a[6].split(',').map(str::to_string).collect();
    let mut script = buzz_act::screenplay::Screenplay::parse(&script_text);
    if let Some(range) = a.get(7) {
        let (lo, hi): (u32, u32) = {
            let mut p = range.split(',');
            (p.next().unwrap_or("1").parse()?, p.next().unwrap_or("999").parse()?)
        };
        script.scenes.retain(|s| s.number >= lo && s.number <= hi);
    }
    let slice = audio_align::cut(&whole, (from_s, from_s + secs), 0.0, 0.0).ok_or_else(|| anyhow::anyhow!("empty slice"))?;
    let wav = out.join("slice.wav");
    std::fs::write(&wav, audio_align::wav_bytes(&slice))?;

    let mut editor = Editor::default();
    editor.assets = library.clone();
    editor.doc.edit("Stage", |scene| {
        scene.stage_mut().frame_rate = 24.0;
        scene.set_frame_count((secs * 24.0).ceil() as u32 + 24);
    });
    editor.doc.end_gesture();
    editor.set_master_audio_from_file(&wav)?;
    for role in &roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| &x.name == role && x.folder.ends_with("Khayal3Baje")) else {
            println!("no {role}");
            continue;
        };
        let lib = library.clone();
        let mut res = None;
        editor.doc.edit("Cast into the Shot", |scene| res = Some(buzz_app::scene_life::cast_into_shot(scene, &lib, &asset, true, 0)));
        editor.doc.end_gesture();
        if let Some(Err(e)) = res {
            anyhow::bail!("{role}: {e}");
        }
    }
    let scene_now = editor.doc.scene().clone();
    editor.sound.refresh(&scene_now);
    let (_, start, clip) = editor.sound.stage_track_trimmed(editor.doc.scene()).ok_or_else(|| anyhow::anyhow!("no stage sound"))?;
    let names: Vec<String> = buzz_app::scene_life::stage_characters(editor.doc.scene()).iter().map(|c| c.name.clone()).collect();
    let t = std::time::Instant::now();
    let hearing = buzz_app::scene_life::hear_dialogue(&clip, &script, &names, 24.0, &mut |_, _| {}, &|| false).map_err(anyhow::Error::msg)?;
    let hear_secs = t.elapsed().as_secs_f64();
    let lines_json: Vec<_> = hearing
        .lines
        .iter()
        .map(|l| json!({"speaker": l.speaker, "narrator": l.narrator, "start": (l.start * 10.0).round() / 10.0, "end": (l.end * 10.0).round() / 10.0, "text": l.text.chars().take(50).collect::<String>()}))
        .collect();
    if std::env::var("HEAR_ONLY").is_ok() {
        println!("{}", serde_json::to_string_pretty(&json!({"model": hearing.model, "hear_secs": hear_secs, "audio_secs": clip.duration_seconds(), "notes": hearing.notes, "lines": lines_json}))?);
        return Ok(());
    }
    let undo_before = editor.doc.can_undo();
    let layers_before = editor.doc.scene().layers().len();
    let report = editor.lip_sync_all_apply(&hearing.lines, &hearing.visemes, start)?;
    let layers_after = editor.doc.scene().layers().len();
    // One undo step: undoing once removes everything it wrote; redo restores it.
    editor.doc.undo();
    let layers_undone = editor.doc.scene().layers().len();
    editor.doc.redo();
    let one_undo_step = layers_undone == layers_before && editor.doc.scene().layers().len() == layers_after;
    // Re-running replaces what it wrote (no second caption layer, same layer count).
    let again = editor.lip_sync_all_apply(&hearing.lines, &hearing.visemes, start)?;
    let rerun_replaces = editor.doc.scene().layers().len() == layers_after && again.matched == report.matched;
    let scene = editor.doc.scene().clone();
    let mut ex = Exporter::new(&GpuPreference::Automatic)?;
    let full = scene.stage().stage_rect();
    let region = Rect::new(full.x0 + full.width() * 0.2, full.y0 + full.height() * 0.1, full.x0 + full.width() * 0.8, full.y0 + full.height() * 0.95);
    // The stills: the middle of each non-narrator line, plus of each narrator line (first 2).
    let fps = 24.0;
    let mut cells: Vec<(String, u32)> = Vec::new();
    let mut shown_narrator = 0;
    for l in &hearing.lines {
        let mid = start + (((l.start + l.end) / 2.0) * fps) as u32;
        if l.narrator {
            if shown_narrator < 2 {
                cells.push((format!("NARRATOR line at {:.1}s", l.start), mid));
                shown_narrator += 1;
            }
        } else {
            cells.push((format!("{} at {:.1}s", l.speaker.clone().unwrap_or_default(), l.start), mid));
        }
    }
    let width = 400;
    let cols = 4u32;
    let rows = (cells.len() as u32).div_ceil(cols).max(1);
    let first = render(&mut ex, &scene, 0, region, width)?;
    let mut sheet = Frame { width: width * cols, height: first.height * rows, pixels: vec![255; (width * cols * first.height * rows * 4) as usize] };
    let mut stills = Vec::new();
    for (i, (label, frame)) in cells.iter().enumerate() {
        let img = render(&mut ex, &scene, *frame, region, width)?;
        paste(&mut sheet, &img, i as u32 % cols, i as u32 / cols);
        // Each still alone, too.
        let p = out.join(format!("still_{i:02}.png"));
        img.write_png(&p)?;
        stills.push(json!({"label": label, "frame": frame, "png": p.display().to_string()}));
    }
    // Faces: a row per character, a column per still (the head only, so the mouths can be read).
    let chars = buzz_app::scene_life::stage_characters(&scene);
    let mut faces = Frame { width: 240 * cells.len() as u32, height: 320 * chars.len() as u32, pixels: Vec::new() };
    faces.pixels = vec![255; (faces.width * faces.height * 4) as usize];
    for (row, ch) in chars.iter().enumerate() {
        let view = buzz_act::pose::rig_shown(&scene, ch.folder, 0).unwrap();
        let face = buzz_act::face::face_of(&scene, &view).unwrap();
        let (_, obj) = scene.find_object(face.head).unwrap();
        let inherited = scene.layers().inherited_transform(face.head_layer, 0u32);
        let hb = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(obj));
        let region = Rect::new(hb.x0, hb.y0, hb.x1, hb.y1);
        for (col, (_, frame)) in cells.iter().enumerate() {
            let img = render(&mut ex, &scene, *frame, region, 240)?;
            for y in 0..img.height.min(320) {
                for x in 0..240 {
                    let src = ((y * img.width + x) * 4) as usize;
                    let dst = (((row as u32 * 320 + y) * faces.width + col as u32 * 240 + x) * 4) as usize;
                    faces.pixels[dst..dst + 4].copy_from_slice(&img.pixels[src..src + 4]);
                }
            }
        }
    }
    let faces_path = out.join("e3_faces.png");
    faces.write_png(&faces_path)?;
    let sheet_path = out.join("e3_stills.png");
    sheet.write_png(&sheet_path)?;
    // The mouth tracks of every character: (frame, chart frame) changes, and the viseme map.
    let mut tracks = serde_json::Map::new();
    for ch in buzz_app::scene_life::stage_characters(&scene) {
        let view = buzz_act::pose::rig_shown(&scene, ch.folder, 0).unwrap();
        let face = buzz_act::face::face_of(&scene, &view).unwrap();
        let spot = face.mouth.unwrap();
        let chart = buzz_act::face::mouth_chart(&scene, &face).unwrap();
        let map = buzz_act::face::viseme_map_of(&scene, chart);
        let track = buzz_act::face::chart_track(&scene, spot, 0, scene.frame_count());
        // Which of this character's lines were open-mouth: frames with a non-rest chart frame.
        let rest = map[0];
        let open_frames: Vec<u32> = (0..scene.frame_count()).filter(|f| buzz_act::face::chart_track(&scene, spot, *f, *f + 1).first().is_some_and(|(_, c)| *c != rest)).collect();
        tracks.insert(ch.name.clone(), json!({"viseme_map": map, "rest_chart_frame": rest, "open_mouth_frames": open_frames.len(), "first_open": open_frames.first(), "track_keys": track.len(), "open_frames": open_frames}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "model": hearing.model, "hear_secs": hear_secs, "audio_secs": clip.duration_seconds(), "notes": hearing.notes,
            "lines": lines_json, "report": {"lines": report.lines, "matched": report.matched, "narrator": report.narrator_lines, "unmatched": report.unmatched, "mouths": report.mouths, "warnings": report.warnings},
            "can_undo_before": undo_before, "one_undo_step": one_undo_step, "rerun_replaces": rerun_replaces, "sheet": sheet_path.display().to_string(), "faces": faces_path.display().to_string(), "face_rows": chars.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), "stills": stills, "mouth": tracks
        }))?
    );
    Ok(())
}
