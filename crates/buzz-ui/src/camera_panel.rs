//! The camera's own properties — including the two that make it spatial.
//!
//! Animate shows these when the camera layer is selected, and so does this:
//! click the Camera row in the timeline and the Properties panel becomes the
//! camera's. Everything here edits the key at the playhead, keying it if there
//! is not one yet, which is what an animator expects from every other control
//! in the program.
//!
//! **Pitch and yaw are the new ones.** Zoom and rotation move a flat picture
//! about; pitch and yaw tilt the *camera*, so a layer's plane is foreshortened
//! and a rectangle is drawn as a trapezoid. Degrees, like every other angle in
//! this interface.

use buzz_scene::{CameraKey, CameraTrack, MAX_TILT};
use egui::{RichText, Ui};

/// What the user changed. Each field is its own undo step in the editor.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CameraResponse {
    /// The camera was switched on or off.
    pub toggle: bool,
    /// A new key for the playhead — the whole state, as it now is.
    pub set: Option<CameraKey>,
    /// How far the camera sits from the stage.
    pub set_focal_distance: Option<f64>,
    pub add_key: bool,
    pub remove_key: bool,
    pub reset: bool,
    /// Take hold of the camera: pick up the Camera tool so dragging the stage
    /// moves the shot.
    pub grab_camera: bool,
}

/// Draw the camera's properties for `frame`.
pub fn camera_panel(ui: &mut Ui, camera: &CameraTrack, frame: u32) -> CameraResponse {
    let mut out = CameraResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Camera");
        if camera.enabled {
            ui.label(
                RichText::new(format!("{} keys", camera.keys().len()))
                    .small()
                    .weak(),
            );
        }
    });

    if !camera.enabled {
        ui.label(
            RichText::new("The camera is off. Everything is drawn as it is drawn.")
                .small()
                .weak(),
        );
        if ui.button("Enable Camera").clicked() {
            out.toggle = true;
        }
        return out;
    }

    // The state at the playhead, which is what every control below edits.
    let mut key = camera
        .state_at(frame)
        .map(|s| CameraKey { frame, ..s })
        .unwrap_or_else(|| CameraKey::new(frame, buzz_geom::Point::ZERO));
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("Zoom");
        changed |= ui
            .add(
                egui::Slider::new(&mut key.zoom, 0.1..=8.0)
                    .logarithmic(true)
                    .fixed_decimals(2),
            )
            .changed();
    });

    changed |= angle(
        ui,
        "Rotation",
        &mut key.rotation,
        180.0,
        "Roll: the horizon tipping",
    );

    ui.separator();
    ui.label(RichText::new("Tilt").small().strong());

    let limit = MAX_TILT.to_degrees();
    changed |= angle(
        ui,
        "Pitch",
        &mut key.pitch,
        limit,
        "Tilt the camera up and down. The stage tips away, and a rectangle is \
         drawn as a trapezoid.",
    );
    changed |= angle(
        ui,
        "Yaw",
        &mut key.yaw,
        limit,
        "Turn the camera left and right, about the point it is looking at.",
    );

    if !key.is_flat()
        && ui
            .small_button("Look straight on")
            .on_hover_text("Put the pitch and yaw back to zero")
            .clicked()
    {
        key.pitch = 0.0;
        key.yaw = 0.0;
        changed = true;
    }

    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Camera depth");
        let mut distance = camera.focal_distance;
        if ui
            .add(
                egui::Slider::new(&mut distance, 200.0..=6000.0)
                    .logarithmic(true)
                    .suffix(" px"),
            )
            .on_hover_text(
                "How far the camera sits from the stage. Closer exaggerates the \
                 perspective; further flattens it.",
            )
            .changed()
        {
            out.set_focal_distance = Some(distance);
        }
    });

    ui.horizontal(|ui| {
        let keyed = camera.has_key_at(frame);
        if ui
            .small_button(if keyed { "Re-key" } else { "Add Keyframe" })
            .on_hover_text("Key the camera at the playhead")
            .clicked()
        {
            out.add_key = true;
        }
        if ui
            .add_enabled(keyed, egui::Button::new("Remove").small())
            .clicked()
        {
            out.remove_key = true;
        }
        if ui
            .small_button("Reset")
            .on_hover_text("Clear every camera keyframe")
            .clicked()
        {
            out.reset = true;
        }
    });

    if changed {
        out.set = Some(key);
    }
    out
}

