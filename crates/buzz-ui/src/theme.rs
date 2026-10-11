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
            Theme::Dark => "The default. Neutral grey, so the artwork's own colours read true.",
            Theme::Light => "A light interface for a bright room.",
            Theme::Midnight => "Near-black and cool, for working in the dark.",
            Theme::Slate => "A softer blue-grey dark, easier over a long session.",
            Theme::Sepia => "Warm paper, for the light-table feel.",
            Theme::Contrast => "Black behind white, with vivid guides. Built for legibility.",
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

    /// The colours this theme answers with — derived once, from its seed;
    /// see [`derive`] and [`colors_table`].
    pub fn colors(self) -> Colors {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        colors_table()[i]
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
    /// Window background behind every panel.
    bg,
    /// Panel interiors.
    panel,
    /// A raised field inside a panel: a text box, a swatch well.
    field,
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
    /// The line **between** panels — dock columns, sections. Recedes rather
    /// than rises: a groove, not a rule.
    hairline,
    /// A line **inside** a panel — a disclosure row, a section separator.
    /// Softer than [`Colors::hairline`]: it reads as structure within
    /// something rather than the edge of it.
    divider,
    /// Ordinary text.
    text,
    /// Secondary text.
    text_dim,
    /// The one colour used for a tab's underline, the active tool, and focus
    /// — see the Decisions in `docs/WORKSPACE_DESIGN_PLAN.md`. Everything
    /// else in the chrome is grey.
    accent,
    /// Text written on a fill of [`Colors::accent`] — whichever of black or
    /// white actually reads against it; not every accent is dark enough for
    /// white, see [`Colors::active`].
    accent_text,
    /// The work area surrounding the stage. Objects here are editable but do
    /// not appear in published output.
    ///
    /// **Never the tone of the stage**, in any theme. The pasteboard's job is
    /// to be clearly not the stage, and a white document on a white surround
    /// loses the edge of the frame — which is the one boundary an animator has
    /// to see at all times. Animate keeps it grey in its light theme for the
    /// same reason, and so does every theme here.
    ///
    /// **This is the "match theme" answer, not necessarily what gets
    /// painted.** View > Pasteboard Colour… lets a user override it; that
    /// override lives in `Workspace::pasteboard`, not here — `Palette` only
    /// ever knows the current *theme*, not the current *workspace*. Whatever
    /// paints the pasteboard (`buzz-app`'s stage view) checks the override
    /// first and falls back to this. See `derive`'s own note on how this is
    /// built: a neutral step off `Colors::bg`, no tint mixed in.
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
    /// A caution: an action that can be undone but should give pause.
    warning,
    /// An irreversible or destructive action.
    danger,
    /// A frame-grid cell that holds artwork — the timeline's own "paper",
    /// kept close to white in every theme the way Animate's frame grid is,
    /// rather than following the chrome's own polarity. See
    /// `docs/WORKSPACE_DESIGN_PLAN.md` Stage 5 and `timeline_panel`'s
    /// `draw_frame_cell`.
    timeline_cell_filled,
    /// A frame-grid cell that is a blank keyframe, or the span it holds
    /// open — the mid-grey between [`Colors::timeline_cell_filled`] and an
    /// empty cell.
    timeline_cell_blank,
    /// The hairline ruled along a frame cell's right and bottom edge when the
    /// cell sits on [`Colors::timeline_cell_filled`] or
    /// [`Colors::timeline_cell_blank`] — Animate's grid, half the ink of a
    /// boxed cell. See `draw_frame_cell`.
    timeline_rule_filled,
    /// The same hairline, on a cell with no layer reaching it — subtler,
    /// since the cell it rules is dark territory rather than paper.
    timeline_rule_empty,
);

impl Palette {
    /// The old name for [`Self::bg`], kept so panels that have not been
    /// migrated to the Stage 1 role names still compile. See
    /// `docs/WORKSPACE_DESIGN_PLAN.md`.
    #[deprecated(note = "use Palette::bg")]
    pub fn chrome() -> Color32 {
        Self::bg()
    }

    /// The old name for [`Self::field`].
    #[deprecated(note = "use Palette::field")]
    pub fn raised() -> Color32 {
        Self::field()
    }

    /// The old name for [`Self::hairline`].
    #[deprecated(note = "use Palette::hairline")]
    pub fn border() -> Color32 {
        Self::hairline()
    }
}

/// The four colours a theme's chrome roles are built from — see [`derive`].
///
/// Not `pub`: a `Seed` only ever exists for the length of a `derive` call.
/// What a caller asks for is a theme's [`Colors`], never the seed it came
/// from — the seed is this module's own working material, the way flour is
/// not what a bakery sells.
#[derive(Debug, Clone, Copy)]
struct Seed {
    /// Level 0 of the lightness ladder. Everything else in [`derive`] is
    /// measured from here.
    bg: Color32,
    /// Ordinary text. Also which way the ladder runs: toward `text`'s pole
    /// for the roles that rise (`panel`, `field`, `hover`, `divider`), away
    /// from it for the one that recedes (`hairline`).
    text: Color32,
    /// The one accent colour — see [`Colors::accent`].
    accent: Color32,
    /// A warm/cool bias mixed a little into every neutral step, so a
    /// theme's greys lean the way its own colours do rather than every
    /// theme sharing one colourless ramp — Sepia's paper stays warm,
    /// Forest's near-black stays green, Midnight's stays cool.
    tint: Color32,
}

