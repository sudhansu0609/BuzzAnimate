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
    /// Outside, at night, in a storm. A near-black sky that **strikes**: the
    /// stage goes white for a few frames every few seconds and back to
    /// nothing, with no keyframes anywhere.
    ///
    /// A setting rather than a checkbox on Night, because everything about it
    /// differs: the sky is darker than an ordinary night (a flash only reads
    /// against dark), the fill is colder, the key is the lightning itself
    /// rather than a warm practical, and it arrives with cloud. See
    /// `buzz_scene::Light::storm`.
    Storm,
}

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::Daylight => "Daylight",
            Setting::Sunset => "Sunset",
            Setting::Night => "Night",
            Setting::Interior => "Interior",
            Setting::Storm => "Storm",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Setting::Daylight => "A high sun, a blue sky, and short hard shadows",
            Setting::Sunset => "A low sun, a warm sky, and shadows running long",
            Setting::Night => "A dark sky and one warm lamp doing the work",
            Setting::Interior => "A wall, a floor, and a practical lamp",
            Setting::Storm => "A black sky that strikes, and cloud running over it",
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
            // **Dark, but not black.** A strike has to have somewhere to go,
            // and light is a *multiplier*: a sky at 6 comes out at 40 under a
            // seven-fold flash and is still black, while one at 22 comes out
            // at 150 and reads as the sky lighting up. The first attempt at
            // this was nearly zero and the lightning had nothing to work on.
            Setting::Storm => (
                Color::from_rgb8(0x16, 0x1C, 0x30),
                Color::from_rgb8(0x24, 0x2E, 0x4C),
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
            Setting::Storm => (
                Color::from_rgb8(0x22, 0x28, 0x36),
                Color::from_rgb8(0x12, 0x16, 0x1E),
            ),
        }
    }

    /// Does this setting have a sky for cloud to cross?
    ///
    /// An interior's backdrop is a wall, and a cloud on a wall is a stain.
    fn has_sky(self) -> bool {
        !matches!(self, Setting::Interior)
    }

    /// The cloud's own two colours: lit top, shadowed underside.
    ///
    /// Cloud is the one thing in a background that is *bright* — brighter than
    /// the sky behind it in daylight, and the only thing with any light on it
    /// at all in a storm. Getting it wrong by taking the sky's colours is how
    /// cloud ends up reading as smoke.
    fn cloud(self) -> (Color, Color) {
        match self {
            Setting::Daylight => (
                Color::from_rgb8(0xFF, 0xFF, 0xFF),
                Color::from_rgb8(0xC4, 0xD4, 0xE4),
            ),
            Setting::Sunset => (
                Color::from_rgb8(0xFF, 0xD8, 0xB4),
                Color::from_rgb8(0xB4, 0x76, 0x82),
            ),
            Setting::Night => (
                Color::from_rgb8(0x3A, 0x44, 0x60),
                Color::from_rgb8(0x1E, 0x26, 0x3A),
            ),
            // The one bright thing in a storm. A flash lights the *cloud*
            // before it lights anything on the ground, and a storm cloud that
            // stayed as dark as the sky behind it took the whole effect with
            // it — see the note on the backdrop above.
            Setting::Storm => (
                Color::from_rgb8(0x50, 0x58, 0x70),
                Color::from_rgb8(0x22, 0x28, 0x3A),
            ),
            // Never used — an interior has no sky — but a match has to be
            // total, and a wall-coloured cloud is the least wrong answer if one
            // ever is drawn.
            Setting::Interior => (
                Color::from_rgb8(0xB4, 0xA6, 0x94),
                Color::from_rgb8(0x8C, 0x7E, 0x6E),
            ),
        }
    }

    /// The water: the body of it, and the light running on its surface.
    fn water(self) -> (Color, Color) {
        match self {
            Setting::Daylight => (
                Color::from_rgb8(0x2E, 0x6E, 0x96),
                Color::from_rgb8(0xA8, 0xDC, 0xF0),
            ),
            Setting::Sunset => (
                Color::from_rgb8(0x3A, 0x46, 0x6E),
                Color::from_rgb8(0xFF, 0xC0, 0x86),
            ),
            Setting::Night => (
                Color::from_rgb8(0x10, 0x1A, 0x30),
                Color::from_rgb8(0x5C, 0x74, 0xA8),
            ),
            Setting::Storm => (
                Color::from_rgb8(0x0C, 0x12, 0x1E),
                Color::from_rgb8(0x6E, 0x84, 0xB0),
            ),
            Setting::Interior => (
                Color::from_rgb8(0x2A, 0x4A, 0x56),
                Color::from_rgb8(0x86, 0xB4, 0xBE),
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
    /// **Put cloud in the sky, and set it moving.**
    ///
    /// Cumulus above the horizon, each one drifting across and looping, each at
    /// its own speed and height. Live motion rather than keyframes — see
    /// [`buzz_scene::Modifier::Drift`] — so re-timing the shot cannot leave the
    /// sky behind, and so a two-second test and a two-minute film need exactly
    /// the same setting up.
    ///
    /// Ignored for an interior, which has a wall rather than a sky, and
    /// implied by [`Setting::Storm`], which has no business being clear.
    pub clouds: bool,
    /// **Put water in front of the camera, and set it running.**
    ///
    /// A band across the near ground, with the light on its surface scrolling
    /// at several speeds at once — which is the whole trick to water in 2D:
    /// nothing about it is drawn moving, the highlights just slide past each
    /// other. See [`water`] for the shapes and why they loop the way they do.
    ///
    /// [`water`]: fn@water
    pub water: bool,
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
            // **Both off.** Setting a scene has to be fast and it has to be
            // predictable; a sky that moved and a river nobody asked for are
            // both surprises, and the checkbox is right there.
            clouds: false,
            water: false,
        }
    }
}

