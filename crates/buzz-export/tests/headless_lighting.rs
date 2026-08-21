//! Prove the lights on the real GPU: an unlit document is untouched, a sun
//! shades and casts, and moving the sun moves what it does.
//!
//! Lighting is the one feature where "it compiles and the numbers look right"
//! proves nothing at all. What matters is whether the picture changes, in the
//! direction the light points — and these read the pixels back to say so.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, LightKind, Scene, ShapeData};
use peniko::Color;

const ART: Color = Color::from_rgb8(0xC0, 0xC0, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping lighting test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A white stage with one grey square in the middle, and room around it for a
/// shadow to fall into.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART),
    );
    scene
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

fn luma(pixel: [u8; 4]) -> f32 {
    0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32
}

/// The promise every existing document depends on: no lights, no change.
#[test]
fn a_document_without_lights_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let scene = document();
        let before = render(exporter, &scene);

        // A rig that exists but holds nothing must also change nothing.
        let mut with_rig = scene.clone();
        let _ = with_rig.lights_mut();
        let after = render(exporter, &with_rig);

        assert_eq!(
            before.pixels, after.pixels,
            "an unlit document must be pixel-identical"
        );
    });
}

/// Switching a sun on has to *do* something — and the artwork must still be
/// recognisable rather than going black.
#[test]
fn adding_a_sun_changes_the_picture_without_destroying_it() {
    with_exporter(|exporter| {
        let plain = document();
        let unlit = render(exporter, &plain);

        let mut lit_scene = document();
        lit_scene.add_light(LightKind::sun());
        let lit = render(exporter, &lit_scene);

        assert_ne!(unlit.pixels, lit.pixels, "the sun did nothing at all");

        let middle = lit.pixel(275, 225);
        assert!(
            luma(middle) > 40.0,
            "the artwork went almost black: {middle:?}"
        );
    });
}

/// The whole point: the shaded side is opposite the light, and swapping the
/// sun's side swaps which edge is dark.
#[test]
fn the_shaded_side_follows_the_sun() {
    with_exporter(|exporter| {
        let mut from_left = document();
        // Azimuth pi: the sun lies towards -x, so the left edge is lit.
        let id = from_left.add_light(LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.5,
        });
        from_left.lights_mut().get_mut(id).expect("the sun").shadows = false;

        let mut from_right = from_left.clone();
        from_right.lights_mut().get_mut(id).expect("the sun").kind = LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.5,
        };

        let left_lit = render(exporter, &from_left);
        let right_lit = render(exporter, &from_right);

        // Just inside each edge of the square, at its middle height.
        let sample = |frame: &Frame, x: u32| luma(frame.pixel(x, 225));
        let (left_edge, right_edge) = (215, 335);

        assert!(
            sample(&left_lit, left_edge) > sample(&left_lit, right_edge) + 4.0,
            "lit from the left, the left edge should be brighter: {} vs {}",
            sample(&left_lit, left_edge),
            sample(&left_lit, right_edge)
        );
        assert!(
            sample(&right_lit, right_edge) > sample(&right_lit, left_edge) + 4.0,
            "lit from the right, the right edge should be brighter: {} vs {}",
            sample(&right_lit, right_edge),
            sample(&right_lit, left_edge)
        );
    });
}

/// A cast shadow lands on the background, on the far side from the sun, and
/// swings round when the sun does.
#[test]
fn the_cast_shadow_falls_away_from_the_sun_and_swings_with_it() {
    with_exporter(|exporter| {
        let mut scene = document();
        let id = scene.add_light(LightKind::Sun {
            // Towards +x and low: the shadow falls to the left of the square.
            azimuth: 0.0,
            elevation: 0.45,
        });
        {
            let sun = scene.lights_mut().get_mut(id).expect("the sun");
            sun.standing_height = 60.0;
            sun.shadow_strength = 0.8;
        }

        let cast_left = render(exporter, &scene);
        let left_of = luma(cast_left.pixel(170, 225));
        let right_of = luma(cast_left.pixel(380, 225));
        assert!(
            left_of < right_of - 20.0,
            "the shadow should darken the background to the left: {left_of} vs {right_of}"
        );

        // Turn the sun around: the shadow must move to the other side.
        scene.lights_mut().get_mut(id).expect("the sun").kind = LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.45,
        };
        let cast_right = render(exporter, &scene);
        let left_of = luma(cast_right.pixel(170, 225));
        let right_of = luma(cast_right.pixel(380, 225));
        assert!(
            right_of < left_of - 20.0,
            "the shadow should have swung to the right: {left_of} vs {right_of}"
        );
    });
}

