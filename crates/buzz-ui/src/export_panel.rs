//! The Export dialog.
//!
//! Animate's Export Image asks three things — how large, with or without a
//! background, and (for a sequence) which frames. This asks the same three and
//! nothing else. The state is separated from the drawing so the arithmetic
//! that matters — keeping the aspect ratio, clamping a range to the timeline —
//! can be tested without a window.

use egui::{RichText, Ui};

/// Which of Animate's two export commands is being set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// One frame, as a PNG.
    Image,
    /// A numbered PNG per frame.
    Sequence,
    /// An MP4 or MOV. CP-6.2.
    Video,
    /// An animated GIF. CP-6.3.
    Gif,
    /// An animated WebP. CP-6.3.
    Webp,
}

impl ExportKind {
    pub fn title(self) -> &'static str {
        match self {
            ExportKind::Image => "Export Image",
            ExportKind::Sequence => "Export PNG Sequence",
            ExportKind::Video => "Export Video",
            ExportKind::Gif => "Export GIF",
            ExportKind::Webp => "Export WebP",
        }
    }

    /// Does this export cover a range of frames rather than one?
    pub fn is_range(self) -> bool {
        matches!(self, Self::Sequence | Self::Video | Self::Gif | Self::Webp)
    }

    /// Does this format need an ffmpeg to encode?
    pub fn needs_ffmpeg(self) -> bool {
        matches!(self, Self::Video | Self::Gif | Self::Webp)
    }
}

/// Everything the dialog remembers.
///
/// View state: an export setting is not part of the artwork, is not saved with
/// it and is not undone.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportState {
    /// Open, and for which command.
    pub open: Option<ExportKind>,
    pub width: u32,
    pub height: u32,
    /// Export with no background.
    pub transparent: bool,
    /// Render only the selection's bounds (a render region) rather than the
    /// whole stage — for re-rendering one corner without redoing the frame.
    pub region_to_selection: bool,
    /// Keep the stage's proportions when either size is edited.
    pub link_aspect: bool,
    /// First and last frame of a sequence, inclusive.
    pub from_frame: u32,
    pub to_frame: u32,
    /// The stage size the sizes were derived from, so the ratio survives.
    stage: (u32, u32),
    /// How long the film is, remembered from the last `open`. See
    /// [`Self::frame_count`].
    frames: u32,
    /// A run in progress: frames done, frames total.
    pub progress: Option<(u32, u32)>,
    /// Video settings. Remembered between exports, unlike the size — a codec
    /// choice is a preference about the machine, not about the document, so
    /// resetting it every time would be an annoyance rather than a safeguard.
    pub video: VideoOptions,
    /// GIF settings, remembered for the same reason.
    pub gif: GifOptions,
    /// Animated-WebP settings.
    pub webp: WebpOptions,
    /// Whether this machine has an ffmpeg to encode with, checked when the
    /// dialog opens rather than when Export is pressed.
    pub ffmpeg: bool,
    /// The name being typed for "save these settings as a preset".
    pub preset_name: String,
    /// **Which preset the settings currently came from**, by name.
    ///
    /// By name rather than by index: the list is the built-ins followed by the
    /// user's own, so saving or removing one shifts every index after it and a
    /// remembered number would start pointing at a different preset.
    pub selected_preset: Option<String>,
}

/// The video choices the dialog offers.
///
/// A plain mirror of `buzz_export::VideoSettings`, kept here so `buzz-ui` does
/// not depend on the exporter — the same separation every other panel has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoOptions {
    pub codec: VideoChoice,
    pub container: ContainerChoice,
    pub quality: u32,
    pub hardware: bool,
    pub audio: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            codec: VideoChoice::H264,
            container: ContainerChoice::Mp4,
            quality: 20,
            hardware: true,
            audio: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoChoice {
    H264,
    Hevc,
    Av1,
    ProRes4444,
}

impl VideoChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC (H.265)",
            Self::Av1 => "AV1",
            Self::ProRes4444 => "ProRes 4444 (alpha)",
        }
    }

    /// What this codec is for, in one line — the difference matters and is not
    /// guessable from the name.
    pub fn help(self) -> &'static str {
        match self {
            Self::H264 => "Plays everywhere. Choose this unless you have a reason not to.",
            Self::Hevc => "Smaller at the same quality; not every player opens it.",
            Self::Av1 => "Smaller again, and needs a recent player.",
            Self::ProRes4444 => {
                "Keeps a real alpha channel for compositing. Large .mov files."
            }
        }
    }

    /// ProRes carries transparency; the export renders on no background.
    pub fn is_alpha(self) -> bool {
        matches!(self, Self::ProRes4444)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerChoice {
    Mp4,
    Mov,
}

impl ContainerChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }
}

