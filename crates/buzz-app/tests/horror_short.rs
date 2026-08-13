//! A five-minute rural horror short, built and measured end to end.
//!
//! # What this is for
//!
//! Every other test here proves one thing works. This one asks a different
//! question: **can somebody actually make a film with it?** A tool can pass
//! every unit test and still be unusable, because the failures that stop a
//! production are not wrong answers — they are things that take four minutes
//! when they should take four seconds, on a document the size a real film
//! reaches.
//!
//! So this builds what a rural horror short actually needs, at the scale it
//! actually reaches, and puts a stopwatch on every stage:
//!
//! * a villager rigged out of reusable part-symbols — head, eyes, mouth,
//!   hands, body — because that is how a character is animated frame by frame
//! * an armature with inverse kinematics on the arm, for reaching and pointing
//! * a night exterior: hut, trees, ground, with layer depth for parallax
//! * a moon and a lantern, casting shadows, which is the whole look of the
//!   genre
//! * fog and a lantern glow as filters
//! * a vignette as a mask, which is the other half of the look
//! * dialogue with automatic lip sync
//! * **7 200 frames** — five minutes at 24fps
//!
//! # The budgets
//!
//! Deliberately generous. These are not benchmarks; they are the line between
//! "slow" and "the application has hung", and a person cannot tell those apart.
//! A number here failing means an animator cannot work, not that the code
//! could be tidier.

use std::time::{Duration, Instant};

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Shape as _, Vec2};
use buzz_scene::{
    Filter, FilterKind, LayerKind, LightKind, Quality, Scene, ShapeData, SymbolKind,
};
use peniko::Color;

/// Five minutes at 24fps.
const FILM_FRAMES: u32 = 24 * 60 * 5;

/// The longest any single authoring action may take before it reads as a hang.
const ACTION_BUDGET: Duration = Duration::from_millis(500);

/// The longest building the whole film may take.
const BUILD_BUDGET: Duration = Duration::from_secs(30);

/// Night colours, because the look is the point.
const MOONLIGHT: Color = Color::from_rgb8(0x8C, 0xA8, 0xC8);
const HUT: Color = Color::from_rgb8(0x2A, 0x22, 0x1C);
const GROUND: Color = Color::from_rgb8(0x14, 0x18, 0x14);
const SKIN: Color = Color::from_rgb8(0x9A, 0x74, 0x52);
const CLOTH: Color = Color::from_rgb8(0x3A, 0x30, 0x44);

fn square(x: f64, y: f64, w: f64, h: f64) -> buzz_geom::BezPath {
    Rect::new(x, y, x + w, y + h).to_path(1e-9)
}

/// A blobby closed path — what drawn artwork is, rather than a rectangle.
///
/// The distinction matters everywhere geometry is processed: booleans, shading
/// crescents, cast shadows and filters all scale with segment count, and a
/// fixture of rectangles hides every cost that a real drawing pays.
fn blob(centre: Point, radius: f64, segments: usize, wobble: f64) -> buzz_geom::BezPath {
    let mut path = buzz_geom::BezPath::new();
    let at = |k: usize| {
        let a = std::f64::consts::TAU * k as f64 / segments as f64;
        let r = radius * (0.78 + 0.30 * (3.0 * a + wobble).sin());
        Point::new(centre.x + r * a.cos(), centre.y + r * a.sin())
    };
    path.move_to(at(0));
    for k in 1..=segments {
        path.line_to(at(k % segments));
    }
    path.close_path();
    path
}

/// Time one stage and report it, so a failure says which stage and by how much.
fn stage<T>(name: &str, budget: Duration, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = work();
    let taken = started.elapsed();
    eprintln!("  {name:<46} {taken:>10.2?}");
    assert!(
        taken < budget,
        "{name} took {taken:?}, over its {budget:?} budget — that is long \
         enough to stop an animator working"
    );
    out
}