/// A lower sun throws a longer shadow — what makes a light feel like a light
/// rather than a drop-shadow filter.
#[test]
fn a_lower_sun_throws_a_longer_shadow() {
    with_exporter(|exporter| {
        let mut reach = |elevation: f64| {
            let mut scene = document();
            let id = scene.add_light(LightKind::Sun {
                azimuth: 0.0,
                elevation,
            });
            {
                let sun = scene.lights_mut().get_mut(id).expect("the sun");
                sun.standing_height = 50.0;
                sun.shadow_strength = 0.9;
            }
            let frame = render(exporter, &scene);

            // How far left of the square the darkening *reaches*: scan
            // inwards from the stage edge and stop at the first dark pixel.
            // Scanning outwards from the square instead measures where the
            // shadow starts, which is the same for every elevation.
            (0..200u32)
                .find(|x| luma(frame.pixel(*x, 225)) < 200.0)
                .unwrap_or(200)
        };

        let high = reach(1.2);
        let low = reach(0.35);
        assert!(
            low < high,
            "a lower sun should reach further left: low starts at x={low}, high at x={high}"
        );
    });
}

/// A warm light makes the artwork warm; a cold one makes it cold.
#[test]
fn the_lights_colour_reaches_the_artwork() {
    with_exporter(|exporter| {
        let mut tint = |colour: Color| {
            let mut scene = document();
            let id = scene.add_light(LightKind::Sun {
                azimuth: 0.0,
                elevation: 1.3,
            });
            {
                let sun = scene.lights_mut().get_mut(id).expect("the sun");
                sun.color = colour;
                sun.intensity = 1.4;
                sun.shadows = false;
            }
            render(exporter, &scene).pixel(275, 225)
        };

        let warm = tint(Color::from_rgb8(0xFF, 0x80, 0x20));
        let cold = tint(Color::from_rgb8(0x20, 0x80, 0xFF));

        assert!(
            warm[0] > cold[0] && cold[2] > warm[2],
            "a warm light should read warmer than a cold one: {warm:?} vs {cold:?}"
        );
    });
}

/// A lamp lights what is near it more than what is far — the difference
/// between a lamp and a sun, in one picture.
#[test]
fn a_lamp_lights_what_is_near_it_more_than_what_is_far() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::from_rgb8(0x20, 0x20, 0x20);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        // Two identical squares: one beside the lamp, one across the stage.
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(40.0, 170.0, 140.0, 270.0).to_path(1e-9), ART),
        );
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(420.0, 170.0, 520.0, 270.0).to_path(1e-9), ART),
        );

        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(60.0, 220.0),
            height: 120.0,
            radius: 180.0,
        });
        {
            let lamp = scene.lights_mut().get_mut(id).expect("the lamp");
            lamp.intensity = 3.0;
            lamp.shadows = false;
        }
        scene.lights_mut().base = Color::from_rgb8(0x18, 0x18, 0x18);

        let frame = render(exporter, &scene);
        let near = luma(frame.pixel(90, 220));
        let far = luma(frame.pixel(470, 220));

        assert!(
            near > far + 15.0,
            "the near square should be brighter than the far one: {near} vs {far}"
        );
    });
}

/// A sky is fill, not direction: it lights everything and casts nothing.
#[test]
fn a_sky_fills_without_casting() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.add_light(LightKind::sky());
        scene.lights_mut().base = Color::BLACK;

        let frame = render(exporter, &scene);

        assert!(
            luma(frame.pixel(275, 225)) > 20.0,
            "the sky should light it"
        );

        let left = luma(frame.pixel(170, 225));
        let right = luma(frame.pixel(380, 225));
        assert!(
            (left - right).abs() < 3.0,
            "a sky must not cast a shadow: {left} vs {right}"
        );
    });
}

