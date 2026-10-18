//! Editor commands and their shortcuts.
//!
//! Menus, keyboard shortcuts and toolbar buttons all raise the same
//! [`Command`]. Keeping one enumeration means a menu item and its shortcut can
//! never drift apart, and the whole key map can be tested without a window.
//!
//! Shortcuts follow Animate's defaults. Where a user would reach for a key out
//! of habit, it does what they expect.

use buzz_doc::SceneKey;
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
    /// **Lip-sync every character from the caption layer**, each over their own
    /// lines and nobody else's. See `Editor::lip_sync_from_captions`.
    LipSyncFromCaptions,
    /// **Transcribe a sound with BuzzEdit**: its words become captions and
    /// the speaker's lip sync, as Lip Sync from Captions does. See
    /// `buzz_app::estate` (`docs/ECOSYSTEM_BRIDGE_PLAN.md` E2).
    TranscribeWithBuzzEdit,
    /// **Send to BuzzEdit**: export an MP4 and hand it to BuzzEdit, as a new
    /// project or into one of its projects. See `buzz_app::estate` (E2).
    SendToBuzzEdit,
    /// **Import Script from Studio**: a Buzzcaf Studio project's script,
    /// built into an episode. See `buzz_app::estate` (E3).
    ImportScriptFromStudio,
    /// **Produce Episode**: a script, a recording and character art in, a
    /// finished episode out (breakdown, cast, backgrounds, audio, build,
    /// render). See `buzz_app::produce` (PRODUCER_PLAN P6).
    ProduceEpisode,

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
    /// Open View > Pasteboard Colour…: "Match theme" (the default) or a
    /// colour picker, persisted in `Workspace::pasteboard`.
    PasteboardColour,

    // Modify
    GroupSelection,
    UngroupSelection,
    /// **Break the selection apart one level** — Animate's Modify ▸ Break
    /// Apart. A group frees its children, an instance drops to its symbol's
    /// contents, and an image-filled shape bakes its transform so the Lasso,
    /// Eraser and Wand can cut it directly. See `Editor::break_apart_selection`.
    BreakApart,
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
    /// Trace Bitmap with the options dialog open first, rather than the
    /// built-in defaults straight away.
    TraceBitmapOptions,
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
    /// Modify ▸ Rig ▸ Extend Part Under Neighbours: continue each selected
    /// part symbol's artwork under whatever hides its cut, to its
    /// transformation point, on a bottom "Extension" layer.
    ExtendPart,

    // Insert
    NewLayer,
    NewLayerFolder,
    /// Move the active layer one step up (towards the foreground).
    MoveLayerUp,
    /// Move the active layer one step down (towards the background).
    MoveLayerDown,
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
    /// Keep the selected library symbol in the Assets library.
    SymbolToAsset,
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
    /// Set the UI's zoom factor, as a whole percentage — Stage 6, item 1 of
    /// `docs/WORKSPACE_DESIGN_PLAN.md`'s View > UI Size (100/125/150/175/200).
    SetUiScale(u32),
    /// Whether egui's own animations (a tooltip fading in, a caret blinking)
    /// run at all — Stage 6, item 5's Reduced Motion.
    ToggleReduceMotion,
    /// Help ▸ About BuzzAnimate.
    About,
    /// Put every panel back where it started.
    ResetWorkspace,
    /// Switch to one of Window > Workspace's named presets — Stage 4, item 2
    /// of `docs/WORKSPACE_DESIGN_PLAN.md`.
    SetWorkspacePreset(super::workspace::WorkspacePreset),
    /// Write the layout out now, rather than waiting for the next change
    /// that autosaves it. Window > Workspace's "Save Current".
    SaveWorkspace,
    /// Keep the document as it is now under a name, to come back to.
    SaveSnapshot,
    /// Open the list of saved snapshots, to restore one.
    Snapshots,
    /// Open the keyboard-shortcut editor.
    ShortcutEditor,
    /// Open the local AI models and LLM backends dialog.
    AiModels,
    /// Voice commands: press to talk (or stop) in "Push to talk" mode. The
    /// transcript goes into the Assistant's request box for review.
    VoicePushToTalk,
    /// Voice commands: mute / unmute the microphone in every mode.
    VoiceMute,
    /// Create, segment, complete, and rig a character from a reference image.
    AiCharacterFromImage,
    /// Identify and rename library symbols and assets using local VLM.
    AiIdentifyAssets,
    /// Auto-rig an existing vector character symbol via skeleton bone capsules.
    AiRigVectorCharacter,
    /// Build and stage a scene from a reference image using local VLM and library assets.
    AiSceneFromImage,
    /// Direct a sequence from natural language brief using local LLM verbs.
    AiDirectStory,
    /// Import a labelled mouth chart as a character's swappable mouth frames.
    /// Agent only: it needs a chart file and a character, which no menu can ask for.
    AiMouthChart,

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
    /// **Show the bones** over the artwork, and let the pointer find them.
    ///
    /// Also in the Bone tool's own options, but a setting you can only reach by
    /// picking up a particular tool is a setting nobody finds — and *hiding*
    /// the rig is most wanted by somebody holding a drawing tool, who is not
    /// holding the Bone tool by definition.
    ToggleBoneRig,
    /// **BM**: switch the Bone tool between Create bones and Move bones.
    /// Does nothing unless the Bone tool is the active tool.
    ToggleBoneMode,

    // Staging and performance
    /// **Set a scene**: ground, backdrop, lights and a cast standing in it.
    ///
    /// Not a template and not clip art — the arrangement, which is the part
    /// that is arithmetic rather than drawing. See `buzz_act::staging`.
    SetScene,
    /// **Direct a story**: a few lines of prose in, a staged scene with the
    /// walks and talks already on the timeline. See `buzz_act::direct`.
    DirectScene,
    /// The scenery half of the Story panel: what is in the shot, and whose
    /// drawings it is made of.
    SceneryFor,
    /// A new, empty scene after this one — the next shot of the film.
    AddScene,
    /// A complete copy of this scene, opened for editing. What the next beat
    /// of a conversation starts from.
    DuplicateScene,
    /// Put one more rigged person on the stage, on a layer of their own.
    AddPerson,
    /// **Insert ▸ Title Card…**: a line of text animated on and off with a
    /// preset, on a new top layer that ignores the lights. See
    /// `buzz_act::title`.
    AddTitleCard,
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

    // -- Sequence: reusable setups, shots cut on the dialogue -----------------
    // See `docs/SEQUENCE_SETUPS_PLAN.md` "Design" and `docs/SEQUENCE_GUIDE.md`.
    /// **Sequence \u{25b8} Set Master Audio\u{2026}**: import the dialogue/soundtrack
    /// that every shot's `audio_in`/`audio_out` are measured against.
    SetMasterAudio,
    /// Point the master audio at a sound already carried by a layer of the
    /// active (Normal) scene, instead of importing a new file.
    SetMasterAudioFromLayer(buzz_scene::LayerId),
    /// **Sequence \u{25b8} Make Setup from This Scene**: mark the active scene a
    /// reusable Setup — a source to stamp shots from, never itself in the
    /// exported film.
    MakeSetupFromScene,
    /// **Sequence \u{25b8} New Shot from Setup \u{25b8} \u{2039}setup\u{203a}**: stamp a new shot from
    /// the chosen setup, spanning the film playhead to the remaining master
    /// audio.
    NewShotFromSetup(SceneKey),
    /// **Sequence strip \u{25b8} Split at Playhead** (S): cut the active shot in two
    /// at its own local playhead. The right half is a new shot stamped fresh
    /// from the same setup; the left keeps every frame it already has.
    SplitAtPlayhead,
    /// **Shot \u{25b8} Try Another Setup \u{25b8} \u{2039}setup\u{203a}**: `(shot, new setup)` — stamp a
    /// new take of the same slice of audio from a different setup. The old
    /// take is kept, just not in the cut.
    TryAnotherSetup(SceneKey, SceneKey),
    /// **Shot \u{25b8} Duplicate Shot**: another take of the given shot, same
    /// slice of audio, its own copy.
    DuplicateShot(SceneKey),
    /// **Shot \u{25b8} Add Missing Setup Layers**: add-only — copies across only
    /// the setup's layers this shot does not already have by name.
    AddMissingSetupLayers(SceneKey),
    /// **Shot \u{25b8} Edit Setup**: switch to editing the setup a shot was
    /// stamped from.
    EditSetup(SceneKey),
    /// **Sequence \u{25b8} Flatten to Master Scene**: a new Normal scene with every
    /// shot in film order laid end to end. The sequence itself is kept.
    FlattenToMasterScene,
    /// **Sequence \u{25b8} Cut on Dialogue \u{25b8} \u{2039}setup\u{203a}**: propose one shot per
    /// spoken line of the master audio, using the chosen setup for any line
    /// whose speaker (from an imported subtitle's captions, if there are
    /// any) does not name a setup of its own. Fills only the stretches of
    /// the master no shot in the cut covers yet — an existing shot is never
    /// re-cut, retrimmed or replaced.
    CutOnDialogue(SceneKey),
    /// **Shot \u{25b8} Continue from Shot \u{25b8} \u{2039}shot\u{203a}** / the Sequence strip's
    /// right-click: `(this shot, an earlier shot of the same setup)` — seed
    /// this shot's still-untouched layers with the source shot's last pose
    /// at frame 0. Never changes the source.
    ContinueFromShot(SceneKey, SceneKey),

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
    /// **Test Movie**: play the whole film, full quality, in an overlay over
    /// the stage — Animate's Ctrl+Enter. Shares that chord with `RunScript`,
    /// which wins only when the Actions code editor has focus.
    TestMovie,
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
    Align {
        op: super::align::Align,
        to_stage: bool,
    },
    Distribute(super::align::Distribute),
    MatchSize(super::align::MatchSize),

    /// Move the selection by whole document units, from the arrow keys.
    ///
    /// Carried as one command with a vector rather than four commands, because
    /// the four are the same action and a menu never shows any of them: this is
    /// a key, and the key says which way.
    Nudge {
        x: i32,
        y: i32,
    },

    /// **Move Frames** — move the timeline's frame selection so its first
    /// frame lands on `to` (0-based), `rows` layer rows down (negative: up),
    /// or copy it there with `copy`. What dragging the selection in the
    /// timeline does, carried as a command so the AI agent can do it as well.
    /// See `docs/TIMELINE_FRAME_MOVE_PLAN.md` M4 and M6.
    MoveFrames {
        to: u32,
        rows: i32,
        copy: bool,
    },
}

