//! scratch probe — delete after use (TRACE_PICTURE_PLAN T15a).
//! zz_bold_fla_probe <file.fla> <out dir> <frame>...
//! Renders the frames and prints the outline style: stroke weights (px on the rendered frame),
//! colours, stroke length vs dark-fill area, per top-level layer.
use std::collections::BTreeMap;
use std::path::PathBuf;

use buzz_scene::{Object, ObjectKind, Paint, Scene};
use kurbo::{Affine, ParamCurve, ParamCurveArclen, PathSeg, Shape};

#[derive(Default)]
struct Stats {
    /// (weight px, path length px, colour)
    strokes: Vec<(f64, f64, [u8; 3])>,
    fill_area: f64,
    dark_fill_area: f64,
    n_fill: usize,
    n_stroke: usize,
    /// transformed dark solid fills (outlines drawn as shapes)
    dark: Vec<buzz_scene::ShapeData>,
}

fn path_len(p: &kurbo::BezPath, scale: f64) -> f64 {
    p.segments()
        .map(|s: PathSeg| s.arclen(0.5))
        .sum::<f64>()
        * scale
}

fn walk(scene: &Scene, o: &Object, t: Affine, depth: u32, st: &mut Stats) {
    let t = t * o.transform;
    let k = t.determinant().abs().sqrt();
    match &o.kind {
        ObjectKind::Shape(sd) => {
            if let Some(s) = &sd.stroke {
                if let Paint::Solid(c) = &s.paint {
                    let [r, g, b, _] = c.to_rgba8().to_u8_array();
                    let w = if s.hairline { 1.0 } else { s.width * k };
                    st.n_stroke += 1;
                    st.strokes.push((w, path_len(&sd.path, k), [r, g, b]));
                }
            }
            if let Some(f) = &sd.fill {
                st.n_fill += 1;
                let a = sd.path.area().abs() * k * k;
                st.fill_area += a;
                if let Paint::Solid(c) = &f.paint {
                    let [r, g, b, _] = c.to_rgba8().to_u8_array();
                    if (r as u32 + g as u32 + b as u32) < 150 {
                        st.dark_fill_area += a;
                        if sd.stroke.is_none() && a > 4.0 {
                            let mut d = sd.clone();
                            d.path = t * sd.path.clone();
                            st.dark.push(d);
                        }
                    }
                }
            }
        }
        ObjectKind::Group(kids) => {
            for c in kids {
                walk(scene, c, t, depth, st);
            }
        }
        ObjectKind::Instance(inst) => {
            if depth > 12 {
                return;
            }
            if let Some(sym) = scene.library().get(inst.symbol) {
                let f = inst.resolve_frame(sym.kind, 0, sym.length());
                for l in sym.layers.iter() {
                    if !matches!(l.kind, buzz_scene::LayerKind::Normal) {
                        continue;
                    }
                    for c in l.objects_at(f) {
                        walk(scene, c, t, depth + 1, st);
                    }
                }
            }
        }
        _ => {}
    }
}

fn pct(v: &[(f64, f64)], q: f64) -> f64 {
    // length-weighted percentile of weights
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let total: f64 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for (w, l) in &v {
        acc += l;
        if acc >= total * q {
            return *w;
        }
    }
    v.last().map_or(0.0, |x| x.0)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out).unwrap();
    let tag = std::path::Path::new(&a[0]).file_stem().unwrap().to_string_lossy().to_string();
    let (scene, _) = buzz_import_xfl::import(&a[0]).unwrap();
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    for fr in a[2..].iter().map(|s| s.parse::<u32>().unwrap()) {
        let settings = buzz_export::ExportSettings::for_stage(&scene);
        let f = ex.render(&scene, fr, &settings).unwrap();
        let img = image::RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap();
        img.save(out.join(format!("{tag}_f{fr}.png"))).unwrap();
        let zoom = scene.camera().state_at(fr).map_or(1.0, |c| c.zoom);
        println!("== {tag} frame {fr} camera zoom {zoom:.3}");
        let mut all: BTreeMap<String, Stats> = BTreeMap::new();
        for layer in scene.layers().iter() {
            for o in layer.objects_at(fr) {
                let name = match o.instance().and_then(|i| scene.library().get(i.symbol)) {
                    Some(s) => format!("{} / {}", layer.name, s.name),
                    None => format!("{} / (shape)", layer.name),
                };
                let st = all.entry(name).or_default();
                walk(&scene, o, Affine::scale(zoom), 0, st);
            }
        }
        let is_char = |n: &str| {
            let l = n.to_lowercase();
            ["thakur", "man", "wona", "woman", "jokhini", "shirt", "cycle", "boy", "girl", "oldman", "fly", "pujari", "head"].iter().any(|k| l.contains(k))
        };
        // (width, weight) of dark fill ridges, bg vs char; plus stroke weights
        let mut agg: [Vec<(f64, f64)>; 2] = [Vec::new(), Vec::new()];
        let mut sagg: [Vec<(f64, f64)>; 2] = [Vec::new(), Vec::new()];
        for (name, st) in &all {
            let k = is_char(name) as usize;
            for d in &st.dark {
                if let Some(wd) = buzz_scene::even_lines::measured_line_width(std::slice::from_ref(d)) {
                    let per = d.path.perimeter(0.5);
                    if wd < 14.0 {
                        agg[k].push((wd, per / 2.0));
                    }
                }
            }
            for s in &st.strokes {
                sagg[k].push((s.0, s.1));
            }
        }
        for (k, nm) in ["BG", "CHAR"].iter().enumerate() {
            println!(
                "  {nm} dark-fill ridge px (shapes {} len {:.0}) p10/med/p90 {:.2}/{:.2}/{:.2}   | strokes (n {} len {:.0}) p10/med/p90 {:.2}/{:.2}/{:.2}",
                agg[k].len(), agg[k].iter().map(|x| x.1).sum::<f64>(), pct(&agg[k], 0.1), pct(&agg[k], 0.5), pct(&agg[k], 0.9),
                sagg[k].len(), sagg[k].iter().map(|x| x.1).sum::<f64>(), pct(&sagg[k], 0.1), pct(&sagg[k], 0.5), pct(&sagg[k], 0.9)
            );
        }
        for (name, st) in &all {
            if st.strokes.is_empty() && st.n_fill == 0 {
                continue;
            }
            let wl: Vec<(f64, f64)> = st.strokes.iter().map(|s| (s.0, s.1)).collect();
            let total_len: f64 = wl.iter().map(|x| x.1).sum();
            let mut cols: BTreeMap<[u8; 3], f64> = BTreeMap::new();
            for s in &st.strokes {
                *cols.entry(s.2).or_default() += s.1;
            }
            let mut cv: Vec<_> = cols.into_iter().collect();
            cv.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let cs: Vec<String> = cv.iter().take(3).map(|(c, l)| format!("#{:02x}{:02x}{:02x}:{:.0}%", c[0], c[1], c[2], 100.0 * l / total_len.max(1e-9))).collect();
            println!(
                "{name}: strokes {} len {:.0}px w p10/med/p90 {:.2}/{:.2}/{:.2}  cols {}  fills {} area {:.0} dark {:.0}",
                st.n_stroke,
                total_len,
                pct(&wl, 0.1),
                pct(&wl, 0.5),
                pct(&wl, 0.9),
                cs.join(" "),
                st.n_fill,
                st.fill_area,
                st.dark_fill_area
            );
        }
    }
}
