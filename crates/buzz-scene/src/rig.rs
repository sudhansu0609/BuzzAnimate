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

use std::hash::Hasher as _;
use std::sync::{Arc, Mutex};

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _};
use buzz_rig::{Armature, SkinBinding, WarpHandle};

use crate::object::{Object, ShapeData};

/// What [`ArmatureData::posed`] last built: the fingerprint of the inputs it
/// was built from, the artwork `Arc`s it was built from, and the result.
type PosedEntry = (u64, Vec<Arc<Object>>, Arc<[Arc<Object>]>);

/// A memo of the posed parts, so that the same pose gives the same `Arc`s.
///
/// # Why this exists
///
/// Every consumer that keys a cache on an object's address (the lighting
/// cache above all) used to see a brand-new `Arc<Object>` per part per call
/// and so never hit. With the memo a held pose returns the very same parts.
///
/// # Why it is not part of the rig's identity
///
/// It is derived data: `Clone` gives an **empty** memo (a clone is a new rig
/// that must prove its own parts), `PartialEq` is always true (two rigs are
/// equal by their bones and parts, not by what happens to be cached), and
/// `Debug` is opaque. Correctness never depends on invalidating it:
/// `armature` and `parts` are public and are edited in place, so
/// [`ArmatureData::posed`] re-derives a fingerprint of everything the posing
/// reads and only reuses the memo when it still matches. The memo also
/// keeps the artwork `Arc`s it was built from alive, so an artwork address
/// in the fingerprint cannot be reused by another allocation, and a
/// copy-on-write edit of an artwork (`Arc::make_mut`) must copy it, which
/// gives a new address.
#[derive(Default)]
pub struct PosedMemo(Mutex<Option<PosedEntry>>);

impl Clone for PosedMemo {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for PosedMemo {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PosedMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PosedMemo(..)")
    }
}

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
    /// Poses this character owns, by name.
    ///
    /// # Why they live on the rig and not on the symbol
    ///
    /// A pose is a list of joint angles, and it only means anything against
    /// the skeleton it was taken from — so it belongs to that skeleton. Here
    /// a rig is an object (§7 item 33), which may or may not be inside a
    /// symbol, so hanging poses off the symbol would leave every rig that is
    /// not in one unable to have any, and would break the moment a rigged
    /// object was moved out of its symbol.
    ///
    /// Kept here they travel wherever the object does: through the clipboard,
    /// into the Assets library, into another document — all of which copy
    /// objects whole.
    pub poses: Vec<NamedPose>,
    /// The rig pattern this skeleton was assembled from, if it was.
    ///
    /// # Why a name and not the pattern itself
    ///
    /// Nothing here reads the pattern back to work out how the rig behaves —
    /// the bones are the truth, and a rig whose bones have been edited by hand
    /// is still that rig. The name is for the Rigging panel: it says which
    /// slot list to show, so that a character can be opened a week later and
    /// have a redrawn arm dropped back into "Elbow L" without rebuilding the
    /// skeleton.
    ///
    /// Bone index and slot index are the same number, which is what makes that
    /// work and is why `assemble` gives every slot a bone whether or not
    /// anything was dropped on it. A name that no longer matches any pattern —
    /// a file from a later version, a pattern that was renamed — leaves the
    /// panel showing the plain bone list, which is what it showed before.
    pub pattern: Option<String>,
    /// The posed parts, memoised (see [`PosedMemo`]). Derived; never part of
    /// equality or of a clone.
    pub posed_memo: PosedMemo,
}

/// A pose worth keeping, and what to call it.
///
/// Serialised by the DTO layer in `buzz-doc`, like everything else in this
/// module — the model does not know about the file format.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedPose {
    pub name: String,
    /// One angle per bone, in bone order — the shape `Armature::pose` returns.
    ///
    /// A pose of the wrong length is applied as far as it goes rather than
    /// refused, which is `Armature::set_pose`'s rule: a rig edited after a
    /// pose was saved should lose that bone's angle, not the whole pose.
    pub angles: Vec<f64>,
}