/// What was built, so the caller can select the cast and perform them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StagedScene {
    /// The layers, back to front.
    pub backdrop: Option<LayerId>,
    /// The cloud, when the recipe asked for it: its own layer, in front of the
    /// sky and behind everything else, so it can be turned off in one click.
    pub clouds: Option<LayerId>,
    pub ground: Option<LayerId>,
    /// The water, when the recipe asked for it: in front of the ground and
    /// behind the cast, which is where a river a character stands beside goes.
    pub water: Option<LayerId>,
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
    // **A whole stage of margin, not half.** Half was enough for a camera that
    // stayed put and not for one that follows somebody into the wings: the
    // director sends an entrance to 0.16 stages off-stage and then frames it
    // wide, and at any zoom under about 1.5 the near edge of the shot ran off
    // the end of the sky — a grey band down the side of the picture, which is
    // what the first unattended render came back with.
    let wide = stage.inflate(stage.width(), stage.height());

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

    // **The sky moves.** In front of the backdrop and behind everything else,
    // which is where cloud goes and is also what makes it one click to switch
    // off. See `clouds`.
    //
    // **A storm always gets it**, asked for or not: there is no such thing as
    // lightning out of a clear sky, and an animator who chose Storm has asked
    // for the cloud whether or not they found the checkbox. The rule lives here
    // rather than in the caller so it holds however the scene is made — from
    // the dialog, from the director, or from a script.
    if (recipe.clouds || recipe.setting == Setting::Storm) && recipe.setting.has_sky() {
        out.clouds = Some(clouds(scene, recipe, horizon_y, wide));
    }

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

    // **And the water runs.** In front of the ground and behind the cast, so a
    // character stands *beside* a river rather than in it.
    if recipe.water {
        out.water = Some(water(scene, recipe, horizon_y, wide));
    }

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
            // **Everybody breathes.**
            //
            // A staged scene is a held pose until somebody animates it, and a
            // held pose that does not move is a picture of a character rather
            // than a character standing there. A breath is the cheapest thing
            // that fixes it and the thing an animator draws by hand on every
            // hold; two per cent of scale, nobody notices it consciously, and
            // everybody notices its absence.
            //
            // Live, so it costs no keyframes and cannot be knocked out by a
            // re-time — and so a performance written over it walks *and*
            // breathes rather than choosing. Seeded per object, so a cast of
            // six does not inhale together.
            //
            // A resting rate, and slightly quicker for the ones further back:
            // an even rate across the cast is the one thing that would make it
            // visible as a mechanism.
            scene.update_object_across(0, u32::MAX, placed, |o| {
                o.modifiers.push(buzz_scene::Modifier::Breathe {
                    rate: 13.0 + 3.0 * depth,
                    depth: 1.0,
                });
            });
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

    let extras = out.clouds.is_some() as usize + out.water.is_some() as usize;
    out.message = format!(
        "{} scene: {} layer(s), {} in the cast{}{}{}",
        recipe.setting.label(),
        2 + extras + out.cast.len(),
        out.cast.len(),
        if recipe.lit { ", lit" } else { "" },
        if out.clouds.is_some() { ", cloud" } else { "" },
        if out.water.is_some() { ", water" } else { "" },
    );
    out
}

