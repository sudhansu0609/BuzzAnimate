//! Spatial index over the stage.
//!
//! # Why an index at all
//!
//! Hit-testing and viewport culling both ask "what is near here?". Answering by
//! scanning every object is `O(n)` per query, which is fine for a hundred
//! objects and hopeless for a hundred thousand. An R-tree answers in roughly
//! `O(log n)`.
//!
//! # Why it is rebuilt rather than mutated
//!
//! The index is derived data. Keeping it incrementally correct through every
//! edit, undo and reorder is a large source of subtle bugs, and a *stale* index
//! silently makes objects unclickable. Instead it is rebuilt from a snapshot on
//! the background pool, and callers can tell whether the one they hold is
//! current. A rebuild for a large scene costs a few milliseconds off the UI
//! thread; a wrong index costs the user their selection.

use buzz_geom::Rect;
use rstar::{AABB, PointDistance, RTree, RTreeObject};

use crate::layer::LayerId;
use crate::object::ObjectId;

/// One entry in the index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexEntry {
    pub object: ObjectId,
    pub layer: LayerId,
    /// World-space bounds.
    pub bounds: Rect,
    /// Paint order across the whole scene; higher is nearer the front.
    pub depth: usize,
}

impl RTreeObject for IndexEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounds.x0, self.bounds.y0],
            [self.bounds.x1, self.bounds.y1],
        )
    }
}

impl PointDistance for IndexEntry {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        self.envelope().distance_2(point)
    }

    fn contains_point(&self, point: &[f64; 2]) -> bool {
        self.bounds.x0 <= point[0]
            && point[0] <= self.bounds.x1
            && self.bounds.y0 <= point[1]
            && point[1] <= self.bounds.y1
    }
}

/// An R-tree over a scene snapshot.
#[derive(Debug, Clone)]
pub struct SpatialIndex {
    tree: RTree<IndexEntry>,
    /// The snapshot revision this was built from.
    revision: u64,
}

impl SpatialIndex {
    /// Build from entries. Bulk loading produces a far better-balanced tree
    /// than inserting one at a time.
    pub fn build(entries: Vec<IndexEntry>, revision: u64) -> Self {
        Self {
            tree: RTree::bulk_load(entries),
            revision,
        }
    }

    pub fn empty() -> Self {
        Self {
            tree: RTree::new(),
            revision: 0,
        }
    }

    /// Which scene revision this index describes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Is this index still valid for `scene_revision`?
    ///
    /// Callers should fall back to a linear scan when it is not, rather than
    /// trusting stale results and silently losing objects.
    pub fn is_current_for(&self, scene_revision: u64) -> bool {
        self.revision == scene_revision
    }

    pub fn len(&self) -> usize {
        self.tree.size()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }

    /// Entries whose bounds intersect `rect`, unordered.
    pub fn query_rect(&self, rect: Rect) -> Vec<IndexEntry> {
        let envelope = AABB::from_corners([rect.x0, rect.y0], [rect.x1, rect.y1]);
        self.tree
            .locate_in_envelope_intersecting(envelope)
            .copied()
            .collect()
    }

    /// Entries whose bounds contain `point`, front-most first.
    ///
    /// A bounding-box test only: the caller still needs a precise geometric
    /// test on the candidates. The index narrows the field, it does not decide.
    pub fn query_point(&self, point: buzz_geom::Point) -> Vec<IndexEntry> {
        let mut hits: Vec<IndexEntry> = self
            .tree
            .locate_all_at_point([point.x, point.y])
            .copied()
            .collect();
        // Front-most first, matching the paint order.
        hits.sort_unstable_by_key(|e| std::cmp::Reverse(e.depth));
        hits
    }

    /// Entries within `radius` of `point`, for tolerant stroke picking.
    pub fn query_near(&self, point: buzz_geom::Point, radius: f64) -> Vec<IndexEntry> {
        let r = radius.max(0.0);
        self.query_rect(Rect::new(
            point.x - r,
            point.y - r,
            point.x + r,
            point.y + r,
        ))
    }
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Point;

    fn entry(id: u64, x: f64, y: f64, size: f64, depth: usize) -> IndexEntry {
        IndexEntry {
            object: ObjectId(id),
            layer: LayerId(1),
            bounds: Rect::new(x, y, x + size, y + size),
            depth,
        }
    }

