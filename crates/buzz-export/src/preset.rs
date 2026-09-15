//! Export presets: a named delivery target you set up once.
//!
//! "YouTube 1080p", "master file", "GIF preview" — each is a bundle of format,
//! size and quality that an animator reaches for again and again. A preset
//! saves choosing all of it every time.
//!
//! # Why presets belong to the person, not the document
//!
//! A preset encodes where a film is *going* — a website, an editor, a client —
//! which is a fact about the animator's world, not about this particular film.
//! So presets are stored app-level (in `export_presets.json`, beside the dock
//! layout) exactly as the workspace and the theme are, and no document format
//! version moves when one is added. A `.buzz` handed to somebody else carries
//! none of them, which is right.
//!
//! # Why a target *height*, not a fixed size
//!
//! This is the one deliberate departure from the sketch in `ARCHITECTURE.md`,
//! which had a preset carry a whole `ExportSettings` with absolute pixels. A
//! preset that hard-codes 1920×1080 distorts — or letterboxes — every stage
//! that is not exactly 16:9, and Animate's own default stage is 550×400, which
//! is not. So a preset carries a target **height** and the width is taken from
//! the document's aspect when the preset is applied. "1080p" then means "1080
//! lines, shaped like this film" rather than "these exact pixels, whatever the
//! film".

use serde::{Deserialize, Serialize};

/// What a preset produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetFormat {
    /// A single PNG of the current frame.
    Png,
    /// A numbered PNG per frame.
    PngSequence,
    Mp4H264,
    Mp4Hevc,
    Mp4Av1,
    MovHevc,
    Gif,
    Webp,
}

impl PresetFormat {
    /// Does producing this need an ffmpeg?
    pub fn needs_ffmpeg(self) -> bool {
        !matches!(self, Self::Png | Self::PngSequence)
    }
}

/// A named bundle of export choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub format: PresetFormat,
    /// Target height in pixels; the width follows the document's aspect. `None`
    /// keeps the stage's own size.
    pub height: Option<u32>,
    /// Quality, meaning per format: for video it is ffmpeg's CRF/CQ scale
    /// (lower is better), for WebP it is `0..=100` (higher is better), and for
    /// GIF and PNG it is ignored.
    pub quality: u32,
    pub transparent: bool,
    /// Mux the soundtrack, for video.
    pub audio: bool,
    /// Encode on the GPU where possible, for video.
    pub hardware: bool,
    /// Keep every pixel exactly, for WebP.
    pub lossless: bool,
    /// A built-in ships with the program and cannot be deleted; a user preset
    /// can. Skipped from the file so only user presets are ever written.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
}

impl ExportPreset {
    /// The presets that ship with the program.
    ///
    /// Three, chosen to cover the three things people actually export for:
    /// something to upload, something to keep, and something to paste into a
    /// chat window.
    pub fn built_ins() -> Vec<ExportPreset> {
        vec![
            ExportPreset {
                name: "YouTube 1080p".into(),
                format: PresetFormat::Mp4H264,
                height: Some(1080),
                // H.264 at CRF 20 is visually clean for line artwork and plays
                // everywhere, which is the whole point of the preset.
                quality: 20,
                transparent: false,
                audio: true,
                hardware: true,
                lossless: false,
                builtin: true,
            },
            ExportPreset {
                name: "Master (HEVC, high quality)".into(),
                format: PresetFormat::MovHevc,
                // The stage's own resolution: a master is kept to re-encode
                // from, so it should not be scaled at all.
                height: None,
                quality: 14,
                transparent: false,
                audio: true,
                hardware: true,
                lossless: false,
                builtin: true,
            },
            ExportPreset {
                name: "GIF preview 480p".into(),
                format: PresetFormat::Gif,
                height: Some(480),
                quality: 0,
                transparent: false,
                audio: false,
                hardware: false,
                lossless: false,
                builtin: true,
            },
        ]
    }

    /// The output size for a given stage, keeping the stage's aspect.
    ///
    /// Returns even dimensions, because H.264 and HEVC refuse odd ones and a
    /// preset that produced an un-encodable size would be a trap.
    pub fn resolve_size(&self, stage: (u32, u32)) -> (u32, u32) {
        let (sw, sh) = (stage.0.max(1), stage.1.max(1));
        let (w, h) = match self.height {
            Some(height) => {
                let height = height.max(1);
                let width = (u64::from(sw) * u64::from(height) / u64::from(sh)) as u32;
                (width.max(1), height)
            }
            None => (sw, sh),
        };
        (even(w), even(h))
    }
}

/// Round up to the nearest even number: the video encoders need it, and it is
/// harmless for the formats that do not.
fn even(n: u32) -> u32 {
    n + (n & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_all_builtin_and_named() {
        let built = ExportPreset::built_ins();
        assert!(built.len() >= 3);
        assert!(built.iter().all(|p| p.builtin));
        assert!(built.iter().all(|p| !p.name.is_empty()));
    }

    /// A target height keeps the stage's aspect rather than a fixed 16:9.
    #[test]
    fn height_keeps_the_stage_aspect() {
        let preset = ExportPreset {
            height: Some(1080),
            ..ExportPreset::built_ins()[0].clone()
        };
        // A 4:3 stage becomes 1440×1080, not 1920×1080.
        assert_eq!(preset.resolve_size((800, 600)), (1440, 1080));
        // A 16:9 stage becomes 1920×1080.
        assert_eq!(preset.resolve_size((1280, 720)), (1920, 1080));
    }

    /// Odd results are rounded up so the video encoders accept them.
    #[test]
    fn sizes_come_out_even() {
        let preset = ExportPreset {
            height: Some(481),
            ..ExportPreset::built_ins()[2].clone()
        };
        let (w, h) = preset.resolve_size((551, 401));
        assert_eq!(h % 2, 0);
        assert_eq!(w % 2, 0);
    }

    #[test]
    fn none_height_keeps_stage_size() {
        let preset = ExportPreset {
            height: None,
            ..ExportPreset::built_ins()[1].clone()
        };
        assert_eq!(preset.resolve_size((640, 480)), (640, 480));
    }

    /// Built-ins are not written; user presets are. That is what keeps the file
    /// to what the user actually added.
    #[test]
    fn only_user_presets_serialise_their_flag() {
        let user = ExportPreset {
            name: "Mine".into(),
            builtin: false,
            ..ExportPreset::built_ins()[0].clone()
        };
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("builtin"), "the false flag is skipped: {json}");

        let round: ExportPreset = serde_json::from_str(&json).unwrap();
        assert!(!round.builtin);
        assert_eq!(round.name, "Mine");
    }
}
