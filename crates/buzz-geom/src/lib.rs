//! Geometry foundation for BuzzAnimate.
//!
//! Everything here is `f64`. That is the whole point: Adobe Animate stores
//! coordinates as *twips* (signed 32-bit fixed point at 1/20 px) and rasterises
//! in `f32`, which is why its zoom stops at 2000%. BuzzAnimate stores document
//! coordinates as `f64` and never lets a large magnitude reach the GPU.
//!
//! See [`camera`] for the mechanism that makes unbounded zoom actually work.

pub mod camera;
pub mod clip;

pub use camera::{Camera, RebasedTransform, RenderSplit};
pub use clip::RenderClip;

/// Re-exported so downstream crates share one `kurbo` and one notion of `Point`.
pub use kurbo::{Affine, BezPath, Circle, Line, Point, Rect, Shape, Size, Vec2};
