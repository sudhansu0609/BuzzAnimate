//! D2 of `docs/AI_AGENT_PLAN.md`: the commands that open a dialog or panel for
//! parameters, run from args instead.
//!
//! Each command fills the same state the dialog holds, from its args, and then
//! calls the very function the dialog's OK / Apply button calls (named in each
//! arm), so a call here and a click there change the document identically. A
//! missing required arg is `ok:false` listing the missing names; an unknown or
//! out-of-range arg is refused before anything is touched.
//!
//! The AI commands whose work is a network / GPU job return `{started:true, job}`
//! and finish on the app's own task loop, committing as one undo step exactly
//! as they do from the AI panel; `wait_for_jobs` (`agent::jobs`) reports how
//! each one ended.

use super::props::layer_by_ref;
use super::{AgentResult, norm, parse_hex, pick_named, report};
use crate::app::App;
use buzz_scene::{LayerId, ObjectId, ObjectKind, Scene, SymbolId};
use buzz_ui::Command;
use serde_json::{Value, json};

/// Dialog commands the agent still cannot run, with why.
///
/// Empty since H2: `ai_character_from_image` now runs the wizard with its default
/// choices as one background job. The list stays so a future dialog that cannot be
/// reduced to args has somewhere to say why.
pub const STILL_NEEDS_DIALOG: &[(&str, &str)] = &[];

/// Is this command run from its args?
pub fn takes_args(command: Command) -> bool {
    args_hint(command).is_some()
}

/// The arg schema shown in the catalog.
pub fn args_hint(command: Command) -> Option<&'static str> {
    use Command::*;
    Some(match command {
        New => "{width?: 1..16384, height?: 1..16384, fps?: 0.01..240, background?: \"#rrggbb\", discard_changes?: bool (required true when the document has unsaved changes)} -> replaces the document (New dialog's Create); unset fields take the remembered New settings",
        LipSync => "{mouth: library symbol name|id (or make_mouth:true for a placeholder), layer?: name|index (default the active layer), silence?: 0..1 (0.06), hold?: frames (2)} -> keys mouths from the main-timeline sound (Lip Sync dialog's Run); needs a sound",
        TraceBitmapOptions => "{element?: id of a picture (else the selection), colours?: 2..64, detail?, smooth?: 0..1, speckle?, drop_background?, mode?: auto|outline|line_art|colour_lines|both, outline_contrast?, line_colour?: \"#rrggbb\"|null, line_smooth?, min_line_length?, line_style?: stroke|brush, fill_enclosed?, max_line_radius?, ink_threshold?: 0..255|null, uniform_width?: number|null} -> Trace Bitmap dialog's Trace",
        SetScene => "{setting: daylight|sunset|night|interior|storm, cast?: 0..6, horizon?: 0.15..0.95, figure_scale?: 0.1..1.5, lit?, clouds?, water?, frames?: 1..16000} -> Story panel's Set the scene",
        DirectScene => "{story: text, sequence?: bool (a scene per paragraph, as the panel's Direct as film)} -> Story panel's Direct: rebuilds the open shot from the prose",
        SceneryFor => "{scenery: forest|village|city|meadow|waterside, horizon?, art?: {trees?|grass?|buildings?|lamps?: library symbol name|id}} -> Story panel's Lay scenery",
        Perform => "{action: walk|run|talk|idle|sit|stand|turn|point|reach|react, element?: rig id (else the selection), from_frame?, to_frame?, amount?, tempo?, distance?, step?} -> Animate Selection's Animate",
        AddFollowThrough => "{element?: rig id (else the selection), root?: bone name|index, stiffness?, damping?, couple?: bool, live?: bool (modifier, default true; false bakes), from_frame?, to_frame?, step?} -> Follow-Through dialog's Add",
        AddTitleCard => "{text: the words (newlines start new lines; Hindi is shaped), font?: family name (default: the system face, or a Hindi face for Devanagari text), size?: 4..1000 (72), color?: \"#rrggbb\" (white), plate?: bool (a rounded backing plate, default false), plate_color?: \"#rrggbb\", plate_opacity?: 0..1 (0.7), preset?: typewriter|pop_words|fade_up|slide_in|stamp (fade_up), hold?: seconds 0..60 (2), position?: centre|lower_third|top (centre), start_frame?: 0-based (default the playhead)} -> Insert > Title Card's Add: a card symbol on a new top layer that ignores the lights, one undo step; returns {layer, symbol, start, frames, units}",
        AddWiggle => "{element?: id (else the selection), amplitude?, frequency?, live?: bool (modifier, default true; false bakes), from_frame?, to_frame?, step?} -> Wiggle dialog's Add",
        AiIdentifyAssets => "{renames?: [{symbol: name|id, name, tags?: [text]}] (applied as the rename dialog's Apply) | start?: true (VLM job, returns {started:true}; review in the AI panel) | apply_proposals?: true (apply the proposals that job produced)}",
        AiRigVectorCharacter => "{symbol: library symbol name|id, element?: instance id on stage} -> {started:true, job}; detects a skeleton on the GPU and rigs it as one undo step when done; call wait_for_jobs for the result",
        AiRigLayers => "{symbol?: library symbol name|id, element?: instance id on stage (its symbol; one of the two is required), frame?: symbol frame the pieces are read at (default 0), overrides?: [{layer: the layer's name now, slot?: slot or display name (Hips, Torso, Head, Upper Arm L, Forearm R, Hand L, Thigh R, Shin L, Foot R, Brow L...) | Extra (stays on its slot, never its lead), name?: new layer name}], apply?: bool (default true), profile?: naming profile name (default My Animate convention: head, body, leftArm, leftWrist = forearm, leftPalm = hand, midCloth, leftThigh, leftShin, leftFoot, screen sides, body/midCloth/thighs are roots, legs folder; Descriptive: Upper Arm R, Forearm R, Hand R, Torso, everything follows the hips; rig_naming.json beside the workspace can override), split_merged?: bool (default: on when a merged drawing is found, i.e. one vector piece covering more than one body part; cuts its fills and strokes along the skeleton into pieces, each child part overlapping its parent across the joint so bends show no gap; off: pieces as drawn)} -> {started:true, job}; AI > Rig Character as Layers: finds the body parts (DWPose when installed, else fitted to the drawings, lower confidence), puts every piece on its own layer inside the symbol named after its body part and rigs them by layer parenting, nothing moving on any frame; an override that moves a piece works the leads and names out again before its own name is used. Call wait_for_jobs: apply:false gives {applied:false, plan:{symbol, frame, source, confidence, low_confidence, pieces:[{layer, new_name, slot, lead, score}], symbol_renames, notes, side_calls, merged_found, split_merged:{drawings, parts, joint_overlaps, unsplit_keys}|null}} and changes nothing; otherwise one undo step and {applied:true, parts, slots, layers_before, layers_after, keys_rebaked, renamed_symbols, undo_steps_added, pieces}",
        AiDirectStory => "{brief: text} -> {started:true, job}; an LLM parses the brief and the result is applied as one undo step when done; call wait_for_jobs for the result",
        AiCharacterFromImage => "{path: image file (PNG/JPG of one character, full body), character_name?: text (default the file name), rig?: layers|bones (default layers)} -> {started:true, job}; the Character from Image wizard with its default choices (pose and skeleton, parts, hidden parts completed, vector trace, biped rig) as one undo step, rigged by layer parenting (one followed layer per part) or, with rig:bones, as an armature; takes a minute or two, so call wait_for_jobs for {character, symbols, elements, rig_kind: layers|bones, rig_root_element, armature_element}. rig_root_element is the id to pass as `element` to perform / ai_direct_story (the root part of a layer rig, the armature of a bone rig); armature_element is null for a layer rig. A rig whose artwork is skinned stays bones, and rig_kind says so. The rigged character is then on stage and in Cast",
        AiMouthChart => "{element: id of the character's rig on stage | character: name (finds the library symbol `<name>_mouth`), path: image file of a labelled mouth chart (mouths in rows on a plain page, coloured mouths with grey/black labels under them), labels?: [text] in reading order (default Raju's 19: Neutral Closed, Small Smile, Broad Smile, Frown, A, E, I, O, U, M/B/P, F/V, L, TH, CH/J, S/Z, Open Laugh, Angry Tight Mouth, Confused Half-Open, Skeptical Side-Mouth)} -> replaces the character's mouth-symbol frames, as one undo step: frames 0..9 are the ten lip-sync visemes (Rest, Ai, E, O, U, L, WQ, MBP, FV, Etc) so lip_sync keeps working, then every other mouth as an extra named frame (10.. : Small Smile, Broad Smile, Frown, I, TH, CH/J, Open Laugh...) to key by hand (an instance's first_frame, or the keyframe label). Sized to the current Rest mouth and drawn in its line weight. Takes a few seconds",
        ClearAllKeyframes => "{symbol: library symbol name|id, keep?: first|playhead (first; playhead = the inner frame a selected instance of it shows, else the open frame when that symbol is being edited, else first), nested?: bool (default true for a character: tag character or folder Cast)} -> Clear All Keyframes: every layer keeps one keyframe at frame 0 holding the kept pose, the rest become normal frames, lengths unchanged, audio-guide layers untouched; one undo step; returns {keyframes_cleared, layers, symbols}",
        CleanUpLayers => "{scope?: timeline|whole_scene (timeline = the timeline being edited; whole_scene = the stage and every library symbol), empty?: bool (true: delete layers with no artwork and no sound), trim?: bool (true: trim blank keyframes after each layer's last content, never below the longest content end), folders?: bool (true: remove folders with no children, including ones this run empties)} -> Clean Up Layers: never deletes a folder with children, a layer something files under or follows, a mask/guide with its run, an audio-guide layer, or the last layer; one undo step; returns {layers_deleted, frames_trimmed, folders_removed, symbols_touched}",
        ArrangeLayers => "{} -> Arrange Layers on the timeline being edited: deletes empty layers, then files top-level layers into folders (one per character with its layer-parenting chain, Background, FX, Props, Audio at the bottom), reusing same-named folders; a layer whose artwork overlaps a layer it would have to pass stays where it is, so the picture never changes; one undo step (none when already tidy); returns {folders_made, layers_moved, layers_left_in_place, emptied}",
        RepairLayerRigs => "{scope?: timeline|whole_scene (timeline = the timeline being edited; whole_scene = the stage and every library symbol)} -> Repair Layer Rigs: rewires layer-parenting rigs built with the old Bones -> Layers wiring (each part's front-most layer leads, the slot's other pieces and its child parts follow it), rebaking every keyframe so no frame changes; an already-correct rig is left alone; one undo step (none when nothing changes); returns {rigs, links_changed, keys_rebaked}",
        EvenLines => "{element?: id of a shape or instance (its symbol, recursively; default the selection, else the symbol being edited, else the whole stage), face_width?: 0.25..12 px, body_width?: 0.25..12 px (defaults: the measured median width of the face / body ink), keep_weight?: bool (true: each line keeps its measured weight relative to the median, 0.5-2x the width), straighten?: 0..1 (0), solid_above?: 0.5..40 px (ink deeper than this radius stays a filled area; default 1.25x the measured width)} -> Even Out Lines (Modify > Shape): filled ink shapes on \"Ink\" layers (and dark filled shapes that come out as line) on every keyframe of every layer become strokes of one width along each line, face lines (eyes, brows, lids, mouth, nose, moustache, head/face layers or symbols) and body lines each at their own width; solid areas stay fills; one undo step; returns {face_lines, body_lines, solids, face_width, body_width}",
        RemoveBackgroundSpecks => "{element?: id of a shape or instance (its symbol and every symbol inside; default the selection, else the symbol being edited, else the whole stage), colour?: \"#rrggbb\" (the sheet's paper colour; default: guessed from the light grey fills that touch the outside), reach?: 0..200 units (12: a speck lies wholly within this of the outline, or touches the outside), max_area?: 0..100000 square units (600: larger light fills are kept)} -> Remove Background Specks (Modify > Shape): fills of the paper colour (mean within dE 10) trapped between hair curls and body-hair strands near the character's outline are deleted (or cut out of a shape with other pieces) on every keyframe of every layer, in the symbols inside too; eyes, eyelids and mouths (by layer, symbol or object name) and larger light areas (a white shirt, a vest) are kept; one undo step; returns {shapes_removed, shapes_clipped, area, keyframes, colour, keyframes_not_judged}",
        RigHead => "{symbol?: head symbol name|id (or a character symbol: the head inside it), element?: instance id on stage (its symbol, same rule); default the selected instance, else the symbol being edited, else the head on the stage} -> Rig Head (Modify > Timeline): a head symbol whose face parts are separate layers or named objects is rebuilt like the user's heads, top first: eyeLid (7-frame blink symbol, resting open on frame 0, single frame), eyeMask (mask: the eye whites' fill without outline), eyeBall (masked: both irises/pupils, a cut iris completed as a circle; gaze = move this instance), eyeWhite (whites + outlines), rightEyebrow, leftEyebrow, extras, mouth (a symbol; an existing mouth-shape symbol is kept), Nose, hair, base; no layer parenting; rest picture unchanged; one undo step; an already rigged head is left alone; returns {head, view, layers, eyes, irises_completed, eye_white, eye_mask, eye_ball, eye_lid, mouth, skin, ink, already_rigged, notes}",
        PasteMotion | PasteMotionSpecial => "{relative?: bool (default true: each key is the change from the copied motion's first key, applied about the target's own transformation point, so it keeps its starting place; false writes the copied transforms as they are), parts?: [position|scale|rotation|colour|timing] (default all; rotation includes skew, colour is the colour effect and alpha, timing an instance's first frame and looping), stretch?: 0.25..4 (1; key offsets scaled and rounded, keys landing on one frame merged)} -> Paste Motion: the motion taken by copy_motion onto every selected object, each on its own layer, starting at the playhead, keyframes at the copied offsets with the copied tweens; frames outside the pasted span keep what they showed; works inside a symbol being edited; one undo step; returns {pasted, relative, parts, stretch}",
        PastePose => "{parts?: [slot names] (default every copied slot; any naming: Shoulder R, Upper Arm R, rightArm...)} -> Paste Pose: the joint angles taken by copy_pose (the selected rig at the playhead: a bone rig, a layer rig part, or a character instance with a rig inside its symbol) onto every selected rig, bone or layer, matched by slot name; angles only, each joint relative to its parent, so the target keeps its limb lengths and where its root stands; keyed at the playhead (inside an instance at the inner frame it shows; Protect Other Keyframes forks a shared symbol as a hand edit would); one undo step; returns {pasted: [slots], skipped: [copied slots the target lacks]}",
        CopyPerformance => "{from?: element id of the character that performs (a bone rig, any part of a layer rig, or an instance with a rig inside; default the selection), to?: element id of the character to paste onto (without it the performance is only copied), range?: [first, last] frames of the source (default the timeline's frame span when it covers the source, else its keyed span), offset?: the frame the paste starts on (default the playhead), stretch?: 0.25..4 (1), travel?: bool (true: also the source instance's own travel, relative, onto the target instance), scale_travel?: bool (true: that travel's change of place scaled by the target/source hip height; 1:1 with a note when not measurable)} -> Copy Performance, then Paste Performance onto 'to': the source's joint angles per key over the range, by slot name (Shoulder R = Upper Arm R = rightArm), keyed on the target's layers or armature with the source's key frames (scaled by stretch) and tweens; angles only, so the target keeps its limb lengths; the root's travel inside the rig scaled by the target/source hip height; layer and bone rigs in any combination; inside an instance through the in-place edit (Protect Other Keyframes forks a shared symbol); one undo step 'Paste Performance'; returns {keys_written, frames, slots, skipped, hip_ratio, travel_keys, notes}",
        PastePerformance | PastePerformanceSpecial => "{range?: [first, last] offsets within the copied performance (default all), offset?: the frame the paste starts on (default the playhead), stretch?: 0.25..4 (1), travel?: bool (true), scale_travel?: bool (true)} -> Paste Performance: the performance taken by copy_performance onto every selected rigged character (see copy_performance); one undo step; returns {keys_written, frames, slots, skipped, hip_ratio, travel_keys, notes}",
        StoryboardMake => "{episode: folder holding script.txt and casting.json (or buzz/casting.json), scenes?: \"4,5\" | \"1-3\" | [4,5] (default every scene), out?: folder for the board (default <casting folder>/storyboard), look?: auto|a|b (auto: A, the real scene setup, when every character has a Cast rig and the set is built; B, an AI sketch from ComfyUI, otherwise; a B that ComfyUI cannot draw falls back to A and says so), breakdown?: path of a breakdown.json to use as it is (no model is asked), offline?: bool (the breakdown from the script's own rules, no model)} -> Storyboard panel's Make, waited for: the breakdown (else LM Studio / the saved Claude model), the scenes built headless, storyboard.json merged with the one on disk (the user's edits kept), a picture per panel in out/panels (a medium/close framed on its subject); returns {board, panels:[{index, id, scene, shot, kind, on, duration_s, look_line, framing}], warnings}",
        StoryboardEdit => "{board?: path of a storyboard.json (default the Storyboard panel's), panel: panel id | 0-based index, kind?: wide|medium|close, on?: character name (\"\" for nobody), action?: text, notes?: text, duration_s?: 0.1..600, look?: auto|a|b, dialogue?: [[speaker, text]], move?: up|down|0-based index, add_after?: bool (a new panel of the user's own after it), split?: bool (two panels, lines and time shared out), delete?: bool (a made panel stays deleted when the board is made again), regenerate?: bool (draw it again as it is now, waited for)} -> edits storyboard.json at once (field edits are kept by a re-made board); returns {index, panel, panels}",
        StoryboardExport => "{board?: path of a storyboard.json (default the Storyboard panel's)} -> Storyboard panel's Export sheets, waited for: pages of six panels with captions (board_p01.png...) and board.pdf beside the board, from the pictures already drawn; returns {pages, pdf, warnings}",
        _ => return None,
    })
}