/// The GIF choices the dialog offers. A plain mirror of
/// `buzz_export::GifSettings`, kept here so `buzz-ui` need not depend on the
/// exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifOptions {
    pub dither: DitherChoice,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            dither: DitherChoice::Bayer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DitherChoice {
    None,
    Bayer,
    FloydSteinberg,
}

impl DitherChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Bayer => "Ordered (steady)",
            Self::FloydSteinberg => "Diffusion (smoother, shimmers)",
        }
    }
}

/// The animated-WebP choices. Mirror of `buzz_export::WebpSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebpOptions {
    pub quality: u32,
    pub lossless: bool,
}

impl Default for WebpOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            lossless: false,
        }
    }
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            open: None,
            width: 550,
            height: 400,
            transparent: false,
            region_to_selection: false,
            link_aspect: true,
            from_frame: 0,
            to_frame: 0,
            stage: (550, 400),
            frames: 1,
            progress: None,
            video: VideoOptions::default(),
            gif: GifOptions::default(),
            webp: WebpOptions::default(),
            ffmpeg: true,
            preset_name: String::new(),
            selected_preset: None,
        }
    }
}

impl ExportState {
    /// Open the dialog, sized to this document.
    ///
    /// The size is reset to the stage each time rather than remembered: an
    /// export at yesterday's dimensions for a document that has since been
    /// resized is a silent way to produce the wrong file.
    pub fn open(&mut self, kind: ExportKind, stage: (u32, u32), frame_count: u32) {
        self.open = Some(kind);
        self.frames = frame_count.max(1);
        self.stage = (stage.0.max(1), stage.1.max(1));
        self.width = self.stage.0;
        self.height = self.stage.1;
        self.from_frame = 0;
        self.to_frame = frame_count.saturating_sub(1);
        self.progress = None;
        // The size has just been reset to the stage, so whatever preset was
        // showing no longer describes these settings.
        self.selected_preset = None;
    }

    /// The frames this document has, remembered from the last `open`.
    ///
    /// A preset can change the export from one frame to a whole film —
    /// "GIF preview" chosen while Export Image was open — and the range
    /// fields were then still whatever the single-frame export left them at,
    /// which is frame zero to frame zero. One frame of GIF.
    pub fn frame_count(&self) -> u32 {
        self.frames
    }

    /// Everything a preset sets, as one comparable value.
    ///
    /// The frame range is deliberately not in it: a preset does not carry one,
    /// so narrowing the range does not stop the settings being that preset.
    fn fingerprint(&self) -> (Option<ExportKind>, u32, u32, bool, VideoOptions, GifOptions, WebpOptions) {
        (
            self.open,
            self.width,
            self.height,
            self.transparent,
            self.video,
            self.gif,
            self.webp,
        )
    }

    pub fn close(&mut self) {
        self.open = None;
        self.progress = None;
    }

    /// Scale relative to the stage, for the "200%" readout.
    pub fn scale(&self) -> f64 {
        self.width as f64 / self.stage.0 as f64
    }

    /// Set the width, carrying the height with it when linked.
    pub fn set_width(&mut self, width: u32) {
        let width = width.clamp(1, MAX_SIDE);
        if self.link_aspect {
            let ratio = self.stage.1 as f64 / self.stage.0 as f64;
            self.height = ((width as f64 * ratio).round() as u32).clamp(1, MAX_SIDE);
        }
        self.width = width;
    }

    /// Set the height, carrying the width with it when linked.
    pub fn set_height(&mut self, height: u32) {
        let height = height.clamp(1, MAX_SIDE);
        if self.link_aspect {
            let ratio = self.stage.0 as f64 / self.stage.1 as f64;
            self.width = ((height as f64 * ratio).round() as u32).clamp(1, MAX_SIDE);
        }
        self.height = height;
    }

