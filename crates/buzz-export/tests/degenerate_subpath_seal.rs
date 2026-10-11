//! **A subpath that encloses nothing draws nothing** (A3 of
//! docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md). A boolean result can carry a subpath of
//! three points a few millionths of a pixel apart beside its real outline. The fill
//! paints nothing there, but the seam seal (`SceneBuilder::fill_shape_paint_sealed`)
//! stroked it, and in f32 its points are one point: the stroke's winding leaked along
//! its tile row and filled a band of tiles in the fill's colour out to the edge of the
//! path's bounds — the 96 × 14 px skin-coloured rectangle under Raju's right hand
//! (render x 96–191, y 434–447). This renders that subpath, verbatim, beside an outline
//! with the same right and bottom edges, and requires nothing to be painted past the
//! outline's own bounds.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use kurbo::BezPath;
use peniko::Color;

#[test]
fn a_degenerate_subpath_is_not_sealed_into_a_band() {
    let mut exporter = match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping degenerate-subpath seal test: no usable GPU ({e})");
            return;
        }
    };
    let outline = Rect::new(43.0, 393.0, 191.0, 444.0);
    let mut path = outline.to_path(1e-9);
    path.extend(
        BezPath::from_svg(
            "M92.41563286635125,432.1693449456416 L92.4156357273742,432.16934470722305 \
             L92.4156356081649,432.16934470722305 Z",
        )
        .unwrap(),
    );

    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(1024.0, 1536.0);
    scene.stage_mut().background = Color::from_rgb8(255, 0, 255);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(layer, ShapeData::filled(path, Color::from_rgb8(249, 162, 99)));
    let mut settings = ExportSettings::for_stage(&scene);
    settings.width = 1024;
    settings.height = 1536;
    let frame = exporter.render(&scene, 0, &settings).unwrap();

    // Painted = further than 8 from the magenta stage in some channel; allowed only
    // within the outline grown by 2 px (its anti-aliasing and seal).
    let allowed = outline.inflate(2.0, 2.0);
    let (mut outside, mut first) = (0usize, None);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let i = ((y * frame.width + x) * 4) as usize;
            let p = &frame.pixels[i..i + 3];
            let painted = p[0] < 247 || p[1] > 8 || p[2] < 247;
            if painted && !allowed.contains(kurbo::Point::new(x as f64 + 0.5, y as f64 + 0.5)) {
                outside += 1;
                first.get_or_insert((x, y, [p[0], p[1], p[2]]));
            }
        }
    }
    assert_eq!(outside, 0, "{outside} px painted outside the outline, first {first:?}");
    // The outline itself is drawn.
    let centre = ((418 * frame.width + 117) * 4) as usize;
    assert_eq!(&frame.pixels[centre..centre + 3], &[249, 162, 99]);
}