// -- argument helpers -------------------------------------------------------------

fn check_keys(a: &Value, allowed: &[&str]) -> Result<(), String> {
    let Some(map) = a.as_object() else { return Ok(()) };
    for key in map.keys() {
        if key != "name" && key != "args" && !allowed.contains(&key.as_str()) {
            return Err(format!("unknown arg '{key}'; valid: {}", allowed.join(", ")));
        }
    }
    Ok(())
}

fn given<'a>(a: &'a Value, key: &str) -> Option<&'a Value> {
    a.get(key).filter(|v| !v.is_null())
}

fn num(a: &Value, key: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    let Some(v) = given(a, key) else { return Ok(None) };
    let n = v
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("'{key}' needs a number"))?;
    if n < min || n > max {
        return Err(format!("'{key}' must be between {min} and {max}"));
    }
    Ok(Some(n))
}

fn int(a: &Value, key: &str, min: u32, max: u32) -> Result<Option<u32>, String> {
    let Some(n) = num(a, key, min as f64, max as f64)? else { return Ok(None) };
    if n.fract() != 0.0 {
        return Err(format!("'{key}' needs a whole number"));
    }
    Ok(Some(n as u32))
}

fn flag(a: &Value, key: &str) -> Result<Option<bool>, String> {
    let Some(v) = given(a, key) else { return Ok(None) };
    v.as_bool().map(Some).ok_or_else(|| format!("'{key}' needs true or false"))
}

fn text<'a>(a: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    let Some(v) = given(a, key) else { return Ok(None) };
    v.as_str().map(Some).ok_or_else(|| format!("'{key}' needs text"))
}

fn missing(list: &[&str]) -> AgentResult {
    AgentResult::err(format!("missing required args: {}", list.join(", ")))
}

fn depth(app: &App) -> usize {
    app.agent_editor().doc.history().undo_depth()
}

/// A library symbol by name, or by id (a number or numeric text).
fn symbol_by_ref(scene: &Scene, v: &Value, what: &str) -> Result<SymbolId, String> {
    let library = scene.library();
    let by_id = v
        .as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
        .map(SymbolId)
        .filter(|id| library.get(*id).is_some());
    let by_name = v.as_str().and_then(|s| library.iter().find(|x| x.name == s).map(|x| x.id));
    by_name.or(by_id).ok_or_else(|| {
        let names: Vec<String> = library.iter().map(|s| format!("{}={}", s.id.0, s.name)).collect();
        format!("no {what} symbol {v}; library: {}", names.join(", "))
    })
}

/// Select `args.element`, as clicking it would. Nothing in the document changes.
fn select_element(app: &mut App, a: &Value) -> Result<(), String> {
    let Some(v) = given(a, "element") else { return Ok(()) };
    let id = ObjectId(v.as_u64().ok_or("'element' needs an element id")?);
    let editor = app.agent_editor_mut();
    if editor.doc.scene().find_object(id).is_none() {
        return Err(format!("no element {} in this scene", id.0));
    }
    editor.selection.select_one(id);
    Ok(())
}

/// The finish every document-changing command shares: the report, the undo
/// steps it added, and `ok:false` with the editor's own words when nothing changed.
fn finish(app: &App, id: &str, before: usize, extra: Value) -> AgentResult {
    let after = depth(app);
    let mut value = report(app, id);
    value["undo_steps_added"] = json!(after as i64 - before as i64);
    if let (Some(map), Some(more)) = (value.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            map.insert(k.clone(), v.clone());
        }
    }
    if after == before {
        let why = app
            .agent_editor()
            .status
            .clone()
            .unwrap_or_else(|| "nothing changed".to_string());
        return AgentResult::err(format!("{id} changed nothing: {why}"));
    }
    AgentResult::ok(value)
}

// -- the commands -------------------------------------------------------------------

/// Run a dialog command from its args. Only called for commands where
/// [`takes_args`] is true.
pub fn run(app: &mut App, command: Command, a: &Value) -> AgentResult {
    let id = command.id();
    match command {
        Command::New => new_document(app, a),
        Command::LipSync => lip_sync(app, a),
        Command::TraceBitmapOptions => trace_options(app, a),
        Command::SetScene => set_scene(app, a),
        Command::DirectScene => direct_scene(app, a),
        Command::SceneryFor => scenery_for(app, a),
        Command::Perform => perform(app, a),
        Command::AddFollowThrough => follow_through(app, a),
        Command::AddWiggle => wiggle(app, a),
        Command::AddTitleCard => title_card(app, a),
        Command::AiIdentifyAssets => identify_assets(app, a),
        Command::AiRigVectorCharacter => rig_vector_character(app, a),
        Command::AiRigLayers => rig_layers(app, a),
        Command::AiDirectStory => direct_story(app, a),
        Command::AiCharacterFromImage => character_from_image(app, a),
        Command::AiMouthChart => mouth_chart(app, a),
        Command::ClearAllKeyframes => clear_symbol_keyframes(app, a),
        Command::CleanUpLayers => clean_up_layers(app, a),
        Command::ArrangeLayers => arrange_layers(app, a),
        Command::RepairLayerRigs => repair_layer_rigs(app, a),
        Command::RigHead => rig_head(app, a),
        Command::EvenLines => even_lines(app, a),
        Command::RemoveBackgroundSpecks => remove_background_specks(app, a),
        Command::PasteMotion | Command::PasteMotionSpecial => paste_motion(app, id, a),
        Command::PastePose => paste_pose(app, id, a),
        Command::CopyPerformance => copy_performance(app, id, a),
        Command::PastePerformance | Command::PastePerformanceSpecial => paste_performance(app, id, a),
        Command::StoryboardMake => storyboard_make(app, a),
        Command::StoryboardEdit => storyboard_edit(app, a),
        Command::StoryboardExport => storyboard_export(app, a),
        _ => AgentResult::err(format!("needs_dialog {id}")),
    }
}