    /// Set both from a multiple of the stage size.
    pub fn set_scale(&mut self, factor: f64) {
        let scale = |v: u32| ((v as f64 * factor).round() as u32).clamp(1, MAX_SIDE);
        self.width = scale(self.stage.0);
        self.height = scale(self.stage.1);
    }

    /// The frame range, as a half-open range ready for the exporter.
    ///
    /// Ordered, so a range typed backwards exports those frames rather than
    /// nothing at all.
    pub fn range(&self) -> std::ops::Range<u32> {
        let first = self.from_frame.min(self.to_frame);
        let last = self.from_frame.max(self.to_frame);
        first..last + 1
    }
}

/// Nothing sensible comes of a side longer than this, and a GPU will refuse it
/// anyway. Bounded here so the field cannot be typed into an allocation that
/// fails much later and much less clearly.
const MAX_SIDE: u32 = 16_384;

/// What the user did in the dialog.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExportResponse {
    /// Go ahead: pick a destination and export.
    pub confirmed: bool,
    /// Close without exporting.
    pub cancelled: bool,
    /// Apply the preset at this index in the list passed in.
    pub apply_preset: Option<usize>,
    /// Save the current settings as a preset under `preset_name`.
    pub save_preset: bool,
}

/// Draw the dialog. Returns what the user chose.
///
/// `presets` are the names to offer, built-ins first; the shell owns the list
/// and acts on [`ExportResponse::apply_preset`] and friends.
pub fn export_dialog(
    ctx: &egui::Context,
    state: &mut ExportState,
    presets: &[String],
) -> ExportResponse {
    let mut response = ExportResponse::default();
    let Some(kind) = state.open else {
        return response;
    };

    let mut still_open = true;
    egui::Window::new(kind.title())
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            if let Some((done, total)) = state.progress {
                progress_view(ui, done, total, &mut response);
                return;
            }
            preset_view(ui, state, presets, &mut response);
            // **Edit a setting by hand and it is no longer that preset.**
            //
            // Otherwise the box goes on naming "YouTube 1080p" over a height
            // the user has since changed, and the one thing it is there to say
            // \u2014 what these settings are \u2014 is wrong. Compared rather than
            // hooked into every control: there are a dozen of them and a
            // forgotten one would be this bug again.
            let before = state.fingerprint();
            settings_view(ui, kind, state, &mut response);
            if state.fingerprint() != before {
                state.selected_preset = None;
            }
        });

    // The window's own close button counts as cancelling.
    if !still_open {
        response.cancelled = true;
    }
    if response.cancelled {
        state.close();
    }
    response
}

/// The preset row at the top of the dialog: pick one to fill the settings, or
/// save the settings as a new one.
fn preset_view(
    ui: &mut Ui,
    state: &mut ExportState,
    presets: &[String],
    response: &mut ExportResponse,
) {
    ui.horizontal(|ui| {
        ui.label("Preset");
        // **What is showing is what is selected.** Every entry used to be drawn
        // unselected and the box always read "Choose", so choosing one gave no
        // sign that anything had happened: the settings below did change, but
        // nothing said which preset they had come from, and opening the box
        // again showed nothing ticked either. A preset that is applied and then
        // immediately forgotten by the only part of the dialog that could show
        // it is a preset that, as far as the user can tell, was never selected.
        //
        // A remembered name no longer in the list \u2014 a user preset since
        // removed \u2014 falls back to the prompt rather than naming something
        // that is not there.
        let shown = state
            .selected_preset
            .as_deref()
            .filter(|name| presets.iter().any(|p| p == name))
            .unwrap_or("Choose\u{2026}");
        egui::ComboBox::from_id_salt("export-preset")
            .selected_text(shown)
            .width(220.0)
            .show_ui(ui, |ui| {
                for (i, name) in presets.iter().enumerate() {
                    let current = state.selected_preset.as_deref() == Some(name.as_str());
                    if ui.selectable_label(current, name).clicked() {
                        response.apply_preset = Some(i);
                    }
                }
            });
        if state.selected_preset.is_some()
            && ui
                .small_button("\u{2715}")
                .on_hover_text("Forget the preset; keep these settings")
                .clicked()
        {
            state.selected_preset = None;
        }
    });

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.preset_name)
                .hint_text("Name these settings")
                .desired_width(160.0),
        );
        if ui.button("Save preset").clicked() {
            response.save_preset = true;
        }
    });

    ui.add_space(4.0);
    ui.separator();
}

