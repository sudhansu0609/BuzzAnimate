//! Editor commands and their shortcuts.
//!
//! Menus, keyboard shortcuts and toolbar buttons all raise the same
//! [`Command`]. Keeping one enumeration means a menu item and its shortcut can
//! never drift apart, and the whole key map can be tested without a window.
//!
//! Shortcuts follow Animate's defaults. Where a user would reach for a key out
//! of habit, it does what they expect.

use egui::{Key, KeyboardShortcut, Modifiers};

/// Everything the editor can be asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    // File
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Quit,
    /// Import an Animate document's symbols into the library.
    ImportToLibrary,
    /// Import an Animate document onto the stage as well as the library.
    ImportToStage,
    /// Write the document back out as an Adobe Animate `.fla`.
    ///
    /// Import was one-way, which makes this somewhere work goes to be finished
    /// rather than a place in a pipeline. A studio, a client or a shelf of
    /// Animate tooling all need the file to be able to go back.
    ExportFla,
    /// Write the current frame out as a PNG.
    ExportImage,
    /// Write a numbered PNG for every frame in a range.
    ExportSequence,
    /// Render the film as an MP4 or MOV. CP-6.2.
    ExportVideo,
    /// Render the film as an animated GIF. CP-6.3.
    ExportGif,
    /// Render the film as an animated WebP. CP-6.3.
    ExportWebp,
    /// Bring a sound file into the library.
    ImportSound,
    /// Bring a bitmap onto the stage, already broken apart into artwork.
    ImportImage,
    /// Put the library's selected sound on the current keyframe.
    AttachSound,
    /// Take the sound off the current keyframe.
    RemoveSound,
    /// Open the Lip Sync dialog for the document's soundtrack.
    LipSync,
    /// Make a mouth symbol with a frame per shape, to draw over.
    NewMouthSymbol,
    /// Mark the soundtrack's beats on the timeline ruler.
    DetectBeats,
    /// **Lay the timeline out to the narration**: find where the voice-over
    /// speaks and where it breathes, stretch the film to cover it, and put a
    /// keyframe at the start of every line. See `buzz_audio::detect_phrases`.
    FitToNarration,
    /// **Read a subtitle file onto the timeline**: a caption layer with the
    /// words keyed to their own timecodes. See `buzz_doc::srt`.
    ImportCaptions,
    /// **Write the caption layer back out as `.srt`**, to upload beside the
    /// film.
    ExportCaptions,

    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    Deselect,
    DuplicateSelection,

    // View
    ZoomIn,
    ZoomOut,
    ZoomActual,
    ZoomFitInWindow,
    ZoomShowFrame,
    ZoomShowAll,
    ToggleRulers,
    ToggleGrid,
    ToggleGuides,
    ToggleSnapping,
    TogglePasteboard,

    // Modify
    GroupSelection,
    UngroupSelection,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    /// **Retarget a performance**: copy one rig's poses onto another with the
    /// same skeleton, so one walk drives a whole cast.
    RetargetPerformance,
    /// **Swap symbol**: point every instance of one symbol at another, keeping
    /// where each stands and how big it is.
    SwapSymbol,
    /// **Ink and paint**: carry this frame's bucket fills onto every keyframe
    /// after it, seeded where each colour already sits.
    PaintThrough,
    /// **On twos**: hold each drawing for two frames over the selected span,
    /// which halves the drawings a shot needs.
    ExposeOnTwos,
    /// The same, on threes.
    ExposeOnThrees,
    /// **Select same colour**: everything on this frame painted the colour the
    /// selection is painted.
    SelectSameColour,
    /// **Thicken the outlines** of everything selected, leaving the fills and
    /// the shapes themselves alone.
    ///
    /// A step rather than a number, because weighting a drawing is something an
    /// animator does *by eye against the rest of the drawing* — press it until
    /// it looks right — and stopping to type a width in a box breaks exactly
    /// the comparison being made. Multiplicative rather than additive, so one
    /// press means the same thing on a hairline and on a heavy outline.
    ThickenStroke,
    /// The same, thinner.
    ThinStroke,
    /// **Trace a bitmap into shapes** — Animate's Modify ▸ Bitmap ▸ Trace
    /// Bitmap. The picture is replaced by the artwork it becomes, so one undo
    /// puts the photograph back. See `buzz_scene::trace`.
    TraceBitmap,
    /// The same, tuned for **a scan of a drawing**: ink and paper, and the
    /// paper thrown away, so what is left is outlines you can paint inside.
    TraceLineArt,
    ConvertLinesToFills,
    ExpandFill,
    SmoothSelection,
    StraightenSelection,
    /// Modify ▸ Shape ▸ Recognise Shape: turn a drawn wobble into the circle,
    /// rectangle or line it was meant to be.
    RecogniseShape,
    /// Modify ▸ Transform ▸ Flip Horizontal.
    FlipHorizontal,
    /// Modify ▸ Transform ▸ Flip Vertical.
    FlipVertical,
    /// Modify ▸ Transform ▸ Rotate 90° Clockwise.
    RotateClockwise,
    /// Modify ▸ Transform ▸ Rotate 90° Counter-clockwise.
    RotateAnticlockwise,

    // Insert
    NewLayer,
    NewLayerFolder,
    /// A guide layer for reference art (rotoscoping): shown faded, never
    /// exported. Import an image onto it to trace over.
    NewReferenceLayer,
    /// **Import a video to trace over**: one frame of it per frame of the film,
    /// on a guide layer.
    ImportVideoReference,
    /// **Import an image sequence**: a folder of numbered pictures, one drawing
    /// to a frame.
    ImportSequenceFolder,
    DeleteLayer,

    // Symbols and library
    /// Wrap the selection in a new symbol, replacing it with an instance.
    ConvertToSymbol,
    /// Create an empty symbol and open it for editing.
    NewSymbol,
    /// Open the selected instance's symbol for editing.
    EditSymbol,
    /// Leave symbol editing and return to the main timeline.
    EditDocument,
    /// Place an instance of the library selection on the stage.
    PlaceInstance,
    DuplicateSymbol,
    DeleteSymbol,
    NewLibraryFolder,

    /// Adopt the selected artwork as the pattern brush's shape.
    BrushFromSelection,

    // Workspace
    /// Show or hide a panel.
    TogglePanel(super::workspace::PanelId),
    /// Stop the layout being rearranged by accident.
    ToggleLayoutLock,
    /// Step to the next interface theme, wrapping. The keyboard's way in;
    /// the Window menu offers them by name instead.
    ToggleTheme,
    /// Use this interface theme.
    SetTheme(super::theme::Theme),
    /// Help ▸ About BuzzAnimate.
    About,
    /// Put every panel back where it started.
    ResetWorkspace,
    /// Keep the document as it is now under a name, to come back to.
    SaveSnapshot,
    /// Open the list of saved snapshots, to restore one.
    Snapshots,
    /// Open the keyboard-shortcut editor.
    ShortcutEditor,

    // Lighting
    /// Add a sun: one direction for the whole stage.
    AddSun,
    /// Add a sky: ambient fill, casting nothing.
    AddSky,
    /// Add a lamp: a point on the stage, with falloff.
    AddLamp,
    /// Add a gloom: a wide wall of darkness thrown across the stage, aimed
    /// against whatever light is already there.
    AddGloom,
    /// Add a fire: a lamp that gutters, in the colour of a hearth.
    AddFire,
    /// Add a storm: a dark sky that strikes, every few seconds, for ever.
    AddStorm,
    /// Show or hide the light handles on the stage.
    ToggleLightGizmos,

    // Staging and performance
    /// **Set a scene**: ground, backdrop, lights and a cast standing in it.
    ///
    /// Not a template and not clip art — the arrangement, which is the part
    /// that is arithmetic rather than drawing. See `buzz_act::staging`.
    SetScene,
    /// **Direct a story**: a few lines of prose in, a staged scene with the
    /// walks and talks already on the timeline. See `buzz_act::direct`.
    DirectScene,
    /// A new, empty scene after this one — the next shot of the film.
    AddScene,
    /// A complete copy of this scene, opened for editing. What the next beat
    /// of a conversation starts from.
    DuplicateScene,
    /// Put one more rigged person on the stage, on a layer of their own.
    AddPerson,
    /// **Animate the selected rig**: a walk, a run, talking, or standing and
    /// breathing, written onto the timeline as ordinary poses.
    Perform,
    /// **Add follow-through**: bake a damped-spring response of a chosen chain
    /// (hair, a tail) to the selected rig's keyed motion. See `buzz_act::physics`.
    AddFollowThrough,
    /// **Add wiggle**: bake a deterministic jitter onto the selected object — an
    /// idle sway, a breeze, a handheld shake. See `buzz_act::physics`.
    AddWiggle,
    /// **Clear modifiers**: remove all live modifiers from the selected object.
    ClearModifiers,
    /// **Bake modifiers**: evaluate the selected object's live modifiers across
    /// the film into keyframes, then remove them — live becomes editable.
    BakeModifiers,
    /// **Set reverse drawing**: make the second selected object the back view of
    /// the first, shown when it is turned to face away.
    SetReverse,
    /// **Clear reverse drawing**: remove the selected object's whole turnaround.
    ClearReverse,
    /// **Add a profile view**: make the second selected object the drawing shown
    /// when the first is turned a quarter turn to its right.
    AddProfileRight,
    /// The same, a quarter turn to its left.
    AddProfileLeft,
    /// **Add a three-quarter view**: the drawing shown part way between the
    /// front and a profile, which is where most acting happens.
    AddThreeQuarterRight,
    /// The same, on the other side.
    AddThreeQuarterLeft,

    // Commands (scripting)
    /// Show or hide the Actions panel.
    ToggleActionsPanel,
    /// Run what is in the Actions panel against this document.
    RunScript,
    /// Empty the Actions panel's Output area.
    ClearScriptOutput,

    // Tweens
    CreateClassicTween,
    CreateMotionTween,
    CreateShapeTween,
    RemoveTween,

    // Timeline
    InsertFrame,
    RemoveFrame,
    InsertKeyframe,
    InsertBlankKeyframe,
    ClearKeyframe,
    PlayPause,
    NextFrame,
    PreviousFrame,
    FirstFrame,
    LastFrame,
    /// Animate's Cut Frames.
    CutFrames,
    /// Animate's Copy Frames.
    CopyFrames,
    /// Animate's Paste Frames.
    PasteFrames,
    /// Empty the frames here, keeping the span — Animate's Clear Frames.
    ClearFrames,
    /// Play this layer's keyframes back to front.
    ReverseFrames,
    ToggleOnionSkin,
    ToggleAutoKeyframe,
    ToggleEditMultipleFrames,

    // Camera
    ToggleCamera,
    AddCameraKeyframe,
    RemoveCameraKeyframe,
    ResetCamera,
    /// **Write a named camera move** from the playhead: a push in, a pan, a
    /// reveal, a drift. Two ordinary eased camera keys, which is what the
    /// animator would have typed. See `buzz_scene::CameraMove`.
    ///
    /// The move is carried on the command rather than read from a panel,
    /// for the same reason `Align` carries `to_stage`: which move it is *is*
    /// the operation, and a command that did a different thing depending on
    /// hidden state is a command you cannot predict.
    AddCameraMove(buzz_scene::CameraMove),

    // Lights
    /// Key the selected light's current state at the playhead.
    AddLightKeyframe,
    /// Remove the selected light's key at the playhead.
    RemoveLightKeyframe,

    // Tools
    SelectTool(super::tools::ToolId),

    /// Keep this document as a starting point for the next film.
    SaveAsTemplate,
    /// Start a document from a saved template. The index is into the list the
    /// menu was drawn from, which is the same list the editor holds.
    NewFromTemplate(usize),

    /// Line the selection up. Animate's Modify ▸ Align.
    ///
    /// `to_stage` is carried on the command rather than read from a panel
    /// toggle, because it changes what the operation *means* — towards each
    /// other, or towards the frame — and a command that did one or the other
    /// depending on hidden state is a command you cannot predict.
    Align { op: super::align::Align, to_stage: bool },
    Distribute(super::align::Distribute),
    MatchSize(super::align::MatchSize),

    /// Move the selection by whole document units, from the arrow keys.
    ///
    /// Carried as one command with a vector rather than four commands, because
    /// the four are the same action and a menu never shows any of them: this is
    /// a key, and the key says which way.
    Nudge { x: i32, y: i32 },
}

