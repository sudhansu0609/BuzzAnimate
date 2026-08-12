//! Import Flash `.swf` movies.
//!
//! # What an SWF is, and how it maps onto a document
//!
//! An SWF is not a document; it is a **tape of instructions for a player**.
//! Definition tags introduce characters into a dictionary, and control tags
//! place, move and remove them on a depth-ordered display list, with
//! `ShowFrame` marking the end of each frame. Importing means running that
//! tape and recording what the player would have shown, as an authorable
//! document.
//!
//! The mapping follows the shape of the format rather than inventing one:
//!
//! | SWF | Document |
//! |---|---|
//! | `DefineShape` | a Graphic symbol in the library |
//! | `DefineSprite` | a MovieClip symbol, with its own timeline |
//! | `PlaceObject` at a depth | an instance on the layer for that depth |
//! | `ShowFrame` | the next frame |
//! | `RemoveObject` | the layer's span ends there |
//!
//! **One layer per depth** is the key decision. SWF's display list is depth
//! ordered and a given depth holds one object at a time, which is exactly what
//! a layer is. Flattening everything onto one layer would lose the stacking
//! order that the movie depends on.
//!
//! # What is not read
//!
//! ActionScript, sounds, video, embedded fonts and text, and bitmaps are
//! recorded in the [`ImportReport`] rather than silently dropped. Bytecode is
//! Phase 8's business; text needs the font subsystem; bitmaps need the media
//! pipeline. Gradients become flat colours, which is visible and fixable
//! rather than invisible and mysterious.

pub mod shape;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use buzz_geom::Affine;
use buzz_scene::{
    FillSpec, Layer, LayerId, LayerStack, Object, ObjectId, Scene, ShapeData, StrokeSpec, Symbol,
    SymbolId, SymbolKind,
};
use swf::{CharacterId, Tag};

pub use shape::StyledPath;

/// Why an import failed outright.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("input/output error: {0}")]
    Io(#[from] std::io::Error),
    #[error("this file could not be read as an SWF: {0}")]
    Swf(String),
    #[error(
        "this SWF is compressed with LZMA, which needs the compressed data \
         intact; the file appears to be truncated or damaged"
    )]
    Truncated,
}

impl From<swf::error::Error> for ImportError {
    fn from(e: swf::error::Error) -> Self {
        Self::Swf(e.to_string())
    }
}

/// What came across, and what did not.
#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    pub frames: usize,
    pub layers: usize,
    pub shapes: usize,
    pub sprites: usize,
    pub instances: usize,
    /// Fills replaced by a flat colour because they are gradients or bitmaps.
    pub approximated_fills: usize,
    /// Features present in the file that this importer does not handle.
    pub unsupported: Vec<String>,
}

impl ImportReport {
    fn note_unsupported(&mut self, what: &str) {
        if let Some(existing) = self
            .unsupported
            .iter_mut()
            .find(|e| e.as_str() == what || e.starts_with(&format!("{what} (x")))
        {
            let count = existing
                .rsplit_once(" (x")
                .and_then(|(_, n)| n.trim_end_matches(')').parse::<usize>().ok())
                .unwrap_or(1);
            *existing = format!("{what} (x{})", count + 1);
        } else {
            self.unsupported.push(what.to_string());
        }
    }

    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} frames, {} layers, {} shapes, {} sprites, {} instances",
            self.frames, self.layers, self.shapes, self.sprites, self.instances
        )
    }
}

/// Import a `.swf` file.
pub fn import(path: impl AsRef<Path>) -> Result<(Scene, ImportReport), ImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    import_bytes(&bytes)
}

/// Import from bytes.
pub fn import_bytes(bytes: &[u8]) -> Result<(Scene, ImportReport), ImportError> {
    let buffer = swf::decompress_swf(bytes)?;
    let movie = swf::parse_swf(&buffer)?;

    let mut report = ImportReport::default();
    let mut scene = Scene::empty();
    let mut ids = IdSource::default();

    let rect = movie.header.stage_size();
    scene.stage_mut().size = buzz_geom::Size::new(
        (rect.x_max - rect.x_min).to_pixels(),
        (rect.y_max - rect.y_min).to_pixels(),
    );
    scene.stage_mut().frame_rate = movie.header.frame_rate().to_f64();
    if let Some(background) = movie.header.background_color() {
        scene.stage_mut().background =
            peniko::Color::from_rgba8(background.r, background.g, background.b, background.a);
    }

    // Characters are defined before they are placed, but a sprite's tags are
    // nested inside it, so the dictionary is built as the tape is walked.
    let mut dictionary: HashMap<CharacterId, SymbolId> = HashMap::new();
    let mut library: Vec<Symbol> = Vec::new();

    let layers = run_timeline(
        &movie.tags,
        &mut dictionary,
        &mut library,
        &mut ids,
        &mut report,
        0,
    );

    for symbol in library {
        scene.library_mut().insert(symbol);
    }

    report.layers = layers.len();
    for (index, layer) in layers.into_iter().enumerate() {
        scene.edit_stage_layers().insert(index, layer);
    }
    scene.reserve_ids_above(ids.peek());

    Ok((scene, report))
}

