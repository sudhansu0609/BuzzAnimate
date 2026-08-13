//! Visual theme.
//!
//! Animate ships a dark interface with a mid-grey pasteboard and a white stage,
//! and creative tools generally follow that convention: a neutral surround
//! keeps the artwork's own colours readable. These values are chosen to match
//! that feel without copying Adobe's assets — colours and metrics only, no
//! icons or artwork.
//!
//! # Two themes, one set of names
//!
//! Animate offers a dark and a light interface, and so does this. Every colour
//! is asked for by name — `Palette::panel()`, not a constant — and the answer
//! depends on which theme is current. That keeps the choice in one place: no
//! piece of chrome decides for itself what "the panel colour" is, so no piece
//! of chrome can be left behind in the wrong theme.
//!
//! The current theme is a process-wide atomic rather than something threaded
//! through every drawing call. There is one window, the chrome is drawn on one
//! thread, and passing a theme handle into every painter in the program to
//! express that would be ceremony rather than safety.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

/// Which interface theme is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    /// Animate's own default, and this program's.
    #[default]
    Dark,
    /// For bright rooms, and for anybody who simply prefers it.
    Light,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
        }
    }

    pub fn other(self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The theme every colour below is answering for.
pub fn theme() -> Theme {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Theme::Dark,
        _ => Theme::Light,
    }
}

/// Switch themes. Call [`apply`] afterwards to restyle an egui context.
pub fn set_theme(value: Theme) {
    CURRENT.store(
        match value {
            Theme::Dark => 0,
            Theme::Light => 1,
        },
        Ordering::Relaxed,
    );
}

/// Panel and chrome colours.
pub struct Palette;

