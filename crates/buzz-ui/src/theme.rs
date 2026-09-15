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
///
/// # Why this is a list rather than a switch
///
/// There were two, and every colour was a two-armed macro: one dark value, one
/// light one, chosen by a `match`. Adding a third would have meant editing
/// twenty of those arms and getting all twenty right. A theme is a *table* of
/// colours now — see [`Colors`] — so adding one is adding a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Theme {
    /// Animate's own default, and this program's.
    #[default]
    Dark,
    /// For bright rooms, and for anybody who simply prefers it.
    Light,
    /// Near-black and cool. For working in the dark, and for the OLED panels
    /// where an almost-black grey is the only thing still drawing power.
    Midnight,
    /// A softer dark: blue-grey, a little lighter than [`Theme::Dark`]. The
    /// contrast between chrome and artwork is lower, which is easier on the
    /// eyes over a long session and worse for picking out fine detail.
    Slate,
    /// Warm paper. The light-table feel, for anyone who finds a grey interface
    /// cold to draw on all day.
    Sepia,
    /// Maximum legibility: black behind white, and vivid guides. For low
    /// vision, for a projector, and for a bright room a normal light theme
    /// cannot cope with.
    Contrast,

    // The four below are the well-known editor palettes rather than this
    // program's own. People arrive already knowing whether they like them, and
    // an animator who has spent a decade in a Dracula editor should not have to
    // leave it behind to draw. Their published colours are used as published;
    // where one is adjusted it is for a job the original palette never had —
    // see `FOREST` and the note on `active`.
    /// Deep green-black, with moss and leaf for the accents.
    Forest,
    /// The Dracula palette: charcoal violet, with purple and pink.
    Dracula,
    /// Ethan Schoonover's Solarized, dark. Blue-green base, low saturation,
    /// tuned so no colour shouts over another.
    SolarizedDark,
    /// Nord: the Arctic palette. Polar-night greys under frost blue.
    Nord,
}

impl Theme {
    /// Every theme, in the order the menu offers them: the two originals
    /// first, then the other darks, then the other lights.
    pub const ALL: [Theme; 10] = [
        Theme::Dark,
        Theme::Light,
        Theme::Midnight,
        Theme::Slate,
        Theme::Sepia,
        Theme::Contrast,
        Theme::Forest,
        Theme::Dracula,
        Theme::SolarizedDark,
        Theme::Nord,
    ];

