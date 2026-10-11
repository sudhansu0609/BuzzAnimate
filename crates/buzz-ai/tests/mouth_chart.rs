//! M1 of `docs/SERIES_EP1_PLAN.md`: a labelled mouth chart becomes a
//! character's mouth-symbol frames.
//!
//! The synthetic chart (drawn ellipses plus text-like boxes) runs always; the
//! real Raju chart is `#[ignore]`d (it needs his file) and writes a preview
//! PNG of every frame:
//!
//! `cargo test -p buzz-ai --test mouth_chart real_raju -- --ignored --nocapture`
//!
//! The preview goes to `$MOUTH_CHART_PREVIEW_DIR`, else the temp dir.

use buzz_ai::face::mouth_chart::{
    UNIT_WIDTH, build_mouth_symbol, describe_plan, frame_plan, read_chart, read_chart_file, replace_mouth_frames,
    MouthChart, RAJU_LABELS,
};
use buzz_audio::{Viseme, VisemeTrack};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, LoopMode, ObjectKind, Scene, SymbolKind};
use kurbo::{Affine, Point};

const PAPER: [u8; 3] = [252, 250, 243];
const SKIN: [u8; 3] = [222, 148, 108];
const LINE: [u8; 3] = [78, 38, 28];
const MAROON: [u8; 3] = [92, 24, 24];
const TEETH: [u8; 3] = [250, 248, 240];
const INK: [u8; 3] = [28, 28, 30];

fn put(img: &mut image::RgbaImage, x: i32, y: i32, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
    }
}

/// A mouth: a skin ellipse with a dark outline, a maroon inside and, if asked,
/// a white teeth strip in the inside (so the teeth are a hole of paper colour).
fn draw_mouth(img: &mut image::RgbaImage, cx: f64, cy: f64, rx: f64, ry: f64, open: bool, teeth: bool) {
    for y in (cy - ry - 3.0) as i32..=(cy + ry + 3.0) as i32 {
        for x in (cx - rx - 3.0) as i32..=(cx + rx + 3.0) as i32 {
            let d = ((x as f64 - cx) / rx).powi(2) + ((y as f64 - cy) / ry).powi(2);
            if d > 1.0 {
                continue;
            }
            // Outline ring, then skin.
            let edge = 1.0 - (3.5 / rx.min(ry)).min(0.5);
            if d > edge * edge {
                put(img, x, y, LINE);
            } else if open {
                let inner = ((x as f64 - cx) / (rx * 0.7)).powi(2) + ((y as f64 - cy) / (ry * 0.62)).powi(2);
                if inner < 1.0 {
                    let top_teeth = teeth && (y as f64) < cy - ry * 0.15 && (y as f64) > cy - ry * 0.5;
                    put(img, x, y, if top_teeth { TEETH } else { MAROON });
                } else {
                    put(img, x, y, SKIN);
                }
            } else {
                put(img, x, y, SKIN);
            }
        }
    }
}

/// Four "letters" (dark boxes with gaps) centred under a mouth, `gap` px below its bottom.
fn draw_label(img: &mut image::RgbaImage, cx: f64, bottom: f64, gap: f64, letters: u32) {
    let (lw, lh, sp) = (9, 15, 5);
    let total = letters as i32 * lw + (letters as i32 - 1) * sp;
    let x0 = cx as i32 - total / 2;
    let y0 = (bottom + gap) as i32;
    for i in 0..letters as i32 {
        for y in 0..lh {
            for x in 0..lw {
                put(img, x0 + i * (lw + sp) + x, y0 + y, INK);
            }
        }
    }
}

/// Eight mouths in three rows (3 + 3 + 2), reading order = `LABELS`, a label
/// under each. Returns the image and each mouth's true centre in reading order.
const LABELS: [&str; 8] = ["Neutral Closed", "A", "E", "O", "U", "M/B/P", "Open Laugh", "Frown"];