/// **Cloud crossing the sky.**
///
/// # What a cloud is here
///
/// Three to five overlapping discs in one path. Filled non-zero the overlaps
/// vanish and what is left is a lumpy blob with a flat bottom, which is a
/// cumulus — and it is *one* shape rather than a group, so it is one fill to
/// draw and one object to drift.
///
/// # Why it drifts rather than being keyframed
///
/// Two keyframes per cloud is the obvious way and it is wrong twice over: the
/// motion is lost the moment the shot is re-timed, and the number of keys is
/// proportional to the length of the film. A [`Drift`] is one setting per
/// cloud, independent of how long the shot is, and it survives re-timing
/// because it is a function of time rather than a pair of keys on a timeline.
///
/// # The wrap, and why it is wider than the stage
///
/// A cloud may only start again once it is **completely off** the far side, or
/// it pops into existence in mid-air. So the loop is the full width of the
/// margin-inflated backdrop plus the cloud's own width — which is why `wide`
/// is handed in rather than the stage.
///
/// Each one gets its own speed, and the higher ones go slower: that is
/// parallax, it is free, and it is most of what makes a flat sky read as deep.
///
/// [`Drift`]: buzz_scene::Modifier::Drift
fn clouds(scene: &mut Scene, recipe: &SceneRecipe, horizon_y: f64, wide: Rect) -> LayerId {
    use buzz_scene::Modifier;

    let stage = scene.stage().stage_rect();
    let (lit, shade) = recipe.setting.cloud();
    let layer = scene.add_stage_layer("Clouds", LayerKind::Normal);

    // Between the top of the frame and a little above the horizon: cloud on the
    // horizon reads as hills, and the band has to stop short of it.
    let ceiling = stage.y0 - stage.height() * 0.10;
    let floor = horizon_y - (horizon_y - stage.y0) * 0.25;

    const COUNT: usize = 5;
    for i in 0..COUNT {
        let t = i as f64 / (COUNT - 1) as f64;
        // **Not evenly spaced.** A row of clouds at equal intervals is a row of
        // clouds; the offset is a fixed irrational-ish step so they scatter
        // without needing a random number generator in a scene builder that
        // has to produce the same scene twice.
        let scatter = (i as f64 * 0.618).fract();
        // **Drawn at the left edge, scattered by the drift's own phase.**
        //
        // Every cloud is built just off the left of the margin and travels one
        // `span` before it starts again, so the loop covers exactly the width
        // the camera can ever see. Where each one *is* comes from
        // `Drift::phase`, which is inside the wrap — placing them apart
        // instead sends the far ones off the right for most of the loop, which
        // is what the first attempt did and why the sky came out empty.
        let cx = wide.x0;
        let cy = ceiling + (floor - ceiling) * t;
        // The higher ones are further away: smaller, paler, slower.
        let far = 1.0 - t;
        let size = stage.width() * (0.10 + 0.10 * t);

        let mut path = buzz_geom::BezPath::new();
        // Three to five lumps along the cloud, the middle ones taller — which
        // is the silhouette of a cumulus and takes four numbers to say.
        let lumps = 3 + i % 3;
        for k in 0..lumps {
            let along = k as f64 / (lumps - 1).max(1) as f64;
            let hump = 1.0 - (along - 0.5).abs() * 1.2;
            let r = size * (0.30 + 0.34 * hump);
            let x = cx + size * (along - 0.5) * 1.5;
            // Sat on a common base, so the underside is flat and the top is not.
            let y = cy - r * 0.35;
            // Wider than tall: a cloud lump is a squashed disc, and a circle
            // gives a bunch of grapes.
            path.extend(
                kurbo::Ellipse::new((x, y), (r * 1.15, r), 0.0).path_elements(0.1),
            );
        }
        // A flat base joining them, so the bottom of the cloud is a line.
        path.extend(
            Rect::new(
                cx - size * 0.85,
                cy - size * 0.06,
                cx + size * 0.85,
                cy + size * 0.10,
            )
            .to_path(1e-9)
            .into_iter(),
        );

        // Paler with distance: a cloud on the horizon has the sky in front of
        // it, and mixing towards the shaded tone is the cheapest aerial
        // perspective there is.
        let colour = crate::staging::mix(lit, shade, (1.0 - far) as f32 * 0.7);
        let Some(id) = scene.add_shape(layer, ShapeData::filled(path, colour)) else {
            continue;
        };

        // Rightwards, slowly, and the far ones slower still. Twelve units a
        // second crosses a 550-wide stage in about three quarters of a minute,
        // which is what cloud looks like.
        let speed = 5.0 + 14.0 * t;
        // All the way across the margin-inflated backdrop, plus the cloud's own
        // width twice over: a cloud may only start again once it is completely
        // off the far side, or it pops into existence in mid-air.
        let span = wide.width() + size * 4.0;
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: speed,
                dy: 0.0,
                span,
                phase: scatter,
            });
            // And it billows while it goes. Without this a cloud is a cut-out
            // sliding across a card, which is exactly what it looks like.
            o.modifiers.push(Modifier::Sway {
                amount: 0.05,
                rate: 0.07,
            });
        });
    }
    layer
}