/// Clear All Keyframes dialog -> `Editor::clear_symbol_keyframes`.
fn clear_symbol_keyframes(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["symbol", "keep", "nested"]) {
        return AgentResult::err(e);
    }
    let Some(v) = given(a, "symbol") else { return missing(&["symbol"]) };
    let id = match symbol_by_ref(app.agent_editor().doc.scene(), v, "") {
        Ok(id) => id,
        Err(e) => return AgentResult::err(e),
    };
    let playhead = match text(a, "keep") {
        Ok(None) | Ok(Some("first")) => false,
        Ok(Some("playhead")) => true,
        Ok(Some(other)) => return AgentResult::err(format!("'keep' must be first or playhead, not '{other}'")),
        Err(e) => return AgentResult::err(e),
    };
    let nested = match flag(a, "nested") {
        Ok(n) => n,
        Err(e) => return AgentResult::err(e),
    };
    let editor = app.agent_editor_mut();
    let nested = nested.unwrap_or_else(|| {
        editor.doc.scene().library().get(id).is_some_and(|s| crate::editor::is_character_symbol(s))
    });
    let keep = if playhead {
        editor
            .selected_instance_inner_frame(id)
            .or_else(|| (editor.doc.scene().editing_symbol() == Some(id)).then_some(editor.current_frame))
            .map(buzz_scene::KeepPose::Frame)
            .unwrap_or(buzz_scene::KeepPose::First)
    } else {
        buzz_scene::KeepPose::First
    };
    let before = depth(app);
    let report = app.agent_editor_mut().clear_symbol_keyframes(id, keep, nested);
    finish(
        app,
        "clear_symbol_keyframes",
        before,
        json!({
            "keyframes_cleared": report.keyframes_cleared,
            "layers": report.layers,
            "symbols": report.symbols,
        }),
    )
}

/// Modify > Timeline > Arrange Layers -> `Editor::arrange_layers`.
fn arrange_layers(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &[]) {
        return AgentResult::err(e);
    }
    let before = depth(app);
    let done = app.agent_editor_mut().arrange_layers();
    let extra = json!({
        "folders_made": done.folders_made,
        "layers_moved": done.layers_moved,
        "layers_left_in_place": done.layers_left_in_place,
        "emptied": done.emptied,
    });
    if !done.changed() {
        // Already tidy is an answer, not a failure.
        let mut value = report(app, "arrange_layers");
        value["undo_steps_added"] = json!(0);
        if let (Some(map), Some(more)) = (value.as_object_mut(), extra.as_object()) {
            for (k, v) in more {
                map.insert(k.clone(), v.clone());
            }
        }
        return AgentResult::ok(value);
    }
    finish(app, "arrange_layers", before, extra)
}

/// Paste Motion / Paste Motion Special -> `Editor::paste_motion` (C1).
fn paste_motion(app: &mut App, id: &str, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["relative", "parts", "stretch"]) {
        return AgentResult::err(e);
    }
    let mut options = crate::editor::MotionPasteOptions::default();
    match flag(a, "relative") {
        Ok(Some(r)) => options.relative = r,
        Ok(None) => {}
        Err(e) => return AgentResult::err(e),
    }
    match num(a, "stretch", 0.25, 4.0) {
        Ok(Some(s)) => options.stretch = s,
        Ok(None) => {}
        Err(e) => return AgentResult::err(e),
    }
    if let Some(parts) = given(a, "parts") {
        let Some(list) = parts.as_array() else {
            return AgentResult::err("'parts' needs a list of position|scale|rotation|colour|timing");
        };
        options.position = false;
        options.scale = false;
        options.rotation = false;
        options.colour = false;
        options.timing = false;
        for part in list {
            match part.as_str() {
                Some("position") => options.position = true,
                Some("scale") => options.scale = true,
                Some("rotation") => options.rotation = true,
                Some("colour") | Some("color") => options.colour = true,
                Some("timing") => options.timing = true,
                _ => {
                    return AgentResult::err(format!(
                        "unknown part {part}; valid: position, scale, rotation, colour, timing"
                    ));
                }
            }
        }
    }
    let before = depth(app);
    let pasted = app.agent_editor_mut().paste_motion(options);
    let parts: Vec<&str> = [
        ("position", options.position),
        ("scale", options.scale),
        ("rotation", options.rotation),
        ("colour", options.colour),
        ("timing", options.timing),
    ]
    .iter()
    .filter(|(_, on)| *on)
    .map(|(n, _)| *n)
    .collect();
    finish(
        app,
        id,
        before,
        json!({
            "pasted": pasted,
            "relative": options.relative,
            "parts": parts,
            "stretch": options.stretch,
        }),
    )
}

/// Paste Pose -> `Editor::paste_pose` (C2).
fn paste_pose(app: &mut App, id: &str, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["parts"]) {
        return AgentResult::err(e);
    }
    let mut parts: Option<Vec<String>> = None;
    if let Some(list) = given(a, "parts") {
        let Some(list) = list.as_array() else {
            return AgentResult::err("'parts' needs a list of slot names");
        };
        let full = buzz_rig::RigPattern::biped_full();
        let mut names = Vec::new();
        for part in list {
            match part.as_str().filter(|n| full.slot_for(n).is_some()) {
                Some(n) => names.push(n.to_string()),
                None => {
                    let slots: Vec<&str> = full.slots.iter().map(|s| s.name.as_str()).collect();
                    return AgentResult::err(format!("unknown slot {part}; valid: {}", slots.join(", ")));
                }
            }
        }
        parts = Some(names);
    }
    let before = depth(app);
    let report = app.agent_editor_mut().paste_pose(parts.as_deref());
    let (pasted, skipped) = report.map(|r| (r.pasted, r.skipped)).unwrap_or_default();
    finish(app, id, before, json!({ "pasted": pasted, "skipped": skipped }))
}

/// A `[first, last]` frame pair.
fn frame_pair(a: &Value, key: &str) -> Result<Option<(u32, u32)>, String> {
    let Some(v) = given(a, key) else { return Ok(None) };
    let pair = v
        .as_array()
        .filter(|p| p.len() == 2)
        .and_then(|p| Some((p[0].as_u64()?, p[1].as_u64()?)))
        .filter(|(x, y)| *x <= u32::MAX as u64 && *y <= u32::MAX as u64)
        .ok_or_else(|| format!("'{key}' needs [first, last] frame numbers"))?;
    Ok(Some((pair.0 as u32, pair.1 as u32)))
}

/// An element id that is in the scene.
fn element_arg(app: &App, a: &Value, key: &str) -> Result<Option<ObjectId>, String> {
    let Some(v) = given(a, key) else { return Ok(None) };
    let id = ObjectId(v.as_u64().ok_or_else(|| format!("'{key}' needs an element id"))?);
    if app.agent_editor().doc.scene().find_object(id).is_none() {
        return Err(format!("no element {} in this scene", id.0));
    }
    Ok(Some(id))
}

/// `{offset?, stretch?, travel?, scale_travel?}` onto the default paste options.
fn performance_options(a: &Value) -> Result<crate::editor::PerformancePasteOptions, String> {
    let mut options = crate::editor::PerformancePasteOptions::default();
    if let Some(o) = int(a, "offset", 0, 1_000_000)? {
        options.offset = Some(o);
    }
    if let Some(s) = num(a, "stretch", 0.25, 4.0)? {
        options.stretch = s;
    }
    if let Some(t) = flag(a, "travel")? {
        options.travel = t;
    }
    if let Some(t) = flag(a, "scale_travel")? {
        options.scale_travel = t;
    }
    Ok(options)
}

/// The report fields Copy / Paste Performance return.
fn performance_report(report: Option<crate::editor::PastePerformanceReport>) -> Value {
    let r = report.unwrap_or_default();
    json!({
        "keys_written": r.keys_written,
        "frames": r.frames,
        "slots": r.slots,
        "skipped": r.skipped,
        "hip_ratio": r.hip_ratio,
        "travel_keys": r.travel_keys,
        "notes": r.notes,
    })
}

/// Copy Performance -> `Editor::copy_performance`, then, with `to`,
/// `Editor::paste_performance` (C3).
fn copy_performance(app: &mut App, id: &str, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["from", "to", "range", "offset", "stretch", "travel", "scale_travel"]) {
        return AgentResult::err(e);
    }
    let (from, to, range, options) = match (
        element_arg(app, a, "from"),
        element_arg(app, a, "to"),
        frame_pair(a, "range"),
        performance_options(a),
    ) {
        (Ok(f), Ok(t), Ok(r), Ok(o)) => (f, t, r, o),
        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => return AgentResult::err(e),
    };
    if let Some(from) = from {
        app.agent_editor_mut().selection.select_one(from);
    }
    if !app.agent_editor_mut().copy_performance(range) {
        let why = app.agent_editor().status.clone().unwrap_or_default();
        return AgentResult::err(format!("{id}: {why}"));
    }
    let Some(to) = to else {
        // Copied only: paste with paste_performance, or give `to`.
        let mut value = report(app, id);
        value["undo_steps_added"] = json!(0);
        if let (Some(map), Some(more)) = (value.as_object_mut(), performance_report(None).as_object()) {
            for (k, v) in more {
                map.insert(k.clone(), v.clone());
            }
        }
        return AgentResult::ok(value);
    };
    app.agent_editor_mut().selection.select_one(to);
    let before = depth(app);
    let done = app.agent_editor_mut().paste_performance(options);
    finish(app, id, before, performance_report(done))
}

/// Paste Performance / Paste Performance Special -> `Editor::paste_performance` (C3).
fn paste_performance(app: &mut App, id: &str, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["range", "offset", "stretch", "travel", "scale_travel"]) {
        return AgentResult::err(e);
    }
    let (range, mut options) = match (frame_pair(a, "range"), performance_options(a)) {
        (Ok(r), Ok(o)) => (r, o),
        (Err(e), _) | (_, Err(e)) => return AgentResult::err(e),
    };
    options.range = range;
    let before = depth(app);
    let done = app.agent_editor_mut().paste_performance(options);
    finish(app, id, before, performance_report(done))
}

/// Repair Layer Rigs -> `Editor::repair_layer_rigs`.
fn repair_layer_rigs(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["scope"]) {
        return AgentResult::err(e);
    }
    let scope = match text(a, "scope") {
        Ok(None) | Ok(Some("timeline")) => buzz_scene::CleanupScope::Timeline,
        Ok(Some("whole_scene")) => buzz_scene::CleanupScope::WholeScene,
        Ok(Some(other)) => {
            return AgentResult::err(format!("'scope' must be timeline or whole_scene, not '{other}'"));
        }
        Err(e) => return AgentResult::err(e),
    };
    let before = depth(app);
    let report = app.agent_editor_mut().repair_layer_rigs(scope);
    finish(
        app,
        "repair_layer_rigs",
        before,
        json!({
            "rigs": report.rigs,
            "links_changed": report.links_changed,
            "keys_rebaked": report.keys_rebaked,
        }),
    )
}

/// Rig Head -> `Editor::rig_head` (`docs/HEAD_RIG_PLAN.md` H1).
fn rig_head(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["symbol", "element"]) {
        return AgentResult::err(e);
    }
    let scene = app.agent_editor().doc.scene();
    let symbol = match (given(a, "symbol"), given(a, "element")) {
        (Some(v), _) => match symbol_by_ref(scene, v, "head") {
            Ok(id) => Some(id),
            Err(e) => return AgentResult::err(e),
        },
        (None, Some(v)) => {
            let Some(id) = v.as_u64() else { return AgentResult::err("'element' needs an element id") };
            match scene.find_object(ObjectId(id)).map(|(_, o)| o.instance().map(|i| i.symbol)) {
                Some(Some(symbol)) => Some(symbol),
                Some(None) => return AgentResult::err(format!("element {id} is not a symbol instance")),
                None => return AgentResult::err(format!("no element {id} in this scene")),
            }
        }
        (None, None) => None,
    };
    let before = depth(app);
    let report = match app.agent_editor_mut().rig_head(symbol) {
        Ok(report) => report,
        Err(e) => return AgentResult::err(e),
    };
    let value = json!({
        "head": report.symbol.map(|s| s.0),
        "view": report.view,
        "layers": report.layers,
        "eyes": report.eyes,
        "irises_completed": report.irises_completed,
        "eye_white": report.eye_white.map(|s| s.0),
        "eye_mask": report.eye_mask.map(|s| s.0),
        "eye_ball": report.eye_ball.map(|s| s.0),
        "eye_lid": report.eye_lid.map(|s| s.0),
        "mouth": report.mouth.map(|s| s.0),
        "skin": format!("#{:02x}{:02x}{:02x}", report.skin[0], report.skin[1], report.skin[2]),
        "ink": format!("#{:02x}{:02x}{:02x}", report.ink[0], report.ink[1], report.ink[2]),
        "already_rigged": report.already_rigged,
        "notes": report.notes,
    });
    if report.already_rigged {
        return AgentResult::ok(value);
    }
    finish(app, "rig_head", before, value)
}

