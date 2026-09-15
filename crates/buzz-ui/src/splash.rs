//! The opening scene.
//!
//! # What this replaces
//!
//! A window used to be created, shown, and only *then* handed to the GPU. The
//! graphics device, the surface, the shader compilation and the first layout
//! all happen after the window exists, and for the second or so they take,
//! what was on screen was whatever the desktop compositor had left there: a
//! blank white client area, then a partly laid-out interface, then a black
//! rectangle where the stage had not yet been measured. Three different
//! pictures in the space of a second, none of them this program's.
//!
//! Two changes fix it, and the other one is in `buzz_app::app`: the window is
//! **created hidden** and revealed only once a frame has actually been drawn
//! into it. So the first thing anybody sees is a finished picture — this one.
//!
//! # Why an overlay rather than a separate window
//!
//! A second window would need its own surface, its own renderer and its own
//! event handling, and it would be the thing standing between the user and the
//! editor for as long as it lived. This is drawn *over* the real interface,
//! which is being laid out and rendered underneath the whole time: by the
//! moment it dissolves, the editor behind it has already settled, measured its
//! stage and started its thumbnails. The opening scene costs the startup
//! nothing — it is spent on work that had to happen anyway.

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme::{self, Palette};

/// The banner, as it ships. The same picture Help ▸ About shows.
const BANNER: &[u8] = include_bytes!("../../../assets/banner-800.png");

/// How long the scene holds at full strength.
///
/// Long enough to be read, short enough that nobody waits for it. It overlaps
/// the startup work rather than adding to it — see the module docs.
const HOLD: Duration = Duration::from_millis(800);

/// How long it takes to dissolve into the editor.
const FADE: Duration = Duration::from_millis(420);

/// How long the contents take to settle into place after the first frame.
const RISE: Duration = Duration::from_millis(450);

/// How wide the artwork is allowed to be, as a fraction of the window and in
/// points. The clamp matters: on a small window an unclamped banner squashes
/// the text under it, and on a very wide one it becomes a wall.
const BANNER_FRACTION: f32 = 0.46;
const BANNER_MIN: f32 = 300.0;
const BANNER_MAX: f32 = 560.0;

/// The opening scene's state: the artwork, and where it is in its run.
#[derive(Default)]
pub struct SplashState {
    /// Uploaded on the first frame it is drawn on, released when it is done.
    banner: Option<egui::TextureHandle>,
    /// When the first frame was drawn. `None` until then, which is what makes
    /// the timing start from the first picture rather than from process start:
    /// the seconds spent creating a device are not seconds anybody saw.
    shown_at: Option<Instant>,
    /// Once this is set the scene never draws again for the life of the app.
    finished: bool,
}

impl std::fmt::Debug for SplashState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplashState")
            .field("shown", &self.shown_at.is_some())
            .field("finished", &self.finished)
            .finish()
    }
}

impl SplashState {
    /// Is the scene still owed frames?
    ///
    /// The window sleeps unless something asks for a frame, so the app folds
    /// this into what it asks for. Without it the scene would freeze on
    /// whatever frame the last input event happened to produce.
    pub fn is_open(&self) -> bool {
        !self.finished
    }

    /// Is the scene still covering the interface completely?
    ///
    /// True through the hold, false from the moment it starts to dissolve —
    /// which is when the editor underneath becomes visible and therefore has to
    /// start accepting input again.
    pub fn is_covering(&self) -> bool {
        match (self.finished, self.shown_at) {
            (true, _) => false,
            (false, None) => true,
            (false, Some(at)) => Instant::now().saturating_duration_since(at) < HOLD,
        }
    }

    /// Skip the rest of the hold: dissolve from here.
    ///
    /// A click, a key, or anything else that says the user is already trying to
    /// work. Nobody should have to wait out a picture they have seen before.
    pub fn skip(&mut self) {
        let now = Instant::now();
        match self.shown_at {
            // Already dissolving, or done — leave it alone, so a second click
            // cannot restart the fade from the top.
            Some(at) if now.saturating_duration_since(at) >= HOLD => {}
            _ => self.shown_at = now.checked_sub(HOLD).or(Some(now)),
        }
    }

