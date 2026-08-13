//! Drawing filtered artwork, and the cache that makes a blur affordable.
//!
//! Most of a filter costs nothing to build — a soft edge is a fill and a few
//! strokes (see `buzz-fx`). **Blur is the exception**: it is a stack of offset
//! copies of the artwork, and every one of them is a boolean. Rebuilding that
//! sixty times a second for artwork that has not changed would be the most
//! expensive thing this renderer does.
//!
//! So blurred geometry is cached exactly as lighting geometry is, on
//! copy-on-write **pointer identity**: an object nobody has edited is still the
//! same `Arc` in every snapshot, so drawing one brush stroke does not rebuild
//! anybody else's blur. The entry holds the `Arc` it keyed on, which is what
//! makes the address safe to compare — nothing can be freed and its address
//! reused while the entry lives.

use std::collections::HashMap;
use std::sync::Arc;

use buzz_fx::{Op, Quality};
use buzz_geom::{Affine, BezPath};
use buzz_scene::Object;
use peniko::Color;

use crate::SceneBuilder;

/// What identifies a piece of blurred geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    object: usize,
    shape: u16,
    /// The blur itself, quantised: two shapes blurred by the same amount share
    /// nothing, but the *same* shape re-blurred by the same amount does.
    radius: (i64, i64),
    bands: usize,
    /// Where the artwork sits, quantised to a tenth of a unit. A shape nudged
    /// by a hundredth of a pixel does not need its blur rebuilding.
    place: (i64, i64),
}

struct Entry {
    #[allow(dead_code, reason = "held for pointer identity, never read")]
    owner: Arc<Object>,
    ops: Arc<Vec<Op>>,
    used: u64,
}

/// Blurred geometry, kept between frames.
#[derive(Default)]
pub struct FilterCache {
    entries: HashMap<Key, Entry>,
    frame: u64,
}

/// How many frames an entry survives without being drawn. Enough to cover
/// onion skinning, which draws neighbouring frames.
const KEEP_FRAMES: u64 = 3;

impl FilterCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    pub fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame.saturating_sub(entry.used) < KEEP_FRAMES);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The ops that draw one path blurred, building them only if they are not
    /// already here.
    ///
    /// `owner` is the object's shared handle when it has one. Tweened and posed
    /// artwork has none — it is rebuilt for the frame being drawn — so its blur
    /// is generated and not kept, which is right: caching something that
    /// changes every frame only wastes the memory.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site; a struct would only move the arguments"
    )]
    pub fn blur(
        &mut self,
        owner: Option<&Arc<Object>>,
        shape_index: u16,
        path: &BezPath,
        color: Color,
        radius: (f64, f64),
        quality: Quality,
        tolerance: f64,
    ) -> Arc<Vec<Op>> {
        let build = || Arc::new(buzz_fx::blur_ops(path, color, radius, quality, tolerance));

        let Some(owner) = owner else {
            return build();
        };

        let bounds = {
            use buzz_geom::Shape as _;
            path.bounding_box()
        };
        let key = Key {
            object: Arc::as_ptr(owner) as usize,
            shape: shape_index,
            radius: (
                (radius.0 * 100.0).round() as i64,
                (radius.1 * 100.0).round() as i64,
            ),
            bands: quality.bands(),
            place: (
                (bounds.x0 * 10.0).round() as i64,
                (bounds.y0 * 10.0).round() as i64,
            ),
        };

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.frame;
            return Arc::clone(&entry.ops);
        }

        let ops = build();
        self.entries.insert(
            key,
            Entry {
                owner: Arc::clone(owner),
                ops: Arc::clone(&ops),
                used: self.frame,
            },
        );
        ops
    }
}

