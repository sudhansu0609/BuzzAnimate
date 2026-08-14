//! Rigged artwork in the document: armatures and warps.
//!
//! # An armature is an object, not a layer
//!
//! **This is a deliberate deviation from Animate and the reason is worth
//! stating.** Animate moves rigged artwork onto an *armature layer* whose
//! keyframes are poses. Here an armature is an [`ObjectKind`], so it sits on
//! an ordinary layer alongside everything else.
//!
//! What that buys is everything the document already does: keyframes hold
//! armatures the way they hold any object, so a pose per keyframe comes free;
//! tweens, undo, grouping, the library, symbol nesting, importing and id
//! remapping all work on it without a second code path. An armature *layer*
//! would need its own rules in each of those places, and each of those rules
//! would be a place to forget.
//!
//! What it costs is that the timeline does not mark a rigged layer as an
//! armature layer, and nothing stops you drawing on it. That is recorded as a
//! gap rather than pretended away.
//!
//! [`ObjectKind`]: crate::ObjectKind

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _};
use buzz_rig::{Armature, SkinBinding, WarpHandle};

use crate::object::{Object, ShapeData};

/// How one piece of artwork follows the bones.
#[derive(Debug, Clone, PartialEq)]
pub enum RigBinding {
    /// **Skinned.** Every point of the path is weighted across nearby bones,
    /// so the artwork bends at the joints. This is what rigging a drawn shape
    /// means.
    Skin(SkinBinding),
    /// **Rigid.** The whole part moves with one bone, unbent. This is what
    /// rigging a chain of symbols means — a forearm drawn as its own symbol
    /// should turn about the elbow, not deform.
    Rigid(usize),
}

/// One piece of artwork attached to an armature.
#[derive(Debug, Clone, PartialEq)]
pub struct RigPart {
    /// The artwork as drawn, in the armature's own coordinates.
    pub artwork: Arc<Object>,
    pub binding: RigBinding,
}

/// A skeleton with artwork attached.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmatureData {
    pub armature: Armature,
    pub parts: Vec<RigPart>,
}

impl ArmatureData {
    pub fn new(armature: Armature) -> Self {
        Self {
            armature,
            parts: Vec::new(),
        }
    }

    /// Attach a shape, skinned to the bones as they are now.
    ///
    /// The armature's current pose is taken as the rest pose for weighting,
    /// which is what "bind" means: the artwork was drawn to match the
    /// skeleton, and that pairing is what every later pose is measured from.
    pub fn bind_shape(&mut self, artwork: Arc<Object>) {
        let binding = match &artwork.kind {
            crate::ObjectKind::Shape(shape) => {
                RigBinding::Skin(buzz_rig::bind_path(&shape.path, &self.armature))
            }
            // Anything that is not a single path — a group, an instance —
            // cannot be skinned point by point, so it rides on its nearest
            // bone instead. Refusing would leave the user with a rig that
            // ignores half their artwork and no explanation.
            _ => RigBinding::Rigid(self.nearest_bone_to(&artwork)),
        };
        self.parts.push(RigPart { artwork, binding });
    }

    /// Attach artwork rigidly to one bone.
    pub fn bind_rigid(&mut self, artwork: Arc<Object>, bone: usize) {
        let bone = bone.min(self.armature.len().saturating_sub(1));
        self.parts.push(RigPart {
            artwork,
            binding: RigBinding::Rigid(bone),
        });
    }