impl ArmatureData {
    pub fn new(armature: Armature) -> Self {
        Self {
            armature,
            parts: Vec::new(),
            poses: Vec::new(),
            pattern: None,
            posed_memo: PosedMemo::default(),
        }
    }

    /// A straight chain of bones from the origin, each `lengths[i]` long and
    /// a child of the one before, all at angle zero. **A test fixture**:
    /// crates that test rigs without depending on `buzz-rig` build theirs here.
    #[doc(hidden)]
    pub fn chain(lengths: &[f64]) -> Self {
        let mut armature = Armature::new(Point::ZERO);
        for (i, length) in lengths.iter().enumerate() {
            let parent = i.checked_sub(1);
            armature.push(buzz_rig::Bone::new(format!("bone{i}"), parent, *length, 0.0));
        }
        Self::new(armature)
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
    ///
    /// **Memoised** (see [`PosedMemo`]): while the pose, the skeleton and the
    /// parts are unchanged the same `Arc`s come back, so a cache keyed on an
    /// object's address can hit across frames.
    pub fn posed(&self) -> Vec<Arc<Object>> {
        let fingerprint = self.pose_fingerprint();
        let mut memo = self
            .posed_memo
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((print, _, parts)) = memo.as_ref()
            && *print == fingerprint
        {
            return parts.to_vec();
        }
        let built: Arc<[Arc<Object>]> = self
            .parts
            .iter()
            .map(|part| Arc::new(self.pose_part(part)))
            .collect();
        let artwork = self.parts.iter().map(|p| p.artwork.clone()).collect();
        let out = built.to_vec();
        *memo = Some((fingerprint, artwork, built));
        out
    }

    /// A hash of everything [`ArmatureData::posed`] reads: the skeleton (root
    /// and every bone's parent, length, rest angle, angle and offset), and for
    /// each part its artwork's address and its binding (a skin's weights too).
    ///
    /// Cheap next to posing; also the "has this rig's pose changed" test the
    /// renderer's caches use.
    pub fn pose_fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let f = |h: &mut std::collections::hash_map::DefaultHasher, v: f64| h.write_u64(v.to_bits());
        f(&mut h, self.armature.root.x);
        f(&mut h, self.armature.root.y);
        h.write_usize(self.armature.len());
        for bone in &self.armature.bones {
            h.write_usize(bone.parent.map_or(usize::MAX, |p| p));
            f(&mut h, bone.length);
            f(&mut h, bone.rest_angle);
            f(&mut h, bone.angle);
            f(&mut h, bone.offset.x);
            f(&mut h, bone.offset.y);
        }
        h.write_usize(self.parts.len());
        for part in &self.parts {
            h.write_usize(Arc::as_ptr(&part.artwork) as usize);
            match &part.binding {
                RigBinding::Rigid(bone) => {
                    h.write_u8(0);
                    h.write_usize(*bone);
                }
                RigBinding::Skin(skin) => {
                    h.write_u8(1);
                    h.write_usize(skin.points.len());
                    for point in &skin.points {
                        h.write_usize(point.len());
                        for (bone, weight) in point {
                            h.write_usize(*bone);
                            f(&mut h, *weight);
                        }
                    }
                }
            }
        }
        h.finish()
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

    /// A tween between two legal poses stays inside the joint's limits, and a
    /// tween towards a pose written straight into the field past them (a
    /// hand-edited file, an old key) is clamped rather than followed.
    #[test]
    fn a_tween_stays_inside_the_joint_limits() {
        let mut from = ArmatureData::new(arm());
        from.armature.bones[1].limits = Some(buzz_rig::JointLimits::new(-0.5, 0.5));
        let mut to = from.clone();
        from.armature.bones[1].angle = -0.4;
        to.armature.bones[1].angle = 0.4;
        for step in 0..=20 {
            let t = step as f64 / 20.0;
            let angle = tween_armature(&from, &to, t).armature.bones[1].angle;
            assert!(
                (-0.5 - 1e-12..=0.5 + 1e-12).contains(&angle),
                "t = {t}: {angle} is outside -0.5..0.5"
            );
        }

        let mut wild = from.clone();
        wild.armature.bones[1].angle = 2.0;
        // Unclamped, half way would be 0.8 and the end 2.0.
        let half = tween_armature(&from, &wild, 0.5).armature.bones[1].angle;
        assert!((half - 0.5).abs() < 1e-9, "half way went to {half}");
        let end = tween_armature(&from, &wild, 1.0).armature.bones[1].angle;
        assert!((end - 0.5).abs() < 1e-9, "the end went to {end}");
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

    /// L1: a held pose returns the very same parts, so a cache keyed on their
    /// address can hit from one frame to the next.
    #[test]
    fn posed_twice_gives_pointer_equal_parts() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());
        data.bind_rigid(limb(), 1);

        let a = data.posed();
        let b = data.posed();
        assert_eq!(a.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            assert!(Arc::ptr_eq(x, y), "a held pose must reuse its parts");
        }
    }