/// A part of the character, as its own symbol, so it can be reused and swapped.
fn part_symbol(
    scene: &mut Scene,
    name: &str,
    path: buzz_geom::BezPath,
    colour: Color,
) -> buzz_scene::SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, Some("Villager"));
    let layer = scene
        .library()
        .get(id)
        .and_then(|s| s.layers.iter().next().map(|l| l.id))
        .expect("a new symbol has a layer");
    scene.enter_symbol(id);
    scene
        .add_shape(layer, ShapeData::filled(path, colour))
        .expect("the part's artwork");
    scene.exit_symbol();
    id
}

/// Build the villager: parts as symbols, assembled into one character symbol.
///
/// This is the reusable-asset shape of the job — the mouth is swapped by lip
/// sync, the eyes blink, the hands move, and none of that is possible if the
/// parts are loose artwork.
fn build_villager(scene: &mut Scene) -> buzz_scene::SymbolId {
    let head = part_symbol(scene, "Head", blob(Point::ZERO, 34.0, 48, 0.4), SKIN);
    let eye_l = part_symbol(scene, "Eye L", blob(Point::ZERO, 7.0, 20, 1.0), Color::WHITE);
    let eye_r = part_symbol(scene, "Eye R", blob(Point::ZERO, 7.0, 20, 2.0), Color::WHITE);
    let mouth = part_symbol(scene, "Mouth", blob(Point::ZERO, 10.0, 24, 0.2), Color::BLACK);
    let body = part_symbol(scene, "Body", blob(Point::ZERO, 52.0, 56, 1.7), CLOTH);
    let hand_l = part_symbol(scene, "Hand L", blob(Point::ZERO, 13.0, 28, 0.9), SKIN);
    let hand_r = part_symbol(scene, "Hand R", blob(Point::ZERO, 13.0, 28, 1.4), SKIN);

    let villager = scene.add_symbol("Villager", SymbolKind::MovieClip, Some("Villager"));
    scene.enter_symbol(villager);
    // One layer per part, as Animate rigs are built — so each can be keyed,
    // swapped and parented on its own.
    for (name, part, at) in [
        ("body", body, Vec2::new(0.0, 70.0)),
        ("hand L", hand_l, Vec2::new(-46.0, 92.0)),
        ("hand R", hand_r, Vec2::new(46.0, 92.0)),
        ("head", head, Vec2::new(0.0, 0.0)),
        ("eye L", eye_l, Vec2::new(-12.0, -6.0)),
        ("eye R", eye_r, Vec2::new(12.0, -6.0)),
        ("mouth", mouth, Vec2::new(0.0, 16.0)),
    ] {
        let layer = scene.add_layer(name, LayerKind::Normal);
        scene
            .add_instance_at(layer, 0, part, Affine::translate(at))
            .expect("the part is placed");
    }
    scene.exit_symbol();
    villager
}

/// The night exterior, on layers at different depths so the camera parallaxes.
fn build_set(scene: &mut Scene) {
    // Back to front, each further layer pushed away so a camera move separates
    // them — the cheapest thing that makes a flat drawing read as a place.
    for (name, depth, colour, count, radius, y) in [
        ("Sky", 2400.0, Color::from_rgb8(0x0B, 0x10, 0x1E), 1, 1400.0, 300.0),
        ("Far trees", 1200.0, Color::from_rgb8(0x10, 0x16, 0x18), 14, 90.0, 520.0),
        ("Hut", 400.0, HUT, 3, 160.0, 620.0),
        ("Near trees", -200.0, Color::from_rgb8(0x08, 0x0C, 0x0A), 6, 150.0, 700.0),
        ("Ground", 0.0, GROUND, 1, 1600.0, 980.0),
    ] {
        let layer = scene.add_layer(name, LayerKind::Normal);
        scene.update_layer(layer, |l| l.depth = depth);
        for i in 0..count {
            let x = if count == 1 {
                960.0
            } else {
                140.0 + (i as f64) * (1640.0 / count.max(1) as f64)
            };
            scene
                .add_shape(
                    layer,
                    ShapeData::filled(blob(Point::new(x, y), radius, 40, i as f64), colour),
                )
                .expect("set dressing");
        }
    }
}

