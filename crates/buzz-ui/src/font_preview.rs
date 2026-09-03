//! **Showing a font in the font.**
//!
//! A list of family names set in the interface's own face tells you what a font
//! is called and nothing about what it looks like, which is the only question
//! being asked while scrolling it. So each row draws its own name using that
//! family's real glyphs.
//!
//! # Outlines, stroked
//!
//! The glyphs come from [`buzz_text::outline_styled`] — the same call the Text
//! tool draws with, so a preview cannot show one thing and the stage another —
//! and are painted as **stroked outlines** rather than filled. egui's
//! tessellator fills a closed path as a convex polygon, which is wrong for every
//! letter with a counter: the hole in an "o" would be painted solid. A hairline
//! tracing of the same curves has no fill rule to get wrong, and at a row's
//! height it reads as the typeface just as well.
//!
//! # Cached
//!
//! Outlining a string means loading and shaping a font file, which is far too
//! much to do per row per frame. Each family is outlined once, normalised to a
//! unit height, and kept; scrolling the list afterwards is arithmetic.

use std::collections::HashMap;

use buzz_geom::Shape as _;
use egui::{Pos2, Sense, Stroke, Ui, Vec2};

/// How tall a preview row is drawn, in points.
const ROW_HEIGHT: f32 = 22.0;

/// The em size the outline is taken at. Large enough that the flattening
/// tolerance below is fine relative to the letterforms.
const OUTLINE_SIZE: f64 = 96.0;

/// How finely curves are broken into lines for painting.
const FLATTEN_TOLERANCE: f64 = 0.25;

/// One family's name, traced, normalised to a unit height with its origin at the
/// top left.
#[derive(Clone, Default)]
struct Traced {
    /// Polylines in 0..=1 vertically; x runs 0..`width`.
    strokes: Vec<Vec<(f32, f32)>>,
    width: f32,
}

/// Font previews, outlined once each and kept for the session.
#[derive(Default)]
pub struct FontPreviews {
    cache: HashMap<(String, bool, bool), Traced>,
}

impl FontPreviews {
    /// Draw one selectable row showing `family` set in itself.
    ///
    /// Falls back to a plain selectable label when the family cannot be
    /// outlined — a font file that will not read should cost the user a nice
    /// preview, not the ability to choose it.
    pub fn row(
        &mut self,
        ui: &mut Ui,
        family: &str,
        style: buzz_scene::FontStyle,
        selected: bool,
        suffix: &str,
    ) -> egui::Response {
        let traced = self.traced(family, style);
        if traced.strokes.is_empty() {
            return ui.selectable_label(selected, format!("{family}{suffix}"));
        }

        let width = (traced.width * ROW_HEIGHT).clamp(40.0, 240.0);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width + 8.0, ROW_HEIGHT),
            Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact_selectable(&response, selected);
            if selected || response.hovered() {
                ui.painter().rect_filled(rect, 2.0, visuals.bg_fill);
            }
            let ink = visuals.text_color();
            let scale = ROW_HEIGHT * 0.8;
            let origin = rect.left_top() + Vec2::new(4.0, ROW_HEIGHT * 0.1);
            let painter = ui.painter().with_clip_rect(rect);
            for line in &traced.strokes {
                let points: Vec<Pos2> = line
                    .iter()
                    .map(|(x, y)| origin + Vec2::new(x * scale, y * scale))
                    .collect();
                if points.len() > 1 {
                    painter.add(egui::Shape::line(points, Stroke::new(1.0, ink)));
                }
            }
        }
        response.on_hover_text(format!("{family} — {}", style.label()))
    }

    /// How wide a row for this family will be, so a dropdown can size itself.
    pub fn row_width(&mut self, family: &str, style: buzz_scene::FontStyle) -> f32 {
        (self.traced(family, style).width * ROW_HEIGHT).clamp(40.0, 240.0) + 8.0
    }

    fn traced(&mut self, family: &str, style: buzz_scene::FontStyle) -> Traced {
        let key = (family.to_string(), style.bold, style.italic);
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let traced = trace(family, style);
        self.cache.insert(key, traced.clone());
        traced
    }
}

/// Outline a family's own name in it, flattened and normalised.
fn trace(family: &str, style: buzz_scene::FontStyle) -> Traced {
    let Some(path) = buzz_text::outline_styled(
        family,
        OUTLINE_SIZE,
        Some(family),
        style,
        buzz_scene::TextAlign::Left,
    ) else {
        return Traced::default();
    };
    let bounds = path.bounding_box();
    if !(bounds.height() > 0.0) || !bounds.width().is_finite() {
        return Traced::default();
    }

    // Normalised by *height*, so every row's letters are the same size whatever
    // the em square of the face happens to be.
    let scale = 1.0 / bounds.height();
    let mut strokes: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    let push = |current: &mut Vec<(f32, f32)>, strokes: &mut Vec<Vec<(f32, f32)>>| {
        if current.len() > 1 {
            strokes.push(std::mem::take(current));
        } else {
            current.clear();
        }
    };
    let place = |p: buzz_geom::Point| {
        (
            ((p.x - bounds.x0) * scale) as f32,
            ((p.y - bounds.y0) * scale) as f32,
        )
    };
    kurbo::flatten(path.iter(), FLATTEN_TOLERANCE, |el| match el {
        buzz_geom::PathEl::MoveTo(p) => {
            push(&mut current, &mut strokes);
            current.push(place(p));
        }
        buzz_geom::PathEl::LineTo(p) => current.push(place(p)),
        buzz_geom::PathEl::ClosePath => {
            if let Some(first) = current.first().copied() {
                current.push(first);
            }
            push(&mut current, &mut strokes);
        }
        // `flatten` emits only moves, lines and closes.
        _ => {}
    });
    push(&mut current, &mut strokes);

    Traced {
        width: (bounds.width() * scale) as f32,
        strokes,
    }
}