    /// How many of [`Self::ALL`] are this program's own, before the well-known
    /// palettes begin. The menu draws a separator here: ten in a flat list is
    /// a wall, and the two groups are chosen for different reasons.
    pub const HOUSE: usize = 6;

    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::Midnight => "Midnight",
            Theme::Slate => "Slate",
            Theme::Sepia => "Sepia",
            Theme::Contrast => "High Contrast",
            Theme::Forest => "Forest",
            Theme::Dracula => "Dracula",
            Theme::SolarizedDark => "Solarized Dark",
            Theme::Nord => "Nord",
        }
    }

    /// What the theme is for, for the tooltip beside its name.
    pub fn description(self) -> &'static str {
        match self {
            Theme::Dark => {
                "The default. Neutral grey, so the artwork's own colours read true."
            }
            Theme::Light => "A light interface for a bright room.",
            Theme::Midnight => "Near-black and cool, for working in the dark.",
            Theme::Slate => "A softer blue-grey dark, easier over a long session.",
            Theme::Sepia => "Warm paper, for the light-table feel.",
            Theme::Contrast => {
                "Black behind white, with vivid guides. Built for legibility."
            }
            Theme::Forest => "Deep green-black, with moss and leaf for the accents.",
            Theme::Dracula => "The Dracula palette: charcoal violet, purple and pink.",
            Theme::SolarizedDark => {
                "Solarized, dark. Low saturation, so no colour shouts over another."
            }
            Theme::Nord => "The Arctic palette: polar-night greys under frost blue.",
        }
    }

    /// Whether egui should start from its dark base rather than its light one.
    ///
    /// Every colour that matters is overridden in [`build_visuals`], but the
    /// base decides the handful that are not — shadows, and the tint of a
    /// disabled widget — and starting a dark theme from the light base leaves
    /// those looking washed out against everything around them.
    pub fn is_dark(self) -> bool {
        !matches!(self, Theme::Light | Theme::Sepia)
    }

    /// The next theme along, wrapping. What the Window menu's cycle command
    /// steps through, and what the keyboard shortcut does.
    pub fn next(self) -> Theme {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// The colours this theme answers with.
    pub fn colors(self) -> Colors {
        match self {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
            Theme::Midnight => MIDNIGHT,
            Theme::Slate => SLATE,
            Theme::Sepia => SEPIA,
            Theme::Contrast => CONTRAST,
            Theme::Forest => FOREST,
            Theme::Dracula => DRACULA,
            Theme::SolarizedDark => SOLARIZED_DARK,
            Theme::Nord => NORD,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The theme every colour below is answering for.
pub fn theme() -> Theme {
    let i = CURRENT.load(Ordering::Relaxed) as usize;
    // Clamped rather than wrapped: a preferences file written by a build that
    // had more themes should land on *a* theme, not on an arbitrary one.
    Theme::ALL[i.min(Theme::ALL.len() - 1)]
}

/// Switch themes. Call [`apply`] afterwards to restyle an egui context.
pub fn set_theme(value: Theme) {
    let i = Theme::ALL.iter().position(|t| *t == value).unwrap_or(0);
    CURRENT.store(i as u8, Ordering::Relaxed);
}

/// The current theme's colours.
fn colors() -> Colors {
    theme().colors()
}

/// Panel and chrome colours.
pub struct Palette;

/// A hex literal as an opaque colour, so a palette reads as a list of colours
/// rather than a column of byte triples.
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The same, premultiplied against an alpha. Only the grid needs it: it is
/// drawn *over* the pasteboard rather than instead of it.
const fn rgba(hex: u32, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, a)
}

/// Declare the colours a theme is made of, once.
///
/// This writes both the [`Colors`] field list and the `Palette::name()`
/// accessor that reads it, so the two cannot drift apart — and so the many
/// places that ask for `Palette::panel()` did not have to change when a theme
/// stopped being one of a pair.
macro_rules! palette {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        /// Every colour the chrome asks for, as one table. One of these *is* a
        /// theme.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Colors {
            $($(#[$doc])* pub $name: Color32,)+
        }

        impl Palette {
            $($(#[$doc])* pub fn $name() -> Color32 { colors().$name })+
        }
    };
}

palette!(
    /// Window chrome behind the panels.
    chrome,
    /// Panel interiors.
    panel,
    /// Slightly raised surfaces: toolbar buttons, headers.
    raised,
    /// Hovered control.
    hover,
    /// Active or selected control.
    ///
    /// This program's own themes keep it in the blue family, because an accent
    /// that changes with the interface stops being one. The well-known
    /// palettes do not: Dracula's identity *is* its purple and Forest's is its
    /// green, and holding them to a house rule would leave four themes that
    /// were only nominally the thing they are named after.
    ///
    /// **Dark enough for white text, whatever the family.** It is a fill with
    /// `Color32::WHITE` written on it — see `build_visuals` — so the bright
    /// half of a palette cannot be used here. Nord's accent is nord10 rather
    /// than the frost blue everyone pictures for exactly this reason, and
    /// `the_accent_can_be_written_on` holds every theme to it.
    active,
    /// Separators and panel edges.
    border,
    /// Ordinary text.
    text,
    /// Secondary text.
    text_dim,
    /// The work area surrounding the stage. Objects here are editable but do
    /// not appear in published output.
    ///
    /// **Never the tone of the stage**, in any theme. The pasteboard's job is
    /// to be clearly not the stage, and a white document on a white surround
    /// loses the edge of the frame — which is the one boundary an animator has
    /// to see at all times. Animate keeps it grey in its light theme for the
    /// same reason, and so does every theme here.
    pasteboard,
    /// Edge of the stage rectangle.
    stage_border,
    /// Ruler background.
    ruler_bg,
    /// Ruler markings.
    ruler_tick,
    /// Ruler numbers.
    ruler_text,
    /// Guides dragged off the rulers. Animate uses cyan-green.
    guide,
    /// A guide that cannot be moved.
    guide_locked,
    /// The drawing grid.
    grid,
    /// Selection marquee and outlines.
    selection,
    /// Transform handles and the transformation point.
    handle_fill,
    /// Their outline.
    handle_stroke,
    /// A snap indicator, shown while a drag is snapping to something.
    snap,
);

/// The default. Neutral grey, so the artwork's own colours read true.
const DARK: Colors = Colors {
    chrome: rgb(0x262626),
    panel: rgb(0x2F2F2F),
    raised: rgb(0x3A3A3A),
    hover: rgb(0x4A4A4A),
    active: rgb(0x2D6CB5),
    border: rgb(0x1C1C1C),
    text: rgb(0xD8D8D8),
    text_dim: rgb(0x9A9A9A),
    pasteboard: rgb(0x535353),
    stage_border: rgb(0x1A1A1A),
    ruler_bg: rgb(0x333333),
    ruler_tick: rgb(0x888888),
    ruler_text: rgb(0xAAAAAA),
    guide: rgb(0x00D0C8),
    guide_locked: rgb(0x8A8A60),
    grid: rgba(0x606060, 0x60),
    selection: rgb(0x00A8FF),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x202020),
    snap: rgb(0xFF4080),
};

/// For bright rooms.
const LIGHT: Colors = Colors {
    chrome: rgb(0xD6D6D6),
    panel: rgb(0xEEEEEE),
    raised: rgb(0xDDDDDD),
    hover: rgb(0xC8C8C8),
    active: rgb(0x2D6CB5),
    border: rgb(0xB4B4B4),
    text: rgb(0x1E1E1E),
    text_dim: rgb(0x5A5A5A),
    pasteboard: rgb(0x8E8E8E),
    stage_border: rgb(0x333333),
    ruler_bg: rgb(0xE4E4E4),
    ruler_tick: rgb(0x707070),
    ruler_text: rgb(0x444444),
    guide: rgb(0x009A94),
    guide_locked: rgb(0x8A8A40),
    grid: rgba(0x404040, 0x40),
    selection: rgb(0x0078D4),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x202020),
    snap: rgb(0xD00050),
};

/// Near-black, cool. The panel is not pure black: a true black behind a dark
/// drawing makes the drawing read as a hole rather than as artwork.
const MIDNIGHT: Colors = Colors {
    chrome: rgb(0x0E1116),
    panel: rgb(0x141922),
    raised: rgb(0x1E2530),
    hover: rgb(0x2A3340),
    active: rgb(0x3B82F6),
    border: rgb(0x080A0E),
    text: rgb(0xD5DCE6),
    text_dim: rgb(0x8B95A5),
    pasteboard: rgb(0x39414F),
    stage_border: rgb(0x05070A),
    ruler_bg: rgb(0x171D27),
    ruler_tick: rgb(0x7C879A),
    ruler_text: rgb(0xA3AEBF),
    guide: rgb(0x22D3EE),
    guide_locked: rgb(0x6B7280),
    grid: rgba(0x46505F, 0x60),
    selection: rgb(0x38BDF8),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x0B0E13),
    snap: rgb(0xFB7185),
};

/// A softer dark: blue-grey, and a step lighter than [`DARK`].
const SLATE: Colors = Colors {
    chrome: rgb(0x2B313B),
    panel: rgb(0x353C48),
    raised: rgb(0x424A58),
    hover: rgb(0x515B6B),
    active: rgb(0x4C86C6),
    border: rgb(0x21262E),
    text: rgb(0xDCE1E8),
    text_dim: rgb(0xA2ABB8),
    pasteboard: rgb(0x5C6675),
    stage_border: rgb(0x1B1F26),
    ruler_bg: rgb(0x3A424F),
    ruler_tick: rgb(0x939DAC),
    ruler_text: rgb(0xB4BDC9),
    guide: rgb(0x2FD4C8),
    guide_locked: rgb(0x8A8A60),
    grid: rgba(0x6E7887, 0x60),
    selection: rgb(0x5AB0F5),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x232830),
    snap: rgb(0xFF6088),
};

