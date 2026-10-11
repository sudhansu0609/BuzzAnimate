//! Dialog for "AI → Character from Image…":
//! 1. Pick a PNG.
//! 2. Background job detects matte and skeleton.
//! 3. Joint editor: interactive canvas with draggable joint markers for human-in-the-loop correction.
//! 4. Background jobs segment parts, complete hidden regions, and trace equal-width vector artwork.
//! 5. Preview dialog with line-width slider.
//! 6. Assembles Graphic symbols, depth-ordered character symbol, and biped armature into Library & Cast.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use crossbeam_channel::Receiver;
use egui::{Color32, ColorImage, Context, Pos2, Rect, Stroke, Ui, Vec2};
use image::{GrayImage, RgbaImage};

use buzz_ai::character::{
    assemble_character, complete_parts, part_masks, skeleton, trace_character_whole_with,
    AssembledCharacter, CompletedParts, PartMasks, Skeleton, TraceCharacterOptions, TracedCharacter,
};
use buzz_ai::pose::Person;
use buzz_ai::segment::matte;

/// Current stage in the Character from Image wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiCharacterStep {
    SelectImage,
    DetectingSkeleton,
    JointEditor,
    ProcessingParts,
    Preview,
    Completed,
}

/// Action produced by the dialog on user confirmation or cancellation.
pub enum AiCharacterDialogAction {
    None,
    /// The character, and which kind of rig the shell should build from it.
    Created(Box<AssembledCharacter>, buzz_rig::RigKind),
    Cancel,
}

/// State for the Character from Image dialog.
pub struct AiCharacterState {
    pub open: bool,
    pub step: AiCharacterStep,
    pub image_path_input: String,
    pub character_name: String,
    pub image_rgba: Option<Arc<RgbaImage>>,
    pub image_texture: Option<egui::TextureHandle>,
    pub matte: Option<Arc<GrayImage>>,
    pub skeleton: Option<Skeleton>,
    /// The skeleton exactly as detected, before any joint-editor correction.
    /// Kept separately from `skeleton` so parts processing can start from an
    /// intact skeleton — joints, bones, *and* face landmarks — and apply
    /// only the joints the user actually moved, rather than rebuilding one
    /// from scratch out of a mostly-empty keypoint array.
    pub detected_skeleton: Option<Skeleton>,
    pub original_joints: Vec<(String, Pos2)>,
    pub edited_joints: Vec<(String, Pos2)>,
    pub dragged_joint: Option<usize>,
    pub uniform_width: f64,
    /// How the parts are traced: bold, colours and the tuning, starting from the saved tuning; `uniform_width` above wins.
    pub trace_options: TraceCharacterOptions,
    pub parts: Option<PartMasks>,
    pub completed_parts: Option<CompletedParts>,
    pub traced: Option<TracedCharacter>,
    pub assembled: Option<Box<AssembledCharacter>>,
    pub status_message: Option<(String, bool)>, // (message, is_error)

    // Background workers
    pub rx_detect: Option<Receiver<Result<(GrayImage, Skeleton, Vec<(String, Pos2)>), String>>>,
    pub rx_process: Option<Receiver<Result<(PartMasks, CompletedParts, TracedCharacter), String>>>,
    /// The OS file picker, opened on a thread of its own — a blocking dialog
    /// call has no place on the UI thread, so "Browse…" hands it to a thread
    /// and this is where the chosen path (or none, on Cancel) arrives.
    pub rx_pick_file: Option<Receiver<Option<PathBuf>>>,
    /// The created character in the standard poses (rest, walk, run, sit,
    /// point, wave), rendered by the shell after **Create** and shown under
    /// the finished step (F7.0).
    pub pose_sheet: Vec<(String, egui::TextureHandle)>,
    /// The joint limits **Create Character** rigs with — the user's saved
    /// `rig_limits.json`, filled in by the app. `None` keeps the built-in
    /// anatomical ones.
    pub rig_limits: Option<buzz_rig::JointLimitDefaults>,
    /// What **Create Character** builds on the stage: an armature, or the
    /// parts on followed layers.
    pub rig_kind: buzz_rig::RigKind,
}

impl Default for AiCharacterState {
    fn default() -> Self {
        Self {
            open: false,
            step: AiCharacterStep::SelectImage,
            image_path_input: String::new(),
            character_name: "Character".to_string(),
            image_rgba: None,
            image_texture: None,
            matte: None,
            skeleton: None,
            detected_skeleton: None,
            original_joints: Vec::new(),
            edited_joints: Vec::new(),
            dragged_joint: None,
            uniform_width: 2.0,
            trace_options: TraceCharacterOptions {
                trace: buzz_scene::TraceOptions::ink_colour().with_saved_tuning(),
                ..Default::default()
            },
            parts: None,
            completed_parts: None,
            traced: None,
            assembled: None,
            status_message: None,
            rx_detect: None,
            rx_process: None,
            rx_pick_file: None,
            pose_sheet: Vec::new(),
            rig_limits: None,
            // LR2: new characters get a layer rig unless Bones is picked.
            rig_kind: buzz_rig::RigKind::Layers,
        }
    }
}

impl std::fmt::Debug for AiCharacterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiCharacterState")
            .field("open", &self.open)
            .field("step", &self.step)
            .field("character_name", &self.character_name)
            .finish()
    }
}

