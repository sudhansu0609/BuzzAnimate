//! **Animate's keyboard shortcuts, brought over.**
//!
//! Animate keeps a keymap as XML — `Configuration/Keyboard Shortcuts/*.xml`,
//! one `<keycmd>` per binding: the command, the key, the modifiers. This reads
//! one and applies it to the [`Workspace`] keymap, so a person who has spent
//! years teaching their hands Animate's keys (and their own JSFL commands' keys)
//! gets the same keys here.
//!
//! # What maps to what
//!
//! * An Animate menu or tool command with a BuzzAnimate equivalent takes the
//!   key ([`TABLE`]).
//! * `Commands.<name>` — a JSFL command on the shelf — binds the key to running
//!   `<name>.jsfl` ([`Workspace::script_keys`]).
//! * Anything else is reported, not guessed at.
//!
//! # Mac and Windows
//!
//! Animate writes the platform's command key as `<cmdkey/>` — Ctrl on Windows,
//! ⌘ on a Mac — and egui's [`egui::Modifiers::COMMAND`] means exactly the same,
//! so a keymap made on Windows works on a Mac with ⌘ where it had Ctrl, as
//! Animate itself does when the same keymap is opened there.
//!
//! Two keys need help on a Mac laptop, which has no numeric keypad: `+` (a
//! keypad key on the Windows keyboard the keymap was made on) also answers to
//! the unshifted `=` beside Backspace, and `*` — a key egui has no name for —
//! is Shift+8, which is how it is typed there.
//!
//! # Clashes
//!
//! A key can do one thing. When the keymap gives a key to one of the person's
//! commands, any BuzzAnimate default on the same key is switched off, and when
//! the keymap itself gives one key to two commands the first keeps it and the
//! second is listed in the report to be given another.

use std::collections::{BTreeMap, BTreeSet};

use crate::command::Command;
use crate::tools::ToolId;
use crate::workspace::{KeyChord, Workspace, command_id};

/// What an Animate binding drives here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Command(String),
    /// A JSFL command on the shelf, by file stem.
    Script(String),
}

/// One `<keycmd>` as written.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimateBinding {
    /// `File.Save`, `Commands.eyeLeft`, …
    pub command: String,
    /// The `<uchar>`, or `None` when the key is a code.
    pub character: Option<String>,
    /// The `<key>` code, for keys that are not characters (F5, Enter, arrows).
    pub code: Option<u32>,
    pub cmdkey: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Animate's key-repeat duplicate of another binding; ignored.
    pub hold: bool,
}

/// Read an Animate keyboard-shortcut file.
pub fn parse(xml: &str) -> Vec<AnimateBinding> {
    let tag = |block: &str, name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let a = block.find(&open)? + open.len();
        let b = block[a..].find(&close)? + a;
        Some(unescape(block[a..b].trim()))
    };
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<keycmd>") {
        let Some(end) = rest[start..].find("</keycmd>") else { break };
        let block = &rest[start + "<keycmd>".len()..start + end];
        rest = &rest[start + end + "</keycmd>".len()..];
        let Some(command) = tag(block, "cmd") else { continue };
        let modifiers = tag(block, "mod").unwrap_or_default();
        out.push(AnimateBinding {
            command,
            character: tag(block, "uchar").filter(|c| !c.is_empty()),
            code: tag(block, "key").and_then(|k| k.parse().ok()),
            cmdkey: modifiers.contains("<cmdkey"),
            ctrl: modifiers.contains("<ctrl"),
            alt: modifiers.contains("<alt"),
            shift: modifiers.contains("<shift"),
            hold: block.contains("<hold"),
        });
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The keys one binding stands for, as chords this program can match: usually
/// one, two for `+` (keypad and the `=` key).
pub fn chords(b: &AnimateBinding) -> Vec<KeyChord> {
    let chord = |key: egui::Key, shift: bool| KeyChord {
        command: b.cmdkey,
        ctrl: b.ctrl,
        alt: b.alt,
        shift,
        key: key.name().to_string(),
    };
    if let Some(code) = b.code {
        return key_for_code(code).map(|k| vec![chord(k, b.shift)]).unwrap_or_default();
    }
    let Some(c) = b.character.as_deref() else { return Vec::new() };
    match c {
        // No egui key: typed as Shift+8 on a keyboard without a keypad.
        "*" => vec![chord(egui::Key::Num8, true)],
        // The keypad's `+`, and the unshifted key it shares on a laptop.
        "+" if !b.shift => vec![chord(egui::Key::Plus, false), chord(egui::Key::Equals, false)],
        _ => key_for_char(c).map(|k| vec![chord(k, b.shift)]).unwrap_or_default(),
    }
}

