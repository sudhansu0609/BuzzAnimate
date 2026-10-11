//! **The exporter's cull is conservative.** `Exporter::draw_into_target` now
//! culls to the output rectangle (grown by a safety margin) instead of
//! encoding the whole document however far it runs past the frame — see
//! `buzz_render::document::FrameOptions::cull`. A cull that removed geometry a
//! genuinely unculled render would have shown would be a wrong picture, not a
//! saved millisecond, so the only honest proof is pixels: render the same
//! document — artwork both on stage and far off it — with the cull on and
//! with it forced off, and compare.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_render::document::FrameOptions;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping export-cull parity test: no usable GPU ({e})"),
    }
}

#[test]
fn culling_to_the_viewport_matches_no_cull() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let layer = scene.add_layer("Art", LayerKind::Normal);
        // On stage — must survive the cull unchanged.
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(100.0, 100.0, 300.0, 300.0).to_path(1e-9),
                Color::from_rgb8(0x20, 0x40, 0xE0),
            ),
        );
        // Far off the stage — invisible either way once the camera frames the
        // stage, but this is what the cull is meant to skip encoding at all.
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(100_000.0, 100_000.0, 100_200.0, 100_200.0).to_path(1e-9),
                Color::from_rgb8(0xE0, 0x20, 0x20),
            ),
        );

        let settings = ExportSettings::for_stage(&scene);
        // The film's own options: lit, and no authoring aid — the same ones
        // `Exporter::render` passes, spelled out so `cull` can be set below.
        let options = FrameOptions {
            lit: true,
            guides: false,
            cull: None,
            ..FrameOptions::default()
        };

        exporter.reset_caches();
        exporter.set_cull(true);
        let culled = exporter
            .render_with(&scene, 0, &settings, &options)
            .expect("render, culled");

        // Each render is an unrelated frame as far as the caches are
        // concerned, exactly as `headless_symbol_cache.rs`'s parity tests
        // reset between an off/on comparison.
        exporter.reset_caches();
        exporter.set_cull(false);
        let unculled = exporter
            .render_with(&scene, 0, &settings, &options)
            .expect("render, unculled");

        assert_eq!(
            (culled.width, culled.height),
            (unculled.width, unculled.height),
            "the two renders differ in size"
        );
        assert_eq!(
            culled.pixels, unculled.pixels,
            "a conservative cull must not change a single pixel"
        );
    });
}
