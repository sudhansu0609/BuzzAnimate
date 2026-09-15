//! Auto-rigging: turning drawings that have been sorted into slots into bones.
//!
//! # The bone is fitted to the artwork, not the artwork to the bone
//!
//! [`figure::build`] goes the other way — it invents a skeleton from a height
//! and draws capsules onto it — and that is right for a figure nobody has
//! drawn yet. It is wrong for a character somebody *has* drawn: their forearm
//! is at the angle they drew it, and a bone laid along the eight-heads default
//! would sit outside the arm it is supposed to turn.
//!
//! So every filled slot takes its **length and direction from the drawing in
//! it**. A part is measured along its own longest axis; the ends of that axis
//! are the ends of the bone. Drop an arm drawn leaning back and the bone leans
//! back with it, which is the difference between a rig that works immediately
//! and one the animator has to straighten out first.
//!
//! # What a round drawing does
//!
//! A head, a hand, a hoof: no longest axis worth the name. Where a part is
//! nearly as wide as it is long, the pattern's own direction is used instead
//! and only the *length* is measured from the drawing. Guessing a direction
//! from noise would point the head sideways as often as up.
//!
//! # Where the joints end up
//!
//! A child bone starts at its parent's tip — that is what an armature *is*, and
//! it is what keeps a chain connected when it bends. So a fitted bone
//! contributes its length and its angle, and its head is wherever the skeleton
//! puts it. On artwork whose parts meet, the two agree; on artwork with a gap
//! between the upper arm and the forearm, the bone bridges the gap rather than
//! leaving a chain in two pieces. The drawing itself does not move either way:
//! a rigidly bound part keeps the position it was drawn at until a bone turns.
//!
//! [`figure::build`]: crate::figure::build

use std::sync::Arc;

use buzz_geom::{Affine, PathEl, Point, Rect, Vec2};
use buzz_rig::{Armature, Bone, RigPattern, wrap_pi};
use buzz_scene::{ArmatureData, Object};

/// How finely a curve is sampled when measuring a part. A twentieth of a
/// document unit is far below anything that changes an angle.
const FLATTEN: f64 = 0.05;

/// A drawing is only given a direction of its own when it is this much longer
/// than it is wide. Below it, the pattern decides — see the module note.
const SLENDER: f64 = 1.2;

/// Build a rig from a pattern and the artwork sorted into its slots.
///
/// `parts` names, for each drawing, the slot it fills — **in the order the
/// drawings should be painted, back to front**. That order is kept, because it
/// is the one the animator already arranged in the Layers panel: binding in
/// slot order instead would put both legs in front of the head on every
/// character ever rigged.
///
/// Returns `None` when no slot has been filled, which is a rig with nothing in
/// it rather than an error worth a type of its own.
pub fn assemble(pattern: &RigPattern, parts: &[(usize, Arc<Object>)]) -> Option<ArmatureData> {
    if pattern.is_empty() {
        return None;
    }

    let count = pattern.slots.len();
    let aim = pattern.world_angles();

    // The bone each filled slot wants, measured off its drawing. A slot named
    // twice keeps the first drawing: two parts in one slot is a mistake the
    // panel does not allow, and picking silently is better than panicking.
    let mut fitted: Vec<Option<(Point, Point)>> = vec![None; count];
    let mut extent: Option<Rect> = None;
    for (slot, artwork) in parts {
        let Some(slot) = pattern.slots.get(*slot).and(Some(*slot)) else {
            continue;
        };
        extent = Some(match extent {
            Some(all) => all.union(artwork.bounds()),
            None => artwork.bounds(),
        });
        if fitted[slot].is_none() {
            fitted[slot] = fit(artwork, aim[slot]);
        }
    }
    let extent = extent?;

    let height = height_of(pattern, &fitted, extent);
    let root = root_of(pattern, &fitted, extent);

    let mut armature = Armature::new(root);
    for (index, slot) in pattern.slots.iter().enumerate() {
        // Parents are always earlier in a pattern, so this bone is already in.
        let parent_angle = match slot.parent {
            Some(parent) => armature.world_angle(parent),
            None => 0.0,
        };
        let (length, direction) = match fitted[index] {
            Some((head, tip)) => {
                let along = tip - head;
                (along.hypot(), along.y.atan2(along.x))
            }
            // An empty slot still gets a bone, so the skeleton is complete: a
            // character drawn without a separate chest can still turn at the
            // waist, and a performance written against this pattern still
            // finds every joint it addresses.
            None => (slot.rest_len * height, aim[index]),
        };
        let mut bone = Bone::new(
            slot.name.clone(),
            slot.parent,
            length,
            wrap_pi(direction - parent_angle),
        );
        bone.limits = slot.limits;
        armature.push(bone);
    }

    // The pose the artwork was drawn in *is* the rest pose, which is what
    // every later pose and tween is measured against.
    armature.set_rest_here();

    let mut rig = ArmatureData::new(armature);
    rig.pattern = Some(pattern.name.clone());
    for (slot, artwork) in parts {
        if *slot < count {
            // Rigid, not skinned: each part is its own drawing, so it should
            // turn about its joint rather than deform. See `RigBinding`.
            rig.bind_rigid(artwork.clone(), *slot);
        }
    }
    Some(rig)
}

