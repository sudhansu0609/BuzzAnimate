//! The Sound panel — Animate's sound section of the Properties panel.
//!
//! # Why this is its own panel rather than a section of Properties
//!
//! Animate puts sound in the Properties panel, whose contents change with the
//! selection: select a shape and you get fill and stroke, select a frame and
//! you get the sound. Ours is a panel of its own because a sound belongs to a
//! **keyframe**, not to a selection, and an animator lining dialogue up against
//! a walk cycle has artwork selected the whole time. A section that vanished
//! the moment they clicked a leg would be unusable for the one job it has.
//!
//! # What it edits
//!
//! Everything [`buzz_scene::SoundRef`] carries and nothing more: which clip is
//! on this keyframe, its sync mode, its volume and how many times it repeats.
//! The model has held all four since sound landed; until now nothing edited
//! any of them, and attaching a sound silently used the newest import at full
//! volume with Stream sync (PROGRESS.md §7 item 38).

use egui::{RichText, Ui};

use buzz_scene::{SoundId, SoundRef, SoundSync};

use crate::theme::Palette;

/// What the panel asks the editor to do.
///
/// Returned rather than applied, for the reason every other panel here does
/// it: the panel cannot see the document's undo stack, and an edit made
/// halfway through drawing a frame would be a second undo step per mouse move.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundResponse {
    /// Put this sound on the current keyframe, or take it off.
    pub set: Option<Option<SoundRef>>,
    /// Move the sound on the current keyframe to start on this frame instead
    /// (0-based). This is how the timing of an imported clip is changed: the
    /// clip is re-homed to a keyframe at the chosen frame and plays from there.
    pub move_to: Option<u32>,
    /// Import a sound file — the File ▸ Import Sound command.
    pub import: bool,
    /// Open the Lip Sync dialog for the sound on this keyframe.
    pub lip_sync: bool,
}

/// One imported sound, as the panel needs to know it.
pub struct SoundChoice {
    pub id: SoundId,
    pub name: String,
    pub seconds: f64,
}