impl AiCharacterState {
    pub fn reset(&mut self) {
        self.step = AiCharacterStep::SelectImage;
        self.image_rgba = None;
        self.image_texture = None;
        self.matte = None;
        self.skeleton = None;
        self.detected_skeleton = None;
        self.original_joints.clear();
        self.edited_joints.clear();
        self.dragged_joint = None;
        self.parts = None;
        self.completed_parts = None;
        self.traced = None;
        self.assembled = None;
        self.status_message = None;
        self.rx_detect = None;
        self.rx_process = None;
        self.pose_sheet.clear();
    }

    /// Polls background task channels for completion.
    pub fn poll(&mut self, ctx: &Context) {
        if let Some(rx) = &self.rx_detect {
            if let Ok(res) = rx.try_recv() {
                self.rx_detect = None;
                match res {
                    Ok((matte_img, skel, joints)) => {
                        self.matte = Some(Arc::new(matte_img));
                        self.detected_skeleton = Some(skel.clone());
                        self.skeleton = Some(skel);
                        self.original_joints = joints.clone();
                        self.edited_joints = joints;
                        self.step = AiCharacterStep::JointEditor;
                        self.status_message = Some((
                            "Skeleton detected. Adjust joint positions if needed.".to_string(),
                            false,
                        ));
                    }
                    Err(err) => {
                        self.step = AiCharacterStep::SelectImage;
                        self.status_message = Some((err, true));
                    }
                }
                ctx.request_repaint();
            }
        }

        if let Some(rx) = &self.rx_process {
            if let Ok(res) = rx.try_recv() {
                self.rx_process = None;
                match res {
                    Ok((parts, completed, traced)) => {
                        self.parts = Some(parts);
                        self.completed_parts = Some(completed);
                        self.traced = Some(traced);
                        self.step = AiCharacterStep::Preview;
                        self.status_message = Some((
                            "Parts segmented, completed, and traced into vector artwork.".to_string(),
                            false,
                        ));
                    }
                    Err(err) => {
                        // A failed re-trace keeps the preview of the trace before it.
                        if self.step != AiCharacterStep::Preview {
                            self.step = AiCharacterStep::JointEditor;
                        }
                        self.status_message = Some((err, true));
                    }
                }
                ctx.request_repaint();
            }
        }

        if let Some(rx) = &self.rx_pick_file {
            if let Ok(picked) = rx.try_recv() {
                self.rx_pick_file = None;
                if let Some(path) = picked {
                    self.image_path_input = path.display().to_string();
                    if self.character_name == "Character" {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            self.character_name = stem.to_string();
                        }
                    }
                }
                ctx.request_repaint();
            } else {
                // Nothing to show while it is open but a spinner would be
                // over the top of the OS's own dialog — just keep the frame
                // loop alive so the answer is picked up the moment it lands.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Renders the "AI → Character from Image…" dialog window.
pub fn ai_character_dialog(ctx: &Context, state: &mut AiCharacterState) -> AiCharacterDialogAction {
    if !state.open {
        return AiCharacterDialogAction::None;
    }

    state.poll(ctx);

    let mut open = state.open;
    let mut action = AiCharacterDialogAction::None;

    egui::Window::new("Character from Image")
        .open(&mut open)
        .default_size([680.0, 560.0])
        .min_width(500.0)
        .min_height(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            render_dialog_body(ui, state, ctx, &mut action);
        });

    state.open = open;
    action
}

/// The dialog's content, drawn into any `Ui` — the AI panel's **Character
/// from Image** section draws exactly this (F7.0), the dialog wraps it.
pub fn character_body(ui: &mut Ui, state: &mut AiCharacterState, ctx: &Context) -> AiCharacterDialogAction {
    let mut action = AiCharacterDialogAction::None;
    render_dialog_body(ui, state, ctx, &mut action);
    if state.step == AiCharacterStep::Completed && !state.pose_sheet.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Pose sheet").strong());
        ui.horizontal_wrapped(|ui| {
            for (name, tex) in &state.pose_sheet {
                ui.vertical(|ui| {
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(96.0, 128.0)));
                    ui.label(egui::RichText::new(name).small());
                });
            }
        });
    }
    action
}

fn render_dialog_body(
    ui: &mut Ui,
    state: &mut AiCharacterState,
    ctx: &Context,
    action: &mut AiCharacterDialogAction,
) {
    // Stepper header
    ui.horizontal(|ui| {
        let steps = [
            ("1. Image", AiCharacterStep::SelectImage),
            ("2. Joints", AiCharacterStep::JointEditor),
            ("3. Preview", AiCharacterStep::Preview),
            ("4. Complete", AiCharacterStep::Completed),
        ];
        for (label, s) in steps {
            let active = state.step == s
                || (s == AiCharacterStep::JointEditor && state.step == AiCharacterStep::DetectingSkeleton)
                || (s == AiCharacterStep::Preview && state.step == AiCharacterStep::ProcessingParts);
            if active {
                ui.strong(label);
            } else {
                ui.label(egui::RichText::new(label).color(Color32::from_gray(140)));
            }
            ui.label("›");
        }
    });
    ui.separator();

    // Status / error banner
    if let Some((msg, is_err)) = &state.status_message {
        let col = if *is_err {
            Color32::from_rgb(220, 80, 80)
        } else {
            Color32::from_rgb(80, 180, 100)
        };
        ui.label(egui::RichText::new(msg).color(col));
        ui.add_space(4.0);
    }

    match state.step {
        AiCharacterStep::SelectImage => {
            render_select_image(ui, state, ctx);
        }
        AiCharacterStep::DetectingSkeleton => {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.spinner();
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Detecting character silhouette & joint landmarks…").size(15.0));
                ui.label(egui::RichText::new("Running BRIA matte & DWPose models").size(12.0).weak());
            });
        }
        AiCharacterStep::JointEditor => {
            render_joint_editor(ui, state, ctx);
        }
        AiCharacterStep::ProcessingParts => {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.spinner();
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Segmenting parts, completing joints & tracing vectors…").size(15.0));
                ui.label(egui::RichText::new("Extracting 20 limb & face parts with uniform line art").size(12.0).weak());
            });
        }
        AiCharacterStep::Preview => {
            render_preview(ui, state, action);
        }
        AiCharacterStep::Completed => {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(egui::RichText::new("✓ Character Assembled & Rigged!").size(18.0).color(Color32::from_rgb(60, 200, 100)));
                ui.add_space(10.0);
                ui.label(format!(
                    "Character '{}' with {} part symbols is saved to the Library and Cast asset folder.",
                    state.character_name,
                    state.assembled.as_ref().map(|a| a.part_symbols.len()).unwrap_or(0)
                ));
                ui.add_space(20.0);
                if ui.button("Close").clicked() {
                    state.open = false;
                }
            });
        }
    }
}