/// Warm paper. The pasteboard is a deeper tan than the panels, so the stage
/// still reads as a lit sheet lying on a desk.
const SEPIA: Colors = Colors {
    chrome: rgb(0xD8CCB3),
    panel: rgb(0xF2E9D8),
    raised: rgb(0xE6DAC3),
    hover: rgb(0xD3C5A8),
    active: rgb(0x2D6CB5),
    border: rgb(0xBCAE92),
    text: rgb(0x2A2116),
    text_dim: rgb(0x6B5B45),
    pasteboard: rgb(0x9C907A),
    stage_border: rgb(0x3A2E1E),
    ruler_bg: rgb(0xEADFC9),
    ruler_tick: rgb(0x8A7B60),
    ruler_text: rgb(0x4A3E2C),
    guide: rgb(0x00857E),
    guide_locked: rgb(0x8A7A40),
    grid: rgba(0x463C2D, 0x40),
    selection: rgb(0x0078D4),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x2A2116),
    snap: rgb(0xC02050),
};

/// Built for legibility rather than for comfort: black behind white, a visible
/// border on every edge, and guides at full saturation.
const CONTRAST: Colors = Colors {
    chrome: rgb(0x000000),
    panel: rgb(0x000000),
    raised: rgb(0x141414),
    hover: rgb(0x2E2E2E),
    active: rgb(0x0A84FF),
    border: rgb(0xFFFFFF),
    text: rgb(0xFFFFFF),
    text_dim: rgb(0xD0D0D0),
    pasteboard: rgb(0x3A3A3A),
    stage_border: rgb(0xFFFFFF),
    ruler_bg: rgb(0x000000),
    ruler_tick: rgb(0xFFFFFF),
    ruler_text: rgb(0xFFFFFF),
    guide: rgb(0x00FFFF),
    guide_locked: rgb(0xFFFF00),
    grid: rgba(0x969696, 0x90),
    selection: rgb(0x00E5FF),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x000000),
    snap: rgb(0xFF2D95),
};