/// How far one arrow press moves the selection, and how far with Shift.
///
/// Animate's numbers. One unit is "line it up"; eight is "get it roughly
/// there", and having both is what stops the arrow keys being useless at one
/// end and imprecise at the other.
pub const NUDGE_STEP: i32 = 1;
pub const NUDGE_STEP_LARGE: i32 = 8;

impl Command {
    /// Label as it appears in a menu.
    pub fn label(self) -> &'static str {
        use Command::*;
        match self {
            New => "New",
            Open => "Open…",
            Save => "Save",
            SaveAs => "Save As…",
            Close => "Close",
            Quit => "Exit",
            ImportToLibrary => "Import to Library…",
            ImportToStage => "Import to Stage…",
            ExportFla => "Export Animate Document (.fla)…",
            ExportImage => "Export Image…",
            ExportSequence => "Export PNG Sequence…",
            ExportVideo => "Export Video…",
            ExportGif => "Export GIF…",
            ExportWebp => "Export WebP…",
            ImportSound => "Import Sound…",
            ImportImage => "Import Image…",
            AttachSound => "Attach Sound to Frame",
            RemoveSound => "Remove Sound from Frame",
            LipSync => "Lip Sync…",
            NewMouthSymbol => "New Mouth Symbol",
            DetectBeats => "Detect Beats",
            FitToNarration => "Fit to Narration",
            ImportCaptions => "Import Captions (.srt)\u{2026}",
            ExportCaptions => "Export Captions (.srt)\u{2026}",

            Undo => "Undo",
            Redo => "Redo",
            Cut => "Cut",
            Copy => "Copy",
            Paste => "Paste",
            Delete => "Delete",
            SelectAll => "Select All",
            Deselect => "Deselect All",
            DuplicateSelection => "Duplicate",

            ZoomIn => "Zoom In",
            ZoomOut => "Zoom Out",
            ZoomActual => "100%",
            ZoomFitInWindow => "Fit in Window",
            ZoomShowFrame => "Show Frame",
            ZoomShowAll => "Show All",
            ToggleRulers => "Rulers",
            ToggleGrid => "Grid",
            ToggleGuides => "Guides",
            ToggleSnapping => "Snap to Objects",
            TogglePasteboard => "Pasteboard",

            GroupSelection => "Group",
            UngroupSelection => "Ungroup",
            BringToFront => "Bring to Front",
            BringForward => "Bring Forward",
            SendBackward => "Send Backward",
            SendToBack => "Send to Back",
            RetargetPerformance => "Retarget Performance",
            SwapSymbol => "Swap Symbol",
            PaintThrough => "Paint Through",
            ExposeOnTwos => "On Twos",
            ExposeOnThrees => "On Threes",
            SelectSameColour => "Select Same Colour",
            ThickenStroke => "Thicken Lines",
            ThinStroke => "Thin Lines",
            TraceBitmap => "Trace Bitmap",
            TraceLineArt => "Trace as Line Art",
            ConvertLinesToFills => "Convert Lines to Fills",
            ExpandFill => "Expand Fill…",
            SmoothSelection => "Smooth",
            StraightenSelection => "Straighten",
            RecogniseShape => "Recognise Shape",
            FlipHorizontal => "Flip Horizontal",
            FlipVertical => "Flip Vertical",
            RotateClockwise => "Rotate 90\u{b0} CW",
            RotateAnticlockwise => "Rotate 90\u{b0} CCW",

            NewLayer => "New Layer",
            NewLayerFolder => "New Folder",
            NewReferenceLayer => "New Reference Layer",
            ImportVideoReference => "Import Video Reference\u{2026}",
            ImportSequenceFolder => "Import Image Sequence\u{2026}",
            DeleteLayer => "Delete Layer",

            ConvertToSymbol => "Convert to Symbol…",
            NewSymbol => "New Symbol…",
            EditSymbol => "Edit Symbol",
            EditDocument => "Edit Document",
            PlaceInstance => "Place on Stage",
            DuplicateSymbol => "Duplicate Symbol",
            DeleteSymbol => "Delete Symbol",
            NewLibraryFolder => "New Library Folder",

            BrushFromSelection => "Create Brush From Selection",

            TogglePanel(_) => "Panel",
            ToggleLayoutLock => "Lock Layout",
            ToggleTheme => "Next Theme",
            SetTheme(_) => "Theme",
            About => "About BuzzAnimate",
            ResetWorkspace => "Reset Layout",
            SaveSnapshot => "Save Snapshot",
            Snapshots => "Snapshots\u{2026}",
            ShortcutEditor => "Keyboard Shortcuts\u{2026}",

            AddSun => "Sun",
            AddSky => "Sky",
            AddLamp => "Lamp",
            AddGloom => "Gloom",
            AddFire => "Fire",
            AddStorm => "Storm",
            ToggleLightGizmos => "Light Handles",

            SetScene => "Set the Scene\u{2026}",
            DirectScene => "Direct a Story\u{2026}",
            AddScene => "Add Scene",
            DuplicateScene => "Duplicate Scene",
            AddPerson => "Add Person",
            Perform => "Animate Selection\u{2026}",
            AddFollowThrough => "Add Follow-Through\u{2026}",
            AddWiggle => "Add Wiggle\u{2026}",
            ClearModifiers => "Clear Modifiers",
            BakeModifiers => "Bake Modifiers",
            SetReverse => "Set Reverse Drawing",
            ClearReverse => "Clear Turnaround",
            AddProfileRight => "Add Profile (right)",
            AddProfileLeft => "Add Profile (left)",
            AddThreeQuarterRight => "Add Three-Quarter (right)",
            AddThreeQuarterLeft => "Add Three-Quarter (left)",

            ToggleActionsPanel => "Actions",
            RunScript => "Run Script",
            ClearScriptOutput => "Clear Output",

            CreateClassicTween => "Create Classic Tween",
            CreateMotionTween => "Create Motion Tween",
            CreateShapeTween => "Create Shape Tween",
            RemoveTween => "Remove Tween",

            InsertFrame => "Insert Frame",
            RemoveFrame => "Remove Frame",
            InsertKeyframe => "Insert Keyframe",
            InsertBlankKeyframe => "Insert Blank Keyframe",
            ClearKeyframe => "Clear Keyframe",
            PlayPause => "Play/Pause",
            NextFrame => "Next Frame",
            PreviousFrame => "Previous Frame",
            FirstFrame => "First Frame",
            LastFrame => "Last Frame",
            CutFrames => "Cut Frames",
            CopyFrames => "Copy Frames",
            PasteFrames => "Paste Frames",
            ClearFrames => "Clear Frames",
            ReverseFrames => "Reverse Frames",
            ToggleOnionSkin => "Onion Skin",
            ToggleAutoKeyframe => "Auto Keyframe",
            ToggleEditMultipleFrames => "Edit Multiple Frames",

            ToggleCamera => "Enable Camera",
            AddCameraKeyframe => "Add Camera Keyframe",
            RemoveCameraKeyframe => "Remove Camera Keyframe",
            AddLightKeyframe => "Add Light Keyframe",
            RemoveLightKeyframe => "Remove Light Keyframe",
            ResetCamera => "Reset Camera",
            // The move names itself; a second label here would be a
            // second place to keep the wording in step.
            AddCameraMove(m) => m.label(),

            SelectTool(_) => "Tool",
            Nudge { .. } => "Nudge",
            SaveAsTemplate => "Save as Template",
            NewFromTemplate(_) => "New from Template",
            Align { op, .. } => op.label(),
            Distribute(op) => op.label(),
            MatchSize(op) => op.label(),
        }
    }

    /// Animate's default shortcut, if it has one.
    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        use Command::*;
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL.plus(Modifiers::SHIFT);
        let ctrl_alt = Modifiers::CTRL.plus(Modifiers::ALT);

        let sc = |m: Modifiers, k: Key| Some(KeyboardShortcut::new(m, k));

        match self {
            New => sc(ctrl, Key::N),
            Open => sc(ctrl, Key::O),
            Save => sc(ctrl, Key::S),
            SaveAs => sc(ctrl_shift, Key::S),
            Close => sc(ctrl, Key::W),
            Quit => sc(ctrl, Key::Q),
            // Animate's import bindings.
            ImportToStage => sc(ctrl, Key::R),
            ImportToLibrary => sc(ctrl_shift, Key::R),
            // Animate has no default binding for Export Image, and inventing
            // one risks colliding with a habit rather than serving it.
            ExportFla => None,
            ExportImage => None,
            ExportSequence => None,
            ExportVideo => None,
            ExportGif => None,
            ExportWebp => None,
            // Animate has no default binding for any of these.
            ImportSound | ImportImage | AttachSound | RemoveSound | LipSync | NewMouthSymbol => {
                None
            }

            Undo => sc(ctrl, Key::Z),
            // Animate accepts both; Ctrl+Y is also handled by the key map.
            Redo => sc(ctrl_shift, Key::Z),
            Cut => sc(ctrl, Key::X),
            Copy => sc(ctrl, Key::C),
            Paste => sc(ctrl, Key::V),
            Delete => sc(Modifiers::NONE, Key::Delete),
            SelectAll => sc(ctrl, Key::A),
            Deselect => sc(ctrl_shift, Key::A),
            DuplicateSelection => sc(ctrl, Key::D),

            ZoomIn => sc(ctrl, Key::Equals),
            ZoomOut => sc(ctrl, Key::Minus),
            ZoomActual => sc(ctrl, Key::Num1),
            ZoomFitInWindow => sc(ctrl, Key::Num3),
            ZoomShowFrame => sc(ctrl, Key::Num2),
            ZoomShowAll => sc(ctrl_shift, Key::W),
            // Animate puts rulers on Ctrl+Alt+Shift+R, not Ctrl+Shift+R —
            // the latter is Import to Library.
            ToggleRulers => sc(ctrl_shift.plus(Modifiers::ALT), Key::R),
            ToggleGrid => sc(ctrl, Key::Quote),
            ToggleGuides => sc(ctrl_semicolon(), Key::Semicolon),
            ToggleSnapping => sc(ctrl_shift, Key::U),
            TogglePasteboard => sc(ctrl_shift, Key::W),

            GroupSelection => sc(ctrl, Key::G),
            UngroupSelection => sc(ctrl_shift, Key::G),
            BringToFront => sc(ctrl_shift, Key::ArrowUp),
            BringForward => sc(ctrl, Key::ArrowUp),
            SendBackward => sc(ctrl, Key::ArrowDown),
            SendToBack => sc(ctrl_shift, Key::ArrowDown),
            ExpandFill => None,
            // **The bracket keys**, as every paint program binds them for the
            // brush size. This is the same gesture aimed at a drawing that is
            // already down: press until the line weight looks right against
            // the rest of the picture. Animate binds neither.
            ThickenStroke => sc(Modifiers::NONE, Key::CloseBracket),
            ThinStroke => sc(Modifiers::NONE, Key::OpenBracket),
            TraceBitmap | TraceLineArt => None,
            ConvertLinesToFills => None,
            SmoothSelection => None,
            RecogniseShape => None,
            StraightenSelection => None,
            // Animate's own: Ctrl+Shift+9 and Ctrl+Shift+7 rotate; the flips
            // have no default there, and none is invented here.
            FlipHorizontal | FlipVertical => None,
            RotateClockwise => sc(ctrl_shift, Key::Num9),
            RotateAnticlockwise => sc(ctrl_shift, Key::Num7),

            NewLayer => sc(ctrl_alt, Key::N),
            NewLayerFolder => sc(ctrl_alt, Key::F),
            DeleteLayer => None,

            // F8 and Ctrl+F8 are muscle memory for anyone who has used
            // Animate; Ctrl+E steps in and out of symbol editing.
            ConvertToSymbol => sc(Modifiers::NONE, Key::F8),
            NewSymbol => sc(ctrl, Key::F8),
            EditSymbol => sc(ctrl, Key::E),
            EditDocument => sc(ctrl, Key::F4),
            PlaceInstance => None,
            DuplicateSymbol => None,
            DeleteSymbol => None,
            NewLibraryFolder => None,

            BrushFromSelection => None,

            // Adding a light is a deliberate, occasional act; the handles get
            // a key because they are turned off to see the picture clean and
            // straight back on to keep working.
            TogglePanel(_) | ResetWorkspace => None,
            // Animate has no shortcut for this; it is a settle-down-and-work
            // action, and Ctrl+Alt+L is free here.
            ToggleLayoutLock => sc(ctrl.plus(Modifiers::ALT), Key::L),
            ToggleTheme => None,
            SetTheme(_) => None,
            About => None,
            SaveSnapshot | Snapshots => None,
            ShortcutEditor => None,
            DetectBeats => None,
            FitToNarration => None,
            ImportCaptions | ExportCaptions => None,
            NewReferenceLayer | ImportVideoReference | SelectSameColour
            | ExposeOnTwos | ExposeOnThrees | PaintThrough | ImportSequenceFolder
            | RetargetPerformance | SwapSymbol => None,

            AddSun | AddSky | AddLamp | AddGloom | AddFire | AddStorm => None,
            // No Animate binding to follow, and these open dialogs rather than
            // acting straight away, so a key would only save the menu.
            SetScene | DirectScene | AddScene | DuplicateScene | AddPerson | Perform
            | AddFollowThrough | AddWiggle | ClearModifiers | BakeModifiers | SetReverse
            | ClearReverse | AddProfileRight | AddProfileLeft | AddThreeQuarterRight
            | AddThreeQuarterLeft => None,
            ToggleLightGizmos => sc(ctrl_shift, Key::L),

            // F9 is Animate's own Actions panel key on Windows.
            ToggleActionsPanel => sc(Modifiers::NONE, Key::F9),
            // Ctrl+Enter is what every code editor runs on, and plain Enter is
            // already Animate's Play/Pause — which a script author would hit
            // constantly by accident if running were bound to it.
            RunScript => sc(ctrl, Key::Enter),
            ClearScriptOutput => None,

            CreateClassicTween => None,
            CreateMotionTween => None,
            CreateShapeTween => None,
            RemoveTween => None,

            // Animate's frame shortcuts, which animators use constantly.
            InsertFrame => sc(Modifiers::NONE, Key::F5),
            RemoveFrame => sc(Modifiers::SHIFT, Key::F5),
            InsertKeyframe => sc(Modifiers::NONE, Key::F6),
            InsertBlankKeyframe => sc(Modifiers::NONE, Key::F7),
            ClearKeyframe => sc(Modifiers::SHIFT, Key::F6),
            PlayPause => sc(Modifiers::NONE, Key::Enter),
            NextFrame => sc(Modifiers::NONE, Key::Period),
            PreviousFrame => sc(Modifiers::NONE, Key::Comma),
            FirstFrame => sc(Modifiers::NONE, Key::Home),
            LastFrame => sc(Modifiers::NONE, Key::End),
            // Animate's own keys for the frame clipboard.
            CutFrames => sc(ctrl_alt, Key::X),
            CopyFrames => sc(ctrl_alt, Key::C),
            PasteFrames => sc(ctrl_alt, Key::V),
            ClearFrames => sc(Modifiers::ALT, Key::Backspace),
            ReverseFrames => None,
            ToggleOnionSkin => None,
            ToggleAutoKeyframe => None,
            ToggleEditMultipleFrames => None,

            ToggleCamera => None,
            AddCameraKeyframe => None,
            RemoveCameraKeyframe => None,
            AddLightKeyframe => None,
            RemoveLightKeyframe => None,
            ResetCamera => None,
            AddCameraMove(_) => None,

            SelectTool(_) => None,
            // The arrow keys, read directly: four directions times two step
            // sizes is eight bindings for one action, and none of them belongs
            // in a menu.
            Nudge { .. } => None,
            // Thirteen operations behind one menu. Animate gives the panel a
            // key, not the operations.
            Align { .. } | Distribute(_) | MatchSize(_) => None,
            // Reached from the File menu; the templates one needs a name, and
            // a keystroke cannot carry one.
            SaveAsTemplate | NewFromTemplate(_) => None,
        }
    }

    /// Does this command need something selected?
    ///
    /// Used to grey out menu items, which is how a user learns what a tool
    /// expects.
    pub fn needs_selection(self) -> bool {
        use Command::*;
        matches!(
            self,
            Cut | Copy
                | Delete
                | DuplicateSelection
                | GroupSelection
                | UngroupSelection
                | BringToFront
                | BringForward
                | SendBackward
                | SendToBack
                | ConvertLinesToFills
                | ExpandFill
                | SmoothSelection
                | RecogniseShape
                | StraightenSelection
                | ConvertToSymbol
                | BrushFromSelection
                | ClearModifiers
                | BakeModifiers
                | SetReverse
                | ClearReverse
                | Nudge { .. }
                | Align { .. }
                | Distribute(_)
                | MatchSize(_)
        )
    }
}