/// Pick between the dark and the light value of one colour.
macro_rules! themed {
    ($(#[$doc:meta])* $name:ident, $dark:expr, $light:expr) => {
        $(#[$doc])*
        pub fn $name() -> Color32 {
            match theme() {
                Theme::Dark => $dark,
                Theme::Light => $light,
            }
        }
    };
}

impl Palette {
    themed!(
        /// Window chrome behind the panels.
        chrome,
        Color32::from_rgb(0x26, 0x26, 0x26),
        Color32::from_rgb(0xD6, 0xD6, 0xD6)
    );
    themed!(
        /// Panel interiors.
        panel,
        Color32::from_rgb(0x2F, 0x2F, 0x2F),
        Color32::from_rgb(0xEE, 0xEE, 0xEE)
    );
    themed!(
        /// Slightly raised surfaces: toolbar buttons, headers.
        raised,
        Color32::from_rgb(0x3A, 0x3A, 0x3A),
        Color32::from_rgb(0xDD, 0xDD, 0xDD)
    );
    themed!(
        /// Hovered control.
        hover,
        Color32::from_rgb(0x4A, 0x4A, 0x4A),
        Color32::from_rgb(0xC8, 0xC8, 0xC8)
    );
    themed!(
        /// Active or selected control. The same blue in both themes: it is the
        /// application's accent, and an accent that changes with the theme
        /// stops being one.
        active,
        Color32::from_rgb(0x2D, 0x6C, 0xB5),
        Color32::from_rgb(0x2D, 0x6C, 0xB5)
    );
    themed!(
        /// Separators and panel edges.
        border,
        Color32::from_rgb(0x1C, 0x1C, 0x1C),
        Color32::from_rgb(0xB4, 0xB4, 0xB4)
    );

    themed!(
        /// Ordinary text.
        text,
        Color32::from_rgb(0xD8, 0xD8, 0xD8),
        Color32::from_rgb(0x1E, 0x1E, 0x1E)
    );
    themed!(
        /// Secondary text.
        text_dim,
        Color32::from_rgb(0x9A, 0x9A, 0x9A),
        Color32::from_rgb(0x5A, 0x5A, 0x5A)
    );

    themed!(
        /// The work area surrounding the stage. Objects here are editable but
        /// do not appear in published output.
        ///
        /// Mid-grey in **both** themes, and deliberately: the pasteboard's job
        /// is to be clearly not the stage, and a white document on a white
        /// surround loses the edge of the frame — which is the one boundary an
        /// animator has to see at all times. Animate keeps it grey in its
        /// light theme for the same reason.
        pasteboard,
        Color32::from_rgb(0x53, 0x53, 0x53),
        Color32::from_rgb(0x8E, 0x8E, 0x8E)
    );
    themed!(
        /// Edge of the stage rectangle.
        stage_border,
        Color32::from_rgb(0x1A, 0x1A, 0x1A),
        Color32::from_rgb(0x33, 0x33, 0x33)
    );

    themed!(
        /// Ruler background.
        ruler_bg,
        Color32::from_rgb(0x33, 0x33, 0x33),
        Color32::from_rgb(0xE4, 0xE4, 0xE4)
    );
    themed!(
        /// Ruler markings.
        ruler_tick,
        Color32::from_rgb(0x88, 0x88, 0x88),
        Color32::from_rgb(0x70, 0x70, 0x70)
    );
    themed!(
        /// Ruler numbers.
        ruler_text,
        Color32::from_rgb(0xAA, 0xAA, 0xAA),
        Color32::from_rgb(0x44, 0x44, 0x44)
    );

    themed!(
        /// Guides dragged off the rulers. Animate uses cyan-green.
        guide,
        Color32::from_rgb(0x00, 0xD0, 0xC8),
        Color32::from_rgb(0x00, 0x9A, 0x94)
    );
    themed!(
        /// A guide that cannot be moved.
        guide_locked,
        Color32::from_rgb(0x8A, 0x8A, 0x60),
        Color32::from_rgb(0x8A, 0x8A, 0x40)
    );
    themed!(
        /// The drawing grid.
        grid,
        Color32::from_rgba_premultiplied(0x60, 0x60, 0x60, 0x60),
        Color32::from_rgba_premultiplied(0x40, 0x40, 0x40, 0x40)
    );

    themed!(
        /// Selection marquee and outlines. The same blue in both themes, for
        /// the same reason as [`Palette::active`].
        selection,
        Color32::from_rgb(0x00, 0xA8, 0xFF),
        Color32::from_rgb(0x00, 0x78, 0xD4)
    );
    themed!(
        /// Transform handles and the transformation point.
        handle_fill,
        Color32::from_rgb(0xFF, 0xFF, 0xFF),
        Color32::from_rgb(0xFF, 0xFF, 0xFF)
    );
    themed!(
        /// Their outline.
        handle_stroke,
        Color32::from_rgb(0x20, 0x20, 0x20),
        Color32::from_rgb(0x20, 0x20, 0x20)
    );

    themed!(
        /// A snap indicator, shown while a drag is snapping to something.
        snap,
        Color32::from_rgb(0xFF, 0x40, 0x80),
        Color32::from_rgb(0xD0, 0x00, 0x50)
    );
}

/// The brand's three colours, in order along the frame.
///
/// Orange is the studio's; blue is the accent this program already uses for
/// selection and for an active control; the grey between them is what keeps
/// the two from arguing. Identical in both themes, because a border that
/// changed colour with the interface would stop being a mark.
pub const BRAND: [Color32; 3] = [
    Color32::from_rgb(0xF8, 0x75, 0x1D),
    Color32::from_rgb(0x8A, 0x8F, 0x98),
    Color32::from_rgb(0x1E, 0x7F, 0xD4),
];

/// How thick the banner is, in points.
pub const BANNER_HEIGHT: f32 = 3.0;

/// Paint the brand's gradient as a band across the top of the window.
///
/// # Why only the top
///
/// It was a frame round all four edges first, and it read as a *highlight* —
/// the shape a program uses to say "this window has focus", or worse, "this
/// element is selected". A band along the top is a masthead instead: it says
/// whose program this is, in the one place a title belongs, and nothing at the
/// edge of the artwork competes with the artwork.
///
/// # Why a mesh
///
/// egui paints flat rectangles; a gradient needs a colour per *vertex*, which
/// means a mesh. The band is a strip of quads whose corners are sampled along
/// the run, so the orange at the left travels through grey and arrives blue at
/// the right in one continuous sweep rather than in bands.
pub fn top_banner(ctx: &egui::Context, rect: egui::Rect) {
    if rect.width() < 8.0 {
        return;
    }

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("brand-banner"),
    ));

    let top = rect.top();
    let bottom = top + BANNER_HEIGHT;
    let left = rect.left();
    let width = rect.width();

    // Enough steps that the ramp is smooth on a wide window and cheap on a
    // narrow one.
    let steps = (width / 24.0).clamp(8.0, 160.0) as usize;

    let mut mesh = egui::Mesh::default();
    for step in 0..steps {
        let t0 = step as f32 / steps as f32;
        let t1 = (step + 1) as f32 / steps as f32;
        let x0 = left + width * t0;
        let x1 = left + width * t1;
        let c0 = brand_at(t0);
        let c1 = brand_at(t1);

        let index = mesh.vertices.len() as u32;
        mesh.vertices.push(vertex(egui::pos2(x0, top), c0));
        mesh.vertices.push(vertex(egui::pos2(x0, bottom), c0));
        mesh.vertices.push(vertex(egui::pos2(x1, bottom), c1));
        mesh.vertices.push(vertex(egui::pos2(x1, top), c1));
        mesh.add_triangle(index, index + 1, index + 2);
        mesh.add_triangle(index, index + 2, index + 3);
    }

    painter.add(egui::Shape::mesh(mesh));
}