/// Even Out Lines dialog -> `Editor::even_out_lines` (`docs/EVEN_LINES_PLAN.md` E2).
fn even_lines(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["element", "face_width", "body_width", "keep_weight", "straighten", "solid_above"]) {
        return AgentResult::err(e);
    }
    let parsed = (|| -> Result<crate::editor::EvenLinesRequest, String> {
        Ok(crate::editor::EvenLinesRequest {
            face_width: num(a, "face_width", 0.25, 12.0)?,
            body_width: num(a, "body_width", 0.25, 12.0)?,
            keep_weight: flag(a, "keep_weight")?,
            straighten: num(a, "straighten", 0.0, 1.0)?,
            solid_above: num(a, "solid_above", 0.5, 40.0)?,
        })
    })();
    let request = match parsed {
        Ok(r) => r,
        Err(e) => return AgentResult::err(e),
    };
    let element = match given(a, "element") {
        Some(v) => {
            let Some(id) = v.as_u64() else { return AgentResult::err("'element' needs an element id") };
            if app.agent_editor().doc.scene().find_object(ObjectId(id)).is_none() {
                return AgentResult::err(format!("no element {id} in this scene"));
            }
            Some(ObjectId(id))
        }
        None => None,
    };
    let before = depth(app);
    let report = match app.agent_editor_mut().even_out_lines(element, request) {
        Ok(report) => report,
        Err(e) => return AgentResult::err(e),
    };
    let value = json!({
        "face_lines": report.face_lines,
        "body_lines": report.body_lines,
        "solids": report.solids,
        "face_width": report.face_width,
        "body_width": report.body_width,
    });
    finish(app, "even_lines", before, value)
}

/// Remove Background Specks -> `Editor::remove_background_specks` (A6b of
/// `docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`).
fn remove_background_specks(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["element", "colour", "reach", "max_area"]) {
        return AgentResult::err(e);
    }
    let parsed = (|| -> Result<crate::editor::SpecksRequest, String> {
        let colour = match text(a, "colour")? {
            Some(t) => {
                let c = parse_hex(t).ok_or("'colour' needs a colour like \"#rrggbb\"")?.to_rgba8();
                Some([c.r, c.g, c.b])
            }
            None => None,
        };
        Ok(crate::editor::SpecksRequest {
            colour,
            reach: num(a, "reach", 0.0, 200.0)?,
            max_area: num(a, "max_area", 0.0, 100000.0)?,
        })
    })();
    let request = match parsed {
        Ok(r) => r,
        Err(e) => return AgentResult::err(e),
    };
    let element = match given(a, "element") {
        Some(v) => {
            let Some(id) = v.as_u64() else { return AgentResult::err("'element' needs an element id") };
            if app.agent_editor().doc.scene().find_object(ObjectId(id)).is_none() {
                return AgentResult::err(format!("no element {id} in this scene"));
            }
            Some(ObjectId(id))
        }
        None => None,
    };
    let before = depth(app);
    let report = match app.agent_editor_mut().remove_background_specks(element, request) {
        Ok(report) => report,
        Err(e) => return AgentResult::err(e),
    };
    let value = json!({
        "shapes_removed": report.shapes_removed,
        "shapes_clipped": report.shapes_clipped,
        "area": report.area,
        "keyframes": report.keyframes,
        "colour": report.colour.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
        "keyframes_not_judged": report.keyframes_not_judged,
    });
    finish(app, "remove_background_specks", before, value)
}

/// Clean Up Layers dialog -> `Editor::clean_up_layers`.
fn clean_up_layers(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["scope", "empty", "trim", "folders"]) {
        return AgentResult::err(e);
    }
    let scope = match text(a, "scope") {
        Ok(None) | Ok(Some("timeline")) => buzz_scene::CleanupScope::Timeline,
        Ok(Some("whole_scene")) => buzz_scene::CleanupScope::WholeScene,
        Ok(Some(other)) => {
            return AgentResult::err(format!("'scope' must be timeline or whole_scene, not '{other}'"));
        }
        Err(e) => return AgentResult::err(e),
    };
    let mut options = buzz_scene::CleanupOptions::default();
    for (key, slot) in [
        ("empty", &mut options.empty_layers),
        ("trim", &mut options.trim),
        ("folders", &mut options.folders),
    ] {
        match flag(a, key) {
            Ok(Some(v)) => *slot = v,
            Ok(None) => {}
            Err(e) => return AgentResult::err(e),
        }
    }
    let before = depth(app);
    let report = app.agent_editor_mut().clean_up_layers(scope, options);
    finish(
        app,
        "clean_up_layers",
        before,
        json!({
            "layers_deleted": report.layers_deleted,
            "frames_trimmed": report.frames_trimmed,
            "folders_removed": report.folders_removed,
            "symbols_touched": report.symbols_touched,
        }),
    )
}

/// New dialog -> `Editor::create_document`.
fn new_document(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["width", "height", "fps", "background", "discard_changes"]) {
        return AgentResult::err(e);
    }
    let editor = app.agent_editor();
    let mut setup = editor.workspace.new_document;
    let parsed = (|| -> Result<(), String> {
        if let Some(w) = num(a, "width", 1.0, 16384.0)? {
            setup.width = w;
        }
        if let Some(h) = num(a, "height", 1.0, 16384.0)? {
            setup.height = h;
        }
        if let Some(f) = num(a, "fps", 0.01, 240.0)? {
            setup.frame_rate = f;
        }
        if let Some(text) = text(a, "background")? {
            let c = parse_hex(text).ok_or("'background' needs a colour like \"#rrggbb\"")?;
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            setup.background = [r, g, b];
        }
        Ok(())
    })();
    if let Err(e) = parsed {
        return AgentResult::err(e);
    }
    let discard = match flag(a, "discard_changes") {
        Ok(d) => d.unwrap_or(false),
        Err(e) => return AgentResult::err(e),
    };
    if editor.doc.is_dirty() && !discard {
        return AgentResult::err(
            "the document has unsaved changes; save it first, or pass discard_changes:true to replace it",
        );
    }
    let editor = app.agent_editor_mut();
    editor.status = None;
    editor.create_document(setup);
    let mut value = report(app, "new");
    value["width"] = json!(setup.width);
    value["height"] = json!(setup.height);
    value["fps"] = json!(setup.frame_rate);
    AgentResult::ok(value)
}

/// Lip Sync dialog -> `Editor::run_lip_sync`.
fn lip_sync(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["mouth", "make_mouth", "layer", "silence", "hold"]) {
        return AgentResult::err(e);
    }
    let make = match flag(a, "make_mouth") {
        Ok(m) => m.unwrap_or(false),
        Err(e) => return AgentResult::err(e),
    };
    if given(a, "mouth").is_none() && !make {
        return missing(&["mouth (a library symbol name|id, or make_mouth:true)"]);
    }
    let silence = match num(a, "silence", 0.0, 1.0) {
        Ok(v) => v.unwrap_or(0.06) as f32,
        Err(e) => return AgentResult::err(e),
    };
    let hold = match int(a, "hold", 0, 1000) {
        Ok(v) => v.unwrap_or(2),
        Err(e) => return AgentResult::err(e),
    };
    let scene = app.agent_editor().doc.scene().clone();
    let layer: LayerId = match given(a, "layer") {
        Some(v) => {
            let text = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            match layer_by_ref(&scene, &text) {
                Ok(l) => l,
                Err(e) => return AgentResult::err(e),
            }
        }
        None => match app.agent_editor().selection.active_layer() {
            Some(l) => l,
            None => return missing(&["layer (no active layer to default to)"]),
        },
    };
    let mouth = if make {
        None
    } else {
        match symbol_by_ref(&scene, &a["mouth"], "mouth") {
            Ok(m) => Some(m),
            Err(e) => return AgentResult::err(e),
        }
    };
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.status = None;
    editor.lip_sync = buzz_ui::LipSyncState::opened();
    editor.lip_sync.silence = silence;
    editor.lip_sync.hold = hold;
    editor.lip_sync.layer = Some(layer.0);
    let mouth = match mouth {
        Some(m) => m,
        // The dialog's "make a placeholder mouth" button.
        None => editor.new_mouth_symbol(),
    };
    editor.lip_sync.mouth = Some(mouth.0);
    editor.run_lip_sync();
    let result = editor.lip_sync.result.clone();
    editor.lip_sync.close();
    if depth(app) == before {
        return AgentResult::err(result.unwrap_or_else(|| "lip sync changed nothing".into()));
    }
    finish(app, "lip_sync", before, json!({"result": result, "mouth": mouth.0, "layer": layer.0}))
}

/// Trace Bitmap dialog -> `Editor::confirm_trace_options`.
fn trace_options(app: &mut App, a: &Value) -> AgentResult {
    use buzz_scene::{LineStyle, TraceMode};
    const KEYS: &[&str] = &[
        "element", "colours", "detail", "smooth", "speckle", "drop_background", "mode",
        "outline_contrast", "line_colour", "line_smooth", "min_line_length", "line_style",
        "fill_enclosed", "max_line_radius", "ink_threshold", "uniform_width",
    ];
    if let Err(e) = check_keys(a, KEYS) {
        return AgentResult::err(e);
    }
    let mut o = app.agent_editor().workspace.trace;
    let parsed = (|| -> Result<(), String> {
        if let Some(n) = int(a, "colours", 2, 64)? {
            o.colours = n as usize;
        }
        if let Some(n) = num(a, "detail", 0.0, 64.0)? {
            o.detail = n;
        }
        if let Some(n) = num(a, "smooth", 0.0, 1.0)? {
            o.smooth = n;
        }
        if let Some(n) = int(a, "speckle", 0, 1_000_000)? {
            o.speckle = n as usize;
        }
        if let Some(b) = flag(a, "drop_background")? {
            o.drop_background = b;
        }
        if let Some(m) = text(a, "mode")? {
            o.mode = pick_named(
                &[TraceMode::Auto, TraceMode::Outline, TraceMode::LineArt, TraceMode::ColourLines, TraceMode::Both, TraceMode::Ink, TraceMode::InkAndColour],
                m,
                "mode",
            )?;
        }
        if let Some(n) = num(a, "outline_contrast", 0.0, 255.0)? {
            o.outline_contrast = n;
        }
        if a.get("line_colour").is_some() {
            o.line_colour = match given(a, "line_colour") {
                None => None,
                Some(v) => Some(
                    v.as_str()
                        .and_then(parse_hex)
                        .ok_or("'line_colour' needs a colour like \"#rrggbb\", or null")?,
                ),
            };
        }
        if let Some(n) = num(a, "line_smooth", 0.0, 1.0)? {
            o.line_smooth = n;
        }
        if let Some(n) = num(a, "min_line_length", 0.0, 100_000.0)? {
            o.min_line_length = n;
        }
        if let Some(s) = text(a, "line_style")? {
            o.line_style = pick_named(&[LineStyle::Stroke, LineStyle::Brush], s, "line_style")?;
        }
        if let Some(b) = flag(a, "fill_enclosed")? {
            o.fill_enclosed = b;
        }
        if let Some(n) = num(a, "max_line_radius", 0.0, 1000.0)? {
            o.max_line_radius = n;
        }
        if a.get("ink_threshold").is_some() {
            o.ink_threshold = int(a, "ink_threshold", 0, 255)?.map(|n| n as u8);
        }
        if a.get("uniform_width").is_some() {
            o.uniform_width = num(a, "uniform_width", 0.0, 1000.0)?;
        }
        Ok(())
    })();
    if let Err(e) = parsed {
        return AgentResult::err(e);
    }
    if let Err(e) = select_element(app, a) {
        return AgentResult::err(e);
    }
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.status = None;
    editor.confirm_trace_options(o);
    editor.doc.end_gesture();
    finish(app, "trace_bitmap_options", before, json!({}))
}

fn setting_arg(a: &Value, required: bool) -> Result<Option<buzz_ui::SettingChoice>, String> {
    match text(a, "setting")? {
        Some(s) => pick_named(&buzz_ui::SettingChoice::ALL, s, "setting").map(Some),
        None if required => Err("missing required args: setting".into()),
        None => Ok(None),
    }
}