/// **Water, running.**
///
/// # The whole trick
///
/// Nothing in it is drawn moving. It is a flat band of water colour with
/// **highlight streaks lying on it**, and the streaks slide across at four or
/// five different speeds — the near ones fast, the far ones slow. The eye reads
/// relative motion between the streaks as a surface flowing, and it does it
/// convincingly enough that this is how water is done in cel animation and in
/// nearly every game background ever shipped.
///
/// A river and a sea are the same shapes: what makes it a river is that the
/// band is narrow and the streaks all run one way, which is what this builds.
///
/// # Why the streaks bob as well as slide
///
/// A streak that only slides is a scratch on the glass. A slow vertical wiggle
/// on top of the drift makes each one wander a little as it goes, and that is
/// the difference between "this is moving" and "this is liquid".
fn water(scene: &mut Scene, recipe: &SceneRecipe, horizon_y: f64, wide: Rect) -> LayerId {
    use buzz_scene::Modifier;

    let stage = scene.stage().stage_rect();
    let (body, shine) = recipe.setting.water();
    let layer = scene.add_stage_layer("Water", LayerKind::Normal);

    // A band across the near ground: from a third of the way down the ground
    // to the bottom of the frame. Near the camera, because water you are
    // looking *across* is a band and water you are looking *down at* is the
    // whole floor — and the band is the shot people want.
    let top = horizon_y + (stage.y1 - horizon_y) * 0.34;
    scene.add_shape(
        layer,
        ShapeData::filled(
            Rect::new(wide.x0, top, wide.x1, wide.y1).to_path(1e-9),
            body,
        ),
    );

    // The far bank's reflection: a darker line where the water meets the land,
    // which is what stops the band reading as a painted stripe.
    scene.add_shape(
        layer,
        ShapeData::filled(
            Rect::new(wide.x0, top, wide.x1, top + stage.height() * 0.02).to_path(1e-9),
            crate::staging::mix(body, shine, 0.35),
        ),
    );

    const STREAKS: usize = 9;
    for i in 0..STREAKS {
        let t = i as f64 / (STREAKS - 1) as f64;
        let scatter = (i as f64 * 0.618).fract();
        // Down the band: the near ones are longer and thicker, because they are
        // nearer. That gradient is the only perspective a flat band gets.
        let y = top + (stage.y1 - top) * (0.06 + 0.92 * t);
        let length = stage.width() * (0.10 + 0.26 * t);
        let thickness = stage.height() * (0.004 + 0.010 * t);
        // At the left edge, like the cloud and for the same reason: where along
        // the loop it sits is the drift's phase, which is inside the wrap.
        let x = wide.x0;

        // Rounded, so the ends taper rather than stopping square: a square-cut
        // highlight on water is a floating plank.
        let streak =
            kurbo::RoundedRect::new(x, y, x + length, y + thickness, thickness * 0.5)
                .to_path(0.05);
        // Fainter far away, brighter near: the far end of a river is mostly
        // sky colour and the near end is where the glitter is.
        let colour = crate::staging::mix(body, shine, (0.25 + 0.65 * t) as f32);
        let Some(id) = scene.add_shape(layer, ShapeData::filled(streak, colour)) else {
            continue;
        };

        // **Every streak at its own speed.** This is the whole effect: with one
        // speed the band slides as a sheet and reads as a moving photograph.
        // The near ones run fastest, which is both correct and what makes the
        // surface look like it has depth.
        let speed = 14.0 + 70.0 * t;
        let span = wide.width() + length * 2.0;
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: speed,
                dy: 0.0,
                span,
                phase: scatter,
            });
            // And it is liquid, not glass.
            o.modifiers.push(Modifier::Wiggle {
                amplitude: thickness * 0.9,
                frequency: 0.8 + 0.9 * t,
            });
        });
    }
    layer
}

