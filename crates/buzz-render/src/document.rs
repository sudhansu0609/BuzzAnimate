//! Encoding a document into a Vello scene.
//!
//! This is the walk from [`buzz_scene::Scene`] down through layers, tweens,
//! groups and nested symbols to the fills and strokes Vello draws. It lives
//! here, below the application, because **three callers must agree exactly**:
//! the window, the exporter, and the headless tests that read pixels back. A
//! second copy of this walk anywhere would be a second answer to "what does
//! this document look like", and the difference would show up as an export
//! that does not match the screen — the single most damaging kind of bug an
//! animation tool can have.
//!
//! Chrome is deliberately *not* here. Rulers, guides, selection handles and
//! the brush preview are authoring aids drawn in screen space by the
//! application, and they must never reach an exported frame.

use buzz_geom::{Affine, Projection, Shape as _};
use buzz_scene::{ColorTransform, LayerKind, Object, ObjectKind, Scene};
use peniko::Color;

use crate::SceneBuilder;
use crate::filters::FilterCache;
use crate::lighting::LightCache;
use std::sync::Arc;

/// How deep a symbol may nest before we stop.
///
/// A symbol containing an instance of itself is a cycle; without a limit the
/// renderer would recurse until the stack ran out. Animate refuses to create
/// one, but an imported or hand-edited file can contain it.
pub const MAX_SYMBOL_DEPTH: usize = 12;

/// Modifiers for drawing one frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct FrameOptions {
    /// Onion-skin ghost alpha. `None` draws the frame normally.
    pub ghost: Option<f64>,
    /// Draw ghosts as outlines rather than faded artwork.
    pub ghost_outlines: bool,
    /// When a mask actually clips what is under it.
    pub masks: MaskDisplay,
    /// Draw the document's lights.
    ///
    /// On by default: an unlit document has no lights, so this costs a boolean
    /// check. Off is for the authoring passes that want the artwork as drawn —
    /// onion-skin ghosts, which are reference rather than picture.
    pub lit: bool,
}

/// When mask layers clip the layers they claim.
///
/// # Why this is a choice at all
///
/// Animate does **not** show the mask effect while you are editing: a mask
/// layer clips its masked layers only once both are *locked*, because you
/// cannot draw inside a region you cannot see. Published output always masks.
/// So the stage asks for [`MaskDisplay::WhenLocked`] and the exporter asks for
/// [`MaskDisplay::Always`], and the difference between the window and the
/// finished frame is a deliberate, Animate-shaped one rather than an accident.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MaskDisplay {
    /// Always clip — what an export or a preview means.
    #[default]
    Always,
    /// Clip only where the mask layer is locked, as Animate's stage does.
    WhenLocked,
}

/// Draw a document's artwork for one frame, with its camera: no onion skins,
/// no preview, no editor chrome.
///
/// This is what an export renders and what a headless test reads back. The
/// document camera is applied here rather than by the caller, because a helper
/// that quietly skipped it would report that a camera pan moves nothing.
pub fn draw_document(builder: &mut SceneBuilder<'_>, scene: &Scene, frame: u32) {
    let camera = scene.camera_transform(frame);
    draw_frame(builder, scene, frame, camera, &FrameOptions::default());
}

/// Draw one frame's layers through a given camera transform.
pub fn draw_frame(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
) {
    draw_frame_lit(builder, scene, frame, camera, options, &mut LightCache::new());
}

/// Everything the draw walk keeps between frames.
///
/// Two caches travel together because they are both generated geometry with
/// the same lifetime and the same eviction rule; passing them separately would
/// mean two more arguments through every function in this file.
#[derive(Debug, Default)]
pub struct DrawCache {
    pub lights: LightCache,
    pub filters: FilterCache,
}

impl DrawCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Draw one frame, reusing lighting geometry between frames.
///
/// The caller owns the cache because the renderer is stateless by design: a
/// `SceneBuilder` lives for one frame, and geometry that took a boolean to
/// build must outlive it. See [`crate::lighting`].
pub fn draw_frame_lit(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    lights: &mut LightCache,
) {
    // The filter cache lives for this one frame, so a blur is rebuilt each
    // time. Callers that draw the same document repeatedly — the window —
    // should hold a `DrawCache` and call `draw_frame_cached` instead.
    let mut cache = DrawCache {
        lights: std::mem::take(lights),
        filters: FilterCache::new(),
    };
    draw_frame_cached(builder, scene, frame, camera, options, &mut cache);
    *lights = cache.lights;
}