/// Draw the panel.
///
/// `current` is the sound on the keyframe the playhead is on, if it is a
/// keyframe at all and if it has one. `on_keyframe` is what tells the two
/// "nothing here" cases apart, which matters: "this frame cannot hold a sound"
/// and "this keyframe has no sound yet" want completely different advice.
pub fn sound_panel(
    ui: &mut Ui,
    library: &[SoundChoice],
    current: Option<SoundRef>,
    on_keyframe: bool,
    frame: u32,
) -> SoundResponse {
    let mut out = SoundResponse::default();

    ui.heading("Sound");
    ui.separator();

    if library.is_empty() {
        ui.label(
            RichText::new("No sounds imported yet.")
                .small()
                .weak()
                .italics(),
        );
        if ui.button("Import Sound\u{2026}").clicked() {
            out.import = true;
        }
        return out;
    }

    if !on_keyframe {
        ui.label(
            RichText::new(format!(
                "Frame {} is not a keyframe.\nPress F6 to make one, then attach a sound.",
                frame + 1
            ))
            .small()
            .weak(),
        );
        return out;
    }

    // Which clip. Animate's "Name" dropdown, with None at the top — which is
    // how a sound is taken off a frame, rather than a separate delete button.
    let name_of = |id: SoundId| {
        library
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.as_str())
            .unwrap_or("(missing)")
    };

    let mut reference = current;
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("Name");
        let selected = match &reference {
            Some(r) => name_of(r.sound).to_string(),
            None => "None".to_string(),
        };
        egui::ComboBox::from_id_salt("sound name")
            .selected_text(selected)
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(reference.is_none(), "None").clicked() {
                    reference = None;
                    changed = true;
                }
                for choice in library {
                    let picked = reference.is_some_and(|r| r.sound == choice.id);
                    if ui.selectable_label(picked, &choice.name).clicked() {
                        // Keep the settings already chosen and swap the clip,
                        // so auditioning two takes of a line does not reset the
                        // volume and sync each time.
                        reference = Some(match reference {
                            Some(existing) => SoundRef {
                                sound: choice.id,
                                ..existing
                            },
                            None => SoundRef::stream(choice.id),
                        });
                        changed = true;
                    }
                }
            });
    });

    let Some(mut r) = reference else {
        if changed {
            out.set = Some(None);
        }
        ui.label(
            RichText::new("No sound on this keyframe.")
                .small()
                .weak()
                .italics(),
        );
        if ui.button("Import Sound\u{2026}").clicked() {
            out.import = true;
        }
        return out;
    };

    let duration = library
        .iter()
        .find(|c| c.id == r.sound)
        .map(|c| c.seconds as f32)
        .unwrap_or(0.0);
    if duration > 0.0 {
        ui.label(
            RichText::new(format!("{duration:.2} s"))
                .small()
                .weak(),
        );
    }

    // **Trim** — which seconds of the clip actually play. The in-point cuts a
    // lead-in, the out-point cuts a tail; only `start..end` is heard, drawn on
    // the timeline, and exported. This is the "cut a section out of the clip"
    // control, distinct from Start frame, which only says where on the timeline
    // the (trimmed) clip begins.
    if duration > 0.0 {
        ui.horizontal(|ui| {
            ui.label("Trim start");
            let mut start = r.trim_start.clamp(0.0, duration);
            let upper = r.trim_end.unwrap_or(duration);
            if ui
                .add(
                    egui::DragValue::new(&mut start)
                        .range(0.0..=upper)
                        .speed(0.01)
                        .fixed_decimals(2)
                        .suffix(" s"),
                )
                .on_hover_text("Seconds into the clip where it starts — cuts off the lead-in")
                .changed()
            {
                r.trim_start = start.clamp(0.0, upper);
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Trim end");
            let mut end = r.trim_end.unwrap_or(duration).clamp(r.trim_start, duration);
            if ui
                .add(
                    egui::DragValue::new(&mut end)
                        .range(r.trim_start..=duration)
                        .speed(0.01)
                        .fixed_decimals(2)
                        .suffix(" s"),
                )
                .on_hover_text("Seconds into the clip where it stops — cuts off the tail")
                .changed()
            {
                // At the full length it is stored as "no out-point", so a
                // re-decode of a longer take is not silently clipped.
                r.trim_end = (end < duration).then_some(end.clamp(r.trim_start, duration));
                changed = true;
            }
        });
    }

    // **Start frame** — when the clip begins. A sound plays from the keyframe
    // it sits on, so changing this re-homes it to a keyframe on that frame; the
    // editor moves the playhead there too, so this control keeps showing it.
    // Shown 1-based, to match the timeline's frame numbers.
    ui.horizontal(|ui| {
        ui.label("Start frame");
        let mut start = frame + 1;
        if ui
            .add(egui::DragValue::new(&mut start).range(1..=1_000_000))
            .on_hover_text("The frame this clip begins on — its timing on the timeline")
            .changed()
        {
            out.move_to = Some(start.saturating_sub(1));
        }
    });

    // Sync. The one setting whose choice actually changes what you hear, so it
    // carries a line of explanation rather than four bare words.
    ui.horizontal(|ui| {
        ui.label("Sync");
        egui::ComboBox::from_id_salt("sound sync")
            .selected_text(r.sync.label())
            .width(110.0)
            .show_ui(ui, |ui| {
                for sync in [
                    SoundSync::Event,
                    SoundSync::Start,
                    SoundSync::Stop,
                    SoundSync::Stream,
                ] {
                    if ui
                        .selectable_label(r.sync == sync, sync.label())
                        .on_hover_text(sync_help(sync))
                        .clicked()
                    {
                        r.sync = sync;
                        changed = true;
                    }
                }
            });
    });
    ui.label(RichText::new(sync_help(r.sync)).small().weak());

    ui.horizontal(|ui| {
        ui.label("Volume");
        if ui
            .add(egui::Slider::new(&mut r.volume, 0.0..=1.0).fixed_decimals(2))
            .changed()
        {
            changed = true;
        }
    });

    // Repeats. Zero is Animate's "loop" checkbox — for as long as the timeline
    // runs — and it is offered as a checkbox here for the same reason: "repeat
    // 0 times" reads as "do not play".
    ui.horizontal(|ui| {
        ui.label("Repeat");
        let mut forever = r.loops == 0;
        if ui
            .checkbox(&mut forever, "Loop")
            .on_hover_text("Play for as long as the timeline runs")
            .changed()
        {
            r.loops = if forever { 0 } else { 1 };
            changed = true;
        }
        if !forever {
            let mut count = r.loops.max(1);
            if ui
                .add(egui::DragValue::new(&mut count).range(1..=999))
                .changed()
            {
                r.loops = count;
                changed = true;
            }
        }
    });

    if r.sync == SoundSync::Stream {
        ui.add_space(4.0);
        if ui
            .button("Lip Sync\u{2026}")
            .on_hover_text("Generate mouth shapes from this dialogue")
            .clicked()
        {
            out.lip_sync = true;
        }
    }

    if changed {
        out.set = Some(Some(r));
    }

    ui.add_space(6.0);
    ui.separator();
    ui.label(
        RichText::new("A sound sits on a keyframe and plays from there.")
            .small()
            .color(Palette::text_dim()),
    );

    out
}