/// **A symbol instance casts a shadow.**
///
/// It used to cast nothing at all, and the note in the renderer said so. That
/// reads as a small gap and is not one: a document imported from Animate is
/// *entirely* symbol instances, so "an instance casts nothing" means a real
/// film casts no shadows whatever — switching shadows on did visibly nothing,
/// which looks like the feature being broken rather than unfinished.
///
/// The same square as every other test here, placed as an instance of a symbol
/// instead of drawn loose, and asserted the same way: darker on the far side
/// from the sun.
#[test]
fn a_symbol_instance_casts_a_shadow_like_loose_artwork() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        // The artwork, inside a symbol. `layers()` follows the open symbol, so
        // drawing into one is the same call as drawing on the stage.
        let symbol = scene.add_symbol("Block", buzz_scene::SymbolKind::Graphic, None);
        assert!(scene.enter_symbol(symbol));
        let inner = scene
            .layers()
            .iter()
            .next()
            .expect("a new symbol has a layer")
            .id;
        scene
            .add_shape(
                inner,
                ShapeData::filled(Rect::new(0.0, 0.0, 150.0, 150.0).to_path(1e-9), ART),
            )
            .expect("the artwork");
        scene.exit_symbol();

        // Placed where `document()` draws its loose square.
        let layer = scene.add_layer("Cast", LayerKind::Normal);
        scene
            .add_instance_at(
                layer,
                0,
                symbol,
                buzz_geom::Affine::translate((200.0, 150.0)),
            )
            .expect("the instance");

        let id = scene.add_light(LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.45,
        });
        {
            let sun = scene.lights_mut().get_mut(id).expect("the sun");
            sun.standing_height = 60.0;
            sun.shadow_strength = 0.8;
        }

        let frame = render(exporter, &scene);
        let left_of = luma(frame.pixel(170, 225));
        let right_of = luma(frame.pixel(380, 225));
        assert!(
            left_of < right_of - 20.0,
            "an instance should cast a shadow to the left just as loose artwork \
             does: {left_of} vs {right_of}"
        );
    });
}

/// **A lamp lays a pool of light across one surface.**
///
/// The test above uses two separate squares, which the old model passed while
/// being unable to light *within* a shape at all: illumination was evaluated
/// once per shape, at that shape's middle, so a wall under a lamp came out one
/// flat colour from end to end. On a 550-unit wall with the lamp a hundred units
/// from the left edge, the pixels at x = 100 and x = 520 were identical.
///
/// That is the difference between a lamp and a tint, so it is asserted across a
/// single shape: bright under the lamp, and falling off all the way out.
#[test]
fn a_lamp_lays_a_pool_across_one_surface() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let layer = scene.add_layer("Wall", LayerKind::Normal);
        // One wall, filling the stage, so every sample below is the same shape.
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), ART),
        );
        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(100.0, 200.0),
            height: 120.0,
            radius: 160.0,
        });
        // No shadow and no crescent: this is about the light itself, and both of
        // those would put their own edges across the samples.
        {
            let lamp = scene.lights_mut().get_mut(id).expect("the lamp");
            lamp.shadows = false;
        }
        scene.lights_mut().modelling = 0.0;

        let frame = render(exporter, &scene);
        let y = 200;
        let under = luma(frame.pixel(100, y));
        let near = luma(frame.pixel(200, y));
        let mid = luma(frame.pixel(300, y));
        let far = luma(frame.pixel(520, y));

        assert!(
            under > near && near > mid && mid > far,
            "the pool must fall off across the wall, not sit flat on it: \
             {under} then {near} then {mid} then {far}"
        );
        assert!(
            under > far + 60.0,
            "and the falloff must be worth seeing: {under} under the lamp \
             against {far} across the room"
        );
    });
}

/// **A lamp glows in the air**, so an empty stage shows where the light is.
///
/// Nothing but the background: if the pool needed artwork to land on, a lamp in
/// an empty shot would be invisible and the gizmo would be the only sign of it.
#[test]
fn a_lamp_glows_over_an_empty_stage() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::from_rgb8(0x20, 0x22, 0x28);
        let _ = scene.add_layer("Empty", LayerKind::Normal);

        let dark = render(exporter, &scene);

        let mut lit = scene.clone();
        lit.add_light(LightKind::Lamp {
            position: Point::new(275.0, 200.0),
            height: 120.0,
            radius: 160.0,
        });
        let glowing = render(exporter, &lit);

        let before = luma(dark.pixel(275, 200));
        let centre = luma(glowing.pixel(275, 200));
        let edge = luma(glowing.pixel(545, 200));

        assert!(
            centre > before + 60.0,
            "the lamp should light the air at its own position: {before} to {centre}"
        );
        assert!(
            centre > edge + 40.0,
            "and fade out across the stage: {centre} at the lamp, {edge} at the edge"
        );
    });
}