fn synthetic_chart() -> (image::RgbaImage, Vec<(f64, f64, f64, f64)>) {
    let mut img = image::RgbaImage::from_pixel(720, 540, image::Rgba([PAPER[0], PAPER[1], PAPER[2], 255]));
    // (cx, cy, rx, ry, open, teeth, label letters, gap to label)
    let specs: [(f64, f64, f64, f64, bool, bool, u32, f64); 8] = [
        (130.0, 90.0, 70.0, 12.0, false, false, 7, 40.0),
        (360.0, 95.0, 62.0, 42.0, true, true, 1, 40.0),
        (590.0, 90.0, 66.0, 26.0, true, true, 1, 40.0),
        (130.0, 280.0, 36.0, 48.0, true, false, 1, 40.0),
        (360.0, 275.0, 34.0, 30.0, false, false, 1, 40.0),
        (590.0, 280.0, 64.0, 10.0, false, false, 5, 40.0),
        // A wide laugh with its label only 10 px under it.
        (180.0, 440.0, 90.0, 40.0, true, true, 9, 10.0),
        (500.0, 435.0, 70.0, 11.0, false, false, 5, 40.0),
    ];
    let mut truth = Vec::new();
    for (cx, cy, rx, ry, open, teeth, letters, gap) in specs {
        draw_mouth(&mut img, cx, cy, rx, ry, open, teeth);
        draw_label(&mut img, cx, cy + ry + 3.0, gap, letters);
        truth.push((cx, cy, rx, ry));
    }
    (img, truth)
}

#[test]
fn a_synthetic_chart_is_sliced_into_whole_labelled_mouths() {
    let (img, truth) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).expect("the synthetic chart reads");
    assert_eq!(chart.mouths.len(), 8);
    assert_eq!(chart.rest_index, 0);

    for (i, (mouth, (cx, cy, rx, ry))) in chart.mouths.iter().zip(&truth).enumerate() {
        assert_eq!(mouth.label, LABELS[i], "labels follow reading order");
        // Whole: the cut box is the ellipse's own box, give or take the
        // anti-aliasing, and so does not include its label.
        let b = mouth.source_box;
        assert!((b.center().x - cx).abs() < 3.0 && (b.center().y - cy).abs() < 3.0, "{}: centre {:?} vs ({cx},{cy})", mouth.label, b.center());
        assert!((b.width() - 2.0 * rx).abs() < 6.0, "{}: width {} vs {}", mouth.label, b.width(), 2.0 * rx);
        assert!((b.height() - 2.0 * ry).abs() < 6.0, "{}: height {} vs {} (a label was swallowed?)", mouth.label, b.height(), 2.0 * ry);
        assert!(!mouth.shapes.is_empty(), "{} traced to nothing", mouth.label);
    }
}

#[test]
fn mouths_share_one_scale_and_are_each_centred_on_the_origin() {
    let (img, truth) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).unwrap();
    let rest = &chart.mouths[0];
    // The closed mouth is UNIT_WIDTH wide.
    assert!((rest.bounds.width() - UNIT_WIDTH).abs() < 6.0, "rest width {}", rest.bounds.width());
    // Every other mouth is at the same scale: its width ratio is the drawing's.
    let unit = UNIT_WIDTH / (2.0 * truth[0].2);
    for (mouth, (_, _, rx, _)) in chart.mouths.iter().zip(&truth) {
        let want = 2.0 * rx * unit;
        assert!((mouth.bounds.width() - want).abs() < 8.0, "{}: {} vs {want}", mouth.label, mouth.bounds.width());
    }
    // Each mouth is centred on the origin by its own box, so a swap keeps it on the same spot.
    for mouth in &chart.mouths {
        let c = mouth.bounds.center();
        assert!(c.x.abs() < 3.0 && c.y.abs() < 3.0, "{} is centred at {:?}", mouth.label, c);
    }
}

#[test]
fn teeth_stay_in_an_open_mouth() {
    let (img, _) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).unwrap();
    let light = |m: &buzz_ai::face::mouth_chart::ChartMouth| {
        m.shapes.iter().any(|s| s.fill.as_ref().is_some_and(|f| {
            let c = f.color().to_rgba8();
            c.r > 225 && c.g > 225 && c.b > 215
        }))
    };
    // A, E and Open Laugh have teeth drawn; the closed mouth and U have none.
    assert!(light(chart.mouth("A").unwrap()), "A lost its teeth");
    assert!(light(chart.mouth("E").unwrap()), "E lost its teeth");
    assert!(light(chart.mouth("Open Laugh").unwrap()), "Open Laugh lost its teeth");
    assert!(!light(chart.mouth("U").unwrap()), "U has no teeth to keep");
}

#[test]
fn a_wrong_label_count_says_how_the_chart_was_read() {
    let (img, _) = synthetic_chart();
    let err = read_chart(&img, &LABELS[..7]).unwrap_err();
    assert!(err.contains("found 8 mouths") && err.contains("3+3+2") && err.contains("7 labels"), "{err}");
    let err = read_chart(&img, &[]).unwrap_err();
    assert!(err.contains("no labels"), "{err}");
    let blank = image::RgbaImage::from_pixel(64, 64, image::Rgba([PAPER[0], PAPER[1], PAPER[2], 255]));
    assert!(read_chart(&blank, &["x"]).unwrap_err().contains("no coloured mouth"));
}

