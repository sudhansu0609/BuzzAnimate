//! Serialisable mirror of [`buzz_scene::Scene`].
//!
//! # Why a separate set of types
//!
//! Deriving `Serialize` directly on the runtime model would weld the file
//! format to internal struct layout: renaming a field or reordering an enum
//! would silently change the format and break every saved document. These DTOs
//! are a deliberate seam. The runtime model is free to evolve; the format
//! changes only when [`FORMAT_VERSION`] says so.
//!
//! # Two representation choices worth stating
//!
//! * **Paths are SVG strings.** Serialising `PathEl` as JSON enums is verbose —
//!   a 200-segment path becomes a wall of objects. `to_svg`/`from_svg` is
//!   compact, diff-friendly, and matches what SVG and XFL already do. It is
//!   also *lossless*: kurbo formats coordinates with Rust's `Display` for
//!   `f64`, which emits the shortest string that round-trips exactly. A test
//!   pins that down for extreme values.
//! * **Colours are `#RRGGBBAA` strings.** peniko can serialise `Color` itself,
//!   but that ties the format to peniko's internal representation across
//!   versions. Hex is stable, readable, and what every other vector format
//!   uses.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, FillMode, Size};
use buzz_scene::{
    FillSpec, Layer, LayerHeight, LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene,
    ShapeData, StageProperties, StrokeSpec,
};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// Bumped only for a breaking change to the on-disk layout.
pub const FORMAT_VERSION: u32 = 1;

/// Anything that can go wrong converting to or from the document model.
#[derive(Debug, thiserror::Error)]
pub enum SerialError {
    #[error("unsupported document version {found}; this build reads up to {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("could not parse path data: {0}")]
    BadPath(String),
    #[error("could not parse colour {0:?}; expected #RRGGBB or #RRGGBBAA")]
    BadColor(String),
}

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