fn ctrl_semicolon() -> Modifiers {
    Modifiers::CTRL
}

/// Every command that carries a keyboard shortcut.
///
/// Exposed so the application can assert that each one is actually bound: a
/// shortcut in this map is a *promise*, and the code that reads the keyboard
/// is a separate list that has to keep it.
pub fn all_with_shortcuts() -> Vec<Command> {
    palette_commands()
        .into_iter()
        .filter(|c| c.shortcut().is_some())
        .collect()
}

/// Every command worth offering by name — the whole menu catalogue, in menu
/// order — for the command palette. The data-carrying commands (a specific
/// tool, panel or template) are reached other ways and left out.
pub fn palette_commands() -> Vec<Command> {
    use Command::*;
    vec![
        New,
        Open,
        Save,
        SaveAs,
        Close,
        Quit,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Delete,
        SelectAll,
        Deselect,
        DuplicateSelection,
        ZoomIn,
        ZoomOut,
        ZoomActual,
        ZoomFitInWindow,
        ZoomShowFrame,
        ZoomShowAll,
        ToggleRulers,
        ToggleGrid,
        ToggleGuides,
        ToggleSnapping,
        TogglePasteboard,
        GroupSelection,
        UngroupSelection,
        BringToFront,
        BringForward,
        SendBackward,
        SendToBack,
        ThickenStroke,
        ThinStroke,
        TraceBitmap,
        TraceLineArt,
        ConvertLinesToFills,
        ExpandFill,
        SmoothSelection,
        StraightenSelection,
        RecogniseShape,
        FlipHorizontal,
        FlipVertical,
        RotateClockwise,
        RotateAnticlockwise,
        NewLayer,
        NewLayerFolder,
        NewReferenceLayer,
        DeleteLayer,
        InsertFrame,
        RemoveFrame,
        InsertKeyframe,
        InsertBlankKeyframe,
        ClearKeyframe,
        PlayPause,
        NextFrame,
        PreviousFrame,
        FirstFrame,
        LastFrame,
        CutFrames,
        CopyFrames,
        PasteFrames,
        ClearFrames,
        ReverseFrames,
        ToggleOnionSkin,
        ToggleAutoKeyframe,
        ToggleEditMultipleFrames,
        ToggleCamera,
        AddCameraKeyframe,
        RemoveCameraKeyframe,
        AddLightKeyframe,
        RemoveLightKeyframe,
        ResetCamera,
        ImportToLibrary,
        ImportToStage,
        ConvertToSymbol,
        NewSymbol,
        EditSymbol,
        EditDocument,
        PlaceInstance,
        DuplicateSymbol,
        DeleteSymbol,
        NewLibraryFolder,
        CreateClassicTween,
        CreateMotionTween,
        CreateShapeTween,
        RemoveTween,
        BrushFromSelection,
        AddSun,
        AddSky,
        AddLamp,
        AddGloom,
        AddFire,
        AddStorm,
        ToggleLightGizmos,
        SetScene,
        DirectScene,
        AddScene,
        DuplicateScene,
        AddPerson,
        Perform,
        AddFollowThrough,
        AddWiggle,
        ClearModifiers,
        BakeModifiers,
        SetReverse,
        ClearReverse,
        ToggleLayoutLock,
        ToggleTheme,
        About,
        SaveSnapshot,
        Snapshots,
        ShortcutEditor,
        ToggleActionsPanel,
        RunScript,
        ClearScriptOutput,
        ExportFla,
        ExportImage,
        ExportSequence,
        ExportVideo,
        ExportGif,
        ExportWebp,
        ImportSound,
        ImportImage,
        AttachSound,
        RemoveSound,
        LipSync,
        NewMouthSymbol,
        DetectBeats,
        FitToNarration,
        ImportCaptions,
        ExportCaptions,
    ]
}