/// The brand gradient at `t`, which runs `0..=1` across the banner.
///
/// Orange to grey to blue, left to right: the studio's colour, the neutral
/// that keeps the two from arguing, and the accent this program already uses
/// for selection.
fn brand_at(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (from, to, local) = if t <= 0.5 {
        (BRAND[0], BRAND[1], t * 2.0)
    } else {
        (BRAND[1], BRAND[2], (t - 0.5) * 2.0)
    };
    mix(from, to, local)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn vertex(pos: egui::Pos2, color: Color32) -> egui::epaint::Vertex {
    egui::epaint::Vertex {
        pos,
        uv: egui::epaint::WHITE_UV,
        color,
    }
}

/// Metrics shared across the chrome.
pub struct Metrics;

impl Metrics {
    /// Width of the ruler strips, in points.
    pub const RULER: f32 = 18.0;
    /// Toolbar button edge.
    pub const TOOL_BUTTON: f32 = 30.0;
    /// Timeline row height at 100%.
    pub const LAYER_ROW: f32 = 20.0;
    /// Frame cell width in the timeline.
    pub const FRAME_WIDTH: f32 = 12.0;
}

/// Apply the theme to an egui context.
///
/// The same dark palette is installed for *both* egui theme slots. BuzzAnimate
/// is a dark-only application, and leaving the light slot untouched would give
/// a half-styled window to anyone whose system theme is light.
pub fn apply(ctx: &egui::Context) {
    let visuals = build_visuals();
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.interact_size.y = 20.0;
    });
}