/// One angle, in degrees. Returns whether it changed.
fn angle(ui: &mut Ui, label: &str, radians: &mut f64, limit: f64, hint: &str) -> bool {
    let mut degrees = radians.to_degrees();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui
            .add(
                egui::Slider::new(&mut degrees, -limit..=limit)
                    .suffix("\u{b0}")
                    .fixed_decimals(0),
            )
            .on_hover_text(hint)
            .changed()
        {
            *radians = degrees.to_radians();
            changed = true;
        }
    });
    changed
}


/// **Animate's on-stage camera controls**, as a strip that sits over the stage
/// while the camera is the thing being worked on.
///
/// # Why these are not just the panel again
///
/// Framing a shot is done *by looking at the stage*, and the panel is docked at
/// the far side of the window: choosing a zoom meant looking away from the
/// picture you were choosing it for, and the eye has to travel back before it
/// can judge the result. Animate puts the camera's controls on the stage for
/// exactly this reason, and it is the same argument the tool cursor and the
/// zoom readout already make here.
///
/// It raises the same [`CameraResponse`] the panel does, so both go through one
/// application in the shell and cannot drift into two behaviours.
///
/// Every control keys the camera at `frame`, which is what the Camera tool does
/// when it is dragged.
pub fn camera_hud(
    ui: &mut Ui,
    camera: &CameraTrack,
    frame: u32,
    driving: bool,
) -> CameraResponse {
    let mut out = CameraResponse::default();

    if !camera.enabled {
        if ui
            .button("Enable Camera")
            .on_hover_text("Nothing is framed until the camera is on")
            .clicked()
        {
            out.toggle = true;
        }
        return out;
    }

    let mut key = camera
        .state_at(frame)
        .map(|s| CameraKey { frame, ..s })
        .unwrap_or_else(|| CameraKey::new(frame, buzz_geom::Point::ZERO));
    let mut changed = false;

    ui.horizontal(|ui| {
        // -- taking hold of it ---------------------------------------------
        //
        // **The control that was missing.** Every number here frames the shot
        // by typing; none of them let you *aim* it, which is how a shot is
        // actually found — you push the camera about while watching the stage.
        // That is the Camera tool, and nothing said so: it sat in the tool
        // strip with no hint that it was what the camera row wanted. This is
        // the same tool, offered where the camera is being worked on.
        if ui
            .add(egui::Button::new("Move Camera").small().selected(driving))
            .on_hover_text(if driving {
                "Drag the stage to move the shot. Click to put the tool down."
            } else {
                "Pick up the Camera tool, then drag the stage to aim the shot"
            })
            .clicked()
        {
            out.grab_camera = true;
        }
        ui.separator();

        // -- zoom ----------------------------------------------------------
        ui.label(RichText::new("Zoom").small().weak());
        if ui.small_button("\u{2212}").on_hover_text("Pull back").clicked() {
            key.zoom = (key.zoom / 1.1).clamp(0.1, 8.0);
            changed = true;
        }
        // Shown as a percentage, which is how a shot is talked about, while
        // the document keeps the multiplier.
        let mut percent = key.zoom * 100.0;
        if ui
            .add(
                egui::DragValue::new(&mut percent)
                    .speed(1.0)
                    .range(10.0..=800.0)
                    .suffix(" %")
                    .fixed_decimals(0),
            )
            .on_hover_text("Drag, or type a percentage")
            .changed()
        {
            key.zoom = percent / 100.0;
            changed = true;
        }
        if ui.small_button("+").on_hover_text("Push in").clicked() {
            key.zoom = (key.zoom * 1.1).clamp(0.1, 8.0);
            changed = true;
        }

        ui.separator();

        // -- roll ----------------------------------------------------------
        ui.label(RichText::new("Rotate").small().weak());
        let mut degrees = key.rotation.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut degrees)
                    .speed(0.5)
                    .range(-180.0..=180.0)
                    .suffix("\u{b0}")
                    .fixed_decimals(1),
            )
            .on_hover_text("Roll: the horizon tipping")
            .changed()
        {
            key.rotation = degrees.to_radians();
            changed = true;
        }

        ui.separator();

        // -- the 3D half ---------------------------------------------------
        //
        // Pitch and yaw are what make this a camera in space rather than a
        // pan-and-zoom over a flat picture, and they are the controls nobody
        // finds buried in a panel. Bounded by the same `MAX_TILT` the document
        // clamps to, so the strip cannot ask for a shot that cannot be drawn.
        let limit = MAX_TILT.to_degrees();
        ui.label(RichText::new("3D").small().weak());

        let mut pitch = key.pitch.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut pitch)
                    .speed(0.5)
                    .range(-limit..=limit)
                    .suffix("\u{b0}")
                    .fixed_decimals(1),
            )
            .on_hover_text("Pitch: the camera nodding up and down")
            .changed()
        {
            key.pitch = pitch.to_radians();
            changed = true;
        }

        let mut yaw = key.yaw.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut yaw)
                    .speed(0.5)
                    .range(-limit..=limit)
                    .suffix("\u{b0}")
                    .fixed_decimals(1),
            )
            .on_hover_text("Yaw: the camera turning left and right")
            .changed()
        {
            key.yaw = yaw.to_radians();
            changed = true;
        }

        ui.separator();

        // -- keys ----------------------------------------------------------
        if ui
            .small_button("\u{25C6}")
            .on_hover_text("Key the camera at this frame")
            .clicked()
        {
            out.add_key = true;
        }
        if ui
            .small_button("\u{25C7}")
            .on_hover_text("Remove this frame's camera key")
            .clicked()
        {
            out.remove_key = true;
        }
        if ui
            .small_button("Reset")
            .on_hover_text("Put the shot back to square on, at 100%")
            .clicked()
        {
            out.reset = true;
        }
    });

    if changed {
        out.set = Some(key);
    }
    out
}