/// The look: a low cold moon and a warm lantern, both casting.
fn light_the_night(scene: &mut Scene) {
    let moon = scene.add_light(LightKind::Sun {
        azimuth: 2.2,
        elevation: 0.35,
    });
    {
        let rig = scene.lights_mut();
        rig.enabled = true;
        rig.base = Color::from_rgb8(0x14, 0x1A, 0x24);
        let light = rig.get_mut(moon).expect("the moon");
        light.color = MOONLIGHT;
        light.intensity = 0.7;
        light.shadows = true;
        light.shadow_strength = 0.75;
        light.standing_height = 80.0;
    }

    let lantern = scene.add_light(LightKind::Lamp {
        position: Point::new(1180.0, 640.0),
        height: 90.0,
        radius: 420.0,
    });
    let light = scene
        .lights_mut()
        .get_mut(lantern)
        .expect("the lantern");
    light.color = Color::from_rgb8(0xFF, 0xB0, 0x50);
    light.intensity = 1.1;
    light.shadows = true;
}

/// Fog, a lantern glow, and a vignette — the rest of the genre's look.
fn add_effects(scene: &mut Scene) {
    // Fog: a blur on the far trees, so distance reads as softness.
    let far = scene
        .layers()
        .iter()
        .find(|l| l.name == "Far trees")
        .map(|l| l.id)
        .expect("the far trees");
    let ids: Vec<_> = scene
        .layers()
        .get(far)
        .map(|l| l.objects_at(0).iter().map(|o| o.id).collect())
        .unwrap_or_default();
    for id in ids {
        scene.update_object(id, |o| {
            o.filters.push(Filter::new(FilterKind::Blur {
                x: 9.0,
                y: 9.0,
                quality: Quality::Medium,
            }));
        });
    }

    // The lantern's halo.
    let glow_layer = scene.add_layer("Lantern", LayerKind::Normal);
    let halo = scene
        .add_shape(
            glow_layer,
            ShapeData::filled(blob(Point::new(1180.0, 640.0), 26.0, 28, 0.0), Color::from_rgb8(0xFF, 0xC8, 0x70)),
        )
        .expect("the lantern");
    scene.update_object(halo, |o| {
        o.filters.push(Filter::new(FilterKind::Glow {
            x: 40.0,
            y: 40.0,
            strength: 1.4,
            color: Color::from_rgb8(0xFF, 0xB0, 0x50),
            inner: false,
            knockout: false,
            quality: Quality::Medium,
        }));
    });

    // The vignette: darkness closing in at the edges. An inverse mask shows
    // the run of Masked layers below it *except* where the mask covers them —
    // so a black sheet, holed out in the middle, leaves darkness at the edges
    // and clear glass over the action.
    //
    // **Masking is positional**, as it is in Animate: the mask claims the
    // unbroken run of `Masked` layers under it. The mask goes on top and the
    // sheet below it is the layer that gets holed.
    let hole = scene.add_layer("Vignette hole", LayerKind::InverseMask);
    scene
        .add_shape(
            hole,
            ShapeData::filled(blob(Point::new(960.0, 540.0), 700.0, 48, 0.0), Color::WHITE),
        )
        .expect("the hole");
    let vignette = scene.add_layer("Vignette", LayerKind::Masked);
    scene
        .add_shape(
            vignette,
            ShapeData::filled(square(-200.0, -200.0, 2320.0, 1480.0), Color::from_rgba8(0, 0, 0, 210)),
        )
        .expect("the vignette");
}

