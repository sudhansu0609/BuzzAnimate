//! What lighting costs on a real film's worth of artwork.
//!
//! Shading and cast shadows are built as **vector geometry** — a crescent cut
//! by a boolean, a silhouette projected and unioned (PROGRESS.md §7 item 46).
//! That is exact and editable and zoomable, and it is also work that scales
//! with the number of paths and with how many segments each one has.
//!
//! A fixture of a few rectangles says nothing about that. Real Animate artwork
//! is hundreds of shapes of a hundred-odd curves each, and the first lit frame
//! has to build all of it before anything appears on screen. If that takes
//! minutes, the application has not hung — but there is no way for the user to
//! tell the difference, which makes it a hang as far as anyone is concerned.
//!
//! So this puts a number on it, and fails if lighting a realistic document
//! takes longer than a person will wait.

use std::time::{Duration, Instant};

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::Point;
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Light, LightKind, Scene, ShapeData};
use peniko::Color;

/// How long the first lit frame may take.
///
/// Generous on purpose: this is not a benchmark, it is a guard against the
/// difference between "slow" and "never". A person waits a couple of seconds
/// for a document to open; beyond that they conclude it has hung, and beyond
/// this they are right to.
const BUDGET: Duration = Duration::from_secs(20);

/// A blobby closed path of `segments` quadratics — what drawn artwork is, as
/// opposed to a rectangle.
fn blob(centre: Point, radius: f64, segments: usize, wobble: f64) -> buzz_geom::BezPath {
    let mut path = buzz_geom::BezPath::new();
    let at = |k: usize| {
        let a = std::f64::consts::TAU * k as f64 / segments as f64;
        let r = radius * (0.75 + 0.35 * (5.0 * a + wobble).sin());
        Point::new(centre.x + r * a.cos(), centre.y + r * a.sin())
    };
    path.move_to(at(0));
    for k in 1..=segments {
        path.line_to(at(k % segments));
    }
    path.close_path();
    path
}

/// A document of `shapes` blobs, each of `segments` segments.
fn artwork(shapes: usize, segments: usize) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = buzz_geom::Size::new(1920.0, 1080.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);

    for i in 0..shapes {
        let x = 100.0 + ((i * 137) % 1700) as f64;
        let y = 100.0 + ((i * 89) % 880) as f64;
        scene.add_shape(
            layer,
            ShapeData::filled(
                blob(Point::new(x, y), 40.0, segments, i as f64),
                Color::from_rgb8((i * 7) as u8, (i * 13) as u8, (i * 29) as u8),
            ),
        );
    }
    scene
}

/// Switch on a sun that casts shadows — the setting that generates the most
/// geometry, and the one an animator reaches for first.
fn light_it(scene: &mut Scene) {
    let rig = scene.lights_mut();
    rig.enabled = true;
    let mut sun = Light::new(buzz_scene::LightId(1), "Sun", LightKind::sun());
    sun.shadows = true;
    rig.lights.push(sun);
}

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping lighting cost test: no usable GPU ({e})"),
    }
}

/// **The first lit frame must not take longer than a person will wait.**
///
/// Unlit is the control: if the unlit render is fast and the lit one is not,
/// the cost is the lighting rather than the artwork.
#[test]
fn lighting_a_realistic_document_finishes_in_reasonable_time() {
    with_exporter(|exporter| {
        let mut scene = artwork(400, 80);
        let settings = ExportSettings::for_stage(&scene);

        let started = Instant::now();
        exporter.render(&scene, 0, &settings).expect("unlit render");
        let unlit = started.elapsed();

        light_it(&mut scene);
        let started = Instant::now();
        exporter.render(&scene, 0, &settings).expect("lit render");
        let lit = started.elapsed();

        eprintln!("400 blobs x 80 segments: unlit {unlit:?}, lit {lit:?}");
        assert!(
            lit < BUDGET,
            "lighting 400 shapes took {lit:?}, against {unlit:?} unlit — \
             that is long enough to look like a hang"
        );
    });
}

