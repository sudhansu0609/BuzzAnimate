//! BuzzAnimate application crate.
//!
//! Exposed as a library as well as a binary so integration tests can drive the
//! same scene-encoding path the window uses, rather than a reimplementation of
//! it that could silently drift.

pub mod app;
pub mod demo;
pub mod editor;
pub mod hud;
pub mod stage;
pub mod tools;