/// The whole film, built once and measured stage by stage.
fn build_film() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(1920.0, 1080.0);
    scene.stage_mut().frame_rate = 24.0;
    scene.stage_mut().background = Color::from_rgb8(0x06, 0x08, 0x0E);

    stage("build the set", ACTION_BUDGET, || build_set(&mut scene));
    let villager = stage("rig the villager out of part symbols", ACTION_BUDGET, || {
        build_villager(&mut scene)
    });

    // The cast: the villager placed several times over the film, which is what
    // a five-minute short with a few shots actually holds.
    stage("place the cast", ACTION_BUDGET, || {
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..6 {
            scene
                .add_instance_at(
                    cast,
                    0,
                    villager,
                    Affine::translate(Vec2::new(300.0 + i as f64 * 240.0, 700.0)),
                )
                .expect("a villager");
        }
    });

    stage("light the night", ACTION_BUDGET, || light_the_night(&mut scene));
    stage("fog, glow and vignette", ACTION_BUDGET, || add_effects(&mut scene));

    // Five minutes of film. Keyframes every half second on the cast layer, as
    // an animator working on twos-and-fours would leave behind.
    // **The film has to be made five minutes long first.** A layer starts one
    // frame long, and `ensure_keyframe` deliberately refuses past the end of a
    // span — there would be no artwork to duplicate. Setting the length is what
    // an animator does with F5, and it is what makes the rest of this real:
    // without it every keyframe below is silently refused and the "film" is one
    // frame long, which would make every measurement here meaningless.
    stage("set the film to five minutes", BUILD_BUDGET, || {
        assert!(
            scene.set_frame_count(FILM_FRAMES),
            "the film should have been lengthened"
        );
    });

    stage("key five minutes of animation", BUILD_BUDGET, || {
        let cast = scene
            .layers()
            .iter()
            .find(|l| l.name == "Cast")
            .map(|l| l.id)
            .expect("the cast layer");
        let mut keyed = 0;
        for frame in (0..FILM_FRAMES).step_by(12) {
            if scene.ensure_keyframe(cast, frame) {
                keyed += 1;
            }
        }
        assert!(
            keyed > 500,
            "only {keyed} keyframes were accepted over five minutes — an              animator cannot key a film they cannot key"
        );
        eprintln!("  {:<46} {keyed:>10}", "(keyframes accepted)");
    });

    scene
}

/// **The whole thing, measured.** Every stage of making the film, with a
/// stopwatch, at the scale a real film reaches.
#[test]
fn a_five_minute_horror_short_can_be_built_and_worked_on() {
    eprintln!("\n--- building a five-minute rural horror short ---");
    let started = Instant::now();
    let scene = build_film();
    let build = started.elapsed();

    eprintln!("  {:<46} {:>10}", "frames", scene.frame_count());
    assert_eq!(
        scene.frame_count(),
        FILM_FRAMES,
        "the film is not five minutes long"
    );
    eprintln!("  {:<46} {:>10}", "layers", scene.layers().iter().count());
    eprintln!("  {:<46} {:>10}", "symbols", scene.library().len());
    eprintln!("  {:<46} {taken:>10.2?}", "TOTAL BUILD", taken = build);

    assert!(
        build < BUILD_BUDGET,
        "building the film took {build:?}, which is longer than anyone will sit through"
    );

    // Now work on it, which is what the tool is for.
    let mut editor = Editor::new(Document::new(scene));

    stage("scrub to the middle of the film", ACTION_BUDGET, || {
        editor.set_frame(FILM_FRAMES / 2);
    });
    stage("scrub to the end", ACTION_BUDGET, || {
        editor.set_frame(FILM_FRAMES - 1);
    });
    stage("scrub back to the start", ACTION_BUDGET, || {
        editor.set_frame(0);
    });

    stage("select everything on a layer", ACTION_BUDGET, || {
        let cast = editor
            .scene()
            .layers()
            .iter()
            .find(|l| l.name == "Cast")
            .map(|l| l.id)
            .expect("the cast layer");
        editor.select_layer(cast);
    });

    stage("click a villager on the stage", ACTION_BUDGET, || {
        let _ = editor.object_at(Point::new(300.0, 700.0), 3.0);
    });

    stage("go inside a villager", ACTION_BUDGET, || {
        editor.enter_or_leave_at(editor.camera.doc_to_screen(Point::new(300.0, 700.0)));
    });
}