/// Hands out ids for the scene being built.
#[derive(Default)]
struct IdSource(u64);

impl IdSource {
    fn take(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
    fn peek(&self) -> u64 {
        self.0
    }
}

/// How deep sprites may nest before we stop following them.
const MAX_SPRITE_DEPTH: usize = 12;

/// What is currently on the display list at one depth.
struct Placed {
    character: CharacterId,
    transform: Affine,
    name: Option<String>,
    /// Frame this placement began on.
    since: u32,
}

/// Run a tag list as a player would, recording what it shows.
///
/// Returns one layer per depth used, ordered so that the highest depth is the
/// front-most layer — matching both SWF's painting order and our layer stack,
/// which is front-first.
fn run_timeline(
    tags: &[Tag<'_>],
    dictionary: &mut HashMap<CharacterId, SymbolId>,
    library: &mut Vec<Symbol>,
    ids: &mut IdSource,
    report: &mut ImportReport,
    depth: usize,
) -> Vec<Layer> {
    // Depth to (keyframes built so far, what is placed now).
    let mut tracks: BTreeMap<u16, Track> = BTreeMap::new();
    let mut frame = 0u32;

    for tag in tags {
        match tag {
            // -- definitions --------------------------------------------------
            Tag::DefineShape(s) => {
                let paths = shape::convert(&s.styles, &s.shape);
                let approximated = paths.iter().filter(|p| p.approximated_fill).count();
                if approximated > 0 {
                    report.approximated_fills += approximated;
                    report.note_unsupported("a gradient or bitmap fill, imported as a flat colour");
                }

                let symbol = build_shape_symbol(s.id, &paths, ids);
                dictionary.insert(s.id, symbol.id);
                library.push(symbol);
                report.shapes += 1;
            }

            Tag::DefineSprite(sprite) => {
                if depth >= MAX_SPRITE_DEPTH {
                    report.note_unsupported("a sprite nested too deeply to follow");
                    continue;
                }
                // A sprite is a timeline of its own, so it runs recursively and
                // becomes a MovieClip.
                let inner = run_timeline(&sprite.tags, dictionary, library, ids, report, depth + 1);

                let mut symbol = Symbol::new(
                    SymbolId(ids.take()),
                    format!("Sprite {}", sprite.id),
                    SymbolKind::MovieClip,
                );
                let mut stack = LayerStack::new();
                for (index, layer) in inner.into_iter().enumerate() {
                    stack.insert(index, layer);
                }
                if stack.is_empty() {
                    stack.push_front(Layer::normal(LayerId(ids.take()), "Layer_1"));
                }
                symbol.layers = stack;

                dictionary.insert(sprite.id, symbol.id);
                library.push(symbol);
                report.sprites += 1;
            }

            // -- display list -------------------------------------------------
            Tag::PlaceObject(place) => {
                let track = tracks.entry(place.depth).or_insert_with(|| Track::new(place.depth));
                let matrix = place.matrix.map(to_affine).unwrap_or(Affine::IDENTITY);
                let name = place.name.map(|n| n.to_string_lossy(swf::UTF_8));

                match place.action {
                    swf::PlaceObjectAction::Place(id) | swf::PlaceObjectAction::Replace(id) => {
                        // Replacing swaps the character but keeps the depth, so
                        // either way the old span ends here and a new one opens.
                        track.close(frame);
                        track.current = Some(Placed {
                            character: id,
                            transform: matrix,
                            name,
                            since: frame,
                        });
                    }
                    swf::PlaceObjectAction::Modify => {
                        // A move is a new keyframe with the same character.
                        if let Some(existing) = &track.current {
                            let character = existing.character;
                            let previous_name = existing.name.clone();
                            let transform = match place.matrix {
                                Some(_) => matrix,
                                // No matrix means "leave it where it was".
                                None => existing.transform,
                            };
                            track.close(frame);
                            track.current = Some(Placed {
                                character,
                                transform,
                                name: name.or(previous_name),
                                since: frame,
                            });
                        }
                    }
                }

                if place.color_transform.is_some() {
                    report.note_unsupported("a colour transform on a placement");
                }
                if place.filters.as_ref().is_some_and(|f| !f.is_empty()) {
                    report.note_unsupported("a filter effect");
                }
                if place.blend_mode.is_some_and(|b| b != swf::BlendMode::Normal) {
                    report.note_unsupported("a blend mode");
                }
                if place.clip_depth.is_some() {
                    // A clip depth is SWF's mask. Our masks are positional, and
                    // inventing one here would guess at which layers it covers.
                    report.note_unsupported("a mask (clip depth)");
                }
            }

            Tag::RemoveObject(remove) => {
                if let Some(track) = tracks.get_mut(&remove.depth) {
                    track.close(frame);
                    track.current = None;
                }
            }

            Tag::ShowFrame => frame += 1,

            // -- knowingly not read -------------------------------------------
            Tag::DoAction(_) | Tag::DoAbc(_) | Tag::DoAbc2(_) | Tag::DoInitAction { .. } => {
                report.note_unsupported("ActionScript");
            }
            Tag::DefineText(_) | Tag::DefineText2(_) | Tag::DefineEditText(_) => {
                report.note_unsupported("text");
            }
            Tag::DefineFont(_)
            | Tag::DefineFont2(_)
            | Tag::DefineFont4(_)
            | Tag::DefineFontInfo(_) => {
                report.note_unsupported("an embedded font");
            }
            Tag::DefineBits { .. }
            | Tag::DefineBitsJpeg2 { .. }
            | Tag::DefineBitsJpeg3(_)
            | Tag::DefineBitsLossless(_) => {
                report.note_unsupported("a bitmap");
            }
            Tag::DefineSound(_) | Tag::SoundStreamHead(_) | Tag::SoundStreamHead2(_) => {
                report.note_unsupported("sound");
            }
            Tag::DefineVideoStream(_) => report.note_unsupported("video"),
            Tag::DefineMorphShape(_) => report.note_unsupported("a morph shape"),
            Tag::DefineButton(_) | Tag::DefineButton2(_) => {
                report.note_unsupported("a button");
            }
            _ => {}
        }
    }

    // The movie ends; every open span ends with it.
    let end = frame.max(1);
    let mut layers = Vec::new();
    for (_depth, mut track) in tracks {
        track.close(end);
        if let Some(layer) = track.into_layer(dictionary, ids, report) {
            layers.push(layer);
        }
    }

    if depth == 0 {
        report.frames = report.frames.max(end as usize);
    }

    // Highest depth in front: our stack is front-first, SWF paints low depths
    // first, so the order reverses.
    layers.reverse();
    layers
}

/// One depth's worth of history.
struct Track {
    depth: u16,
    current: Option<Placed>,
    /// (start frame, end frame, what was placed).
    spans: Vec<(u32, u32, Placed)>,
}

impl Track {
    fn new(depth: u16) -> Self {
        Self {
            depth,
            current: None,
            spans: Vec::new(),
        }
    }

    /// End whatever is placed, if anything, at `frame`.
    fn close(&mut self, frame: u32) {
        if let Some(placed) = self.current.take() {
            // A placement replaced on the same frame it began never showed;
            // dropping it avoids a zero-length keyframe.
            if frame > placed.since {
                self.spans.push((placed.since, frame, placed));
            }
        }
    }

    /// Turn the history into a layer, or nothing if it never showed anything.
    fn into_layer(
        self,
        dictionary: &HashMap<CharacterId, SymbolId>,
        ids: &mut IdSource,
        report: &mut ImportReport,
    ) -> Option<Layer> {
        if self.spans.is_empty() {
            return None;
        }

        let mut layer = Layer::normal(LayerId(ids.take()), format!("Depth {}", self.depth));
        let mut keyframes: Vec<buzz_scene::Keyframe> = Vec::new();
        let mut end = 0u32;

        for (start, finish, placed) in self.spans {
            // A gap since the last span needs a blank keyframe, or the
            // previous artwork would keep showing through it.
            if start > end && !keyframes.is_empty() {
                keyframes.push(buzz_scene::Keyframe {
                    start: end,
                    objects: std::sync::Arc::new(Vec::new()),
                    label: None,
                    tween: buzz_scene::Tween::default(),
                });
            }

            let objects = match dictionary.get(&placed.character) {
                Some(symbol) => {
                    let mut object = Object::instance_of(ObjectId(ids.take()), *symbol);
                    object.transform = placed.transform;
                    object.name = placed.name;
                    report.instances += 1;
                    vec![std::sync::Arc::new(object)]
                }
                None => {
                    // A placement of something never defined, or of a character
                    // this importer skipped, such as a button or a bitmap.
                    report.note_unsupported("a placement of an unsupported character");
                    Vec::new()
                }
            };

            keyframes.push(buzz_scene::Keyframe {
                start,
                objects: std::sync::Arc::new(objects),
                label: None,
                tween: buzz_scene::Tween::default(),
            });
            end = finish;
        }

        layer.frames = buzz_scene::LayerTimeline::from_parts(keyframes, end.max(1));
        Some(layer)
    }
}

/// Wrap a shape's paths in a Graphic symbol, as Animate's own SWF import does.
fn build_shape_symbol(id: CharacterId, paths: &[StyledPath], ids: &mut IdSource) -> Symbol {
    let mut symbol = Symbol::new(
        SymbolId(ids.take()),
        format!("Shape {id}"),
        SymbolKind::Graphic,
    );

    let objects: Vec<std::sync::Arc<Object>> = paths
        .iter()
        .map(|styled| {
            let shape = ShapeData {
                path: styled.path.clone(),
                fill: styled.fill.map(FillSpec::solid),
                stroke: styled
                    .stroke
                    .map(|(color, width)| StrokeSpec::new(color, width)),
                blend: buzz_scene::PaintBlend::Normal,
            };
            std::sync::Arc::new(Object::shape(ObjectId(ids.take()), shape))
        })
        .collect();

    let mut layer = Layer::normal(LayerId(ids.take()), "Layer_1");
    layer.frames = buzz_scene::LayerTimeline::from_parts(
        vec![buzz_scene::Keyframe {
            start: 0,
            objects: std::sync::Arc::new(objects),
            label: None,
            tween: buzz_scene::Tween::default(),
        }],
        1,
    );
    symbol.layers.push_front(layer);
    symbol
}

/// SWF's matrix, in twips, as a document transform in pixels.
fn to_affine(m: swf::Matrix) -> Affine {
    Affine::new([
        m.a.to_f64(),
        m.b.to_f64(),
        m.c.to_f64(),
        m.d.to_f64(),
        m.tx.to_pixels(),
        m.ty.to_pixels(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use swf::{
        Compression, Fixed8, Header, PlaceObject, PlaceObjectAction, Rectangle, ShapeFlag,
        ShapeStyles, Twips,
    };

    fn rect(width: f64, height: f64) -> Rectangle<Twips> {
        Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(width),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(height),
        }
    }

    /// A red square, as a DefineShape.
    fn square(id: CharacterId, size: f64) -> Tag<'static> {
        let styles = ShapeStyles {
            fill_styles: vec![swf::FillStyle::Color(swf::Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            })],
            line_styles: vec![],
        };
        let d = |dx: f64, dy: f64| swf::PointDelta::new(Twips::from_pixels(dx), Twips::from_pixels(dy));
        Tag::DefineShape(swf::Shape {
            version: 1,
            id,
            shape_bounds: rect(size, size),
            edge_bounds: rect(size, size),
            flags: ShapeFlag::empty(),
            styles,
            shape: vec![
                swf::ShapeRecord::StyleChange(Box::new(swf::StyleChangeData {
                    move_to: Some(swf::Point::new(Twips::ZERO, Twips::ZERO)),
                    fill_style_0: None,
                    fill_style_1: Some(1),
                    line_style: None,
                    new_styles: None,
                })),
                swf::ShapeRecord::StraightEdge { delta: d(size, 0.0) },
                swf::ShapeRecord::StraightEdge { delta: d(0.0, size) },
                swf::ShapeRecord::StraightEdge { delta: d(-size, 0.0) },
                swf::ShapeRecord::StraightEdge { delta: d(0.0, -size) },
            ],
        })
    }

    fn place(depth: u16, id: CharacterId, x: f64) -> Tag<'static> {
        Tag::PlaceObject(Box::new(PlaceObject {
            version: 2,
            action: PlaceObjectAction::Place(id),
            depth,
            matrix: Some(swf::Matrix::translate(Twips::from_pixels(x), Twips::ZERO)),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            class_name: None,
            filters: None,
            background_color: None,
            blend_mode: None,
            clip_actions: None,
            has_image: false,
            is_bitmap_cached: None,
            is_visible: None,
            amf_data: None,
        }))
    }

    /// Write a real SWF so the whole path — decompress, parse, convert — is
    /// exercised, rather than only the half of it we control.
    fn write_swf(tags: Vec<Tag<'_>>, frames: u16) -> Vec<u8> {
        let header = Header {
            compression: Compression::None,
            version: 6,
            stage_size: rect(550.0, 400.0),
            frame_rate: Fixed8::from_f32(24.0),
            num_frames: frames,
        };
        let mut out = Vec::new();
        swf::write_swf(&header, &tags, &mut out).expect("the fixture writes");
        out
    }

    #[test]
    fn a_shape_becomes_a_library_symbol() {
        let bytes = write_swf(vec![square(1, 20.0), Tag::ShowFrame], 1);
        let (scene, report) = import_bytes(&bytes).expect("the SWF parses");

        assert_eq!(report.shapes, 1);
        assert_eq!(scene.library().len(), 1);

        let symbol = scene.library().iter().next().unwrap();
        assert_eq!(symbol.kind, SymbolKind::Graphic);
        let bounds = symbol.bounds().expect("the symbol has artwork");
        assert_eq!((bounds.width(), bounds.height()), (20.0, 20.0));
    }

    #[test]
    fn the_stage_takes_its_size_frame_rate_and_background_from_the_header() {
        let bytes = write_swf(
            vec![
                Tag::SetBackgroundColor(swf::Color { r: 0x33, g: 0x66, b: 0x99, a: 255 }),
                Tag::ShowFrame,
            ],
            1,
        );
        let (scene, _) = import_bytes(&bytes).unwrap();

        assert_eq!(scene.stage().size.width, 550.0);
        assert_eq!(scene.stage().size.height, 400.0);
        assert_eq!(scene.stage().frame_rate, 24.0);
        assert_eq!(
            scene.stage().background.to_rgba8().to_u8_array()[..3],
            [0x33, 0x66, 0x99]
        );
    }

    /// The central mapping decision: SWF's display list is depth-ordered and
    /// one object deep, which is what a layer is. Flattening would lose the
    /// stacking order the movie depends on.
    #[test]
    fn each_depth_becomes_its_own_layer() {
        let bytes = write_swf(
            vec![
                square(1, 10.0),
                square(2, 10.0),
                place(1, 1, 0.0),
                place(2, 2, 50.0),
                Tag::ShowFrame,
            ],
            1,
        );
        let (scene, report) = import_bytes(&bytes).unwrap();

        assert_eq!(report.layers, 2, "two depths, two layers");
        assert_eq!(report.instances, 2);
        assert_eq!(scene.stage_layers().len(), 2);
    }

    /// SWF paints low depths first, so depth 2 is *in front of* depth 1. Our
    /// layer stack is front-first, so the order has to reverse on the way in.
    #[test]
    fn a_higher_depth_ends_up_in_front() {
        let bytes = write_swf(
            vec![
                square(1, 10.0),
                square(2, 10.0),
                place(1, 1, 0.0),
                place(2, 2, 0.0),
                Tag::ShowFrame,
            ],
            1,
        );
        let (scene, _) = import_bytes(&bytes).unwrap();

        let names: Vec<&str> = scene
            .stage_layers()
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Depth 2", "Depth 1"],
            "the front-most layer comes first in the stack"
        );
    }