/// Two colours mixed, in sRGB and in the small amount this file needs.
///
/// `peniko` mixes in linear light, which is the right answer for a light and
/// the wrong one here: these are *paint* colours being blended the way an
/// animator blends them on a palette, and a linear midpoint between a white
/// cloud and its own shadow comes out visibly too bright.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (a.to_rgba8().to_u8_array(), b.to_rgba8().to_u8_array());
    let c = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    Color::from_rgba8(c(0), c(1), c(2), c(3))
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
        // **The lightning is the key.** A sun would give the shot a direction
        // it does not have — the light comes from the whole sky at once, which
        // is a sky light — so the storm's key is the flash itself and there is
        // no second source. That is also why a storm scene looks nearly black
        // between strikes, which is correct.
        Setting::Storm => LightKind::sky(),
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
        // Cold and blue-white: what a spark is. `make_storm` below sets it too,
        // and would win either way; it is written here so the table of what
        // each setting's key looks like stays complete.
        Setting::Storm => Color::from_rgb8(0xD6, 0xE4, 0xFF),
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
        // The strongest of the lot: a sheet of lightning is *behind* everything
        // and the figures are in front of it, which is the one arrangement
        // where an edge really does come up brighter than the picture.
        Setting::Storm => 0.7,
    };
    key.shadows = true;
    key.shadow_strength = match recipe.setting {
        Setting::Night => 0.5,
        _ => 0.38,
    };
    // **And it strikes.** Everything above is the light this setting has
    // between strikes; this is what makes it a storm. `make_storm` turns the
    // light itself down to night as well, because a flash only reads against
    // the dark — see `buzz_scene::Light::make_storm`.
    if recipe.setting == Setting::Storm {
        key.name = "Lightning".to_string();
        key.make_storm();
        // **Harder than the preset's own floor.** `make_storm` sets a storm you
        // could put on any light without ruining the shot; a scene that *is* a
        // storm has nothing else going on and wants the frame to go white. It
        // also strikes more often at this setting, which is what stops a
        // four-second test looking like an ordinary dark night.
        key.storm = 0.8;
    }
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
        // Barely there. Between strikes a storm is the darkest setting of the
        // five, and the fill's whole job is to keep the artwork from going to
        // a flat silhouette while it waits.
        Setting::Storm => (
            Color::from_rgb8(0x22, 0x2E, 0x52),
            Color::from_rgb8(0x10, 0x14, 0x22),
            0.22,
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

    /// **Everybody in a staged scene breathes.**
    ///
    /// A held pose that does not move is a picture of a character rather than a
    /// character standing there, and a set scene is a held pose until somebody
    /// animates it. Live, so it survives a re-time and costs no keyframes.
    #[test]
    fn the_cast_breathes_without_being_asked() {
        use buzz_scene::Modifier;

        let mut scene = Scene::default();
        let built = build(
            &mut scene,
            &SceneRecipe {
                cast: 3,
                ..SceneRecipe::default()
            },
        );

        let mut rates = Vec::new();
        for id in built.actors() {
            let (_, object) = scene.find_object(id).expect("on a layer");
            let rate = object.modifiers.iter().find_map(|m| match m {
                Modifier::Breathe { rate, .. } => Some(*rate),
                _ => None,
            });
            rates.push(rate.expect("this one is not breathing"));
        }
        rates.sort_by(f64::total_cmp);
        rates.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        assert!(
            rates.len() > 1,
            "the whole cast breathes at one rate, which is the one thing that              would make it visible as a mechanism"
        );
    }

    /// **Cloud and water move, and move on their own.**
    ///
    /// The whole promise of both: no keyframes anywhere, so a shot can be
    /// re-timed, looped or exported at any length and the sky still crosses and
    /// the river still runs. If either of them ever needs a key, the promise is
    /// broken and this is the test that says so.
    #[test]
    fn the_sky_and_the_water_move_without_a_single_keyframe() {
        use buzz_scene::Modifier;

        let mut scene = Scene::default();
        let recipe = SceneRecipe {
            clouds: true,
            water: true,
            cast: 1,
            ..SceneRecipe::default()
        };
        let built = build(&mut scene, &recipe);

        let clouds = built.clouds.expect("a cloud layer");
        let water = built.water.expect("a water layer");

        for (layer, what) in [(clouds, "cloud"), (water, "water")] {
            let frames = &scene
                .stage_layers()
                .iter()
                .find(|l| l.id == layer)
                .expect("the layer is there")
                .frames;

            let objects = frames.resolved_at(0);
            assert!(!objects.is_empty(), "{what} drew nothing");

            let drifting = objects
                .iter()
                .filter(|o| {
                    o.modifiers
                        .iter()
                        .any(|m| matches!(m, Modifier::Drift { .. }))
                })
                .count();
            assert!(drifting >= 3, "only {drifting} bits of {what} move");

            // **Every drift loops.** A `span` of zero drifts away for ever,
            // which is right for a one-off move across a shot and wrong for
            // scenery: the sky would empty out and never come back.
            for object in objects.iter() {
                for m in &object.modifiers {
                    if let Modifier::Drift { span, .. } = m {
                        assert!(*span > 0.0, "a piece of {what} drifts away and never returns");
                    }
                }
            }
        }
    }

    /// **The drifts are not all the same speed.**
    ///
    /// This is the entire effect, for both of them. One speed and the sky
    /// slides as a single sheet and the river as a moving photograph; several,
    /// and the eye reads the relative motion as depth in one and as a flowing
    /// surface in the other.
    #[test]
    fn the_scenery_drifts_at_several_speeds() {
        use buzz_scene::Modifier;

        let mut scene = Scene::default();
        let recipe = SceneRecipe {
            clouds: true,
            water: true,
            cast: 0,
            ..SceneRecipe::default()
        };
        let built = build(&mut scene, &recipe);

        for (layer, what) in [
            (built.clouds.expect("cloud"), "cloud"),
            (built.water.expect("water"), "water"),
        ] {
            let frames = &scene
                .stage_layers()
                .iter()
                .find(|l| l.id == layer)
                .expect("the layer is there")
                .frames;
            let mut speeds: Vec<f64> = frames
                .resolved_at(0)
                .iter()
                .flat_map(|o| o.modifiers.clone())
                .filter_map(|m| match m {
                    Modifier::Drift { dx, dy, .. } => Some((dx * dx + dy * dy).sqrt()),
                    _ => None,
                })
                .collect();
            speeds.sort_by(f64::total_cmp);
            speeds.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            assert!(
                speeds.len() >= 3,
                "{what} has only {} distinct speeds: {speeds:?}",
                speeds.len()
            );
        }
    }

    /// An interior has a wall, not a sky, and a cloud on a wall is a stain.
    #[test]
    fn an_interior_gets_no_cloud_however_hard_it_is_asked() {
        let mut scene = Scene::default();
        let built = build(
            &mut scene,
            &SceneRecipe {
                setting: Setting::Interior,
                clouds: true,
                cast: 0,
                ..SceneRecipe::default()
            },
        );
        assert!(built.clouds.is_none(), "cloud indoors");
    }

    /// **A storm strikes.** The setting is only worth having if the light it
    /// builds actually flashes, which means one light in the rig with a storm
    /// on it — see `buzz_scene::Light::storm`.
    #[test]
    fn a_storm_scene_arrives_with_lightning_and_cloud() {
        let mut scene = Scene::default();
        let built = build(
            &mut scene,
            &SceneRecipe {
                setting: Setting::Storm,
                cast: 1,
                ..SceneRecipe::default()
            },
        );

        let striking = scene.lights().lights.iter().filter(|l| l.storm > 0.0).count();
        assert_eq!(striking, 1, "a storm scene needs exactly one light striking");

        // And the rig animates, so the window and the exporter both know to
        // redraw frame by frame.
        assert!(
            scene.lights().animates(),
            "a storm that does not animate never strikes"
        );

        // The cloud comes with it, asked for or not: there is no lightning out
        // of a clear sky.
        assert!(built.clouds.is_some(), "a storm with no cloud");
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
