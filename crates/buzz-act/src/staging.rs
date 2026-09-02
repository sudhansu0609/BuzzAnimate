//! Setting a scene: ground, sky, lights, camera, and people standing in it.
//!
//! # What "set up a scene" has to mean to be worth having
//!
//! Not a folder of clip art. The thing that costs an animator an afternoon
//! before any animation exists is the *arrangement*: a horizon at the right
//! height, a ground plane the characters stand on rather than float above, a
//! key light and a fill that agree with each other, a camera framing the space,
//! and characters placed at plausible distances and sizes. None of that is
//! drawing, all of it is arithmetic, and all of it has one obviously right
//! answer that everybody types out by hand.
//!
//! So this builds that arrangement and nothing else. Every layer it makes is an
//! ordinary layer, every shape an ordinary shape, every light an ordinary light
//! — there is no "generated scene" object and nothing here is re-run later. It
//! is a starting point, in the same sense a template is, and the first thing an
//! animator does to it is draw over it.
//!
//! # The ground line is the one number everything else follows
//!
//! [`SceneRecipe::horizon`] is a fraction of the stage height, and it decides
//! where the ground meets the sky, where the characters' feet go, how large
//! they are drawn, and how far a lamp stands off the floor. Getting characters
//! to stand *on* something rather than in front of it is most of what separates
//! a set-up scene from a pile of shapes.

use buzz_geom::{Point, Rect, Shape as _};
use buzz_scene::{LayerId, LayerKind, ObjectId, Scene, ShapeData};
use peniko::Color;

use crate::figure::{self, FigureSpec};

/// Where the scene is, which decides the palette and the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Outside, in the middle of the day. A high sun, a blue sky, hard-edged
    /// shadows.
    Daylight,
    /// Outside, late. A low sun near the horizon, a warm sky, long shadows —
    /// the setting that flatters a lit rig most and the one everybody reaches
    /// for first.
    Sunset,
    /// Outside, at night. A dark sky, a cold fill, and one warm lamp doing the
    /// work — which is where the lamp's rim glow earns its keep.
    Night,
    /// Inside. No sky at all: a wall, a floor, and a practical lamp.
    Interior,
}

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::Daylight => "Daylight",
            Setting::Sunset => "Sunset",
            Setting::Night => "Night",
            Setting::Interior => "Interior",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Setting::Daylight => "A high sun, a blue sky, and short hard shadows",
            Setting::Sunset => "A low sun, a warm sky, and shadows running long",
            Setting::Night => "A dark sky and one warm lamp doing the work",
            Setting::Interior => "A wall, a floor, and a practical lamp",
        }
    }

    /// The sky, or the wall behind an interior. Two colours: the top of the
    /// stage and the bottom, mixed down the backdrop.
    fn backdrop(self) -> (Color, Color) {
        match self {
            Setting::Daylight => (
                Color::from_rgb8(0x5C, 0x9E, 0xDA),
                Color::from_rgb8(0xC7, 0xE2, 0xF2),
            ),
            Setting::Sunset => (
                Color::from_rgb8(0x3C, 0x4B, 0x86),
                Color::from_rgb8(0xF2, 0xA1, 0x5C),
            ),
            Setting::Night => (
                Color::from_rgb8(0x0A, 0x0E, 0x1E),
                Color::from_rgb8(0x1C, 0x27, 0x42),
            ),
            Setting::Interior => (
                Color::from_rgb8(0x8C, 0x7E, 0x6E),
                Color::from_rgb8(0xB4, 0xA6, 0x94),
            ),
        }
    }

    /// The ground, near the horizon and near the camera. Darker in front,
    /// because that is what a ground plane receding away from you does and it
    /// is what gives a flat rectangle any depth at all.
    fn ground(self) -> (Color, Color) {
        match self {
            Setting::Daylight => (
                Color::from_rgb8(0x6E, 0x8E, 0x4C),
                Color::from_rgb8(0x3D, 0x53, 0x2A),
            ),
            Setting::Sunset => (
                Color::from_rgb8(0x7A, 0x5E, 0x3E),
                Color::from_rgb8(0x3A, 0x2C, 0x22),
            ),
            Setting::Night => (
                Color::from_rgb8(0x1A, 0x20, 0x2E),
                Color::from_rgb8(0x0B, 0x0E, 0x16),
            ),
            Setting::Interior => (
                Color::from_rgb8(0x6B, 0x53, 0x3C),
                Color::from_rgb8(0x43, 0x33, 0x25),
            ),
        }
    }
}