/// Draw one frame, reusing both lighting and filter geometry between frames.
///
/// This is what the window calls. [`draw_frame_lit`] exists for callers that
/// only have a [`LightCache`]; it builds a filter cache for the one frame,
/// which is correct but rebuilds every blur each time.
pub fn draw_frame_cached(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    cache.lights.begin(if options.lit { scene.revision() } else { 0 });
    cache.filters.begin();
    draw_layers(builder, scene, scene.layers(), frame, camera, options, cache);
    cache.lights.end();
    cache.filters.end();
}

/// Draw one layer stack — the stage's, or a symbol's.
///
/// Masks are resolved here rather than by the caller because the rule is
/// positional: a mask claims the unbroken run of masked layers below it, and
/// only the stack knows what "below" means.
fn draw_layers(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    layers: &buzz_scene::LayerStack,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    // Which mask, if any, clips each layer, and whether that mask is in force.
    let masks = active_masks(layers, options.masks);
    let mut open_mask: Option<buzz_scene::LayerId> = None;

    for layer in layers.drawable_at(frame) {
        // A mask layer's own artwork is never drawn: it is a stencil, and
        // Animate hides it for the same reason. It still has to be *found*,
        // which is what `active_masks` did.
        if layer.kind == LayerKind::Mask && masks.values().any(|m| *m == layer.id) {
            continue;
        }

        // Masked layers arrive in one unbroken run per mask, so the clip is
        // opened once for the run rather than once per layer.
        let wanted = masks.get(&layer.id).copied();
        if wanted != open_mask {
            if open_mask.is_some() {
                builder.pop_isolation();
                open_mask = None;
            }
            if let Some(mask_id) = wanted
                && let Some(path) = mask_geometry(
                    layers,
                    mask_id,
                    frame,
                    Affine::IDENTITY,
                    &scene
                        .camera_projection_at_depth(frame, 0.0)
                        .unwrap_or_else(|| Projection::from_affine(camera)),
                    builder.tolerance(),
                )
            {
                builder.push_clip(&path);
                open_mask = Some(mask_id);
            }
        }

        // Layer parenting: what this layer inherits from the layer it
        // follows. Resolved here because only the stack knows the chain.
        let follows = layers.inherited_transform(layer.id, frame);
        draw_layer(builder, scene, layer, frame, camera, follows, options, cache);
    }

    if open_mask.is_some() {
        builder.pop_isolation();
    }
}

/// Every layer that is currently clipped, and by which mask.
fn active_masks(
    layers: &buzz_scene::LayerStack,
    display: MaskDisplay,
) -> std::collections::BTreeMap<buzz_scene::LayerId, buzz_scene::LayerId> {
    let mut out = std::collections::BTreeMap::new();
    for group in layers.mask_groups() {
        let in_force = match display {
            MaskDisplay::Always => true,
            MaskDisplay::WhenLocked => layers.get(group.mask).is_some_and(|l| l.locked),
        };
        if !in_force {
            continue;
        }
        for masked in group.masked {
            out.insert(masked, group.mask);
        }
    }
    out
}

/// The mask layer's artwork, as one path in render space.
///
/// Every shape it holds contributes, filled — Animate masks with the mask
/// layer's *shapes*, ignoring their colour, and a mask made of three separate
/// blobs shows through all three. Strokes are ignored for the same reason
/// Animate ignores them: a mask is a region, and a stroke is a line.
fn mask_geometry(
    layers: &buzz_scene::LayerStack,
    mask: buzz_scene::LayerId,
    frame: u32,
    place: Affine,
    projection: &Projection,
    tolerance: f64,
) -> Option<buzz_geom::BezPath> {
    let layer = layers.get(mask)?;
    let mut combined = buzz_geom::BezPath::new();
    // Built in document space and projected once, like everything else: a mask
    // on a tilted layer has to be foreshortened by exactly the same lens as the
    // artwork it clips, or it would clip the wrong region.
    let place = place * layers.inherited_transform(mask, frame);

    for object in layer.frames.resolved_at(frame).iter() {
        let mut flat = Vec::new();
        object.flatten(place, &mut flat);
        for (transform, shape) in flat {
            if shape.fill.is_none() {
                continue;
            }
            for element in (transform * shape.path).elements() {
                combined.push(*element);
            }
        }
    }

    if combined.elements().is_empty() {
        return None;
    }
    let mapped = projection.map_path(&combined, tolerance);
    (!mapped.elements().is_empty()).then_some(mapped)
}