/// How far one arrow press moves the selection, and how far with Shift.
///
/// Animate's numbers. One unit is "line it up"; eight is "get it roughly
/// there", and having both is what stops the arrow keys being useless at one
/// end and imprecise at the other.
pub const NUDGE_STEP: i32 = 1;
pub const NUDGE_STEP_LARGE: i32 = 8;

/// Declares every [`Command`] variant once, with its stable agent id.
///
/// Both [`Command::id`] and the unit-variant table are generated from this one
/// list, and `id` is an exhaustive `match`, so a new variant fails to compile
/// until it is added here (and so classified for the AI agent).
macro_rules! command_table {
    (
        unit { $($u:ident => $uid:literal,)* }
        data { $($d:ident $dpat:tt => $did:literal, $hint:literal;)* }
    ) => {
        impl Command {
            /// The stable snake_case id of this command, for scripts and the AI
            /// agent. Data-carrying variants share one id and differ by args.
            pub fn id(self) -> &'static str {
                match self {
                    $(Command::$u => $uid,)*
                    $(Command::$d $dpat => $did,)*
                }
            }

            /// Every command that carries no data, in declaration order.
            pub fn unit_commands() -> &'static [Command] {
                &[$(Command::$u),*]
            }

            /// `(id, args hint)` for every data-carrying command.
            pub fn data_command_hints() -> &'static [(&'static str, &'static str)] {
                &[$(($did, $hint)),*]
            }

            fn unit_from_id(id: &str) -> Option<Command> {
                match id {
                    $($uid => Some(Command::$u),)*
                    _ => None,
                }
            }
        }
    };
}

