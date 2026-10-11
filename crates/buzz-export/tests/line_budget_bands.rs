//! **A frame too big for one GPU pass still has to draw** (Phase 4d).
//!
//! `DrawCache::over_budget` — the ladder still over either the segment
//! ceiling or [`buzz_render::document::LINE_BUDGET`] even at
//! `LightDetail::Flat`, the bottom of the automatic ladder — sends the
//! exporter into `Exporter::render_bands`: the same frame, split into
//! horizontal strips each culled to its own rows and rendered as its own
//! Vello pass, then stitched back into one picture. The fixture is many
//! thin, segment-heavy
//! rows stacked down the stage, wide enough to overrun both budgets on their
//! own (no lighting to trim, so the ladder cannot draw its way under either
//! one) while staying almost entirely opaque — so a picture that comes back
//! mostly transparent means the bands did not actually draw it.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{BezPath, Point, Size};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

/// A wide, thin row whose top edge zigzags — many straight segments, and (bar
/// a thin strip at the very top) a solid fill underneath them, so the row's
/// own rectangle stays almost entirely covered.
fn zigzag_row(y0: f64, y1: f64, width: f64, segments: usize) -> BezPath {
    let amp = (y1 - y0) * 0.05;
    let mut path = BezPath::new();
    path.move_to(Point::new(0.0, y1));
    path.line_to(Point::new(width, y1));
    for i in 0..=segments {
        let t = i as f64 / segments as f64;
        let x = width * (1.0 - t);
        let jag = if i % 2 == 0 { 0.0 } else { amp };
        path.line_to(Point::new(x, y0 + jag));
    }
    path.close_path();
    path
}

/// A document whose artwork alone — nothing lit, so there is no lighting for
/// the ladder to trim before it settles at `Flat`, still sealed — overruns
/// both `segment_ceiling` and `LINE_BUDGET` at a 512x512 export. Built from
/// many rows stacked down the stage, so a horizontal band's cull genuinely
/// drops most of them rather than every band re-encoding the whole thing.
fn overloaded_scene() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(512.0, 512.0);
    let layer = scene.add_layer("Rows", LayerKind::Normal);

    // 220 rows x 10,000 segments each is 2.2M raw fill lines — comfortably
    // past both the ~1.46M segment ceiling and the 1.8M line budget at
    // 512x512 even sealed, since the automatic ladder no longer gives up the
    // seal to buy headroom (`docs/RENDER_SEAMS_PLAN.md` Step 1).
    const ROWS: usize = 220;
    const SEGMENTS_PER_ROW: usize = 10_000;
    let row_height = 512.0 / ROWS as f64;
    for i in 0..ROWS {
        let y0 = i as f64 * row_height;
        let y1 = y0 + row_height;
        let path = zigzag_row(y0, y1, 512.0, SEGMENTS_PER_ROW);
        scene.add_shape(
            layer,
            ShapeData::filled(path, Color::from_rgb8(0x20, 0x40, 0x80)),
        );
    }
    scene
}

#[test]
fn a_frame_over_both_budgets_still_exports_mostly_opaque() {
    let mut exporter = match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping over-budget band test: no usable GPU ({e})");
            return;
        }
    };
    // P1 (`docs/IMPORT_PERF_PLAN.md`) scales `segment_ceiling`/`line_budget`
    // by the adapter's own transient buffer scale, which `Exporter::new`
    // above just picked (up to 8x on a big GPU). P2 moved that scale onto
    // this exporter's own `DrawCache`, so it is pinned back here rather than
    // through the process-wide `vello_encoding::set_buffer_scale` — which
    // only ever rises now, and would race every other GPU context in the
    // suite — so this fixture's fixed geometry stays over budget regardless
    // of which GPU runs the suite. That is what this test means to
    // exercise, not the scale picker itself.
    exporter.set_budget_scale(1);

    let scene = overloaded_scene();
    let settings = ExportSettings {
        width: 512,
        height: 512,
        // No background to paint over: a transparent pixel afterwards can
        // only mean a band failed to draw there.
        transparent: true,
        region: None,
    };

    let frame = exporter
        .render(&scene, 0, &settings)
        .expect("export a frame over both budgets");

    assert!(
        exporter.lights_over_budget(),
        "the fixture must actually be over budget, or this test proves nothing"
    );

    let total = (frame.width * frame.height) as usize;
    let mut opaque = 0usize;
    for y in 0..frame.height {
        for x in 0..frame.width {
            if frame.pixel(x, y)[3] > 0 {
                opaque += 1;
            }
        }
    }
    let fraction = opaque as f64 / total as f64;
    assert!(
        fraction > 0.95,
        "expected > 95% opaque pixels once the bands were stitched back \
         together, got {:.1}% ({opaque}/{total})",
        fraction * 100.0
    );
}
