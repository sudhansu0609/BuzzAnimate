//! scratch probe — delete after use (SPLIT_PICTURE_PLAN F1-F4).
//! Runs exactly the app's Split Picture chain on a PNG:
//! resample <=1920, split_plates, extract_objects, trace_plates(filled), add_to_scene.
//!
//! zz_split_probe <picture.png> <out dir>
//! Writes plates/{sky,far,mid,ground}.png, filled/{...}.png, objects/<n>_<label>_{mask,crop}.png,
//! vlm/ (from BUZZ_SPLIT_DEBUG_DIR), layers/<name>.png, composite.png, source.png.
use std::path::PathBuf;

use image::{GrayImage, RgbaImage};

fn opaque_pct(img: &RgbaImage) -> f64 {
    let n = img.pixels().filter(|p| p[3] > 128).count();
    100.0 * n as f64 / (img.width() * img.height()) as f64
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let src = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    for d in ["plates", "filled", "objects", "vlm", "layers"] {
        std::fs::create_dir_all(out.join(d)).unwrap();
    }
    // SAFETY: single-threaded at this point.
    unsafe { std::env::set_var("BUZZ_SPLIT_DEBUG_DIR", out.join("vlm")) };

    let full = image::open(&src).unwrap().to_rgba8();
    let long = full.width().max(full.height());
    let picture = if long > 1920 {
        let s = 1920.0 / long as f64;
        image::imageops::resize(
            &full,
            ((full.width() as f64 * s).round() as u32).max(1),
            ((full.height() as f64 * s).round() as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        full
    };
    picture.save(out.join("source.png")).unwrap();
    let (pw, ph) = picture.dimensions();

    let t = std::time::Instant::now();
    let plates = buzz_app::backdrop::split_plates(&picture);
    println!("split_plates {:?}, horizon {}", t.elapsed(), plates.horizon);
    for (n, p) in [("sky", &plates.sky), ("far", &plates.far), ("mid", &plates.mid), ("ground", &plates.ground)] {
        p.save(out.join("plates").join(format!("{n}.png"))).unwrap();
    }
    println!(
        "PLATES sky {:.1}% far {:.1}% mid {:.1}% ground {:.1}%",
        opaque_pct(&plates.sky),
        opaque_pct(&plates.far),
        opaque_pct(&plates.mid),
        opaque_pct(&plates.ground)
    );

    let opts = buzz_app::bg_objects::ObjectOptions::default();
    let t = std::time::Instant::now();
    let cut = buzz_app::bg_objects::extract_objects(&picture, &plates, &opts);
    println!("extract_objects {:?}", t.elapsed());
    for n in &cut.notes {
        println!("note: {n}");
    }
    let f = &cut.filled_plates;
    for (n, p) in [("sky", &f.sky), ("far", &f.far), ("mid", &f.mid), ("ground", &f.ground)] {
        p.save(out.join("filled").join(format!("{n}.png"))).unwrap();
    }
    println!(
        "FILLED sky {:.1}% far {:.1}% mid {:.1}% ground {:.1}%",
        opaque_pct(&f.sky),
        opaque_pct(&f.far),
        opaque_pct(&f.mid),
        opaque_pct(&f.ground)
    );
    for (i, o) in cut.objects.iter().enumerate() {
        let name = format!("{}_{}{}", i, o.label.replace(' ', "_"), o.n);
        o.mask.save(out.join("objects").join(format!("{name}_mask.png"))).unwrap();
        let mut crop = picture.clone();
        for (x, y, p) in o.mask.enumerate_pixels() {
            if p[0] <= 127 {
                crop.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        let [x0, y0, x1, y1] = o.bbox;
        image::imageops::crop_imm(&crop, x0, y0, x1 - x0, y1 - y0).to_image().save(out.join("objects").join(format!("{name}_crop.png"))).unwrap();
        let mpx = o.mask.pixels().filter(|p| p[0] > 127).count();
        let box_px = ((x1 - x0) * (y1 - y0)) as f64;
        println!(
            "OBJECT {} {} bbox {:?} mask_px {} ({:.1}% of box, {:.2}% of picture) plate {}",
            o.label,
            o.n,
            o.bbox,
            mpx,
            100.0 * mpx as f64 / box_px,
            100.0 * mpx as f64 / (pw * ph) as f64,
            o.depth_plate.name()
        );
        // F2 dup metric: per plate, opaque px under the mask and mean colour diff vs the object's own pixels.
        let mut line = String::from("   dup:");
        for (pn, p) in [("sky", &f.sky), ("far", &f.far), ("mid", &f.mid), ("ground", &f.ground)] {
            let (mut n, mut diff) = (0u64, 0f64);
            for (x, y, m) in o.mask.enumerate_pixels() {
                if m[0] > 127 && p.get_pixel(x, y)[3] > 128 {
                    n += 1;
                    let (c, s) = (p.get_pixel(x, y), picture.get_pixel(x, y));
                    diff += (0..3).map(|k| (c[k] as f64 - s[k] as f64).abs()).sum::<f64>() / 3.0;
                }
            }
            line += &format!(" {pn} n={n} diff={:.1};", if n > 0 { diff / n as f64 } else { -1.0 });
        }
        println!("{line}");
    }

    let mut scene = buzz_app::backdrop::trace_plates(&cut.filled_plates, &opts.trace);
    let placed = buzz_app::bg_objects::add_to_scene(&cut, &mut scene, "Probe", (pw, ph));
    println!("placed {} objects; layers: {:?}", placed.len(), scene.layers().iter().map(|l| l.name.clone()).collect::<Vec<_>>());

    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let render = |ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, path: PathBuf| {
        let settings = buzz_export::ExportSettings::for_stage(scene);
        let f = ex.render(scene, 0, &settings).unwrap();
        RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap().save(path).unwrap();
    };
    render(&mut ex, &scene, out.join("composite.png"));
    let ids: Vec<_> = scene.layers().iter().map(|l| (l.id, l.name.clone())).collect();
    for (k, (keep, name)) in ids.iter().enumerate() {
        let mut s = scene.clone();
        for (id, _) in &ids {
            if id != keep {
                s.update_layer(*id, |l| l.visible = false);
            }
        }
        render(&mut ex, &s, out.join("layers").join(format!("{k:02}_{}.png", name.replace(' ', "_"))));
    }
    let _ = GrayImage::new(1, 1);
}