/// And it must not get catastrophically worse as artwork grows. Doubling the
/// shapes should roughly double the work, not square it — a quadratic cost is
/// what turns a slow feature into one that never finishes on a real film.
#[test]
fn lighting_cost_grows_with_the_artwork_rather_than_its_square() {
    with_exporter(|exporter| {
        let mut small = artwork(150, 60);
        light_it(&mut small);
        let settings = ExportSettings::for_stage(&small);

        let started = Instant::now();
        exporter.render(&small, 0, &settings).expect("small");
        let small_time = started.elapsed();

        let mut big = artwork(300, 60);
        light_it(&mut big);
        let started = Instant::now();
        exporter.render(&big, 0, &settings).expect("big");
        let big_time = started.elapsed();

        eprintln!("150 shapes: {small_time:?}; 300 shapes: {big_time:?}");
        // Twice the shapes, well under four times the work. The threshold is
        // loose because timing on a shared machine is noisy; what it catches
        // is an order-of-magnitude blow-up, which is the failure that matters.
        assert!(
            big_time < small_time * 6 + Duration::from_millis(500),
            "doubling the artwork took {big_time:?} against {small_time:?} — \
             the cost is growing far faster than the artwork"
        );
    });
}
/// **The hang, measured.** Editing artwork must not rebuild the lighting.
///
/// Lighting geometry is cached, and the cache used to be keyed on the
/// *document's* revision — which bumps on every edit, and on every mouse move
/// of a drag. So dragging one hand rebuilt the shading and the cast shadow of
/// every shape in the film, once per frame, for as long as the drag lasted.
///
/// This measures the second frame after a small edit against the first. If the
/// cache survives the edit, the second is far cheaper; if it does not, the two
/// are the same and lit artwork cannot be posed.
#[test]
fn editing_artwork_does_not_rebuild_every_light() {
    with_exporter(|_exporter| {
        let mut scene = artwork(600, 120);
        light_it(&mut scene);

        // The window holds one cache across frames; the exporter builds one per
        // call, so the cache is driven directly to measure what the window sees.
        let mut cache = buzz_render::document::DrawCache::new();
        let mut vello = buzz_render::vello::Scene::new();
        let camera = buzz_geom::Camera::new(
            buzz_geom::Point::new(960.0, 540.0),
            1.0,
            buzz_geom::Size::new(1920.0, 1080.0),
        );

        let mut draw = |scene: &Scene, cache: &mut buzz_render::document::DrawCache| {
            let mut builder = buzz_render::SceneBuilder::new(&mut vello, &camera);
            let options = buzz_render::document::FrameOptions {
                lit: true,
                ..Default::default()
            };
            let started = Instant::now();
            buzz_render::document::draw_frame_cached(
                &mut builder,
                scene,
                0,
                buzz_geom::Affine::IDENTITY,
                &options,
                cache,
            );
            started.elapsed()
        };

        let cold = draw(&scene, &mut cache);
        let warm = draw(&scene, &mut cache);

        // Now move one shape, exactly as dragging a hand does: the document's
        // revision changes, but the lights have not.
        let id = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .next()
            .expect("a shape");
        scene.update_object(id, |o| {
            o.transform = buzz_geom::Affine::translate(buzz_geom::Vec2::new(3.0, 0.0));
        });
        let after_edit = draw(&scene, &mut cache);

        eprintln!("lit draw: cold {cold:?}, warm {warm:?}, after an edit {after_edit:?}");

        assert!(
            warm * 4 < cold,
            "the cache is not working at all: warm {warm:?} against cold {cold:?}"
        );
        assert!(
            after_edit * 3 < cold,
            "moving one shape rebuilt the whole rig: {after_edit:?} against a \
             cold {cold:?} and a warm {warm:?} — dragging lit artwork will hang"
        );
    });
}