fn render_select_image(ui: &mut Ui, state: &mut AiCharacterState, ctx: &Context) {
    ui.heading("Select Reference Image");
    ui.add_space(8.0);
    ui.label("Choose a character illustration or concept art PNG. BuzzAnimate will isolate the character, detect the humanoid skeleton, complete overlapping joints, and trace vector artwork.");

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label("Character Name:");
        ui.text_edit_singleline(&mut state.character_name);
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Image Path:");
        ui.text_edit_singleline(&mut state.image_path_input);
        let picking = state.rx_pick_file.is_some();
        if ui
            .add_enabled(!picking, egui::Button::new(if picking { "Browsing…" } else { "Browse…" }))
            .clicked()
        {
            // The OS dialog blocks whatever thread calls it; that must not be
            // this one. See `AiCharacterState::rx_pick_file` and `poll`.
            let (tx, rx) = crossbeam_channel::bounded(1);
            state.rx_pick_file = Some(rx);
            thread::spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                    .pick_file();
                let _ = tx.send(picked);
            });
        }
    });

    ui.add_space(16.0);
    let can_start = !state.image_path_input.trim().is_empty();
    if ui.add_enabled(can_start, egui::Button::new("Detect Pose & Skeleton →")).clicked() {
        start_skeleton_detection(state, ctx);
    }
}

/// What the "Detect Pose & Skeleton →" button does. Public so headless UI
/// tests can drive the wizard by calling exactly what the click handler
/// calls, without a real pointer event.
pub fn start_skeleton_detection(state: &mut AiCharacterState, ctx: &Context) {
    let path = PathBuf::from(state.image_path_input.trim());
    if !path.exists() {
        state.status_message = Some((format!("File not found: {:?}", path), true));
        return;
    }

    let Ok(img) = image::open(&path) else {
        state.status_message = Some((format!("Failed to decode image at {:?}", path), true));
        return;
    };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());

    // Upload texture for joint editor preview
    let color_img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    state.image_texture = Some(ctx.load_texture("ai-char-preview", color_img, Default::default()));
    state.image_rgba = Some(Arc::new(rgba.clone()));

    // Infer character name from file stem if default
    if state.character_name == "Character" {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            state.character_name = stem.to_string();
        }
    }

    state.step = AiCharacterStep::DetectingSkeleton;
    state.status_message = None;

    let (tx, rx) = crossbeam_channel::unbounded();
    state.rx_detect = Some(rx);

    thread::spawn(move || {
        let res = detect_skeleton_task(&path, &rgba);
        let _ = tx.send(res);
    });
}

fn detect_skeleton_task(
    path: &Path,
    img: &RgbaImage,
) -> Result<(GrayImage, Skeleton, Vec<(String, Pos2)>), String> {
    // 1. Matte
    let matte = matte(img, None).map_err(|e| format!("Matte segmentation failed: {e}"))?;

    // 2. DWPose or sidecar .joints.json
    let mut detected_person = None;
    match buzz_ai::pose::detect(img) {
        Ok(persons) if !persons.is_empty() => {
            detected_person = Some(persons[0].clone());
        }
        _ => {}
    }

    let skel = if let Some(p) = detected_person {
        skeleton(&p)
    } else {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let sidecar = path.with_file_name(format!("{stem}.joints.json"));
        if sidecar.exists() {
            let s = std::fs::read_to_string(&sidecar).map_err(|e| e.to_string())?;
            let true_joints: Vec<(String, (f64, f64))> =
                serde_json::from_str(&s).map_err(|e| e.to_string())?;
            let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
            for (name, (x, y)) in &true_joints {
                let (xf, yf) = (*x as f32, *y as f32);
                match name.as_str() {
                    "Head" => keypoints[0] = (xf, yf, 0.95),
                    "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
                    "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
                    "Elbow L" => keypoints[7] = (xf, yf, 0.95),
                    "Elbow R" => keypoints[8] = (xf, yf, 0.95),
                    "Hand L" => keypoints[9] = (xf, yf, 0.95),
                    "Hand R" => keypoints[10] = (xf, yf, 0.95),
                    "Thigh L" => keypoints[11] = (xf, yf, 0.95),
                    "Thigh R" => keypoints[12] = (xf, yf, 0.95),
                    "Knee L" => keypoints[13] = (xf, yf, 0.95),
                    "Knee R" => keypoints[14] = (xf, yf, 0.95),
                    "Foot L" => keypoints[15] = (xf, yf, 0.95),
                    "Foot R" => keypoints[16] = (xf, yf, 0.95),
                    _ => {}
                }
            }
            let person = Person {
                bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
                keypoints,
            };
            skeleton(&person)
        } else {
            // Heuristic fallback skeleton
            let w = img.width() as f32;
            let h = img.height() as f32;
            let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
            keypoints[0] = (w * 0.5, h * 0.15, 0.9);
            keypoints[5] = (w * 0.35, h * 0.3, 0.9);
            keypoints[6] = (w * 0.65, h * 0.3, 0.9);
            keypoints[7] = (w * 0.3, h * 0.5, 0.9);
            keypoints[8] = (w * 0.7, h * 0.5, 0.9);
            keypoints[9] = (w * 0.25, h * 0.7, 0.9);
            keypoints[10] = (w * 0.75, h * 0.7, 0.9);
            keypoints[11] = (w * 0.4, h * 0.55, 0.9);
            keypoints[12] = (w * 0.6, h * 0.55, 0.9);
            keypoints[13] = (w * 0.4, h * 0.75, 0.9);
            keypoints[14] = (w * 0.6, h * 0.75, 0.9);
            keypoints[15] = (w * 0.4, h * 0.95, 0.9);
            keypoints[16] = (w * 0.6, h * 0.95, 0.9);
            let person = Person {
                bbox: [0.0, 0.0, w, h],
                keypoints,
            };
            skeleton(&person)
        }
    };

    let mut joints = Vec::new();
    for j in &skel.joints {
        joints.push((j.name.clone(), Pos2::new(j.x, j.y)));
    }

    Ok((matte, skel, joints))
}