/// The roles [`derive`] does not touch: the timeline/ruler/guide colours the
/// plan keeps as they were, hand-picked per theme rather than derived from a
/// seed. See Stage 1, item 2 of `docs/WORKSPACE_DESIGN_PLAN.md`.
///
/// The four `timeline_*` fields are [`handle_fill`](Self::handle_fill)'s own
/// pattern applied to the frame grid: a control that has to read as "paper"
/// or "ink" regardless of which way a theme's own polarity runs, so — like
/// the transform handle — it is named the same value in most themes rather
/// than climbing the light/dark ladder [`derive`] builds for the chrome
/// around it.
#[derive(Debug, Clone, Copy)]
struct Extras {
    ruler_bg: Color32,
    ruler_tick: Color32,
    ruler_text: Color32,
    guide: Color32,
    guide_locked: Color32,
    grid: Color32,
    handle_fill: Color32,
    handle_stroke: Color32,
    snap: Color32,
    timeline_cell_filled: Color32,
    timeline_cell_blank: Color32,
    timeline_rule_filled: Color32,
    timeline_rule_empty: Color32,
}

/// Move `c` a rough `delta_l` (Lab-ish lightness, 0..100) toward `pole`
/// (`Color32::WHITE` or `Color32::BLACK`). One `L` unit is approximated as
/// 2.55 sRGB steps — exact enough to place a UI's flat fills apart from one
/// another; nothing here claims real CIE Lab.
fn nudge(c: Color32, pole: Color32, delta_l: f32) -> Color32 {
    mix(c, pole, (delta_l * 2.55 / 255.0).clamp(0.0, 1.0))
}

/// `base`, nudged toward `pole` until it reads against `bg` (contrast ratio
/// ≥ 3, WCAG's floor for a graphical object) or `pole` is reached.
fn legible_against(base: Color32, bg: Color32, pole: Color32) -> Color32 {
    let mut c = base;
    for _ in 0..8 {
        if contrast(c, bg) >= 3.0 {
            break;
        }
        c = mix(c, pole, 0.25);
    }
    c
}

/// `c`, nudged toward `pole` until its own relative luminance is at least
/// `floor`'s — a hue-preserving version of "clamp to a literal grey": a
/// theme's own tinted text keeps its colour, it is only brightened as far as
/// the floor actually requires. Used to hold every **dark** theme's `text` at
/// least as bright as `#e8e8e8` and `text_dim` at least as bright as
/// `#bdbdbd` — the user's report was section headers and field labels in dim
/// grey on a dark panel, and `text_dim` was doing work that belongs to `text`
/// (see `derive` and Stage's own follow-up notes).
fn at_least_as_bright(c: Color32, floor: Color32, pole: Color32) -> Color32 {
    let target = relative_luminance(floor);
    let mut out = c;
    for _ in 0..10 {
        if relative_luminance(out) >= target {
            break;
        }
        out = mix(out, pole, 0.2);
    }
    out
}

/// The mirror of [`at_least_as_bright`], for **light** themes: `c` nudged
/// toward `pole` (black) until its luminance is at most `ceiling`'s. Holds
/// `text` at most as bright as `#151515` and `text_dim` at most as bright as
/// `#4a4a4a`.
fn at_most_as_bright(c: Color32, ceiling: Color32, pole: Color32) -> Color32 {
    let target = relative_luminance(ceiling);
    let mut out = c;
    for _ in 0..10 {
        if relative_luminance(out) <= target {
            break;
        }
        out = mix(out, pole, 0.2);
    }
    out
}