    /// L1: the memo is re-derived, never trusted. Every public field that the
    /// posing reads invalidates it when edited in place.
    #[test]
    fn a_changed_pose_or_skeleton_gives_new_parts() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());
        data.bind_rigid(limb(), 1);
        let before = data.posed();

        data.armature.set_pose(&[0.0, 0.7]);
        let moved = data.posed();
        assert!(!Arc::ptr_eq(&before[1], &moved[1]));
        let again = data.posed();
        assert!(Arc::ptr_eq(&moved[1], &again[1]));

        // A bone edited by hand (not through the angle) is a change too.
        data.armature.bones[1].length = 70.0;
        let longer = data.posed();
        assert!(!Arc::ptr_eq(&moved[1], &longer[1]));
        data.armature.root = Point::new(5.0, 5.0);
        let shifted = data.posed();
        assert!(!Arc::ptr_eq(&longer[1], &shifted[1]));

        // So is an artwork replaced in place.
        data.parts[1].artwork = limb();
        let swapped = data.posed();
        assert!(!Arc::ptr_eq(&shifted[1], &swapped[1]));

        // And a re-bound skin.
        let skin_before = swapped[0].clone();
        data.armature.bones[0].angle = 0.3;
        data.rebind();
        let skin_after = data.posed();
        assert!(!Arc::ptr_eq(&skin_before, &skin_after[0]));
    }

    /// L1: a clone is a new rig. It does not share the memo (its parts are
    /// its own) yet still compares equal.
    #[test]
    fn a_cloned_armature_does_not_share_the_memo_but_is_equal() {
        let mut data = ArmatureData::new(arm());
        data.bind_rigid(limb(), 1);
        let original = data.posed();

        let copy = data.clone();
        assert_eq!(copy, data);
        let copied = copy.posed();
        assert!(!Arc::ptr_eq(&original[0], &copied[0]));
        assert_eq!(
            original[0].transform, copied[0].transform,
            "same pose, same geometry"
        );
        // And the original's memo was not disturbed by the clone.
        assert!(Arc::ptr_eq(&original[0], &data.posed()[0]));
    }

    /// L1: the pose that the memo returns is the pose the rig is in, bit for
    /// bit, whether it was just built or remembered.
    #[test]
    fn the_memoised_pose_equals_a_fresh_one() {
        let mut data = ArmatureData::new(arm());
        data.bind_shape(limb());
        data.bind_rigid(limb(), 1);
        data.armature.bones[1].angle = 0.4;

        let remembered = {
            data.posed();
            data.posed()
        };
        let fresh = data.clone().posed();
        for (a, b) in remembered.iter().zip(&fresh) {
            assert_eq!(a.transform, b.transform);
            match (&a.kind, &b.kind) {
                (ObjectKind::Shape(x), ObjectKind::Shape(y)) => assert_eq!(x.path, y.path),
                _ => panic!("expected shapes"),
            }
        }
    }
}