    #[test]
    fn a_placement_carries_its_matrix() {
        let bytes = write_swf(vec![square(1, 10.0), place(3, 1, 120.0), Tag::ShowFrame], 1);
        let (scene, _) = import_bytes(&bytes).unwrap();

        let object = scene
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .next()
            .expect("something was placed");
        assert_eq!(
            object.transform.as_coeffs()[4],
            120.0,
            "the translation should survive the twips conversion"
        );
    }

    /// A sprite has a timeline of its own, so it must become a MovieClip with
    /// its own layers rather than being inlined onto the stage.
    #[test]
    fn a_sprite_becomes_a_movie_clip_with_its_own_timeline() {
        let sprite = Tag::DefineSprite(swf::Sprite {
            id: 10,
            num_frames: 2,
            tags: vec![place(1, 1, 0.0), Tag::ShowFrame, Tag::ShowFrame],
        });
        let bytes = write_swf(
            vec![square(1, 10.0), sprite, place(1, 10, 0.0), Tag::ShowFrame],
            1,
        );
        let (scene, report) = import_bytes(&bytes).unwrap();

        assert_eq!(report.sprites, 1);
        let clip = scene
            .library()
            .iter()
            .find(|s| s.kind == SymbolKind::MovieClip)
            .expect("the sprite became a movie clip");
        assert!(!clip.layers.is_empty(), "and kept its own layers");

        // And the stage holds an instance of the clip, not of the shape.
        let placed = scene
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .filter_map(|o| o.instance())
            .find(|i| i.symbol == clip.id);
        assert!(placed.is_some(), "the sprite is instanced on the stage");
    }