/// Draw one layer's artwork.
#[allow(clippy::too_many_arguments, reason = "one call site; a struct would only move the arguments")]
fn draw_layer(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    layer: &buzz_scene::Layer,
    frame: u32,
    camera: Affine,
    // What this layer inherits from the layer it follows — Animate's Layer
    // Parenting. Identity for a layer that follows nothing, which is why a
    // document that uses none of this draws exactly as it did before.
    follows: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    {
        // How this layer is projected onto the frame.
        //
        // Layer depth draws artwork further from the camera smaller, and slides
        // it less as the camera pans; a **tilted** camera also foreshortens it,
        // turning the layer's rectangle into a trapezoid. Both fall out of the
        // same projection. A layer at or behind the camera, or turned past
        // edge-on, is skipped rather than drawn inside out.
        //
        // `camera` arrives already resolved for depth zero and is used as-is
        // when the shot is flat — which keeps every untilted document rendering
        // through exactly the affine it always did, down to the bit.
        let projection = if layer.depth == 0.0 && !scene.camera_has_tilt() {
            Projection::from_affine(camera)
        } else {
            match scene.camera_projection_at_depth(frame, layer.depth) {
                Some(projection) => projection,
                None => return,
            }
        };

        // Guides are authoring aids: visible on stage, never exported.
        let outline = layer.outline || (options.ghost.is_some() && options.ghost_outlines);
        let tint = outline.then_some(layer.color);
        let faded = layer.kind == LayerKind::Guide;

        // Resolve tweens: what is drawn between two keyframes exists nowhere
        // in the document and has to be interpolated.
        // How this layer is lit. Worked out once here rather than per shape:
        // every shape on a layer shares its depth, and therefore its distance
        // from the light and the length of what it casts.
        let rig = scene.lights();
        let lit = options.lit && rig.is_active() && tint.is_none();
        let stage_height = scene.stage().size.height;
        // Whether this layer is lit at all is decided here; *how much* light
        // reaches each shape is worked out per shape, because a lamp's whole
        // character is that it varies across the stage. Deciding it once per
        // layer made two squares either side of a lamp exactly as bright as
        // each other, which is a sun pretending to be a lamp.
        let lighting = lit.then_some((stage_height, layer.depth));

        // Layer parenting happens in the plane, before the lens.
        let projection = projection.pre_affine(follows);

        let ctx = DrawCtx {
            scene,
            frame,
            tint,
            faded,
            ghost: options.ghost,
            effect: ColorTransform::default(),
            adjust: None,
            blur: None,
            depth: 0,
            lighting,
            layer_depth: layer.depth,
            projection,
        };
        let resolved = layer.frames.resolved_at(frame);

        // **Shadows first, and all of them, before any of this layer's
        // artwork.** A shadow falls on what is *behind* its caster: drawing
        // each one just before its own shape would let a character's shadow
        // land on the character standing next to it on the same layer, which
        // is never what a flat drawing means.
        if lit
            && let Some(key) = rig.key()
        {
            let height = scene.shadow_height(layer.depth, key);
            for (object, owner) in resolved.iter_owned() {
                cast_shadows(
                    builder,
                    object,
                    owner,
                    Affine::IDENTITY,
                    key,
                    height,
                    layer.depth,
                    cache,
                    &ctx,
                );
            }
        }

        // A layer holding build-up paint is drawn into its own transparent
        // group. Additive compositing sums with the destination, and without
        // the group the destination would include the stage — a dark stroke on
        // a white background would sum to white and disappear. Inside, the sum
        // starts from nothing and means what it should.
        //
        // The group is skipped entirely when nothing on the layer is additive,
        // because every group is a render target and they are not free.
        let accumulates = resolved
            .iter()
            .any(|object| has_additive_paint(object, scene, 0));

        if accumulates {
            // Bounded to the layer's own artwork: an unbounded group would
            // cost a full-viewport buffer whatever the layer contains.
            //
            // Through the camera, because that is where the artwork actually
            // lands — a layer moved by depth or by a camera pan would
            // otherwise be clipped against where its geometry used to be.
            let bounds = layer
                .bounds_at(frame)
                .and_then(|b| ctx.projection.map_rect_bounds(b))
                .map(|b| b.inflate(2.0, 2.0))
                .unwrap_or_else(|| builder.clip_bounds());
            builder.push_isolation(bounds.intersect(builder.clip_bounds()));
        }

        // Filters on the layer itself, which Animate does not have: the
        // whole layer is one subject, so a blurred background layer is one
        // effect rather than one per object on it.
        let layer_fx = (!layer.filters.is_empty()).then(|| {
            let mut silhouette = buzz_geom::BezPath::new();
            for object in resolved.iter() {
                append_silhouette(object, Affine::IDENTITY, &mut silhouette);
            }
            buzz_fx::build(&layer.filters, &silhouette)
        });

        if let Some(fx) = &layer_fx {
            crate::filters::draw_ops(builder, &fx.behind, &ctx.projection, ctx.ghost.unwrap_or(1.0));
        }

        let layer_ctx = match layer_fx.as_ref().and_then(|fx| fx.adjust) {
            Some(adjust) => DrawCtx {
                adjust: Some(adjust),
                ..ctx.clone()
            },
            None => ctx.clone(),
        };

        if !layer_fx.as_ref().is_some_and(|fx| fx.hide_subject) {
            for (object, owner) in resolved.iter_owned() {
                let mut object_ctx = layer_ctx.clone();
                // A layer blur applies to every shape on the layer.
                object_ctx.blur = layer_fx.as_ref().and_then(|fx| fx.blur);
                draw_object(
                    builder,
                    object,
                    owner,
                    Affine::IDENTITY,
                    &object_ctx,
                    cache,
                );
            }
        }

        if let Some(fx) = &layer_fx {
            crate::filters::draw_ops(builder, &fx.over, &ctx.projection, ctx.ghost.unwrap_or(1.0));
        }

        if accumulates {
            builder.pop_isolation();
        }
    }
}