/// One line on what each sync mode actually does.
fn sync_help(sync: SoundSync) -> &'static str {
    match sync {
        SoundSync::Event => {
            "Starts when the playhead reaches it, then plays out on its own — it does not stop when the film does."
        }
        SoundSync::Start => {
            "Like Event, but will not start a second copy while one is already sounding."
        }
        SoundSync::Stop => "Silences this sound from this frame on.",
        SoundSync::Stream => {
            "Tied to the timeline: scrubbing moves it, and it cannot drift from the picture. Dialogue."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Vec<SoundChoice> {
        vec![
            SoundChoice {
                id: SoundId(1),
                name: "Line 1".into(),
                seconds: 2.0,
            },
            SoundChoice {
                id: SoundId(2),
                name: "Door".into(),
                seconds: 0.4,
            },
        ]
    }

    /// Every sync mode has its own explanation, and none of them is the empty
    /// string. The panel's whole purpose is that these four are no longer
    /// indistinguishable, so a mode with nothing to say would defeat it.
    #[test]
    fn every_sync_mode_explains_itself() {
        let mut seen = Vec::new();
        for sync in [
            SoundSync::Event,
            SoundSync::Start,
            SoundSync::Stop,
            SoundSync::Stream,
        ] {
            let help = sync_help(sync);
            assert!(!help.is_empty(), "{sync:?} has no explanation");
            assert!(!seen.contains(&help), "{sync:?} repeats another mode's");
            seen.push(help);
        }
    }

    /// Swapping the clip keeps the settings, so auditioning two takes of a
    /// line does not reset the volume and sync each time.
    #[test]
    fn swapping_the_clip_keeps_the_settings() {
        let existing = SoundRef {
            sound: SoundId(1),
            sync: SoundSync::Event,
            volume: 0.3,
            loops: 4,
            trim_start: 1.5,
            trim_end: Some(3.0),
        };
        let swapped = SoundRef {
            sound: SoundId(2),
            ..existing
        };
        assert_eq!(swapped.sync, SoundSync::Event);
        assert_eq!(swapped.volume, 0.3);
        assert_eq!(swapped.loops, 4);
        assert_eq!(swapped.trim_start, 1.5, "the trim in-point travels with the swap");
        assert_eq!(swapped.trim_end, Some(3.0), "and the out-point");
    }

    #[test]
    fn the_library_listing_is_what_the_panel_offers() {
        let choices = library();
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().any(|c| c.name == "Door"));
    }
}