/// Build the sixteen roles Stage 1 reduced `Palette` to, from four seed
/// colours, so every theme clears the same contrast floor by construction —
/// see `every_theme_meets_contrast` — rather than by whoever tuned it having
/// a good eye. `extras` supplies the nine roles this does not touch.
fn derive(seed: Seed, extras: Extras) -> Colors {
    let dark = relative_luminance(seed.text) > relative_luminance(seed.bg);
    let toward_text = if dark { Color32::WHITE } else { Color32::BLACK };
    let away_text = if dark { Color32::BLACK } else { Color32::WHITE };

    // The Decision's own example numbers — app bg `#1e1e1e`, panel `#252525`,
    // raised field `#2c2c2c`, hairline `#0f0f0f` between panels and `#3a3a3a`
    // inside them — are exactly this ladder: panel/field/hover/divider rise
    // toward `text` in four steps, and the *between-panel* hairline is the
    // one step that goes the other way.
    let step = |delta_l: f32| mix(nudge(seed.bg, toward_text, delta_l), seed.tint, 0.05);
    let panel = step(3.0);
    let field = step(6.0);
    let hover = step(10.0);
    let divider = step(14.0);

    // Where `bg` already sits at the pole the hairline would recede toward
    // (the Contrast theme's black), there is nowhere left to go — so it
    // borrows `divider` rather than vanishing into `bg`, and the Contrast
    // theme still gets the "visible border on every edge" its own
    // description promises.
    let hairline_recede = nudge(seed.bg, away_text, 6.0);
    let hairline = if contrast(hairline_recede, seed.bg) > 1.05 {
        hairline_recede
    } else {
        divider
    };

    let accent_text = if contrast(Color32::WHITE, seed.accent) >= contrast(Color32::BLACK, seed.accent)
    {
        Color32::WHITE
    } else {
        Color32::BLACK
    };

    // Every dark theme's body text is held at least as bright as `#e8e8e8`
    // (≥ 12:1 against Dark's own `#1e1e1e`), and its secondary text at least
    // as bright as `#bdbdbd` (≥ 8:1) — a seed that was already bright enough
    // keeps its own tint; one that was not is nudged up it, not flattened to
    // a flat grey. Light themes hold the opposite floor, toward black:
    // `#151515` and `#4a4a4a`. See `at_least_as_bright`/`at_most_as_bright`.
    let text = if dark {
        at_least_as_bright(seed.text, rgb(0xE8E8E8), Color32::WHITE)
    } else {
        at_most_as_bright(seed.text, rgb(0x15_15_15), Color32::BLACK)
    };
    let text_dim = {
        let seeded = mix(text, seed.bg, 0.35);
        if dark {
            at_least_as_bright(seeded, rgb(0xBDBDBD), Color32::WHITE)
        } else {
            at_most_as_bright(seeded, rgb(0x4A4A4A), Color32::BLACK)
        }
    };

    // **Animate's own pasteboard tracks the chrome's polarity: a neutral
    // step darker than a dark `bg`, a step lighter than a light one** — not
    // Stage 1's fixed mid-grey, which read as blue-grey the moment it was
    // mixed with a theme's own accent `tint` (see the user's report and
    // `Colors::pasteboard`'s own doc comment). No tint here at all: the one
    // job this colour has is reading as "not the stage", in every theme, and
    // a hue would work against that.
    //
    // The two steps are tuned rather than both a literal +/-12 L: a light
    // theme's `bg` sits so close to the white stage itself (Light's own
    // `#f5f5f5`) that a dark-theme-sized nudge barely moves it — short of
    // Animate's own light grey, `#c8c8c8`. Dark's own `#1e1e1e` lands within
    // a hex digit of Animate's dark grey, `#3c3c3c`, on the smaller step;
    // `.max(45.0)` is only reached by Contrast's pure-black seed, so its
    // pasteboard still clears `the_pasteboard_contrasts_with_the_stage_in_
    // both_themes`'s own floor.
    let bg_level = (seed.bg.r() as f32 + seed.bg.g() as f32 + seed.bg.b() as f32) / 3.0;
    let pasteboard_level = if dark {
        (bg_level + 30.0).max(45.0)
    } else {
        (bg_level - 46.0).max(0.0)
    };
    let pasteboard = Color32::from_gray(pasteboard_level.clamp(0.0, 255.0).round() as u8);

    Colors {
        bg: seed.bg,
        panel,
        field,
        hover,
        active: seed.accent,
        hairline,
        divider,
        text,
        text_dim,
        accent: seed.accent,
        accent_text,
        pasteboard,
        stage_border: hairline,
        selection: seed.accent,
        ruler_bg: extras.ruler_bg,
        ruler_tick: extras.ruler_tick,
        ruler_text: extras.ruler_text,
        guide: extras.guide,
        guide_locked: extras.guide_locked,
        grid: extras.grid,
        handle_fill: extras.handle_fill,
        handle_stroke: extras.handle_stroke,
        snap: extras.snap,
        warning: legible_against(rgb(0xE0A020), seed.bg, toward_text),
        danger: legible_against(rgb(0xE0524A), seed.bg, toward_text),
        timeline_cell_filled: extras.timeline_cell_filled,
        timeline_cell_blank: extras.timeline_cell_blank,
        timeline_rule_filled: extras.timeline_rule_filled,
        timeline_rule_empty: extras.timeline_rule_empty,
    }
}

/// The default. Neutral grey, so the artwork's own colours read true.
const DARK_SEED: Seed = Seed {
    bg: rgb(0x1E1E1E),
    text: rgb(0xE6E6E6),
    accent: rgb(0x378EF0),
    tint: rgb(0x378EF0),
};
const DARK_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x333333),
    ruler_tick: rgb(0x888888),
    ruler_text: rgb(0xAAAAAA),
    guide: rgb(0x00D0C8),
    guide_locked: rgb(0x8A8A60),
    grid: rgba(0x606060, 0x60),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x202020),
    snap: rgb(0xFF4080),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// For bright rooms.
