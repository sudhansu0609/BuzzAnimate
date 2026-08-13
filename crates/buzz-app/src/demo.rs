//! The Phase 0 zoom target.
//!
//! A flat test scene proves nothing about zoom: once you pass a few hundred
//! percent there is simply nothing left to look at. This artwork is built so
//! that **every decade of zoom reveals a fresh generation of detail**, each one
//! a tenth the size of the last.
//!
//! Generation `g` sits at scale `10^-g`. So at 100% you see generation 0, at
//! 1 000% generation 1, and at Animate's 2 000% ceiling you are still only just
//! into generation 2 — with fourteen more waiting below.

use buzz_geom::{Affine, BezPath, Camera, Circle, Point, Rect, Shape};
use buzz_render::SceneBuilder;
use peniko::Color;
use vello::Scene;

/// One drawable primitive in document space.
pub struct Item {
    pub path: BezPath,
    pub fill: Option<Color>,
    /// Stroke colour and width, in document units.
    pub stroke: Option<(Color, f64)>,
    /// Which zoom generation this belongs to, for the HUD.
    pub generation: usize,
}

/// What one frame actually drew, and what it skipped.
///
/// Surfaced in the HUD so culling is observable rather than a silent behaviour
/// that could hide a bug — an empty stage should always be explainable.
#[derive(Debug, Default, Clone, Copy)]
pub struct CullStats {
    pub drawn: usize,
    /// Outside the visible document rect.
    pub culled_offscreen: usize,
    /// Too small on screen to register.
    pub culled_tiny: usize,
    pub min_generation: Option<usize>,
    pub max_generation: Option<usize>,
}

impl CullStats {
    pub fn culled(&self) -> usize {
        self.culled_offscreen + self.culled_tiny
    }

    /// Range of detail generations currently on screen, e.g. `"9–11"`.
    pub fn generation_range(&self) -> String {
        match (self.min_generation, self.max_generation) {
            (Some(a), Some(b)) if a == b => a.to_string(),
            (Some(a), Some(b)) => format!("{a}–{b}"),
            _ => "—".to_string(),
        }
    }
}

/// A scene with self-similar detail across many orders of magnitude.
pub struct ZoomTarget {
    pub items: Vec<Item>,
    pub center: Point,
    pub generations: usize,
}

/// Distinct hue per generation so it is obvious when a new one comes into view.
const PALETTE: [Color; 6] = [
    Color::from_rgb8(0xFF, 0x6B, 0x35),
    Color::from_rgb8(0x00, 0xD9, 0xA3),
    Color::from_rgb8(0x5B, 0x8F, 0xF9),
    Color::from_rgb8(0xFF, 0xD1, 0x66),
    Color::from_rgb8(0xE0, 0x6C, 0xD9),
    Color::from_rgb8(0x7A, 0xE5, 0x82),
];

impl ZoomTarget {
    /// Build `generations` levels of detail centred on `center`.
    ///
    /// Sixteen generations spans a factor of 1e16, comfortably past the point
    /// where `f64` coordinate storage becomes the limit — so the artwork is
    /// never the thing that runs out first.
    pub fn new(center: Point, base_radius: f64, generations: usize) -> Self {
        let mut items = Vec::new();

        for g in 0..generations {
            let r = base_radius * 0.1f64.powi(g as i32);
            let color = PALETTE[g % PALETTE.len()];
            let hairline = r * 0.015;

            // Ring marking this generation's extent.
            items.push(Item {
                path: Circle::new(center, r).to_path(1e-12),
                fill: None,
                stroke: Some((color, hairline)),
                generation: g,
            });

            // Satellites around the ring: visible structure while zooming in,
            // and they make rotation and drift obvious if the maths is wrong.
            for i in 0..8 {
                let a = std::f64::consts::TAU * (i as f64) / 8.0;
                let p = Point::new(center.x + a.cos() * r * 0.62, center.y + a.sin() * r * 0.62);
                items.push(Item {
                    path: Circle::new(p, r * 0.05).to_path(1e-12),
                    fill: Some(color),
                    stroke: None,
                    generation: g,
                });
            }

            // A square frame at 1/10 scale: this is exactly the region the next
            // generation occupies, so it reads as "keep zooming here".
            let inner = r * 0.1;
            items.push(Item {
                path: Rect::new(
                    center.x - inner,
                    center.y - inner,
                    center.x + inner,
                    center.y + inner,
                )
                .to_path(1e-12),
                fill: None,
                stroke: Some((color.multiply_alpha(0.55), hairline * 0.6)),
                generation: g,
            });

            // Crosshair ticks, offset so each generation is distinguishable
            // from its parent rather than a plain copy.
            let tick = r * 0.28;
            let rot = Affine::rotate_about(0.3 * g as f64, center);
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let mut p = BezPath::new();
                p.move_to(Point::new(
                    center.x + dx * r * 0.72,
                    center.y + dy * r * 0.72,
                ));
                p.line_to(Point::new(
                    center.x + dx * (r * 0.72 + tick),
                    center.y + dy * (r * 0.72 + tick),
                ));
                items.push(Item {
                    path: rot * p,
                    fill: None,
                    stroke: Some((color, hairline)),
                    generation: g,
                });
            }
        }

