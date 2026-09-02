//! Drawable objects on the stage.
//!
//! Objects are **immutable once shared**. Editing goes through
//! [`Arc::make_mut`], which clones an object only if another snapshot still
//! references it — so an edit touches the changed object and nothing else.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, FillMode, Point, Rect, Shape as _};
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::gradient::Gradient;
use crate::image::ImageFill;

/// Stable identity for an object, preserved across edits and undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u64);

/// What a fill or a stroke is painted with.
///
/// # Why the gradient is behind an `Arc`
///
/// A gradient carries a list of stops, so it is not `Copy`, and a fill is read
/// on every shape of every frame. Sharing it means a `Paint` is a tag and a
/// pointer however many stops it has, and — because artwork is immutable once
/// shared (see the module header) — copying a shape shares its gradient rather
/// than duplicating it. Editing one goes through [`Arc::make_mut`], exactly as
/// editing an object does.
/// **Not `Serialize`.** A paint can now hold a bitmap, and deriving the format
/// off the runtime type would embed a decoded photograph — tens of megabytes of
/// pixels — inside `document.json`. The `.buzz` format is written by the DTO
/// layer in `buzz-doc`, which stores the image once in `media/` and refers to
/// it by id; see the module header there for why that separation exists at all.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid(Color),
    Gradient(Arc<Gradient>),
    /// A bitmap. See [`crate::image`] for why an image is a fill rather than
    /// a kind of object — in short, because that is what Animate's Break Apart
    /// produces, and it makes every vector tool work on a photograph.
    Image(Box<ImageFill>),
}

impl Paint {
    /// One colour standing in for this paint.
    ///
    /// Everything that can only work in one colour goes through here: the
    /// lighting model, outline view, the Swatches panel, the colour wells. For
    /// a gradient it is the ramp's weighted mean — see
    /// [`Gradient::average_color`].
    pub fn color(&self) -> Color {
        match self {
            Self::Solid(c) => *c,
            Self::Gradient(g) => g.average_color(),
            Self::Image(i) => i.average_color(),
        }
    }

    pub fn gradient(&self) -> Option<&Gradient> {
        match self {
            Self::Gradient(g) => Some(g),
            _ => None,
        }
    }

    pub fn is_gradient(&self) -> bool {
        matches!(self, Self::Gradient(_))
    }

    /// The bitmap this paint draws, if it is one.
    pub fn image(&self) -> Option<&ImageFill> {
        match self {
            Self::Image(i) => Some(i),
            _ => None,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image(_))
    }

    /// The same paint with every colour in it passed through `f`.
    ///
    /// A colour effect, an Adjust Color filter and an onion-skin ghost are all
    /// defined as functions of one colour; this is how they reach a gradient,
    /// which is a list of them.
    pub fn map_colors(&self, f: impl Fn(Color) -> Color) -> Self {
        match self {
            Self::Solid(c) => Self::Solid(f(*c)),
            Self::Gradient(g) => Self::Gradient(Arc::new(g.map_colors(f))),
            // A bitmap's colours are its pixels, and rewriting thirty million
            // of them for a tint would cost more than the frame it is drawn
            // in. The renderer lays the effect **over** the picture instead, as
            // a blend; recorded in PROGRESS.md §7.
            //
            // **Callers must know that.** Returning the image untouched is the
            // right answer here and a silent no-op to anything that assumes
            // this recolours what it is given — which is how the lights came to
            // do nothing at all to imported artwork for a whole phase. See
            // `buzz_render::document::draw_lit_composited` for what lighting does
            // instead, and `buzz_light::Illumination::as_filter` for the
            // arithmetic that makes the two agree.
            Self::Image(i) => Self::Image(i.clone()),
        }
    }

    /// The same paint carried through a transform, so a gradient stays where it
    /// was painted when the artwork moves.
    ///
    /// A solid colour is unaffected, which is why this can be applied blindly.
    pub fn transformed(&self, t: Affine) -> Self {
        match self {
            Self::Solid(c) => Self::Solid(*c),
            Self::Gradient(g) => Self::Gradient(Arc::new(g.transformed(t))),
            Self::Image(i) => Self::Image(Box::new(i.transformed(t))),
        }
    }