/// Story panel's **Set the scene** -> `Editor::story_set_scene`.
fn set_scene(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["setting", "cast", "horizon", "figure_scale", "lit", "clouds", "water", "frames"]) {
        return AgentResult::err(e);
    }
    let mut state = app.agent_editor().story.clone();
    state.frames = app.agent_editor().doc.scene().frame_count();
    let parsed = (|| -> Result<(), String> {
        state.setting = setting_arg(a, true)?.expect("required");
        if let Some(n) = int(a, "cast", 0, 6)? {
            state.cast = n as usize;
        }
        if let Some(n) = num(a, "horizon", 0.15, 0.95)? {
            state.horizon = n;
        }
        if let Some(n) = num(a, "figure_scale", 0.1, 1.5)? {
            state.figure_scale = n;
        }
        if let Some(b) = flag(a, "lit")? {
            state.lit = b;
        }
        if let Some(b) = flag(a, "clouds")? {
            state.clouds = b;
        }
        if let Some(b) = flag(a, "water")? {
            state.water = b;
        }
        if let Some(n) = int(a, "frames", 1, 16000)? {
            state.frames = n;
        }
        Ok(())
    })();
    if let Err(e) = parsed {
        return AgentResult::err(e);
    }
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.status = None;
    editor.story = state.clone();
    editor.story_set_scene(&state);
    finish(app, "set_scene", before, json!({}))
}

/// Story panel's **Direct** -> `Editor::story_direct` (or `direct_sequence`).
fn direct_scene(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["story", "sequence"]) {
        return AgentResult::err(e);
    }
    let story = match text(a, "story") {
        Ok(Some(s)) if !s.trim().is_empty() => s.to_string(),
        Ok(_) => return missing(&["story"]),
        Err(e) => return AgentResult::err(e),
    };
    let sequence = match flag(a, "sequence") {
        Ok(s) => s.unwrap_or(false),
        Err(e) => return AgentResult::err(e),
    };
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.status = None;
    if sequence {
        let shots = editor.direct_sequence(&story);
        editor.doc.end_gesture();
        if shots == 0 {
            let why = editor.status.clone().unwrap_or_else(|| "nothing readable in the story".into());
            return AgentResult::err(why);
        }
        // Each paragraph is a scene of its own, which the history does not
        // count as a step, so success is the shots made rather than a depth.
        let mut value = report(app, "direct_scene");
        value["undo_steps_added"] = json!(depth(app) as i64 - before as i64);
        value["shots"] = json!(shots);
        return AgentResult::ok(value);
    }
    let mut state = std::mem::take(&mut editor.story);
    state.draft = story;
    editor.story_direct(&mut state);
    let report_text = state.report.clone();
    let ignored = state.ignored.clone();
    editor.story = state;
    editor.doc.end_gesture();
    finish(app, "direct_scene", before, json!({"report": report_text, "ignored": ignored}))
}

/// Story panel's **Lay scenery** -> `Editor::story_lay_scenery`.
fn scenery_for(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["scenery", "horizon", "art"]) {
        return AgentResult::err(e);
    }
    let Some(kind) = (match text(a, "scenery") {
        Ok(k) => k,
        Err(e) => return AgentResult::err(e),
    }) else {
        return missing(&["scenery"]);
    };
    let choice = match pick_named(&buzz_ui::SceneryChoice::ALL, kind, "scenery") {
        Ok(c) => c,
        Err(e) => return AgentResult::err(e),
    };
    let scene = app.agent_editor().doc.scene().clone();
    let mut state = app.agent_editor().story.clone();
    state.scenery = choice;
    let parsed = (|| -> Result<(), String> {
        if let Some(n) = num(a, "horizon", 0.15, 0.95)? {
            state.horizon = n;
        }
        if let Some(art) = given(a, "art") {
            let map = art.as_object().ok_or("'art' needs an object like {trees: \"Tree\"}")?;
            for (part, v) in map {
                // `StoryState::art` is indexed in `SceneryPartChoice::ALL` order.
                let found = buzz_ui::SceneryPartChoice::ALL
                    .iter()
                    .position(|p| norm(&format!("{p:?}")) == norm(part))
                    .ok_or_else(|| format!("unknown scenery part '{part}'; valid: trees, grass, buildings, lamps"))?;
                state.art[found] = symbol_by_ref(&scene, v, part)?.0;
            }
        }
        Ok(())
    })();
    if let Err(e) = parsed {
        return AgentResult::err(e);
    }
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.status = None;
    editor.story = state.clone();
    let pieces = editor.story_lay_scenery(&state);
    finish(app, "scenery_for", before, json!({"pieces": pieces}))
}

/// The frame range arguments, defaulting as the dialogs open: the playhead to
/// the end of the film.
fn frame_range(app: &App, a: &Value) -> Result<(u32, u32), String> {
    let editor = app.agent_editor();
    let last = editor.doc.scene().frame_count().saturating_sub(1);
    let from = int(a, "from_frame", 0, 16000)?.unwrap_or(editor.current_frame.min(last));
    let to = int(a, "to_frame", 0, 16000)?.unwrap_or(last).max(from);
    Ok((from, to))
}

/// Animate Selection dialog -> `Editor::perform_selection`.
fn perform(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["action", "element", "from_frame", "to_frame", "amount", "tempo", "distance", "step"]) {
        return AgentResult::err(e);
    }
    let Some(action) = (match text(a, "action") {
        Ok(v) => v,
        Err(e) => return AgentResult::err(e),
    }) else {
        return missing(&["action"]);
    };
    let mut state = std::mem::take(&mut app.agent_editor_mut().staging);
    let built = (|| -> Result<(), String> {
        state.action = pick_named(&buzz_ui::ActionChoice::ALL, action, "action")?;
        let (from, to) = frame_range(app, a)?;
        state.open_perform(from, to);
        if let Some(n) = num(a, "amount", 0.0, 10.0)? {
            state.amount = n;
        }
        if let Some(n) = num(a, "tempo", 0.05, 10.0)? {
            state.tempo = n;
        }
        if let Some(n) = num(a, "distance", -100_000.0, 100_000.0)? {
            state.distance = n;
        }
        if let Some(n) = int(a, "step", 1, 48)? {
            state.step = n;
        }
        select_element(app, a)
    })();
    let editor = app.agent_editor_mut();
    if let Err(e) = built {
        editor.staging = state;
        return AgentResult::err(e);
    }
    let before = editor.doc.history().undo_depth();
    editor.doc.end_gesture();
    editor.status = None;
    editor.perform_selection(&mut state);
    let problem = state.problem.take();
    state.close();
    editor.staging = state;
    editor.doc.end_gesture();
    if let Some(p) = problem {
        return AgentResult::err(p);
    }
    finish(app, "perform", before, json!({}))
}

/// Follow-Through dialog -> `Editor::add_follow_through`.
fn follow_through(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(
        a,
        &["element", "root", "stiffness", "damping", "couple", "live", "from_frame", "to_frame", "step"],
    ) {
        return AgentResult::err(e);
    }
    if let Err(e) = select_element(app, a) {
        return AgentResult::err(e);
    }
    // The selected rig's bone names, gathered as the command does.
    let bones: Option<Vec<String>> = {
        let editor = app.agent_editor();
        let scene = editor.doc.scene();
        editor.selection.iter().find_map(|id| {
            scene.find_object(id).and_then(|(_, o)| match &o.kind {
                ObjectKind::Armature(rig) => {
                    Some(rig.armature.bones.iter().map(|b| b.name.clone()).collect())
                }
                _ => None,
            })
        })
    };
    let Some(bones) = bones else {
        return AgentResult::err("select a rigged character (or pass element: a rig id) to add follow-through to");
    };
    let mut state = std::mem::take(&mut app.agent_editor_mut().staging);
    let built = (|| -> Result<(), String> {
        let (from, to) = frame_range(app, a)?;
        state.open_physics(from, to, bones.clone());
        if let Some(root) = given(a, "root") {
            let index = match root.as_str() {
                Some(name) => bones
                    .iter()
                    .position(|b| b == name)
                    .or_else(|| name.trim().parse::<usize>().ok().filter(|i| *i < bones.len()))
                    .ok_or_else(|| format!("no bone '{name}'; bones: {}", bones.join(", ")))?,
                None => root
                    .as_u64()
                    .map(|i| i as usize)
                    .filter(|i| *i < bones.len())
                    .ok_or_else(|| format!("'root' needs a bone name or index; bones: {}", bones.join(", ")))?,
            };
            state.physics_root = index;
        }
        if let Some(n) = num(a, "stiffness", 0.1, 10_000.0)? {
            state.spring_stiffness = n;
        }
        if let Some(n) = num(a, "damping", 0.0, 1000.0)? {
            state.spring_damping = n;
        }
        if let Some(b) = flag(a, "couple")? {
            state.physics_couple = b;
        }
        state.physics_live = flag(a, "live")?.unwrap_or(true);
        if let Some(n) = int(a, "step", 1, 48)? {
            state.step = n;
        }
        Ok(())
    })();
    let editor = app.agent_editor_mut();
    if let Err(e) = built {
        editor.staging = state;
        return AgentResult::err(e);
    }
    let before = editor.doc.history().undo_depth();
    editor.doc.end_gesture();
    editor.status = None;
    editor.add_follow_through(&mut state);
    let problem = state.problem.take();
    state.close();
    editor.staging = state;
    editor.doc.end_gesture();
    if let Some(p) = problem {
        return AgentResult::err(p);
    }
    finish(app, "add_follow_through", before, json!({}))
}

/// Wiggle dialog -> `Editor::add_wiggle`.
fn wiggle(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["element", "amplitude", "frequency", "live", "from_frame", "to_frame", "step"]) {
        return AgentResult::err(e);
    }
    if let Err(e) = select_element(app, a) {
        return AgentResult::err(e);
    }
    let any = {
        let editor = app.agent_editor();
        let scene = editor.doc.scene();
        editor.selection.iter().any(|id| scene.find_object(id).is_some())
    };
    if !any {
        return AgentResult::err("select an object (or pass element: an id) to add a wiggle to");
    }
    let mut state = std::mem::take(&mut app.agent_editor_mut().staging);
    let built = (|| -> Result<(), String> {
        let (from, to) = frame_range(app, a)?;
        state.open_wiggle(from, to);
        if let Some(n) = num(a, "amplitude", 0.0, 10_000.0)? {
            state.wiggle_amplitude = n;
        }
        if let Some(n) = num(a, "frequency", 0.01, 100.0)? {
            state.wiggle_frequency = n;
        }
        state.wiggle_live = flag(a, "live")?.unwrap_or(true);
        if let Some(n) = int(a, "step", 1, 48)? {
            state.step = n;
        }
        Ok(())
    })();
    let editor = app.agent_editor_mut();
    if let Err(e) = built {
        editor.staging = state;
        return AgentResult::err(e);
    }
    let before = editor.doc.history().undo_depth();
    editor.doc.end_gesture();
    editor.status = None;
    editor.add_wiggle(&mut state);
    let problem = state.problem.take();
    state.close();
    editor.staging = state;
    editor.doc.end_gesture();
    if let Some(p) = problem {
        return AgentResult::err(p);
    }
    finish(app, "add_wiggle", before, json!({}))
}