fn key_for_char(c: &str) -> Option<egui::Key> {
    use egui::Key::*;
    let mut chars = c.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match ch.to_ascii_uppercase() {
        'A' => A, 'B' => B, 'C' => C, 'D' => D, 'E' => E, 'F' => F, 'G' => G, 'H' => H,
        'I' => I, 'J' => J, 'K' => K, 'L' => L, 'M' => M, 'N' => N, 'O' => O, 'P' => P,
        'Q' => Q, 'R' => R, 'S' => S, 'T' => T, 'U' => U, 'V' => V, 'W' => W, 'X' => X,
        'Y' => Y, 'Z' => Z,
        '0' => Num0, '1' => Num1, '2' => Num2, '3' => Num3, '4' => Num4,
        '5' => Num5, '6' => Num6, '7' => Num7, '8' => Num8, '9' => Num9,
        '=' => Equals, '-' => Minus, '+' => Plus, '/' => Slash, '\\' => Backslash,
        ',' => Comma, '.' => Period, ';' => Semicolon, '\'' => Quote, '`' => Backtick,
        '[' => OpenBracket, ']' => CloseBracket,
        _ => return None,
    })
}

/// Animate's `<key>` codes, read off its own default keymap (F5 is Insert
/// Frame, Ctrl+Enter Test Movie, the arrows nudge, …).
fn key_for_code(code: u32) -> Option<egui::Key> {
    use egui::Key::*;
    Some(match code {
        1 => Space,
        2 => Backspace,
        3 => Tab,
        4 | 5 => Enter,
        6 => Escape,
        7 => F1, 8 => F2, 9 => F3, 10 => F4, 11 => F5, 12 => F6,
        13 => F7, 14 => F8, 15 => F9, 16 => F10, 17 => F11, 18 => F12,
        35 => Delete,
        36 => Home,
        37 => End,
        38 => PageUp,
        39 => PageDown,
        42 => ArrowLeft,
        43 => ArrowRight,
        44 => ArrowUp,
        45 => ArrowDown,
        _ => return None,
    })
}

