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

#[cfg(test)]
mod tests {
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
