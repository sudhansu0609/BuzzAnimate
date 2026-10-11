//! Contact sheets for the title-card presets (T in
//! `docs/PRODUCTION_SPEED_PLAN.md`): every preset at four frames — part-way
//! in, the end of the in, the hold, part-way out — one sheet per language.
//!
//! ```text
//! cargo run -p buzz-app --example title_sheet -- <out-dir>
//! ```
//!
//! Writes `<out-dir>/titles-en.png`, `<out-dir>/titles-hi.png` and
//! `<out-dir>/typewriter-hi.png` (every step of a Hindi Typewriter, to see
//! that no conjunct or matra is split), and prints each Typewriter's units.

use buzz_act::title::{self, TitlePlate, TitlePosition, TitlePreset, TitleSpec};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::Scene;
use image::{Rgba, RgbaImage};
use peniko::Color;

const CELL_W: u32 = 480;
const CELL_H: u32 = 270;
const LABEL_H: u32 = 22;

fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        _ => [0; 7],
    }
}

fn label(img: &mut RgbaImage, text: &str, x: u32, y: u32, scale: u32) {
    let ink = Rgba([0x30, 0x30, 0x38, 255]);
    for (k, c) in text.to_uppercase().chars().enumerate() {
        let rows = glyph(c);
        let gx = x + k as u32 * 6 * scale;
        for (ry, row) in rows.iter().enumerate() {
            for rx in 0..5u32 {
                if row & (0x10 >> rx) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (gx + rx * scale + dx, y + ry as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, ink);
                        }
                    }
                }
            }
        }
    }
}

/// A 1280×720 stage with a mid-blue sky and a green ground, so white text and
/// a dark plate both read.
fn stage() -> Scene {
    let mut scene = Scene::default();
    {
        let s = scene.stage_mut();
        s.size = kurbo::Size::new(1280.0, 720.0);
        s.background = Color::from_rgb8(0x5A, 0x8F, 0xC8);
        s.frame_rate = 24.0;
    }
    let ground = scene.add_stage_layer("Ground", buzz_scene::LayerKind::Normal);
    let id = scene.next_object_id();
    let rect = kurbo::Rect::new(0.0, 470.0, 1280.0, 720.0);
    let shape = buzz_scene::Object::shape(
        id,
        buzz_scene::ShapeData::filled(kurbo::Shape::to_path(&rect, 0.1), Color::from_rgb8(0x4E, 0x8A, 0x3C)),
    );
    scene.update_layer(ground, |l| {
        l.frames.set_objects(0, vec![std::sync::Arc::new(shape)]);
        while l.frames.length() < 240 {
            l.frames.insert_frame(l.frames.length());
        }
    });
    scene
}

fn spec_for(text: &str, preset: TitlePreset) -> TitleSpec {
    let mut spec = TitleSpec::new(text);
    spec.preset = preset;
    spec.size = 72.0;
    spec.hold_seconds = 1.0;
    spec.start = 0;
    // A plate on two of the five, to see it come and go.
    if matches!(preset, TitlePreset::PopWords | TitlePreset::SlideIn) {
        spec.plate = Some(TitlePlate { color: Color::from_rgb8(0x14, 0x14, 0x1C), opacity: 0.7 });
    }
    if preset == TitlePreset::SlideIn {
        spec.position = TitlePosition::LowerThird;
    }
    spec
}

fn paste(
    exporter: &mut Exporter,
    sheet: &mut RgbaImage,
    scene: &Scene,
    frame: u32,
    (x0, y0): (u32, u32),
    title: &str,
) -> anyhow::Result<()> {
    let mut settings = ExportSettings::for_stage(scene);
    settings.width = CELL_W;
    settings.height = CELL_H;
    let picture = exporter.render(scene, frame, &settings)?;
    let cell = RgbaImage::from_raw(picture.width, picture.height, picture.pixels)
        .ok_or_else(|| anyhow::anyhow!("the rendered cell has the wrong buffer size"))?;
    for (x, y, p) in cell.enumerate_pixels() {
        sheet.put_pixel(x0 + x, y0 + LABEL_H + y, Rgba([p[0], p[1], p[2], 255]));
    }
    label(sheet, title, x0 + 6, y0 + 4, 2);
    Ok(())
}