fn render_joint_editor(ui: &mut Ui, state: &mut AiCharacterState, ctx: &Context) {
    ui.horizontal(|ui| {
        ui.heading("Joint Editor (Human-in-the-Loop)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Reset Joints").clicked() {
                state.edited_joints = state.original_joints.clone();
            }
        });
    });
    ui.label("Drag any joint marker that was misaligned by the AI model. Accurate joints produce clean limb rotation pivots.");

    ui.add_space(6.0);

    // Interactive Canvas
    let available_size = ui.available_size() - Vec2::new(0.0, 48.0);
    let (response, painter) = ui.allocate_painter(available_size, egui::Sense::drag());

    if let (Some(tex), Some(img)) = (&state.image_texture, &state.image_rgba) {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let scale = (available_size.x / iw).min(available_size.y / ih).max(0.01);
        let canvas_w = iw * scale;
        let canvas_h = ih * scale;
        let offset_x = response.rect.min.x + (available_size.x - canvas_w) * 0.5;
        let offset_y = response.rect.min.y + (available_size.y - canvas_h) * 0.5;
        let img_rect = Rect::from_min_size(Pos2::new(offset_x, offset_y), Vec2::new(canvas_w, canvas_h));

        // Draw image
        painter.image(
            tex.id(),
            img_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        // Helper coordinate conversions
        let to_screen = |p: Pos2| Pos2::new(offset_x + p.x * scale, offset_y + p.y * scale);
        let to_image = |p: Pos2| Pos2::new((p.x - offset_x) / scale, (p.y - offset_y) / scale);

        // Draw bone lines between joints
        let bone_pairs = [
            ("Head", "Neck"),
            ("Neck", "Chest"),
            ("Chest", "Hips"),
            ("Chest", "Shoulder L"),
            ("Shoulder L", "Elbow L"),
            ("Elbow L", "Hand L"),
            ("Chest", "Shoulder R"),
            ("Shoulder R", "Elbow R"),
            ("Elbow R", "Hand R"),
            ("Hips", "Thigh L"),
            ("Thigh L", "Knee L"),
            ("Knee L", "Foot L"),
            ("Hips", "Thigh R"),
            ("Thigh R", "Knee R"),
            ("Knee R", "Foot R"),
        ];

        for (a_name, b_name) in bone_pairs {
            let pos_a = state.edited_joints.iter().find(|(n, _)| n == a_name).map(|(_, p)| to_screen(*p));
            let pos_b = state.edited_joints.iter().find(|(n, _)| n == b_name).map(|(_, p)| to_screen(*p));
            if let (Some(pa), Some(pb)) = (pos_a, pos_b) {
                painter.line_segment([pa, pb], Stroke::new(3.0, Color32::from_rgb(0, 230, 200)));
            }
        }

        // Handle dragging
        let pointer = response.interact_pointer_pos();
        if response.drag_started() {
            if let Some(ptr) = pointer {
                let mut best_dist = 16.0;
                let mut chosen = None;
                for (idx, (_, pt)) in state.edited_joints.iter().enumerate() {
                    let scr = to_screen(*pt);
                    let d = (scr - ptr).length();
                    if d < best_dist {
                        best_dist = d;
                        chosen = Some(idx);
                    }
                }
                state.dragged_joint = chosen;
            }
        }

        if response.dragged() {
            if let (Some(j_idx), Some(ptr)) = (state.dragged_joint, pointer) {
                let img_pt = to_image(ptr);
                state.edited_joints[j_idx].1 = Pos2::new(
                    img_pt.x.clamp(0.0, iw),
                    img_pt.y.clamp(0.0, ih),
                );
            }
        }

        if response.drag_stopped() {
            state.dragged_joint = None;
        }

        // Draw joint handles
        for (idx, (name, pt)) in state.edited_joints.iter().enumerate() {
            let scr = to_screen(*pt);
            let is_dragged = state.dragged_joint == Some(idx);
            let col = if is_dragged {
                Color32::YELLOW
            } else {
                Color32::from_rgb(255, 100, 100)
            };
            painter.circle_filled(scr, 6.0, col);
            painter.circle_stroke(scr, 7.0, Stroke::new(1.5, Color32::BLACK));

            // Show name on hover
            if let Some(ptr) = pointer {
                if (scr - ptr).length() < 12.0 {
                    painter.text(
                        scr + Vec2::new(10.0, -10.0),
                        egui::Align2::LEFT_BOTTOM,
                        name,
                        egui::FontId::proportional(12.0),
                        Color32::WHITE,
                    );
                }
            }
        }
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Next: Segment & Trace Parts →").clicked() {
            start_parts_processing(state, ctx);
        }
    });
}