        Self {
            items,
            center,
            generations,
        }
    }

    /// Encode the artwork into `scene` for `camera`.
    ///
    /// # Culling here is purely an optimisation
    ///
    /// Correctness is handled by document-space clipping inside
    /// [`SceneBuilder`], which bounds both segment count and coordinate
    /// magnitude for shapes far larger than the viewport. Phase 0 instead
    /// *dropped* such shapes, which was wrong: a huge circle whose arc crosses
    /// the view would vanish, as would any background larger than the stage.
    /// That limitation is now retired.
    ///
    /// The two remaining tests are cheap bounding-box rejections that spare the
    /// clipper work it would otherwise do correctly but pointlessly. Removing
    /// them would cost performance, never correctness.
    pub fn encode(&self, scene: &mut Scene, camera: &Camera) -> CullStats {
        /// Below this on-screen size an item cannot be seen at all.
        const MIN_SCREEN_PX: f64 = 0.35;

        let visible = camera.visible_doc_rect();
        let mut stats = CullStats::default();
        let mut b = SceneBuilder::new(scene, camera);
        let scale = b.view_scale();

        for item in &self.items {
            let bb = item.path.bounding_box();

            // `Rect::intersect` clamps to zero size rather than producing
            // negative extents, so a width test here would never fire and the
            // rejection would be dead code. `overlaps` is the real predicate.
            if !bb.overlaps(visible) {
                stats.culled_offscreen += 1;
                continue;
            }

            let screen_size = bb.width().hypot(bb.height()) * scale;
            if screen_size < MIN_SCREEN_PX {
                stats.culled_tiny += 1;
                continue;
            }

            if let Some(fill) = item.fill {
                b.fill_shape(&item.path, fill);
            }
            if let Some((color, width)) = item.stroke {
                // Below a pixel a stroke would vanish, leaving the deeper
                // generations looking empty. Clamp to a hairline instead.
                if width * scale < 1.0 {
                    b.stroke_hairline(&item.path, color, 1.0);
                } else {
                    b.stroke_shape(&item.path, color, width);
                }
            }

            stats.drawn += 1;
            stats.min_generation = Some(match stats.min_generation {
                Some(g) => g.min(item.generation),
                None => item.generation,
            });
            stats.max_generation = Some(match stats.max_generation {
                Some(g) => g.max(item.generation),
                None => item.generation,
            });
        }

        stats
    }

    /// Frame generation `g` in `viewport`, the way a user zooming in would.
    pub fn camera_for_generation(&self, g: usize, viewport: buzz_geom::Size) -> Camera {
        let mut cam = Camera::new(self.center, 1.0, viewport);
        // Generation g has radius base * 10^-g; frame it with a little margin.
        cam.set_zoom_percent(100.0 * 10f64.powi(g as i32) * 1.6);
        cam
    }

    /// Which generation is the natural subject at this zoom.
    ///
    /// Used by the HUD to report how deep the view currently is.
    pub fn generation_at_zoom(&self, zoom: f64) -> usize {
        if zoom <= 0.0 || !zoom.is_finite() {
            return 0;
        }
        (zoom.log10().max(0.0).round() as usize).min(self.generations.saturating_sub(1))
    }
}

impl Default for ZoomTarget {
    fn default() -> Self {
        // Centred away from the origin on purpose: a target at (0, 0) would
        // hide exactly the large-coordinate precision problems this project
        // exists to solve.
        Self::new(Point::new(1024.0, 768.0), 300.0, 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_generation_contributes_geometry() {
        let t = ZoomTarget::new(Point::new(1000.0, 1000.0), 300.0, 16);
        for g in 0..16 {
            assert!(
                t.items.iter().any(|i| i.generation == g),
                "generation {g} produced no items"
            );
        }
    }

    /// The point of the artwork: detail keeps arriving as you zoom.
    #[test]
    fn detail_shrinks_by_a_decade_per_generation() {
        let t = ZoomTarget::new(Point::ORIGIN, 300.0, 8);

        let extent = |g: usize| {
            t.items
                .iter()
                .filter(|i| i.generation == g)
                .map(|i| i.path.bounding_box().width())
                .fold(0.0f64, f64::max)
        };

        for g in 1..8 {
            let ratio = extent(g - 1) / extent(g);
            assert!(
                (ratio - 10.0).abs() < 0.5,
                "generation {g} should be a tenth of {}, ratio was {ratio}",
                g - 1
            );
        }
    }

    #[test]
    fn deepest_generation_is_far_below_animates_ceiling() {
        let t = ZoomTarget::default();
        let smallest = t
            .items
            .iter()
            .map(|i| i.path.bounding_box().width())
            .fold(f64::INFINITY, f64::min);

        // Animate tops out at 20x. Confirm the artwork has detail that is
        // simply unreachable there.
        let visible_at_animate_max = 1.0 / 20.0;
        assert!(
            smallest < visible_at_animate_max * 1e-6,
            "smallest feature {smallest} should be far beyond Animate's reach"
        );
    }

    #[test]
    fn generation_at_zoom_tracks_decades() {
        let t = ZoomTarget::default();
        assert_eq!(t.generation_at_zoom(1.0), 0);
        assert_eq!(t.generation_at_zoom(10.0), 1);
        assert_eq!(t.generation_at_zoom(1e6), 6);
        // Saturates rather than panicking past the deepest generation.
        assert_eq!(t.generation_at_zoom(1e30), t.generations - 1);
        assert_eq!(t.generation_at_zoom(f64::NAN), 0);
    }
}