/// A lamp with its glow turned off draws no pool — and still shades and casts,
/// which is the whole reason the control is separate from the light's strength.
#[test]
fn a_lamp_with_no_glow_still_casts() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.stage_mut().background = Color::from_rgb8(0x20, 0x22, 0x28);
        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(120.0, 100.0),
            height: 200.0,
            radius: 400.0,
        });
        let lit = render(exporter, &scene);

        scene.lights_mut().get_mut(id).expect("the lamp").glow = 0.0;
        let dimmed = render(exporter, &scene);

        // The empty corner the pool reached is back to the bare background.
        let background = luma(dimmed.pixel(60, 40));
        let was = luma(lit.pixel(60, 40));
        assert!(
            was > background + 20.0,
            "the pool should have been lighting that corner: {was} against {background}"
        );

        // But the shadow is still thrown: somewhere below the square is darker
        // than the stage around it.
        let shadow = luma(dimmed.pixel(300, 330));
        assert!(
            shadow < background + 4.0,
            "a lamp with no glow must still cast: {shadow} against {background}"
        );
    });
}

/// **A sun added from the panel throws a shadow you can see.**
///
/// The default was a sun 52° up, and a shadow is the caster's standing height
/// over the tangent of the elevation: at 52° with the artwork standing 40 off
/// the background, that is 31 units, which on any real drawing lands entirely
/// *underneath* the thing that cast it. The first thing anyone does after adding
/// a sun is look for the shadow, and finding none reads as the sun not working.
///
/// So this asserts the default, not a hand-tuned rig: add a sun the way the
/// panel's button does, and there must be a shadow on the floor.
#[test]
fn a_sun_at_its_defaults_throws_a_visible_shadow() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.add_light(LightKind::sun());
        let frame = render(exporter, &scene);

        // The square sits at 200..350 across and 150..300 down; the floor below
        // and to one side of it is where a shadow from a sun up and to the right
        // must land.
        let floor = (150..200)
            .map(|x| luma(frame.pixel(x, 310)))
            .fold(f32::MAX, f32::min);
        let stage = luma(frame.pixel(40, 40));

        assert!(
            floor < stage - 30.0,
            "no shadow on the floor beside the square: {floor} against a bare \
             stage at {stage}"
        );
    });
}

/// **A sky's Strength does something.**
///
/// It used to be folded into the colour with `multiply_alpha`, which moves the
/// alpha channel — and the only thing that reads a sky's colour takes the three
/// colour channels and drops the alpha. So the one control that could make a
/// sky brighter moved a number nothing read, at every setting.
#[test]
fn a_skys_strength_changes_the_picture() {
    with_exporter(|exporter| {
        let mut scene = document();
        let id = scene.add_light(LightKind::sky());
        scene.lights_mut().base = Color::from_rgb8(0x20, 0x20, 0x20);

        let read = |exporter: &mut Exporter, scene: &Scene| luma(render(exporter, scene).pixel(275, 225));

        scene.lights_mut().get_mut(id).expect("the sky").intensity = 0.25;
        let dim = read(exporter, &scene);
        scene.lights_mut().get_mut(id).expect("the sky").intensity = 1.0;
        let mid = read(exporter, &scene);
        scene.lights_mut().get_mut(id).expect("the sky").intensity = 3.0;
        let bright = read(exporter, &scene);

        assert!(
            bright > mid + 20.0 && mid > dim + 20.0,
            "a sky's strength must light the artwork: {dim} then {mid} then {bright}"
        );
    });
}

/// **The side of a character facing a lamp is the bright one.**
///
/// The report this exists for: a lamp lit a character evenly, with no bright
/// side and no dark side. The rig was right and the *placement* was not — a new
/// lamp arrived at the middle of the view, which is the one position with no
/// direction in the plane, so there was no crescent to draw and the pool was
/// symmetrical. This pins the behaviour rather than the placement: a lamp to one
/// side must light that side.
#[test]
fn the_side_of_a_character_facing_a_lamp_is_brighter() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::from_rgb8(0x30, 0x30, 0x34);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(215.0, 100.0, 335.0, 300.0).to_path(1e-9), ART),
        );
        scene.add_light(LightKind::Lamp {
            position: Point::new(120.0, 80.0),
            height: 160.0,
            radius: 320.0,
        });

        let frame = render(exporter, &scene);
        let facing = luma(frame.pixel(222, 200));
        let away = luma(frame.pixel(328, 200));

        assert!(
            facing > away + 40.0,
            "the lamp side of the body should be the lit one: {facing} facing \
             the lamp against {away} away from it"
        );
    });
}