    /// Interpolate towards `other`, for a tween.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        match (self, other) {
            (Self::Solid(a), Self::Solid(b)) => Self::Solid(crate::gradient::lerp_color(*a, *b, t)),
            (Self::Gradient(a), Self::Gradient(b)) => Self::Gradient(Arc::new(a.lerp(b, t))),
            // A solid tweening to a gradient, or the reverse. Interpolating the
            // flat colour towards the gradient's average would move the colour
            // and then jump to a ramp at the far end, which reads as a glitch
            // rather than as a transition. It switches instead, at the halfway
            // point, the way the spread mode does.
            (a, b) => {
                if t < 0.5 {
                    a.clone()
                } else {
                    b.clone()
                }
            }
        }
    }
}

impl From<Color> for Paint {
    fn from(c: Color) -> Self {
        Self::Solid(c)
    }
}

impl From<Gradient> for Paint {
    fn from(g: Gradient) -> Self {
        Self::Gradient(Arc::new(g))
    }
}

/// How a shape is painted inside.
#[derive(Debug, Clone, PartialEq)]
pub struct FillSpec {
    pub paint: Paint,
    pub rule: FillMode,
}

impl FillSpec {
    pub fn solid(color: Color) -> Self {
        Self {
            paint: Paint::Solid(color),
            rule: FillMode::NonZero,
        }
    }

    pub fn gradient(gradient: Gradient) -> Self {
        Self {
            paint: Paint::Gradient(Arc::new(gradient)),
            rule: FillMode::NonZero,
        }
    }

    /// A shape filled with a bitmap — what Break Apart produces.
    pub fn image(image: ImageFill) -> Self {
        Self {
            paint: Paint::Image(Box::new(image)),
            rule: FillMode::NonZero,
        }
    }

    /// The one colour this fill stands for. See [`Paint::color`].
    pub fn color(&self) -> Color {
        self.paint.color()
    }
}

/// How a shape's outline is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSpec {
    pub paint: Paint,
    /// Width in document units.
    pub width: f64,
    /// Animate's "hairline": always one pixel regardless of zoom.
    pub hairline: bool,
}

impl StrokeSpec {
    pub fn new(color: Color, width: f64) -> Self {
        Self {
            paint: Paint::Solid(color),
            width,
            hairline: false,
        }
    }

    pub fn gradient(gradient: Gradient, width: f64) -> Self {
        Self {
            paint: Paint::Gradient(Arc::new(gradient)),
            width,
            hairline: false,
        }
    }

    pub fn hairline(color: Color) -> Self {
        Self {
            paint: Paint::Solid(color),
            width: 0.0,
            hairline: true,
        }
    }

    /// The one colour this stroke stands for. See [`Paint::color`].
    pub fn color(&self) -> Color {
        self.paint.color()
    }
}

/// How a shape combines with the paint already on its layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PaintBlend {
    /// Ordinary source-over compositing. Two translucent shapes at alpha 0.2
    /// and 0.3 overlap to `0.2 + 0.3 x (1 - 0.2) = 0.44`, because the second
    /// only paints on what the first left showing. This is what Animate does
    /// and what every other shape in the document uses.
    #[default]
    Normal,
    /// **Build-up.** Opacities *add* where shapes overlap, so alpha 0.2 over
    /// alpha 0.3 gives exactly 0.5, and paint deepens as you work over it —
    /// the way ink or airbrush does.
    ///
    /// # Why this needs an isolation group
    ///
    /// Additive compositing sums the source and destination outright. Applied
    /// straight to the canvas it would sum with the *stage* as well: a black
    /// stroke at alpha 0.2 over a white background gives `white + a little
    /// black`, which clamps back to white and the stroke disappears. So a
    /// layer holding additive paint is rendered into its own transparent
    /// group, where the sum starts from nothing and means what it should, and
    /// that group is then composited over the stage normally.
    ///
    /// The layer is therefore the accumulation surface: additive strokes build
    /// up with everything on their own layer and composite normally onto the
    /// layers below, which is how a paint program's layer behaves.
    Additive,
}

impl PaintBlend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Additive => "Build Up",
        }
    }

    pub fn is_additive(self) -> bool {
        matches!(self, Self::Additive)
    }

    /// The alpha two overlapping shapes produce under this mode.
    ///
    /// Exposed and tested because it is the whole observable point of the
    /// mode, and because it is far easier to reason about here than by reading
    /// pixels back off a GPU — though a headless test does that too.
    pub fn combine_alpha(self, under: f64, over: f64) -> f64 {
        let (under, over) = (under.clamp(0.0, 1.0), over.clamp(0.0, 1.0));
        match self {
            Self::Normal => under + over * (1.0 - under),
            Self::Additive => (under + over).min(1.0),
        }
    }
}

