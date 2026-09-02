//! Render a staged scene with a walk in it, for looking at.
//!
//! `cargo run -p buzz-app --example staged_shot -- <out-dir>`
//!
//! Not a test: what it produces is a picture, and the only thing that can judge
//! a picture is somebody looking at it. The assertions that a machine *can*
//! make live in `tests/staged_scene_renders.rs`.

use buzz_act::{Action, Performance, SceneRecipe, Setting};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::Scene;

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());

    let mut scene = Scene::default();
    let built = buzz_act::stage_scene(
        &mut scene,
        &SceneRecipe {
            setting: Setting::Sunset,
            cast: 2,
            frames: 48,
            ..SceneRecipe::default()
        },
    );
    let actors: Vec<_> = built.actors().collect();
    buzz_act::perform(
        &mut scene,
        actors[0],
        &Performance {
            distance: 220.0,
            ..Performance::new(Action::Walk, 0..24)
        },
    )
    .expect("the walk applies");
    if let Some(other) = actors.get(1) {
        buzz_act::perform(&mut scene, *other, &Performance::new(Action::Talk, 0..24))
            .expect("the talk applies");
    }

    let settings = ExportSettings::for_stage(&scene);
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    for frame in [0u32, 6, 12] {
        let rendered = exporter.render(&scene, frame, &settings)?;
        let path = std::path::PathBuf::from(&out).join(format!("shot-{frame:02}.png"));
        rendered.write_png(&path)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