    fn nearest_bone_to(&self, artwork: &Object) -> usize {
        let centre = artwork.bounds().center();
        self.armature
            .nearest_bone(centre)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    /// The artwork as it looks in the current pose.
    ///
    /// Rebuilt on demand rather than stored, for the same reason a tweened
    /// frame is: a pose is a handful of angles, and keeping a deformed copy in
    /// the document would mean two things that can disagree — and the stored
    /// one would be the one that got saved.
    pub fn posed(&self) -> Vec<Arc<Object>> {
        self.parts
            .iter()
            .map(|part| Arc::new(self.pose_part(part)))
            .collect()
    }

    fn pose_part(&self, part: &RigPart) -> Object {
        let mut object = (*part.artwork).clone();
        match &part.binding {
            RigBinding::Skin(binding) => {
                if let crate::ObjectKind::Shape(shape) = &mut object.kind {
                    shape.path = buzz_rig::deform_path(&shape.path, binding, &self.armature);
                }
            }
            RigBinding::Rigid(bone) => {
                // Applied on the left, so the bone's motion happens in the
                // armature's space and the part's own placement is preserved
                // underneath it.
                object.transform = self.armature.pose_transform(*bone) * object.transform;
            }
        }
        object
    }

    /// Re-weight every skinned part against the pose the bones are in now.
    ///
    /// Called when the skeleton changes — a bone added, moved or removed —
    /// because weights computed against the old rest pose would attach artwork
    /// to bones that are no longer where they were.
    pub fn rebind(&mut self) {
        let armature = self.armature.clone();
        for part in &mut self.parts {
            if let RigBinding::Skin(_) = part.binding
                && let crate::ObjectKind::Shape(shape) = &part.artwork.kind
            {
                part.binding = RigBinding::Skin(buzz_rig::bind_path(&shape.path, &armature));
            }
        }
    }

    /// Bounds of the posed artwork, falling back to the bones themselves.
    ///
    /// A rig with no artwork yet — which is what it looks like for the first
    /// few seconds of building one — still has an extent, or it could not be
    /// selected or scrolled to.
    pub fn local_bounds(&self) -> Rect {
        let artwork = self
            .posed()
            .iter()
            .map(|o| o.bounds())
            .reduce(|a, b| a.union(b));

        match (artwork, self.armature.bounds()) {
            (Some(art), Some(bones)) => art.union(bones),
            (Some(art), None) => art,
            (None, Some(bones)) => bones,
            (None, None) => Rect::ZERO,
        }
    }

    /// Every bone as a head-to-tip segment, for drawing and picking.
    pub fn segments(&self) -> Vec<(Point, Point)> {
        self.armature.joints()
    }
}

/// Artwork with warp handles on it — Animate's Asset Warp tool.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpData {
    /// The artwork as drawn.
    pub shape: ShapeData,
    pub handles: Vec<WarpHandle>,
    /// How local each handle's influence is.
    pub rigidity: f64,
}

impl WarpData {
    /// Make artwork warpable.
    ///
    /// The path is **subdivided** on the way in, exactly, because a warp moves
    /// points and a rectangle has four: without interior points, dragging a
    /// handle in the middle of a straight edge would move nothing and the tool
    /// would look broken. See [`buzz_rig::warp::subdivide_path`].
    pub fn new(shape: ShapeData) -> Self {
        let mut shape = shape;
        shape.path = buzz_rig::warp::subdivide_path(&shape.path, buzz_rig::warp::WARP_SUBDIVISION);
        Self {
            shape,
            handles: Vec::new(),
            rigidity: buzz_rig::warp::DEFAULT_RIGIDITY,
        }
    }

    /// Put a starting grid of handles over the artwork.
    pub fn with_grid(mut self, columns: usize, rows: usize) -> Self {
        self.handles = buzz_rig::warp::grid_handles(self.shape.path.bounding_box(), columns, rows);
        self
    }

    /// Add a handle where the user clicked.
    pub fn add_handle(&mut self, at: Point) -> usize {
        self.handles.push(WarpHandle::new(at));
        self.handles.len() - 1
    }