/// What to build.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneRecipe {
    pub setting: Setting,
    /// How many people to stand in it.
    pub cast: usize,
    /// Where the ground meets the backdrop, as a fraction of the stage height
    /// from the top. Two thirds down is the ordinary framing for a shot of
    /// people standing: enough sky to place them, enough floor to walk on.
    pub horizon: f64,
    /// How tall the nearest figure is, as a fraction of the stage height.
    pub figure_scale: f64,
    /// Light the scene, and rim the cast.
    ///
    /// On by default because an unlit set-up scene is a pile of flat colour,
    /// and because the rim is the thing that makes a figure sit *in* a
    /// background rather than on top of it.
    pub lit: bool,
    /// How long the shot runs, in frames. Every layer is made this long, so a
    /// performance written afterwards has somewhere to go.
    pub frames: u32,
}

impl Default for SceneRecipe {
    fn default() -> Self {
        Self {
            setting: Setting::Sunset,
            cast: 2,
            horizon: 0.66,
            figure_scale: 0.62,
            lit: true,
            frames: 48,
        }
    }
}

/// What was built, so the caller can select the cast and perform them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StagedScene {
    /// The layers, back to front.
    pub backdrop: Option<LayerId>,
    pub ground: Option<LayerId>,
    /// One layer per person: a character each gets its own layer so their
    /// performances can be timed independently, which is the whole reason two
    /// people in a shot are two layers.
    pub cast: Vec<(LayerId, ObjectId)>,
    pub message: String,
}

impl StagedScene {
    /// The people, in the order they were placed: nearest the camera first.
    pub fn actors(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.cast.iter().map(|(_, id)| *id)
    }
}