/// A filled and/or stroked path.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeData {
    pub path: BezPath,
    pub fill: Option<FillSpec>,
    pub stroke: Option<StrokeSpec>,
    /// How this shape combines with the paint under it.
    pub blend: PaintBlend,
}

impl ShapeData {
    pub fn filled(path: BezPath, color: Color) -> Self {
        Self {
            path,
            fill: Some(FillSpec::solid(color)),
            stroke: None,
            blend: PaintBlend::Normal,
        }
    }

    pub fn stroked(path: BezPath, color: Color, width: f64) -> Self {
        Self {
            path,
            fill: None,
            stroke: Some(StrokeSpec::new(color, width)),
            blend: PaintBlend::Normal,
        }
    }

    /// The same shape, painting additively.
    pub fn with_blend(mut self, blend: PaintBlend) -> Self {
        self.blend = blend;
        self
    }
}

/// What an object actually is.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    Shape(ShapeData),
    /// Animate's Group: children move together but stay individually editable
    /// once you enter the group.
    Group(Vec<Arc<Object>>),
    /// A placed instance of a library symbol.
    ///
    /// Instances carry no artwork of their own — that lives in the library —
    /// so editing the symbol updates every instance at once.
    Instance(crate::symbol::SymbolInstance),
    /// Artwork rigged to a skeleton — Animate's Bone tool.
    ///
    /// The deformed artwork is derived from the pose rather than stored, so a
    /// keyframe holds a handful of angles and there is only ever one answer to
    /// what the rig looks like. See [`crate::rig`].
    Armature(crate::rig::ArmatureData),
    /// Artwork with warp handles on it — Animate's Asset Warp tool.
    Warp(crate::rig::WarpData),
}

/// Where an object sits and faces **in space** — Animate's 3D Rotation and 3D
/// Translation, on one object.
///
/// # What this is for
///
/// Layer depth arranges whole layers in space, and the camera can now tilt; but
/// every object still lies flat in its layer's plane, so a camera move slides
/// the layers past each other without ever turning anything. That reads as
/// cards sliding, which is what it is.
///
/// Giving an object its own angles makes it a plane of its own. Build a tree
/// out of three cards at slightly different angles and the camera passing it
/// turns them past each other; do the same with the walls of a house and it
/// has corners. It is still flat artwork — but flat artwork that faces
/// somewhere.
///
/// # Deliberately on every object
///
/// Animate allows 3D only on movie clip instances, because its 3D is a
/// property of a display object with a cached surface. Here it is a plane in a
/// projection, which costs nothing extra, so any object may have it: a shape, a
/// group, a symbol, a rigged character. Recorded as a deviation in §7.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Spatial {
    /// Tip about the horizontal axis, in radians. Animate's rotationX.
    pub rotation_x: f64,
    /// Turn about the vertical axis. Animate's rotationY.
    pub rotation_y: f64,
    /// Spin in the object's own plane. Animate's rotationZ.
    ///
    /// This one is a plain rotation and stays affine, but it belongs here
    /// rather than in [`Object::transform`] because it happens *after* the
    /// other two: spinning a card and then tipping it is not the same as
    /// tipping it and then spinning it.
    pub rotation_z: f64,
    /// How far in front of or behind its layer the object sits, in document
    /// units. Animate's translationZ. Negative is towards the camera.
    pub z: f64,
}

impl Spatial {
    /// Does this leave the object flat in its layer, exactly as it was before
    /// there was any such thing?
    ///
    /// The render path, the hit test and the format all take the old route when
    /// this is true — which is every object in every document that does not use
    /// it.
    pub fn is_flat(&self) -> bool {
        self.rotation_x == 0.0 && self.rotation_y == 0.0 && self.rotation_z == 0.0 && self.z == 0.0
    }

    /// The object's plane, as two basis vectors.
    pub fn basis(&self) -> ([f64; 3], [f64; 3]) {
        buzz_geom::Projection::rotated_basis(self.rotation_x, self.rotation_y, self.rotation_z)
    }