    /// The nearest handle to a point, and how far away it is.
    pub fn nearest_handle(&self, to: Point) -> Option<(usize, f64)> {
        self.handles
            .iter()
            .enumerate()
            .map(|(i, h)| (i, (h.current - to).hypot()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The artwork as the handles currently have it.
    pub fn warped(&self) -> ShapeData {
        let mut shape = self.shape.clone();
        shape.path = buzz_rig::warp_path(&self.shape.path, &self.handles, self.rigidity);
        shape
    }

    /// Put every handle back where it was placed.
    pub fn reset(&mut self) {
        for handle in &mut self.handles {
            handle.current = handle.rest;
        }
    }

    /// Where the handles are now, for a keyframe.
    pub fn positions(&self) -> Vec<Point> {
        self.handles.iter().map(|h| h.current).collect()
    }

    /// Adopt handle positions, as a tween does.
    pub fn set_positions(&mut self, positions: &[Point]) {
        for (handle, position) in self.handles.iter_mut().zip(positions) {
            handle.current = *position;
        }
    }

    pub fn local_bounds(&self) -> Rect {
        let mut bounds = self.warped().path.bounding_box();
        if let Some(stroke) = &self.shape.stroke
            && !stroke.hairline
        {
            let half = stroke.width * 0.5;
            bounds = bounds.inflate(half, half);
        }
        bounds
    }
}

/// Interpolate two armature poses for a tween.
pub fn tween_armature(from: &ArmatureData, to: &ArmatureData, t: f64) -> ArmatureData {
    let mut out = from.clone();
    let pose = Armature::tween_pose(&from.armature.pose(), &to.armature.pose(), t);
    out.armature.set_pose(&pose);
    out
}

/// Interpolate two sets of warp handles for a tween.
pub fn tween_warp(from: &WarpData, to: &WarpData, t: f64) -> WarpData {
    let mut out = from.clone();
    let moved: Vec<Point> = from
        .handles
        .iter()
        .zip(&to.handles)
        .map(|(a, b)| a.current.lerp(b.current, t))
        .collect();
    out.set_positions(&moved);
    out
}

/// Transform an armature's own geometry — used when the object itself is
/// scaled or moved, so the bones follow the artwork they belong to.
pub fn transform_armature(armature: &mut Armature, transform: Affine) {
    armature.root = transform * armature.root;

    // Bone lengths are scaled by the transform's scale factor. Taken from the
    // determinant so a uniform scale is exact; a non-uniform one has no single
    // right answer and the area-preserving compromise at least keeps the rig
    // proportionate rather than letting bones and artwork drift apart.
    let scale = transform.determinant().abs().sqrt();
    if (scale - 1.0).abs() > f64::EPSILON {
        for bone in &mut armature.bones {
            bone.length *= scale;
        }
    }
}

/// A path through the deformation, for hit-testing: what the user sees is what
/// they can click.
pub fn posed_paths(kind: &crate::ObjectKind) -> Option<Vec<BezPath>> {
    match kind {
        crate::ObjectKind::Armature(data) => {
            let mut paths = Vec::new();
            for object in data.posed() {
                let mut flat = Vec::new();
                object.flatten(Affine::IDENTITY, &mut flat);
                paths.extend(flat.into_iter().map(|(t, shape)| t * shape.path));
            }
            Some(paths)
        }
        crate::ObjectKind::Warp(data) => Some(vec![data.warped().path]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, ObjectKind};
    use buzz_rig::Bone;
    use peniko::Color;
    use std::f64::consts::FRAC_PI_2;

    fn arm() -> Armature {
        let mut armature = Armature::new(Point::ZERO);
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(Bone::new("fore", Some(0), 50.0, 0.0));
        armature
    }

    fn limb() -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(1),
            ShapeData::filled(
                Rect::new(0.0, -10.0, 100.0, 10.0).to_path(1e-9),
                Color::WHITE,
            ),
        ))
    }

    #[test]
    fn binding_a_shape_skins_it() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());