fn settings_view(
    ui: &mut Ui,
    kind: ExportKind,
    state: &mut ExportState,
    response: &mut ExportResponse,
) {
    egui::Grid::new("export-size")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Width");
            let mut width = state.width;
            if ui
                .add(
                    egui::DragValue::new(&mut width)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                )
                .changed()
            {
                state.set_width(width);
            }
            ui.end_row();

            ui.label("Height");
            let mut height = state.height;
            if ui
                .add(
                    egui::DragValue::new(&mut height)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                )
                .changed()
            {
                state.set_height(height);
            }
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut state.link_aspect, "Keep proportions");
            ui.end_row();
        });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Scale").small().weak());
        for factor in [0.5, 1.0, 2.0, 4.0] {
            let label = format!("{}%", (factor * 100.0) as i32);
            if ui.small_button(label).clicked() {
                state.set_scale(factor);
            }
        }
        ui.label(
            RichText::new(format!("{:.0}%", state.scale() * 100.0))
                .small()
                .weak(),
        );
    });

    ui.add_space(4.0);
    ui.checkbox(&mut state.transparent, "Transparent background")
        .on_hover_text("Leave the stage colour out, so the artwork can be composited elsewhere");
    ui.checkbox(&mut state.region_to_selection, "Render only the selection")
        .on_hover_text(
            "Render just the selection's bounds, scaled to the export size — a render region \
             for re-doing one corner. Ignored with nothing selected.",
        );

    if kind.needs_ffmpeg() && !state.ffmpeg {
        ui.add_space(4.0);
        ui.separator();
        ui.colored_label(
            egui::Color32::from_rgb(220, 120, 90),
            "No ffmpeg found on this machine.",
        );
        ui.label(
            RichText::new("This format needs one. On Windows: winget install Gyan.FFmpeg")
                .small()
                .weak(),
        );
    }

    if kind == ExportKind::Gif {
        ui.add_space(4.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Dithering");
            egui::ComboBox::from_id_salt("gif-dither")
                .selected_text(state.gif.dither.label())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for d in [
                        DitherChoice::Bayer,
                        DitherChoice::FloydSteinberg,
                        DitherChoice::None,
                    ] {
                        ui.selectable_value(&mut state.gif.dither, d, d.label());
                    }
                });
        });
        ui.label(
            RichText::new("A GIF has 256 colours; dithering trades pattern for banding.")
                .small()
                .weak(),
        );
    }

    if kind == ExportKind::Webp {
        ui.add_space(4.0);
        ui.separator();
        ui.checkbox(&mut state.webp.lossless, "Lossless")
            .on_hover_text("Exact pixels, larger file. Suits flat vector artwork.");
        if !state.webp.lossless {
            ui.horizontal(|ui| {
                ui.label("Quality");
                ui.add(egui::Slider::new(&mut state.webp.quality, 10..=100).show_value(false))
                    .on_hover_text("Higher is better and larger");
            });
        }
    }

    if kind == ExportKind::Video {
        ui.add_space(4.0);
        ui.separator();

        egui::Grid::new("export-video")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Format");
                egui::ComboBox::from_id_salt("container")
                    .selected_text(state.video.container.label())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for c in [ContainerChoice::Mp4, ContainerChoice::Mov] {
                            ui.selectable_value(&mut state.video.container, c, c.label());
                        }
                    });
                ui.end_row();

                ui.label("Codec");
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(state.video.codec.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for c in [
                            VideoChoice::H264,
                            VideoChoice::Hevc,
                            VideoChoice::Av1,
                            VideoChoice::ProRes4444,
                        ] {
                            ui.selectable_value(&mut state.video.codec, c, c.label())
                                .on_hover_text(c.help());
                        }
                    });
                ui.end_row();

                // ProRes only lives in a .mov, so the container follows the codec.
                if state.video.codec.is_alpha() {
                    state.video.container = ContainerChoice::Mov;
                }

                ui.label("Quality");
                // ffmpeg's scale runs backwards — lower is better — which is
                // the opposite of what a slider labelled "quality" implies. So
                // the slider is shown the way round a user expects and
                // inverted here, rather than asking them to learn ffmpeg's.
                let mut quality = 51u32.saturating_sub(state.video.quality);
                if ui
                    .add(egui::Slider::new(&mut quality, 10..=45).show_value(false))
                    .on_hover_text("Higher is better and larger")
                    .changed()
                {
                    state.video.quality = 51u32.saturating_sub(quality);
                }
                ui.end_row();
            });

        ui.label(RichText::new(state.video.codec.help()).small().weak());
        ui.checkbox(&mut state.video.hardware, "Encode on the GPU")
            .on_hover_text("Use NVENC where the machine has it. Falls back to the CPU otherwise.");
        ui.checkbox(&mut state.video.audio, "Include the soundtrack");
    }

    if kind.is_range() {
        ui.add_space(4.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut state.from_frame).range(0..=u32::MAX));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut state.to_frame).range(0..=u32::MAX));
            let count = state.range().len();
            let what = if kind == ExportKind::Sequence {
                format!("({count} files)")
            } else {
                format!("({count} frames)")
            };
            ui.label(RichText::new(what).small().weak());
        });
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Export…").clicked() {
            response.confirmed = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn progress_view(ui: &mut Ui, done: u32, total: u32, response: &mut ExportResponse) {
    let fraction = if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    };
    ui.label(format!("Exporting frame {done} of {total}"));
    ui.add(egui::ProgressBar::new(fraction).show_percentage());
    ui.add_space(6.0);
    if ui.button("Stop").clicked() {
        // Stopping keeps what has already been written: half a sequence is
        // usually still worth having, and deleting the user's files because
        // they changed their mind would be worse than leaving them.
        response.cancelled = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ExportState {
        let mut s = ExportState::default();
        s.open(ExportKind::Image, (550, 400), 10);
        s
    }

    #[test]
    fn opening_sizes_the_dialog_to_the_document() {
        let s = state();
        assert_eq!((s.width, s.height), (550, 400));
        assert_eq!(s.open, Some(ExportKind::Image));
        assert_eq!(s.to_frame, 9, "the last frame, not the count");
    }

    /// A document resized since the last export must not be exported at the
    /// old dimensions.
    #[test]
    fn reopening_takes_the_current_stage_size() {
        let mut s = state();
        s.set_scale(4.0);
        assert_eq!(s.width, 2200);

        s.open(ExportKind::Image, (1920, 1080), 5);
        assert_eq!((s.width, s.height), (1920, 1080));
    }

    #[test]
    fn linked_sizes_keep_the_stage_proportions() {
        let mut s = state();
        s.set_width(1100);
        assert_eq!(s.height, 800, "height should follow");

        s.set_height(200);
        assert_eq!(s.width, 275, "and width should follow back");
    }

    #[test]
    fn unlinked_sizes_move_independently() {
        let mut s = state();
        s.link_aspect = false;
        s.set_width(1000);
        assert_eq!(s.height, 400, "height should not have moved");
    }

    #[test]
    fn scale_buttons_set_both_sides_from_the_stage() {
        let mut s = state();
        s.set_scale(2.0);
        assert_eq!((s.width, s.height), (1100, 800));
        assert_eq!(s.scale(), 2.0);

        s.set_scale(0.5);
        assert_eq!((s.width, s.height), (275, 200));
    }

    /// Absurd sizes are refused at the field rather than at the GPU, where the
    /// failure would be an allocation error a long way from the cause.
    #[test]
    fn sizes_are_bounded() {
        let mut s = state();
        s.set_width(10_000_000);
        assert_eq!(s.width, MAX_SIDE);
        s.link_aspect = false;
        s.set_height(0);
        assert_eq!(s.height, 1);
    }

    #[test]
    fn a_frame_range_is_inclusive_and_survives_being_typed_backwards() {
        let mut s = state();
        s.from_frame = 2;
        s.to_frame = 5;
        assert_eq!(s.range(), 2..6);
        assert_eq!(s.range().len(), 4);

        s.from_frame = 5;
        s.to_frame = 2;
        assert_eq!(
            s.range(),
            2..6,
            "a backwards range still exports those frames"
        );

        s.from_frame = 3;
        s.to_frame = 3;
        assert_eq!(s.range(), 3..4, "a single frame is one file");
    }

    #[test]
    fn closing_clears_any_progress() {
        let mut s = state();
        s.progress = Some((3, 10));
        s.close();
        assert!(s.open.is_none());
        assert!(s.progress.is_none());
    }
}
