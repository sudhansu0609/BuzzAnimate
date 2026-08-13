//! The BuzzAnimate application shell.
//!
//! Theme, menus, panels and the editor-facing state they operate on. The split
//! from `buzz-app` is deliberate: everything here is testable without opening a
//! window, including the shortcut map, the snapping arithmetic and the panels
//! themselves.

pub mod actions_panel;
pub mod brush;
pub mod camera_panel;
pub mod command;
pub mod depth_panel;
pub mod export_panel;
pub mod filter_panel;
pub mod icons;
pub mod about;
pub mod assets_panel;
pub mod library_panel;
pub mod new_document;
pub mod recovery_panel;
pub mod swatch_panel;
pub mod light_panel;
pub mod sound_panel;
pub mod lipsync_panel;
pub mod panels;
pub mod rig_panel;
pub mod selection;
pub mod style;
pub mod theme;
pub mod timeline_panel;
pub mod tools;
pub mod view;
pub mod workspace;

pub use actions_panel::{ActionsResponse, ActionsState, SampleEntry, actions_panel};
pub use brush::{BrushKind, BrushSettings, PatternShape};
pub use camera_panel::{CameraResponse, camera_panel};
pub use command::Command;
pub use depth_panel::{DepthResponse, depth_panel};
pub use export_panel::{ContainerChoice, VideoChoice, VideoOptions,ExportKind, ExportResponse, ExportState, export_dialog};
pub use filter_panel::{FilterPanelState, FilterResponse, FilterTarget, filter_panel};
pub use about::{AboutState, about_dialog};
pub use assets_panel::{AssetAction, AssetPanelState, assets_panel};
pub use library_panel::{LibraryState, library_panel};
pub use recovery_panel::{RecoveryChoice, RecoveryEntry, RecoveryState, recovery_dialog};
pub use new_document::{
    DocumentSetup, NewDocumentResponse, NewDocumentState, new_document_dialog,
};
pub use swatch_panel::{SwatchState, swatch_panel};
pub use light_panel::{LightPanelState, LightResponse, light_panel};
pub use sound_panel::{SoundChoice, SoundResponse, sound_panel};
pub use lipsync_panel::{Choice, LipSyncResponse, LipSyncState, lip_sync_dialog};
pub use rig_panel::{RigResponse, rig_panel};
pub use selection::Selection;
pub use style::{DrawStyle, DrawingMode, StrokeKind};
pub use theme::{Metrics, Palette};
pub use timeline_panel::{
    FrameAction, TimelineResponse, TimelineState, TweenRequest, Waveform, timeline_panel,
};
pub use tools::{ToolId, ToolStatus, all_tools, tool_for_key};
pub use view::{Guide, Orientation, SnapSettings, SnapTarget, Snapped, ViewSettings};
pub use workspace::{Dock, PanelId, Slot, Workspace};