fn build_visuals() -> Visuals {
    let mut visuals = match theme() {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };

    visuals.panel_fill = Palette::panel();
    visuals.window_fill = Palette::panel();
    visuals.extreme_bg_color = Palette::chrome();
    visuals.faint_bg_color = Palette::raised();
    visuals.override_text_color = Some(Palette::text());

    visuals.widgets.noninteractive.bg_fill = Palette::panel();
    visuals.widgets.noninteractive.weak_bg_fill = Palette::panel();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Palette::border());
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Palette::text_dim());

    visuals.widgets.inactive.bg_fill = Palette::raised();
    visuals.widgets.inactive.weak_bg_fill = Palette::raised();
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Palette::text());

    visuals.widgets.hovered.bg_fill = Palette::hover();
    visuals.widgets.hovered.weak_bg_fill = Palette::hover();
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.widgets.active.bg_fill = Palette::active();
    visuals.widgets.active.weak_bg_fill = Palette::active();
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.selection.bg_fill = Palette::active();
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    // Square-ish corners read as a professional tool rather than a web app.
    let radius = CornerRadius::same(2);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    visuals.window_corner_radius = radius;

    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f32 {
        0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
    }

    /// Run something in each theme, and leave the theme as it was found.
    ///
    /// The current theme is process-wide, so a test that changed it and walked
    /// away would make its neighbours fail depending on the order they ran in.
    fn in_each_theme(mut body: impl FnMut(Theme)) {
        let was = theme();
        for t in [Theme::Dark, Theme::Light] {
            set_theme(t);
            body(t);
        }
        set_theme(was);
    }

    /// The band runs through all three of the brand's colours, in order.
    #[test]
    fn the_brand_gradient_runs_orange_to_grey_to_blue() {
        assert_eq!(brand_at(0.0), BRAND[0], "orange at the left");
        assert_eq!(brand_at(0.5), BRAND[1], "grey in the middle");
        assert_eq!(brand_at(1.0), BRAND[2], "blue at the right");
    }

    /// No step along it should jump: a gradient that moved in visible bands
    /// would look like a rendering fault rather than a masthead.
    #[test]
    fn the_brand_gradient_is_continuous() {
        let mut previous = brand_at(0.0);
        for i in 1..=200 {
            let c = brand_at(i as f32 / 200.0);
            let step = (c.r() as i32 - previous.r() as i32)
                .abs()
                .max((c.g() as i32 - previous.g() as i32).abs())
                .max((c.b() as i32 - previous.b() as i32).abs());
            assert!(step <= 6, "a jump of {step} at {i}/200");
            previous = c;
        }
    }

    /// The banner paints without panicking at any size, including the absurd
    /// ones a window manager can produce mid-resize.
    #[test]
    fn the_banner_paints_at_any_size() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            for size in [
                egui::vec2(0.0, 0.0),
                egui::vec2(4.0, 4.0),
                egui::vec2(200.0, 40.0),
                egui::vec2(3840.0, 2160.0),
            ] {
                top_banner(
                    ui.ctx(),
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size),
                );
            }
        });
    }

    #[test]
    fn the_theme_applies_to_both_theme_slots() {
        in_each_theme(|_| {
            let ctx = egui::Context::default();
            apply(&ctx);
            for slot in [egui::Theme::Dark, egui::Theme::Light] {
                assert_eq!(
                    ctx.style_of(slot).visuals.panel_fill,
                    Palette::panel(),
                    "{slot:?} slot was left unstyled"
                );
            }
        });
    }

    /// The pasteboard must be clearly distinct from a white stage in **either**
    /// theme, or the boundary of the exported area becomes invisible — which
    /// is the trap a light interface walks straight into.
    #[test]
    fn the_pasteboard_contrasts_with_the_stage_in_both_themes() {
        in_each_theme(|t| {
            let luminance = lum(Palette::pasteboard());
            assert!(
                (40.0..200.0).contains(&luminance),
                "{t:?}: the pasteboard should be mid-grey, got luminance {luminance}"
            );
            assert!(
                (255.0 - luminance) > 40.0,
                "{t:?}: a white stage would disappear into the pasteboard"
            );
        });
    }

    #[test]
    fn text_is_readable_against_panels_in_both_themes() {
        in_each_theme(|t| {
            assert!(
                (lum(Palette::text()) - lum(Palette::panel())).abs() > 80.0,
                "{t:?}: insufficient contrast between text and panel background"
            );
            assert!(
                (lum(Palette::text_dim()) - lum(Palette::panel())).abs() > 40.0,
                "{t:?}: secondary text is too close to the panel behind it"
            );
        });
    }

    /// **The light theme has to be light.** A palette that forgot to invert
    /// would still pass every contrast test above while looking exactly like
    /// the dark one.
    #[test]
    fn the_light_theme_is_lighter_than_the_dark_one() {
        set_theme(Theme::Dark);
        let dark_panel = lum(Palette::panel());
        let dark_text = lum(Palette::text());
        set_theme(Theme::Light);
        let light_panel = lum(Palette::panel());
        let light_text = lum(Palette::text());
        set_theme(Theme::Dark);

        assert!(
            light_panel > dark_panel + 100.0,
            "the light theme's panels are not light: {light_panel} against {dark_panel}"
        );
        assert!(
            light_text < dark_text - 100.0,
            "the light theme's text is not dark: {light_text} against {dark_text}"
        );
    }

    /// Chrome drawn against the rulers has to be readable in both, and the
    /// ruler is the one strip that is neither panel nor stage.
    #[test]
    fn ruler_numbers_are_readable_in_both_themes() {
        in_each_theme(|t| {
            assert!(
                (lum(Palette::ruler_text()) - lum(Palette::ruler_bg())).abs() > 60.0,
                "{t:?}: ruler numbers are too close to the ruler behind them"
            );
        });
    }

    /// Switching is a toggle, and it comes back.
    #[test]
    fn the_theme_switches_and_returns() {
        let was = theme();
        set_theme(Theme::Light);
        assert_eq!(theme(), Theme::Light);
        assert_eq!(Theme::Light.other(), Theme::Dark);
        set_theme(Theme::Dark);
        assert_eq!(theme(), Theme::Dark);
        set_theme(was);
    }
}

