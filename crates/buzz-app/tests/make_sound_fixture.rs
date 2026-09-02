//! Write a document with dialogue, a nested character and a mask, for looking
//! at by hand.
//!
//! ```sh
//! cargo test -p buzz-app --test make_sound_fixture -- --ignored --nocapture
//! ```
//!
//! Ignored by default because it writes files and exists to be *looked at*:
//! the waveform in the timeline, the mask clipping on the stage, and lip sync
//! run from inside the head symbol are all things only a picture can settle.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_scene::{
    LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData, SoundRef, SymbolInstance, SymbolKind,
};
use peniko::Color;

const SKIN: Color = Color::from_rgb8(0xF2, 0xC2, 0x9B);
const HAIR: Color = Color::from_rgb8(0x5A, 0x3C, 0x28);
const SHIRT: Color = Color::from_rgb8(0x3F, 0x7C, 0xC4);
const SKY: Color = Color::from_rgb8(0x9A, 0xD1, 0xF5);

/// Speech-shaped audio: syllables, pauses and two fricatives.
fn dialogue_wav() -> Vec<u8> {
    let rate = 44_100u32;
    let mut samples: Vec<f32> = Vec::new();

    fn tone(samples: &mut Vec<f32>, rate: u32, seconds: f64, hz: f64, amplitude: f32) {
        let count = (seconds * rate as f64) as usize;
        for i in 0..count {
            let t = i as f64 / rate as f64;
            let v = (t * hz * std::f64::consts::TAU).sin()
                + 0.5 * (t * hz * 2.0 * std::f64::consts::TAU).sin();
            // Fade the ends so each syllable does not click.
            let edge = (i.min(count - i) as f32 / (0.01 * rate as f32)).min(1.0);
            samples.push((v as f32 / 1.5) * amplitude * edge);
        }
    }
    fn hiss(samples: &mut Vec<f32>, rate: u32, seconds: f64, amplitude: f32) {
        let count = (seconds * rate as f64) as usize;
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut previous = 0.0f32;
        for _ in 0..count {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let white = (state >> 40) as f32 / 8_388_608.0 - 1.0;
            let value = white - previous;
            previous = white;
            samples.push(value * amplitude);
        }
    }
    fn quiet(samples: &mut Vec<f32>, rate: u32, seconds: f64) {
        samples.extend(std::iter::repeat_n(0.0, (seconds * rate as f64) as usize));
    }

    quiet(&mut samples, rate, 0.15);
    tone(&mut samples, rate, 0.18, 520.0, 0.75);
    tone(&mut samples, rate, 0.22, 300.0, 0.80);
    quiet(&mut samples, rate, 0.22);
    tone(&mut samples, rate, 0.12, 900.0, 0.70);
    hiss(&mut samples, rate, 0.10, 0.50);
    quiet(&mut samples, rate, 0.10);
    tone(&mut samples, rate, 0.14, 420.0, 0.80);
    hiss(&mut samples, rate, 0.08, 0.45);
    tone(&mut samples, rate, 0.20, 1_500.0, 0.75);
    quiet(&mut samples, rate, 0.30);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
        for s in &samples {
            writer
                .write_sample((s.clamp(-1.0, 1.0) * 30_000.0) as i16)
                .expect("sample");
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

fn shape(id: u64, path: buzz_geom::BezPath, colour: Color) -> Arc<Object> {
    Arc::new(Object::shape(ObjectId(id), ShapeData::filled(path, colour)))
}

/// A character: a head symbol with a face and an empty mouth layer, inside a
/// body symbol — the arrangement lip sync is for.
fn build_character(scene: &mut Scene) -> (buzz_scene::SymbolId, buzz_scene::SymbolId) {
    let head = scene.add_symbol("Head", SymbolKind::MovieClip, None);
    let face_layer = scene
        .library()
        .get(head)
        .expect("head")
        .layers
        .iter()
        .next()
        .expect("a layer")
        .id;

    scene.library_mut().update(head, |symbol| {
        symbol.layers.update(face_layer, |layer| {
            layer.name = "Face".into();
            layer.frames.set_objects(
                0,
                vec![
                    shape(
                        2001,
                        kurbo::Ellipse::new(Point::ZERO, (55.0, 62.0), 0.0).to_path(0.05),
                        SKIN,
                    ),
                    shape(
                        2002,
                        kurbo::Ellipse::new(Point::new(0.0, -48.0), (56.0, 30.0), 0.0)
                            .to_path(0.05),
                        HAIR,
                    ),
                    shape(
                        2003,
                        kurbo::Ellipse::new(Point::new(-20.0, -8.0), (7.0, 9.0), 0.0).to_path(0.05),
                        Color::from_rgb8(0x22, 0x22, 0x22),
                    ),
                    shape(
                        2004,
                        kurbo::Ellipse::new(Point::new(20.0, -8.0), (7.0, 9.0), 0.0).to_path(0.05),
                        Color::from_rgb8(0x22, 0x22, 0x22),
                    ),
                ],
            );
        });
        // The mouth goes on its own layer, above the face, and starts empty:
        // lip sync fills it.
        symbol.layers.push_front(buzz_scene::Layer::normal(
            buzz_scene::LayerId(2100),
            "Mouth",
        ));
    });

    let body = scene.add_symbol("Character", SymbolKind::MovieClip, None);
    let body_layer = scene
        .library()
        .get(body)
        .expect("body")
        .layers
        .iter()
        .next()
        .expect("a layer")
        .id;
    scene.library_mut().update(body, |symbol| {
        symbol.layers.update(body_layer, |layer| {
            layer.name = "Body".into();
            layer.frames.set_objects(
                0,
                vec![
                    shape(
                        2201,
                        Rect::new(-45.0, 60.0, 45.0, 190.0).to_path(1e-9),
                        SHIRT,
                    ),
                    Arc::new(Object {
                        id: ObjectId(2202),
                        name: Some("Head".into()),
                        transform: Affine::IDENTITY,
                        kind: ObjectKind::Instance(SymbolInstance::new(head)),
                        locked: false,
                        visible: true,
                        filters: Vec::new(),
                        blend: Default::default(),
                        spatial: Default::default(),
                        pivot: None,
                        modifiers: Vec::new(),
                        text: None,
                    }),
                ],
            );
        });
    });

    (body, head)
}

#[test]
#[ignore = "writes a fixture to look at by hand"]
fn write_sound_fixture() {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;

    // -- a mask, so the effect can be seen on the stage --------------------
    //
    // Animate only shows a mask once its layer is *locked*, so the fixture
    // locks it: an unlocked mask would draw as ordinary artwork and the
    // fixture would prove nothing.
    let masked = scene.add_layer("Sky (masked)", LayerKind::Masked);
    scene.add_shape(
        masked,
        ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), SKY),
    );
    let mask = scene.add_layer("Porthole (mask)", LayerKind::Mask);
    scene.add_shape(
        mask,
        ShapeData::filled(
            kurbo::Ellipse::new(Point::new(430.0, 90.0), (70.0, 70.0), 0.0).to_path(0.05),
            Color::BLACK,
        ),
    );
    scene.update_layer(mask, |l| l.locked = true);
    scene.update_layer(masked, |l| l.locked = true);

    // -- the character on the root -----------------------------------------
    //
    // The character goes on the document's *original* layer, found by name:
    // `add_layer` inserts at the front, so "the first layer" is whichever was
    // added last — which put the character inside the mask and made it
    // invisible the first time this fixture was written.
    let (body, _head) = build_character(&mut scene);
    let stage_layer = scene
        .layers()
        .iter()
        .find(|l| l.name == "Layer_1")
        .expect("the document's first layer")
        .id;
    scene.update_layer(stage_layer, |l| l.name = "Character".into());
    scene.add_instance_at(stage_layer, 0, body, Affine::translate((190.0, 120.0)));

    // -- the dialogue, on the root -----------------------------------------
    let wav = dialogue_wav();
    let clip = buzz_audio::Clip::decode(&wav, "Dialogue").expect("decode");
    let sound = scene.add_sound(
        "Dialogue",
        Arc::new(wav),
        "wav",
        clip.sample_rate,
        clip.channels,
        clip.len() as u64,
    );

    let audio_layer = scene.add_layer("Audio", LayerKind::Normal);
    // The layer has to be long enough to show the whole take.
    let frames = clip.duration_frames(scene.stage().frame_rate);
    scene.update_layer(audio_layer, |l| {
        for _ in 0..frames {
            l.frames.insert_frame(l.frames.length());
        }
    });
    assert!(
        scene.set_frame_sound(audio_layer, 0, Some(SoundRef::stream(sound))),
        "the dialogue should attach to frame 0"
    );

    // Every other layer runs as long, so the timeline reads as one scene.
    let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in ids {
        scene.update_layer(id, |l| {
            while l.frames.length() < frames {
                l.frames.insert_frame(l.frames.length());
            }
        });
    }

    let path = std::env::temp_dir().join("sound-fixture.buzz");
    let mut document = buzz_doc::Document::new(scene.clone());
    document.save_as(&path).expect("save");

    println!("wrote {}", path.display());
    println!(
        "  {:.2}s of dialogue over {} frames at {} fps",
        clip.duration_seconds(),
        frames,
        scene.stage().frame_rate
    );
    println!("  open it, then: Commands > Lip Sync, inside Character > Head, layer 'Mouth'");
}