/// Format a command's built-in shortcut the way a menu shows it.
pub fn shortcut_text(ctx: &egui::Context, command: Command) -> String {
    format_shortcut(ctx, command.shortcut())
}

/// Format an already-resolved shortcut (which may be the user's override or
/// `None`) the way a menu shows it.
pub fn format_shortcut(ctx: &egui::Context, shortcut: Option<egui::KeyboardShortcut>) -> String {
    shortcut.map(|s| ctx.format_shortcut(&s)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_palette_lists_real_commands_with_labels() {
        let palette = palette_commands();
        assert!(palette.len() > 50, "the palette should list the whole catalogue");
        // Every listed command has a non-empty, non-generic label.
        for c in &palette {
            assert!(!c.label().is_empty(), "{c:?} has no label");
        }
        // A few landmarks a user would search for are present.
        assert!(palette.contains(&Command::Save));
        assert!(palette.contains(&Command::PlayPause));
        assert!(palette.contains(&Command::ExportVideo));
        // Every shortcut-bearing command is a subset of the palette.
        for c in all_with_shortcuts() {
            assert!(palette.contains(&c), "{c:?} has a shortcut but is not in the palette");
        }
    }

    /// Every command with a shortcut, for exhaustive checks.
    fn all_commands() -> Vec<Command> {
        use Command::*;
        vec![
            New,
            Open,
            Save,
            SaveAs,
            Close,
            Quit,
            Undo,
            Redo,
            Cut,
            Copy,
            Paste,
            Delete,
            SelectAll,
            Deselect,
            DuplicateSelection,
            ZoomIn,
            ZoomOut,
            ZoomActual,
            ZoomFitInWindow,
            ZoomShowFrame,
            ZoomShowAll,
            ToggleRulers,
            ToggleGrid,
            ToggleGuides,
            ToggleSnapping,
            TogglePasteboard,
            GroupSelection,
            UngroupSelection,
            BringToFront,
            BringForward,
            SendBackward,
            SendToBack,
            ThickenStroke,
            ThinStroke,
            TraceBitmap,
            TraceLineArt,
            ConvertLinesToFills,
            ExpandFill,
            SmoothSelection,
            StraightenSelection,
            RecogniseShape,
            FlipHorizontal,
            FlipVertical,
            RotateClockwise,
            RotateAnticlockwise,
            NewLayer,
            NewLayerFolder,
            DeleteLayer,
            InsertFrame,
            RemoveFrame,
            InsertKeyframe,
            InsertBlankKeyframe,
            ClearKeyframe,
            PlayPause,
            NextFrame,
            PreviousFrame,
            FirstFrame,
            LastFrame,
            CutFrames,
            CopyFrames,
            PasteFrames,
            ClearFrames,
            ReverseFrames,
            ToggleOnionSkin,
            ToggleAutoKeyframe,
            ToggleEditMultipleFrames,
            ToggleCamera,
            AddCameraKeyframe,
            RemoveCameraKeyframe,
            AddLightKeyframe,
            RemoveLightKeyframe,
            ResetCamera,
            ImportToLibrary,
            ImportToStage,
            ConvertToSymbol,
            NewSymbol,
            EditSymbol,
            EditDocument,
            PlaceInstance,
            DuplicateSymbol,
            DeleteSymbol,
            NewLibraryFolder,
            CreateClassicTween,
            CreateMotionTween,
            CreateShapeTween,
            RemoveTween,
            BrushFromSelection,
            AddSun,
            AddSky,
            AddLamp,
            AddGloom,
            AddFire,
            AddStorm,
            ToggleLightGizmos,
            SetScene,
            DirectScene,
            AddScene,
            DuplicateScene,
            AddPerson,
            Perform,
            AddFollowThrough,
            AddWiggle,
            ClearModifiers,
            BakeModifiers,
            SetReverse,
            ClearReverse,
            ToggleLayoutLock,
            ToggleTheme,
            About,
            ToggleActionsPanel,
            RunScript,
            ClearScriptOutput,
            ExportImage,
            ExportSequence,
            ExportVideo,
            ExportGif,
            ExportWebp,
            ImportSound,
            ImportImage,
            ImportImage,
            AttachSound,
            RemoveSound,
            LipSync,
            NewMouthSymbol,
            DetectBeats,
            FitToNarration,
            ImportCaptions,
            ExportCaptions,
        ]
    }

    #[test]
    fn every_command_has_a_label() {
        for c in all_commands() {
            assert!(!c.label().is_empty(), "{c:?} has no label");
        }
    }

    /// Two menu items sharing a shortcut means one of them silently never
    /// fires, which is very hard to notice by hand.
    #[test]
    fn shortcuts_do_not_collide() {
        let mut seen: HashMap<String, Command> = HashMap::new();
        for c in all_commands() {
            let Some(sc) = c.shortcut() else { continue };
            let key = format!("{:?}+{:?}", sc.modifiers, sc.logical_key);
            if let Some(previous) = seen.insert(key.clone(), c) {
                // TogglePasteboard and ZoomShowAll deliberately share a binding
                // in Animate; anything else is a mistake.
                let allowed = matches!(
                    (previous, c),
                    (Command::ZoomShowAll, Command::TogglePasteboard)
                        | (Command::TogglePasteboard, Command::ZoomShowAll)
                );
                assert!(allowed, "{previous:?} and {c:?} both bind {key}");
            }
        }
    }

    /// Animators use these dozens of times a minute; they must match Animate.
    #[test]
    fn the_frame_shortcuts_match_animate() {
        let expect = |c: Command, m: Modifiers, k: Key| {
            let sc = c
                .shortcut()
                .unwrap_or_else(|| panic!("{c:?} has no shortcut"));
            assert_eq!((sc.modifiers, sc.logical_key), (m, k), "{c:?}");
        };

        expect(Command::InsertFrame, Modifiers::NONE, Key::F5);
        expect(Command::InsertKeyframe, Modifiers::NONE, Key::F6);
        expect(Command::InsertBlankKeyframe, Modifiers::NONE, Key::F7);
        expect(Command::RemoveFrame, Modifiers::SHIFT, Key::F5);
        expect(Command::ClearKeyframe, Modifiers::SHIFT, Key::F6);
        expect(Command::PlayPause, Modifiers::NONE, Key::Enter);
    }

    /// F8 is the single most-pressed key in a symbol-based workflow, and
    /// Ctrl+E is how you get inside what it made.
    #[test]
    fn the_symbol_shortcuts_match_animate() {
        let expect = |c: Command, m: Modifiers, k: Key| {
            let sc = c
                .shortcut()
                .unwrap_or_else(|| panic!("{c:?} has no shortcut"));
            assert_eq!((sc.modifiers, sc.logical_key), (m, k), "{c:?}");
        };

        expect(Command::ConvertToSymbol, Modifiers::NONE, Key::F8);
        expect(Command::NewSymbol, Modifiers::CTRL, Key::F8);
        expect(Command::EditSymbol, Modifiers::CTRL, Key::E);
    }

    #[test]
    fn the_familiar_shortcuts_are_what_a_user_expects() {
        let expect = |c: Command, m: Modifiers, k: Key| {
            let sc = c
                .shortcut()
                .unwrap_or_else(|| panic!("{c:?} has no shortcut"));
            assert_eq!(sc.modifiers, m, "{c:?} modifiers");
            assert_eq!(sc.logical_key, k, "{c:?} key");
        };

        expect(Command::Save, Modifiers::CTRL, Key::S);
        expect(Command::Undo, Modifiers::CTRL, Key::Z);
        expect(Command::Copy, Modifiers::CTRL, Key::C);
        expect(Command::Paste, Modifiers::CTRL, Key::V);
        expect(Command::SelectAll, Modifiers::CTRL, Key::A);
        expect(Command::GroupSelection, Modifiers::CTRL, Key::G);
    }

    #[test]
    fn selection_dependent_commands_are_marked() {
        assert!(Command::Delete.needs_selection());
        assert!(Command::GroupSelection.needs_selection());
        assert!(!Command::Save.needs_selection());
        assert!(!Command::NewLayer.needs_selection());
    }
}
