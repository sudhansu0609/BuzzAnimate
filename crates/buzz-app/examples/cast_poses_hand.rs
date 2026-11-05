//! Gate for CAST_POSES_PLAN C2: a hand `switch_lifted_pose` on a produced scene.
//! `-- <scene.buzz> <character folder name> <switch frame> <view> <out dir> <frame,frame,...>`
//! Writes the switched copy to `<out>/switched.buzz` (the input is not touched) and one PNG per frame
//! of the BEFORE and the AFTER document, cropped round the character.
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (path, name, at, view, out) = (&a[0], &a[1], a[2].parse::<u32>()?, &a[3], std::path::PathBuf::from(&a[4]));
    let frames: Vec<u32> = a[5].split(',').filter_map(|f| f.parse().ok()).collect();
    std::fs::create_dir_all(&out)?;
    let before = buzz_doc::format::load(path)?;
    let mut after = before.clone();
    let folder = after.stage_layers().iter().find(|l| l.name == *name && l.cast_source.is_some()).map(|l| l.id).ok_or_else(|| anyhow::anyhow!("no lifted character {name}"))?;
    let pose = buzz_act::pose::pose_for_view(&after, folder, view).ok_or_else(|| anyhow::anyhow!("no {view} pose"))?;
    let report = buzz_act::pose::switch_lifted_pose(&mut after, folder, at, pose).map_err(anyhow::Error::msg)?;
    println!("{report:?}");
    buzz_doc::format::save(&after, out.join("switched.buzz"))?;
    // Mouth shape (frame of the mouth chart) of whichever pose shows, per frame.
    for (label, scene) in [("before", &before), ("after", &after)] {
        let mut line = Vec::new();
        for f in at.saturating_sub(4)..at + 40 {
            let shape = buzz_act::pose::mouth_shape_at(scene, folder, f);
            line.push(shape.map_or("-".to_string(), |s| s.to_string()));
        }
        println!("{label} mouth shapes from {}: {}", at.saturating_sub(4), line.join(" "));
    }
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    for (label, scene) in [("before", &before), ("after", &after)] {
        for f in &frames {
            let frame = exporter.render(scene, *f, &ExportSettings::scaled(scene, 0.5))?;
            frame.write_png(&out.join(format!("{label}_{f:04}.png")))?;
        }
    }
    println!("shown before at {at}: {:?}, after: {:?}", buzz_act::pose::shown_pose(&before, folder, at), buzz_act::pose::shown_pose(&after, folder, at));
    Ok(())
}
