//! BuzzAnimate application crate.
//!
//! Exposed as a library as well as a binary so integration tests can drive the
//! same scene-encoding path the window uses, rather than a reimplementation of
//! it that could silently drift.

pub mod agent;
pub mod agent_bridge;
pub mod api;
pub mod animate_assets;
pub mod asset_index;
pub mod asset_sets;
pub mod app;
pub mod demo;
pub mod dialogs;
pub mod editor;
pub mod encode_ahead;
pub mod episode;
pub mod estate;
pub mod export_service;
pub mod headless;
pub mod hatch;
pub mod hud;
pub mod import;
pub mod lights;
pub mod list_edit;
pub mod ports;
pub mod presets;
pub mod preview;
pub mod profile;
pub mod rig_limits;
pub mod rigging;
pub mod sound;
pub mod stage;
pub mod staging;
pub mod story;
pub mod tasks;
pub mod test_movie;
pub mod thumbnails;
pub mod title_bar;
pub mod tools;
pub mod warm_open;
pub mod ui_snapshot;
pub mod voice_bridge;
