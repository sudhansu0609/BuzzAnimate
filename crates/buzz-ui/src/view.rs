//! Stage view settings: rulers, grid, guides and snapping.
//!
//! All of it is plain data and arithmetic, deliberately kept out of the drawing
//! code so it can be tested without a window. Snapping in particular is easy to
//! get subtly wrong — snapping in *screen* space instead of document space
//! makes the snap distance change with zoom, which feels broken at 1000% and
//! useless at 10%.

use buzz_geom::{Point, Rect};
use serde::{Deserialize, Serialize};

/// A guide dragged off a ruler.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    /// Position along the perpendicular axis, in document units.
    pub position: f64,
    pub orientation: Orientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    /// Runs left to right; `position` is a y coordinate.
    Horizontal,
    /// Runs top to bottom; `position` is an x coordinate.
    Vertical,
}

/// What the stage snaps to. Mirrors Animate's View ▸ Snapping submenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapSettings {
    pub to_guides: bool,
    pub to_grid: bool,
    pub to_objects: bool,
    pub to_pixels: bool,
}

impl Default for SnapSettings {
    fn default() -> Self {
        // Animate enables guide and object snapping by default, not grid.
        Self {
            to_guides: true,
            to_grid: false,
            to_objects: true,
            to_pixels: false,
        }
    }
}

impl SnapSettings {
    pub fn any(&self) -> bool {
        self.to_guides || self.to_grid || self.to_objects || self.to_pixels
    }
}

/// Everything about how the stage is displayed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewSettings {
    pub show_rulers: bool,
    pub show_grid: bool,
    pub show_guides: bool,
    pub lock_guides: bool,
    /// Draw artwork sitting outside the stage rectangle.
    pub show_pasteboard: bool,
    pub snap: SnapSettings,
    /// Grid spacing in document units.
    pub grid_spacing: f64,
    pub guides: Vec<Guide>,
    /// How close, in *screen pixels*, a drag must come before it snaps.
    pub snap_tolerance_px: f64,
    /// How rough a drawing may be and still be recognised as a circle, a
    /// rectangle or a line. Animate keeps the same choice in Preferences.
    pub shape_tolerance: buzz_geom::Tolerance,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            show_rulers: true,
            show_grid: false,
            show_guides: true,
            lock_guides: false,
            show_pasteboard: true,
            snap: SnapSettings::default(),
            shape_tolerance: buzz_geom::Tolerance::Normal,
            // Animate's default grid.
            grid_spacing: 10.0,
            guides: Vec::new(),
            snap_tolerance_px: 8.0,
        }
    }
}

/// What a snap latched onto, so the stage can show why it moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapTarget {
    Guide(Orientation),
    Grid,
    ObjectEdge,
    Pixel,
}

/// The outcome of snapping a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapped {
    pub point: Point,
    pub x_target: Option<SnapTarget>,
    pub y_target: Option<SnapTarget>,
}

impl Snapped {
    pub fn unchanged(point: Point) -> Self {
        Self {
            point,
            x_target: None,
            y_target: None,
        }
    }

    pub fn did_snap(&self) -> bool {
        self.x_target.is_some() || self.y_target.is_some()
    }
}

impl ViewSettings {
    pub fn add_guide(&mut self, guide: Guide) {
        self.guides.push(guide);
    }