/// Animate command → this program's. Tools are spelled as their commands.
pub const TABLE: &[(&str, &str)] = &[
    ("File.New", "New"),
    ("File.Open", "Open"),
    ("File.Close", "Close"),
    ("File.Save", "Save"),
    ("File.SaveAs", "SaveAs"),
    ("File.Quit", "Quit"),
    ("Import.ImportToStage", "ImportToStage"),
    ("Import.ImportToLibrary", "ImportToLibrary"),
    ("Edit.Undo", "Undo"),
    ("Edit.Redo", "Redo"),
    ("Edit.Cut", "Cut"),
    ("Edit.Copy", "Copy"),
    ("Edit.Paste", "Paste"),
    ("Edit.Duplicate", "DuplicateSelection"),
    ("Edit.SelectAll", "SelectAll"),
    ("Edit.DeselectAll", "Deselect"),
    ("Edit.EditSymbols", "EditSymbol"),
    ("Edit.Clear", "Delete"),
    ("View.ZoomIn", "ZoomIn"),
    ("View.ZoomOut", "ZoomOut"),
    ("View.Rulers", "ToggleRulers"),
    ("Magnification.Percent100", "ZoomActual"),
    ("Magnification.ShowFrame", "ZoomShowFrame"),
    ("Magnification.ShowAll", "ZoomShowAll"),
    ("Grid.ShowGrid", "ToggleGrid"),
    ("Guides.ShowGuides", "ToggleGuides"),
    ("Snapping.SnapToObjects", "ToggleSnapping"),
    ("Modify.Group", "GroupSelection"),
    ("Modify.Ungroup", "UngroupSelection"),
    ("Modify.BreakApart", "BreakApart"),
    ("Modify.ConvertToSymbol", "ConvertToSymbol"),
    ("Insert.NewSymbol", "NewSymbol"),
    ("Insert.ClassicTween", "CreateClassicTween"),
    ("Timeline.InsertFrame", "InsertFrame"),
    ("Timeline.RemoveFrames", "RemoveFrame"),
    ("Timeline.ConvertToKeyframes", "InsertKeyframe"),
    ("Timeline.InsertKeyframe", "InsertKeyframe"),
    ("Timeline.ConvertToBlankKeyframes", "InsertBlankKeyframe"),
    ("Timeline.ClearKeyframe", "ClearKeyframe"),
    ("Timeline.ClearFrames", "ClearFrames"),
    ("Timeline.CutFrames", "CutFrames"),
    ("Timeline.CopyFrames", "CopyFrames"),
    ("Timeline.PasteFrames", "PasteFrames"),
    ("Timeline.OnionSkin", "ToggleOnionSkin"),
    ("Timeline.AutoKeyframing", "ToggleAutoKeyframe"),
    ("Timeline.EditMultipleFrames", "ToggleEditMultipleFrames"),
    ("Control.Play", "PlayPause"),
    ("Control.StepForwardOneFrame", "NextFrame"),
    ("Control.StepBackwardOneFrame", "PreviousFrame"),
    ("Control.Rewind", "FirstFrame"),
    ("Control.GoToEnd", "LastFrame"),
    ("Goto.First", "FirstFrame"),
    ("Goto.Last", "LastFrame"),
    ("TestMovie.Test", "TestMovie"),
    ("Transform.FlipHorizontal", "FlipHorizontal"),
    ("Transform.Rotate90CW", "RotateClockwise"),
    ("Transform.Rotate90CCW", "RotateAnticlockwise"),
    ("Symbol.DuplicateSymbol", "DuplicateSymbol"),
    ("Arrange.BringToFront", "BringToFront"),
    ("Arrange.BringForward", "BringForward"),
    ("Arrange.SendBackward", "SendBackward"),
    ("Arrange.SendToBack", "SendToBack"),
    ("Window.Actions", "ToggleActionsPanel"),
    ("Tool.Selection", "SelectTool(Selection)"),
    ("Tool.SubSelection", "SelectTool(Subselection)"),
    ("Tool.Lasso", "SelectTool(Lasso)"),
    ("Tool.FreeTransform", "SelectTool(FreeTransform)"),
    ("Tool.Pen", "SelectTool(Pen)"),
    ("Tool.Text", "SelectTool(Text)"),
    ("Tool.Line", "SelectTool(Line)"),
    ("Tool.Rectangle", "SelectTool(Rectangle)"),
    ("Tool.Oval", "SelectTool(Oval)"),
    ("Tool.Brush", "SelectTool(Brush)"),
    ("Tool.IKBone", "SelectTool(Bone)"),
    ("Tool.AssetWarp", "SelectTool(AssetWarp)"),
    ("Tool.PaintBucket", "SelectTool(PaintBucket)"),
    ("Tool.InkBottle", "SelectTool(InkBottle)"),
    ("Tool.Eyedropper", "SelectTool(Eyedropper)"),
    ("Tool.Eraser", "SelectTool(Eraser)"),
    ("Tool.Camera", "SelectTool(Camera)"),
    ("Tool.Hand", "SelectTool(Hand)"),
    ("Tool.Zoom", "SelectTool(Zoom)"),
    ("Tool.GradientTransform", "SelectTool(GradientTransform)"),
];

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// Commands and tools given keys.
    pub commands: usize,
    /// Shelf commands given keys.
    pub scripts: usize,
    /// `"Ink Bottle (S)"` — defaults switched off because the keymap gave
    /// their key to something else.
    pub unbound: Vec<String>,
    /// `"6: eyeRight keeps it; 2RevLeftHandWristPalm has no key now"`.
    pub conflicts: Vec<String>,
    /// Animate commands with no equivalent here.
    pub unmapped: Vec<String>,
}