/// Deep green-black. Not a recognised palette but the one most often asked
/// for alongside them, and the accents are picked from what a forest actually
/// offers: moss, new leaf, and the orange of a fallen one for the snap mark.
const FOREST: Colors = Colors {
    chrome: rgb(0x0B1410),
    panel: rgb(0x101B15),
    raised: rgb(0x17271E),
    hover: rgb(0x21362A),
    active: rgb(0x2F7D53),
    border: rgb(0x060D0A),
    text: rgb(0xD3E3D8),
    text_dim: rgb(0x8AA394),
    pasteboard: rgb(0x33463C),
    stage_border: rgb(0x040907),
    ruler_bg: rgb(0x132018),
    ruler_tick: rgb(0x7C9687),
    ruler_text: rgb(0xA6BFB0),
    guide: rgb(0x3FD9A0),
    guide_locked: rgb(0x7E8A5E),
    grid: rgba(0x4A5F52, 0x60),
    selection: rgb(0x5BD98E),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x08110C),
    snap: rgb(0xFF7A59),
};

/// Dracula, from its published palette: background `#282A36`, foreground
/// `#F8F8F2`, and the six brights.
///
/// The accent is a deeper purple than the palette's `#BD93F9`, because that one
/// is meant to be written *with* rather than written *on* — see `active`. The
/// comment colour `#6272A4` is likewise lifted for the dimmed text, which in an
/// interface has to stay readable rather than recede.
const DRACULA: Colors = Colors {
    chrome: rgb(0x21222C),
    panel: rgb(0x282A36),
    raised: rgb(0x343746),
    hover: rgb(0x44475A),
    active: rgb(0x7E52C9),
    border: rgb(0x191A21),
    text: rgb(0xF8F8F2),
    text_dim: rgb(0x9BA3C9),
    pasteboard: rgb(0x4A4D63),
    stage_border: rgb(0x14151B),
    ruler_bg: rgb(0x2D2F3D),
    ruler_tick: rgb(0x8B93B8),
    ruler_text: rgb(0xBDC3E0),
    guide: rgb(0x8BE9FD),
    guide_locked: rgb(0xF1FA8C),
    grid: rgba(0x6272A4, 0x60),
    selection: rgb(0xFF79C6),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x21222C),
    snap: rgb(0xFF5555),
};