command_table! {
    unit {
            New => "new",
            Open => "open",
            Save => "save",
            SaveAs => "save_as",
            Close => "close",
            Quit => "quit",
            ImportToLibrary => "import_to_library",
            ImportToStage => "import_to_stage",
            ExportFla => "export_fla",
            ExportImage => "export_image",
            ExportSequence => "export_sequence",
            ExportVideo => "export_video",
            ExportGif => "export_gif",
            ExportWebp => "export_webp",
            ImportSound => "import_sound",
            ImportImage => "import_image",
            AttachSound => "attach_sound",
            RemoveSound => "remove_sound",
            LipSync => "lip_sync",
            NewMouthSymbol => "new_mouth_symbol",
            DetectBeats => "detect_beats",
            FitToNarration => "fit_to_narration",
            ImportCaptions => "import_captions",
            ExportCaptions => "export_captions",
            LipSyncFromCaptions => "lip_sync_from_captions",
            TranscribeWithBuzzEdit => "buzzedit_transcribe",
            SendToBuzzEdit => "buzzedit_send",
            ImportScriptFromStudio => "studio_import_script",
            ProduceEpisode => "produce_episode_dialog",
            Undo => "undo",
            Redo => "redo",
            Cut => "cut",
            Copy => "copy",
            Paste => "paste",
            Delete => "delete",
            SelectAll => "select_all",
            Deselect => "deselect",
            DuplicateSelection => "duplicate_selection",
            ZoomIn => "zoom_in",
            ZoomOut => "zoom_out",
            ZoomActual => "zoom_actual",
            ZoomFitInWindow => "zoom_fit_in_window",
            ZoomShowFrame => "zoom_show_frame",
            ZoomShowAll => "zoom_show_all",
            ToggleRulers => "toggle_rulers",
            ToggleGrid => "toggle_grid",
            ToggleGuides => "toggle_guides",
            ToggleSnapping => "toggle_snapping",
            TogglePasteboard => "toggle_pasteboard",
            PasteboardColour => "pasteboard_colour",
            GroupSelection => "group_selection",
            UngroupSelection => "ungroup_selection",
            BreakApart => "break_apart",
            BringToFront => "bring_to_front",
            BringForward => "bring_forward",
            SendBackward => "send_backward",
            SendToBack => "send_to_back",
            RetargetPerformance => "retarget_performance",
            SwapSymbol => "swap_symbol",
            PaintThrough => "paint_through",
            ExposeOnTwos => "expose_on_twos",
            ExposeOnThrees => "expose_on_threes",
            SelectSameColour => "select_same_colour",
            ThickenStroke => "thicken_stroke",
            ThinStroke => "thin_stroke",
            TraceBitmap => "trace_bitmap",
            TraceLineArt => "trace_line_art",
            TraceBitmapOptions => "trace_bitmap_options",
            ConvertLinesToFills => "convert_lines_to_fills",
            ExpandFill => "expand_fill",
            SmoothSelection => "smooth_selection",
            StraightenSelection => "straighten_selection",
            RecogniseShape => "recognise_shape",
            FlipHorizontal => "flip_horizontal",
            FlipVertical => "flip_vertical",
            RotateClockwise => "rotate_clockwise",
            RotateAnticlockwise => "rotate_anticlockwise",
            ExtendPart => "extend_part",
            NewLayer => "new_layer",
            NewLayerFolder => "new_layer_folder",
            MoveLayerUp => "move_layer_up",
            MoveLayerDown => "move_layer_down",
            NewReferenceLayer => "new_reference_layer",
            ImportVideoReference => "import_video_reference",
            ImportSequenceFolder => "import_sequence_folder",
            DeleteLayer => "delete_layer",
            ConvertToSymbol => "convert_to_symbol",
            NewSymbol => "new_symbol",
            EditSymbol => "edit_symbol",
            EditDocument => "edit_document",
            PlaceInstance => "place_instance",
            DuplicateSymbol => "duplicate_symbol",
            SymbolToAsset => "symbol_to_asset",
            DeleteSymbol => "delete_symbol",
            NewLibraryFolder => "new_library_folder",
            BrushFromSelection => "brush_from_selection",
            ToggleLayoutLock => "toggle_layout_lock",
            ToggleTheme => "toggle_theme",
            ToggleReduceMotion => "toggle_reduce_motion",
            About => "about",
            ResetWorkspace => "reset_workspace",
            SaveWorkspace => "save_workspace",
            SaveSnapshot => "save_snapshot",
            Snapshots => "snapshots",
            ShortcutEditor => "shortcut_editor",
            AiModels => "ai_models",
            VoicePushToTalk => "voice_push_to_talk",
            VoiceMute => "voice_mute",
            AiCharacterFromImage => "ai_character_from_image",
            AiIdentifyAssets => "ai_identify_assets",
            AiRigVectorCharacter => "ai_rig_vector_character",
            AiSceneFromImage => "ai_scene_from_image",
            AiDirectStory => "ai_direct_story",
            AiMouthChart => "ai_mouth_chart",
            AddSun => "add_sun",
            AddSky => "add_sky",
            AddLamp => "add_lamp",
            AddGloom => "add_gloom",
            AddFire => "add_fire",
            AddStorm => "add_storm",
            ToggleLightGizmos => "toggle_light_gizmos",
            ToggleBoneRig => "toggle_bone_rig",
            ToggleBoneMode => "toggle_bone_mode",
            SetScene => "set_scene",
            DirectScene => "direct_scene",
            SceneryFor => "scenery_for",
            AddScene => "add_scene",
            DuplicateScene => "duplicate_scene",
            AddPerson => "add_person",
            AddTitleCard => "add_title_card",
            Perform => "perform",
            AddFollowThrough => "add_follow_through",
            AddWiggle => "add_wiggle",
            ClearModifiers => "clear_modifiers",
            BakeModifiers => "bake_modifiers",
            SetReverse => "set_reverse",
            ClearReverse => "clear_reverse",
            AddProfileRight => "add_profile_right",
            AddProfileLeft => "add_profile_left",
            AddThreeQuarterRight => "add_three_quarter_right",
            AddThreeQuarterLeft => "add_three_quarter_left",
            SetMasterAudio => "set_master_audio",
            MakeSetupFromScene => "make_setup_from_scene",
            SplitAtPlayhead => "split_at_playhead",
            FlattenToMasterScene => "flatten_to_master_scene",
            ToggleActionsPanel => "toggle_actions_panel",
            RunScript => "run_script",
            ClearScriptOutput => "clear_script_output",
            CreateClassicTween => "create_classic_tween",
            CreateMotionTween => "create_motion_tween",
            CreateShapeTween => "create_shape_tween",
            RemoveTween => "remove_tween",
            InsertFrame => "insert_frame",
            RemoveFrame => "remove_frame",
            InsertKeyframe => "insert_keyframe",
            InsertBlankKeyframe => "insert_blank_keyframe",
            ClearKeyframe => "clear_keyframe",
            PlayPause => "play_pause",
            TestMovie => "test_movie",
            NextFrame => "next_frame",
            PreviousFrame => "previous_frame",
            FirstFrame => "first_frame",
            LastFrame => "last_frame",
            CutFrames => "cut_frames",
            CopyFrames => "copy_frames",
            PasteFrames => "paste_frames",
            ClearFrames => "clear_frames",
            ReverseFrames => "reverse_frames",
            ToggleOnionSkin => "toggle_onion_skin",
            ToggleAutoKeyframe => "toggle_auto_keyframe",
            ToggleEditMultipleFrames => "toggle_edit_multiple_frames",
            ToggleCamera => "toggle_camera",
            AddCameraKeyframe => "add_camera_keyframe",
            RemoveCameraKeyframe => "remove_camera_keyframe",
            ResetCamera => "reset_camera",
            AddLightKeyframe => "add_light_keyframe",
            RemoveLightKeyframe => "remove_light_keyframe",
            SaveAsTemplate => "save_as_template",
    }
    data {
        TogglePanel(_) => "toggle_panel", "{panel: panel id e.g. layers|timeline|ai}";
        SetTheme(_) => "set_theme", "{theme: dark|light|midnight|slate|sepia|contrast|forest|dracula|solarized_dark|nord}";
        SetUiScale(_) => "set_ui_scale", "{scale: whole percent e.g. 100|125|150}";
        SetWorkspacePreset(_) => "set_workspace_preset", "{name: essentials|animator|classic}";
        SetMasterAudioFromLayer(_) => "set_master_audio_from_layer", "{layer: layer id number}";
        NewShotFromSetup(_) => "new_shot_from_setup", "{setup: scene key number}";
        TryAnotherSetup(_, _) => "try_another_setup", "{shot: scene key, setup: scene key}";
        DuplicateShot(_) => "duplicate_shot", "{shot: scene key number}";
        AddMissingSetupLayers(_) => "add_missing_setup_layers", "{shot: scene key number}";
        EditSetup(_) => "edit_setup", "{setup: scene key number}";
        CutOnDialogue(_) => "cut_on_dialogue", "{setup: scene key number}";
        ContinueFromShot(_, _) => "continue_from_shot", "{shot: scene key, source: scene key}";
        AddCameraMove(_) => "add_camera_move", "{move: push_in|pull_out|pan_left|pan_right|reveal|drift}";
        SelectTool(_) => "select_tool", "{tool: tool id e.g. selection|brush|oval|paint_bucket}";
        NewFromTemplate(_) => "new_from_template", "{index: template index}";
        Align{..} => "align", "{edge: left_edges|horizontal_centres|right_edges|top_edges|vertical_centres|bottom_edges, to_stage?: bool}";
        Distribute(_) => "distribute", "{mode: horizontal_centres|vertical_centres|horizontal_spacing|vertical_spacing}";
        MatchSize(_) => "match_size", "{mode: width|height|both}";
        Nudge{..} => "nudge", "{dx: int, dy: int}";
        MoveFrames{..} => "move_frames", "{to: 0-based frame the timeline frame selection's first frame lands on, rows?: int layer rows down (negative up, default 0), copy?: bool}";
    }
}