    /// Removing an object ends its span; the artwork must not linger for the
    /// rest of the movie.
    #[test]
    fn removing_an_object_ends_its_span() {
        let bytes = write_swf(
            vec![
                square(1, 10.0),
                place(1, 1, 0.0),
                Tag::ShowFrame,
                Tag::ShowFrame,
                Tag::RemoveObject(swf::RemoveObject {
                    depth: 1,
                    character_id: None,
                }),
                Tag::ShowFrame,
                Tag::ShowFrame,
            ],
            4,
        );
        let (scene, _) = import_bytes(&bytes).unwrap();

        let layer = scene.stage_layers().iter().next().expect("one layer");
        assert!(
            !layer.objects_at(0).is_empty(),
            "the object shows on frame 0"
        );
        assert!(
            layer.objects_at(3).is_empty(),
            "and is gone after it is removed"
        );
    }

    /// Moving an object mid-movie is a new keyframe, which is what makes the
    /// import animate rather than sit still.
    #[test]
    fn moving_an_object_creates_a_second_keyframe() {
        let modify = Tag::PlaceObject(Box::new(PlaceObject {
            version: 2,
            action: PlaceObjectAction::Modify,
            depth: 1,
            matrix: Some(swf::Matrix::translate(
                Twips::from_pixels(200.0),
                Twips::ZERO,
            )),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            class_name: None,
            filters: None,
            background_color: None,
            blend_mode: None,
            clip_actions: None,
            has_image: false,
            is_bitmap_cached: None,
            is_visible: None,
            amf_data: None,
        }));

        let bytes = write_swf(
            vec![
                square(1, 10.0),
                place(1, 1, 0.0),
                Tag::ShowFrame,
                modify,
                Tag::ShowFrame,
            ],
            2,
        );
        let (scene, _) = import_bytes(&bytes).unwrap();

        let layer = scene.stage_layers().iter().next().unwrap();
        assert_eq!(layer.frames.keyframe_count(), 2, "the move is a keyframe");

        let first = layer.objects_at(0)[0].transform.as_coeffs()[4];
        let second = layer.objects_at(1)[0].transform.as_coeffs()[4];
        assert_eq!((first, second), (0.0, 200.0));
    }