/// What the "Next: Segment & Trace Parts →" button does. Public for the same
/// reason as [`start_skeleton_detection`].
pub fn start_parts_processing(state: &mut AiCharacterState, _ctx: &Context) {
    let Some(rgba) = state.image_rgba.clone() else { return; };
    let Some(matte) = state.matte.clone() else { return; };
    let Some(detected) = state.detected_skeleton.clone() else { return; };

    // Apply the joint editor's corrections onto the *detected* skeleton,
    // rather than rebuilding one from scratch out of a 133-keypoint array
    // with only 13 slots ever filled in — that used to zero every face
    // landmark and write "Head" into the Nose keypoint. See
    // `apply_edited_joints`.
    let skel = apply_edited_joints(&detected, &state.edited_joints);
    state.skeleton = Some(skel.clone());

    state.step = AiCharacterStep::ProcessingParts;
    state.status_message = None;

    let options = TraceCharacterOptions { uniform_width: state.uniform_width, ..state.trace_options };
    let (tx, rx) = crossbeam_channel::unbounded();
    state.rx_process = Some(rx);

    thread::spawn(move || {
        let _ = tx.send(process_parts(&rgba, &matte, &skel, &options));
    });
}

/// What the preview's **Re-trace** button does: trace the completed parts
/// again with `state.trace_options` (and the stroke width) on a worker
/// thread. The masks and the completion are reused — only the trace runs
/// again — and the result arrives through `rx_process` like the first
/// trace's, so the preview shows it. Public for the same reason as
/// [`start_skeleton_detection`].
pub fn start_retrace(state: &mut AiCharacterState) {
    if state.rx_process.is_some() {
        return;
    }
    let (Some(rgba), Some(matte), Some(skel), Some(parts), Some(completed)) = (
        state.image_rgba.clone(),
        state.matte.clone(),
        state.skeleton.clone(),
        state.parts.clone(),
        state.completed_parts.clone(),
    ) else {
        return;
    };
    let options = TraceCharacterOptions { uniform_width: state.uniform_width, ..state.trace_options };
    state.status_message = None;
    let (tx, rx) = crossbeam_channel::unbounded();
    state.rx_process = Some(rx);

    thread::spawn(move || {
        let traced = trace_character_whole_with(&rgba, &completed, &skel, &matte, &options)
            .map_err(|e| format!("Vector tracing failed: {e}"));
        let _ = tx.send(traced.map(|traced| (parts, completed, traced)));
    });
}

/// The wizard's "Segment & Trace Parts" work: part masks, LaMa completion of
/// the hidden regions, then the equal-width vector trace. Shared by the
/// button and by [`character_from_image`], so the two cannot drift.
fn process_parts(
    rgba: &RgbaImage,
    matte: &GrayImage,
    skel: &Skeleton,
    trace_options: &TraceCharacterOptions,
) -> Result<(PartMasks, CompletedParts, TracedCharacter), String> {
    let parts = part_masks(rgba, matte, skel).map_err(|e| format!("Part masking failed: {e}"))?;
    let completed = complete_parts(rgba, matte, skel, &parts)
        .map_err(|e| format!("Part completion failed: {e}"))?;
    let traced = trace_character_whole_with(rgba, &completed, skel, matte, trace_options)
        .map_err(|e| format!("Vector tracing failed: {e}"))?;
    Ok((parts, completed, traced))
}

/// The models the whole wizard runs on, by the names AI ▸ Models shows.
pub const CHARACTER_MODELS: &[buzz_ai::models::ModelId] = &[
    buzz_ai::models::ModelId::AnimeSegmentation,
    buzz_ai::models::ModelId::DwPoseDetector,
    buzz_ai::models::ModelId::DwPoseEstimator,
    buzz_ai::models::ModelId::Sam2Encoder,
    buzz_ai::models::ModelId::Sam2Decoder,
    buzz_ai::models::ModelId::LaMa,
];

/// Names of the [`CHARACTER_MODELS`] that `ready` says are not installed.
/// Takes the check as a closure so a test needs no model on disk.
pub fn missing_character_models(ready: impl Fn(buzz_ai::models::ModelId) -> bool) -> Vec<&'static str> {
    CHARACTER_MODELS.iter().filter(|m| !ready(**m)).map(|m| m.name()).collect()
}

/// **The whole wizard with its default choices and nobody at the keyboard**:
/// detect the skeleton, accept the joints unedited, segment and complete the
/// parts, trace at the wizard's default line width, assemble. Blocks for as long
/// as the models take: call it from a worker thread. `progress` gets a short
/// note at each step. The caller adds the result to the document
/// (`Editor::create_ai_character`), as **Create Character** does.
///
/// `limits` are the joint limits to rig with (the user's saved
/// `rig_limits.json`); `None` keeps the built-in anatomical ones.
pub fn character_from_image(
    path: &Path,
    name: &str,
    limits: Option<&buzz_rig::JointLimitDefaults>,
    progress: &mut dyn FnMut(&str),
) -> Result<AssembledCharacter, String> {
    let defaults = AiCharacterState::default();
    let options = TraceCharacterOptions { uniform_width: defaults.uniform_width, ..defaults.trace_options };
    character_from_image_with(path, name, &options, limits, progress)
}