/// **Drawing the film, which is where the cost actually is.**
///
/// Building a document is bookkeeping; painting it is booleans, blurs, shading
/// crescents and cast shadows over every visible shape. This measures the
/// first frame — everything cold — and then a run of frames as playback and
/// scrubbing would, which is what an animator lives in.
#[test]
fn the_film_draws_fast_enough_to_animate_against() {
    let Ok(mut exporter) = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)
    else {
        eprintln!("skipping: no usable GPU");
        return;
    };

    let scene = build_film();
    let settings = buzz_export::ExportSettings::for_stage(&scene);

    eprintln!("\n--- drawing the film ---");

    let started = Instant::now();
    let frame = exporter.render(&scene, 0, &settings).expect("the first frame");
    let cold = started.elapsed();

    // Written out so the film can be *looked at*. Numbers say it is fast;
    // only a picture says it is right, and a lit night scene is exactly the
    // kind of thing that can be fast and completely wrong.
    let out = std::env::temp_dir().join("buzzanimate-horror-frame.png");
    frame.write_png(&out).expect("the frame is written");
    eprintln!("  {:<46} {}", "wrote", out.display());
    eprintln!("  {:<46} {cold:>10.2?}", "first frame, everything cold");

    // A run of frames, as playback does. The exporter builds a fresh cache per
    // call, so this is the *worst* case — the window keeps one across frames.
    let frames = 24;
    let started = Instant::now();
    for f in 0..frames {
        exporter.render(&scene, f * 24, &settings).expect("a frame");
    }
    let run = started.elapsed();
    let each = run / frames;
    eprintln!("  {:<46} {each:>10.2?}", "per frame over a second of film");

    assert!(
        cold < Duration::from_secs(10),
        "the first lit frame took {cold:?} — an animator opening this film \
         would conclude it had hung"
    );
    assert!(
        each < Duration::from_secs(2),
        "each frame costs {each:?}; scrubbing and playback are unusable"
    );
}

/// **Rigging: an armature over the villager's arm, posed with IK.**
///
/// Head, hand and mouth movement is the whole of character animation, and the
/// rig is what makes it two keyframes instead of forty drawings. This checks
/// the rig can be built, posed and keyed at film scale without stalling.
#[test]
fn an_arm_can_be_rigged_posed_and_keyed() {
    use buzz_rig::{Armature, Bone};

    let mut scene = build_film();
    let cast = scene
        .layers()
        .iter()
        .find(|l| l.name == "Cast")
        .map(|l| l.id)
        .expect("the cast layer");

    eprintln!("\n--- rigging ---");

    // A three-bone arm: shoulder, elbow, wrist.
    let armature = stage("build a three-bone arm", ACTION_BUDGET, || {
        let mut a = Armature::new(Point::new(300.0, 640.0));
        let shoulder = a.push(Bone::new("shoulder", None, 70.0, 0.4));
        let elbow = a.push(Bone::new("elbow", Some(shoulder), 64.0, 0.5));
        a.push(Bone::new("wrist", Some(elbow), 30.0, 0.2));
        a
    });

    let rig = stage("bind artwork to the arm", ACTION_BUDGET, || {
        let mut binding = buzz_scene::ArmatureData::new(armature.clone());
        binding.bind_shape(std::sync::Arc::new(buzz_scene::Object::shape(
            buzz_scene::ObjectId(999_001),
            ShapeData::filled(blob(Point::new(340.0, 660.0), 46.0, 40, 0.3), SKIN),
        )));
        binding
    });

    let arm = stage("place the rig on the stage", ACTION_BUDGET, || {
        let mut object = buzz_scene::Object::shape(
            buzz_scene::ObjectId(999_002),
            ShapeData::filled(square(0.0, 0.0, 1.0, 1.0), SKIN),
        );
        object.kind = buzz_scene::ObjectKind::Armature(rig.clone());
        scene
            .add_object_at(cast, 0, object)
            .expect("the rig is placed")
    });

    // Pose it by reaching, which is what IK is for — and key the pose, twice,
    // so it tweens.
    stage("solve IK and key two poses", ACTION_BUDGET, || {
        for (frame, target) in [(0u32, Point::new(420.0, 560.0)), (48, Point::new(240.0, 720.0))] {
            scene.ensure_keyframe(cast, frame);
            scene.update_object_at(frame, arm, |o| {
                if let buzz_scene::ObjectKind::Armature(rig) = &mut o.kind {
                    let tip = rig.armature.len().saturating_sub(1);
                    buzz_rig::ik::solve_to(
                        &mut rig.armature,
                        tip,
                        target,
                        &buzz_rig::ik::IkOptions::default(),
                    );
                    rig.rebind();
                }
            });
        }
    });

    stage("resolve the tween halfway between the poses", ACTION_BUDGET, || {
        let _ = scene
            .layers()
            .get(cast)
            .map(|l| l.objects_at(24).len())
            .unwrap_or(0);
    });
}