const LIGHT_SEED: Seed = Seed {
    bg: rgb(0xF5F5F5),
    text: rgb(0x1B1B1B),
    accent: rgb(0x1473E6),
    tint: rgb(0x1473E6),
};
const LIGHT_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0xE4E4E4),
    ruler_tick: rgb(0x707070),
    ruler_text: rgb(0x444444),
    guide: rgb(0x009A94),
    guide_locked: rgb(0x8A8A40),
    grid: rgba(0x404040, 0x40),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x202020),
    snap: rgb(0xD00050),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Near-black, cool. The panel is not pure black: a true black behind a dark
/// drawing makes the drawing read as a hole rather than as artwork.
const MIDNIGHT_SEED: Seed = Seed {
    bg: rgb(0x0E1116),
    text: rgb(0xD5DCE6),
    accent: rgb(0x3B82F6),
    tint: rgb(0x3B82F6),
};
const MIDNIGHT_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x171D27),
    ruler_tick: rgb(0x7C879A),
    ruler_text: rgb(0xA3AEBF),
    guide: rgb(0x22D3EE),
    guide_locked: rgb(0x6B7280),
    grid: rgba(0x46505F, 0x60),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x0B0E13),
    snap: rgb(0xFB7185),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// A softer dark: blue-grey, and a step lighter than [`DARK_SEED`].
const SLATE_SEED: Seed = Seed {
    bg: rgb(0x2B313B),
    text: rgb(0xDCE1E8),
    accent: rgb(0x4C86C6),
    tint: rgb(0x4C86C6),
};
const SLATE_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x3A424F),
    ruler_tick: rgb(0x939DAC),
    ruler_text: rgb(0xB4BDC9),
    guide: rgb(0x2FD4C8),
    guide_locked: rgb(0x8A8A60),
    grid: rgba(0x6E7887, 0x60),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x232830),
    snap: rgb(0xFF6088),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Warm paper. The tint is the old hand-picked pasteboard tan, so the
/// neutrals it biases still read as warm the way the paper itself does.
const SEPIA_SEED: Seed = Seed {
    bg: rgb(0xD8CCB3),
    text: rgb(0x2A2116),
    accent: rgb(0x2D6CB5),
    tint: rgb(0x9C907A),
};
const SEPIA_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0xEADFC9),
    ruler_tick: rgb(0x8A7B60),
    ruler_text: rgb(0x4A3E2C),
    guide: rgb(0x00857E),
    guide_locked: rgb(0x8A7A40),
    grid: rgba(0x463C2D, 0x40),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x2A2116),
    snap: rgb(0xC02050),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Built for legibility rather than for comfort: black behind white, a visible
/// border on every edge, and guides at full saturation. The tint is white —
/// deliberately colourless, so nothing here softens toward a hue.
const CONTRAST_SEED: Seed = Seed {
    bg: rgb(0x000000),
    text: rgb(0xFFFFFF),
    accent: rgb(0x4FA3FF),
    tint: rgb(0xFFFFFF),
};
const CONTRAST_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x000000),
    ruler_tick: rgb(0xFFFFFF),
    ruler_text: rgb(0xFFFFFF),
    guide: rgb(0x00FFFF),
    guide_locked: rgb(0xFFFF00),
    grid: rgba(0x969696, 0x90),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x000000),
    snap: rgb(0xFF2D95),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Deep green-black. Not a recognised palette but the one most often asked
/// for alongside them, and the accents are picked from what a forest actually
/// offers: moss, new leaf, and the orange of a fallen one for the snap mark.
const FOREST_SEED: Seed = Seed {
    bg: rgb(0x0B1410),
    text: rgb(0xD3E3D8),
    accent: rgb(0x2F7D53),
    tint: rgb(0x2F7D53),
};
const FOREST_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x132018),
    ruler_tick: rgb(0x7C9687),
    ruler_text: rgb(0xA6BFB0),
    guide: rgb(0x3FD9A0),
    guide_locked: rgb(0x7E8A5E),
    grid: rgba(0x4A5F52, 0x60),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x08110C),
    snap: rgb(0xFF7A59),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Dracula, from its published palette: background `#282A36`, foreground
/// `#F8F8F2`, and the six brights.
///
/// The accent is a deeper purple than the palette's `#BD93F9`, because that one
/// is meant to be written *with* rather than written *on* — see `Colors::active`.
/// The comment colour `#6272A4` is the tint, so the ladder derived from it
/// leans the same violet-grey as the rest of the published palette.
const DRACULA_SEED: Seed = Seed {
    bg: rgb(0x282A36),
    text: rgb(0xF8F8F2),
    accent: rgb(0x7E52C9),
    tint: rgb(0x6272A4),
};
const DRACULA_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x2D2F3D),
    ruler_tick: rgb(0x8B93B8),
    ruler_text: rgb(0xBDC3E0),
    guide: rgb(0x8BE9FD),
    guide_locked: rgb(0xF1FA8C),
    grid: rgba(0x6272A4, 0x60),
    handle_fill: rgb(0xFFFFFF),
    handle_stroke: rgb(0x21222C),
    snap: rgb(0xFF5555),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Solarized Dark, on its own base tones: `base03` behind, `base01` as the