/// Solarized Dark, on its own base tones: `base03` behind, `base02` raised,
/// `base1` for text and `base00` for what recedes.
///
/// The palette is built around a fixed lightness relationship rather than
/// around contrast, which is why its text sits closer to its background than
/// any other theme here. That is the point of it, and it still clears AA.
const SOLARIZED_DARK: Colors = Colors {
    chrome: rgb(0x00212B),
    panel: rgb(0x002B36),
    raised: rgb(0x073642),
    hover: rgb(0x0F4B5C),
    active: rgb(0x268BD2),
    border: rgb(0x00161C),
    text: rgb(0x93A1A1),
    text_dim: rgb(0x657B83),
    pasteboard: rgb(0x3E5B63),
    stage_border: rgb(0x001015),
    ruler_bg: rgb(0x052F3A),
    ruler_tick: rgb(0x587B85),
    ruler_text: rgb(0x93A1A1),
    guide: rgb(0x2AA198),
    guide_locked: rgb(0xB58900),
    grid: rgba(0x586E75, 0x60),
    selection: rgb(0x268BD2),
    handle_fill: rgb(0xFDF6E3),
    handle_stroke: rgb(0x00212B),
    snap: rgb(0xDC322F),
};

/// Nord, by its own numbering: the Polar Night greys `nord0`-`nord3` for the
/// chrome, Snow Storm for the text, Frost for the accents and Aurora for the
/// marks that have to be noticed.
const NORD: Colors = Colors {
    chrome: rgb(0x2E3440),
    panel: rgb(0x3B4252),
    raised: rgb(0x434C5E),
    hover: rgb(0x4C566A),
    active: rgb(0x5E81AC),
    border: rgb(0x242933),
    text: rgb(0xECEFF4),
    text_dim: rgb(0xA3AEC2),
    pasteboard: rgb(0x616E85),
    stage_border: rgb(0x21262F),
    ruler_bg: rgb(0x39404F),
    ruler_tick: rgb(0x92A0B5),
    ruler_text: rgb(0xC3CCDA),
    guide: rgb(0x8FBCBB),
    guide_locked: rgb(0xEBCB8B),
    grid: rgba(0x616E85, 0x60),
    selection: rgb(0x88C0D0),
    handle_fill: rgb(0xECEFF4),
    handle_stroke: rgb(0x2E3440),
    snap: rgb(0xBF616A),
};

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
pub fn brand_at(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (from, to, local) = if t <= 0.5 {
        (BRAND[0], BRAND[1], t * 2.0)
    } else {
        (BRAND[1], BRAND[2], (t - 0.5) * 2.0)
    };
    mix(from, to, local)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub(crate) fn vertex(pos: egui::Pos2, color: Color32) -> egui::epaint::Vertex {
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
    /// How much width a vertical scroll bar takes out of a panel.
    ///
    /// Public because the dock has to subtract it when it works out whether a
    /// column is wide enough for what is in it.
    pub const SCROLL_BAR: f32 = SCROLL_BAR;
}

/// Width of a scroll bar, including the margins either side of it.
///
/// Narrow on purpose: this is width taken away from every panel in a dock
/// column, and the bar is a position indicator far more often than it is a
/// thing anybody drags. Checked against the style in `dock_columns`, so the
/// two cannot drift apart.
const SCROLL_BAR: f32 = 9.0;

/// Apply the theme to an egui context.
///
/// The chosen palette is installed into *both* egui theme slots. Which theme
/// is in use is BuzzAnimate's own setting, not the system's, and leaving the
/// other slot untouched would give a half-styled window to anyone whose system
/// theme disagreed with the one they picked here.
pub fn apply(ctx: &egui::Context) {
    let visuals = build_visuals();
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.interact_size.y = 20.0;

        // **Scroll bars take space; they do not sit on top of the panel.**
        //
        // egui's default bar floats over the content, and every dock column is
        // a scroll area — so the bar was drawn across the right-hand edge of
        // whatever panel was in it. That edge is where the panels keep the
        // controls that were reported missing: the dock menu on every panel
        // header, the Layers panel's new/delete buttons, the Library's own
        // controls. A bar that reserves its width covers nothing.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 1.0;
    });
}