/// Does this object, or anything inside it, paint additively?
///
/// Instances are followed into the library, because a symbol full of build-up
/// paint placed on a layer still needs that layer isolated.
fn has_additive_paint(object: &Object, scene: &Scene, depth: usize) -> bool {
    if depth >= MAX_SYMBOL_DEPTH {
        return false;
    }
    match &object.kind {
        ObjectKind::Shape(shape) => shape.blend.is_additive(),
        ObjectKind::Group(children) => children
            .iter()
            .any(|child| has_additive_paint(child, scene, depth + 1)),
        ObjectKind::Warp(warp) => warp.shape.blend.is_additive(),
        ObjectKind::Armature(rig) => rig
            .parts
            .iter()
            .any(|part| has_additive_paint(&part.artwork, scene, depth + 1)),
        ObjectKind::Instance(instance) => {
            scene.library().get(instance.symbol).is_some_and(|symbol| {
                symbol.layers.iter().any(|layer| {
                    layer
                        .all_objects()
                        .any(|child| has_additive_paint(child, scene, depth + 1))
                })
            })
        }
    }
}

/// State the draw walk carries down through groups and nested symbols.
///
/// Descending into an instance changes nearly all of it — the frame, the
/// colour effect, the depth — so it travels as one value rather than as a
/// growing argument list.
#[derive(Clone)]
struct DrawCtx<'a> {
    scene: &'a Scene,
    /// Which frame of *this* timeline is being drawn. A nested graphic symbol
    /// runs on its own frame number, not the stage's.
    frame: u32,
    /// Outline view: draw silhouettes in this colour instead of the artwork.
    tint: Option<Color>,
    /// Guide layer.
    faded: bool,
    /// Onion-skin ghost alpha.
    ghost: Option<f64>,
    /// Accumulated colour effect from every enclosing instance.
    effect: ColorTransform,
    /// Adjust Color from a filter on this object or its layer, applied to
    /// every colour inside it.
    adjust: Option<buzz_fx::ColorAdjust>,
    /// A blur inherited from a filter, applied to each shape as it is drawn.
    blur: Option<(f64, f64, buzz_fx::Quality)>,
    depth: usize,
    /// Whether this layer is lit, and what the lights need to know about it:
    /// the stage's height, for the sky's gradient, and the layer's depth.
    ///
    /// How much light reaches each *shape* is worked out per shape, because a
    /// lamp varies across the stage. Carried down through groups and nested
    /// symbols, so a shape inside a character inside a scene is lit by the
    /// same sun as everything else.
    lighting: Option<(f64, f64)>,
    /// The depth of the layer being drawn, which sets how far a lamp is from
    /// it and how long its shadow runs.
    layer_depth: f64,
    /// How this layer's plane lands on the frame: depth, tilt, the camera's
    /// pan and zoom, and layer parenting, all in one.
    ///
    /// **Everything is drawn through this and nothing else.** Geometry is
    /// accumulated in *document* space on the way down — an object's placement,
    /// a group's, a symbol's — and projected once at the leaf. Carrying a
    /// pre-multiplied "world" transform alongside was what let lighting
    /// geometry be drawn through the object's own placement twice, and it
    /// cannot happen now: there is one transform, and it is applied in one
    /// place.
    projection: Projection,
}