    /// Remove the guide nearest `position` on `orientation`, within tolerance.
    pub fn remove_guide_near(
        &mut self,
        orientation: Orientation,
        position: f64,
        tolerance: f64,
    ) -> bool {
        if self.lock_guides {
            return false;
        }
        let found = self
            .guides
            .iter()
            .enumerate()
            .filter(|(_, g)| g.orientation == orientation)
            .filter(|(_, g)| (g.position - position).abs() <= tolerance)
            .min_by(|a, b| {
                (a.1.position - position)
                    .abs()
                    .total_cmp(&(b.1.position - position).abs())
            })
            .map(|(i, _)| i);

        match found {
            Some(i) => {
                self.guides.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn clear_guides(&mut self) {
        if !self.lock_guides {
            self.guides.clear();
        }
    }

    /// Snap a document-space point.
    ///
    /// `zoom` converts the screen-space tolerance into document units, so the
    /// snap always feels the same distance on screen whatever the zoom — the
    /// detail that makes snapping usable at 50% and at 5000% alike.
    ///
    /// `object_edges` are candidate coordinates from nearby geometry; the
    /// caller supplies them because only it knows what is on screen.
    pub fn snap_point(&self, point: Point, zoom: f64, object_edges: &[Rect]) -> Snapped {
        if !self.snap.any() || !(zoom.is_finite() && zoom > 0.0) {
            return Snapped::unchanged(point);
        }

        let tolerance = self.snap_tolerance_px / zoom;
        let mut best_x: Option<(f64, f64, SnapTarget)> = None;
        let mut best_y: Option<(f64, f64, SnapTarget)> = None;

        let mut consider = |axis_x: bool, candidate: f64, target: SnapTarget| {
            let current = if axis_x { point.x } else { point.y };
            let distance = (candidate - current).abs();
            if distance > tolerance {
                return;
            }
            let slot = if axis_x { &mut best_x } else { &mut best_y };
            if slot.is_none_or(|(_, d, _)| distance < d) {
                *slot = Some((candidate, distance, target));
            }
        };

        if self.snap.to_guides && self.show_guides {
            for guide in &self.guides {
                match guide.orientation {
                    Orientation::Vertical => {
                        consider(true, guide.position, SnapTarget::Guide(guide.orientation));
                    }
                    Orientation::Horizontal => {
                        consider(false, guide.position, SnapTarget::Guide(guide.orientation));
                    }
                }
            }
        }

        if self.snap.to_grid && self.grid_spacing > 0.0 {
            let nearest = |v: f64| (v / self.grid_spacing).round() * self.grid_spacing;
            consider(true, nearest(point.x), SnapTarget::Grid);
            consider(false, nearest(point.y), SnapTarget::Grid);
        }

        if self.snap.to_objects {
            for rect in object_edges {
                for x in [rect.x0, rect.center().x, rect.x1] {
                    consider(true, x, SnapTarget::ObjectEdge);
                }
                for y in [rect.y0, rect.center().y, rect.y1] {
                    consider(false, y, SnapTarget::ObjectEdge);
                }
            }
        }

        if self.snap.to_pixels {
            consider(true, point.x.round(), SnapTarget::Pixel);
            consider(false, point.y.round(), SnapTarget::Pixel);
        }

        Snapped {
            point: Point::new(
                best_x.map(|(v, _, _)| v).unwrap_or(point.x),
                best_y.map(|(v, _, _)| v).unwrap_or(point.y),
            ),
            x_target: best_x.map(|(_, _, t)| t),
            y_target: best_y.map(|(_, _, t)| t),
        }
    }

    /// Grid line spacing that stays legible at the current zoom.
    ///
    /// Drawing every 10-unit line at 5% zoom would be a grey smear, and at
    /// 5000% the lines would be miles apart. The spacing steps by powers of ten
    /// so lines stay roughly 8 px apart or more.
    pub fn effective_grid_spacing(&self, zoom: f64) -> f64 {
        const MIN_SCREEN_GAP: f64 = 8.0;
        let base = if self.grid_spacing > 0.0 {
            self.grid_spacing
        } else {
            10.0
        };
        if !(zoom.is_finite() && zoom > 0.0) {
            return base;
        }

        let mut spacing = base;
        // Coarsen when zoomed out.
        while spacing * zoom < MIN_SCREEN_GAP {
            spacing *= 10.0;
            if !spacing.is_finite() {
                return base;
            }
        }
        // Refine when zoomed in, but never below the configured spacing.
        while spacing > base && spacing * zoom / 10.0 >= MIN_SCREEN_GAP {
            spacing /= 10.0;
        }
        spacing
    }

    /// Ruler tick spacing, chosen the same way but coarser so labels fit.
    pub fn ruler_step(&self, zoom: f64) -> f64 {
        const MIN_LABEL_GAP: f64 = 55.0;
        if !(zoom.is_finite() && zoom > 0.0) {
            return 100.0;
        }
        // 1, 2, 5, 10, 20, 50, … in document units.
        let mut step = 1.0f64;
        let mut cycle = 0;
        while step * zoom < MIN_LABEL_GAP {
            step *= match cycle % 3 {
                0 => 2.0,
                1 => 2.5,
                _ => 2.0,
            };
            cycle += 1;
            if !step.is_finite() || cycle > 200 {
                break;
            }
        }
        while step > 1.0 && step * zoom / 2.0 >= MIN_LABEL_GAP {
            step /= 2.0;
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ViewSettings {
        ViewSettings::default()
    }

    #[test]
    fn defaults_match_animate() {
        let v = settings();
        assert!(v.show_rulers, "Animate shows rulers by default");
        assert!(!v.show_grid, "the grid is off by default");
        assert!(v.snap.to_guides && v.snap.to_objects);
        assert!(!v.snap.to_grid);
        assert_eq!(v.grid_spacing, 10.0);
    }

    #[test]
    fn nothing_moves_when_snapping_is_off() {
        let mut v = settings();
        v.snap = SnapSettings {
            to_guides: false,
            to_grid: false,
            to_objects: false,
            to_pixels: false,
        };
        let p = Point::new(3.3, 7.7);
        let s = v.snap_point(p, 1.0, &[]);
        assert_eq!(s.point, p);
        assert!(!s.did_snap());
    }

    #[test]
    fn a_point_snaps_to_a_nearby_guide() {
        let mut v = settings();
        v.add_guide(Guide {
            position: 100.0,
            orientation: Orientation::Vertical,
        });

        let s = v.snap_point(Point::new(103.0, 50.0), 1.0, &[]);
        assert_eq!(s.point.x, 100.0);
        assert_eq!(s.point.y, 50.0, "the other axis must not move");
        assert!(matches!(s.x_target, Some(SnapTarget::Guide(_))));
    }

    /// The detail that makes snapping usable at any zoom.
    #[test]
    fn snap_distance_is_constant_on_screen_not_in_document_units() {
        let mut v = settings();
        v.add_guide(Guide {
            position: 100.0,
            orientation: Orientation::Vertical,
        });

        // At 1x, 8 px of tolerance is 8 document units, so 5 away snaps.
        assert_eq!(
            v.snap_point(Point::new(105.0, 0.0), 1.0, &[]).point.x,
            100.0
        );

        // At 10x, 8 px is 0.8 units, so 5 away is far too distant.
        assert_eq!(
            v.snap_point(Point::new(105.0, 0.0), 10.0, &[]).point.x,
            105.0
        );

        // But 0.5 away does snap at 10x.
        assert_eq!(
            v.snap_point(Point::new(100.5, 0.0), 10.0, &[]).point.x,
            100.0
        );
    }

    #[test]
    fn the_nearest_of_several_guides_wins() {
        let mut v = settings();
        for position in [100.0, 104.0, 96.0] {
            v.add_guide(Guide {
                position,
                orientation: Orientation::Vertical,
            });
        }
        let s = v.snap_point(Point::new(103.0, 0.0), 1.0, &[]);
        assert_eq!(s.point.x, 104.0, "should take the closest guide");
    }

    #[test]
    fn grid_snapping_rounds_to_the_nearest_intersection() {
        let mut v = settings();
        v.snap.to_grid = true;
        v.snap.to_guides = false;
        v.snap.to_objects = false;
        v.grid_spacing = 10.0;

        let s = v.snap_point(Point::new(23.0, 37.0), 1.0, &[]);
        assert_eq!(s.point, Point::new(20.0, 40.0));
        assert!(matches!(s.x_target, Some(SnapTarget::Grid)));
    }

    #[test]
    fn objects_snap_on_edges_and_centres() {
        let mut v = settings();
        v.snap.to_guides = false;
        let rect = Rect::new(0.0, 0.0, 100.0, 50.0);

        // Left edge.
        assert_eq!(
            v.snap_point(Point::new(2.0, 200.0), 1.0, &[rect]).point.x,
            0.0
        );
        // Centre.
        assert_eq!(
            v.snap_point(Point::new(48.0, 200.0), 1.0, &[rect]).point.x,
            50.0
        );
        // Right edge.
        assert_eq!(
            v.snap_point(Point::new(103.0, 200.0), 1.0, &[rect]).point.x,
            100.0
        );
    }

    #[test]
    fn pixel_snapping_rounds_to_whole_units() {
        let mut v = settings();
        v.snap = SnapSettings {
            to_guides: false,
            to_grid: false,
            to_objects: false,
            to_pixels: true,
        };
        let s = v.snap_point(Point::new(10.4, 20.6), 1.0, &[]);
        assert_eq!(s.point, Point::new(10.0, 21.0));
    }

    #[test]
    fn hidden_guides_do_not_snap() {
        let mut v = settings();
        v.show_guides = false;
        v.add_guide(Guide {
            position: 100.0,
            orientation: Orientation::Vertical,
        });
        assert_eq!(
            v.snap_point(Point::new(101.0, 0.0), 1.0, &[]).point.x,
            101.0
        );
    }

    #[test]
    fn guides_can_be_added_and_removed() {
        let mut v = settings();
        v.add_guide(Guide {
            position: 50.0,
            orientation: Orientation::Horizontal,
        });
        assert_eq!(v.guides.len(), 1);

        assert!(
            !v.remove_guide_near(Orientation::Vertical, 50.0, 3.0),
            "wrong axis"
        );
        assert!(
            !v.remove_guide_near(Orientation::Horizontal, 90.0, 3.0),
            "too far"
        );
        assert!(v.remove_guide_near(Orientation::Horizontal, 51.0, 3.0));
        assert!(v.guides.is_empty());
    }

    #[test]
    fn locked_guides_cannot_be_removed() {
        let mut v = settings();
        v.add_guide(Guide {
            position: 50.0,
            orientation: Orientation::Horizontal,
        });
        v.lock_guides = true;

        assert!(!v.remove_guide_near(Orientation::Horizontal, 50.0, 3.0));
        v.clear_guides();
        assert_eq!(v.guides.len(), 1, "locked guides must survive");
    }

    /// The grid must stay legible across the zoom range the engine supports.
    #[test]
    fn grid_spacing_adapts_to_zoom() {
        let v = settings();

        // Zoomed way out: the grid coarsens rather than becoming a smear.
        let far = v.effective_grid_spacing(0.01);
        assert!(far >= 100.0, "at 1% the grid should coarsen, got {far}");
        assert!(far * 0.01 >= 8.0, "lines would be closer than 8 px apart");

        // At 1:1 the configured spacing is used as-is.
        assert_eq!(v.effective_grid_spacing(1.0), 10.0);

        // Zoomed in it never goes finer than configured.
        assert_eq!(v.effective_grid_spacing(50.0), 10.0);
    }

    #[test]
    fn grid_spacing_survives_absurd_zoom_without_hanging() {
        let v = settings();
        for zoom in [1e-12, 1e12, f64::MIN_POSITIVE, 1e300] {
            let s = v.effective_grid_spacing(zoom);
            assert!(s.is_finite() && s > 0.0, "bad spacing {s} at zoom {zoom}");
        }
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(v.effective_grid_spacing(bad).is_finite());
        }
    }

    #[test]
    fn ruler_steps_stay_readable() {
        let v = settings();
        for zoom in [0.05, 0.25, 1.0, 4.0, 20.0, 1000.0] {
            let step = v.ruler_step(zoom);
            assert!(step > 0.0 && step.is_finite(), "bad step at zoom {zoom}");
            let gap = step * zoom;
            assert!(
                gap >= 40.0,
                "labels would overlap at zoom {zoom}: {gap} px apart"
            );
        }
        for bad in [0.0, f64::NAN] {
            assert!(v.ruler_step(bad).is_finite());
        }
    }

    #[test]
    fn view_settings_round_trip_through_json() {
        let mut v = settings();
        v.add_guide(Guide {
            position: 42.0,
            orientation: Orientation::Vertical,
        });
        v.show_grid = true;

        let json = serde_json::to_string(&v).unwrap();
        let back: ViewSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