    /// End it now, with no dissolve at all. For a run that has no business
    /// showing one — a script, a test.
    pub fn dismiss(&mut self) {
        self.finished = true;
        self.banner = None;
    }
}

/// Draw the opening scene over everything else. Returns whether it drew.
///
/// `status` is the one line of genuinely useful information a startup screen
/// can carry — which graphics adapter the program came up on. It is the first
/// thing anybody investigating a slow or wrong-looking window is asked for, and
/// putting it here means it has been seen once before it is ever needed.
pub fn opening_scene(ctx: &egui::Context, state: &mut SplashState, status: &str) -> bool {
    if state.finished {
        return false;
    }

    let now = Instant::now();
    let started = *state.shown_at.get_or_insert(now);
    let elapsed = now.saturating_duration_since(started);

    if elapsed >= HOLD + FADE {
        state.finished = true;
        // The texture is video memory for a picture that will never be drawn
        // again; About uploads its own when it is opened.
        state.banner = None;
        return false;
    }

    if state.banner.is_none()
        && let Some(image) = decode(BANNER)
    {
        state.banner = Some(ctx.load_texture("opening-banner", image, Default::default()));
    }

    // **Two curves, deliberately separate.** `veil` is the scene's hold on the
    // window and only moves at the end; `entrance` is the contents arriving and
    // only moves at the beginning. Driving both from one number gave a scene
    // that faded in and straight back out, which reads as a glitch rather than
    // as an opening.
    let veil = if elapsed <= HOLD {
        1.0
    } else {
        let t = (elapsed - HOLD).as_secs_f32() / FADE.as_secs_f32();
        1.0 - ease_out(t)
    };
    let entrance = ease_out(elapsed.as_secs_f32() / RISE.as_secs_f32());
    let ink = entrance * veil;

    let screen = ctx.viewport_rect();
    let covering = elapsed < HOLD;

    let response = egui::Area::new(egui::Id::new("buzz-opening-scene"))
        .order(egui::Order::Foreground)
        .movable(false)
        // Only while it is opaque. Once the editor behind it can be seen it can
        // be used: a fade that still swallowed clicks would be a window that
        // looks ready and does nothing.
        .interactable(covering)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());
            let response = ui.allocate_response(
                screen.size(),
                if covering {
                    Sense::click()
                } else {
                    Sense::empty()
                },
            );
            paint(ui.painter(), screen, state, status, veil, ink, elapsed);
            response
        })
        .inner;

    if response.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.skip();
    }

    // Nothing else would: the window is idle at this point, and the whole scene
    // is an animation.
    ctx.request_repaint();
    true
}