/// Where everything actually is, printed. A picture that is wrong says so;
/// this says *why*.
#[test]
fn report_where_the_artwork_is() {
    let scene = build_film();
    eprintln!("\n--- the film's contents ---");
    eprintln!("  stage {:?}", scene.stage().size);
    for layer in scene.layers().iter() {
        let objects = layer.objects_at(0);
        let bounds = objects
            .iter()
            .map(|o| scene.resolved_bounds(o))
            .reduce(|a, b| a.union(b));
        eprintln!(
            "  {:<16} depth {:>8.0}  {:>2} objects  {}",
            layer.name,
            layer.depth,
            objects.len(),
            match bounds {
                Some(b) => format!(
                    "{:.0},{:.0} .. {:.0},{:.0}",
                    b.x0, b.y0, b.x1, b.y1
                ),
                None => "empty".to_string(),
            }
        );
    }
    eprintln!("  camera focal {:?}", scene.camera().focal_distance);
}

/// Render the film with pieces switched off, to find which one is wrong.
#[test]
fn bisect_the_render() {
    let Ok(mut exporter) = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)
    else {
        return;
    };
    let dir = std::env::temp_dir();

    /// One variant: a name and the thing switched off.
    type Variant = (&'static str, Box<dyn Fn(&mut Scene)>);
    let variants: Vec<Variant> = vec![
        ("00-full", Box::new(|_: &mut Scene| {})),
        ("01-nolight", Box::new(|s: &mut Scene| { s.lights_mut().enabled = false; })),
        ("02-noeffects", Box::new(|s: &mut Scene| {
            for name in ["Vignette", "Vignette hole", "Lantern"] {
                let found = s.layers().iter().find(|l| l.name == name).map(|l| l.id);
                if let Some(id) = found {
                    s.remove_layer(id);
                }
            }
        })),
        ("03-nodepth", Box::new(|s: &mut Scene| {
            let ids: Vec<_> = s.layers().iter().map(|l| l.id).collect();
            for id in ids { s.update_layer(id, |l| l.depth = 0.0); }
        })),
        ("04-nocast", Box::new(|s: &mut Scene| {
            let found = s.layers().iter().find(|l| l.name == "Cast").map(|l| l.id);
            if let Some(id) = found {
                s.remove_layer(id);
            }
        })),
    ];

    for (name, tweak) in variants {
        let mut scene = build_film();
        tweak(&mut scene);
        let settings = buzz_export::ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");
        let path = dir.join(format!("buzz-horror-{name}.png"));
        frame.write_png(&path).expect("write");
        // A crude signature: how much of the frame is not background.
        let lit = frame.pixels.chunks_exact(4).filter(|p| p[0] > 40 || p[1] > 40 || p[2] > 40).count();
        eprintln!("  {name:<14} {:>8} bright px  {}", lit, path.display());
    }
}