/// The frame order the lip-sync writer depends on, and the extra named frames.
#[test]
fn frames_keep_the_lip_sync_order_and_append_the_rest_by_name() {
    let (img, _) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).unwrap();
    let plan = frame_plan(&chart);

    // Frames 0..=9 are the ten visemes at the frame number `Viseme::frame` says.
    for v in [
        Viseme::Rest, Viseme::Ai, Viseme::E, Viseme::O, Viseme::U,
        Viseme::L, Viseme::WQ, Viseme::MBP, Viseme::FV, Viseme::Etc,
    ] {
        let f = plan.iter().find(|f| f.viseme == Some(v)).unwrap_or_else(|| panic!("{v:?} missing"));
        assert_eq!(f.index, v.frame(), "{v:?} moved");
        assert_eq!(f.name, v.label());
    }
    let by = |v: Viseme| chart.mouths[plan.iter().find(|f| f.viseme == Some(v)).unwrap().mouth].label.as_str();
    assert_eq!(by(Viseme::Rest), "Neutral Closed");
    assert_eq!(by(Viseme::Ai), "A");
    assert_eq!(by(Viseme::E), "E");
    assert_eq!(by(Viseme::O), "O");
    assert_eq!(by(Viseme::U), "U");
    assert_eq!(by(Viseme::WQ), "U", "WQ takes the rounded U");
    assert_eq!(by(Viseme::MBP), "M/B/P");
    // This chart has no L, F/V or S/Z: those fall back to the closed mouth.
    assert_eq!(by(Viseme::L), "Neutral Closed");
    assert_eq!(by(Viseme::FV), "Neutral Closed");
    assert!(describe_plan(&chart).iter().any(|l| l.contains("L ") && l.contains("no label")));

    // Everything the visemes did not use follows, in chart order, from frame 10.
    let extras: Vec<(u32, &str)> = plan.iter().filter(|f| f.viseme.is_none()).map(|f| (f.index, f.name.as_str())).collect();
    assert_eq!(extras, vec![(10, "Open Laugh"), (11, "Frown")]);
    assert_eq!(plan.len(), 12);
}

#[test]
fn a_symbol_built_from_a_chart_has_every_frame_labelled_and_filled() {
    let (img, _) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).unwrap();
    let mut next = 1u64;
    let sym = build_mouth_symbol(buzz_scene::SymbolId(7), "Hero_mouth", &chart, 0.5, Some(1.0), &mut || {
        next += 1;
        buzz_scene::ObjectId(next)
    });
    let layer = sym.layers.iter().next().unwrap();
    assert_eq!(layer.frames.length(), 12);
    for f in frame_plan(&chart) {
        let key = layer.frames.keyframe_at(f.index).unwrap_or_else(|| panic!("no keyframe at {}", f.index));
        assert_eq!(key.start, f.index);
        assert_eq!(key.label.as_deref(), Some(f.name.as_str()));
        assert!(!layer.frames.objects_at(f.index).is_empty(), "{} is empty", f.name);
    }
    // Every stroke was forced to the one width, and the closed mouth is 50 wide.
    for object in layer.frames.all_objects() {
        if let ObjectKind::Shape(shape) = &object.kind {
            if let Some(s) = &shape.stroke {
                assert_eq!(s.width, 1.0);
            }
        }
    }
    let rest: kurbo::Rect = layer.frames.objects_at(0).iter().map(|o| o.bounds()).reduce(|a, b| a.union(b)).unwrap();
    assert!((rest.width() - 50.0).abs() < 4.0, "rest {}", rest.width());
}