/// Everything the scene is made of, painted back to front.
fn paint(
    painter: &egui::Painter,
    screen: Rect,
    state: &SplashState,
    status: &str,
    veil: f32,
    ink: f32,
    elapsed: Duration,
) {
    backdrop(painter, screen, veil);

    let banner_w = (screen.width() * BANNER_FRACTION).clamp(BANNER_MIN, BANNER_MAX);
    // Fall back to the artwork's own 16:9 if it could not be decoded, so the
    // layout below is the same shape either way.
    let ratio = state
        .banner
        .as_ref()
        .map(|b| {
            let size = b.size_vec2();
            size.y / size.x.max(1.0)
        })
        .unwrap_or(9.0 / 16.0);
    let banner_h = banner_w * ratio;

    // The block is measured before it is drawn so it can be centred as a whole.
    // Nudged above centre by a twentieth of the window: an optically centred
    // block sits slightly high, and a mathematically centred one reads as low.
    const TITLE: f32 = 30.0;
    const SUBTITLE: f32 = 13.0;
    const TRACK: f32 = 3.0;
    let block_h = banner_h + 26.0 + TITLE + 10.0 + SUBTITLE + 26.0 + TRACK;
    let mut y = screen.center().y - block_h * 0.5 - screen.height() * 0.05;
    // The contents rise into place; the backdrop does not move.
    y += (1.0 - ink.min(1.0)) * 14.0;

    let cx = screen.center().x;

    let banner_rect = Rect::from_min_size(pos2(cx - banner_w * 0.5, y), vec2(banner_w, banner_h));
    if let Some(banner) = &state.banner {
        painter.image(
            banner.id(),
            banner_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(ink),
        );
    }
    // A hairline round the artwork, so it is a plate on the backdrop rather
    // than a hole in it.
    painter.rect_stroke(
        banner_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, Palette::border().gamma_multiply(ink * 0.8)),
        StrokeKind::Inside,
    );
    y += banner_h + 26.0;

    painter.text(
        pos2(cx, y),
        Align2::CENTER_TOP,
        "BuzzAnimate",
        FontId::proportional(TITLE),
        Palette::text().gamma_multiply(ink),
    );
    y += TITLE + 10.0;

    painter.text(
        pos2(cx, y),
        Align2::CENTER_TOP,
        format!(
            "version {} \u{00B7} GPU-accelerated vector animation",
            env!("CARGO_PKG_VERSION")
        ),
        FontId::proportional(SUBTITLE),
        Palette::text_dim().gamma_multiply(ink),
    );
    y += SUBTITLE + 26.0;

    progress(painter, cx, y, banner_w, TRACK, ink, elapsed);

    if !status.is_empty() {
        painter.text(
            pos2(cx, screen.bottom() - 26.0),
            Align2::CENTER_BOTTOM,
            status,
            FontId::proportional(11.0),
            Palette::text_dim().gamma_multiply(ink * 0.75),
        );
    }
}

/// The ground the scene stands on: the interface's own chrome colour, deepened
/// towards the bottom.
///
/// Theme-aware, and that is not decoration. The scene dissolves *into* the
/// editor, so a fixed dark backdrop under a light interface would end the
/// opening with exactly the abrupt light-to-dark switch this whole module
/// exists to remove.
fn backdrop(painter: &egui::Painter, screen: Rect, veil: f32) {
    let base = Palette::chrome();
    let top = shade(base, 1.12).gamma_multiply(veil);
    let bottom = shade(base, 0.72).gamma_multiply(veil);

    let mut mesh = egui::Mesh::default();
    mesh.vertices.push(theme::vertex(screen.left_top(), top));
    mesh.vertices.push(theme::vertex(screen.right_top(), top));
    mesh.vertices.push(theme::vertex(screen.right_bottom(), bottom));
    mesh.vertices.push(theme::vertex(screen.left_bottom(), bottom));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));

    // The masthead the rest of the program wears, so the opening is recognisably
    // the same application as what comes after it.
    let band = Rect::from_min_size(
        screen.left_top(),
        vec2(screen.width(), theme::BANNER_HEIGHT),
    );
    let mut strip = egui::Mesh::default();
    let steps = (band.width() / 24.0).clamp(8.0, 160.0) as usize;
    for step in 0..steps {
        let t0 = step as f32 / steps as f32;
        let t1 = (step + 1) as f32 / steps as f32;
        let x0 = band.left() + band.width() * t0;
        let x1 = band.left() + band.width() * t1;
        let c0 = theme::brand_at(t0).gamma_multiply(veil);
        let c1 = theme::brand_at(t1).gamma_multiply(veil);
        let i = strip.vertices.len() as u32;
        strip.vertices.push(theme::vertex(pos2(x0, band.top()), c0));
        strip.vertices.push(theme::vertex(pos2(x0, band.bottom()), c0));
        strip.vertices.push(theme::vertex(pos2(x1, band.bottom()), c1));
        strip.vertices.push(theme::vertex(pos2(x1, band.top()), c1));
        strip.add_triangle(i, i + 1, i + 2);
        strip.add_triangle(i, i + 2, i + 3);
    }
    painter.add(egui::Shape::mesh(strip));
}