/// Every panel, for naming one in a command's args.
const ALL_PANELS: [super::workspace::PanelId; 21] = {
    use super::workspace::PanelId::*;
    [
        Tools, ToolOptions, Layers, Properties, Color, Swatches, Depth, Rig, Filters, Lighting,
        Sound, Library, Assets, Cast, Timeline, Sequence, Actions, Story, MotionEditor, Tasks, Ai,
    ]
};

/// Lower-case with separators removed, so `Paint Bucket`, `paint_bucket` and
/// `PaintBucket` all name the same thing.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn pick_named<T: Copy + std::fmt::Debug>(all: &[T], name: &str, what: &str) -> Result<T, String> {
    let want = norm(name);
    all.iter()
        .copied()
        .find(|v| norm(&format!("{v:?}")) == want)
        .ok_or_else(|| format!("unknown {what} '{name}'"))
}

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing string arg '{key}'"))
}

fn arg_i64(args: &serde_json::Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("missing integer arg '{key}'"))
}

fn arg_key(args: &serde_json::Value, key: &str) -> Result<SceneKey, String> {
    let n = arg_i64(args, key)?;
    u32::try_from(n)
        .map(SceneKey)
        .map_err(|_| format!("arg '{key}' is not a valid scene key"))
}

impl Command {
    /// The command with this id, built from `args` when it carries data.
    ///
    /// The inverse of [`Command::id`]: every command round-trips. An unknown id
    /// or a missing/invalid argument is an `Err` describing it, never a panic.
    pub fn from_id(id: &str, args: &serde_json::Value) -> Result<Command, String> {
        use Command::*;
        if let Some(unit) = Self::unit_from_id(id) {
            return Ok(unit);
        }
        Ok(match id {
            "toggle_panel" => TogglePanel(pick_named(&ALL_PANELS, arg_str(args, "panel")?, "panel")?),
            "set_theme" => SetTheme(pick_named(&super::theme::Theme::ALL, arg_str(args, "theme")?, "theme")?),
            "set_ui_scale" => {
                let n = arg_i64(args, "scale")?;
                SetUiScale(u32::try_from(n).map_err(|_| "scale must be a positive percent".to_string())?)
            }
            "set_workspace_preset" => SetWorkspacePreset(pick_named(
                &super::workspace::WorkspacePreset::ALL,
                arg_str(args, "name")?,
                "workspace preset",
            )?),
            "set_master_audio_from_layer" => {
                let n = arg_i64(args, "layer")?;
                SetMasterAudioFromLayer(buzz_scene::LayerId(
                    u64::try_from(n).map_err(|_| "layer must be a layer id".to_string())?,
                ))
            }
            "new_shot_from_setup" => NewShotFromSetup(arg_key(args, "setup")?),
            "try_another_setup" => TryAnotherSetup(arg_key(args, "shot")?, arg_key(args, "setup")?),
            "duplicate_shot" => DuplicateShot(arg_key(args, "shot")?),
            "add_missing_setup_layers" => AddMissingSetupLayers(arg_key(args, "shot")?),
            "edit_setup" => EditSetup(arg_key(args, "setup")?),
            "cut_on_dialogue" => CutOnDialogue(arg_key(args, "setup")?),
            "continue_from_shot" => ContinueFromShot(arg_key(args, "shot")?, arg_key(args, "source")?),
            "add_camera_move" => {
                use buzz_scene::CameraMove as M;
                AddCameraMove(pick_named(
                    &[M::PushIn, M::PullOut, M::PanLeft, M::PanRight, M::Reveal, M::Drift],
                    arg_str(args, "move")?,
                    "camera move",
                )?)
            }
            "select_tool" => SelectTool(pick_named(&super::tools::all_tools(), arg_str(args, "tool")?, "tool")?),
            "new_from_template" => {
                let n = arg_i64(args, "index")?;
                NewFromTemplate(usize::try_from(n).map_err(|_| "index must be >= 0".to_string())?)
            }
            "align" => {
                use super::align::Align as A;
                Align {
                    op: pick_named(
                        &[A::LeftEdges, A::HorizontalCentres, A::RightEdges, A::TopEdges, A::VerticalCentres, A::BottomEdges],
                        arg_str(args, "edge")?,
                        "alignment edge",
                    )?,
                    to_stage: args.get("to_stage").and_then(|v| v.as_bool()).unwrap_or(false),
                }
            }
            "distribute" => {
                use super::align::Distribute as D;
                Distribute(pick_named(
                    &[D::HorizontalCentres, D::VerticalCentres, D::HorizontalSpacing, D::VerticalSpacing],
                    arg_str(args, "mode")?,
                    "distribute mode",
                )?)
            }
            "match_size" => {
                use super::align::MatchSize as S;
                MatchSize(pick_named(&[S::Width, S::Height, S::Both], arg_str(args, "mode")?, "match-size mode")?)
            }
            "nudge" => {
                let x = i32::try_from(arg_i64(args, "dx")?).map_err(|_| "dx out of range".to_string())?;
                let y = i32::try_from(arg_i64(args, "dy")?).map_err(|_| "dy out of range".to_string())?;
                Nudge { x, y }
            }
            "move_frames" => {
                let to = u32::try_from(arg_i64(args, "to")?)
                    .map_err(|_| "to must be a frame number, 0 or more".to_string())?;
                let rows = match args.get("rows") {
                    None => 0,
                    Some(v) => v
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .ok_or_else(|| "rows must be a whole number".to_string())?,
                };
                let copy = args.get("copy").and_then(|v| v.as_bool()).unwrap_or(false);
                MoveFrames { to, rows, copy }
            }
            other => return Err(format!("unknown command '{other}'")),
        })
    }
}

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
            LipSyncFromCaptions => "Lip Sync from Captions",
            TranscribeWithBuzzEdit => "Transcribe with BuzzEdit\u{2026}",
            SendToBuzzEdit => "Send to BuzzEdit\u{2026}",
            ImportScriptFromStudio => "Import Script from Studio\u{2026}",
            ProduceEpisode => "Produce Episode\u{2026}",

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
            PasteboardColour => "Pasteboard Colour\u{2026}",

            GroupSelection => "Group",
            UngroupSelection => "Ungroup",
            BreakApart => "Break Apart",
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
            TraceBitmapOptions => "Trace Bitmap\u{2026}",
            ConvertLinesToFills => "Convert Lines to Fills",
            ExpandFill => "Expand Fill…",
            SmoothSelection => "Smooth",
            StraightenSelection => "Straighten",
            RecogniseShape => "Recognise Shape",
            FlipHorizontal => "Flip Horizontal",
            FlipVertical => "Flip Vertical",
            RotateClockwise => "Rotate 90\u{b0} CW",
            RotateAnticlockwise => "Rotate 90\u{b0} CCW",
            ExtendPart => "Extend Part Under Neighbours",

            NewLayer => "New Layer",
            NewLayerFolder => "New Folder",
            MoveLayerUp => "Move Layer Up",
            MoveLayerDown => "Move Layer Down",
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
            SymbolToAsset => "Keep Symbol as Asset",
            DeleteSymbol => "Delete Symbol",
            NewLibraryFolder => "New Library Folder",

            BrushFromSelection => "Create Brush From Selection",

            TogglePanel(_) => "Panel",
            ToggleLayoutLock => "Lock Layout",
            ToggleTheme => "Next Theme",
            SetTheme(_) => "Theme",
            SetUiScale(_) => "UI Size",
            ToggleReduceMotion => "Reduce Motion",
            About => "About BuzzAnimate",
            ResetWorkspace => "Reset Layout",
            SetWorkspacePreset(_) => "Workspace",
            SaveWorkspace => "Save Current Workspace",
            SaveSnapshot => "Save Snapshot",
            Snapshots => "Snapshots\u{2026}",
            ShortcutEditor => "Keyboard Shortcuts\u{2026}",
            AiModels => "AI Models\u{2026}",
            VoicePushToTalk => "Voice: Push to Talk",
            VoiceMute => "Voice: Mute Microphone",
            AiCharacterFromImage => "Character from Image\u{2026}",
            AiIdentifyAssets => "Identify & Rename Assets\u{2026}",
            AiRigVectorCharacter => "Auto-Rig Vector Character\u{2026}",
            AiSceneFromImage => "Scene from Image\u{2026}",
            AiDirectStory => "Direct Story with LLM\u{2026}",
            AiMouthChart => "Mouth Chart\u{2026}",

            AddSun => "Sun",
            AddSky => "Sky",
            AddLamp => "Lamp",
            AddGloom => "Gloom",
            AddFire => "Fire",
            AddStorm => "Storm",
            ToggleLightGizmos => "Light Handles",
            ToggleBoneRig => "Bones",
            ToggleBoneMode => "Bone Tool: Create / Move",

            SetScene => "Set the Scene\u{2026}",
            DirectScene => "Direct a Story\u{2026}",
            SceneryFor => "Scenery\u{2026}",
            AddScene => "Add Scene",
            DuplicateScene => "Duplicate Scene",
            AddPerson => "Add Person",
            AddTitleCard => "Title Card\u{2026}",
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

            SetMasterAudio => "Set Master Audio\u{2026}",
            SetMasterAudioFromLayer(_) => "Set Master Audio",
            MakeSetupFromScene => "Make Setup from This Scene",
            NewShotFromSetup(_) => "New Shot from Setup",
            SplitAtPlayhead => "Split at Playhead",
            TryAnotherSetup(..) => "Try Another Setup",
            DuplicateShot(_) => "Duplicate Shot",
            AddMissingSetupLayers(_) => "Add Missing Setup Layers",
            EditSetup(_) => "Edit Setup",
            FlattenToMasterScene => "Flatten to Master Scene",
            CutOnDialogue(_) => "Cut on Dialogue",
            ContinueFromShot(..) => "Continue from Shot",

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
            TestMovie => "Test Movie",
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
            MoveFrames { copy: true, .. } => "Copy Frames",
            MoveFrames { .. } => "Move Frames",
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
        // Uses COMMAND so that on macOS it maps to Command (⌘) and on Windows/Linux to Ctrl.
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND.plus(Modifiers::SHIFT);
        let ctrl_alt = Modifiers::COMMAND.plus(Modifiers::ALT);

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
            BreakApart => sc(ctrl, Key::B),
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
            TraceBitmap | TraceLineArt | TraceBitmapOptions => None,
            ConvertLinesToFills => None,
            SmoothSelection => None,
            RecogniseShape => None,
            StraightenSelection => None,
            // Animate's own: Ctrl+Shift+9 and Ctrl+Shift+7 rotate; the flips
            // have no default there, and none is invented here.
            FlipHorizontal | FlipVertical => None,
            RotateClockwise => sc(ctrl_shift, Key::Num9),
            RotateAnticlockwise => sc(ctrl_shift, Key::Num7),
            ExtendPart => None,

            NewLayer => sc(ctrl_alt, Key::N),
            NewLayerFolder => sc(ctrl_alt, Key::F),
            MoveLayerUp => sc(ctrl_alt, Key::ArrowUp),
            MoveLayerDown => sc(ctrl_alt, Key::ArrowDown),
            DeleteLayer => None,

            // F8 and Ctrl+F8 are muscle memory for anyone who has used
            // Animate; Ctrl+E steps in and out of symbol editing.
            ConvertToSymbol => sc(Modifiers::NONE, Key::F8),
            NewSymbol => sc(ctrl, Key::F8),
            EditSymbol => sc(ctrl, Key::E),
            EditDocument => sc(ctrl, Key::F4),
            PlaceInstance => None,
            DuplicateSymbol | SymbolToAsset => None,
            DeleteSymbol => None,
            NewLibraryFolder => None,

            BrushFromSelection => None,

            // Adding a light is a deliberate, occasional act; the handles get
            // a key because they are turned off to see the picture clean and
            // straight back on to keep working.
            // The AI panel (F7.0) gets one: it is where every AI flow lives,
            // and Ctrl+Alt+I is free.
            TogglePanel(super::workspace::PanelId::Ai) => sc(ctrl_alt, Key::I),
            TogglePanel(_) | ResetWorkspace | SetWorkspacePreset(_) | SaveWorkspace => None,
            // Animate has no shortcut for this; it is a settle-down-and-work
            // action, and Ctrl+Alt+L is free here.
            ToggleLayoutLock => sc(ctrl.plus(Modifiers::ALT), Key::L),
            ToggleTheme => None,
            SetTheme(_) => None,
            PasteboardColour => None,
            SetUiScale(_) => None,
            ToggleReduceMotion => None,
            About => None,
            SaveSnapshot | Snapshots => None,
            ShortcutEditor => None,
            DetectBeats => None,
            FitToNarration => None,
            ImportCaptions | ExportCaptions | LipSyncFromCaptions => None,
            NewReferenceLayer | ImportVideoReference | SelectSameColour | ExposeOnTwos
            | ExposeOnThrees | PaintThrough | ImportSequenceFolder | RetargetPerformance
            | SwapSymbol => None,

            AddSun | AddSky | AddLamp | AddGloom | AddFire | AddStorm => None,
            // No Animate binding to follow, and these open dialogs rather than
            // acting straight away, so a key would only save the menu.
            SetScene | DirectScene | SceneryFor | AddScene | DuplicateScene | AddPerson
            | AddTitleCard | Perform | AddFollowThrough | AddWiggle | ClearModifiers | BakeModifiers
            | SetReverse | ClearReverse | AddProfileRight | AddProfileLeft
            | AddThreeQuarterRight | AddThreeQuarterLeft => None,
            // `SplitAtPlayhead`'s `S` is scoped to the Sequence strip having
            // the pointer's attention (`sequence_panel` itself reads the key),
            // not a document-wide binding — so it carries none here, and
            // never collides with a tool's own shortcut.
            SetMasterAudio | SetMasterAudioFromLayer(_) | MakeSetupFromScene
            | NewShotFromSetup(_) | SplitAtPlayhead | TryAnotherSetup(..)
            | DuplicateShot(_) | AddMissingSetupLayers(_) | EditSetup(_)
            | FlattenToMasterScene | CutOnDialogue(_) | ContinueFromShot(..) => None,
            ToggleLightGizmos => sc(ctrl_shift, Key::L),
            // Ctrl+Shift+B, beside the light handles it sits next to in the
            // View menu. Animate has no equivalent; there was nothing to clash.
            ToggleBoneRig => sc(ctrl_shift, Key::B),
            // BM: Shift+M, beside the Bone tool's own `M`. Only does anything
            // while the Bone tool is held (see `Editor::run`).
            ToggleBoneMode => sc(Modifiers::SHIFT, Key::M),

            // Voice commands. F5-F9 are taken (frames, symbols, Actions);
            // F12 and Shift+F12 were free.
            VoicePushToTalk => sc(Modifiers::NONE, Key::F12),
            VoiceMute => sc(Modifiers::SHIFT, Key::F12),
            // F9 is Animate's own Actions panel key on Windows.
            ToggleActionsPanel => sc(Modifiers::NONE, Key::F9),
            // Ctrl+Enter is what every code editor runs on, and plain Enter is
            // already Animate's Play/Pause — which a script author would hit
            // constantly by accident if running were bound to it.
            RunScript => sc(ctrl, Key::Enter),
            ClearScriptOutput => None,

            CreateClassicTween => sc(ctrl_alt, Key::T),
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
            // Shares this chord with `RunScript` on purpose: whichever one
            // applies depends on whether the code editor has focus, decided
            // by `script_shortcuts` rather than by this map. See
            // `shortcuts_do_not_collide` below.
            TestMovie => sc(ctrl, Key::Enter),
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
            // A drag in the timeline, and the agent's command; where the
            // frames go is not something a keystroke can say.
            MoveFrames { .. } => None,
            // Thirteen operations behind one menu. Animate gives the panel a
            // key, not the operations.
            Align { .. } | Distribute(_) | MatchSize(_) => None,
            // Reached from the File menu; the templates one needs a name, and
            // a keystroke cannot carry one.
            SaveAsTemplate | NewFromTemplate(_) => None,
            // No Animate binding to follow: they reach other apps.
            TranscribeWithBuzzEdit | SendToBuzzEdit | ImportScriptFromStudio => None,
            // A dialog with a whole form behind it.
            ProduceEpisode => None,
            AiModels | AiCharacterFromImage | AiIdentifyAssets | AiRigVectorCharacter | AiSceneFromImage | AiDirectStory | AiMouthChart => None,
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
                | BreakApart
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
                | ExtendPart
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
    Modifiers::COMMAND
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
        PasteboardColour,
        GroupSelection,
        UngroupSelection,
        BreakApart,
        BringToFront,
        BringForward,
        SendBackward,
        SendToBack,
        ThickenStroke,
        ThinStroke,
        TraceBitmap,
        TraceLineArt,
        TraceBitmapOptions,
        ConvertLinesToFills,
        ExpandFill,
        SmoothSelection,
        StraightenSelection,
        RecogniseShape,
        FlipHorizontal,
        FlipVertical,
        RotateClockwise,
        RotateAnticlockwise,
        ExtendPart,
        NewLayer,
        NewLayerFolder,
        MoveLayerUp,
        MoveLayerDown,
        NewReferenceLayer,
        DeleteLayer,
        InsertFrame,
        RemoveFrame,
        InsertKeyframe,
        InsertBlankKeyframe,
        ClearKeyframe,
        PlayPause,
        TestMovie,
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
        SymbolToAsset,
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
        ToggleBoneRig,
        ToggleBoneMode,
        SetScene,
        DirectScene,
        AddScene,
        DuplicateScene,
        AddPerson,
        AddTitleCard,
        Perform,
        AddFollowThrough,
        AddWiggle,
        ClearModifiers,
        BakeModifiers,
        SetReverse,
        ClearReverse,
        SetMasterAudio,
        MakeSetupFromScene,
        SplitAtPlayhead,
        FlattenToMasterScene,
        ToggleLayoutLock,
        ToggleTheme,
        ToggleReduceMotion,
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
        SendToBuzzEdit,
        ImportScriptFromStudio,
        ProduceEpisode,
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
        LipSyncFromCaptions,
        TranscribeWithBuzzEdit,
        AiModels,
        VoicePushToTalk,
        VoiceMute,
        AiCharacterFromImage,
        AiIdentifyAssets,
        AiRigVectorCharacter,
        AiSceneFromImage,
        AiDirectStory,
    ]
}