    /// Interpolate, so a motion tween can turn an object as it moves.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        // Angles take the short way round, as every other angle here does.
        let turn = |a: f64, b: f64| {
            let full = std::f64::consts::TAU;
            let mut delta = (b - a) % full;
            if delta > full / 2.0 {
                delta -= full;
            } else if delta < -full / 2.0 {
                delta += full;
            }
            a + delta * t
        };
        Self {
            rotation_x: turn(self.rotation_x, other.rotation_x),
            rotation_y: turn(self.rotation_y, other.rotation_y),
            rotation_z: turn(self.rotation_z, other.rotation_z),
            z: self.z + (other.z - self.z) * t,
        }
    }
}

/// An object placed on a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub id: ObjectId,
    /// Optional instance name, as shown in the Properties panel.
    pub name: Option<String>,
    /// Placement on the stage, relative to the layer.
    pub transform: Affine,
    pub kind: ObjectKind,
    /// Animate lets you lock individual objects as well as layers.
    pub locked: bool,
    pub visible: bool,

    /// Filters on this object — blur, drop shadow, glow, bevel, adjust colour.
    ///
    /// Animate allows these on movie clips, buttons and text only, because a
    /// raster filter needs a cached surface to work on. These are geometry
    /// (see `buzz-fx`), so there is nothing to cache and no reason to refuse
    /// them on a plain shape. Recorded as a deviation rather than an oversight.
    ///
    /// Empty for almost every object ever made, and an empty `Vec` allocates
    /// nothing.
    pub filters: Vec<buzz_fx::Filter>,

    /// How this object combines with what is painted behind it — Animate's
    /// Blend list. Distinct from [`ShapeData::blend`], which is about how one
    /// brush stroke accumulates with the next.
    pub blend: buzz_fx::Blend,

    /// Which way this object faces in space. Flat by default.
    pub spatial: Spatial,

    /// **The transformation point** — what this object rotates, skews and
    /// turns in space about. Animate's white circle on the Free Transform
    /// gizmo.
    ///
    /// In the object's **own** coordinates, before [`Object::transform`], so
    /// it stays where it was put on the artwork however the object is then
    /// moved, scaled or rotated. `None` means the centre of what the object
    /// actually covers, which is where Animate starts one and what everything
    /// here did before there was such a field — so a document that never
    /// touches it behaves exactly as it always did.
    ///
    /// Resolving `None` needs the library for an instance, so ask the scene:
    /// [`crate::Scene::pivot_of`].
    pub pivot: Option<Point>,

    /// **Live modifiers** — spring follow-through, wiggle — evaluated when the
    /// object is drawn rather than baked to keyframes (see [`crate::Modifier`]).
    /// Empty for almost every object, and an empty `Vec` allocates nothing, so
    /// this costs those objects nothing.
    pub modifiers: Vec<crate::Modifier>,

    /// **The text this object was typed as**, when it is text. The glyph
    /// outlines themselves live in the object's [`ShapeData`] — this is only
    /// what they were made from, kept so the words stay editable. `None` for
    /// everything that is not text, which is almost everything.
    pub text: Option<TextData>,

    /// **The drawing shown when this object is turned to face away** — a real
    /// turnaround's back view, rather than the front mirrored. The renderer
    /// swaps to it once the object's yaw passes edge-on. `None` for everything
    /// that has no separate back, which is almost everything.
    pub reverse: Option<Arc<Object>>,
}

/// The source of a text object: the string and its size. The rendered outlines
/// are the object's `ShapeData`; regenerating them from this is how editing the
/// words works. See [`crate::Object::text`].
#[derive(Debug, Clone, PartialEq)]
pub struct TextData {
    pub content: String,
    /// Nominal glyph height, in document units.
    pub size: f64,
}

impl Object {
    pub fn shape(id: ObjectId, shape: ShapeData) -> Self {
        Self {
            id,
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Shape(shape),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: buzz_fx::Blend::Normal,
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            reverse: None,
        }
    }

    pub fn group(id: ObjectId, children: Vec<Arc<Object>>) -> Self {
        Self {
            id,
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Group(children),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: buzz_fx::Blend::Normal,
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            reverse: None,
        }
    }