fn sheet(exporter: &mut Exporter, text: &str, file: &std::path::Path) -> anyhow::Result<()> {
    let cols = 4u32;
    let rows = TitlePreset::ALL.len() as u32;
    let mut img = RgbaImage::from_pixel(cols * CELL_W, rows * (CELL_H + LABEL_H), Rgba([0xF2, 0xF0, 0xEA, 255]));
    for (r, preset) in TitlePreset::ALL.into_iter().enumerate() {
        let mut scene = stage();
        let card = title::add_title_card(&mut scene, &spec_for(text, preset))
            .map_err(|e| anyhow::anyhow!("{preset:?}: {e}"))?;
        let in_len = title::in_frames(preset, card.units);
        let out_start = card.frames - title::OUT_FRAMES;
        let picks = [
            ("IN", in_len / 3),
            ("MID", (in_len * 2) / 3),
            ("HOLD", (in_len + out_start) / 2),
            ("OUT", out_start + title::OUT_FRAMES / 2),
        ];
        println!(
            "{preset:?}: {} units, in {in_len} f, card {} f, picks {:?}",
            card.units,
            card.frames,
            picks.iter().map(|p| p.1).collect::<Vec<_>>()
        );
        for (c, (name, frame)) in picks.into_iter().enumerate() {
            let tag = format!("{} {name} F{frame}", preset.id().replace('_', " "));
            paste(exporter, &mut img, &scene, frame, (c as u32 * CELL_W, r as u32 * (CELL_H + LABEL_H)), &tag)?;
        }
        // The last frame of the card must be transparent: compare it with the
        // stage drawn without the card.
        let last = card.frames - 1;
        let mut settings = ExportSettings::for_stage(&scene);
        settings.width = CELL_W;
        settings.height = CELL_H;
        let with = exporter.render(&scene, last, &settings)?;
        let bare = exporter.render(&stage(), last, &settings)?;
        let diff = with
            .pixels
            .iter()
            .zip(&bare.pixels)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        println!("  last frame {last}: max diff vs bare stage = {diff}");
    }
    img.save(file)?;
    println!("Wrote {}", file.display());
    Ok(())
}

fn typewriter_strip(exporter: &mut Exporter, text: &str, file: &std::path::Path) -> anyhow::Result<()> {
    let mut scene = stage();
    let card = title::add_title_card(&mut scene, &spec_for(text, TitlePreset::Typewriter))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let steps = card.units as u32;
    let cols = 4u32;
    let rows = steps.div_ceil(cols);
    let mut img = RgbaImage::from_pixel(cols * CELL_W, rows * (CELL_H + LABEL_H), Rgba([0xF2, 0xF0, 0xEA, 255]));
    for k in 0..steps {
        let frame = 2 * k;
        let (c, r) = (k % cols, k / cols);
        paste(exporter, &mut img, &scene, frame, (c * CELL_W, r * (CELL_H + LABEL_H)), &format!("F{frame}"))?;
    }
    img.save(file)?;
    println!("Wrote {}", file.display());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".to_string()));
    std::fs::create_dir_all(&out)?;
    let english = "Chirailiya \u{2014} Episode 1";
    let hindi = "चिरैलिया \u{2014} नये पड़ोसी";
    for text in [english, hindi, "क्षत्रिय नमस्ते", "संस्कृत हिन्दी पत्नी"] {
        let units = title::units(text, 72.0, None, TitlePreset::Typewriter);
        let words = title::units(text, 72.0, None, TitlePreset::PopWords);
        println!(
            "{text}: typewriter {:?} | words {:?} | font {:?}",
            units.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
            words.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
            title::family_for(text, None)
        );
    }
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    sheet(&mut exporter, english, &out.join("titles-en.png"))?;
    sheet(&mut exporter, hindi, &out.join("titles-hi.png"))?;
    typewriter_strip(&mut exporter, hindi, &out.join("typewriter-hi.png"))?;
    typewriter_strip(&mut exporter, "क्षत्रिय नमस्ते", &out.join("typewriter-kshatriya.png"))?;
    Ok(())
}
