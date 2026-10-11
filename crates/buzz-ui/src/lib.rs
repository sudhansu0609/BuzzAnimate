//! The BuzzAnimate application shell.
//!
//! Theme, menus, panels and the editor-facing state they operate on. The split
//! from `buzz-app` is deliberate: everything here is testable without opening a
//! window, including the shortcut map, the snapping arithmetic and the panels
//! themselves.

pub mod about;
pub mod actions_panel;
pub mod ai_character_dialog;
pub mod ai_models_dialog;
pub mod ai_panel;
pub mod ai_rename_dialog;
pub mod align;
pub mod arrange_dialog;
pub mod assets_panel;
pub mod brush;
pub mod camera_panel;
pub mod character_panel;
pub mod command;
pub mod command_palette;
pub mod depth_panel;
pub mod export_panel;
pub mod filter_panel;
pub mod font_preview;
pub mod icon_sheet;
pub mod icons;
pub mod joint_widget;
pub mod library_panel;
pub mod light_panel;
pub mod lipsync_panel;
pub mod motion_editor;
pub mod new_document;
pub mod panels;
pub mod recovery_panel;
pub mod rig_panel;
pub mod selection;
pub mod sequence_panel;
pub mod shortcut_editor;
pub mod sound_panel;
pub mod splash;
pub mod staging_panel;
pub mod story_panel;
pub mod style;
pub mod swatch_panel;
pub mod tasks_panel;
pub mod theme;
pub mod timeline_panel;
pub mod title_dialog;
pub mod tools;
pub mod trace_dialog;
pub mod view;
pub mod widgets;
pub mod workspace;

pub use about::{AboutState, about_dialog};
pub use actions_panel::{ActionsResponse, ActionsState, SampleEntry, SavedScript, actions_panel};
pub use ai_character_dialog::{
    AiCharacterDialogAction, AiCharacterState, AiCharacterStep, ai_character_dialog,
};
pub use ai_models_dialog::{AiModelsState, ai_models_dialog, ModelDownloadProgress};
pub use ai_rename_dialog::{AiRenameAction, AiRenameState};
pub use align::{Align, Distribute, MatchSize};
pub use arrange_dialog::{ArrangeAction, ArrangeDialogState, arrange_dialog};
pub use assets_panel::{AssetAction, AssetPanelState, ThumbnailSize, assets_panel};
pub use brush::{BrushKind, BrushSettings, BrushTips, PatternShape};
pub use camera_panel::{CameraPanelState, CameraResponse, camera_hud, camera_panel};
pub use character_panel::{
    CAST_FOLDER, CastAction, CastPanelState, CastThumbnailSource, cast_panel,
};
pub use command::Command;
pub use command_palette::{CommandPaletteState, command_palette};
pub use depth_panel::{DepthResponse, depth_panel};
pub use export_panel::{
    ContainerChoice, DitherChoice, ExportKind, ExportResponse, ExportState, GifOptions,
    PreviewShot, VideoChoice, VideoOptions, WebpOptions, export_dialog,
};
pub use filter_panel::{
    FilterPanelState, FilterResponse, FilterTarget, LayerEffects, filter_panel,
};
pub use font_preview::FontPreviews;
pub use library_panel::{LibraryState, MotionFilter, library_panel};
pub use light_panel::{LightPanelState, LightResponse, light_panel};
pub use lipsync_panel::{Choice, LipSyncResponse, LipSyncState, lip_sync_dialog};
pub use motion_editor::{MotionEditorResponse, MotionEditorState, motion_editor_panel};
pub use new_document::{DocumentSetup, NewDocumentResponse, NewDocumentState, new_document_dialog};
pub use title_dialog::{
    TITLE_POSITIONS, TITLE_PRESETS, TitleCardResponse, TitleCardSettings, TitleCardState,
    title_card_dialog,
};
pub use recovery_panel::{RecoveryChoice, RecoveryEntry, RecoveryState, recovery_dialog};
pub use rig_panel::{DraggedPart, LoosePart, RigPanelState, RigResponse, rig_panel};
pub use selection::{Selection, TimelineSelection, expand_collapsed_folders, layer_range};
pub use sequence_panel::{SequenceResponse, SequenceView, ShotBlock, sequence_panel};
pub use shortcut_editor::{ShortcutEditorState, shortcut_editor};
pub use sound_panel::{SoundChoice, SoundResponse, sound_panel};
pub use splash::{SplashState, opening_scene};
pub use staging_panel::{
    ActionChoice, SettingChoice, StagingDialog, StagingResponse, StagingState, staging_dialog,
};
pub use story_panel::{
    SceneryChoice, SceneryPartChoice, ShotSummary, StoryResponse, StoryState, story_panel,
};
pub use style::{
    DrawStyle, DrawingMode, EraserMode, FillKind, StrokeKind, SymmetryMode, SymmetrySettings,
};
pub use swatch_panel::{SwatchState, swatch_panel};
pub use tasks_panel::{FinishedRow, TaskAction, TaskRow, TasksView, tasks_panel};
pub use theme::{Metrics, Palette};
pub use timeline_panel::{
    FrameAction, MoveFrames, TimelineResponse, TimelineState, TweenRequest, Waveform,
    frame_move_targets, timeline_panel,
    timeline_panel_with_tools_slot,
};
pub use tools::{ToolId, ToolStatus, all_tools, tool_for_key};
pub use trace_dialog::{TraceDialogAction, trace_dialog};
pub use view::{
    Guide, Orientation, PerspectiveGuides, SnapSettings, SnapTarget, Snapped, ViewSettings,
};
pub use workspace::{
    AiSettings, BoneMode, Dock, GroupId, KeyChord, PanelId, RigOptions, Section, Slot, Workspace, command_id,
};