/// [`character_from_image`] traced with `trace_options` instead of the
/// wizard's defaults (the examples' `--tuning` / `--bold`).
pub fn character_from_image_with(
    path: &Path,
    name: &str,
    trace_options: &TraceCharacterOptions,
    limits: Option<&buzz_rig::JointLimitDefaults>,
    progress: &mut dyn FnMut(&str),
) -> Result<AssembledCharacter, String> {
    let rgba = image::open(path)
        .map_err(|e| format!("Failed to decode image at {path:?}: {e}"))?
        .to_rgba8();
    progress("Detecting pose and skeleton");
    let (matte, detected, joints) = detect_skeleton_task(path, &rgba)?;
    // The joint editor, accepted as it came: the edited joints are the detected ones.
    let skel = apply_edited_joints(&detected, &joints);
    progress("Segmenting, completing and tracing parts");
    let (_, _, traced) = process_parts(&rgba, &matte, &skel, trace_options)?;
    progress("Assembling the rig");
    assemble_character(&skel, &traced, name, Some((rgba.width() as f64, rgba.height() as f64)), limits)
        .map_err(|e| format!("Assembly failed: {e}"))
}

/// Applies the joint editor's human-in-the-loop corrections onto a clone of
/// the originally *detected* skeleton, by name, instead of rebuilding a
/// skeleton from a fresh `Person`.
///
/// The joint editor only ever shows and moves the 21 named joints
/// `skeleton()` produces (see `detect_skeleton_task`); it does not touch the
/// face landmarks or bones directly. So: every edited joint is written back
/// onto the matching joint by name; the face landmarks are carried along by
/// however far "Nose" (or "Head", if "Nose" was not moved) moved, since they
/// are not editable joints of their own but are detected relative to the
/// same face; and every bone is rebuilt from the resulting joint positions,
/// the same pairing `skeleton()` used to build them the first time — bones
/// are derived state, not independent of the joints.
fn apply_edited_joints(detected: &Skeleton, edited_joints: &[(String, Pos2)]) -> Skeleton {
    let mut skel = detected.clone();

    let nose_before = skel.joint("Nose").map(|j| (j.x, j.y));
    let head_before = skel.joint("Head").map(|j| (j.x, j.y));

    for (name, pos) in edited_joints {
        if let Some(j) = skel.joints.iter_mut().find(|j| &j.name == name) {
            j.x = pos.x;
            j.y = pos.y;
        }
    }

    let anchor_before = nose_before.or(head_before);
    let anchor_after = skel
        .joint("Nose")
        .map(|j| (j.x, j.y))
        .or_else(|| skel.joint("Head").map(|j| (j.x, j.y)));
    if let (Some((bx, by)), Some((ax, ay))) = (anchor_before, anchor_after) {
        let (dx, dy) = (ax - bx, ay - by);
        if dx != 0.0 || dy != 0.0 {
            let shift = |pts: &mut [(f32, f32, f32)]| {
                for p in pts.iter_mut() {
                    p.0 += dx;
                    p.1 += dy;
                }
            };
            shift(&mut skel.face.left_eye);
            shift(&mut skel.face.right_eye);
            shift(&mut skel.face.left_brow);
            shift(&mut skel.face.right_brow);
            shift(&mut skel.face.mouth);
        }
    }

    skel.bones = rebuild_bones(&skel.joints);
    skel
}

/// Rebuilds every bone from the current joint positions, mirroring the head
/// / tail pairing `character::skeleton()` uses when it first builds them.
fn rebuild_bones(joints: &[buzz_ai::character::Joint]) -> Vec<buzz_ai::character::Bone> {
    let at = |name: &str| -> (f32, f32) {
        joints
            .iter()
            .find(|j| j.name == name)
            .map(|j| (j.x, j.y))
            .unwrap_or((0.0, 0.0))
    };
    let conf = |name: &str| -> f32 {
        joints
            .iter()
            .find(|j| j.name == name)
            .map(|j| j.confidence)
            .unwrap_or(0.0)
    };
    let make = |name: &str, head_name: &str, tail_name: &str| -> buzz_ai::character::Bone {
        let head = at(head_name);
        let tail = at(tail_name);
        let dx = tail.0 - head.0;
        let dy = tail.1 - head.1;
        buzz_ai::character::Bone {
            name: name.to_string(),
            head,
            tail,
            confidence: conf(head_name).min(conf(tail_name)),
            length: (dx * dx + dy * dy).sqrt(),
        }
    };
    vec![
        make("Hips", "Hips", "Chest"),
        make("Chest", "Chest", "Neck"),
        make("Head", "Neck", "Head"),
        make("Shoulder L", "Shoulder L", "Elbow L"),
        make("Elbow L", "Elbow L", "Wrist L"),
        make("Hand L", "Wrist L", "Hand L"),
        make("Shoulder R", "Shoulder R", "Elbow R"),
        make("Elbow R", "Elbow R", "Wrist R"),
        make("Hand R", "Wrist R", "Hand R"),
        make("Thigh L", "Thigh L", "Knee L"),
        make("Knee L", "Knee L", "Ankle L"),
        make("Foot L", "Ankle L", "Foot L"),
        make("Thigh R", "Thigh R", "Knee R"),
        make("Knee R", "Knee R", "Ankle R"),
        make("Foot R", "Ankle R", "Foot R"),
    ]
}