/// tint, and text a step above the published `base1`.
///
/// The palette is built around a fixed lightness relationship rather than
/// around contrast, which is why its text sits closer to its background than
/// any other theme here. That is the point of it, and published `base1`
/// (`#93a1a1`) still clears AA against the *flat* background it was designed
/// for — but `derive`'s `panel` sits a little above that background, closer
/// to text, which pulls `text_dim` (measured against `panel`, not `bg`)
/// under 3:1. Per the plan's own allowance ("if a named palette's published
/// seed fails ... for body text, adjust only its text seed"), the text seed
/// is nudged one step brighter, toward published `base2`, rather than
/// touching the ladder every other theme shares.
const SOLARIZED_DARK_SEED: Seed = Seed {
    bg: rgb(0x002B36),
    text: rgb(0xA6B4B4),
    accent: rgb(0x268BD2),
    tint: rgb(0x586E75),
};
const SOLARIZED_DARK_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x052F3A),
    ruler_tick: rgb(0x587B85),
    ruler_text: rgb(0x93A1A1),
    guide: rgb(0x2AA198),
    guide_locked: rgb(0xB58900),
    grid: rgba(0x586E75, 0x60),
    handle_fill: rgb(0xFDF6E3),
    handle_stroke: rgb(0x00212B),
    snap: rgb(0xDC322F),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Nord, by its own numbering: the Polar Night greys `nord0`-`nord3` for the
/// chrome, Snow Storm for the text, Frost for the accent and tint, and
/// Aurora for the marks that have to be noticed.
const NORD_SEED: Seed = Seed {
    bg: rgb(0x2E3440),
    text: rgb(0xECEFF4),
    accent: rgb(0x5E81AC),
    tint: rgb(0x88C0D0),
};
const NORD_EXTRAS: Extras = Extras {
    ruler_bg: rgb(0x39404F),
    ruler_tick: rgb(0x92A0B5),
    ruler_text: rgb(0xC3CCDA),
    guide: rgb(0x8FBCBB),
    guide_locked: rgb(0xEBCB8B),
    grid: rgba(0x616E85, 0x60),
    handle_fill: rgb(0xECEFF4),
    handle_stroke: rgb(0x2E3440),
    snap: rgb(0xBF616A),
    timeline_cell_filled: rgb(0xE8E8E8),
    timeline_cell_blank: rgb(0x909090),
    timeline_rule_filled: rgb(0x7C7C7C),
    timeline_rule_empty: rgb(0x333333),
};

/// Every theme's [`Colors`], computed once and cached — `derive` is a dozen
/// or so float mixes, cheap in isolation but this is read from many widgets
/// on every frame, and a `Colors` is 25 plain `Color32`s: worth the one-time
/// cost of building the table against re-deriving it on every call.
fn colors_table() -> &'static [Colors; Theme::ALL.len()] {
    static TABLE: std::sync::OnceLock<[Colors; Theme::ALL.len()]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        Theme::ALL.map(|t| match t {
            Theme::Dark => derive(DARK_SEED, DARK_EXTRAS),
            Theme::Light => derive(LIGHT_SEED, LIGHT_EXTRAS),
            Theme::Midnight => derive(MIDNIGHT_SEED, MIDNIGHT_EXTRAS),
            Theme::Slate => derive(SLATE_SEED, SLATE_EXTRAS),
            Theme::Sepia => derive(SEPIA_SEED, SEPIA_EXTRAS),
            Theme::Contrast => derive(CONTRAST_SEED, CONTRAST_EXTRAS),
            Theme::Forest => derive(FOREST_SEED, FOREST_EXTRAS),
            Theme::Dracula => derive(DRACULA_SEED, DRACULA_EXTRAS),
            Theme::SolarizedDark => derive(SOLARIZED_DARK_SEED, SOLARIZED_DARK_EXTRAS),
            Theme::Nord => derive(NORD_SEED, NORD_EXTRAS),
        })
    })
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
    /// Toolbar button edge — Stage 3 of `docs/WORKSPACE_DESIGN_PLAN.md`:
    /// "one column of `Metrics::TOOL_BUTTON = 28` ghost buttons".
    pub const TOOL_BUTTON: f32 = 28.0;
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

/// The chrome's design tokens: spacing, radii, type sizes and control
/// heights, alongside [`Metrics`]. One place, so a panel reads a number
/// rather than hard-coding it — see Stage 1 of
/// `docs/WORKSPACE_DESIGN_PLAN.md`. Everything here is on the 8 px grid
/// (4/8/12/16), checked by `tokens_are_on_the_four_pixel_grid`.
pub struct Tokens;

