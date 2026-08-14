//! Render one frame of any document this program can read, straight to a PNG.
//!
//! The point is *looking at it*. Import fidelity cannot be settled by counting
//! shapes — a drawing that arrives as the right number of wrong outlines
//! passes every count there is. This renders a frame headlessly so it can be
//! compared with what Animate shows:
//!
//! ```text
//! cargo run -p buzz-export --example shot -- "scene.fla" 0 out.png
//! ```

use anyhow::{Context, Result};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;

fn main() -> Result<()> {
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    // `--no-camera` renders the stage as drawn, which is how a camera move can
    // be told apart from artwork that is simply not there.
    let no_camera = raw.iter().any(|a| a == "--no-camera");
    raw.retain(|a| a != "--no-camera");
    let mut args = raw.into_iter();
    let path = args
        .next()
        .context("usage: shot <file> [frame] [out.png]")?;
    let frame: u32 = args.next().and_then(|f| f.parse().ok()).unwrap_or(0);
    let out = args.next().unwrap_or_else(|| "shot.png".to_string());

    let extension = std::path::Path::new(&path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut scene = match extension.as_str() {
        "buzz" => buzz_doc::Document::open(&path)?.scene().clone(),
        // A published `.swf` takes the other road in: Animate has already
        // resolved its fills, so nothing here has to reassemble them.
        "swf" => {
            let (scene, report) = buzz_import_swf::import(&path)?;
            eprintln!("{}", report.summary());
            scene
        }
        _ => {
            let (scene, report) = buzz_import_xfl::import(&path)?;
            eprintln!("{}", report.summary());
            scene
        }
    };

    if no_camera {
        scene.camera_mut().enabled = false;
    }

    // `--only <name>` throws the document away and renders one library symbol
    // on its own, which is how a character that arrives in pieces gets looked
    // at without the scene around it.
    if let Some(wanted) = std::env::args().skip_while(|a| a != "--only").nth(1) {
        let found = scene
            .library()
            .iter()
            .find(|s| s.name.contains(&wanted))
            .map(|s| (s.id, s.bounds()))
            .expect("no symbol of that name");
        scene.camera_mut().enabled = false;
        let layers: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
        for id in layers {
            scene.remove_layer(id);
        }
        let layer = scene.add_layer("Only", buzz_scene::LayerKind::Normal);
        let mut object = buzz_scene::Object::instance_of(buzz_scene::ObjectId(999_001), found.0);
        // The instance is placed at frame 0 and the timeline is then as long
        // as the symbol, so `shot <frame>` walks the symbol's own timeline.
        if let Some(buzz_scene::ObjectKind::Instance(i)) = Some(&mut object.kind) {
            i.first_frame = 0;
        }
        // Centred on the stage, at a size that fits it.
        if let Some(bounds) = found.1 {
            let stage = scene.stage().size;
            let scale = (stage.width / bounds.width().max(1.0))
                .min(stage.height / bounds.height().max(1.0))
                * 0.8;
            object.transform = buzz_geom::Affine::translate(buzz_geom::Vec2::new(
                stage.width / 2.0,
                stage.height / 2.0,
            )) * buzz_geom::Affine::scale(scale)
                * buzz_geom::Affine::translate(-bounds.center().to_vec2());
        }
        scene.add_object(layer, object);
        // As long as the symbol, so a frame number walks its timeline.
        let length = scene
            .library()
            .get(found.0)
            .map(|s| s.length())
            .unwrap_or(1);
        scene.set_frame_count(length.max(1));
    }
    // `--camera-flip` undoes the importer's inversion, for settling which way
    // round Animate's camera matrix reads.
    if std::env::args().any(|a| a == "--camera-flip") {
        let keys: Vec<_> = scene.camera().keys().to_vec();
        let track = scene.camera_mut();
        track.clear();
        for mut key in keys {
            key.zoom = 1.0 / key.zoom;
            track.set_key(key);
        }
    }

    let mut exporter = Exporter::new(&GpuPreference::default())?;
    let settings = ExportSettings::for_stage(&scene);
    let rendered = exporter.render(&scene, frame, &settings)?;
    rendered.write_png(std::path::Path::new(&out))?;
    eprintln!(
        "frame {frame} of {} -> {out} ({}x{})",
        scene.frame_count(),
        rendered.width,
        rendered.height
    );
    Ok(())
}