/// The default chord of a command or tool id, as this program ships it.
fn default_chord(id: &str, commands: &[Command]) -> Option<KeyChord> {
    if let Some(tool) = id.strip_prefix("SelectTool(").and_then(|r| r.strip_suffix(')')) {
        let tool = crate::tools::all_tools().into_iter().find(|t| format!("{t:?}") == tool)?;
        let key = tool.shortcut()?;
        return Some(KeyChord::from_shortcut(egui::KeyboardShortcut::new(egui::Modifiers::NONE, key)));
    }
    let command = commands.iter().find(|c| command_id(**c) == id)?;
    command.shortcut().map(KeyChord::from_shortcut)
}

/// Apply an Animate keymap to `workspace`. `commands` is every command the
/// keyboard can raise (tools are added here), so clashing defaults can be found.
pub fn apply(workspace: &mut Workspace, bindings: &[AnimateBinding], commands: &[Command]) -> ImportReport {
    let mut report = ImportReport::default();
    let table: BTreeMap<&str, &str> = TABLE.iter().copied().collect();

    // Who claims each chord, first come first served.
    let mut owner: BTreeMap<KeyChord, Target> = BTreeMap::new();
    let mut wanted: Vec<(Target, KeyChord)> = Vec::new();
    let mut unmapped: BTreeSet<String> = BTreeSet::new();
    for b in bindings.iter().filter(|b| !b.hold) {
        let target = if let Some(name) = b.command.strip_prefix("Commands.") {
            Target::Script(name.to_string())
        } else if let Some(id) = table.get(b.command.as_str()) {
            Target::Command((*id).to_string())
        } else {
            unmapped.insert(b.command.clone());
            continue;
        };
        for chord in chords(b) {
            match owner.get(&chord) {
                Some(t) if *t == target => {}
                Some(t) => report.conflicts.push(format!(
                    "{}: {} keeps it; {} has no key now",
                    chord.label(),
                    target_name(t),
                    target_name(&target)
                )),
                None => {
                    owner.insert(chord.clone(), target.clone());
                    wanted.push((target.clone(), chord));
                }
            }
        }
    }
    report.unmapped = unmapped.into_iter().collect();

    // Grouped per target, in the keymap's order.
    let mut per_target: Vec<(Target, Vec<KeyChord>)> = Vec::new();
    for (target, chord) in wanted {
        match per_target.iter_mut().find(|(t, _)| *t == target) {
            Some((_, list)) => list.push(chord),
            None => per_target.push((target, vec![chord])),
        }
    }

    workspace.script_keys.clear();
    workspace.extra_keys.clear();
    let mut given: BTreeSet<String> = BTreeSet::new();
    for (target, list) in per_target {
        match target {
            Target::Script(name) => {
                workspace.script_keys.insert(name, list);
                report.scripts += 1;
            }
            Target::Command(id) => {
                // **Animate's keys are added, not swapped in.** The built-in key
                // stays unless the keymap gave it to something else: Redo keeps
                // Cmd+Shift+Z beside Animate's Cmd+Y, Insert Keyframe keeps F6
                // beside S. Only a default that is now somebody else's key goes.
                let default = default_chord(&id, commands);
                let keep_default = default.as_ref().is_some_and(|d| owner.get(d).is_none_or(|t| *t == Target::Command(id.clone())));
                let extras: Vec<KeyChord> = if keep_default {
                    workspace.keymap.remove(&id);
                    list.into_iter().filter(|c| Some(c) != default.as_ref()).collect()
                } else {
                    let (first, rest) = list.split_first().expect("never empty");
                    workspace.keymap.insert(id.clone(), Some(first.clone()));
                    rest.to_vec()
                };
                if !extras.is_empty() {
                    workspace.extra_keys.insert(id.clone(), extras);
                }
                given.insert(id);
                report.commands += 1;
            }
        }
    }

    // Every default left standing on a key the keymap gave away is switched off.
    let mut ids: Vec<String> = commands.iter().map(|c| command_id(*c)).collect();
    ids.extend(crate::tools::all_tools().into_iter().map(|t| command_id(Command::SelectTool(t))));
    ids.push(command_id(Command::PlayPause));
    for id in ids {
        if given.contains(&id) {
            continue;
        }
        let effective = match workspace.keymap.get(&id) {
            Some(chord) => chord.clone(),
            None => default_chord(&id, commands),
        };
        if let Some(chord) = effective
            && owner.contains_key(&chord)
        {
            workspace.keymap.insert(id.clone(), None);
            report.unbound.push(format!("{} ({})", pretty(&id), chord.label()));
        }
    }
    report
}