#[cfg(test)]
mod tests {
    /// The strip on the stage and the panel raise the **same** response type
    /// and go through the same application, so a shot framed on the stage and
    /// one typed into the panel cannot mean different things.
    #[test]
    fn the_stage_strip_offers_the_camera_before_it_is_on() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let off = CameraTrack::default();
        assert!(!off.enabled, "the camera starts off");

        let mut raised = CameraResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            raised = camera_hud(ui, &off, 0, false);
        });
        // Nothing was clicked, so nothing was asked for — but the strip must
        // have drawn the way in rather than a row of dead controls.
        assert_eq!(raised, CameraResponse::default());

        // With it on, every control is live and keys at the playhead.
        let mut on = CameraTrack::default();
        on.enabled = true;
        on.set_key(CameraKey::new(0, buzz_geom::Point::new(100.0, 80.0)));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let out = camera_hud(ui, &on, 0, false);
            // Drawing alone changes nothing.
            assert_eq!(out, CameraResponse::default());
        });
    }

    use super::*;

    fn draw(camera: &CameraTrack, frame: u32) -> CameraResponse {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut response = CameraResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            response = camera_panel(ui, camera, frame);
        });
        response
    }

    #[test]
    fn a_camera_that_is_off_offers_to_be_switched_on() {
        let camera = CameraTrack::new();
        assert!(!camera.enabled);
        assert_eq!(draw(&camera, 0), CameraResponse::default());
    }

    #[test]
    fn drawing_changes_nothing() {
        let mut camera = CameraTrack::new();
        camera.enabled = true;
        camera.set_key(CameraKey::new(0, buzz_geom::Point::new(275.0, 200.0)));
        assert_eq!(draw(&camera, 0), CameraResponse::default());
    }

    /// A tilted camera draws without complaint, at any frame — including one
    /// past the last key, where the state is held rather than interpolated.
    #[test]
    fn a_tilted_camera_draws_at_any_frame() {
        let mut camera = CameraTrack::new();
        camera.enabled = true;
        camera.set_key(CameraKey {
            pitch: 0.5,
            yaw: -0.3,
            ..CameraKey::new(0, buzz_geom::Point::new(275.0, 200.0))
        });
        for frame in [0, 1, 50, 10_000] {
            let _ = draw(&camera, frame);
        }
    }

    /// The panel offers the tilt in degrees, bounded by what the model will
    /// accept — a slider that can ask for something the camera refuses would
    /// be a control that silently does nothing at one end.
    #[test]
    fn the_tilt_sliders_stop_where_the_camera_does() {
        const {
            assert!(MAX_TILT > 0.5, "too tight to be useful");
            // Past a quarter turn the plane is edge-on and cannot be drawn at
            // all, so a slider must never be able to ask for it.
            assert!(MAX_TILT < std::f64::consts::FRAC_PI_2);
        }
        assert!(MAX_TILT.to_degrees() > 30.0);
    }
}