/// The figure's height in document units, inferred from what was dropped.
///
/// Every slot's `rest_len` is a fraction of it, so each filled slot is one
/// estimate of the whole: a thigh that came out 235 units long says the
/// character is about 1000 tall. The **median** of those estimates is taken
/// rather than the mean, because one part drawn with a long shadow or a stray
/// point in it should not stretch every unfilled bone in the rig.
fn height_of(pattern: &RigPattern, fitted: &[Option<(Point, Point)>], extent: Rect) -> f64 {
    let mut estimates: Vec<f64> = fitted
        .iter()
        .enumerate()
        .filter_map(|(index, fit)| {
            let (head, tip) = (*fit)?;
            let length = (tip - head).hypot();
            let share = pattern.slots.get(index)?.rest_len;
            (length > 1e-6 && share > 1e-9).then_some(length / share)
        })
        .collect();

    if estimates.is_empty() {
        // Nothing measurable was dropped. The artwork still has a size, and
        // its longest side is a better guess than a constant.
        return extent.width().max(extent.height()).max(1.0);
    }
    estimates.sort_by(f64::total_cmp);
    estimates[estimates.len() / 2]
}

/// Where the armature starts, in the artwork's own coordinates.
///
/// The first *root* slot that was filled decides it: for a biped that is the
/// hips, and failing that a thigh, whose head is the hip joint either way.
/// With no root slot filled at all there is nothing in the drawing that says
/// where the character is anchored, so the middle of the artwork is used —
/// visibly wrong rather than quietly wrong, and one drag of the Bone tool from
/// fixing.
fn root_of(pattern: &RigPattern, fitted: &[Option<(Point, Point)>], extent: Rect) -> Point {
    pattern
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.parent.is_none())
        .find_map(|(index, _)| fitted.get(index).copied().flatten())
        .map_or_else(|| extent.center(), |(head, _)| head)
}

/// Measure a drawing: where its bone starts and where it ends.
///
/// `aim` is the direction the pattern expects this part to point, in world
/// radians. It settles two things the drawing cannot: which end is the head,
/// and what to do when the drawing is round.
fn fit(artwork: &Object, aim: f64) -> Option<(Point, Point)> {
    let points = outline(artwork);
    if points.len() < 2 {
        return None;
    }

    let centre = mean(&points);
    let expected = Vec2::new(aim.cos(), aim.sin());
    let axis = principal_axis(&points, centre).unwrap_or(expected);

    // Along the drawing's own axis, and across it. A part that is not clearly
    // longer one way than the other has no direction to give.
    let across = Vec2::new(-axis.y, axis.x);
    let axis = if span(&points, centre, axis) > span(&points, centre, across) * SLENDER {
        axis
    } else {
        expected
    };

    // Pointing the way the pattern says, so the *head* is the end nearest the
    // joint: a forearm drawn right-to-left and one drawn left-to-right both
    // start at the elbow.
    let axis = if axis.dot(expected) < 0.0 { -axis } else { axis };

    let mut near = f64::INFINITY;
    let mut far = f64::NEG_INFINITY;
    for point in &points {
        let along = (*point - centre).dot(axis);
        near = near.min(along);
        far = far.max(along);
    }
    if !(far - near).is_finite() || far - near < 1e-9 {
        return None;
    }
    Some((centre + axis * near, centre + axis * far))
}

/// Every point on a drawing's outline, in the coordinates it sits at.
///
/// Falls back to the corners of the bounding box for anything `flatten` cannot
/// reach into — a symbol instance needs the library to resolve, and a part that
/// is a placed instance should still be measurable.
fn outline(artwork: &Object) -> Vec<Point> {
    let mut shapes = Vec::new();
    artwork.flatten(Affine::IDENTITY, &mut shapes);

    let mut points = Vec::new();
    for (transform, shape) in &shapes {
        let path = *transform * shape.path.clone();
        kurbo::flatten(path.iter(), FLATTEN, |element| match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(p),
            _ => {}
        });
    }

    if points.is_empty() {
        let bounds = artwork.bounds();
        points.extend([
            Point::new(bounds.x0, bounds.y0),
            Point::new(bounds.x1, bounds.y0),
            Point::new(bounds.x1, bounds.y1),
            Point::new(bounds.x0, bounds.y1),
        ]);
    }
    points
}