fn render_preview(
    ui: &mut Ui,
    state: &mut AiCharacterState,
    action: &mut AiCharacterDialogAction,
) {
    ui.heading("Vector Artwork & Rig Preview");
    ui.add_space(8.0);

    let part_count = state.traced.as_ref().map(|t| t.parts.len()).unwrap_or(0);
    ui.label(format!("Segmented {part_count} body & face parts. Ready to assemble into Graphic symbols and biped armature."));

    ui.add_space(10.0);
    ui.group(|ui| {
        ui.label(egui::RichText::new("Line Art & Color Palette").strong());
        ui.add_space(4.0);

        // A trace takes seconds, so the settings wait for **Re-trace** rather
        // than tracing on every slider move; they are greyed while it runs.
        let tracing = state.rx_process.is_some();
        ui.add_enabled_ui(!tracing, |ui| {
            ui.horizontal(|ui| {
                ui.label("Uniform Stroke Width:");
                let mut uw = state.uniform_width;
                if ui.add(egui::Slider::new(&mut uw, 1.0..=8.0).suffix(" px")).changed() {
                    state.uniform_width = uw;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Bold:");
                ui.add(egui::Slider::new(&mut state.trace_options.trace.ink_bold, -1.0..=3.0).step_by(0.05).suffix(" px"))
                    .on_hover_text("px added to each side of every line; 0 = the source's weight, negative = thinner");
            });
            ui.horizontal(|ui| {
                ui.label("Colours:");
                ui.add(egui::Slider::new(&mut state.trace_options.trace.colours, 2..=64))
                    .on_hover_text("How many colours the fills under the ink are reduced to");
            });
            crate::trace_dialog::tuning_section(ui, &mut state.trace_options.trace.tuning, true);
            if ui
                .button("Re-trace")
                .on_hover_text("Trace the parts again with these settings (takes a few seconds)")
                .clicked()
            {
                start_retrace(state);
            }
        });
        if tracing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Tracing…");
            });
        }

        if let Some(traced) = &state.traced {
            ui.add_space(6.0);
            ui.label("Shared Quantized Palette:");
            ui.horizontal_wrapped(|ui| {
                for c in &traced.palette {
                    let rgba = c.to_rgba8();
                    let color32 = Color32::from_rgb(rgba.r, rgba.g, rgba.b);
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(24.0, 24.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, color32);
                    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
                }
            });
        }
    });

    ui.add_space(10.0);
    ui.group(|ui| {
        ui.label(egui::RichText::new("Face & Expression Rigging").strong());
        ui.add_space(4.0);
        ui.label("• Eyes: Separated eye whites, pupils, and multi-frame eyelids (open, half, closed).");
        ui.label("• Look-At: Pupils move smoothly within eye contour bounds via gaze coordinates.");
        ui.label("• Mouth: Restyled 10-viseme Graphic symbol (Rest, Ai, E, O, U, L, WQ, MBP, FV, Etc) matching character line art.");
        ui.label("• Lip-Sync: Ready for automated phoneme-to-viseme track generation.");
    });

    ui.add_space(10.0);
    crate::rig_panel::rig_kind_row(ui, &mut state.rig_kind);

    ui.add_space(16.0);
    let tracing = state.rx_process.is_some();
    ui.horizontal(|ui| {
        if ui.add_enabled(!tracing, egui::Button::new("← Back to Joints")).clicked() {
            state.step = AiCharacterStep::JointEditor;
        }

        let create_btn = egui::Button::new(
            egui::RichText::new("Create Character in Library & Cast").strong(),
        );
        if ui.add_enabled(!tracing, create_btn).clicked() {
            *action = create_character(state);
        }
    });
}