/// **Dialogue: a five-minute take, analysed and lip-synced.**
///
/// A horror short is carried by its narration, and lip sync is the step that
/// turns five minutes of audio into a few thousand mouth keyframes. It runs on
/// the whole take at once, so it is the single most likely place for the tool
/// to sit and think for minutes with nothing on screen.
#[test]
fn five_minutes_of_dialogue_can_be_analysed_and_lip_synced() {
    use buzz_audio::Clip;

    eprintln!("\n--- dialogue ---");

    // Five minutes of speech-like audio: bursts of noise with gaps, which is
    // what the analyser actually has to segment. Silence would be free and
    // would prove nothing.
    let rate = 44_100;
    let clip = stage("synthesise a five-minute take", Duration::from_secs(10), || {
        let samples: Vec<f32> = (0..rate * 60 * 5)
            .map(|i| {
                let t = i as f64 / rate as f64;
                // A word every half second, with a pause between.
                let speaking = (t * 2.0).fract() < 0.6;
                if !speaking {
                    return 0.0;
                }
                let tone = (t * 220.0 * std::f64::consts::TAU).sin();
                let formant = (t * 900.0 * std::f64::consts::TAU).sin() * 0.4;
                ((tone + formant) * 0.4) as f32
            })
            .collect();
        Clip::new("Narration", rate as u32, 1, samples).expect("a clip")
    });

    let mut scene = build_film();

    let track = stage("analyse it for visemes", Duration::from_secs(20), || {
        buzz_audio::analysis::analyse_visemes(
            &clip,
            24.0,
            &buzz_audio::analysis::LipSyncOptions::default(),
        )
    });
    eprintln!("  {:<46} {:>10}", "(viseme frames)", track.len());
    assert!(
        track.len() > 6000,
        "a five-minute take should analyse to about 7 200 frames, got {}",
        track.len()
    );

    // A mouth symbol with one frame per shape, and a layer to put it on.
    let mouth = buzz_app::lipsync::placeholder_mouth(&mut scene, "Mouth shapes");
    let layer = scene.add_layer("Lip sync", LayerKind::Normal);

    stage("write the mouth keyframes", Duration::from_secs(20), || {
        buzz_app::lipsync::write_track(
            &mut scene,
            &track,
            0,
            layer,
            mouth,
            Affine::translate(Vec2::new(300.0, 716.0)),
        )
    });

    let keys = scene
        .layers()
        .get(layer)
        .map(|l| l.frames.keyframes().len())
        .unwrap_or(0);
    eprintln!("  {:<46} {keys:>10}", "(mouth keyframes written)");
    assert!(keys > 100, "only {keys} mouth keyframes over five minutes");

    // And the document still works afterwards: scrubbing through a film with
    // thousands of keyframes on a layer is the thing that has to stay quick.
    let mut editor = Editor::new(Document::new(scene));
    stage("scrub through the lip-synced film", ACTION_BUDGET, || {
        for f in (0..FILM_FRAMES).step_by(97) {
            editor.set_frame(f);
        }
    });
}