/// Format a command's built-in shortcut the way a menu shows it.
pub fn shortcut_text(ctx: &egui::Context, command: Command) -> String {
    format_shortcut(ctx, command.shortcut())
}

/// Format an already-resolved shortcut (which may be the user's override or
/// `None`) the way a menu shows it.
pub fn format_shortcut(ctx: &egui::Context, shortcut: Option<egui::KeyboardShortcut>) -> String {
    shortcut
        .map(|s| ctx.format_shortcut(&s))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_palette_lists_real_commands_with_labels() {
        let palette = palette_commands();
        assert!(
            palette.len() > 50,
            "the palette should list the whole catalogue"
        );
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
            assert!(
                palette.contains(&c),
                "{c:?} has a shortcut but is not in the palette"
            );
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
            PasteboardColour,
            GroupSelection,
            UngroupSelection,
            BreakApart,
            BringToFront,
            BringForward,
            SendBackward,
            SendToBack,
            ThickenStroke,
            ThinStroke,
            TraceBitmap,
            TraceLineArt,
            TraceBitmapOptions,
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
            MoveLayerUp,
            MoveLayerDown,
            DeleteLayer,
            InsertFrame,
            RemoveFrame,
            InsertKeyframe,
            InsertBlankKeyframe,
            ClearKeyframe,
            PlayPause,
            TestMovie,
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
            SymbolToAsset,
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
            ToggleBoneRig,
            ToggleBoneMode,
            SetScene,
            DirectScene,
            AddScene,
            DuplicateScene,
            AddPerson,
            AddTitleCard,
            Perform,
            AddFollowThrough,
            AddWiggle,
            ClearModifiers,
            BakeModifiers,
            SetReverse,
            ClearReverse,
            ToggleLayoutLock,
            ToggleTheme,
            ToggleReduceMotion,
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
            LipSyncFromCaptions,
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
                //
                // RunScript and TestMovie deliberately share Ctrl+Enter too:
                // `script_shortcuts` decides between them by whether the
                // Actions code editor has focus, so the chord never actually
                // fires both.
                let allowed = matches!(
                    (previous, c),
                    (Command::ZoomShowAll, Command::TogglePasteboard)
                        | (Command::TogglePasteboard, Command::ZoomShowAll)
                        | (Command::RunScript, Command::TestMovie)
                        | (Command::TestMovie, Command::RunScript)
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
        expect(Command::NewSymbol, Modifiers::COMMAND, Key::F8);
        expect(Command::EditSymbol, Modifiers::COMMAND, Key::E);
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

        expect(Command::Save, Modifiers::COMMAND, Key::S);
        expect(Command::Undo, Modifiers::COMMAND, Key::Z);
        expect(Command::Copy, Modifiers::COMMAND, Key::C);
        expect(Command::Paste, Modifiers::COMMAND, Key::V);
        expect(Command::SelectAll, Modifiers::COMMAND, Key::A);
        expect(Command::GroupSelection, Modifiers::COMMAND, Key::G);
    }

    #[test]
    fn selection_dependent_commands_are_marked() {
        assert!(Command::Delete.needs_selection());
        assert!(Command::GroupSelection.needs_selection());
        assert!(!Command::Save.needs_selection());
        assert!(!Command::NewLayer.needs_selection());
    }

    #[test]
    fn every_unit_command_round_trips_through_its_id() {
        let mut seen = std::collections::HashSet::new();
        for &c in Command::unit_commands() {
            assert!(seen.insert(c.id()), "duplicate id {}", c.id());
            assert_eq!(Command::from_id(c.id(), &serde_json::Value::Null), Ok(c));
        }
        assert_eq!(Command::unit_commands().len() + Command::data_command_hints().len(), 198);
    }

    #[test]
    fn data_commands_round_trip_and_bad_args_are_errors() {
        use serde_json::json;
        let cases = [
            ("select_tool", json!({"tool": "paint_bucket"}), Command::SelectTool(crate::tools::ToolId::PaintBucket)),
            ("set_ui_scale", json!({"scale": 150}), Command::SetUiScale(150)),
            ("nudge", json!({"dx": 2, "dy": -3}), Command::Nudge { x: 2, y: -3 }),
            (
                "move_frames",
                json!({"to": 5, "rows": -1, "copy": true}),
                Command::MoveFrames { to: 5, rows: -1, copy: true },
            ),
            ("move_frames", json!({"to": 5}), Command::MoveFrames { to: 5, rows: 0, copy: false }),
            ("new_from_template", json!({"index": 1}), Command::NewFromTemplate(1)),
            ("new_shot_from_setup", json!({"setup": 4}), Command::NewShotFromSetup(SceneKey(4))),
        ];
        for (id, args, want) in cases {
            let got = Command::from_id(id, &args).unwrap();
            assert_eq!(got, want);
            assert_eq!(got.id(), id);
        }
        assert!(Command::from_id("select_tool", &json!({})).is_err());
        assert!(Command::from_id("select_tool", &json!({"tool": "nope"})).is_err());
        assert!(Command::from_id("no_such_command", &json!({})).is_err());
    }
}