impl DrawCtx<'_> {
    /// Final colour for a fill or stroke, with every modifier applied.
    ///
    /// Instance effects come first because they are part of the artwork; the
    /// guide fade and ghost alpha are authoring overlays applied on top.
    fn colour(&self, c: Color) -> Color {
        let adjusted = match self.adjust {
            Some(adjust) => adjust.apply(c),
            None => c,
        };
        self.overlay(self.effect.apply(adjusted))
    }

    /// Put document-space geometry where the lens says it goes.
    ///
    /// The single place a projection is applied. With no tilt it is an affine
    /// and the path keeps its curves; with tilt the path is flattened and
    /// mapped, and anything behind the camera is clipped away.
    fn project(&self, path: &buzz_geom::BezPath, tolerance: f64) -> buzz_geom::BezPath {
        self.projection.map_path(path, tolerance)
    }

    /// Apply only the authoring overlays.
    ///
    /// Outline view draws in the layer's identifying colour, which is chrome
    /// rather than artwork — running it through an instance's tint would
    /// defeat the point of colour-coding layers.
    fn overlay(&self, c: Color) -> Color {
        let c = if self.faded { fade(c) } else { c };
        match self.ghost {
            Some(alpha) => c.multiply_alpha(alpha as f32),
            None => c,
        }
    }
}

/// Draw one object.
///
/// Two transforms travel together: `parent` carries the camera and is what
/// geometry is drawn through, while `doc` is the same accumulation **without**
/// the camera — document space, which is where the lights live. Deriving one
/// from the other would mean inverting the camera per shape, and a camera
/// scaled to nothing has no inverse.
fn draw_object(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    if !object.visible {
        return;
    }

    // Filters and blend modes are the object's own, so they are applied here,
    // around everything it draws — including everything inside a group or a
    // symbol, which is what makes "blur this character" mean the character
    // rather than each of its pieces.
    if !object.filters.is_empty() || object.blend.needs_group() {
        draw_filtered(builder, object, owner, doc, ctx, cache);
        return;
    }

    draw_object_inner(builder, object, owner, doc, ctx, cache);
}

