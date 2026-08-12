//! Drawable objects on the stage.
//!
//! Objects are **immutable once shared**. Editing goes through
//! [`Arc::make_mut`], which clones an object only if another snapshot still
//! references it — so an edit touches the changed object and nothing else.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, FillMode, Point, Rect, Shape as _};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// Stable identity for an object, preserved across edits and undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u64);

/// How a shape is painted inside.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FillSpec {
    pub color: Color,
    pub rule: FillMode,
}

impl FillSpec {
    pub fn solid(color: Color) -> Self {
        Self {
            color,
            rule: FillMode::NonZero,
        }
    }
}

/// How a shape's outline is painted.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StrokeSpec {
    pub color: Color,
    /// Width in document units.
    pub width: f64,
    /// Animate's "hairline": always one pixel regardless of zoom.
    pub hairline: bool,
}

impl StrokeSpec {
    pub fn new(color: Color, width: f64) -> Self {
        Self {
            color,
            width,
            hairline: false,
        }
    }

    pub fn hairline(color: Color) -> Self {
        Self {
            color,
            width: 0.0,
            hairline: true,
        }
    }
}

/// A filled and/or stroked path.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeData {
    pub path: BezPath,
    pub fill: Option<FillSpec>,
    pub stroke: Option<StrokeSpec>,
}

impl ShapeData {
    pub fn filled(path: BezPath, color: Color) -> Self {
        Self {
            path,
            fill: Some(FillSpec::solid(color)),
            stroke: None,
        }
    }

    pub fn stroked(path: BezPath, color: Color, width: f64) -> Self {
        Self {
            path,
            fill: None,
            stroke: Some(StrokeSpec::new(color, width)),
        }
    }
}

/// What an object actually is.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    Shape(ShapeData),
    /// Animate's Group: children move together but stay individually editable
    /// once you enter the group.
    Group(Vec<Arc<Object>>),
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
        }
    }

    /// Bounds after this object's own transform.
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
        }
    }

    /// Number of leaf shapes, for diagnostics and progress reporting.
    pub fn shape_count(&self) -> usize {
        match &self.kind {
            ObjectKind::Shape(_) => 1,
            ObjectKind::Group(children) => children.iter().map(|c| c.shape_count()).sum(),
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
        assert!((bb.x0 - 100.0).abs() < 1e-9 && (bb.y0 - 50.0).abs() < 1e-9, "{bb:?}");
        assert!((bb.width() - 10.0).abs() < 1e-9);
    }

    /// A rotated rectangle's bounds must cover all four corners.
    #[test]
    fn rotation_expands_bounds_correctly() {
        let o = shape_at(1, -5.0, -5.0)
            .with_transform(Affine::rotate(std::f64::consts::FRAC_PI_4));
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

    #[test]
    fn hairline_strokes_do_not_inflate_bounds() {
        let o = Object::shape(
            ObjectId(1),
            ShapeData {
                path: square(0.0, 0.0, 10.0),
                fill: None,
                stroke: Some(StrokeSpec::hairline(Color::WHITE)),
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
        assert!((bb.x0 - 0.0).abs() < 1e-9 && (bb.x1 - 110.0).abs() < 1e-9, "{bb:?}");
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