/// M2's core: a character's own mouth symbol gets the chart's frames, keeps
/// its id and its size on the face, and lip sync still writes onto it.
#[test]
fn replacing_a_characters_mouth_frames_keeps_the_symbol_and_lip_sync_working() {
    let (img, _) = synthetic_chart();
    let chart = read_chart(&img, &LABELS).unwrap();

    let mut scene = Scene::default();
    let mouth = buzz_act::lipsync::placeholder_mouth(&mut scene, "Hero_mouth");
    let before = scene.library().get(mouth).unwrap().layers.iter().next().unwrap().frames.length();
    assert_eq!(before, 10);

    let applied = replace_mouth_frames(&mut scene, mouth, &chart).unwrap();
    assert_eq!(applied.symbol, mouth);
    assert_eq!(applied.frames.len(), 12);
    assert_eq!(applied.layers_replaced, 1);
    // The closed mouth is as wide as the placeholder's Rest was (26) and centred where it was.
    assert!((applied.rest_width - 26.0).abs() < 0.01, "{}", applied.rest_width);
    let sym = scene.library().get(mouth).unwrap();
    let layer = sym.layers.iter().next().unwrap();
    assert_eq!(layer.frames.length(), 12);
    let rest = layer.frames.objects_at(0).iter().map(|o| o.bounds()).reduce(|a, b| a.union(b)).unwrap();
    assert!((rest.width() - 26.0).abs() < 2.0 && rest.center().to_vec2().hypot() < 1.5, "{rest:?}");
    // Object ids come from the scene, so none collide.
    let mut ids: Vec<u64> = layer.frames.all_objects().map(|o| o.id.0).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), layer.frames.all_objects().count());

    // Lip sync writes onto the new symbol exactly as before.
    let track = VisemeTrack { frames: vec![Viseme::Rest, Viseme::Ai, Viseme::Ai, Viseme::MBP, Viseme::O, Viseme::Rest] , fps: 24.0 };
    let target = scene.add_layer("Hero mouth", LayerKind::Normal);
    let report = buzz_act::lipsync::write_track(&mut scene, &track, 0, target, mouth, Affine::IDENTITY);
    assert!(report.keyframes >= 4, "{report:?}");
    let shown: Vec<u32> = scene
        .layers()
        .get(target)
        .unwrap()
        .frames
        .keyframes()
        .iter()
        .flat_map(|k| k.objects.iter())
        .filter_map(|o| match &o.kind {
            ObjectKind::Instance(i) => Some(i.first_frame),
            _ => None,
        })
        .collect();
    assert!(shown.contains(&Viseme::Ai.frame()) && shown.contains(&Viseme::O.frame()), "{shown:?}");
}

// -- the real chart ---------------------------------------------------------------

const RAJU_CHART: &str = r"B:\youtubeProjects\Originals\series\ep1\raju\ChatGPT Image Oct 6, 2026, 03_29_03 AM.png";

/// Where each mouth is in the 1448x1086 chart (centre x, centre y), for a
/// loose "the right mouth got the right label" check.
const RAJU_CENTRES: [(f64, f64); 19] = [
    (193.0, 143.0), (541.0, 141.0), (892.0, 140.0), (1243.0, 147.0),
    (160.0, 392.0), (453.0, 399.0), (755.0, 393.0), (1032.0, 392.0), (1310.0, 394.0),
    (141.0, 634.0), (374.0, 621.0), (601.0, 627.0), (834.0, 632.0), (1076.0, 633.0), (1312.0, 631.0),
    (195.0, 883.0), (543.0, 879.0), (885.0, 868.0), (1249.0, 867.0),
];

/// Draw frame-index digits (3x5 pixels, scaled) so the preview can be read
/// against the printed legend.
fn draw_number(img: &mut image::RgbaImage, mut x: i32, y: i32, n: u32, s: i32) {
    const DIGITS: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b010, 0b010, 0b010],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    for ch in n.to_string().chars() {
        let rows = DIGITS[ch.to_digit(10).unwrap() as usize];
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..3 {
                if bits & (0b100 >> rx) != 0 {
                    for dy in 0..s {
                        for dx in 0..s {
                            put(img, x + rx * s + dx, y + ry as i32 * s + dy, [20, 20, 20]);
                        }
                    }
                }
            }
        }
        x += 4 * s;
    }
}