/// **A gloom darkens the side it rolls in from, and only that side.**
///
/// The whole claim of a wall of dark: it has a direction, and turning it round
/// swaps which end of the picture is buried. Read off the stage rather than off
/// the artwork, because it falls on everything the frame contains — which is
/// the difference between darkness and a shading crescent.
#[test]
fn a_gloom_darkens_the_side_it_rolls_in_from() {
    with_exporter(|exporter| {
        let mut from_left = document();
        let id = from_left.add_light(LightKind::Gloom {
            edge: Point::new(-40.0, 0.0),
            facing: 0.0,
            throw: 320.0,
            width: 4000.0,
        });

        let mut from_right = from_left.clone();
        from_right.lights_mut().get_mut(id).expect("the gloom").kind = LightKind::Gloom {
            edge: Point::new(590.0, 0.0),
            facing: std::f64::consts::PI,
            throw: 320.0,
            width: 4000.0,
        };

        let left = render(exporter, &from_left);
        let right = render(exporter, &from_right);

        let (near, far) = ((20, 200), (520, 200));

        let (buried, clear) = (
            luma(left.pixel(near.0, near.1)),
            luma(left.pixel(far.0, far.1)),
        );
        assert!(
            buried < clear - 40.0,
            "the wall stands off the left, so the left is the dark end: \
             {buried} against {clear}"
        );

        let (buried, clear) = (
            luma(right.pixel(far.0, far.1)),
            luma(right.pixel(near.0, near.1)),
        );
        assert!(
            buried < clear - 40.0,
            "turned round, the dark end must swap with it: {buried} against {clear}"
        );
    });
}

/// Past the end of its throw a gloom does nothing at all.
///
/// This is what makes one aimable. A wall of dark that faded to *almost*
/// nothing over the whole document would be a wash over the picture with a
/// gradient in it, and there would be no way to light a shot against it.
#[test]
fn a_gloom_leaves_the_picture_alone_past_its_throw() {
    with_exporter(|exporter| {
        let mut lit = document();
        lit.add_light(LightKind::sun());
        let without = render(exporter, &lit);

        let mut darkened = lit.clone();
        darkened.add_light(LightKind::Gloom {
            edge: Point::new(-40.0, 0.0),
            facing: 0.0,
            throw: 320.0,
            width: 4000.0,
        });
        let with = render(exporter, &darkened);

        let (before, after) = (
            luma(without.pixel(520, 200)),
            luma(with.pixel(520, 200)),
        );
        assert!(
            (before - after).abs() < 2.0,
            "two hundred units past the end of the throw, the picture must be \
             what it was: {before} became {after}"
        );

        let (before, after) = (luma(without.pixel(20, 200)), luma(with.pixel(20, 200)));
        assert!(
            after < before - 40.0,
            "at the wall it must be markedly darker: {before} became {after}"
        );
    });
}

/// A single square, centred, with room around it — so the two vertical edges of
/// one shape can be compared against each other and against its middle.
fn slab() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x22, 0x22, 0x26);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(200.0, 120.0, 350.0, 300.0).to_path(1e-9), ART),
    );
    scene
}

/// The mean brightness of a column of the artwork, in document x.
fn column(frame: &Frame, x: u32) -> f32 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for y in 140..280 {
        sum += luma(frame.pixel(x, y));
        n += 1;
    }
    sum / n as f32
}