    pub fn with_transform(mut self, transform: Affine) -> Self {
        self.transform = transform;
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Bounds in the object's own space, before [`Self::transform`].
    pub fn local_bounds(&self) -> Rect {
        match &self.kind {
            ObjectKind::Shape(s) => {
                let mut bb = s.path.bounding_box();
                // A stroke extends beyond the path; selection handles and
                // culling both need the painted extent, not the geometric one.
                if let Some(stroke) = &s.stroke
                    && !stroke.hairline
                {
                    let half = stroke.width * 0.5;
                    bb = bb.inflate(half, half);
                }
                bb
            }
            ObjectKind::Group(children) => children
                .iter()
                .map(|c| c.bounds())
                .reduce(|a, b| a.union(b))
                .unwrap_or(Rect::ZERO),
            // An instance's extent depends on the library, which is not
            // reachable from here. Callers that need real bounds resolve them
            // through `Scene::instance_bounds`; this keeps hit-testing and
            // culling from silently treating an instance as empty.
            ObjectKind::Instance(_) => Rect::new(-1.0, -1.0, 1.0, 1.0),
            // Rigged artwork is measured **posed**: the bounds of a bent arm
            // are not the bounds it was drawn at, and selection handles left
            // behind where the artwork used to be are worse than none.
            ObjectKind::Armature(rig) => rig.local_bounds(),
            ObjectKind::Warp(warp) => warp.local_bounds(),
        }
    }

    /// Bounds after this object's own transform.
    /// The transformation point in the object's own space, when it does not
    /// need the library to work out.
    ///
    /// An instance measures nothing on its own — [`Object::local_bounds`]
    /// returns a placeholder for one — so anything holding a scene should use
    /// [`crate::Scene::pivot_local_of`], which resolves it properly.
    pub fn pivot_local(&self) -> Point {
        self.pivot.unwrap_or_else(|| self.local_bounds().center())
    }

    pub fn bounds(&self) -> Rect {
        let local = self.local_bounds();
        if local == Rect::ZERO {
            return local;
        }
        transform_rect(self.transform, local)
    }

    /// Flatten to `(accumulated transform, shape)` pairs in paint order.
    ///
    /// Groups nest arbitrarily, so both rendering and hit-testing need the
    /// resolved world transform of every leaf. Collecting once avoids walking
    /// the tree separately for each.
    pub fn flatten(&self, parent: Affine, out: &mut Vec<(Affine, ShapeData)>) {
        if !self.visible {
            return;
        }
        let world = parent * self.transform;
        match &self.kind {
            ObjectKind::Shape(s) => out.push((world, s.clone())),
            ObjectKind::Group(children) => {
                for child in children {
                    child.flatten(world, out);
                }
            }
            // Instances need the library to resolve, so they are skipped here
            // and expanded by the renderer, which has it.
            ObjectKind::Instance(_) => {}
            // Rigged artwork flattens to what it currently *looks* like, which
            // is what every caller means: the renderer, hit-testing and
            // culling all want the posed artwork, never the drawn one.
            ObjectKind::Armature(rig) => {
                for part in rig.posed() {
                    part.flatten(world, out);
                }
            }
            ObjectKind::Warp(warp) => out.push((world, warp.warped())),
        }
    }

    /// Number of leaf shapes, for diagnostics and progress reporting.
    pub fn shape_count(&self) -> usize {
        match &self.kind {
            ObjectKind::Shape(_) => 1,
            ObjectKind::Group(children) => children.iter().map(|c| c.shape_count()).sum(),
            ObjectKind::Instance(_) => 1,
            ObjectKind::Armature(rig) => rig.parts.iter().map(|p| p.artwork.shape_count()).sum(),
            ObjectKind::Warp(_) => 1,
        }
    }

    /// The symbol this object instantiates, if it is an instance.
    pub fn instance(&self) -> Option<&crate::symbol::SymbolInstance> {
        match &self.kind {
            ObjectKind::Instance(i) => Some(i),
            _ => None,
        }
    }

    /// Place a library symbol.
    pub fn instance_of(id: ObjectId, symbol: crate::symbol::SymbolId) -> Self {
        Self {
            id,
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Instance(crate::symbol::SymbolInstance::new(symbol)),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: buzz_fx::Blend::Normal,
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            reverse: None,
        }
    }
}

/// Bounding box of a transformed rectangle.
///
/// Rotation means the transformed corners are not axis-aligned, so this takes
/// the bounds of all four rather than transforming two opposite corners — a
/// classic source of clipped-off geometry.
pub fn transform_rect(t: Affine, r: Rect) -> Rect {
    let corners = [
        t * Point::new(r.x0, r.y0),
        t * Point::new(r.x1, r.y0),
        t * Point::new(r.x1, r.y1),
        t * Point::new(r.x0, r.y1),
    ];
    let mut out = Rect::from_points(corners[0], corners[1]);
    for c in &corners[2..] {
        out = out.union_pt(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Rect as KRect;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        KRect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    fn shape_at(id: u64, x: f64, y: f64) -> Object {
        Object::shape(
            ObjectId(id),
            ShapeData::filled(square(x, y, 10.0), Color::WHITE),
        )
    }

    #[test]
    fn shape_bounds_follow_the_transform() {
        let o = shape_at(1, 0.0, 0.0).with_transform(Affine::translate((100.0, 50.0)));
        let bb = o.bounds();
        assert!(
            (bb.x0 - 100.0).abs() < 1e-9 && (bb.y0 - 50.0).abs() < 1e-9,
            "{bb:?}"
        );
        assert!((bb.width() - 10.0).abs() < 1e-9);
    }

    /// A rotated rectangle's bounds must cover all four corners.
    #[test]
    fn rotation_expands_bounds_correctly() {
        let o = shape_at(1, -5.0, -5.0).with_transform(Affine::rotate(std::f64::consts::FRAC_PI_4));
        let bb = o.bounds();
        let expected = 10.0 * std::f64::consts::SQRT_2;
        assert!(
            (bb.width() - expected).abs() < 1e-6,
            "45-degree rotation should widen 10 to {expected}, got {}",
            bb.width()
        );
    }

    #[test]
    fn stroke_width_is_included_in_bounds() {
        let plain = Object::shape(
            ObjectId(1),
            ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE),
        );
        let stroked = Object::shape(
            ObjectId(2),
            ShapeData::stroked(square(0.0, 0.0, 10.0), Color::WHITE, 4.0),
        );
        assert!((plain.bounds().width() - 10.0).abs() < 1e-9);
        assert!(
            (stroked.bounds().width() - 14.0).abs() < 1e-9,
            "a width-4 stroke should add 2 each side, got {}",
            stroked.bounds().width()
        );
    }

    /// The behaviour build-up exists for: two translucent strokes at 0.2 and
    /// 0.3 overlap at exactly 0.5, not the 0.44 ordinary compositing gives.
    #[test]
    fn build_up_adds_opacities_where_normal_compositing_does_not() {
        let under = 0.2;
        let over = 0.3;

        let additive = PaintBlend::Additive.combine_alpha(under, over);
        assert!(
            (additive - 0.5).abs() < 1e-12,
            "0.2 and 0.3 should build up to 0.5, got {additive}"
        );

        let normal = PaintBlend::Normal.combine_alpha(under, over);
        assert!(
            (normal - 0.44).abs() < 1e-12,
            "ordinary compositing gives 0.2 + 0.3x0.8 = 0.44, got {normal}"
        );
    }

    /// Opacity cannot exceed fully opaque however much paint is laid down.
    #[test]
    fn build_up_saturates_at_opaque_rather_than_overflowing() {
        assert_eq!(PaintBlend::Additive.combine_alpha(0.6, 0.7), 1.0);
        assert_eq!(PaintBlend::Additive.combine_alpha(1.0, 1.0), 1.0);

        // And out-of-range input is clamped rather than believed.
        assert_eq!(PaintBlend::Additive.combine_alpha(-5.0, 0.25), 0.25);
        assert_eq!(PaintBlend::Normal.combine_alpha(2.0, 0.5), 1.0);
    }

    /// Painting onto nothing gives back exactly what was painted, in either
    /// mode — the two only differ where they overlap something.
    #[test]
    fn painting_on_empty_canvas_is_the_same_in_both_modes() {
        for alpha in [0.0, 0.15, 0.5, 1.0] {
            assert_eq!(PaintBlend::Normal.combine_alpha(0.0, alpha), alpha);
            assert_eq!(PaintBlend::Additive.combine_alpha(0.0, alpha), alpha);
        }
    }

    /// Repeated strokes build up linearly, which is what makes working over an
    /// area deepen it predictably.
    #[test]
    fn repeated_build_up_strokes_accumulate_in_equal_steps() {
        let mut alpha = 0.0;
        for expected in [0.2, 0.4, 0.6, 0.8, 1.0] {
            alpha = PaintBlend::Additive.combine_alpha(alpha, 0.2);
            assert!(
                (alpha - expected).abs() < 1e-12,
                "got {alpha}, want {expected}"
            );
        }
        // The sixth stroke can add nothing; it is already opaque.
        assert_eq!(PaintBlend::Additive.combine_alpha(alpha, 0.2), 1.0);
    }

    #[test]
    fn shapes_composite_normally_unless_asked_otherwise() {
        let plain = ShapeData::filled(square(0.0, 0.0, 1.0), Color::BLACK);
        assert_eq!(plain.blend, PaintBlend::Normal);
        assert!(!plain.blend.is_additive());

        let built = plain.with_blend(PaintBlend::Additive);
        assert!(built.blend.is_additive());
        assert_eq!(built.blend.label(), "Build Up");
    }

    #[test]
    fn hairline_strokes_do_not_inflate_bounds() {
        let o = Object::shape(
            ObjectId(1),
            ShapeData {
                path: square(0.0, 0.0, 10.0),
                fill: None,
                stroke: Some(StrokeSpec::hairline(Color::WHITE)),
                blend: PaintBlend::Normal,
            },
        );
        // A hairline is a screen-space width; it has no document-space extent.
        assert!((o.bounds().width() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn group_bounds_enclose_all_children() {
        let g = Object::group(
            ObjectId(10),
            vec![
                Arc::new(shape_at(1, 0.0, 0.0)),
                Arc::new(shape_at(2, 100.0, 100.0)),
            ],
        );
        let bb = g.bounds();
        assert!(
            (bb.x0 - 0.0).abs() < 1e-9 && (bb.x1 - 110.0).abs() < 1e-9,
            "{bb:?}"
        );
    }

    #[test]
    fn nested_transforms_accumulate() {
        let inner = Arc::new(shape_at(1, 0.0, 0.0).with_transform(Affine::translate((10.0, 0.0))));
        let group = Object::group(ObjectId(10), vec![inner])
            .with_transform(Affine::translate((100.0, 0.0)));

        let mut leaves = Vec::new();
        group.flatten(Affine::IDENTITY, &mut leaves);
        assert_eq!(leaves.len(), 1);

        let world = leaves[0].0;
        let origin = world * Point::new(0.0, 0.0);
        assert!(
            (origin.x - 110.0).abs() < 1e-9,
            "transforms should compose to 110, got {}",
            origin.x
        );
    }

    #[test]
    fn invisible_objects_are_skipped_when_flattening() {
        let mut hidden = shape_at(1, 0.0, 0.0);
        hidden.visible = false;
        let g = Object::group(
            ObjectId(10),
            vec![Arc::new(hidden), Arc::new(shape_at(2, 20.0, 0.0))],
        );

        let mut leaves = Vec::new();
        g.flatten(Affine::IDENTITY, &mut leaves);
        assert_eq!(leaves.len(), 1, "the hidden child should not be emitted");
    }

    #[test]
    fn deeply_nested_groups_flatten_and_count() {
        let leaf = Arc::new(shape_at(1, 0.0, 0.0));
        let mut node = Arc::new(Object::group(ObjectId(100), vec![leaf]));
        for i in 0..8 {
            node = Arc::new(Object::group(ObjectId(200 + i), vec![node]));
        }
        assert_eq!(node.shape_count(), 1);

        let mut leaves = Vec::new();
        node.flatten(Affine::IDENTITY, &mut leaves);
        assert_eq!(leaves.len(), 1);
    }

    #[test]
    fn an_empty_group_has_zero_bounds_and_no_leaves() {
        let g = Object::group(ObjectId(1), vec![]);
        assert_eq!(g.bounds(), Rect::ZERO);
        assert_eq!(g.shape_count(), 0);

        let mut leaves = Vec::new();
        g.flatten(Affine::IDENTITY, &mut leaves);
        assert!(leaves.is_empty());
    }

    /// Structural sharing: cloning must not deep-copy children.
    #[test]
    fn cloning_shares_children_rather_than_copying_them() {
        let child = Arc::new(shape_at(1, 0.0, 0.0));
        let group = Object::group(ObjectId(10), vec![Arc::clone(&child)]);

        let before = Arc::strong_count(&child);
        let copy = group.clone();
        let after = Arc::strong_count(&child);

        assert_eq!(
            after,
            before + 1,
            "cloning a group should add one reference, not duplicate the child"
        );
        assert_eq!(group, copy);
    }
}