/// Title Card dialog -> `Editor::add_title_card`.
fn title_card(app: &mut App, a: &Value) -> AgentResult {
    let keys = [
        "text", "font", "size", "color", "plate", "plate_color", "plate_opacity", "preset", "hold",
        "position", "start_frame",
    ];
    if let Err(e) = check_keys(a, &keys) {
        return AgentResult::err(e);
    }
    let mut settings = app.agent_editor().title_card.settings.clone();
    let mut start = None;
    let parsed = (|| -> Result<(), String> {
        let Some(words) = text(a, "text")? else {
            return Err("missing required args: text".into());
        };
        if words.trim().is_empty() {
            return Err("'text' is empty".into());
        }
        settings.text = words.to_string();
        if let Some(f) = text(a, "font")? {
            if !f.trim().is_empty() && buzz_text::family(f.trim()).is_none() {
                return Err(format!("no font family '{f}' is installed"));
            }
            settings.font = f.trim().to_string();
        }
        if let Some(n) = num(a, "size", 4.0, 1000.0)? {
            settings.size = n;
        }
        let colour = |key: &str| -> Result<Option<[u8; 3]>, String> {
            let Some(t) = text(a, key)? else { return Ok(None) };
            let c = parse_hex(t).ok_or(format!("'{key}' needs a colour like \"#rrggbb\""))?;
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            Ok(Some([r, g, b]))
        };
        if let Some(c) = colour("color")? {
            settings.color = c;
        }
        if let Some(p) = flag(a, "plate")? {
            settings.plate = p;
        }
        if let Some(c) = colour("plate_color")? {
            settings.plate_color = c;
        }
        if let Some(n) = num(a, "plate_opacity", 0.0, 1.0)? {
            settings.plate_opacity = n;
        }
        if let Some(p) = text(a, "preset")? {
            let preset = buzz_act::title::TitlePreset::from_id(p).ok_or(format!(
                "unknown preset '{p}'; valid: typewriter, pop_words, fade_up, slide_in, stamp"
            ))?;
            settings.preset = buzz_ui::TITLE_PRESETS
                .iter()
                .find(|(id, _)| *id == preset.id())
                .map(|(id, _)| *id)
                .unwrap_or("fade_up");
        }
        if let Some(n) = num(a, "hold", 0.0, 60.0)? {
            settings.hold_seconds = n;
        }
        if let Some(p) = text(a, "position")? {
            let position = buzz_act::title::TitlePosition::from_id(p)
                .ok_or(format!("unknown position '{p}'; valid: centre, lower_third, top"))?;
            settings.position = buzz_ui::TITLE_POSITIONS
                .iter()
                .find(|(id, _)| *id == position.id())
                .map(|(id, _)| *id)
                .unwrap_or("centre");
        }
        start = int(a, "start_frame", 0, 1_000_000)?;
        Ok(())
    })();
    if let Err(e) = parsed {
        return AgentResult::err(e);
    }
    let editor = app.agent_editor_mut();
    let before = editor.doc.history().undo_depth();
    let playhead = editor.current_frame;
    if let Some(f) = start {
        editor.current_frame = f;
    }
    editor.status = None;
    let made = editor.add_title_card(&settings);
    editor.current_frame = playhead;
    editor.title_card.settings = settings;
    match made {
        Ok(card) => {
            let name = app
                .agent_editor()
                .doc
                .scene()
                .stage_layers()
                .iter()
                .find(|l| l.id == card.layer)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            finish(
                app,
                "add_title_card",
                before,
                json!({
                    "layer": name,
                    "layer_id": card.layer.0,
                    "symbol": card.symbol.0,
                    "start": card.start,
                    "frames": card.frames,
                    "units": card.units,
                }),
            )
        }
        Err(e) => AgentResult::err(e),
    }
}

/// Identify & Rename dialog -> `buzz_ai::naming::apply_renames_to_scene`, the
/// call its Apply button makes; or the VLM job it starts.
fn identify_assets(app: &mut App, a: &Value) -> AgentResult {
    use buzz_ai::naming::{AssetCategory, AssetView, RenameProposal};
    if let Err(e) = check_keys(a, &["renames", "start", "apply_proposals"]) {
        return AgentResult::err(e);
    }
    let start = flag(a, "start").map(|v| v.unwrap_or(false));
    let apply = flag(a, "apply_proposals").map(|v| v.unwrap_or(false));
    let (start, apply) = match (start, apply) {
        (Ok(s), Ok(p)) => (s, p),
        (Err(e), _) | (_, Err(e)) => return AgentResult::err(e),
    };
    let proposals: Vec<RenameProposal> = if let Some(list) = given(a, "renames") {
        let Some(list) = list.as_array().filter(|l| !l.is_empty()) else {
            return AgentResult::err("'renames' needs a non-empty list of {symbol, name, tags?}");
        };
        let scene = app.agent_editor().doc.scene();
        let mut out = Vec::new();
        for (i, item) in list.iter().enumerate() {
            let Some(symbol) = item.get("symbol") else {
                return AgentResult::err(format!("renames[{i}] needs a 'symbol' (name or id)"));
            };
            let Some(new_name) = item.get("name").and_then(|v| v.as_str()).map(str::trim).filter(|n| !n.is_empty()) else {
                return AgentResult::err(format!("renames[{i}] needs a non-empty 'name'"));
            };
            let id = match symbol_by_ref(scene, symbol, "renamed") {
                Ok(id) => id,
                Err(e) => return AgentResult::err(e),
            };
            let old_name = scene.library().get(id).map(|s| s.name.clone()).unwrap_or_default();
            let tags = item
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|t| t.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            out.push(RenameProposal {
                id: id.0.to_string(),
                old_name,
                new_name: new_name.to_string(),
                category: AssetCategory::Other,
                view: AssetView::Na,
                tags,
                selected: true,
            });
        }
        out
    } else if apply {
        let found = app.agent_editor().ai_rename.proposals.clone();
        if found.is_empty() {
            return AgentResult::err(
                "no proposals to apply yet; start the job with start:true and wait for it to finish (status line)",
            );
        }
        found
    } else if start {
        let editor = app.agent_editor_mut();
        editor.status = None;
        editor.open_ai_rename();
        return AgentResult::ok(json!({
            "id": "ai_identify_assets",
            "started": true,
            "note": "a vision-model job; when it finishes, call again with apply_proposals:true (or review in the AI panel)",
            "status": editor.status,
        }));
    } else {
        return missing(&["renames | start:true | apply_proposals:true"]);
    };

    let count = proposals.iter().filter(|p| p.selected).count();
    let before = depth(app);
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    editor.doc.edit("AI: Identify and Rename Assets", |scene| {
        buzz_ai::naming::apply_renames_to_scene(scene, &proposals);
    });
    editor.doc.end_gesture();
    editor.status = Some(format!("Applied AI renames to {count} assets"));
    finish(app, "ai_identify_assets", before, json!({"renamed": count}))
}

/// Auto-Rig Vector Character -> `App::ai_rig_vector_character_async`.
fn rig_vector_character(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["symbol", "element"]) {
        return AgentResult::err(e);
    }
    let Some(symbol) = given(a, "symbol") else {
        return missing(&["symbol"]);
    };
    let scene = app.agent_editor().doc.scene().clone();
    let id = match symbol_by_ref(&scene, symbol, "character") {
        Ok(id) => id,
        Err(e) => return AgentResult::err(e),
    };
    if let Err(e) = select_element(app, a) {
        return AgentResult::err(e);
    }
    let editor = app.agent_editor_mut();
    if given(a, "element").is_none() {
        // The symbol chosen in the Library, with nothing on stage overriding it.
        editor.selection.clear();
    }
    editor.library.selected = Some(id);
    editor.status = None;
    app.agent_forget_last_task();
    app.ai_rig_vector_character_async();
    if let Some(why) = app.agent_editor().status.clone() {
        return AgentResult::err(why);
    }
    let Some(job) = app.agent_track_last_job("ai_rig_vector_character") else {
        return AgentResult::err("the rigging job did not start");
    };
    AgentResult::ok(super::jobs::started(
        "ai_rig_vector_character",
        job,
        "skeleton detection runs off the UI thread; the armature replaces the instance (or lands on a new layer) as one undo step when done",
    ))
}

/// Rig Character as Layers -> `App::ai_rig_layers_async`; the finished job
/// applies the plan as the review dialog's Apply does (`App::commit_layer_rig`)
/// or, with `apply:false`, returns it.
fn rig_layers(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["symbol", "element", "frame", "overrides", "apply", "profile", "split_merged"]) {
        return AgentResult::err(e);
    }
    if given(a, "symbol").is_none() && given(a, "element").is_none() {
        return missing(&["symbol | element"]);
    }
    let frame = match int(a, "frame", 0, 1_000_000) {
        Ok(f) => f.unwrap_or(0),
        Err(e) => return AgentResult::err(e),
    };
    let apply = match flag(a, "apply") {
        Ok(f) => f.unwrap_or(true),
        Err(e) => return AgentResult::err(e),
    };
    let profile = match text(a, "profile") {
        Ok(profile) => profile.map(str::to_string),
        Err(e) => return AgentResult::err(e),
    };
    // R4: cut merged drawings along the skeleton; unset = when there are any.
    let split_merged = match flag(a, "split_merged") {
        Ok(f) => f,
        Err(e) => return AgentResult::err(e),
    };
    if let Err(e) = crate::rigging::naming_profile(profile.as_deref()) {
        return AgentResult::err(e);
    }
    let scene = app.agent_editor().doc.scene().clone();
    let id = match given(a, "element") {
        Some(v) => {
            let Some(element) = v.as_u64().map(ObjectId) else {
                return AgentResult::err("'element' needs an element id");
            };
            match scene.find_object(element).and_then(|(_, o)| o.instance().map(|i| i.symbol)) {
                Some(id) => id,
                None => return AgentResult::err(format!("element {} is not a symbol instance on stage", element.0)),
            }
        }
        None => match symbol_by_ref(&scene, &a["symbol"], "character") {
            Ok(id) => id,
            Err(e) => return AgentResult::err(e),
        },
    };
    let layer_names: Vec<String> = scene
        .library()
        .get(id)
        .map(|s| s.layers.iter().map(|l| l.name.clone()).collect())
        .unwrap_or_default();
    let mut overrides = Vec::new();
    if let Some(list) = given(a, "overrides") {
        let Some(list) = list.as_array() else {
            return AgentResult::err("'overrides' needs a list of {layer, slot?, name?}");
        };
        for o in list {
            if let Err(e) = check_keys(o, &["layer", "slot", "name"]) {
                return AgentResult::err(format!("overrides: {e}"));
            }
            let layer = match text(o, "layer") {
                Ok(Some(l)) => l.to_string(),
                Ok(None) => return missing(&["overrides[].layer"]),
                Err(e) => return AgentResult::err(format!("overrides: {e}")),
            };
            if !layer_names.contains(&layer) {
                return AgentResult::err(format!("overrides: no layer '{layer}'; layers: {}", layer_names.join(", ")));
            }
            let slot = match text(o, "slot") {
                Ok(slot) => slot.map(str::to_string),
                Err(e) => return AgentResult::err(format!("overrides: {e}")),
            };
            if let Some(slot) = &slot
                && let Err(e) = crate::app::layer_rig_slot_by_name(slot)
            {
                return AgentResult::err(format!("overrides: {e}"));
            }
            let name = match text(o, "name") {
                Ok(name) => name.map(str::to_string),
                Err(e) => return AgentResult::err(format!("overrides: {e}")),
            };
            overrides.push(crate::app::LayerRigOverride { layer, slot, name });
        }
    }
    if let Err(e) = select_element(app, a) {
        return AgentResult::err(e);
    }
    let editor = app.agent_editor_mut();
    if given(a, "element").is_none() {
        // The symbol chosen in the Library, with nothing on stage overriding it.
        editor.selection.clear();
        editor.library.selected = Some(id);
    }
    editor.status = None;
    app.agent_forget_last_task();
    app.ai_rig_layers_async(frame, Some(crate::app::LayerRigRequest { overrides, apply, profile, split_merged }));
    let Some(job) = app.agent_track_last_job("ai_rig_layers") else {
        let why = app.agent_editor().status.clone().unwrap_or_else(|| "the planning job did not start".into());
        return AgentResult::err(why);
    };
    AgentResult::ok(super::jobs::started(
        "ai_rig_layers",
        job,
        if apply {
            "body-part detection runs off the UI thread; the layer rig is applied as one undo step when done"
        } else {
            "body-part detection runs off the UI thread; the plan comes back without changing anything"
        },
    ))
}

/// Direct Story with LLM -> `App::ai_direct_story_from`.
fn direct_story(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["brief"]) {
        return AgentResult::err(e);
    }
    let brief = match text(a, "brief") {
        Ok(Some(b)) if !b.trim().is_empty() => b.to_string(),
        Ok(_) => return missing(&["brief"]),
        Err(e) => return AgentResult::err(e),
    };
    app.agent_editor_mut().status = None;
    app.agent_forget_last_task();
    app.ai_direct_story_from(brief);
    if let Some(why) = app.agent_editor().status.clone() {
        return AgentResult::err(why);
    }
    let Some(job) = app.agent_track_last_job("ai_direct_story") else {
        return AgentResult::err("the story job did not start");
    };
    AgentResult::ok(super::jobs::started(
        "ai_direct_story",
        job,
        "an LLM parses the brief off the UI thread; the script is applied as one undo step when it returns",
    ))
}