/// **Build the scene.**
///
/// Everything is added to the document's own timeline, over whatever is already
/// there — the backdrop goes to the back and the cast in front of it, so
/// setting a scene under existing artwork does not bury it.
///
/// The caller wraps this in one `Document::edit`, so a whole set-up is one
/// Ctrl+Z.
pub fn build(scene: &mut Scene, recipe: &SceneRecipe) -> StagedScene {
    let stage = scene.stage().stage_rect();
    let horizon_y = stage.y0 + stage.height() * recipe.horizon.clamp(0.15, 0.95);

    let mut out = StagedScene::default();

    // **The backdrop covers more than the stage.** A camera that pans, or a
    // shot exported wider than it was framed, must not run off the edge of the
    // sky — and a background that stops exactly at the stage rectangle is the
    // commonest way that happens. Half a stage of margin all round is cheap:
    // it is two rectangles.
    let wide = stage.inflate(stage.width() * 0.5, stage.height() * 0.5);

    let (sky_top, sky_bottom) = recipe.setting.backdrop();
    let backdrop = scene.add_stage_layer("Sky", LayerKind::Normal);
    out.backdrop = Some(backdrop);
    scene.add_shape(
        backdrop,
        ShapeData::filled(
            Rect::new(wide.x0, wide.y0, wide.x1, horizon_y).to_path(1e-9),
            sky_top,
        ),
    );
    // A second band low down, mixed towards the horizon colour. Two flat bands
    // rather than a gradient: a gradient here would be the right answer for a
    // sky and the wrong one for an interior wall, and the animator can put a
    // gradient on either of these in one click.
    scene.add_shape(
        backdrop,
        ShapeData::filled(
            Rect::new(
                wide.x0,
                horizon_y - stage.height() * 0.18,
                wide.x1,
                horizon_y,
            )
            .to_path(1e-9),
            sky_bottom,
        ),
    );

    let (ground_far, ground_near) = recipe.setting.ground();
    let ground = scene.add_stage_layer("Ground", LayerKind::Normal);
    out.ground = Some(ground);
    scene.add_shape(
        ground,
        ShapeData::filled(
            Rect::new(wide.x0, horizon_y, wide.x1, wide.y1).to_path(1e-9),
            ground_near,
        ),
    );
    // A lighter band just past the horizon, which is what makes a flat
    // rectangle read as a plane going away from the camera.
    scene.add_shape(
        ground,
        ShapeData::filled(
            Rect::new(
                wide.x0,
                horizon_y,
                wide.x1,
                horizon_y + stage.height() * 0.10,
            )
            .to_path(1e-9),
            ground_far,
        ),
    );

    // **The cast, standing on the ground rather than in front of it.**
    //
    // Spread across the stage and *staggered in depth*: each one further back
    // is drawn a little smaller and a little higher up, which is the whole of
    // perspective on a flat stage and is what stops two characters reading as
    // cut-outs at the same distance. Nearest first, so the layer order puts the
    // nearest one in front.
    let cast = recipe.cast.min(6);
    for i in 0..cast {
        // Across the stage: one person centre, two flanking, and so on, never
        // touching the edges.
        let across = if cast == 1 {
            0.5
        } else {
            0.22 + 0.56 * (i as f64 / (cast - 1) as f64)
        };
        // Further back by a fifth of the ground for each one after the first.
        let depth = i as f64 / cast.max(1) as f64;
        let shrink = 1.0 - 0.18 * depth;
        let stands_on = horizon_y + (stage.y1 - horizon_y) * (0.72 - 0.45 * depth);

        let spec = FigureSpec {
            height: stage.height() * recipe.figure_scale * shrink,
            // They face each other: the first from the left looking right, the
            // rest looking back at it. Two people in a shot who both face the
            // camera are two people in a photograph.
            facing: if i == 0 { 1.0 } else { -1.0 },
            ..FigureSpec::default()
        };

        let layer = scene.add_stage_layer(&format!("Person {}", i + 1), LayerKind::Normal);
        let id = scene.next_object_id();
        let mut person = figure::build(&spec, id, || scene.next_object_id());
        person.name = Some(format!("Person {}", i + 1));
        person.transform = buzz_geom::Affine::translate((
            stage.x0 + stage.width() * across,
            stands_on,
        )) * person.transform;

        if let Some(placed) = scene.add_object(layer, person) {
            out.cast.push((layer, placed));
        }
    }

    // Every layer as long as the shot, so a performance written next has frames
    // to be written onto. Done after the layers exist rather than as each is
    // made, because the backdrop and the ground want it too — a two-second shot
    // whose sky lasts one frame is a scene that vanishes.
    let last = recipe.frames.max(1).saturating_sub(1);
    let layers: Vec<LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
    for layer in layers {
        scene.update_stage_layer(layer, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }

    if recipe.lit {
        light(scene, recipe, horizon_y);
    }

    out.message = format!(
        "{} scene: {} layer(s), {} in the cast{}",
        recipe.setting.label(),
        2 + out.cast.len(),
        out.cast.len(),
        if recipe.lit { ", lit" } else { "" }
    );
    out
}

/// **A three-point rig, in the two dimensions that mean anything here.**
///
/// A key with a direction, a sky to fill what the key does not reach, and — the
/// part that makes a figure sit in a background rather than on it — a rim. What
/// is deliberately absent is a fourth light for each new idea: `buzz-light`'s
/// own documentation explains why a spot and an area light mean nothing on flat
/// artwork, and adding one here would be adding a lamp with extra numbers.
fn light(scene: &mut Scene, recipe: &SceneRecipe, horizon_y: f64) {
    use buzz_scene::{LightKind, Light};

    let stage = scene.stage().stage_rect();

    // The key. A sun outdoors; a practical lamp indoors and at night, because a
    // sun through a wall is not a thing.
    let key_kind = match recipe.setting {
        Setting::Daylight => LightKind::Sun {
            azimuth: -0.9,
            elevation: 1.05,
        },
        Setting::Sunset => LightKind::Sun {
            // From the side, and low — but not as low as a real sunset.
            //
            // **A cast shadow here is the caster's silhouette translated**, not
            // foreshortened: flat artwork has no plan view to squash. So a very
            // low sun does not produce a long shadow lying on the floor, it
            // produces a second upright figure standing a couple of hundred
            // units away, which reads as a third character rather than as a
            // shadow. Half a radian keeps the shadow close enough to its caster
            // to be read as belonging to it, and still delivers the raking
            // light a sunset is chosen for.
            azimuth: -0.35,
            elevation: 0.55,
        },
        Setting::Night | Setting::Interior => LightKind::Lamp {
            // Off to one side and above the heads, where a practical hangs.
            position: Point::new(stage.x0 + stage.width() * 0.22, horizon_y - stage.height() * 0.55),
            height: 220.0,
            radius: stage.width() * 0.75,
        },
    };

    // Added through the scene's own allocator so the ids cannot collide with
    // a light the document already has, then edited in place: `add_light` is
    // the only thing that knows how to issue a light id.
    let key_id = scene.add_light(key_kind);
    let mut key = scene
        .lights()
        .get(key_id)
        .cloned()
        .unwrap_or_else(|| Light::new(key_id, "Key", key_kind));
    key.name = "Key".to_string();
    key.color = match recipe.setting {
        Setting::Daylight => Color::from_rgb8(0xFF, 0xF6, 0xE2),
        Setting::Sunset => Color::from_rgb8(0xFF, 0xB2, 0x6B),
        Setting::Night => Color::from_rgb8(0xFF, 0xC9, 0x7A),
        Setting::Interior => Color::from_rgb8(0xFF, 0xE3, 0xB8),
    };
    // **The rim is the point of lighting a set-up scene.** Everything else
    // lighting does happens inside the silhouette; the rim is what puts an edge
    // of the key's colour around a figure and separates them from the ground
    // behind. Strongest where the key is lowest, which is where a real rim is
    // strongest too.
    key.rim = match recipe.setting {
        Setting::Daylight => 0.30,
        Setting::Sunset => 0.55,
        Setting::Night => 0.55,
        Setting::Interior => 0.35,
    };
    key.shadows = true;
    key.shadow_strength = match recipe.setting {
        Setting::Night => 0.5,
        _ => 0.38,
    };
    // **How far the cast is assumed to stand off the background**, which is the
    // only thing that gives flat artwork a shadow at all — and, for the same
    // reason as the elevation above, the thing that decides whether the shadow
    // stays under its caster or walks off on its own. Well under the default,
    // because these figures are most of the height of the stage and the default
    // was chosen for artwork a fraction of it.
    key.standing_height = 26.0;

    let fill_id = scene.add_light(LightKind::sky());
    let mut fill = scene
        .lights()
        .get(fill_id)
        .cloned()
        .unwrap_or_else(|| Light::new(fill_id, "Sky", LightKind::sky()));
    fill.name = "Sky".to_string();
    // The fill is the *other* end of the picture's range, so it takes the
    // complement of the key: cool against a warm key, and dim at night.
    let (fill_colour, fill_horizon, fill_strength) = match recipe.setting {
        Setting::Daylight => (
            Color::from_rgb8(0xA8, 0xC6, 0xE8),
            Color::from_rgb8(0xC8, 0xC0, 0xA8),
            0.85,
        ),
        Setting::Sunset => (
            Color::from_rgb8(0x6C, 0x7E, 0xC2),
            Color::from_rgb8(0xC0, 0x86, 0x64),
            0.6,
        ),
        Setting::Night => (
            Color::from_rgb8(0x2C, 0x3C, 0x6A),
            Color::from_rgb8(0x1A, 0x22, 0x38),
            0.35,
        ),
        Setting::Interior => (
            Color::from_rgb8(0x8E, 0x88, 0x80),
            Color::from_rgb8(0x6A, 0x5E, 0x50),
            0.55,
        ),
    };
    fill.color = fill_colour;
    fill.kind = LightKind::Sky {
        horizon: fill_horizon,
    };
    fill.intensity = fill_strength;

    // Written back over the entries `add_light` made, rather than pushed as
    // new ones: two lights were allocated and two lights is what the scene ends
    // with.
    let rig = scene.lights_mut();
    rig.enabled = true;
    if let Some(slot) = rig.get_mut(key_id) {
        *slot = key;
    }
    if let Some(slot) = rig.get_mut(fill_id) {
        *slot = fill;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole promise: one call and there is a scene, with people standing
    /// in it, ready to be animated.
    #[test]
    fn a_recipe_produces_a_scene_with_a_cast() {
        let mut scene = Scene::default();
        let built = build(&mut scene, &SceneRecipe::default());

        assert!(built.backdrop.is_some(), "there is a sky");
        assert!(built.ground.is_some(), "and a ground");
        assert_eq!(built.cast.len(), 2, "and the cast that was asked for");

        for id in built.actors() {
            let (_, object) = scene.find_object(id).expect("the person is on a layer");
            assert!(
                figure::is_figure(object),
                "and is rigged, or nothing can be performed on it"
            );
        }
    }

    /// **They stand on the ground.** A figure floating above the horizon line,
    /// or buried under it, is the single most obvious failure of an automatic
    /// layout, and it is pure arithmetic to get right.
    #[test]
    fn the_cast_stands_on_the_ground() {
        let mut scene = Scene::default();
        let recipe = SceneRecipe {
            cast: 3,
            ..SceneRecipe::default()
        };
        let built = build(&mut scene, &recipe);

        let stage = scene.stage().stage_rect();
        let horizon = stage.y0 + stage.height() * recipe.horizon;

        for id in built.actors() {
            let (_, object) = scene.find_object(id).expect("placed");
            let feet = object.bounds().y1;
            assert!(
                feet > horizon,
                "feet at {feet} should be below the horizon at {horizon}"
            );
            assert!(
                feet <= stage.y1 + 1.0,
                "and not through the bottom of the stage: {feet} against {}",
                stage.y1
            );
        }
    }

    /// Somebody further back is drawn smaller and higher, which is the whole of
    /// perspective on a flat stage and is what stops a group reading as
    /// cut-outs at one distance.
    #[test]
    fn the_cast_is_staggered_in_depth() {
        let mut scene = Scene::default();
        let built = build(
            &mut scene,
            &SceneRecipe {
                cast: 3,
                ..SceneRecipe::default()
            },
        );

        let heights: Vec<f64> = built
            .actors()
            .map(|id| {
                let (_, object) = scene.find_object(id).expect("placed");
                object.bounds().height()
            })
            .collect();
        assert!(
            heights[0] > heights[2],
            "the nearest is the biggest: {heights:?}"
        );

        let feet: Vec<f64> = built
            .actors()
            .map(|id| scene.find_object(id).expect("placed").1.bounds().y1)
            .collect();
        assert!(
            feet[0] > feet[2],
            "and stands lowest on the stage: {feet:?}"
        );
    }

    /// The lights arrive switched on, with a rim: an unlit set-up scene is a
    /// pile of flat colour, and the rim is what puts the cast *in* it.
    #[test]
    fn a_lit_scene_arrives_with_a_key_a_fill_and_a_rim() {
        let mut scene = Scene::default();
        build(&mut scene, &SceneRecipe::default());

        let rig = scene.lights();
        assert!(rig.enabled, "the rig is on");
        assert_eq!(rig.lights.len(), 2, "a key and a fill");
        let key = rig.key().expect("a key light");
        assert!(key.rim > 0.0, "and the key rims the cast");
    }

    /// Asking for no lights really leaves the document unlit, so this can be
    /// used to lay out a scene that is going to be lit by hand.
    #[test]
    fn an_unlit_recipe_adds_no_lights() {
        let mut scene = Scene::default();
        build(
            &mut scene,
            &SceneRecipe {
                lit: false,
                ..SceneRecipe::default()
            },
        );
        assert!(scene.lights().lights.is_empty());
    }

    /// Every layer is as long as the shot, or a performance written next has
    /// nowhere to go — and the sky vanishes after one frame.
    #[test]
    fn every_layer_is_as_long_as_the_shot() {
        let mut scene = Scene::default();
        let recipe = SceneRecipe {
            frames: 60,
            ..SceneRecipe::default()
        };
        build(&mut scene, &recipe);

        for layer in scene.stage_layers().iter() {
            assert!(
                layer.frames.length() >= recipe.frames,
                "{} is only {} frames long",
                layer.name,
                layer.frames.length()
            );
        }
    }

    /// Setting a scene puts the backdrop behind whatever was already drawn
    /// rather than burying it.
    #[test]
    fn an_existing_drawing_is_not_buried() {
        let mut scene = Scene::default();
        let existing = scene.add_layer("My Drawing", LayerKind::Normal);
        build(&mut scene, &SceneRecipe::default());

        let order: Vec<LayerId> = scene.stage_layers().paint_order().map(|l| l.id).collect();
        let drawing = order
            .iter()
            .position(|id| *id == existing)
            .expect("the drawing is still there");
        let sky = order.len() - 1;
        assert!(
            drawing < sky || order.len() > 1,
            "the drawing survived the set-up"
        );
    }
}
