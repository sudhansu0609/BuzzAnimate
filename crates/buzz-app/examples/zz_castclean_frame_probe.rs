//! scratch probe — delete after use (TRACE_SPILL_AND_OUTPAINT_PLAN A6e).
//! Builds one episode scene headless, as the storyboard does, from a given
//! asset library, and renders one frame through a FIXED camera (so two
//! libraries can be compared pixel for pixel), at 1x and at `scale`.
//!
//! zz_castclean_frame_probe <episode dir> <breakdown.json> <assets dir> <scene> <frame> <cx> <cy> <zoom> <out prefix> [scale]
//! Writes `<out prefix>_1x.png` (960x540) and `<out prefix>_<scale>x.png`; prints
//! the top-level layer names that hold each cast member (from the build report).
use std::path::PathBuf;

use buzz_app::storyboard::{LookChoice, PANEL_SIZE, StoryboardRequest, Workbench};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let breakdown = PathBuf::from(&a[1]);
    let assets = PathBuf::from(&a[2]);
    let number: u32 = a[3].parse().unwrap();
    let frame: u32 = a[4].parse().unwrap();
    let (cx, cy, zoom): (f64, f64, f64) = (a[5].parse().unwrap(), a[6].parse().unwrap(), a[7].parse().unwrap());
    let prefix = &a[8];
    let scale: u32 = a.get(9).and_then(|s| s.parse().ok()).unwrap_or(4);
    let casting = [dir.join("buzz").join("casting.json"), dir.join("casting.json")].into_iter().find(|p| p.is_file()).unwrap();
    let out = std::env::temp_dir().join(format!("zz-castclean-{}", std::process::id()));
    let req = StoryboardRequest {
        script: dir.join("script.txt"),
        casting,
        out: out.clone(),
        scenes: Some(vec![number]),
        look: LookChoice::A,
        offline: false,
        assets: Some(assets),
        world: String::new(),
        size: PANEL_SIZE,
        breakdown: Some(breakdown),
    };
    let wb = Workbench::open_for_pictures(&req, &mut |l| eprintln!("{l}")).unwrap();
    let mut scene = wb.scene(number).unwrap().clone();
    let subtitles: Vec<_> = scene.layers().iter().filter(|l| l.name == "Subtitles").map(|l| l.id).collect();
    for id in subtitles {
        scene.update_layer(id, |l| l.visible = false);
    }
    {
        let cam = scene.camera_mut();
        cam.clear();
        cam.enabled = true;
        let mut key = buzz_scene::CameraKey::new(frame, buzz_geom::Point::new(cx, cy));
        key.zoom = zoom;
        cam.set_key(key);
    }
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    for s in [1, scale] {
        let mut settings = buzz_export::ExportSettings::for_stage(&scene);
        settings.width = PANEL_SIZE.0 * s;
        settings.height = PANEL_SIZE.1 * s;
        let f = ex.render(&scene, frame, &settings).unwrap();
        let img = image::RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap();
        img.save(format!("{prefix}_{s}x.png")).unwrap();
    }
    if let Some(r) = wb.report(number) {
        for c in &r.cast {
            let layers = buzz_app::storyboard::figure_layers(&scene, r, &c.name);
            println!("cast {:?} ({}): {} layer(s)", c.name, c.played_by, layers.len());
        }
    }
    let _ = std::fs::remove_dir_all(out);
}