/// Draw an object that carries filters, a blend mode, or both.
///
/// Split out so the ordinary path — which is every object in almost every
/// document — stays a straight walk with nothing to skip past.
fn draw_filtered(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    // The subject's outline, in document space — the space the filter ops are
    // built in and projected from.
    let mut silhouette = buzz_geom::BezPath::new();
    append_silhouette(object, doc * object.transform, &mut silhouette);
    let painted = buzz_fx::build(&object.filters, &silhouette);

    // A blend mode needs a group to blend against; so does anything the filter
    // paints outside the artwork, or the group would clip it away.
    let reach = object
        .filters
        .iter()
        .filter(|f| f.enabled)
        .map(|f| f.kind.reach())
        .fold(0.0f64, f64::max);
    let grouped = object.blend.needs_group();
    if grouped {
        let bounds = ctx
            .projection
            .map_rect_bounds(buzz_scene::object::transform_rect(doc, object.bounds()))
            .map(|b| b.inflate(reach + 2.0, reach + 2.0))
            .unwrap_or_else(|| builder.clip_bounds());
        builder.push_blend(bounds.intersect(builder.clip_bounds()), object.blend);
    }

    let alpha = ctx.ghost.unwrap_or(1.0);
    crate::filters::draw_ops(builder, &painted.behind, &ctx.projection, alpha);

    if !painted.hide_subject {
        let inner = DrawCtx {
            adjust: painted.adjust.or(ctx.adjust),
            blur: painted.blur.or(ctx.blur),
            ..ctx.clone()
        };
        draw_object_inner(builder, object, owner, doc, &inner, cache);
    }

    crate::filters::draw_ops(builder, &painted.over, &ctx.projection, alpha);

    if grouped {
        builder.pop_isolation();
    }
}

/// The artwork itself, with no filters around it.
fn draw_object_inner(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    let doc = doc * object.transform;

    match &object.kind {
        ObjectKind::Group(children) => {
            for child in children {
                draw_object(builder, child, Some(child), doc, ctx, cache);
            }
        }

        // Rigged artwork draws **posed**. The deformation happens here, at
        // draw time, from the angles the document actually stores — the same
        // arrangement tweens use, and for the same reason: a stored deformed
        // copy would be a second answer that could disagree with the first.
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                // Posed rig artwork is rebuilt from the bones every frame, so
                // like a tween it has no identity worth caching against.
                draw_object(builder, &part, None, doc, ctx, cache);
            }
        }

        ObjectKind::Warp(warp) => {
            draw_shape(builder, owner, 1, &warp.warped(), doc, ctx, cache);
        }

        ObjectKind::Instance(instance) => {
            if ctx.depth >= MAX_SYMBOL_DEPTH {
                return;
            }
            let Some(symbol) = ctx.scene.library().get(instance.symbol) else {
                // A dangling reference draws nothing rather than crashing; the
                // Library panel is where a missing symbol gets reported.
                return;
            };

            // A graphic follows the parent playhead; a movie clip shows its
            // first frame while authoring.
            let inner = instance.resolve_frame(symbol.kind, ctx.frame, symbol.length());

            let mut inner_ctx = ctx.clone();
            inner_ctx.frame = inner;
            inner_ctx.depth += 1;
            // Compose rather than replace: a tinted symbol inside a faded one
            // must show both effects.
            inner_ctx.effect = instance.color.compose(&ctx.effect);

            // A symbol has its own layer stack, so it has its own masks. They
            // are always in force inside a symbol: the "only when locked" rule
            // is about editing the mask, and you are not editing this one —
            // you are looking at an instance of it placed somewhere else.
            let masks = active_masks(&symbol.layers, MaskDisplay::Always);
            let mut open_mask: Option<buzz_scene::LayerId> = None;

            for layer in symbol.layers.drawable_at(inner) {
                if layer.kind == LayerKind::Mask && masks.values().any(|m| *m == layer.id) {
                    continue;
                }

                let wanted = masks.get(&layer.id).copied();
                if wanted != open_mask {
                    if open_mask.is_some() {
                        builder.pop_isolation();
                        open_mask = None;
                    }
                    if let Some(mask_id) = wanted
                        && let Some(path) = mask_geometry(
                            &symbol.layers,
                            mask_id,
                            inner,
                            doc,
                            &ctx.projection,
                            builder.tolerance(),
                        )
                    {
                        builder.push_clip(&path);
                        open_mask = Some(mask_id);
                    }
                }

                // An outline already in force wins — the stage layer's outline
                // toggle applies to everything it contains.
                let layer_ctx = DrawCtx {
                    tint: ctx.tint.or_else(|| layer.outline.then_some(layer.color)),
                    faded: ctx.faded || layer.kind == LayerKind::Guide,
                    ..inner_ctx.clone()
                };
                // A symbol's layers can follow each other too, which is
                // how a character symbol is rigged inside itself.
                let follows = symbol.layers.inherited_transform(layer.id, inner);
                for (child, owner) in layer.frames.resolved_at(inner).iter_owned() {
                    draw_object(builder, child, owner, doc * follows, &layer_ctx, cache);
                }
            }

            if open_mask.is_some() {
                builder.pop_isolation();
            }
        }

        ObjectKind::Shape(shape) => draw_shape(builder, owner, 0, shape, doc, ctx, cache),
    }
}

