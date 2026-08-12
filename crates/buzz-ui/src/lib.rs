//! The BuzzAnimate application shell.
//!
//! Theme, menus, panels and the editor-facing state they operate on. The split
//! from `buzz-app` is deliberate: everything here is testable without opening a
//! window, including the shortcut map, the snapping arithmetic and the panels
//! themselves.

pub mod brush;
pub mod command;
pub mod depth_panel;
pub mod library_panel;
pub mod panels;
pub mod selection;
pub mod style;
pub mod theme;
pub mod timeline_panel;
pub mod tools;
pub mod view;

pub use brush::{BrushKind, BrushSettings, PatternShape};
pub use command::Command;
pub use depth_panel::{DepthResponse, depth_panel};
pub use library_panel::{LibraryState, library_panel};
pub use selection::Selection;
pub use style::{DrawStyle, DrawingMode, StrokeKind};
pub use theme::{Metrics, Palette};
pub use timeline_panel::{
    FrameAction, TimelineResponse, TimelineState, TweenRequest, timeline_panel,
};
pub use tools::{ToolId, ToolStatus, all_tools, tool_for_key};
pub use view::{Guide, Orientation, SnapSettings, SnapTarget, Snapped, ViewSettings};
