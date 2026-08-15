//! BuzzAnimate application crate.
//!
//! Exposed as a library as well as a binary so integration tests can drive the
//! same scene-encoding path the window uses, rather than a reimplementation of
//! it that could silently drift.

pub mod animate_assets;
pub mod app;
pub mod demo;
pub mod dialogs;
pub mod editor;
pub mod export_service;
pub mod hud;
pub mod import;
pub mod lights;
pub mod lipsync;
pub mod presets;
pub mod rigging;
pub mod sound;
pub mod stage;
pub mod tasks;
pub mod thumbnails;
pub mod tools;