/// Collect an object's filled outlines into one path, through `transform`.
///
/// Concatenated rather than unioned: filled non-zero, overlapping subpaths
/// merge visually, which is what a silhouette means — and a union would be a
/// boolean per object per frame for a result nobody would be able to tell
/// apart. Strokes are left out for the same reason a mask ignores them: a
/// silhouette is a region, and a stroke is a line.
fn append_silhouette(object: &Object, transform: Affine, out: &mut buzz_geom::BezPath) {
    let mut flat = Vec::new();
    object.flatten(transform, &mut flat);
    for (place, shape) in flat {
        if shape.fill.is_none() {
            continue;
        }
        for element in (place * shape.path).elements() {
            out.push(*element);
        }
    }
}

/// Paint one shape's fill and stroke, lit.
#[allow(clippy::too_many_arguments, reason = "one call path; a struct would only move the arguments")]
fn draw_shape(
    builder: &mut SceneBuilder<'_>,
    owner: Option<&Arc<Object>>,
    index: u16,
    shape: &buzz_scene::ShapeData,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    // The shape where it sits in the document, and where the lens puts it.
    // Everything drawn below goes through the second; everything *measured* —
    // lighting, which is a property of the scene rather than of the view — uses
    // the first.
    let placed = doc * shape.path.clone();
    let path = ctx.project(&placed, builder.tolerance());

    // Outline view: draw the silhouette in the layer colour instead of the
    // artwork, which is what the timeline's outline column does.
    if let Some(color) = ctx.tint {
        builder.stroke_hairline(&path, ctx.overlay(color), 1.0);
        return;
    }

    // Where this shape sits in the document, which is what the lights are
    // measured against.
    let here = placed.bounding_box().center();
    let light = ctx
        .lighting
        .map(|(stage_height, depth)| ctx.scene.lights().illuminate(here, depth, stage_height));

    if let Some(fill) = shape.fill {
        // The light reaches the fill first: this is the tint that makes a warm
        // key look warm and a blue sky fill look cold, before any geometry is
        // drawn on top of it.
        let base = match &light {
            Some(light) => light.apply(fill.color),
            None => fill.color,
        };
        let color = ctx.colour(base);

        // A blur replaces the fill rather than joining it: the artwork *is*
        // the soft stack of copies, and drawing the sharp shape as well would
        // put a hard edge back in the middle of it.
        if let Some((rx, ry, quality)) = ctx.blur {
            let ops = cache.filters.blur(
                owner,
                index,
                &shape.path,
                color,
                (rx, ry),
                quality,
                builder.tolerance(),
            );
            crate::filters::draw_ops(builder, &ops, &ctx.projection.pre_affine(doc), 1.0);
            if let Some(stroke) = shape.stroke {
                // A stroke under a blur is softened as its own outline, so a
                // blurred drawing keeps its lines instead of losing them.
                let outline = buzz_geom::outline_stroke(
                    &shape.path,
                    buzz_geom::StrokeStyle::new(stroke.width.max(f64::MIN_POSITIVE)),
                    builder.tolerance(),
                );
                let colour = ctx.colour(match &light {
                    Some(light) => light.apply(stroke.color),
                    None => stroke.color,
                });
                let ops = buzz_fx::blur_ops(
                    &outline,
                    colour,
                    (rx, ry),
                    quality,
                    builder.tolerance(),
                );
                crate::filters::draw_ops(builder, &ops, &ctx.projection.pre_affine(doc), 1.0);
            }
            return;
        }
        // Build-up paint sums its opacity with what is under it. The enclosing
        // isolation group, opened by `draw_frame`, is what keeps that sum away
        // from the stage behind it.
        if shape.blend.is_additive() {
            builder.fill_shape_additive(&path, color);
        } else {
            builder.fill_shape(&path, color);
        }

        // Then the modelling: the terminator away from the key light, and the
        // glint towards it. Both are the artwork's own outline, offset, which
        // is what makes them read as form rather than as a filter over it.
        if let Some(light) = &light
            && let Some(key) = ctx.scene.lights().key()
        {
            // **The same arguments the shadow pass used.** The cache keys on
            // the object, not on what the caller happens to want, so asking
            // for a different `height` or `modelling` here would either miss
            // the entry or — worse, and this is how it was found — quietly
            // return the shadow pass's geometry, which was built with
            // modelling switched off and therefore had no crescents at all.
            //
            // One entry holds all three pieces; each pass draws the ones it
            // is responsible for, and the booleans are paid for once.
            let modelling = ctx.scene.lights().modelling;
            let height = ctx.scene.shadow_height(ctx.layer_depth, key);
            let geometry = cache.lights.shade(
                owner,
                index,
                shape,
                doc,
                key,
                height,
                ctx.layer_depth,
                modelling,
            );

            if let Some(shade) = &geometry.shade {
                let shaded = light.apply_shaded(fill.color);
                let drawn = ctx.project(shade, builder.tolerance());
                builder.fill_shape(&drawn, ctx.colour(shaded));
            }
            if let Some(highlight) = &geometry.highlight {
                let glint = light.highlight(fill.color, key.color, modelling);
                let drawn = ctx.project(highlight, builder.tolerance());
                builder.fill_shape(&drawn, ctx.colour(glint));
            }
        }
    }

    if let Some(stroke) = shape.stroke {
        // A stroke is lit but never shaded: a terminator across a one-pixel
        // line is noise, and an outline that changed width with the light
        // would read as a drawing mistake.
        let base = match &light {
            Some(light) => light.apply(stroke.color),
            None => stroke.color,
        };
        let color = ctx.colour(base);
        if stroke.hairline {
            builder.stroke_hairline(&path, color, 1.0);
        } else {
            builder.stroke_shape(&path, color, stroke.width);
        }
    }
}