impl std::fmt::Debug for FilterCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterCache")
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// Draw a list of filter ops through `world`.
///
/// The ops arrive in the subject's own space; `world` puts them where the
/// artwork is. Clips are balanced by `buzz-fx` — it is tested — but this
/// counts them anyway and closes any that are left, because an unbalanced clip
/// would swallow the rest of the frame rather than fail visibly.
pub fn draw_ops(builder: &mut SceneBuilder<'_>, ops: &[Op], world: Affine, alpha: f64) {
    let mut open = 0usize;

    for op in ops {
        match op {
            Op::Fill {
                path,
                color,
                even_odd,
            } => {
                let colour = fade(*color, alpha);
                if *even_odd {
                    builder.fill_shape_even_odd(&(world * path.clone()), colour);
                } else {
                    builder.fill_shape(&(world * path.clone()), colour);
                }
            }
            Op::Stroke {
                path,
                color,
                width,
                transform,
            } => {
                builder.stroke_transformed(
                    &(world * *transform * path.clone()),
                    fade(*color, alpha),
                    *width,
                    world * *transform,
                );
            }
            Op::PushClip(path) => {
                builder.push_clip(&(world * path.clone()));
                open += 1;
            }
            Op::PopClip => {
                if open > 0 {
                    builder.pop_isolation();
                    open -= 1;
                }
            }
        }
    }

    for _ in 0..open {
        builder.pop_isolation();
    }
}

/// A filter's colour, faded by the authoring overlays (onion skin, guides).
fn fade(color: Color, alpha: f64) -> Color {
    if alpha >= 1.0 {
        color
    } else {
        color.multiply_alpha(alpha as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{ObjectId, ShapeData};

    fn object(id: u64) -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(0.0, 0.0, 40.0, 40.0).to_path(1e-9), Color::BLACK),
        ))
    }

    fn square() -> BezPath {
        Rect::new(0.0, 0.0, 40.0, 40.0).to_path(1e-9)
    }

    #[test]
    fn a_blur_is_built_once_and_then_reused() {
        let mut cache = FilterCache::new();
        let owner = object(1);
        cache.begin();

        let first = cache.blur(
            Some(&owner),
            0,
            &square(),
            Color::BLACK,
            (6.0, 6.0),
            Quality::Low,
            0.01,
        );
        assert_eq!(cache.len(), 1);

        let second = cache.blur(
            Some(&owner),
            0,
            &square(),
            Color::BLACK,
            (6.0, 6.0),
            Quality::Low,
            0.01,
        );
        assert!(Arc::ptr_eq(&first, &second), "the blur was rebuilt");
        assert_eq!(cache.len(), 1);
    }

    /// Changing the blur changes the geometry, so it must not hit the old
    /// entry — the bug this key exists to prevent.
    #[test]
    fn a_different_radius_is_a_different_entry() {
        let mut cache = FilterCache::new();
        let owner = object(1);
        cache.begin();

        cache.blur(Some(&owner), 0, &square(), Color::BLACK, (6.0, 6.0), Quality::Low, 0.01);
        cache.blur(Some(&owner), 0, &square(), Color::BLACK, (12.0, 6.0), Quality::Low, 0.01);
        cache.blur(Some(&owner), 0, &square(), Color::BLACK, (6.0, 6.0), Quality::High, 0.01);
        assert_eq!(cache.len(), 3);
    }

    /// Artwork with no shared handle — a tween, a pose — is rebuilt every
    /// frame rather than filling the cache with entries that can never hit.
    #[test]
    fn unowned_artwork_is_not_cached() {
        let mut cache = FilterCache::new();
        cache.begin();
        cache.blur(None, 0, &square(), Color::BLACK, (6.0, 6.0), Quality::Low, 0.01);
        assert!(cache.is_empty());
    }

    #[test]
    fn entries_are_dropped_when_they_stop_being_drawn() {
        let mut cache = FilterCache::new();
        let owner = object(1);
        cache.begin();
        cache.blur(Some(&owner), 0, &square(), Color::BLACK, (6.0, 6.0), Quality::Low, 0.01);
        cache.end();
        assert_eq!(cache.len(), 1, "still fresh");

        for _ in 0..KEEP_FRAMES + 1 {
            cache.begin();
            cache.end();
        }
        assert!(cache.is_empty(), "stale geometry was kept");
    }
}