/// Character from Image -> `App::agent_character_async`: the wizard with its
/// default choices, as a job. The wizard's own name box is `character_name`
/// here because `name` is already the command's id when the arguments are flat.
fn character_from_image(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["path", "character_name", "rig"]) {
        return AgentResult::err(e);
    }
    let path = match text(a, "path") {
        Ok(Some(p)) if !p.trim().is_empty() => p.trim().to_string(),
        Ok(_) => return missing(&["path"]),
        Err(e) => return AgentResult::err(e),
    };
    // LR2: a layer rig unless the call asks for bones.
    let kind = match text(a, "rig") {
        Ok(None) => buzz_rig::RigKind::Layers,
        Ok(Some(r)) => match buzz_rig::RigKind::parse(r) {
            Some(kind) => kind,
            None => return AgentResult::err(format!("'rig' must be \"layers\" or \"bones\", not '{r}'")),
        },
        Err(e) => return AgentResult::err(e),
    };
    let from_args = |key: &str| text(a, key).ok().flatten().map(str::trim).filter(|n| !n.is_empty());
    let name = from_args("character_name")
        // `{name:"ai_character_from_image", path}` puts the command's id here.
        .or_else(|| from_args("name").filter(|n| *n != "ai_character_from_image"))
        .map(str::to_string)
        .or_else(|| {
            std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).map(str::to_string)
        })
        .unwrap_or_else(|| "Character".to_string());
    app.agent_editor_mut().status = None;
    match app.agent_character_async(std::path::PathBuf::from(path), name, kind) {
        Ok(started) => AgentResult::ok(started),
        Err(e) => AgentResult::err(e),
    }
}

/// AI Mouth Chart -> `buzz_ai::face::mouth_chart::replace_mouth_frames`.
///
/// Reads the chart (a few seconds: the mouths are traced), finds the
/// character's mouth symbol, and replaces its frames inside one undo step. The
/// symbol keeps its id, so every instance of it and every lip-sync keyframe
/// that already shows one of its frames carries on working.
fn mouth_chart(app: &mut App, a: &Value) -> AgentResult {
    use buzz_ai::face::mouth_chart as chart;
    if let Err(e) = check_keys(a, &["element", "character", "path", "labels"]) {
        return AgentResult::err(e);
    }
    let path = match text(a, "path") {
        Ok(Some(p)) if !p.trim().is_empty() => p.trim().to_string(),
        Ok(_) => return missing(&["path"]),
        Err(e) => return AgentResult::err(e),
    };
    let element = match given(a, "element") {
        Some(v) => match v.as_u64() {
            Some(id) => Some(ObjectId(id)),
            None => return AgentResult::err("'element' needs an element id"),
        },
        None => None,
    };
    let character = match text(a, "character") {
        Ok(c) => c.map(str::trim).filter(|c| !c.is_empty()),
        Err(e) => return AgentResult::err(e),
    };
    if element.is_none() && character.is_none() {
        return missing(&["element | character"]);
    }
    if element.is_some() && character.is_some() {
        return AgentResult::err("give 'element' or 'character', not both");
    }
    let labels: Vec<String> = match given(a, "labels") {
        None => chart::RAJU_LABELS.iter().map(|l| l.to_string()).collect(),
        Some(v) => {
            let list: Option<Vec<String>> = v
                .as_array()
                .filter(|l| !l.is_empty())
                .and_then(|l| l.iter().map(|x| x.as_str().map(|s| s.trim().to_string())).collect());
            match list {
                Some(l) if l.iter().all(|s| !s.is_empty()) => l,
                _ => return AgentResult::err("'labels' needs a non-empty list of text, in the chart's reading order"),
            }
        }
    };

    // Find the symbol before the (slower) trace, so a wrong name fails at once.
    let symbol = match mouth_symbol_for(app.agent_editor().doc.scene(), element, character) {
        Ok(id) => id,
        Err(e) => return AgentResult::err(e),
    };
    let symbol_name = app
        .agent_editor()
        .doc
        .scene()
        .library()
        .get(symbol)
        .map(|s| s.name.clone())
        .unwrap_or_default();

    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let parsed = match chart::read_chart_file(std::path::Path::new(&path), &label_refs) {
        Ok(c) => c,
        Err(e) => return AgentResult::err(format!("mouth chart: {e}")),
    };

    let before = depth(app);
    let mut applied = None;
    let editor = app.agent_editor_mut();
    editor.status = None;
    editor.doc.end_gesture();
    editor.doc.edit("AI: Mouth Chart", |scene| {
        applied = Some(chart::replace_mouth_frames(scene, symbol, &parsed));
    });
    editor.doc.end_gesture();
    let applied = match applied {
        Some(Ok(done)) => done,
        Some(Err(e)) => return AgentResult::err(format!("mouth chart: {e}")),
        None => return AgentResult::err("mouth chart: nothing was applied"),
    };
    editor.status = Some(format!("Mouth chart: {} frames in '{symbol_name}'", applied.frames.len()));

    let frames: Vec<Value> = applied
        .frames
        .iter()
        .map(|(frame, name)| json!({"frame": frame, "name": name}))
        .collect();
    finish(
        app,
        "ai_mouth_chart",
        before,
        json!({
            "symbol": symbol.0,
            "symbol_name": symbol_name,
            "frames": frames,
            "frame_sources": chart::describe_plan(&parsed),
            "rest_width": applied.rest_width,
            "line_width": applied.line_width,
            "layers_replaced": applied.layers_replaced,
            "notes": parsed.notes,
        }),
    )
}

/// The mouth symbol of a character: the one drawn on the rig `element`, or the
/// library symbol named for `character` (`Raju` finds `Raju_mouth`).
fn mouth_symbol_for(scene: &Scene, element: Option<ObjectId>, character: Option<&str>) -> Result<SymbolId, String> {
    let library = scene.library();
    let is_mouth = |name: &str| name.to_lowercase().contains("mouth");
    let mut found: Vec<SymbolId> = Vec::new();
    let what;
    if let Some(id) = element {
        let Some((_, object)) = scene.find_object(id) else {
            return Err(format!("no element {} in this scene", id.0));
        };
        collect_mouth_symbols(scene, object, 0, &mut found);
        what = format!("element {}", id.0);
    } else {
        let who = character.unwrap_or_default();
        let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>();
        let want = key(who);
        let exact = format!("{want}mouth");
        let named: Vec<SymbolId> = library
            .iter()
            .filter(|s| key(&s.name) == exact)
            .map(|s| s.id)
            .collect();
        found = if named.is_empty() {
            library
                .iter()
                .filter(|s| is_mouth(&s.name) && key(&s.name).starts_with(&want))
                .map(|s| s.id)
                .collect()
        } else {
            named
        };
        what = format!("character '{who}'");
    }
    found.sort_by_key(|id| id.0);
    found.dedup();
    let describe = |ids: &[SymbolId]| {
        ids.iter()
            .map(|id| format!("{}={}", id.0, library.get(*id).map(|s| s.name.as_str()).unwrap_or("?")))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match found.as_slice() {
        [one] => Ok(*one),
        [] => {
            let mouths: Vec<SymbolId> = library.iter().filter(|s| is_mouth(&s.name)).map(|s| s.id).collect();
            Err(format!(
                "no mouth symbol found for {what}; mouth symbols in the library: {}",
                if mouths.is_empty() { "none".to_string() } else { describe(&mouths) }
            ))
        }
        many => Err(format!(
            "{what} has several mouth symbols ({}); pass the rig as 'element' or name the one you want",
            describe(many)
        )),
    }
}

/// Every mouth symbol drawn by `object`: through a rig's parts, a group's
/// children and (a few levels of) nested symbols.
fn collect_mouth_symbols(scene: &Scene, object: &buzz_scene::Object, level: usize, out: &mut Vec<SymbolId>) {
    if level > 4 {
        return;
    }
    match &object.kind {
        ObjectKind::Armature(rig) => {
            for part in &rig.parts {
                collect_mouth_symbols(scene, &part.artwork, level + 1, out);
            }
        }
        ObjectKind::Group(children) => {
            for child in children {
                collect_mouth_symbols(scene, child, level + 1, out);
            }
        }
        ObjectKind::Instance(instance) => {
            let Some(symbol) = scene.library().get(instance.symbol) else { return };
            if symbol.name.to_lowercase().contains("mouth") {
                out.push(symbol.id);
            } else {
                for layer in symbol.layers.iter() {
                    for inner in layer.all_objects() {
                        collect_mouth_symbols(scene, inner, level + 1, out);
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod mouth_chart_tests {
    use super::*;
    use buzz_render::GpuPreference;
    use buzz_scene::{Object, ObjectKind};
    use std::sync::Arc;

    /// A scene with `Hero_mouth` (ten placeholder frames), a rig that draws it
    /// (rigged like the character pipeline does: an instance bound to a bone)
    /// and a second character's mouth.
    fn cast_scene() -> (Scene, SymbolId, ObjectId) {
        let mut scene = Scene::default();
        let hero = buzz_act::lipsync::placeholder_mouth(&mut scene, "Hero_mouth");
        buzz_act::lipsync::placeholder_mouth(&mut scene, "Villain_mouth");
        let mut armature = buzz_rig::Armature::new(buzz_geom::Point::ZERO);
        armature.push(buzz_rig::Bone::new("Head", None, 20.0, 0.0));
        let mut rig = buzz_scene::ArmatureData::new(armature);
        rig.bind_rigid(Arc::new(Object::instance_of(scene.next_object_id(), hero)), 0);
        let rig_id = scene.next_object_id();
        let mut object = Object::group(rig_id, Vec::new());
        object.kind = ObjectKind::Armature(rig);
        let layer = scene.add_layer("Hero", buzz_scene::LayerKind::Normal);
        scene.add_object(layer, object);
        (scene, hero, rig_id)
    }

    #[test]
    fn a_rig_finds_its_mouth_and_a_name_finds_the_named_one() {
        let (scene, hero, rig) = cast_scene();
        assert_eq!(mouth_symbol_for(&scene, Some(rig), None), Ok(hero));
        assert_eq!(mouth_symbol_for(&scene, None, Some("Hero")), Ok(hero));
        assert_eq!(mouth_symbol_for(&scene, None, Some("hero")), Ok(hero), "names match without case");
        let err = mouth_symbol_for(&scene, None, Some("Nobody")).unwrap_err();
        assert!(err.contains("Hero_mouth") && err.contains("Villain_mouth"), "{err}");
        let err = mouth_symbol_for(&scene, Some(ObjectId(99_999)), None).unwrap_err();
        assert!(err.contains("no element"), "{err}");
    }

    /// A 2x2 chart: coloured mouths with a grey label bar under each.
    fn write_chart(path: &std::path::Path) {
        let (w, h) = (420u32, 320u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[252, 250, 243, 255]);
        }
        let mut put = |x: i32, y: i32, c: [u8; 3]| {
            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                px[o..o + 3].copy_from_slice(&c);
            }
        };
        // (cx, cy, rx, ry, open)
        let mouths = [
            (105.0, 70.0, 70.0, 12.0, false),
            (315.0, 75.0, 60.0, 40.0, true),
            (105.0, 220.0, 62.0, 30.0, true),
            (315.0, 220.0, 70.0, 11.0, false),
        ];
        for (cx, cy, rx, ry, open) in mouths {
            for y in (cy - ry - 3.0) as i32..=(cy + ry + 3.0) as i32 {
                for x in (cx - rx - 3.0) as i32..=(cx + rx + 3.0) as i32 {
                    let d = ((x as f64 - cx) / rx).powi(2) + ((y as f64 - cy) / ry).powi(2);
                    if d > 1.0 {
                        continue;
                    }
                    let edge = 1.0 - (3.5 / rx.min(ry)).min(0.5);
                    let c = if d > edge * edge {
                        [78, 38, 28]
                    } else if open && ((x as f64 - cx) / (rx * 0.7)).powi(2) + ((y as f64 - cy) / (ry * 0.6)).powi(2) < 1.0 {
                        [92, 24, 24]
                    } else {
                        [222, 148, 108]
                    };
                    put(x, y, c);
                }
            }
            // The label: three dark boxes under the mouth.
            for letter in 0..3 {
                for y in 0..14 {
                    for x in 0..9 {
                        put(cx as i32 - 22 + letter * 15 + x, (cy + ry + 30.0) as i32 + y, [28, 28, 30]);
                    }
                }
            }
        }
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&px).unwrap();
    }

    fn frames_of(app: &App, symbol: SymbolId) -> u32 {
        let scene = app.agent_editor().doc.scene();
        scene.library().get(symbol).unwrap().layers.iter().next().unwrap().frames.length()
    }

    #[test]
    fn ai_mouth_chart_replaces_the_mouth_frames_as_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let chart = dir.path().join("chart.png");
        write_chart(&chart);
        let path = chart.to_string_lossy().to_string();

        let mut app = App::new(GpuPreference::Automatic);
        let (scene, hero, rig) = cast_scene();
        app.agent_editor_mut().doc.edit("setup", |s| *s = scene);
        app.agent_editor_mut().doc.end_gesture();
        let before = depth(&app);
        assert_eq!(frames_of(&app, hero), 10);
        let labels = json!(["Neutral Closed", "A", "E", "Frown"]);

        // What is missing or wrong is refused in words, and nothing changes.
        let r = run(&mut app, Command::AiMouthChart, &json!({"character": "Hero"}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("missing required args: path")), "{r:?}");
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": path}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("element | character")), "{r:?}");
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": path, "character": "Hero", "element": rig.0}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("not both")), "{r:?}");
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": path, "character": "Hero", "bogus": 1}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("unknown arg 'bogus'")), "{r:?}");
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": path, "character": "Nobody", "labels": labels}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("no mouth symbol")), "{r:?}");
        // The default (Raju's 19) labels do not fit a four-mouth chart.
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": path, "character": "Hero"}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("found 4 mouths") && e.contains("19 labels")), "{r:?}");
        let r = run(&mut app, Command::AiMouthChart, &json!({"path": "Z:/nowhere/chart.png", "character": "Hero", "labels": labels}));
        assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("cannot read")), "{r:?}");
        assert_eq!(depth(&app), before, "refusals change nothing");
        assert_eq!(frames_of(&app, hero), 10);

        // By character name, then by the rig's element id.
        for target in [json!({"character": "Hero"}), json!({"element": rig.0})] {
            let mut args = json!({"path": path, "labels": labels});
            args.as_object_mut().unwrap().extend(target.as_object().unwrap().clone());
            let r = run(&mut app, Command::AiMouthChart, &args);
            assert!(r.ok, "{:?}", r.error);
            assert_eq!(r.value["undo_steps_added"], json!(1), "{}", r.value);
            assert_eq!(r.value["symbol_name"], json!("Hero_mouth"));
            let frames = r.value["frames"].as_array().unwrap();
            // Four mouths: ten viseme frames (the chart has no L, F/V...) and the one left over.
            assert_eq!(frames.len(), 11, "{frames:?}");
            assert_eq!(frames[0], json!({"frame": 0, "name": "Rest"}));
            assert_eq!(frames[10], json!({"frame": 10, "name": "Frown"}));
            assert_eq!(frames_of(&app, hero), 11);
        }

        // One undo brings the placeholder mouths back.
        let before_second = depth(&app);
        assert_eq!(before_second, before + 2);
        app.agent_editor_mut().doc.undo();
        app.agent_editor_mut().doc.undo();
        assert_eq!(frames_of(&app, hero), 10);
    }
}

// -- the storyboard (`docs/STORYBOARD_PLAN.md` S3) ----------------------------------

/// How long a storyboard call waits for the panel's worker.
const STORYBOARD_WAIT: std::time::Duration = std::time::Duration::from_secs(1800);

fn storyboard_panels(app: &mut App) -> Value {
    let sb = app.storyboard_mut();
    let Some(board) = &sb.board else { return json!([]) };
    let rows: Vec<Value> = board
        .panels
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (look_line, framing) = sb.picture_note(&p.id).unwrap_or_default();
            json!({"index": i, "id": p.id, "scene": p.scene, "shot": p.shot, "kind": p.kind, "on": p.on,
                   "duration_s": p.duration_s, "look_line": look_line, "framing": framing})
        })
        .collect();
    Value::Array(rows)
}