/// Does the bundled font actually have this character?
///
/// # Why this exists
///
/// egui ships a small subset of Unicode, and a character it lacks is drawn as
/// an empty box. This project has shipped that twice — `▼` in the Library and
/// again in the Actions panel — and no test caught either, because a missing
/// glyph is still a valid string. Anything that puts a symbol on screen can
/// now assert it will be *drawn*, which is the difference between a check and
/// a hope.
///
/// # How it can tell
///
/// egui does not report a missing glyph — it quietly substitutes the *same*
/// replacement box for every one, and lays that out with a perfectly ordinary
/// width. So this measures a character nothing on earth bundles, and treats
/// anything that comes out exactly that wide as missing too. It is a
/// fingerprint rather than a lookup, and it is honest about that; the cost of
/// being wrong is being told to choose a different symbol.
pub fn font_has(ctx: &egui::Context, text: &str) -> bool {
    let width = |c: char| {
        let font = egui::FontId::proportional(14.0);
        ctx.fonts_mut(|fonts| fonts.glyph_width(&font, c))
    };
    // An Egyptian hieroglyph: not in egui's bundled subset, and not going to be.
    let tofu = width('\u{13000}');

    text.chars().all(|c| {
        let w = width(c);
        w > 0.0 && (w - tofu).abs() > f32::EPSILON
    })
}

#[cfg(test)]
mod glyph_tests {
    use super::*;

    /// Every symbol this program draws as text must exist in the bundled font.
    ///
    /// The list is the point: adding a character to the interface means adding
    /// it here, and finding out in a second rather than in a screenshot. Two
    /// missing glyphs have shipped in this project already.
    #[test]
    fn every_symbol_in_the_interface_has_a_glyph() {
        let ctx = egui::Context::default();
        apply(&ctx);
        // Build the font atlas before asking it anything.
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.label("warm the atlas");
        });

        let mut missing = Vec::new();
        for (symbol, what) in [
            ("\u{00B0}", "degrees"),
            ("\u{00B7}", "the separator dot"),
            ("\u{00D7}", "the multiplication sign in the HUD"),
            ("\u{2013}", "an en dash"),
            ("\u{2014}", "an em dash"),
            ("\u{2022}", "a bullet"),
            ("\u{2026}", "an ellipsis"),
            ("\u{23F7}", "the disclosure arrow"),
            ("\u{270B}", "the hand on the stage zoom control"),
            ("\u{2212}", "the minus on the stage zoom control"),
            ("\u{25B6}", "play, and a closed folder"),
            ("\u{25C0}", "step back"),
            ("\u{25D1}", "the gradient tool"),
            ("\u{2606}", "the star tool"),
            ("\u{2714}", "the menu tick"),
            ("\u{2795}", "add"),
            ("\u{1F50D}", "the library search"),
            ("\u{1F512}", "a locked layer"),
            ("\u{1F513}", "an unlocked layer"),
            ("\u{1F5D1}", "delete a layer"),
        ] {
            if !font_has(&ctx, symbol) {
                missing.push(format!("{what} ({symbol:?})"));
            }
        }

        // Reported together: finding these one character at a time, rebuilding
        // between each, is how an afternoon disappears.
        assert!(
            missing.is_empty(),
            "these would draw as empty boxes: {}",
            missing.join(", ")
        );
    }

    /// The characters this project has been caught out by, kept as a record
    /// so nobody reaches for them again.
    #[test]
    fn the_symbols_this_font_lacks_are_still_missing() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.label("warm the atlas");
        });

        for (symbol, what) in [
            ("\u{2261}", "the hamburger menu"),
            ("\u{2715}", "a multiplication sign, for delete"),
            ("\u{25A2}", "a rounded square, for outline view"),
            ("\u{21B3}", "a hierarchy arrow, for a child bone"),
            ("\u{25B8}", "a small right triangle, for menu paths"),
            ("\u{25BC}", "a filled down triangle"),
            ("\u{25BE}", "a small down triangle, for a dropdown"),
        ] {
            assert!(
                !font_has(&ctx, symbol),
                "{what} ({symbol:?}) renders after all: it could be used again"
            );
        }
    }

    /// And the check itself has to be able to fail, or it proves nothing.
    #[test]
    fn a_character_the_font_lacks_is_reported_as_missing() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.label("warm the atlas");
        });

        // A glyph from a script egui does not bundle.
        assert!(
            !font_has(&ctx, "\u{13000}"),
            "the check passes things the font does not have"
        );
    }
}
