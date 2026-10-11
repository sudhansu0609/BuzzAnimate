//! Step 1 of `docs/RENDER_SEAMS_PLAN.md`, measured: a frame over budget at
//! `LightDetail::Flat` still exports with every seam sealed.
//!
//! # The regression this guards against
//!
//! Before that plan, `LightDetail::down` walked a frame that stayed over
//! budget at `Flat` one level further, to `Unsealed`, which drops the
//! half-pixel stroke `SceneBuilder::fill_shape_paint_sealed` adds to every
//! opaque fill (see `LightDetail`'s doc in `crates/buzz-render/src/document.rs`).
//! Zooming out puts more artwork on screen, which is exactly what pushes a
//! frame over budget, so zooming out opened a white hairline wherever two
//! colours met. Phase 4's banded rendering
//! (`docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`) makes an over-budget frame safe
//! without giving anything up, so the automatic ladder now stops at `Flat`
//! and hands an over-budget frame to `Exporter::render_bands` instead — still
//! sealed.
//!
//! The fixture pairs two abutting opaque squares — the seam under test — with
//! enough off-edge-but-visible padding geometry (many thin zigzag rows, the
//! same trick `line_budget_bands.rs` uses) to push the frame's own artwork
//! over both `segment_ceiling` and `LINE_BUDGET`, forcing band mode.
//!
//! Skips cleanly when no GPU is available.

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{BezPath, Point, Rect, Shape as _, Size};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xC0, 0x20, 0x20);
const BLUE: Color = Color::from_rgb8(0x20, 0x20, 0xC0);

/// A wide, thin row whose top edge zigzags — many straight segments over an
/// otherwise solid fill — the same shape `line_budget_bands::zigzag_row`
/// builds, copied here so this fixture forces the same over-budget path
/// without depending on another test file.
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

/// Two rectangles meeting at `x = 255.5` in a clear strip at the top of the
/// stage, on a white background, plus 220 rows of 10,000-segment padding
/// below them — the same 2.2M raw fill lines `line_budget_bands.rs` uses to
/// overrun both `segment_ceiling` and `LINE_BUDGET` at this size, comfortably
/// past even with nothing given up: the ladder has nowhere automatic left to
/// go once it is `Flat`.
fn scene_over_budget_at_flat() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(512.0, 612.0);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(56.0, 10.0, 255.5, 90.0).to_path(1e-9), RED),
    );
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(255.5, 10.0, 456.0, 90.0).to_path(1e-9), BLUE),
    );

    const ROWS: usize = 220;
    const SEGMENTS_PER_ROW: usize = 10_000;
    let row_height = 512.0 / ROWS as f64;
    for i in 0..ROWS {
        let y0 = 100.0 + i as f64 * row_height;
        let y1 = y0 + row_height;
        let path = zigzag_row(y0, y1, 512.0, SEGMENTS_PER_ROW);
        scene.add_shape(
            layer,
            ShapeData::filled(path, Color::from_rgb8(0x20, 0x40, 0x80)),
        );
    }
    scene
}

/// Average of the three colour channels — a scalar "how light is this pixel",
/// which is what a white seam raises.
fn lightness(pixel: [u8; 4]) -> f64 {
    (f64::from(pixel[0]) + f64::from(pixel[1]) + f64::from(pixel[2])) / 3.0
}

#[test]
fn a_scene_over_budget_at_flat_still_seals_its_seam() {
    let mut exporter = match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping seam-under-bands test: no usable GPU ({e})");
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

    let scene = scene_over_budget_at_flat();
    let settings = ExportSettings {
        width: 512,
        height: 612,
        transparent: false,
        region: None,
    };
    let frame = exporter
        .render(&scene, 0, &settings)
        .expect("export a frame over budget at Flat");

    assert!(
        exporter.lights_over_budget(),
        "the fixture must actually be over budget at Flat, or this test proves nothing"
    );

    // Deep inside each square — clear of the seam and of the padding rows
    // below — as the reference for "how light is this colour", so the test
    // does not hard-code a shade.
    let red_lightness = lightness(frame.pixel(150, 50));
    let blue_lightness = lightness(frame.pixel(360, 50));
    let reference = red_lightness.max(blue_lightness);

    // Sweep the columns either side of the join, down the height of the
    // squares. The worst pixel is the one that matters: a seam one column
    // wide is exactly the complaint.
    let split = 255.5f64;
    let mut worst = 0.0f64;
    let mut worst_at = (0, 0);
    for x in (split as u32).saturating_sub(3)..=(split as u32 + 3) {
        for y in 20..80 {
            let l = lightness(frame.pixel(x, y));
            if l > worst {
                worst = l;
                worst_at = (x, y);
            }
        }
    }

    assert!(
        worst <= reference + 8.0,
        "a pale seam runs along the join even in band mode: pixel at \
         {worst_at:?} is {worst:.1}, more than 8/255 lighter than either \
         neighbouring colour ({reference:.1})"
    );
}