/// Load `args.board` into the panel when it names another board.
fn storyboard_board_arg(app: &mut App, a: &Value) -> Result<(), String> {
    let Some(path) = text(a, "board")? else { return Ok(()) };
    let path = std::path::PathBuf::from(path);
    let sb = app.storyboard_mut();
    if sb.board_path.as_deref() != Some(path.as_path()) {
        sb.load(Some(&path))?;
        sb.wait(STORYBOARD_WAIT)?;
    }
    Ok(())
}

/// Storyboard panel's Make -> `StoryboardPanel::make`, waited for.
fn storyboard_make(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["episode", "scenes", "out", "look", "breakdown", "offline"]) {
        return AgentResult::err(e);
    }
    let parsed = (|| -> Result<(String, String, String, String, crate::storyboard::LookChoice, bool), String> {
        let episode = text(a, "episode")?.ok_or("missing required args: episode")?.to_string();
        let scenes = match given(a, "scenes") {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|v| v.as_u64().map(|n| n.to_string()).ok_or("'scenes' needs scene numbers"))
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
            Some(_) => return Err("'scenes' needs text like \"4,5\" or a list of numbers".into()),
        };
        let look = match text(a, "look")? {
            None | Some("auto") => crate::storyboard::LookChoice::Auto,
            Some("a") | Some("A") => crate::storyboard::LookChoice::A,
            Some("b") | Some("B") => crate::storyboard::LookChoice::B,
            Some(other) => return Err(format!("'look' must be auto, a or b, not '{other}'")),
        };
        Ok((
            episode,
            scenes,
            text(a, "out")?.unwrap_or("").to_string(),
            text(a, "breakdown")?.unwrap_or("").to_string(),
            look,
            flag(a, "offline")?.unwrap_or(false),
        ))
    })();
    let (episode, scenes, out, breakdown, look, offline) = match parsed {
        Ok(p) => p,
        Err(e) => return AgentResult::err(e),
    };
    app.reveal_storyboard();
    let sb = app.storyboard_mut();
    if sb.busy() {
        let _ = sb.wait(STORYBOARD_WAIT);
    }
    sb.episode = episode;
    sb.scenes = scenes;
    sb.out = out;
    sb.breakdown = breakdown;
    sb.look = look;
    sb.offline = offline;
    if let Err(e) = sb.make() {
        return AgentResult::err(e);
    }
    if let Err(e) = sb.wait(STORYBOARD_WAIT) {
        return AgentResult::err(format!("storyboard_make: {e}"));
    }
    let board = sb.board_path.as_ref().map(|p| p.display().to_string());
    let status = sb.status.clone();
    let panels = storyboard_panels(app);
    AgentResult::ok(json!({"board": board, "status": status, "panels": panels}))
}

/// Storyboard panel's fields and buttons -> `StoryboardPanel::edit` and friends.
fn storyboard_edit(app: &mut App, a: &Value) -> AgentResult {
    const KEYS: [&str; 14] = [
        "board", "panel", "kind", "on", "action", "notes", "duration_s", "look", "dialogue", "move", "add_after", "split", "delete", "regenerate",
    ];
    if let Err(e) = check_keys(a, &KEYS) {
        return AgentResult::err(e);
    }
    if let Err(e) = storyboard_board_arg(app, a) {
        return AgentResult::err(e);
    }
    let Some(which) = given(a, "panel").cloned() else { return missing(&["panel"]) };
    let result = (|| -> Result<usize, String> {
        let sb = app.storyboard_mut();
        let board = sb.board.as_ref().ok_or("no board: storyboard_make first, or pass board")?;
        let index = match &which {
            Value::Number(n) => n.as_u64().map(|n| n as usize).filter(|n| *n < board.panels.len()).ok_or(format!("no panel {which}"))?,
            Value::String(id) => board.panels.iter().position(|p| p.id == *id).ok_or(format!("no panel '{id}'"))?,
            _ => return Err("'panel' needs a panel id or a 0-based index".into()),
        };
        let kind = text(a, "kind")?.map(str::to_string);
        if let Some(k) = &kind
            && !["wide", "medium", "close"].contains(&k.as_str())
        {
            return Err(format!("'kind' must be wide, medium or close, not '{k}'"));
        }
        let look = match text(a, "look")? {
            None => None,
            Some("auto") => Some(crate::storyboard::Look::Auto),
            Some("a") | Some("A") => Some(crate::storyboard::Look::Real),
            Some("b") | Some("B") => Some(crate::storyboard::Look::Sketch),
            Some(other) => return Err(format!("'look' must be auto, a or b, not '{other}'")),
        };
        let dialogue = match given(a, "dialogue") {
            None => None,
            Some(Value::Array(rows)) => Some(
                rows.iter()
                    .map(|r| match r.as_array().map(|x| x.as_slice()) {
                        Some([w, t]) => Ok((w.as_str().unwrap_or("").to_string(), t.as_str().unwrap_or("").to_string())),
                        _ => Err("'dialogue' needs [[speaker, text], …]".to_string()),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Some(_) => return Err("'dialogue' needs [[speaker, text], …]".into()),
        };
        let on = text(a, "on")?.map(str::to_string);
        let action = text(a, "action")?.map(str::to_string);
        let notes = text(a, "notes")?.map(str::to_string);
        let duration = num(a, "duration_s", 0.1, 600.0)?;
        let mv = given(a, "move").cloned();
        let (add_after, split, delete, regenerate) =
            (flag(a, "add_after")?.unwrap_or(false), flag(a, "split")?.unwrap_or(false), flag(a, "delete")?.unwrap_or(false), flag(a, "regenerate")?.unwrap_or(false));
        sb.edit(index, |p| {
            if let Some(k) = kind {
                p.kind = k;
            }
            if let Some(o) = on {
                p.on = o;
            }
            if let Some(t) = action {
                p.action = t;
            }
            if let Some(t) = notes {
                p.notes = t;
            }
            if let Some(d) = duration {
                p.duration_s = d;
            }
            if let Some(l) = look {
                p.look = l;
            }
            if let Some(d) = dialogue {
                p.dialogue = d;
            }
        })?;
        let mut index = index;
        if let Some(m) = mv {
            let to = match &m {
                Value::String(s) if s == "up" => index.saturating_sub(1),
                Value::String(s) if s == "down" => index + 1,
                Value::Number(n) => n.as_u64().ok_or("'move' needs up, down or a 0-based index")? as usize,
                _ => return Err("'move' needs up, down or a 0-based index".into()),
            };
            index = sb.move_panel(index, to)?;
        }
        if split {
            sb.split(index)?;
        }
        if add_after {
            index = sb.add_after(index)?;
        }
        if regenerate {
            sb.regenerate(index)?;
            sb.wait(STORYBOARD_WAIT)?;
        }
        if delete {
            sb.delete(index)?;
            return Ok(usize::MAX);
        }
        Ok(index)
    })();
    match result {
        Ok(index) => {
            let sb = app.storyboard_mut();
            let panel = sb.board.as_ref().and_then(|b| b.panels.get(index)).map(|p| {
                let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
                if let Some(m) = v.as_object_mut() {
                    m.remove("generated");
                }
                v
            });
            let panels = storyboard_panels(app);
            AgentResult::ok(json!({"index": if index == usize::MAX { Value::Null } else { json!(index) }, "panel": panel, "panels": panels}))
        }
        Err(e) => AgentResult::err(e),
    }
}

/// Storyboard panel's Export sheets -> `StoryboardPanel::export`, waited for.
fn storyboard_export(app: &mut App, a: &Value) -> AgentResult {
    if let Err(e) = check_keys(a, &["board"]) {
        return AgentResult::err(e);
    }
    if let Err(e) = storyboard_board_arg(app, a) {
        return AgentResult::err(e);
    }
    let sb = app.storyboard_mut();
    if sb.busy() {
        let _ = sb.wait(STORYBOARD_WAIT);
    }
    sb.last_export = None;
    if let Err(e) = sb.export() {
        return AgentResult::err(e);
    }
    if let Err(e) = sb.wait(STORYBOARD_WAIT) {
        return AgentResult::err(format!("storyboard_export: {e}"));
    }
    match &sb.last_export {
        Some(o) => AgentResult::ok(json!({
            "pages": o.pages.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "pdf": o.pdf.as_ref().map(|p| p.display().to_string()),
            "warnings": o.warnings,
        })),
        None => AgentResult::err("storyboard_export: no sheets were written"),
    }
}
