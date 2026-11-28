//! **The Shadow layer's properties** (user 2026-11-28): click the Shadow row
//! in the timeline and the Properties panel lists every object on the stage
//! at the playhead, each with its own shadow — on or off, which way it falls,
//! how long, its offset, scale and skew about the object's feet, how soft,
//! how dark and what colour. A change is keyed the way the camera's is: at
//! the playhead with auto-keyframe on, else on the key before it.

use buzz_scene::{ObjectId, ObjectShadow};
use egui::{RichText, Ui};

/// One object on the stage, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowEntry {
    pub object: ObjectId,
    /// What it is called: its symbol's name, else its layer's.
    pub name: String,
    /// Its shadow at the playhead; `None` when it has none.
    pub shadow: Option<ObjectShadow>,
}

/// What the panel was asked to do this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ShadowEdit {
    /// One object's shadow, as it is to be.
    Set(ObjectId, ObjectShadow),
    /// Every listed object's shadow on (those without one get the default),
    /// or every one off.
    All(bool),
}

/// Draw the panel. `keyed_here`: the Shadow row has a key at the playhead;
/// `auto_key`: a change keys the playhead.
pub fn shadow_panel(ui: &mut Ui, entries: &[ShadowEntry], locked: bool, keyed_here: bool, auto_key: bool) -> Option<ShadowEdit> {
    let mut edit = None;
    ui.heading("Object Shadows");
    ui.label(
        RichText::new(if locked {
            "The Shadow row is locked \u{2014} unlock it to change the shadows."
        } else if keyed_here {
            "Changes here edit the shadow keyframe at the playhead."
        } else if auto_key {
            "Changes here add a shadow keyframe at the playhead."
        } else {
            "Auto Keyframe is off: changes edit the shadow keyframe before the playhead."
        })
        .small()
        .weak(),
    );
    ui.add_space(4.0);
    if entries.is_empty() {
        ui.label(RichText::new("Nothing on the stage at this frame.").weak());
        return None;
    }
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            if ui.button("All on").clicked() {
                edit = Some(ShadowEdit::All(true));
            }
            if ui.button("All off").clicked() {
                edit = Some(ShadowEdit::All(false));
            }
        });
        ui.separator();
        for entry in entries {
            let mut shadow = entry.shadow.unwrap_or(ObjectShadow { on: false, ..ObjectShadow::default() });
            let before = shadow;
            let id = ui.make_persistent_id(("shadow-entry", entry.object.0));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    ui.checkbox(&mut shadow.on, "").on_hover_text("This object casts its shadow");
                    ui.label(&entry.name);
                })
                .body(|ui| {
                    ui.add_enabled_ui(shadow.on, |ui| shadow_controls(ui, &mut shadow, entry.object));
                });
            if shadow != before {
                edit = Some(ShadowEdit::Set(entry.object, shadow));
            }
        }
    });
    edit
}

/// The numbers of one shadow.
fn shadow_controls(ui: &mut Ui, shadow: &mut ObjectShadow, object: ObjectId) {
    egui::Grid::new(("shadow-grid", object.0)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Direction");
        let mut angle = shadow.angle.rem_euclid(360.0);
        if ui
            .add(egui::DragValue::new(&mut angle).speed(1.0).range(0.0..=360.0).suffix("\u{b0}"))
            .on_hover_text("Which way it falls: 0\u{b0} right, 90\u{b0} down towards you, 180\u{b0} left")
            .changed()
        {
            shadow.angle = angle;
        }
        ui.end_row();

        ui.label("Length");
        let mut length = shadow.length * 100.0;
        if ui
            .add(egui::DragValue::new(&mut length).speed(1.0).range(0.0..=300.0).suffix("%"))
            .on_hover_text("As a share of the object's height")
            .changed()
        {
            shadow.length = length / 100.0;
        }
        ui.end_row();

        ui.label("Offset");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut shadow.offset.x).speed(0.5).prefix("x "));
            ui.add(egui::DragValue::new(&mut shadow.offset.y).speed(0.5).prefix("y "));
        });
        ui.end_row();

        ui.label("Scale");
        ui.horizontal(|ui| {
            let mut sx = shadow.scale_x * 100.0;
            let mut sy = shadow.scale_y * 100.0;
            if ui.add(egui::DragValue::new(&mut sx).speed(1.0).range(1.0..=1000.0).prefix("w ").suffix("%")).changed() {
                shadow.scale_x = sx / 100.0;
            }
            if ui.add(egui::DragValue::new(&mut sy).speed(1.0).range(1.0..=1000.0).prefix("h ").suffix("%")).changed() {
                shadow.scale_y = sy / 100.0;
            }
        });
        ui.end_row();

        ui.label("Skew");
        ui.add(egui::DragValue::new(&mut shadow.skew).speed(0.5).range(-80.0..=80.0).suffix("\u{b0}"));
        ui.end_row();

        ui.label("Softness");
        ui.add(egui::DragValue::new(&mut shadow.softness).speed(0.25).range(0.0..=100.0))
            .on_hover_text("How soft its edge is: the blur, in stage units");
        ui.end_row();

        ui.label("Darkness");
        let mut dark = shadow.darkness * 100.0;
        if ui.add(egui::DragValue::new(&mut dark).speed(1.0).range(0.0..=100.0).suffix("%")).changed() {
            shadow.darkness = dark / 100.0;
        }
        ui.end_row();

        ui.label("Colour");
        let [r, g, b, _] = shadow.color.to_rgba8().to_u8_array();
        let mut rgb = [r, g, b];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            shadow.color = peniko::Color::from_rgb8(rgb[0], rgb[1], rgb[2]);
        }
        ui.end_row();
    });
    if ui.small_button("Reset").on_hover_text("Back to the default shadow").clicked() {
        *shadow = ObjectShadow::default();
    }
}