/// What the "Create Character in Library & Cast" button does. Public for the
/// same reason as [`start_skeleton_detection`].
pub fn create_character(state: &mut AiCharacterState) -> AiCharacterDialogAction {
    let (Some(skel), Some(traced)) = (&state.skeleton, &state.traced) else {
        return AiCharacterDialogAction::None;
    };
    let img_dim = state.image_rgba.as_ref().map(|img| (img.width() as f64, img.height() as f64));
    match assemble_character(skel, traced, &state.character_name, img_dim, state.rig_limits.as_ref()) {
        Ok(assembled) => {
            let boxed = Box::new(assembled);
            state.assembled = Some(boxed.clone());
            state.step = AiCharacterStep::Completed;
            AiCharacterDialogAction::Created(boxed, state.rig_kind)
        }
        Err(err) => {
            state.status_message = Some((format!("Assembly failed: {err}"), true));
            AiCharacterDialogAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ai::character::{FaceFeatures, Joint};

    /// LR2: **Create Character** builds a layer rig unless Bones is picked.
    #[test]
    fn a_fresh_ai_character_dialog_defaults_to_a_layer_rig() {
        assert_eq!(AiCharacterState::default().rig_kind, buzz_rig::RigKind::Layers);
    }

    fn sample_skeleton() -> Skeleton {
        let joint = |name: &str, x: f32, y: f32| Joint {
            name: name.to_string(),
            x,
            y,
            confidence: 0.9,
        };
        let joints = vec![
            joint("Head", 100.0, 10.0),
            joint("Nose", 100.0, 30.0),
            joint("Neck", 100.0, 50.0),
            joint("Chest", 100.0, 90.0),
            joint("Hips", 100.0, 130.0),
            joint("Shoulder L", 70.0, 55.0),
            joint("Elbow L", 60.0, 90.0),
            joint("Wrist L", 55.0, 120.0),
            joint("Hand L", 53.0, 130.0),
            joint("Shoulder R", 130.0, 55.0),
            joint("Elbow R", 140.0, 90.0),
            joint("Wrist R", 145.0, 120.0),
            joint("Hand R", 147.0, 130.0),
            joint("Thigh L", 85.0, 130.0),
            joint("Knee L", 82.0, 180.0),
            joint("Ankle L", 80.0, 230.0),
            joint("Foot L", 78.0, 235.0),
            joint("Thigh R", 115.0, 130.0),
            joint("Knee R", 118.0, 180.0),
            joint("Ankle R", 120.0, 230.0),
            joint("Foot R", 122.0, 235.0),
        ];
        let bones = rebuild_bones(&joints);
        Skeleton {
            joints,
            bones,
            face: FaceFeatures {
                left_eye: vec![(90.0, 25.0, 0.9)],
                right_eye: vec![(110.0, 25.0, 0.9)],
                left_brow: vec![(88.0, 20.0, 0.9)],
                right_brow: vec![(112.0, 20.0, 0.9)],
                mouth: vec![(100.0, 35.0, 0.9)],
            },
            character_height: 240.0,
        }
    }

    fn joints_as_edited(skel: &Skeleton) -> Vec<(String, Pos2)> {
        skel.joints
            .iter()
            .map(|j| (j.name.clone(), Pos2::new(j.x, j.y)))
            .collect()
    }

    /// A missing model is named, as AI ▸ Models names it, so the Assistant can
    /// tell the user which one to download.
    #[test]
    fn missing_character_models_are_named() {
        use buzz_ai::models::ModelId;
        assert!(missing_character_models(|_| true).is_empty());
        let all = missing_character_models(|_| false);
        assert_eq!(all.len(), CHARACTER_MODELS.len());
        let only_lama = missing_character_models(|m| m != ModelId::LaMa);
        assert_eq!(only_lama, vec![ModelId::LaMa.name()]);
    }

    /// **Accepting the detected joints unchanged must leave the skeleton
    /// exactly as detected** — joints, bones, and face landmarks alike.
    /// This is the regression the old 133-keypoint round trip failed even
    /// with no edits at all: it zeroed every face landmark and mislabelled
    /// "Head" as the Nose keypoint regardless of what the user did.
    #[test]
    fn unedited_joints_reproduce_the_detected_skeleton_exactly() {
        let detected = sample_skeleton();
        let edited = joints_as_edited(&detected);
        let rebuilt = apply_edited_joints(&detected, &edited);
        assert_eq!(rebuilt, detected);
    }

    /// **Moving one joint changes only what depends on it** — its own
    /// position and the bones that touch it — and leaves every other joint,
    /// every other bone, and the face landmarks exactly where they were.
    #[test]
    fn moving_one_joint_changes_only_that_joint_and_its_bones() {
        let detected = sample_skeleton();
        let mut edited = joints_as_edited(&detected);
        let idx = edited
            .iter()
            .position(|(n, _)| n == "Elbow L")
            .expect("Elbow L is one of the edited joints");
        edited[idx].1 = Pos2::new(40.0, 95.0);

        let rebuilt = apply_edited_joints(&detected, &edited);

        for (before, after) in detected.joints.iter().zip(rebuilt.joints.iter()) {
            if before.name == "Elbow L" {
                assert_eq!((after.x, after.y), (40.0, 95.0));
            } else {
                assert_eq!(
                    (after.x, after.y),
                    (before.x, before.y),
                    "{} must not move when only Elbow L is edited",
                    before.name
                );
            }
        }

        // Only the bones that touch "Elbow L" — "Shoulder L" (tail) and
        // "Elbow L" (head) itself — are expected to change; "Hand L" runs
        // Wrist L -> Hand L and does not touch the elbow.
        for (before, after) in detected.bones.iter().zip(rebuilt.bones.iter()) {
            let touches_elbow = before.name == "Elbow L" || before.name == "Shoulder L";
            if touches_elbow {
                assert_ne!(after, before, "{} must move with Elbow L", before.name);
            } else {
                assert_eq!(after, before, "{} must not move", before.name);
            }
        }

        assert_eq!(
            rebuilt.face, detected.face,
            "moving a limb joint must not move the face"
        );
    }

    /// Moving "Nose" carries the face landmarks along by the same delta,
    /// since they are detected relative to the face DWPose found, not
    /// recomputed from the edited joints.
    #[test]
    fn moving_the_nose_carries_the_face_landmarks_by_the_same_delta() {
        let detected = sample_skeleton();
        let mut edited = joints_as_edited(&detected);
        let idx = edited
            .iter()
            .position(|(n, _)| n == "Nose")
            .expect("Nose is one of the edited joints");
        let (dx, dy) = (5.0, -8.0);
        edited[idx].1 = Pos2::new(edited[idx].1.x + dx, edited[idx].1.y + dy);

        let rebuilt = apply_edited_joints(&detected, &edited);

        for (before, after) in detected.face.left_eye.iter().zip(rebuilt.face.left_eye.iter()) {
            assert_eq!((after.0, after.1), (before.0 + dx, before.1 + dy));
        }
        for (before, after) in detected.face.mouth.iter().zip(rebuilt.face.mouth.iter()) {
            assert_eq!((after.0, after.1), (before.0 + dx, before.1 + dy));
        }
    }
}