/// **The edge facing the light catches it.**
///
/// The report: a lamp changes the overall colour of a drawing and nothing else,
/// so it reads as a tint rather than as light. What makes light read as light on
/// a flat drawing is the *edge*: the side of a figure facing the lamp is
/// brighter than the middle of the same figure, and the far edge is not.
#[test]
fn the_edge_facing_a_lamp_is_brighter_than_the_middle() {
    with_exporter(|exporter| {
        let mut scene = slab();
        // Off to the left, level with the slab.
        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(60.0, 200.0),
            height: 90.0,
            radius: 400.0,
        });
        scene.lights_mut().get_mut(id).expect("the lamp").shadows = false;

        let frame = render(exporter, &scene);
        let near = column(&frame, 203);
        let middle = column(&frame, 275);
        let far = column(&frame, 347);

        assert!(
            near > middle + 6.0,
            "the edge facing the lamp must catch it: edge {near:.1}, middle {middle:.1}"
        );
        assert!(
            near > far + 12.0,
            "and the far edge must not: near {near:.1}, far {far:.1}"
        );
    });
}

/// Turn the lamp round and the lit edge swaps with it. A rim that stayed put
/// would be an outline drawn on the artwork rather than light falling on it.
#[test]
fn the_lit_edge_swaps_when_the_lamp_crosses_the_stage() {
    with_exporter(|exporter| {
        let mut left = slab();
        let id = left.add_light(LightKind::Lamp {
            position: Point::new(60.0, 200.0),
            height: 90.0,
            radius: 400.0,
        });
        left.lights_mut().get_mut(id).expect("the lamp").shadows = false;

        let mut right = left.clone();
        right.lights_mut().get_mut(id).expect("the lamp").kind = LightKind::Lamp {
            position: Point::new(490.0, 200.0),
            height: 90.0,
            radius: 400.0,
        };

        let from_left = render(exporter, &left);
        let from_right = render(exporter, &right);

        assert!(
            column(&from_left, 203) > column(&from_left, 347),
            "lit from the left, the left edge is the bright one"
        );
        assert!(
            column(&from_right, 347) > column(&from_right, 203),
            "lit from the right, they must swap"
        );
    });
}

/// **The edge a wall of dark arrives at goes dark.**
///
/// The other half of the report: a gloom washes the picture down evenly, so the
/// figures standing in it lose their form. The side of a figure the darkness
/// arrives from has to be darker than the middle of the same figure, exactly as
/// the lit side is brighter.
#[test]
fn the_edge_a_gloom_arrives_at_is_darker_than_the_middle() {
    with_exporter(|exporter| {
        let mut scene = slab();
        scene.add_light(LightKind::Lamp {
            position: Point::new(490.0, 120.0),
            height: 150.0,
            radius: 300.0,
        });
        // A wall standing off the left, throwing right across the slab.
        scene.add_light(LightKind::Gloom {
            edge: Point::new(-60.0, 0.0),
            facing: 0.0,
            throw: 700.0,
            width: 4000.0,
        });

        let frame = render(exporter, &scene);
        let near = column(&frame, 203);
        let middle = column(&frame, 275);

        assert!(
            near < middle - 4.0,
            "the edge the darkness arrives at must go dark: edge {near:.1}, \
             middle {middle:.1}"
        );
    });
}

/// **A fire moves.** Two frames of the same document, and the picture is not
/// the same picture — without a keyframe anywhere in the file.
#[test]
fn a_fire_lamp_changes_from_frame_to_frame() {
    with_exporter(|exporter| {
        let mut scene = slab();
        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(80.0, 200.0),
            height: 110.0,
            radius: 320.0,
        });
        scene.lights_mut().get_mut(id).expect("the lamp").make_fire();
        assert!(scene.lights().animates(), "a fire animates with no keys");

        let settings = ExportSettings::for_stage(&scene);
        let levels: Vec<f32> = (0..8)
            .map(|f| {
                let frame = exporter.render(&scene, f, &settings).expect("render");
                luma(frame.pixel(210, 200))
            })
            .collect();

        let low = levels.iter().copied().fold(f32::MAX, f32::min);
        let high = levels.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            high > low + 4.0,
            "the fire did not move across eight frames: {levels:?}"
        );
        // Distinct *levels*, not distinct consecutive pairs. The reading is
        // eight bits at one pixel, so two frames a hair apart round to the same
        // number without the fire having held still — asserting on every
        // neighbouring pair measures the readback's resolution rather than the
        // light.
        let mut seen: Vec<f32> = levels.clone();
        seen.sort_by(f32::total_cmp);
        seen.dedup();
        assert!(
            seen.len() >= 5,
            "eight frames of fire came out at only {} different levels: {levels:?}",
            seen.len()
        );
    });
}