/// Draw every shadow this object throws, before the layer's artwork.
#[allow(clippy::too_many_arguments, reason = "one call site")]
fn cast_shadows(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    key: &buzz_light::Light,
    height: f64,
    depth: f64,
    cache: &mut DrawCache,
    ctx: &DrawCtx<'_>,
) {
    // Asked for with modelling on so the entry this builds is the same one
    // `draw_shape` will look up: one set of booleans, used by both passes.
    let modelling = ctx.scene.lights().modelling;
    if !object.visible {
        return;
    }
    let doc = doc * object.transform;

    match &object.kind {
        ObjectKind::Shape(shape) => {
            if shape.fill.is_none() {
                // An unfilled outline has nothing to block the light with.
                return;
            }
            let geometry = cache.lights.shade(owner, 0, shape, doc, key, height, depth, modelling);
            if let Some(cast) = &geometry.cast {
                let shadow = Color::from_rgba8(
                    0,
                    0,
                    0,
                    (key.shadow_strength.clamp(0.0, 1.0) * 255.0) as u8,
                );
                let drawn = ctx.project(cast, builder.tolerance());
                builder.fill_shape(&drawn, ctx.overlay(shadow));
            }
        }
        ObjectKind::Group(children) => {
            for child in children {
                cast_shadows(builder, child, Some(child), doc, key, height, depth, cache, ctx);
            }
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                cast_shadows(builder, &part, None, doc, key, height, depth, cache, ctx);
            }
        }
        ObjectKind::Warp(warp) => {
            let shape = warp.warped();
            if shape.fill.is_none() {
                return;
            }
            let geometry = cache.lights.shade(owner, 1, &shape, doc, key, height, depth, modelling);
            if let Some(cast) = &geometry.cast {
                let shadow = Color::from_rgba8(
                    0,
                    0,
                    0,
                    (key.shadow_strength.clamp(0.0, 1.0) * 255.0) as u8,
                );
                let drawn = ctx.project(cast, builder.tolerance());
                builder.fill_shape(&drawn, ctx.overlay(shadow));
            }
        }
        // A symbol's shadow is the shadow of what it contains, which needs the
        // library and the instance's own frame — the same walk `draw_object`
        // does. Left for now: an instance casts nothing (PROGRESS §7).
        ObjectKind::Instance(_) => {}
    }
}

/// Guide layers draw faintly, so they read as reference rather than artwork.
fn fade(color: Color) -> Color {
    color.multiply_alpha(0.35)
}