/// The waiting line: a segment sweeping the width of the artwork.
///
/// Indeterminate on purpose. The startup does not know its own length — the
/// adapter, the shader cache and the size of the document being opened all move
/// it — and a bar that claims a percentage it cannot know is worse than one
/// that only says "still working".
fn progress(
    painter: &egui::Painter,
    cx: f32,
    y: f32,
    width: f32,
    height: f32,
    ink: f32,
    elapsed: Duration,
) {
    let track = Rect::from_min_size(pos2(cx - width * 0.5, y), vec2(width, height));
    let radius = CornerRadius::same((height * 0.5).round() as u8);
    painter.rect_filled(track, radius, Palette::border().gamma_multiply(ink * 0.9));

    // A cosine ping-pong rather than a wrap: the segment eases at each end
    // instead of teleporting back to the left.
    let segment = width * 0.3;
    let phase = (elapsed.as_secs_f32() / 1.4).fract();
    let travel = 0.5 - 0.5 * (phase * TAU).cos();
    let x = track.left() + travel * (width - segment);
    painter.rect_filled(
        Rect::from_min_size(pos2(x, y), vec2(segment, height)),
        radius,
        theme::brand_at(travel).gamma_multiply(ink),
    );
}

/// Multiply a colour's brightness, keeping it in range.
fn shade(c: Color32, factor: f32) -> Color32 {
    let f = |x: u8| ((x as f32) * factor).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Decelerating ease over `0..=1`, clamped at both ends.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// Decode a PNG into something egui can upload.
fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let mut pixels = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());

    let size = [info.width as usize, info.height as usize];
    match info.color_type {
        png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied(size, &pixels)),
        png::ColorType::Rgb => Some(egui::ColorImage::from_rgb(size, &pixels)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: &mut SplashState) -> bool {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut drew = false;
        let _ = ctx.run_ui(Default::default(), |ui| {
            drew = opening_scene(ui.ctx(), state, "Test adapter");
        });
        drew
    }

    /// Rewind the clock instead of sleeping for a second and a half in the
    /// suite. The state is a start instant and nothing else, so moving it back
    /// is exactly equivalent to time having passed.
    fn age(state: &mut SplashState, by: Duration) {
        state.shown_at = state.shown_at.and_then(|at| at.checked_sub(by));
    }

    #[test]
    fn it_covers_the_window_and_then_gets_out_of_the_way() {
        let mut state = SplashState::default();
        assert!(state.is_open(), "it has not run yet");

        assert!(run(&mut state), "the first frame draws it");
        assert!(state.is_covering(), "and nothing behind it is usable yet");

        age(&mut state, HOLD + Duration::from_millis(1));
        assert!(run(&mut state), "it is still on screen while it dissolves");
        assert!(
            !state.is_covering(),
            "but the editor behind it takes input again from the first fading frame"
        );

        age(&mut state, FADE);
        assert!(!run(&mut state), "past the fade it is gone");
        assert!(!state.is_open(), "and it never comes back");
        assert!(!run(&mut state), "not even when asked again");
    }

    /// The defect this guards: a fade that restarted from full opacity every
    /// time the user clicked would leave a window that could not be dismissed.
    #[test]
    fn a_click_shortens_it_and_a_second_one_does_not_undo_that() {
        let mut state = SplashState::default();
        run(&mut state);

        state.skip();
        assert!(!state.is_covering(), "skipping starts the dissolve");

        let after_first = state.shown_at;
        state.skip();
        assert_eq!(
            state.shown_at, after_first,
            "a second skip must not push the fade back to its start"
        );
    }

    #[test]
    fn a_run_that_should_not_show_one_can_say_so() {
        let mut state = SplashState::default();
        state.dismiss();
        assert!(!state.is_open());
        assert!(!run(&mut state), "dismissed means it never draws");
    }

    /// The picture has to be one this program can decode: a broken file would
    /// open the app on an empty backdrop, and the opening scene is the one
    /// screen nobody gets a second chance at.
    #[test]
    fn the_banner_decodes() {
        let image = decode(BANNER).expect("the banner should decode");
        assert!(
            image.width() > 200 && image.height() > 100,
            "{:?}",
            image.size
        );
    }

    #[test]
    fn the_ease_stays_in_range() {
        assert_eq!(ease_out(-1.0), 0.0);
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(4.0), 1.0);
        assert!(ease_out(0.5) > 0.5, "it decelerates, so it is ahead by half");
    }
}