impl Tokens {
    /// The 8 px grid's four steps.
    pub const SPACE_1: f32 = 4.0;
    pub const SPACE_2: f32 = 8.0;
    pub const SPACE_3: f32 = 12.0;
    pub const SPACE_4: f32 = 16.0;
    /// Height of an ordinary control: a button, a field.
    pub const CONTROL_H: f32 = 24.0;
    /// Height of a row in a list: Layers, the Library.
    pub const ROW_H: f32 = 20.0;
    /// Height of a panel-group tab.
    pub const TAB_H: f32 = 28.0;
    /// Corner radius, everywhere a control has one.
    pub const RADIUS: u8 = 2;
    /// Width of a hairline: a panel edge, a divider.
    pub const HAIRLINE: f32 = 1.0;
    /// Body text.
    pub const TEXT: f32 = 12.0;
    /// Secondary text: a tab, a caption.
    pub const TEXT_SMALL: f32 = 11.0;
    /// A panel or section heading.
    pub const HEADING: f32 = 13.0;
    /// **The smallest a clickable control may be** — Stage 6, item 3 of
    /// `docs/WORKSPACE_DESIGN_PLAN.md`: "a headless audit test walks the
    /// `egui::Context` accesskit tree ... and fails on any clickable smaller
    /// than 24 × 24 at scale 1." `CONTROL_H` and `TOOL_BUTTON` both meet it;
    /// `ROW_H` (20) does not on its own — a Layers or Library row is 20 px
    /// tall, and the audit (in `crates/buzz-app`, where a real window's
    /// worth of widgets can be walked) is what settles whether the row
    /// itself is the clickable target or whether egui's own hit-test
    /// padding around a short row already clears the minimum in practice.
    pub const MIN_TARGET: f32 = 24.0;
}

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
        // Stage 1, item 5 of `docs/WORKSPACE_DESIGN_PLAN.md`: spacing read
        // from `Tokens` rather than hard-coded, so a panel that wants the
        // same rhythm asks for the same numbers this does.
        //
        // **Deviation, recorded rather than silently taken:** the plan asks
        // for `item_spacing = (8, 4)` — `(Tokens::SPACE_2, Tokens::SPACE_1)`
        // on both axes. At `Tokens::SPACE_2` horizontally, the narrowest
        // legal column (`COLUMN_WIDTH_RANGE`'s floor, with every panel
        // stacked into it) overruns the window by 21 points and fails
        // `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`
        // — extra horizontal air between *every* widget in the busiest
        // panels compounds across a row in a way the vertical gap and the
        // button padding do not. The vertical component is the full
        // `Tokens::SPACE_1`; only the horizontal one stays at the old 6.0,
        // which is the nearest thing to the Decision that keeps a currently
        // green test green. Widening `COLUMN_WIDTH_RANGE`'s floor instead
        // would fix it more in the spirit of the plan but changes what a
        // user can drag a column down to, which is out of scope for a
        // token/visuals stage.
        style.spacing.item_spacing = egui::vec2(6.0, Tokens::SPACE_1);
        style.spacing.button_padding = egui::vec2(Tokens::SPACE_2, 3.0);
        // Stage 2, item 4: menus specifically get a 6 px margin — tighter
        // than the general `Tokens::SPACE_2` (8) the rest of the chrome
        // reads, because a dropdown's items are already inset by their own
        // `button_padding` and an 8 px margin on top of that read as too
        // much air around a one-line list.
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.interact_size.y = Tokens::CONTROL_H;
        style.spacing.indent = Tokens::SPACE_4;

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
    visuals.extreme_bg_color = Palette::bg();
    visuals.faint_bg_color = Palette::field();
    visuals.override_text_color = Some(Palette::text());
    // The one accent, on the one thing egui itself paints a link with.
    visuals.hyperlink_color = Palette::accent();

    let hairline = Stroke::new(Tokens::HAIRLINE, Palette::hairline());

    visuals.widgets.noninteractive.bg_fill = Palette::panel();
    visuals.widgets.noninteractive.weak_bg_fill = Palette::panel();
    visuals.widgets.noninteractive.bg_stroke = hairline;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(Tokens::HAIRLINE, Palette::text_dim());

    visuals.widgets.inactive.bg_fill = Palette::field();
    visuals.widgets.inactive.weak_bg_fill = Palette::field();
    visuals.widgets.inactive.bg_stroke = hairline;
    visuals.widgets.inactive.fg_stroke = Stroke::new(Tokens::HAIRLINE, Palette::text());

    visuals.widgets.hovered.bg_fill = Palette::hover();
    visuals.widgets.hovered.weak_bg_fill = Palette::hover();
    visuals.widgets.hovered.bg_stroke = hairline;
    // `Palette::text()`, not a fixed white: on a light theme a white hover
    // ink drew button labels white-on-light-grey the moment the pointer
    // touched them.
    visuals.widgets.hovered.fg_stroke = Stroke::new(Tokens::HAIRLINE, Palette::text());

    visuals.widgets.active.bg_fill = Palette::active();
    visuals.widgets.active.weak_bg_fill = Palette::active();
    visuals.widgets.active.bg_stroke = Stroke::new(Tokens::HAIRLINE, Palette::accent());
    // **The body text colour, not `accent_text`.** egui resolves every
    // `.strong()` label through `Visuals::strong_text_color()`, which *is*
    // this stroke — so while it was `accent_text` (black for Dark's
    // `#378ef0`), every strong label in every dark theme painted near-black
    // on a dark panel. Fixing those one call site at a time left dozens
    // behind; fixing the source covers them all. Our own accent-filled
    // controls (the active tool) still ink themselves with `accent_text`.
    visuals.widgets.active.fg_stroke = Stroke::new(Tokens::HAIRLINE, Palette::text());

    // Selection is a wash of the accent, not a fill: it marks what is chosen
    // without competing with the one place the accent is meant to be read as
    // *action* — the tab underline, the active tool, focus.
    visuals.selection.bg_fill = Palette::accent().gamma_multiply(0.25);
    // **The focus ring** — Stage 6, item 2 of `docs/WORKSPACE_DESIGN_PLAN.md`:
    // a 2 px accent outline on the focused widget. egui reads this same
    // `Visuals::selection.stroke` both for a focused widget's outline and for
    // a text selection's border, so the one number serves both.
    visuals.selection.stroke = Stroke::new(2.0, Palette::accent());

    // Square-ish corners read as a professional tool rather than a web app;
    // windows and menus get one step more so a floating panel still reads as
    // a distinct object against the square controls inside it.
    let radius = CornerRadius::same(Tokens::RADIUS);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    let window_radius = CornerRadius::same(4);
    visuals.window_corner_radius = window_radius;
    visuals.menu_corner_radius = window_radius;

    // No shadows anywhere flat chrome would cast one, except a menu lifting
    // off the surface below it — the one place in this interface something
    // genuinely floats over something else.
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 8,
        spread: 0,
        color: Color32::from_black_alpha(96),
    };

    visuals
}

