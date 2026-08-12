//! Geometry foundation for BuzzAnimate.
//!
//! Everything here is `f64`. That is the whole point: Adobe Animate stores
//! coordinates as *twips* (signed 32-bit fixed point at 1/20 px) and rasterises
//! in `f32`, which is why its zoom stops at 2000%. BuzzAnimate stores document
//! coordinates as `f64` and never lets a large magnitude reach the GPU.
//!
//! See [`camera`] for the mechanism that makes unbounded zoom actually work.

pub mod boolean;
pub mod camera;
pub mod clip;
pub mod edit;
pub mod hit;

pub use boolean::{BoolOp, BooleanOptions, FillMode, boolean, boolean_many, union_all};
pub use camera::{Camera, RebasedTransform, RenderSplit};
pub use clip::RenderClip;
pub use edit::{StrokeStyle, expand_fill, outline_stroke, smooth, straighten};
pub use hit::{Hit, HitPart, HitTarget, NearestPoint, hit_test_all, hit_test_topmost};

/// Re-exported so downstream crates share one `kurbo` and one notion of `Point`.
pub use kurbo::{Affine, BezPath, Circle, Line, Point, Rect, Shape, Size, Vec2};