fn target_name(t: &Target) -> String {
    match t {
        Target::Script(s) => s.clone(),
        Target::Command(id) => pretty(id),
    }
}

/// `SelectTool(InkBottle)` → `Ink Bottle tool`; `EditSymbol` → `Edit Symbol`.
fn pretty(id: &str) -> String {
    let (word, suffix) = match id.strip_prefix("SelectTool(").and_then(|r| r.strip_suffix(')')) {
        Some(tool) => (tool, " tool"),
        None => (id, ""),
    };
    let mut out = String::new();
    for (i, c) in word.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out + suffix
}

/// Find Animate keymap files in a Configuration folder (`Keyboard Shortcuts/`,
/// or `Shortcuts/` as a copied folder may name it), newest first.
pub fn keymap_files(config: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = ["Keyboard Shortcuts", "Shortcuts"]
        .iter()
        .flat_map(|d| std::fs::read_dir(config.join(d)).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("xml")))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    files.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kc(cmd: &str, key: &str, m: &str) -> String {
        let mods: String = m.split('+').filter(|s| !s.is_empty()).map(|s| format!("<{s}/>")).collect();
        let key = if let Some(code) = key.strip_prefix('#') {
            format!("<key>{code}</key>")
        } else {
            format!("<uchar>{key}</uchar>")
        };
        format!("<keycmd>\r\n<cmd>{cmd}</cmd>\r\n{key}\r\n<mod>\r\n{mods}\r\n</mod>\r\n</keycmd>\r\n")
    }

    fn keymap(entries: &[(&str, &str, &str)]) -> String {
        let mut s = String::from("<?xml version='1.0'?>\r\n<keymap app=\"Adobe Animate\" version=\"23.0.0\">\r\n");
        for (c, k, m) in entries {
            s += &kc(c, k, m);
        }
        s + "</keymap>"
    }

    fn commands() -> Vec<Command> {
        vec![Command::Save, Command::ImportToStage, Command::EditSymbol, Command::InsertKeyframe, Command::InsertFrame, Command::RenameFaceLayers]
    }

    #[test]
    fn a_keymap_is_read() {
        let b = parse(&keymap(&[("File.Save", "S", "cmdkey"), ("Timeline.InsertFrame", "#11", ""), ("Grid.ShowGrid", "&apos;", "cmdkey")]));
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].command, "File.Save");
        assert!(b[0].cmdkey && !b[0].shift);
        assert_eq!(chords(&b[1])[0].key, "F5");
        assert_eq!(chords(&b[2])[0].key, egui::Key::Quote.name());
    }

    /// **The person's keys, with BuzzAnimate's clashing defaults stepping aside**
    /// — exactly the three their own BuzzAnimate keymap switched off by hand.
    #[test]
    fn scripts_take_their_keys_and_clashing_defaults_step_aside() {
        let mut ws = Workspace::default();
        let b = parse(&keymap(&[
            ("Commands.renameLayers", "R", "cmdkey"),
            ("Commands.renameHeadLayers", "R", "shift"),
            ("Commands.copyExtendFrames", "E", "cmdkey"),
            ("Commands.eyeLeft", "4", ""),
            ("Timeline.InsertKeyframe", "S", ""),
            ("Timeline.ConvertToKeyframes", "#12", ""),
        ]));
        let report = apply(&mut ws, &b, &commands());
        assert_eq!(report.scripts, 4);
        assert_eq!(ws.script_keys["renameLayers"][0], KeyChord { command: true, ctrl: false, alt: false, shift: false, key: "R".into() });
        for id in ["ImportToStage", "EditSymbol", "RenameFaceLayers"] {
            assert_eq!(ws.keymap.get(id), Some(&None), "{id} should be switched off: {report:?}");
        }
        // Insert Keyframe keeps its own F6, and gains S.
        assert!(!ws.keymap.contains_key("InsertKeyframe"), "F6 is still its key");
        assert_eq!(ws.extra_keys["InsertKeyframe"], vec![KeyChord { command: false, ctrl: false, alt: false, shift: false, key: "S".into() }]);
        // The Ink Bottle's S went to Insert Keyframe.
        assert_eq!(ws.keymap.get("SelectTool(InkBottle)"), Some(&None));
        assert!(report.unbound.iter().any(|u| u.contains("Ink Bottle")), "{:?}", report.unbound);
    }

    #[test]
    fn one_key_given_twice_keeps_the_first() {
        let mut ws = Workspace::default();
        let b = parse(&keymap(&[("Commands.eyeRight", "6", ""), ("Commands.2RevLeftHandWristPalm", "6", "")]));
        let report = apply(&mut ws, &b, &commands());
        assert!(ws.script_keys.contains_key("eyeRight"));
        assert!(!ws.script_keys.contains_key("2RevLeftHandWristPalm"));
        assert_eq!(report.conflicts.len(), 1);
    }

    #[test]
    fn keypad_keys_also_answer_on_a_laptop() {
        let b = parse(&keymap(&[("Commands.eyebrowsUp", "+", ""), ("Commands.eyeBrowUpLess", "+", "shift"), ("Commands.rotateEyeBrowUp", "*", "")]));
        let up = chords(&b[0]);
        assert_eq!(up.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), vec!["Plus", "Equals"]);
        assert!(chords(&b[1])[0].shift);
        let star = chords(&b[2]);
        assert_eq!((star[0].key.as_str(), star[0].shift), ("8", true));
    }

    /// A command whose own key went to something else moves to Animate's key.
    #[test]
    fn a_tool_whose_letter_was_taken_moves_to_its_animate_key() {
        let mut ws = Workspace::default();
        let b = parse(&keymap(&[("Insert.ClassicTween", "C", ""), ("Tool.Camera", "M", "")]));
        let commands = [Command::CreateClassicTween];
        apply(&mut ws, &b, &commands);
        assert_eq!(ws.keymap.get("SelectTool(Camera)").cloned().flatten().map(|c| c.key), Some("M".into()));
        assert_eq!(ws.keymap.get("SelectTool(Bone)"), Some(&None), "M was the Bone tool's");
    }

    #[test]
    fn a_default_keymap_entry_leaves_no_override() {
        let mut ws = Workspace::default();
        apply(&mut ws, &parse(&keymap(&[("File.Save", "S", "cmdkey")])), &commands());
        assert!(!ws.keymap.contains_key("Save"), "Save's key is already Cmd+S");
    }
}