    #[test]
    fn actionscript_and_sound_are_reported_rather_than_dropped_silently() {
        let bytes = write_swf(
            vec![
                Tag::DoAction(&[0x00]),
                Tag::ShowFrame,
            ],
            1,
        );
        let (_, report) = import_bytes(&bytes).unwrap();

        assert!(!report.is_complete());
        assert!(
            report.unsupported.iter().any(|u| u.contains("ActionScript")),
            "{:?}",
            report.unsupported
        );
    }

    #[test]
    fn rubbish_is_refused_rather_than_panicking() {
        for bytes in [
            b"not an swf".as_slice(),
            b"FWS".as_slice(),
            &[0xFF; 128],
            &[],
        ] {
            let _ = import_bytes(bytes);
        }
    }

    /// Ids handed out by the importer must not be reissued by later editing.
    #[test]
    fn the_scene_does_not_reuse_imported_ids() {
        let bytes = write_swf(vec![square(1, 10.0), place(1, 1, 0.0), Tag::ShowFrame], 1);
        let (mut scene, _) = import_bytes(&bytes).unwrap();

        let used: std::collections::BTreeSet<u64> =
            scene.library().iter().map(|s| s.id.0).collect();
        let fresh = scene.add_symbol("Afterwards", SymbolKind::Graphic, None);
        assert!(!used.contains(&fresh.0));
    }
}