fn build_visuals() -> Visuals {
    let mut visuals = if theme().is_dark() {
        Visuals::dark()
    } else {
        Visuals::light()
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

    /// WCAG relative luminance: gamma-corrected, and weighted for the eye.
    /// Not the same thing as `lum` above, which is a quick perceptual
    /// brightness used for comparing two greys.
    fn relative_luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    /// WCAG contrast ratio between two colours, 1.0 (identical) to 21.0
    /// (black against white).
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Run something in each theme, and leave the theme as it was found.
    ///
    /// The current theme is process-wide, so a test that changed it and walked
    /// away would make its neighbours fail depending on the order they ran in.
    fn in_each_theme(mut body: impl FnMut(Theme)) {
        let was = theme();
        for t in Theme::ALL {
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

    /// Every theme can be set, and reads back as itself.
    ///
    /// The theme is a process-wide index rather than the value, so a variant
    /// added in the middle of `ALL` without its storage being updated would
    /// come back as its neighbour. That is exactly the sort of thing that
    /// shows up as one panel in the wrong colours much later.
    #[test]
    fn every_theme_sets_and_reads_back() {
        let was = theme();
        for t in Theme::ALL {
            set_theme(t);
            assert_eq!(theme(), t, "{} did not read back", t.label());
        }
        set_theme(was);
    }

    /// Stepping lands on every theme and comes back to where it started, so
    /// the cycle cannot strand anyone on a subset of the list.
    #[test]
    fn stepping_visits_every_theme_and_returns() {
        let mut seen = Vec::new();
        let mut t = Theme::Dark;
        for _ in 0..Theme::ALL.len() {
            seen.push(t);
            t = t.next();
        }
        assert_eq!(t, Theme::Dark, "the cycle should return to its start");
        for expected in Theme::ALL {
            assert!(seen.contains(&expected), "{} is not on the cycle", expected.label());
        }
    }

    /// **Every theme has to be legible**, not just the two that were here
    /// first. Four hand-tuned palettes are four chances to leave dim text on
    /// a panel it cannot be read against, and that is not something a compiler
    /// can catch.
    ///
    /// The ratio is WCAG's, and 4.5 is its AA threshold for body text. Dimmed
    /// text is held to 3.0 — it is secondary by design, and holding it to the
    /// body-text bar would just make it a second body text.
    #[test]
    fn every_theme_is_legible() {
        in_each_theme(|t| {
            let panel = Palette::panel();
            for (name, fg, floor) in [
                ("text", Palette::text(), 4.5),
                ("text_dim", Palette::text_dim(), 3.0),
            ] {
                let ratio = contrast(fg, panel);
                assert!(
                    ratio >= floor,
                    "{}: {name} on the panel is {ratio:.2}:1, below {floor}:1",
                    t.label()
                );
            }
        });
    }

    /// **White is written on the accent**, so the accent cannot be one of a
    /// palette's bright colours.
    ///
    /// `build_visuals` fills an active control with `Palette::active` and puts
    /// `Color32::WHITE` on top. Dracula's `#BD93F9` and Nord's frost blue are
    /// the obvious accents to reach for and both fail this: the label goes
    /// nearly invisible the moment a control is pressed. 3.0 is WCAG AA for
    /// text at this weight.
    #[test]
    fn the_accent_can_be_written_on() {
        in_each_theme(|t| {
            let ratio = contrast(Color32::WHITE, Palette::active());
            assert!(
                ratio >= 3.0,
                "{}: white on the accent is {ratio:.2}:1, so a pressed control \
                 cannot be read",
                t.label()
            );
        });
    }

    /// The pasteboard must never be the tone of the stage, or the edge of the
    /// frame disappears — see the note on `Palette::pasteboard`. The stage is
    /// the document's own colour, and white is the one everybody starts with.
    #[test]
    fn the_pasteboard_is_never_mistaken_for_the_stage() {
        in_each_theme(|t| {
            let ratio = contrast(Palette::pasteboard(), Color32::WHITE);
            assert!(
                ratio >= 1.6,
                "{}: the pasteboard is {ratio:.2}:1 against a white stage,                  which loses the edge of the frame",
                t.label()
            );
        });
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
            ("\u{23F7}", "the disclosure arrow, and an open panel"),
            ("\u{270B}", "the hand on the stage zoom control"),
            ("\u{2212}", "the minus on the stage zoom control"),
            ("\u{25B6}", "play, a closed folder, and a rolled-up panel"),
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
            // These two were the panel headers' roll-up triangle, in every
            // dock column, for as long as the dock has existed: a whole
            // window's worth of empty boxes that no test looked at because
            // the characters were never added to the list above.
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