/// Render every frame of the chart's mouth symbol into one contact sheet:
/// frame order, five to a row, the frame number under each. `None` if there is
/// no GPU to render with.
fn write_preview(chart: &MouthChart, path: &std::path::Path) -> Option<()> {
    let plan = frame_plan(chart);
    let (cols, cell_w, cell_h, zoom) = (5usize, 330.0, 260.0, 2.6);
    let rows = plan.len().div_ceil(cols);
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(cols as f64 * cell_w, rows as f64 * cell_h);
    scene.stage_mut().background = peniko::Color::from_rgb8(236, 236, 240);
    let id = scene.add_symbol("Mouth", SymbolKind::Graphic, None);
    let mut next = 100_000u64;
    let sym = build_mouth_symbol(id, "Mouth", chart, 1.0, None, &mut || {
        next += 1;
        buzz_scene::ObjectId(next)
    });
    scene.library_mut().insert(sym);
    let layer = scene.add_layer("Frames", LayerKind::Normal);
    for (i, f) in plan.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        let mut object = buzz_scene::Object::instance_of(scene.next_object_id(), id);
        if let ObjectKind::Instance(inst) = &mut object.kind {
            inst.loop_mode = LoopMode::SingleFrame;
            inst.first_frame = f.index;
        }
        let centre = Point::new((col as f64 + 0.5) * cell_w, (row as f64 + 0.42) * cell_h);
        object.transform = Affine::translate(centre.to_vec2()) * Affine::scale(zoom);
        scene.add_object(layer, object);
    }
    let mut exporter = Exporter::new(&GpuPreference::Automatic).ok()?;
    let settings = ExportSettings::for_stage(&scene);
    let frame = exporter.render(&scene, 0, &settings).ok()?;
    let mut sheet = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)?;
    for (i, f) in plan.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        draw_number(&mut sheet, (col as f64 * cell_w + cell_w / 2.0) as i32 - 12, ((row as f64 + 0.86) * cell_h) as i32, f.index, 4);
    }
    sheet.save(path).ok()
}

#[test]
#[ignore = "needs Raju's chart image; writes a preview PNG"]
fn real_raju_chart_imports_whole_and_labelled() {
    let path = std::env::var("RAJU_MOUTH_CHART").unwrap_or_else(|_| RAJU_CHART.to_string());
    if !std::path::Path::new(&path).is_file() {
        eprintln!("Skipping: {path} not found");
        return;
    }
    let started = std::time::Instant::now();
    let chart = read_chart_file(std::path::Path::new(&path), &RAJU_LABELS).expect("Raju's chart reads");
    eprintln!("chart read and traced in {:.1}s; {} notes {:?}", started.elapsed().as_secs_f64(), chart.notes.len(), chart.notes);
    assert_eq!(chart.mouths.len(), 19);

    for (i, mouth) in chart.mouths.iter().enumerate() {
        let c = mouth.source_box.center();
        let (ex, ey) = RAJU_CENTRES[i];
        eprintln!(
            "{:>2} {:<22} src ({:>4.0},{:>4.0}) {:>3.0}x{:<3.0}  units {:>5.1}x{:<5.1}  {} shapes",
            i, mouth.label, c.x, c.y, mouth.source_box.width(), mouth.source_box.height(),
            mouth.bounds.width(), mouth.bounds.height(), mouth.shapes.len()
        );
        assert!((c.x - ex).abs() < 30.0 && (c.y - ey).abs() < 30.0, "{} was cut from ({:.0},{:.0}), expected near ({ex},{ey})", mouth.label, c.x, c.y);
    }
    // The closed mouth is the unit.
    let closed = chart.mouth("Neutral Closed").unwrap();
    assert!((closed.bounds.width() - UNIT_WIDTH).abs() < 6.0);

    // Whole, with no straight chord cut across it: `trace_mouth` asserts (in
    // debug builds, which tests are) that no corner contact reaches the tracer,
    // which is what made it close a loop with a long straight chord. Told for
    // the record: the longest single outline segment, in chart units (the
    // closed mouth is 100 wide); honest curve segments run to about 30.
    let mut longest = (0.0f64, "");
    for mouth in &chart.mouths {
        for shape in mouth.shapes.iter().filter(|s| s.fill.is_some()) {
            let l = longest_segment(&shape.path);
            if l > longest.0 {
                longest = (l, mouth.label.as_str());
            }
        }
    }
    eprintln!("longest single outline segment: {:.1} units ({})", longest.0, longest.1);

    for line in describe_plan(&chart) {
        eprintln!("frame {line}");
    }

    let dir = std::env::var("MOUTH_CHART_PREVIEW_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("raju_mouth_chart_preview.png");
    match write_preview(&chart, &out) {
        Some(()) => eprintln!("preview: {}", out.display()),
        None => eprintln!("no GPU: preview not written"),
    }
}

/// The longest single segment (end point to end point) of a path.
fn longest_segment(path: &kurbo::BezPath) -> f64 {
    use kurbo::PathEl;
    let (mut last, mut worst) = (Point::ORIGIN, 0.0f64);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => last = p,
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                worst = worst.max(last.distance(p));
                last = p;
            }
            PathEl::ClosePath => {}
        }
    }
    worst
}
