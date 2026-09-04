//! Staging and performance: building a scene, and animating the people in it.
//!
//! # What this crate is for
//!
//! Two jobs sit between "a blank document" and "a shot worth drawing over", and
//! neither of them is drawing:
//!
//! 1. **Setting the scene** ([`staging`]) — a ground plane, a backdrop, a light
//!    rig that agrees with itself, and characters standing on the floor at
//!    plausible sizes and distances.
//! 2. **The performance** ([`perform`]) — a walk, a run, standing and talking,
//!    standing and breathing, written onto the timeline as poses.
//!
//! Both of them are arithmetic that every animator does by hand at the start of
//! every shot, and both have an obviously right answer that nobody enjoys
//! typing out. Neither is a substitute for animating: what comes out is
//! ordinary layers, ordinary shapes, ordinary keyframes and ordinary poses, and
//! the first thing anyone does with it is change it.
//!
//! # What this crate is *not*
//!
//! It is not a solver, it is not a simulation and there is no model in it. A
//! walk cycle here is the handful of curves an animator draws for one, written
//! down with the reason for each — see [`perform::pose_at`], which is the whole
//! of the animation and is meant to be read.
//!
//! It also does not touch the mouth. Lip sync is a fact about the soundtrack
//! and belongs to the analysis that reads it; a performance is a choice about
//! the body. The two run independently on the same character and neither knows
//! about the other, which is the arrangement that lets you re-record the
//! dialogue without re-animating the gestures.
//!
//! # Nothing here is live
//!
//! There is no "walking" property on an object and nothing re-runs. A
//! performance writes keyframes and is then gone, so every frame of it can be
//! edited, retimed or deleted like any other. A generated thing that stayed
//! generated would be a thing you cannot draw on top of, which is the opposite
//! of what a drawing tool is for.

pub mod autorig;
pub mod direct;
pub mod figure;
pub mod motion_path;
pub mod perform;
pub mod physics;
pub mod scenery;
pub mod staging;

pub use autorig::assemble;
pub use buzz_physics::{Spring, Wiggle};
pub use direct::{DirectError, DirectedScene, PlannedBeat, PlannedShot, direct, split_shots};
pub use figure::{FigureSpec, Joint, Palette, build as build_figure, is_figure, rest_pose};
pub use motion_path::{MotionError, MotionPathOptions, MotionReport, follow as follow_path};
pub use scenery::{Scenery, SceneryReport, lay as lay_scenery, lay_weather, weather_for};
pub use physics::{PhysicsError, PhysicsReport, bake as follow_through_bake, wiggle as wiggle_bake};
pub use perform::{
    Action, Beat, PerformError, PerformReport, Performance, apply as perform,
    apply_from as perform_from, pose_at,
};
pub use staging::{SceneRecipe, Setting, StagedScene, build as stage_scene};
