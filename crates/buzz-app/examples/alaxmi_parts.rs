//! alaxmi_parts: cut a built episode (.buzz, one scene per script scene) into parts.
//!
//!   alaxmi_parts --doc Khayal3Baje.buzz --measure
//!       prints each scene's name and length in seconds (JSON).
//!   alaxmi_parts --doc Khayal3Baje.buzz --ranges "1-3,4-5" --out-dir parts --prefix part_
//!       writes part_01.buzz, part_02.buzz ...: the same document with every scene outside the range deleted.
//!   alaxmi_parts --doc Khayal3Baje.buzz --lines
//!       prints every dialogue-line start (scene, global frame, label) from the `Voice` layers: where a part may be cut.
//!   alaxmi_parts --doc Khayal3Baje.buzz --cuts "3600,7200,..." --out-dir parts --prefix part_
//!       parts cut at those global frames (the film's scenes end to end): a cut inside a scene trims that scene's
//!       frames (every layer) in the part before and the part after.
use anyhow::{Context, Result, bail};
use buzz_doc::Document;
use std::path::PathBuf;

/// Keep the camera's keys (and focus keys) of frames `first..=last`, moved to start at 0, with the state at `first` keyed
/// at 0. The camera's last key also sets a scene's length, so without this a trimmed scene would still play in full.
fn trim_camera(scene: &mut buzz_scene::Scene, first: u32, last: u32) {
    let keys: Vec<_> = scene.camera().keys().to_vec();
    let focus: Vec<_> = scene.camera().focus_keys().to_vec();
    let start = scene.camera().state_at(first);
    let focus_start = scene.camera().focus_at(first);
    let cam = scene.camera_mut();
    for k in &keys {
        cam.remove_key(k.frame);
    }
    for k in keys.iter().filter(|k| k.frame > first && k.frame <= last) {
        cam.set_key(buzz_scene::camera_track::CameraKey { frame: k.frame - first, ..*k });
    }
    if let Some(s) = start {
        cam.set_key(buzz_scene::camera_track::CameraKey { frame: 0, ..s });
    }
    if !focus.is_empty() {
        let mut kept: Vec<_> = focus
            .iter()
            .filter(|k| k.frame > first && k.frame <= last)
            .map(|k| buzz_scene::camera_track::FocusKey { frame: k.frame - first, ..*k })
            .collect();
        kept.insert(0, buzz_scene::camera_track::FocusKey { frame: 0, ..focus_start });
        cam.set_focus_keys(kept);
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut doc_path, mut ranges, mut out_dir, mut prefix, mut measure) = (None, None, None, "part_".to_string(), false);
    let (mut lines, mut cuts) = (false, None::<String>);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--doc" => doc_path = Some(PathBuf::from(value("--doc")?)),
            "--ranges" => ranges = Some(value("--ranges")?),
            "--out-dir" => out_dir = Some(PathBuf::from(value("--out-dir")?)),
            "--prefix" => prefix = value("--prefix")?,
            "--measure" => measure = true,
            "--lines" => lines = true,
            "--cuts" => cuts = Some(value("--cuts")?),
            other => bail!("unknown flag {other}"),
        }
    }
    let doc_path = doc_path.context("--doc is required")?;
    let doc = Document::open(&doc_path).with_context(|| doc_path.display().to_string())?;
    let fps = doc.scene().stage().frame_rate.max(1.0);
    let names = doc.scene_names();
    if measure {
        let list: Vec<serde_json::Value> = (0..names.len())
            .map(|i| serde_json::json!({"index": i + 1, "name": names[i], "frames": doc.playing_length(i), "seconds": (doc.playing_length(i) as f64 / fps * 100.0).round() / 100.0}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"fps": fps, "scenes": list}))?);
        return Ok(());
    }
    if lines || cuts.is_some() {
        let lens: Vec<u32> = (0..names.len()).map(|i| doc.playing_length(i)).collect();
        if lines {
            let mut start = 0u32;
            let mut out = Vec::new();
            for (i, len) in lens.iter().enumerate() {
                let mut d = Document::open(&doc_path)?;
                d.switch_scene(i);
                let scene = d.scene();
                for layer in scene.stage_layers().iter().filter(|l| l.name == "Voice") {
                    for k in layer.frames.keyframes() {
                        if let Some(label) = &k.label {
                            out.push(serde_json::json!({"scene": i + 1, "frame": start + k.start, "seconds": ((start + k.start) as f64 / fps * 100.0).round() / 100.0, "label": label}));
                        }
                    }
                }
                start += len;
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }
        let out_dir = out_dir.context("--out-dir is required")?;
        std::fs::create_dir_all(&out_dir)?;
        let total: u32 = lens.iter().sum();
        let mut marks: Vec<u32> = vec![0];
        for c in cuts.unwrap_or_default().split(',').filter(|s| !s.trim().is_empty()) {
            marks.push(c.trim().parse()?);
        }
        marks.push(total);
        let mut starts = vec![0u32];
        for l in &lens {
            starts.push(starts.last().unwrap() + l);
        }
        for (n, w) in marks.windows(2).enumerate() {
            let (g0, g1) = (w[0], w[1]);
            let mut part = Document::open(&doc_path)?;
            for i in (0..names.len()).rev() {
                if starts[i + 1] <= g0 || starts[i] >= g1 {
                    part.delete_scene(i);
                }
            }
            let kept: Vec<usize> = (0..names.len()).filter(|&i| starts[i + 1] > g0 && starts[i] < g1).collect();
            for (slot, &i) in kept.iter().enumerate() {
                let first = g0.saturating_sub(starts[i]);
                let last = (g1.min(starts[i + 1]) - starts[i]).saturating_sub(1);
                if first == 0 && last + 1 >= lens[i] {
                    continue;
                }
                part.edit_scene_at(slot, "trim for part", |scene| {
                    trim_camera(scene, first, last);
                    let ids: Vec<_> = scene.stage_layers().iter().map(|l| l.id).collect();
                    for id in ids {
                        scene.update_stage_layer(id, |l| {
                            if let Some(seg) = l.frames.segment(first, last) {
                                l.frames = buzz_scene::timeline::LayerTimeline::from_parts(seg.keyframes, seg.length);
                            }
                        });
                    }
                });
            }
            let path = out_dir.join(format!("{prefix}{:02}.buzz", n + 1));
            part.save_as(&path).with_context(|| path.display().to_string())?;
            let frames: u32 = (0..part.scene_names().len()).map(|i| part.playing_length(i)).sum();
            println!("{} frames {g0}-{g1}: {:.1} s -> {}", path.file_name().unwrap().to_string_lossy(), frames as f64 / fps, path.display());
        }
        return Ok(());
    }
    let ranges = ranges.context("--ranges is required (or --measure)")?;
    let out_dir = out_dir.context("--out-dir is required")?;
    std::fs::create_dir_all(&out_dir)?;
    for (n, r) in ranges.split(',').enumerate() {
        let (a, b) = match r.trim().split_once('-') {
            Some((a, b)) => (a.trim().parse::<usize>()?, b.trim().parse::<usize>()?),
            None => (r.trim().parse::<usize>()?, r.trim().parse::<usize>()?),
        };
        if a == 0 || b < a || b > names.len() {
            bail!("range {r}: scenes are 1..={}", names.len());
        }
        let mut part = Document::open(&doc_path)?;
        // Delete from the back so the indices stay right.
        for i in (0..names.len()).rev() {
            if i + 1 < a || i + 1 > b {
                part.delete_scene(i);
            }
        }
        let path = out_dir.join(format!("{prefix}{:02}.buzz", n + 1));
        part.save_as(&path).with_context(|| path.display().to_string())?;
        let frames: u32 = (0..part.scene_names().len()).map(|i| part.playing_length(i)).sum();
        println!("{} scenes {a}-{b}: {:.1} s -> {}", path.file_name().unwrap().to_string_lossy(), frames as f64 / fps, path.display());
    }
    Ok(())
}