    fn grid(n: u64) -> Vec<IndexEntry> {
        (0..n)
            .map(|i| {
                let x = (i % 100) as f64 * 20.0;
                let y = (i / 100) as f64 * 20.0;
                entry(i, x, y, 10.0, i as usize)
            })
            .collect()
    }

    #[test]
    fn an_empty_index_answers_without_panicking() {
        let idx = SpatialIndex::empty();
        assert!(idx.is_empty());
        assert!(idx.query_point(Point::new(0.0, 0.0)).is_empty());
        assert!(idx.query_rect(Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
    }

    #[test]
    fn point_queries_find_only_what_covers_the_point() {
        let idx = SpatialIndex::build(
            vec![entry(1, 0.0, 0.0, 10.0, 0), entry(2, 100.0, 100.0, 10.0, 1)],
            1,
        );
        let hits = idx.query_point(Point::new(5.0, 5.0));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, ObjectId(1));

        assert!(idx.query_point(Point::new(50.0, 50.0)).is_empty());
    }

    #[test]
    fn overlapping_hits_come_back_frontmost_first() {
        let idx = SpatialIndex::build(
            vec![
                entry(1, 0.0, 0.0, 50.0, 0),
                entry(2, 10.0, 10.0, 50.0, 5),
                entry(3, 20.0, 20.0, 50.0, 2),
            ],
            1,
        );
        let hits = idx.query_point(Point::new(30.0, 30.0));
        let order: Vec<usize> = hits.iter().map(|h| h.depth).collect();
        assert_eq!(order, vec![5, 2, 0], "expected front-to-back by depth");
    }

    #[test]
    fn rect_queries_return_everything_that_intersects() {
        let idx = SpatialIndex::build(grid(1000), 1);
        let found = idx.query_rect(Rect::new(0.0, 0.0, 45.0, 45.0));
        // The 20-unit grid puts 3x3 cells inside a 45-unit window.
        assert_eq!(found.len(), 9, "got {} entries", found.len());
    }

    #[test]
    fn near_queries_widen_the_search_by_the_radius() {
        let idx = SpatialIndex::build(vec![entry(1, 0.0, 0.0, 10.0, 0)], 1);
        assert!(idx.query_near(Point::new(15.0, 5.0), 1.0).is_empty());
        assert_eq!(idx.query_near(Point::new(15.0, 5.0), 6.0).len(), 1);
    }

    /// The index must agree with a brute-force scan, or it is worse than none.
    #[test]
    fn index_results_match_a_linear_scan() {
        let entries = grid(5000);
        let idx = SpatialIndex::build(entries.clone(), 1);

        for rect in [
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(500.0, 200.0, 700.0, 400.0),
            Rect::new(-50.0, -50.0, 5.0, 5.0),
            Rect::new(9000.0, 9000.0, 9100.0, 9100.0),
        ] {
            let mut from_index: Vec<u64> =
                idx.query_rect(rect).iter().map(|e| e.object.0).collect();
            let mut brute: Vec<u64> = entries
                .iter()
                .filter(|e| e.bounds.overlaps(rect))
                .map(|e| e.object.0)
                .collect();
            from_index.sort_unstable();
            brute.sort_unstable();
            assert_eq!(
                from_index, brute,
                "index disagreed with a scan for {rect:?}"
            );
        }
    }

    #[test]
    fn staleness_is_detectable() {
        let idx = SpatialIndex::build(grid(10), 7);
        assert!(idx.is_current_for(7));
        assert!(!idx.is_current_for(8), "a stale index must report itself");
        assert_eq!(idx.revision(), 7);
    }

    #[test]
    fn a_large_index_builds_and_queries_quickly() {
        let start = std::time::Instant::now();
        let idx = SpatialIndex::build(grid(50_000), 1);
        let build = start.elapsed();

        let start = std::time::Instant::now();
        for i in 0..1000 {
            let x = (i % 100) as f64 * 20.0;
            let _ = idx.query_point(Point::new(x + 5.0, 5.0));
        }
        let queries = start.elapsed();

        assert_eq!(idx.len(), 50_000);
        // Generous bounds: this guards against an accidental O(n) query path,
        // not against small performance drift.
        assert!(
            build.as_millis() < 5_000,
            "building 50k entries took {build:?}"
        );
        assert!(
            queries.as_millis() < 2_000,
            "1000 point queries took {queries:?}"
        );
    }
}