fn mean(points: &[Point]) -> Point {
    let sum = points
        .iter()
        .fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    (sum / points.len() as f64).to_point()
}

/// How far the points reach along `direction`.
fn span(points: &[Point], centre: Point, direction: Vec2) -> f64 {
    let mut near = f64::INFINITY;
    let mut far = f64::NEG_INFINITY;
    for point in points {
        let along = (*point - centre).dot(direction);
        near = near.min(along);
        far = far.max(along);
    }
    (far - near).max(0.0)
}

/// The direction a cloud of points is longest in.
///
/// The larger eigenvector of the covariance matrix, in closed form because a
/// 2×2 has one. A bounding box would have done for a limb drawn upright and
/// would have been useless for one drawn at forty-five degrees, where the box
/// is square and says nothing.
fn principal_axis(points: &[Point], centre: Point) -> Option<Vec2> {
    let mut xx = 0.0;
    let mut xy = 0.0;
    let mut yy = 0.0;
    for point in points {
        let d = *point - centre;
        xx += d.x * d.x;
        xy += d.x * d.y;
        yy += d.y * d.y;
    }
    let n = points.len() as f64;
    let (xx, xy, yy) = (xx / n, xy / n, yy / n);

    // A perfectly symmetric cloud — a circle, a square — has no axis at all,
    // and the arithmetic below would hand back whichever way the rounding
    // fell. Saying so lets the caller use the pattern's direction instead.
    let spread = xx + yy;
    if spread < 1e-12 {
        return None;
    }
    let difference = xx - yy;
    let root = (difference * difference + 4.0 * xy * xy).sqrt();
    if root < spread * 1e-6 {
        return None;
    }

    let largest = (spread + root) * 0.5;
    // Either row of `M - λI` is an eigenvector turned a quarter; the better
    // conditioned of the two is the one with the larger entries.
    let axis = if xy.abs() > 1e-12 {
        Vec2::new(largest - yy, xy)
    } else if xx >= yy {
        Vec2::new(1.0, 0.0)
    } else {
        Vec2::new(0.0, 1.0)
    };
    let length = axis.hypot();
    (length > 1e-12).then(|| axis / length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{ObjectId, ObjectKind, ShapeData};
    use peniko::Color;
    use std::f64::consts::FRAC_PI_2;

    /// A limb drawn as a bar from `head` to `tip`, `width` across.
    fn bar(id: u64, head: Point, tip: Point, width: f64) -> Arc<Object> {
        let along = tip - head;
        let length = along.hypot();
        let rect = Rect::new(0.0, -width * 0.5, length, width * 0.5);
        let place = Affine::translate(head.to_vec2()) * Affine::rotate(along.y.atan2(along.x));
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(place * rect.to_path(1e-9), Color::WHITE),
        ))
    }

    fn disc(id: u64, centre: Point, radius: f64) -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(
                buzz_geom::Circle::new(centre, radius).to_path(0.05),
                Color::WHITE,
            ),
        ))
    }

    /// The whole point of fitting: a bone that lies along the drawing rather
    /// than along a default the artist never saw.
    #[test]
    fn a_bone_lands_along_the_drawing_it_was_dropped_on() {
        let pattern = RigPattern::prop();
        // An arm drawn leaning forty-five degrees, not the straight-up default.
        let leaning = bar(1, Point::new(0.0, 0.0), Point::new(70.0, -70.0), 12.0);

        let rig = assemble(&pattern, &[(1, leaning)]).expect("a rig");
        let arm = &rig.armature.bones[1];

        assert!(
            (arm.length - 70.0 * 2f64.sqrt()).abs() < 2.0,
            "the bone did not take the drawing's length: {}",
            arm.length
        );
        let world = rig.armature.world_angle(1);
        assert!(
            (wrap_pi(world - (-FRAC_PI_2 * 0.5))).abs() < 0.1,
            "the bone did not take the drawing's angle: {world}"
        );
    }

    /// A drawing has two ends and only the pattern knows which is the joint.
    #[test]
    fn a_part_drawn_backwards_still_starts_at_its_joint() {
        let pattern = RigPattern::prop();
        // Drawn right to left, while the pattern expects the arm to point up
        // and away. The tip must still be the far end from the base.
        let backwards = bar(1, Point::new(0.0, 0.0), Point::new(0.0, 60.0), 10.0);
        let forwards = bar(2, Point::new(0.0, 60.0), Point::new(0.0, 0.0), 10.0);

        let a = assemble(&pattern, &[(1, backwards)]).expect("a rig");
        let b = assemble(&pattern, &[(1, forwards)]).expect("a rig");
        assert!(
            (a.armature.bones[1].angle - b.armature.bones[1].angle).abs() < 1e-6,
            "the same drawing, drawn the other way round, gave a different bone"
        );
    }

    /// A head is round. Guessing a direction from it would point it sideways
    /// as often as up.
    #[test]
    fn a_round_part_takes_the_patterns_direction_rather_than_a_guess() {
        let pattern = RigPattern::prop();
        let head = disc(1, Point::new(0.0, -100.0), 30.0);

        let rig = assemble(&pattern, &[(2, head)]).expect("a rig");
        // The tip slot points along its parent, which points up.
        let world = rig.armature.world_angle(2);
        assert!(
            (wrap_pi(world - -FRAC_PI_2)).abs() < 0.05,
            "a round part was given a direction of its own: {world}"
        );
        assert!(
            (rig.armature.bones[2].length - 60.0).abs() < 2.0,
            "the length should still come from the drawing"
        );
    }

    /// The reason `parts` carries an order at all.
    #[test]
    fn parts_are_bound_in_the_order_they_are_painted_not_in_slot_order() {
        let pattern = RigPattern::prop();
        let base = bar(10, Point::new(0.0, 0.0), Point::new(0.0, -50.0), 20.0);
        let tip = bar(30, Point::new(0.0, -90.0), Point::new(0.0, -120.0), 14.0);

        // The tip is behind the base in the layer stack, so it must stay behind.
        let rig = assemble(&pattern, &[(2, tip), (0, base)]).expect("a rig");
        let ids: Vec<u64> = rig.parts.iter().map(|p| p.artwork.id.0).collect();
        assert_eq!(ids, [30, 10]);
    }

    /// An empty slot still needs a bone, or a performance addressing it by
    /// index would move the wrong limb.
    #[test]
    fn every_slot_gets_a_bone_even_the_empty_ones() {
        let pattern = RigPattern::biped();
        let thigh = bar(1, Point::new(0.0, 0.0), Point::new(0.0, 235.0), 30.0);

        let rig = assemble(&pattern, &[(7, thigh)]).expect("a rig");
        assert_eq!(rig.armature.len(), pattern.slots.len());
        assert!(
            rig.armature.bones.iter().all(|b| b.length > 1.0),
            "an unfilled slot came out with no bone worth the name"
        );
        // One thigh of 235 units says the figure is about a thousand tall, so
        // the head — 0.18 of it — should be around 180.
        let head = &rig.armature.bones[2];
        assert!(
            (head.length - 180.0).abs() < 20.0,
            "the figure was not scaled to its artwork: head is {}",
            head.length
        );
    }

    #[test]
    fn the_rig_remembers_the_pattern_it_was_built_from() {
        let pattern = RigPattern::biped();
        let part = bar(1, Point::new(0.0, 0.0), Point::new(0.0, -100.0), 20.0);
        let rig = assemble(&pattern, &[(0, part)]).expect("a rig");
        assert_eq!(rig.pattern.as_deref(), Some("Biped"));
    }

    #[test]
    fn nothing_dropped_makes_no_rig() {
        assert!(assemble(&RigPattern::biped(), &[]).is_none());
    }

    /// A rigidly bound part sits exactly where it was drawn until a bone
    /// turns. If it did not, rigging a character would move it.
    #[test]
    fn rigging_does_not_move_the_artwork() {
        let pattern = RigPattern::prop();
        let base = bar(1, Point::new(20.0, 300.0), Point::new(20.0, 240.0), 18.0);
        let before = base.bounds();

        let rig = assemble(&pattern, &[(0, base)]).expect("a rig");
        let posed = rig.posed();
        let after = posed[0].bounds();

        assert!(
            (after.x0 - before.x0).abs() < 1e-6 && (after.y0 - before.y0).abs() < 1e-6,
            "the drawing moved when it was rigged: {before:?} then {after:?}"
        );
    }

    /// The biped pattern is what a performance is written against, so a rig
    /// built from it has to satisfy the same check `figure::build` does.
    #[test]
    fn an_assembled_biped_is_a_figure_a_performance_can_drive() {
        let pattern = RigPattern::biped();
        let parts: Vec<(usize, Arc<Object>)> = (0..pattern.slots.len())
            .map(|slot| {
                let y = slot as f64 * 20.0;
                (
                    slot,
                    bar(slot as u64 + 1, Point::new(0.0, y), Point::new(0.0, y + 60.0), 12.0),
                )
            })
            .collect();

        let rig = assemble(&pattern, &parts).expect("a rig");
        let object = Object {
            kind: ObjectKind::Armature(rig),
            ..(*bar(99, Point::ZERO, Point::new(0.0, 1.0), 1.0)).clone()
        };
        assert!(crate::figure::is_figure(&object));
    }
}