fn color_to_hex(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

fn color_from_hex(s: &str) -> Result<Color, SerialError> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| SerialError::BadColor(s.to_string()))
    };
    match hex.len() {
        6 => Ok(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Ok(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => Err(SerialError::BadColor(s.to_string())),
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentDto {
    pub format_version: u32,
    pub stage: StageDto,
    /// Front to back, as in the timeline.
    pub layers: Vec<LayerDto>,
    /// Highest id in use, so the allocator can resume safely.
    pub max_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDto {
    pub width: f64,
    pub height: f64,
    pub background: String,
    pub frame_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDto {
    pub id: u64,
    pub name: String,
    pub kind: LayerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    pub visible: bool,
    pub locked: bool,
    #[serde(default)]
    pub outline: bool,
    pub color: String,
    #[serde(default)]
    pub height: LayerHeight,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDto {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Affine coefficients `[a, b, c, d, e, f]`.
    #[serde(default = "identity_coeffs")]
    pub transform: [f64; 6],
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    pub kind: ObjectKindDto,
}

fn identity_coeffs() -> [f64; 6] {
    Affine::IDENTITY.as_coeffs()
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ObjectKindDto {
    Shape {
        /// SVG path data.
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<StrokeDto>,
    },
    Group {
        children: Vec<ObjectDto>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillDto {
    pub color: String,
    #[serde(default)]
    pub rule: FillMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrokeDto {
    pub color: String,
    pub width: f64,
    #[serde(default)]
    pub hairline: bool,
}

// ---------------------------------------------------------------------------
// Scene -> DTO
// ---------------------------------------------------------------------------

impl DocumentDto {
    pub fn from_scene(scene: &Scene) -> Self {
        let mut max_id = 0u64;
        let layers: Vec<LayerDto> = scene
            .layers()
            .iter()
            .map(|layer| {
                max_id = max_id.max(layer.id.0);
                LayerDto {
                    id: layer.id.0,
                    name: layer.name.clone(),
                    kind: layer.kind,
                    parent: layer.parent.map(|p| p.0),
                    visible: layer.visible,
                    locked: layer.locked,
                    outline: layer.outline,
                    color: color_to_hex(layer.color),
                    height: layer.height,
                    collapsed: layer.collapsed,
                    objects: layer
                        .objects
                        .iter()
                        .map(|o| ObjectDto::from_object(o, &mut max_id))
                        .collect(),
                }
            })
            .collect();

        Self {
            format_version: FORMAT_VERSION,
            stage: StageDto {
                width: scene.stage.size.width,
                height: scene.stage.size.height,
                background: color_to_hex(scene.stage.background),
                frame_rate: scene.stage.frame_rate,
            },
            layers,
            max_id,
        }
    }

    /// Rebuild a scene. Fails only on genuinely malformed data.
    pub fn to_scene(&self) -> Result<Scene, SerialError> {
        if self.format_version > FORMAT_VERSION {
            return Err(SerialError::UnsupportedVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }

        let mut scene = Scene::empty();
        scene.stage = StageProperties {
            size: Size::new(self.stage.width, self.stage.height),
            background: color_from_hex(&self.stage.background)?,
            frame_rate: self.stage.frame_rate,
        };

        for (index, dto) in self.layers.iter().enumerate() {
            let mut layer = Layer::new(LayerId(dto.id), dto.name.clone(), dto.kind);
            layer.parent = dto.parent.map(LayerId);
            layer.visible = dto.visible;
            layer.locked = dto.locked;
            layer.outline = dto.outline;
            layer.color = color_from_hex(&dto.color)?;
            layer.height = dto.height;
            layer.collapsed = dto.collapsed;
            for object in &dto.objects {
                layer.push_object(Arc::new(object.to_object()?));
            }
            scene.edit_layers().insert(index, layer);
        }

        // Raise the allocator past everything the file already uses, so a new
        // object cannot collide with a loaded one.
        scene.reserve_ids_above(self.max_id);
        Ok(scene)
    }
}

impl ObjectDto {
    fn from_object(object: &Object, max_id: &mut u64) -> Self {
        *max_id = (*max_id).max(object.id.0);
        let kind = match &object.kind {
            ObjectKind::Shape(s) => ObjectKindDto::Shape {
                path: s.path.to_svg(),
                fill: s.fill.map(|f| FillDto {
                    color: color_to_hex(f.color),
                    rule: f.rule,
                }),
                stroke: s.stroke.map(|s| StrokeDto {
                    color: color_to_hex(s.color),
                    width: s.width,
                    hairline: s.hairline,
                }),
            },
            ObjectKind::Group(children) => ObjectKindDto::Group {
                children: children
                    .iter()
                    .map(|c| Self::from_object(c, max_id))
                    .collect(),
            },
        };

        Self {
            id: object.id.0,
            name: object.name.clone(),
            transform: object.transform.as_coeffs(),
            visible: object.visible,
            locked: object.locked,
            kind,
        }
    }

    fn to_object(&self) -> Result<Object, SerialError> {
        let kind = match &self.kind {
            ObjectKindDto::Shape { path, fill, stroke } => {
                let parsed =
                    BezPath::from_svg(path).map_err(|e| SerialError::BadPath(e.to_string()))?;
                ObjectKind::Shape(ShapeData {
                    path: parsed,
                    fill: fill
                        .as_ref()
                        .map(|f| {
                            Ok::<_, SerialError>(FillSpec {
                                color: color_from_hex(&f.color)?,
                                rule: f.rule,
                            })
                        })
                        .transpose()?,
                    stroke: stroke
                        .as_ref()
                        .map(|s| {
                            Ok::<_, SerialError>(StrokeSpec {
                                color: color_from_hex(&s.color)?,
                                width: s.width,
                                hairline: s.hairline,
                            })
                        })
                        .transpose()?,
                })
            }
            ObjectKindDto::Group { children } => ObjectKind::Group(
                children
                    .iter()
                    .map(|c| c.to_object().map(Arc::new))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };

        Ok(Object {
            id: ObjectId(self.id),
            name: self.name.clone(),
            transform: Affine::new(self.transform),
            kind,
            locked: self.locked,
            visible: self.visible,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Point, Shape as _};
    use kurbo::{Circle, Rect};

    fn sample_scene() -> Scene {
        let mut scene = Scene::empty();
        let base = scene.add_layer("Background", LayerKind::Normal);
        let art = scene.add_layer("Artwork", LayerKind::Normal);

        scene.add_shape(
            base,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9),
                Color::from_rgb8(0x22, 0x44, 0x88),
            ),
        );
        scene.add_shape(
            art,
            ShapeData::stroked(
                // A realistic authoring tolerance. kurbo grows a circle's
                // segment count from `radius / tolerance`, so a stress value
                // like 1e-9 would yield ~60 cubics where a drawing tool
                // produces 4.
                Circle::new(Point::new(100.0, 100.0), 40.0).to_path(0.05),
                Color::from_rgb8(0xFF, 0x00, 0x66),
                2.5,
            ),
        );
        scene.update_layer(art, |l| {
            l.locked = true;
            l.outline = true;
            l.height = LayerHeight::Double;
        });
        scene
    }

    #[test]
    fn colours_round_trip_through_hex() {
        for c in [
            Color::WHITE,
            Color::BLACK,
            Color::from_rgb8(0x12, 0x34, 0x56),
            Color::from_rgba8(0xAB, 0xCD, 0xEF, 0x80),
        ] {
            let hex = color_to_hex(c);
            let back = color_from_hex(&hex).unwrap();
            assert_eq!(
                c.to_rgba8().to_u8_array(),
                back.to_rgba8().to_u8_array(),
                "colour changed through {hex}"
            );
        }
    }

    #[test]
    fn opaque_colours_are_written_without_an_alpha_byte() {
        assert_eq!(color_to_hex(Color::from_rgb8(0x12, 0x34, 0x56)), "#123456");
        assert_eq!(
            color_to_hex(Color::from_rgba8(0x12, 0x34, 0x56, 0x7F)),
            "#1234567F"
        );
    }

    #[test]
    fn malformed_colours_are_rejected_clearly() {
        for bad in ["", "#12", "#12345", "nonsense", "#GGGGGG"] {
            assert!(
                color_from_hex(bad).is_err(),
                "{bad:?} should not have parsed"
            );
        }
        // Both with and without the hash are accepted.
        assert!(color_from_hex("123456").is_ok());
    }

    /// Paths must survive the round trip *exactly*, including the extreme
    /// coordinates the unbounded-zoom design produces.
    #[test]
    fn path_round_trip_is_lossless_even_at_extreme_scales() {
        let mut path = BezPath::new();
        path.move_to(Point::new(1234.5678901234567, -987.6543210987654));
        path.line_to(Point::new(1e-9, 1e9));
        path.curve_to(
            Point::new(0.1, 0.2),
            Point::new(1e-12, 3.0),
            Point::new(1e6 + 1e-7, 2.5),
        );
        path.quad_to(Point::new(5.5, 6.5), Point::new(7.25, 8.125));
        path.close_path();

        let svg = path.to_svg();
        let back = BezPath::from_svg(&svg).expect("our own output must parse");

        assert_eq!(
            path.elements(),
            back.elements(),
            "path changed through SVG round trip:\n{svg}"
        );
    }

    #[test]
    fn a_scene_survives_a_full_round_trip() {
        let scene = sample_scene();
        let dto = DocumentDto::from_scene(&scene);
        let back = dto.to_scene().unwrap();

        assert_eq!(back.stage.size, scene.stage.size);
        assert_eq!(back.stage.frame_rate, scene.stage.frame_rate);
        assert_eq!(back.layers().len(), scene.layers().len());
        assert_eq!(back.shape_count(), scene.shape_count());

        for (a, b) in scene.layers().iter().zip(back.layers().iter()) {
            assert_eq!(a.id, b.id, "layer ids must be preserved");
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.locked, b.locked);
            assert_eq!(a.outline, b.outline);
            assert_eq!(a.height, b.height);
            assert_eq!(a.objects.len(), b.objects.len());
        }
    }

    #[test]
    fn layer_order_is_preserved() {
        let mut scene = Scene::empty();
        for i in 0..5 {
            scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
        }
        let before: Vec<String> = scene.layers().iter().map(|l| l.name.clone()).collect();

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let after: Vec<String> = back.layers().iter().map(|l| l.name.clone()).collect();

        assert_eq!(before, after, "front-to-back order must survive");
    }

    #[test]
    fn nested_groups_and_transforms_survive() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);

        let leaf = Arc::new(
            Object::shape(
                ObjectId(90),
                ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            )
            .with_transform(Affine::translate((3.0, 4.0))),
        );
        let inner = Arc::new(Object::group(ObjectId(91), vec![leaf]));
        let outer = Object::group(ObjectId(92), vec![inner])
            .with_transform(Affine::scale(2.0))
            .with_name("nest");
        scene.add_object(layer, outer).unwrap();

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, restored) = back.find_object(ObjectId(92)).unwrap();

        assert_eq!(restored.name.as_deref(), Some("nest"));
        assert_eq!(restored.shape_count(), 1);
        assert_eq!(
            restored.transform.as_coeffs(),
            Affine::scale(2.0).as_coeffs()
        );
        assert_eq!(restored.bounds(), scene.find_object(ObjectId(92)).unwrap().1.bounds());
    }

    #[test]
    fn ids_are_reserved_after_loading() {
        let scene = sample_scene();
        let mut back = DocumentDto::from_scene(&scene).to_scene().unwrap();

        let existing: Vec<u64> = back
            .layers()
            .iter()
            .flat_map(|l| l.objects.iter().map(|o| o.id.0))
            .chain(back.layers().iter().map(|l| l.id.0))
            .collect();
        let fresh = back.next_object_id();

        assert!(
            !existing.contains(&fresh.0),
            "new id {} collided with one already in the file",
            fresh.0
        );
    }

    #[test]
    fn a_future_format_version_is_refused_rather_than_misread() {
        let mut dto = DocumentDto::from_scene(&sample_scene());
        dto.format_version = FORMAT_VERSION + 5;
        assert!(matches!(
            dto.to_scene(),
            Err(SerialError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn corrupt_path_data_produces_an_error_not_a_panic() {
        let mut dto = DocumentDto::from_scene(&sample_scene());
        if let Some(layer) = dto.layers.iter_mut().find(|l| !l.objects.is_empty())
            && let ObjectKindDto::Shape { path, .. } = &mut layer.objects[0].kind
        {
            *path = "this is not a path".into();
        }
        assert!(matches!(dto.to_scene(), Err(SerialError::BadPath(_))));
    }

    #[test]
    fn json_is_stable_and_reasonably_compact() {
        let scene = sample_scene();
        let dto = DocumentDto::from_scene(&scene);

        let a = serde_json::to_string(&dto).unwrap();
        let b = serde_json::to_string(&DocumentDto::from_scene(&scene)).unwrap();
        assert_eq!(a, b, "serialising twice must produce identical bytes");

        // Round-trip through JSON, not just through the DTO.
        let parsed: DocumentDto = serde_json::from_str(&a).unwrap();
        assert_eq!(parsed.to_scene().unwrap().shape_count(), scene.shape_count());

        // Cost is dominated by path data, so measure per segment rather than
        // in absolute bytes — the latter just tracks how detailed the test
        // artwork happens to be.
        //
        // Coordinates are written at full `f64` precision, which is verbose
        // (`1234.5678901234567` is 18 characters). That is a deliberate
        // trade: losslessness matters more than JSON size, and the container's
        // deflate absorbs most of it. Storing `PathEl` enums as JSON objects
        // would be several times worse *and* no more accurate.
        let segments: usize = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects.iter())
            .map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => s.path.elements().len(),
                buzz_scene::ObjectKind::Group(_) => 0,
            })
            .sum();
        let per_segment = a.len() / segments.max(1);
        assert!(
            per_segment < 200,
            "{} bytes for {segments} segments ({per_segment} each) is too verbose",
            a.len()
        );
    }

    #[test]
    fn optional_fields_may_be_absent_from_json() {
        // A minimal hand-written document must load, so the format is
        // hand-editable and tolerant of older files.
        // `r##` rather than `r#`: the JSON contains `"#FFFFFF"`, and the `"#`
        // sequence would close a single-hash raw string early.
        let json = r##"{
            "format_version": 1,
            "stage": { "width": 550, "height": 400, "background": "#FFFFFF", "frame_rate": 24 },
            "layers": [
                { "id": 1, "name": "Layer_1", "kind": "Normal", "visible": true,
                  "locked": false, "color": "#0099FF" }
            ],
            "max_id": 1
        }"##;
        let dto: DocumentDto = serde_json::from_str(json).unwrap();
        let scene = dto.to_scene().unwrap();
        assert_eq!(scene.layers().len(), 1);
        assert_eq!(scene.shape_count(), 0);
    }
}