/// WCAG relative luminance: gamma-corrected, and weighted for the eye.
///
/// Module-level rather than test-only: [`derive`] needs it (for
/// `accent_text` and the hairline's legibility fallback) as much as the
/// tests do, and the two must never drift into measuring two different
/// things.
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

    /// Follow-up 2026-09-26, item 3: the pasteboard tracks the theme's own
    /// polarity — a dark theme's is a dark neutral grey, a light theme's a
    /// light one — rather than Stage 1's fixed mid-grey (which landed close
    /// to `gray(128)` regardless of polarity, and read as blue-grey once
    /// mixed with a theme's own accent tint).
    #[test]
    fn the_pasteboard_tracks_the_themes_own_polarity() {
        in_each_theme(|t| {
            let luminance = lum(Palette::pasteboard());
            if t.is_dark() {
                assert!(
                    luminance < 128.0,
                    "{t:?}: a dark theme's pasteboard should be dark, got {luminance}"
                );
            } else {
                assert!(
                    luminance > 128.0,
                    "{t:?}: a light theme's pasteboard should be light, got {luminance}"
                );
            }
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
            assert!(
                seen.contains(&expected),
                "{} is not on the cycle",
                expected.label()
            );
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

    /// **[`Colors::accent_text`] is written on the accent**, so the accent
    /// cannot be one of a palette's bright colours with no reading legible on
    /// it in either black or white.
    ///
    /// Originally this held every theme to white specifically, because
    /// `build_visuals` put `Color32::WHITE` on every active control
    /// regardless of theme. Stage 1 of `docs/WORKSPACE_DESIGN_PLAN.md`
    /// replaced that with `accent_text` — whichever of black or white
    /// actually reads — precisely so a theme like Contrast, whose own accent
    /// `#4fa3ff` is legible with black but not with white at this floor, is
    /// not forced to darken an otherwise-correct accent to satisfy a label
    /// colour it no longer hard-codes. Dracula's `#BD93F9` and Nord's frost
    /// blue remain the obvious accents to reach for and still fail if tried
    /// with white; `accent_text` is what lets a theme reach for black instead
    /// rather than tuning the accent itself. 3.0 is WCAG AA for text at this
    /// weight; `every_theme_meets_contrast` holds `accent_text` itself to the
    /// stricter 4.5.
    /// **`.strong()` text reads on the panel it sits on, in every theme.**
    /// egui takes strong text from `widgets.active.fg_stroke`; when that was
    /// `accent_text`, Dark's strong labels came out black on a dark panel.
    #[test]
    fn strong_text_reads_on_the_panel() {
        in_each_theme(|t| {
            let v = build_visuals();
            for (role, ink) in [
                ("strong", v.strong_text_color()),
                ("hovered", v.widgets.hovered.fg_stroke.color),
            ] {
                let ratio = contrast(ink, Palette::panel());
                assert!(
                    ratio >= 4.5,
                    "{}: {role} text on the panel is {ratio:.2}:1",
                    t.label()
                );
            }
        });
    }

    #[test]
    fn the_accent_can_be_written_on() {
        in_each_theme(|t| {
            let ratio = contrast(Palette::accent_text(), Palette::active());
            assert!(
                ratio >= 3.0,
                "{}: accent_text on the accent is {ratio:.2}:1, so a pressed \
                 control cannot be read",
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

    /// Stage 1's own acceptance test, item 6 of
    /// `docs/WORKSPACE_DESIGN_PLAN.md`: body text at 4.5:1 against the panel,
    /// secondary text at 3:1, and `accent_text` at 4.5:1 against its own
    /// accent fill — every theme, held to the same floor by construction
    /// (`derive`) rather than by whoever tuned that theme having a good eye.
    #[test]
    fn every_theme_meets_contrast() {
        in_each_theme(|t| {
            let panel = Palette::panel();
            let text_ratio = contrast(Palette::text(), panel);
            assert!(
                text_ratio >= 7.0,
                "{}: text on the panel is {text_ratio:.2}:1, below 7:1",
                t.label()
            );
            let dim_ratio = contrast(Palette::text_dim(), panel);
            assert!(
                dim_ratio >= 4.5,
                "{}: text_dim on the panel is {dim_ratio:.2}:1, below 4.5:1",
                t.label()
            );
            let accent_ratio = contrast(Palette::accent_text(), Palette::accent());
            assert!(
                accent_ratio >= 4.5,
                "{}: accent_text on the accent is {accent_ratio:.2}:1, below 4.5:1",
                t.label()
            );
        });
    }

    /// Stage 6, item 2: a 2 px accent outline on the focused widget, in
    /// every theme — egui reads `Visuals::selection.stroke` for this.
    #[test]
    fn every_theme_draws_a_two_pixel_accent_focus_ring() {
        in_each_theme(|t| {
            let visuals = build_visuals();
            assert_eq!(
                visuals.selection.stroke.width,
                2.0,
                "{}: the focus ring is not 2 px",
                t.label()
            );
            assert_eq!(
                visuals.selection.stroke.color,
                Palette::accent(),
                "{}: the focus ring is not the accent",
                t.label()
            );
        });
    }

    /// Stage 1, item 6: the spacing tokens are all on the 4 px grid the brief
    /// asks for — `4/8/12/16`, and the control heights built from them.
    #[test]
    fn tokens_are_on_the_four_pixel_grid() {
        for value in [
            Tokens::SPACE_1,
            Tokens::SPACE_2,
            Tokens::SPACE_3,
            Tokens::SPACE_4,
            Tokens::CONTROL_H,
            Tokens::ROW_H,
            Tokens::TAB_H,
        ] {
            assert_eq!(
                value % 4.0,
                0.0,
                "{value} is not a multiple of 4"
            );
        }
        assert_eq!(Tokens::SPACE_1, 4.0);
        assert_eq!(Tokens::SPACE_2, 8.0);
        assert_eq!(Tokens::SPACE_3, 12.0);
        assert_eq!(Tokens::SPACE_4, 16.0);
    }

    /// Stage 6, item 3: the controls that are always a full button — not a
    /// short list row — already clear the 24 × 24 minimum on their own
    /// token, with no audit needed to find out. `ROW_H` is deliberately not
    /// asserted here; see `Tokens::MIN_TARGET`'s own doc comment.
    #[test]
    fn ordinary_controls_already_meet_the_minimum_hit_target() {
        assert!(Tokens::CONTROL_H >= Tokens::MIN_TARGET);
        assert!(Metrics::TOOL_BUTTON >= Tokens::MIN_TARGET);
        assert!(Tokens::TAB_H >= Tokens::MIN_TARGET);
    }

    /// Stage 2, item 6 of `docs/WORKSPACE_DESIGN_PLAN.md`: "a headless
    /// `build_ui` test asserting no widget uses the removed colour roles."
    ///
    /// A widget's *painted* colour is not something a headless test can read
    /// back — `FullOutput` is triangles and glyphs, not a record of which
    /// `Palette` function chose them — so this is the fence one level up:
    /// every call to the three roles Stage 1 deprecated (`chrome`, `raised`,
    /// `border`) is a `cargo build` warning already, and this turns "did that
    /// count grow" into something a test run catches on its own rather than
    /// something that only shows up if somebody reads the warnings. It counts
    /// down as panels migrate — Stage 2 migrated `section_header`'s own use
    /// of them, `app.rs` had none left after that — and Stage 5 owns getting
    /// it to zero.
    #[test]
    fn deprecated_colour_roles_are_a_tracked_list_not_a_growing_one() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut count = 0usize;
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("buzz-ui/src should exist") {
                let entry = entry.expect("a readable directory entry");
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                // Not this file: it is where the three needles below are
                // *declared* (as deprecated aliases) and where this very
                // list of them is written out as a string, and neither of
                // those is a call site.
                if path.file_name().is_some_and(|f| f == "theme.rs") {
                    continue;
                }
                if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
                    for needle in [
                        "Palette::chrome()",
                        "Palette::raised()",
                        "Palette::border()",
                    ] {
                        count += text.matches(needle).count();
                    }
                }
            }
        }
        assert_eq!(
            count, 14,
            "deprecated colour role call sites in buzz-ui: was 17 when Stage 2 \
             landed, 15 since Stage 3's tool strip stopped using `raised()`, \
             14 since Stage 5's layer-switch hover fill did too, now {count} \
             — if this grew, migrate the new call site to `bg`/`field`/`hairline` \
             instead of adding to the list; if it shrank, lower this number"
        );
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