/// **A working session on the finished film.**
///
/// This is what an animator actually does: scrub to a frame, pick a part, drag
/// it, key it, undo, draw a correction, repeat — on a document that is already
/// lit, filtered, masked and five minutes long. Each of those must stay
/// instant, and the one that matters most is *dragging with the lights on*,
/// because that is where the tool used to rebuild every crescent and shadow in
/// the film once per mouse move.
///
/// The window keeps one `DrawCache` across frames, so this drives that rather
/// than the exporter's fresh-cache-per-call path.
#[test]
fn an_animator_can_work_on_the_finished_film() {
    let scene = build_film();
    let mut editor = Editor::new(Document::new(scene));

    let mut vello = buzz_render::vello::Scene::new();
    let mut cache = buzz_render::document::DrawCache::new();

    // Draw one frame the way the window does, and report how long it took.
    let mut draw = |editor: &Editor,
                    vello: &mut buzz_render::vello::Scene,
                    cache: &mut buzz_render::document::DrawCache| {
        let mut builder = buzz_render::SceneBuilder::new(vello, &editor.camera);
        let scene = editor.scene();
        let options = buzz_render::document::FrameOptions {
            masks: buzz_render::document::MaskDisplay::WhenLocked,
            lit: true,
            place: scene.edit_place(),
            ..Default::default()
        };
        let started = Instant::now();
        buzz_render::document::draw_frame_cached(
            &mut builder,
            scene,
            editor.current_frame,
            scene.camera_transform(editor.current_frame),
            &options,
            cache,
        );
        started.elapsed()
    };

    eprintln!("\n--- a working session ---");

    let cold = draw(&editor, &mut vello, &mut cache);
    eprintln!("  {:<46} {cold:>10.2?}", "first frame");

    // Pick a villager and drag it thirty times, as one gesture does.
    let id = editor
        .object_at(Point::new(300.0, 700.0), 4.0)
        .expect("a villager under the pointer");
    editor.selection.select_one(id);

    let mut worst = Duration::ZERO;
    let started = Instant::now();
    for step in 0..30 {
        editor.doc.edit("Move", |scene| {
            scene.update_object(id, |o| {
                o.transform = Affine::translate(Vec2::new(300.0 + step as f64, 700.0));
            });
        });
        worst = worst.max(draw(&editor, &mut vello, &mut cache));
    }
    let drag = started.elapsed();
    eprintln!("  {:<46} {drag:>10.2?}", "a 30-step drag, lit, redrawn each step");
    eprintln!("  {:<46} {worst:>10.2?}", "worst single frame during the drag");

    assert!(
        worst < Duration::from_millis(120),
        "a frame during a drag took {worst:?} — at that rate posing lit \
         artwork is impossible, which is the one thing lighting is for"
    );

    // Scrubbing across the film, redrawing each time.
    let started = Instant::now();
    let mut worst_scrub = Duration::ZERO;
    for f in (0..FILM_FRAMES).step_by(FILM_FRAMES as usize / 24) {
        editor.set_frame(f);
        worst_scrub = worst_scrub.max(draw(&editor, &mut vello, &mut cache));
    }
    eprintln!("  {:<46} {:>10.2?}", "scrub across the film, redrawn", started.elapsed());
    eprintln!("  {:<46} {worst_scrub:>10.2?}", "worst single frame while scrubbing");
    assert!(
        worst_scrub < Duration::from_millis(250),
        "scrubbing costs {worst_scrub:?} a frame; the playhead will not keep up"
    );

    // Undo the whole drag.
    let started = Instant::now();
    for _ in 0..30 {
        editor.doc.undo();
    }
    eprintln!("  {:<46} {:>10.2?}", "undo the whole drag", started.elapsed());
}

/// **Getting the film out.** Ten seconds of it, encoded, and extrapolated.
///
/// A five-minute export is minutes of work by definition; what matters is that
/// it is *linear* and that it uses the machine. Encoding a chunk and scaling up
/// says whether the full film is half an hour or half a day.
#[test]
fn the_film_exports_at_a_usable_rate() {
    if !buzz_export::ffmpeg_available() {
        eprintln!("skipping export test: no ffmpeg");
        return;
    }
    let scene = build_film();
    let settings = buzz_export::ExportSettings::for_stage(&scene);
    let dir = std::env::temp_dir();
    let path = dir.join("buzz-horror-clip.mp4");

    eprintln!("\n--- export ---");
    let chunk = 240; // ten seconds
    let started = Instant::now();
    let report = buzz_export::export_video(
        &scene,
        0..chunk,
        &path,
        &settings,
        &buzz_export::VideoSettings::default(),
        &buzz_render::GpuPreference::Automatic,
        &[],
        |_, _| true,
    );
    let taken = started.elapsed();

    match report {
        Ok(report) => {
            let per_frame = taken / chunk;
            let whole = per_frame * FILM_FRAMES;
            eprintln!("  {:<46} {taken:>10.2?}", "ten seconds of film, encoded");
            eprintln!("  {:<46} {per_frame:>10.2?}", "per frame");
            eprintln!("  {:<46} {whole:>10.1?}", "the whole five minutes would take");
            eprintln!("  {:<46} {:>10}", "encoder", report.encoder);
            assert_eq!(report.frames, chunk);
            assert!(
                whole < Duration::from_secs(20 * 60),
                "the whole film would take {whole:?} to export"
            );
        }
        Err(e) => {
            let message = format!("{e:#}");
            if message.contains("GPU") || message.contains("adapter") || message.contains("device")
            {
                eprintln!("skipping: {message}");
            } else {
                panic!("export failed: {message}");
            }
        }
    }
}