        assert_eq!(data.parts.len(), 1);
        assert!(matches!(data.parts[0].binding, RigBinding::Skin(_)));
    }

    /// A group or an instance cannot be skinned point by point, so it rides on
    /// a bone instead of being refused.
    #[test]
    fn binding_a_group_attaches_it_rigidly() {
        let mut data = ArmatureData::new(arm());
        let group = Arc::new(Object::group(ObjectId(2), vec![limb()]));
        data.bind_shape(group);

        assert!(matches!(data.parts[0].binding, RigBinding::Rigid(_)));
    }

    #[test]
    fn artwork_at_rest_is_where_it_was_drawn() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());

        let posed = data.posed();
        assert_eq!(posed.len(), 1);
        let ObjectKind::Shape(shape) = &posed[0].kind else {
            panic!("expected a shape");
        };
        let original = Rect::new(0.0, -10.0, 100.0, 10.0);
        let bounds = shape.path.bounding_box();
        assert!((bounds.x1 - original.x1).abs() < 1e-6, "{bounds:?}");
    }

    #[test]
    fn posing_the_armature_deforms_the_artwork() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());
        data.armature.bones[1].angle = FRAC_PI_2;

        let posed = data.posed();
        let ObjectKind::Shape(shape) = &posed[0].kind else {
            panic!("expected a shape");
        };
        assert!(
            shape.path.bounding_box().y1 > 40.0,
            "the forearm should have swung down: {:?}",
            shape.path.bounding_box()
        );
    }

    #[test]
    fn a_rigid_part_turns_with_its_bone_without_deforming() {
        let mut data = ArmatureData::new(arm());
        data.bind_rigid(limb(), 1);
        data.armature.bones[1].angle = FRAC_PI_2;

        let posed = data.posed();
        let ObjectKind::Shape(shape) = &posed[0].kind else {
            panic!("expected a shape");
        };
        // The geometry is untouched; only the transform moved.
        assert_eq!(
            shape.path.bounding_box(),
            Rect::new(0.0, -10.0, 100.0, 10.0)
        );
        assert_ne!(posed[0].transform, Affine::IDENTITY);
    }

    #[test]
    fn bounds_cover_the_bones_even_with_no_artwork() {
        let data = ArmatureData::new(arm());
        let bounds = data.local_bounds();
        assert!(bounds.width() >= 100.0, "{bounds:?}");
    }

    #[test]
    fn tweening_a_pose_lands_between_the_two() {
        let mut from = ArmatureData::new(arm());
        from.bind_shape(limb());
        let mut to = from.clone();
        to.armature.bones[1].angle = 1.0;

        let half = tween_armature(&from, &to, 0.5);
        assert!((half.armature.bones[1].angle - 0.5).abs() < 1e-9);

        let end = tween_armature(&from, &to, 1.0);
        assert!((end.armature.bones[1].angle - 1.0).abs() < 1e-9);
    }

    /// Making artwork warpable subdivides it — more points, same shape. So
    /// this compares the *geometry*, not the path string: an identical string
    /// would mean subdivision had silently stopped happening, and the middle
    /// of a straight edge would go back to being unbendable.
    #[test]
    fn a_warp_at_rest_is_the_artwork_as_drawn() {
        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9),
            Color::WHITE,
        );
        let warp = WarpData::new(shape.clone()).with_grid(3, 3);

        assert_eq!(warp.handles.len(), 9);

        let drawn = shape.path.bounding_box();
        let at_rest = warp.warped().path.bounding_box();
        assert!((drawn.x0 - at_rest.x0).abs() < 1e-9);
        assert!((drawn.y0 - at_rest.y0).abs() < 1e-9);
        assert!((drawn.x1 - at_rest.x1).abs() < 1e-9);
        assert!((drawn.y1 - at_rest.y1).abs() < 1e-9);
        assert!((shape.path.area() - warp.warped().path.area()).abs() < 1e-6);

        assert!(
            warp.shape.path.elements().len() > shape.path.elements().len(),
            "the artwork should have been subdivided so it can bend"
        );
    }

    #[test]
    fn dragging_a_warp_handle_moves_the_artwork() {
        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9),
            Color::WHITE,
        );
        let mut warp = WarpData::new(shape).with_grid(3, 3);
        warp.handles[0].current = Point::new(-50.0, -50.0);

        assert!(warp.warped().path.bounding_box().x0 < -10.0);

        warp.reset();
        assert!(
            warp.warped().path.bounding_box().x0.abs() < 1e-9,
            "reset failed"
        );
    }

    #[test]
    fn tweening_a_warp_moves_its_handles_between_the_two() {
        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9),
            Color::WHITE,
        );
        let from = WarpData::new(shape).with_grid(2, 2);
        let mut to = from.clone();
        to.handles[0].current = Point::new(-100.0, 0.0);

        let half = tween_warp(&from, &to, 0.5);
        assert!((half.handles[0].current.x - -50.0).abs() < 1e-9);
    }

    /// Scaling the object must carry the bones with the artwork, or the rig
    /// would be left behind at its old size.
    #[test]
    fn transforming_an_armature_scales_its_bones() {
        let mut armature = arm();
        transform_armature(&mut armature, Affine::scale(2.0));

        assert!((armature.bones[0].length - 100.0).abs() < 1e-9);
        assert!((armature.tip(1) - Point::new(200.0, 0.0)).hypot() < 1e-9);
    }

    #[test]
    fn rebinding_follows_a_changed_skeleton() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());
        let before = data.parts[0].binding.clone();

        // The rig is redrawn somewhere else entirely.
        data.armature = Armature::new(Point::new(0.0, 500.0));
        data.armature.push(Bone::new("new", None, 50.0, 0.0));
        data.rebind();

        assert_ne!(
            data.parts[0].binding, before,
            "the weights should have followed the new skeleton"
        );
    }
}
