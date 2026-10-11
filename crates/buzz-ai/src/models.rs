//! Model registry, download management, and SHA-256 verification.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Identifier for each AI model used by BuzzAnimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelId {
    /// Anime character segmentation / matte without alpha (ISNet-anime).
    AnimeSegmentation,
    /// DWPose person bounding box detector (YOLOX-l).
    DwPoseDetector,
    /// DWPose whole-body 133-keypoint estimator (dw-ll_ucoco_384).
    DwPoseEstimator,
    /// Segment Anything 2.1 hiera-tiny image encoder.
    Sam2Encoder,
    /// Segment Anything 2.1 hiera-tiny mask decoder.
    Sam2Decoder,
    /// LaMa (large-mask inpainting) for raster completion.
    LaMa,
    /// SigLIP vision encoder for image-to-asset similarity.
    SigLip,
    /// Whisper base.en speech-to-text (encoder; the decoder and the vocabulary
    /// ride along as extra files). F7.9.
    Whisper,
    /// Whisper small, multilingual (int8): the "Hey Buzz" voice commands in
    /// English, Hindi and Hinglish. Stage V2.
    WhisperMulti,
}

impl ModelId {
    pub fn all() -> &'static [ModelId] {
        &[
            ModelId::AnimeSegmentation,
            ModelId::DwPoseDetector,
            ModelId::DwPoseEstimator,
            ModelId::Sam2Encoder,
            ModelId::Sam2Decoder,
            ModelId::LaMa,
            ModelId::SigLip,
            ModelId::Whisper,
            ModelId::WhisperMulti,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::AnimeSegmentation => "Anime Segmentation (ISNet)",
            Self::DwPoseDetector => "DWPose Detector (YOLOX-l)",
            Self::DwPoseEstimator => "DWPose Estimator (133 kp)",
            Self::Sam2Encoder => "SAM 2.1 Encoder (Hiera-Tiny)",
            Self::Sam2Decoder => "SAM 2.1 Decoder (Hiera-Tiny)",
            Self::LaMa => "LaMa Inpainting",
            Self::SigLip => "SigLIP Vision Encoder",
            Self::Whisper => "Whisper base.en (speech to text)",
            Self::WhisperMulti => "Whisper small multilingual (voice commands)",
        }
    }
}

/// An additional file that must be downloaded and verified alongside a
/// model's main file (e.g. ONNX external-data weights), placed in the same
/// directory under its own `filename`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraFile {
    pub filename: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// A pinned entry in the model registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: ModelId,
    pub filename: &'static str,
    pub url: &'static str,
    /// Hex-encoded SHA-256 digest. If empty, SHA verification is skipped (e.g. testing).
    pub sha256: &'static str,
    pub bytes: u64,
    pub licence: &'static str,
    pub description: &'static str,
    /// Extra files (e.g. ONNX external-data `.onnx_data`) downloaded and
    /// verified alongside the main file, into the same directory.
    pub extra_files: &'static [ExtraFile],
}

/// The built-in registry of official models.
///
/// URLs, SHA-256 digests, and byte counts verified against the Hugging Face
/// API on 2026-09-24 (see `docs/LOCAL_AI_PLAN.md`, "C0-fix + real
/// verification of C1-C4").
pub static REGISTRY: &[ModelEntry] = &[
    ModelEntry {
        id: ModelId::AnimeSegmentation,
        filename: "isnetis.onnx",
        url: "https://huggingface.co/skytnt/anime-seg/resolve/main/isnetis.onnx",
        sha256: "f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99",
        bytes: 176_069_933,
        licence: "Apache-2.0",
        description: "Matte extraction for character art without an alpha channel.",
        extra_files: &[],
    },
    ModelEntry {
        id: ModelId::DwPoseDetector,
        filename: "yolox_l.onnx",
        url: "https://huggingface.co/yzd-v/DWPose/resolve/main/yolox_l.onnx",
        sha256: "7860ae79de6c89a3c1eb72ae9a2756c0ccfbe04b7791bb5880afabd97855a411",
        bytes: 216_746_733,
        licence: "Apache-2.0",
        description: "Detects human character bounding boxes for pose estimation.",
        extra_files: &[],
    },
    ModelEntry {
        id: ModelId::DwPoseEstimator,
        filename: "dw-ll_ucoco_384.onnx",
        url: "https://huggingface.co/yzd-v/DWPose/resolve/main/dw-ll_ucoco_384.onnx",
        sha256: "724f4ff2439ed61afb86fb8a1951ec39c6220682803b4a8bd4f598cd913b1843",
        bytes: 134_399_116,
        licence: "Apache-2.0",
        description: "Extracts 133 whole-body keypoints (body, face, hands, feet).",
        extra_files: &[],
    },
    ModelEntry {
        id: ModelId::Sam2Encoder,
        filename: "vision_encoder.onnx",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/vision_encoder.onnx",
        sha256: "4f30aacd3aaefbca81a0b7fe4c1fc96345570ea0a6f80ced599493d1b3be2e8c",
        bytes: 354_238,
        licence: "Apache-2.0",
        description: "Image feature encoder for interactive segment anything masks (transformers.js layout).",
        extra_files: &[ExtraFile {
            filename: "vision_encoder.onnx_data",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/vision_encoder.onnx_data",
            sha256: "e83df9866a5afe68ea7f0f721f18f65137fc3acbf0da1c74e946d363e09c69cc",
            bytes: 134_084_864,
        }],
    },
    ModelEntry {
        id: ModelId::Sam2Decoder,
        filename: "prompt_encoder_mask_decoder.onnx",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx",
        sha256: "874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf",
        bytes: 213_114,
        licence: "Apache-2.0",
        description: "Mask decoder generating character part segmentations from prompts.",
        extra_files: &[ExtraFile {
            filename: "prompt_encoder_mask_decoder.onnx_data",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx_data",
            sha256: "e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47",
            bytes: 20_958_208,
        }],
    },
    ModelEntry {
        id: ModelId::LaMa,
        filename: "lama_fp32.onnx",
        url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
        sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
        bytes: 208_044_816,
        licence: "Apache-2.0",
        description: "Inpaints hidden and occluded raster areas behind foreground limbs.",
        extra_files: &[],
    },
    ModelEntry {
        id: ModelId::SigLip,
        filename: "vision_model_quantized.onnx",
        url: "https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/onnx/vision_model_quantized.onnx",
        sha256: "ef14a954f3d57e1806666432bd9785004c1dc27100aa260eee0cb0f10a5de058",
        bytes: 99_499_129,
        licence: "Apache-2.0",
        description: "Vision embedding model for matching scene reference objects to assets (C10, later).",
        extra_files: &[],
    },
    ModelEntry {
        id: ModelId::Whisper,
        filename: "whisper-base.en_encoder_model.onnx",
        url: "https://huggingface.co/Xenova/whisper-base.en/resolve/main/onnx/encoder_model.onnx",
        sha256: "3af4d8b7515f01f1c313afe0d691768ff084169b8e1ec1cb4d975ce2ac465f6e",
        bytes: 82_474_863,
        licence: "MIT",
        description: "Speech to text for captions and lip sync (F7.9): Whisper base.en encoder, decoder and vocabulary.",
        extra_files: &[
            ExtraFile {
                filename: "whisper-base.en_decoder_model.onnx",
                url: "https://huggingface.co/Xenova/whisper-base.en/resolve/main/onnx/decoder_model.onnx",
                sha256: "803d14c05a61b48b3993d2be821e39eb6e4de2205348ec31408776f7cf1cfef9",
                bytes: 208_302_147,
            },
            ExtraFile {
                filename: "whisper-base.en_vocab.json",
                url: "https://huggingface.co/Xenova/whisper-base.en/resolve/main/vocab.json",
                sha256: "f6bd25a65e4e63ca31360e9fb11c7e4f9a391a78385d640acd814092dd6eee4f",
                bytes: 999_186,
            },
        ],
    },
    ModelEntry {
        id: ModelId::WhisperMulti,
        filename: "whisper-small_encoder_model_quantized.onnx",
        url: "https://huggingface.co/Xenova/whisper-small/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "969f5ac12974340386bf7a02ea6626003e5e2dee396ffc6ab0eec282bf55ba06",
        bytes: 92_324_809,
        licence: "MIT",
        description: "Speech to text for \"Hey Buzz\" voice commands in English, Hindi and Hinglish: Whisper small multilingual (int8) encoder, decoder and tokenizer.",
        extra_files: &[
            ExtraFile {
                filename: "whisper-small_decoder_model_quantized.onnx",
                url: "https://huggingface.co/Xenova/whisper-small/resolve/main/onnx/decoder_model_quantized.onnx",
                sha256: "1514de20d1d3f177486c76260ee943849e483f4971bd7285d060050c2d67d485",
                bytes: 155_991_279,
            },
            ExtraFile {
                filename: "whisper-small_vocab.json",
                url: "https://huggingface.co/Xenova/whisper-small/resolve/main/vocab.json",
                sha256: "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a",
                bytes: 1_036_584,
            },
            ExtraFile {
                filename: "whisper-small_added_tokens.json",
                url: "https://huggingface.co/Xenova/whisper-small/resolve/main/added_tokens.json",
                sha256: "ce949fe720c14311cb6c446e69cfe340dc669d7b006077a6feed6ae571dd7e88",
                bytes: 2_082,
            },
        ],
    },
];

/// Returns the registry entry for a model ID.
pub fn get_entry(id: ModelId) -> &'static ModelEntry {
    REGISTRY
        .iter()
        .find(|e| e.id == id)
        .expect("ModelId must exist in REGISTRY")
}

/// Directory where downloaded ONNX models are stored.
pub fn models_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("BUZZANIMATE_MODELS_DIR") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("models")
}

/// Status of an ONNX model file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    /// File does not exist on disk.
    Missing,
    /// File exists and is ready for use.
    Ready { path: PathBuf, bytes: u64 },
    /// File exists but failed size or checksum verification.
    Corrupted(String),
}

impl ModelStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Checks the status of a model on disk.
pub fn status(id: ModelId) -> ModelStatus {
    let entry = get_entry(id);
    let path = models_dir().join(entry.filename);
    if !path.exists() {
        return ModelStatus::Missing;
    }
    // A model is only usable with all of its extra files (external weights,
    // Whisper's decoder and vocabulary).
    if entry
        .extra_files
        .iter()
        .any(|x| !models_dir().join(x.filename).exists())
    {
        return ModelStatus::Missing;
    }
    match fs::metadata(&path) {
        Ok(meta) => {
            let bytes = meta.len();
            if bytes == 0 {
                return ModelStatus::Corrupted("File is empty (0 bytes)".into());
            }
            ModelStatus::Ready { path, bytes }
        }
        Err(e) => ModelStatus::Corrupted(format!("Failed to read metadata: {e}")),
    }
}

/// Errors that can occur during model verification or downloading.
#[derive(Debug, Error)]
pub enum ModelError {
    #[error("Download cancelled by user")]
    Cancelled,
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SHA-256 mismatch for {filename}: expected {expected}, got {got}")]
    ShaMismatch {
        filename: String,
        expected: String,
        got: String,
    },
    #[error("Registry error: {0}")]
    Other(String),
}

/// Computes the hex-encoded SHA-256 hash of a file.
pub fn compute_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Ensures the model is downloaded and verified in `models_dir()`.
///
/// If already present and valid, returns `Ok(path)`.
/// Otherwise, downloads into `<filename>.part`, checking `cancel()` periodically,
/// verifies the SHA-256 digest, and atomically renames to `<filename>`.
/// If cancelled, errored, or if SHA-256 fails, the `.part` file is removed.
pub fn ensure<F, C>(id: ModelId, mut progress: F, cancel: C) -> Result<PathBuf, ModelError>
where
    F: FnMut(u64, u64),
    C: Fn() -> bool,
{
    let entry = get_entry(id);
    ensure_entry(entry, &models_dir(), &mut progress, &cancel)
}

/// Internal implementation of ensure given a specific entry and destination directory.
///
/// Downloads and verifies `entry`'s main file, then any `entry.extra_files`
/// (e.g. ONNX external-data weights), all into `dir`. Returns the path to
/// the main file.
pub fn ensure_entry<F, C>(
    entry: &ModelEntry,
    dir: &Path,
    progress: &mut F,
    cancel: &C,
) -> Result<PathBuf, ModelError>
where
    F: FnMut(u64, u64),
    C: Fn() -> bool,
{
    let target = download_and_verify(entry.filename, entry.url, entry.sha256, entry.bytes, dir, progress, cancel)?;
    for extra in entry.extra_files {
        download_and_verify(extra.filename, extra.url, extra.sha256, extra.bytes, dir, progress, cancel)?;
    }
    Ok(target)
}

/// Downloads and SHA-256-verifies a single file into `dir`, atomically via a `.part` file.
/// If the file already exists and matches the pinned SHA-256 (or none is pinned), returns early.
#[allow(clippy::too_many_arguments)]
fn download_and_verify<F, C>(
    filename: &str,
    url: &str,
    sha256: &str,
    default_bytes: u64,
    dir: &Path,
    progress: &mut F,
    cancel: &C,
) -> Result<PathBuf, ModelError>
where
    F: FnMut(u64, u64),
    C: Fn() -> bool,
{
    fs::create_dir_all(dir)?;
    let target = dir.join(filename);
    if target.exists() {
        if let Ok(meta) = fs::metadata(&target) {
            if meta.len() > 0 {
                // If sha256 is pinned, verify it
                if !sha256.is_empty() {
                    match compute_sha256(&target) {
                        Ok(hash) if hash.eq_ignore_ascii_case(sha256) => {
                            return Ok(target);
                        }
                        Ok(hash) => {
                            tracing::warn!(
                                "Existing model {:?} has SHA mismatch ({hash} != {}), redownloading",
                                target,
                                sha256
                            );
                            let _ = fs::remove_file(&target);
                        }
                        Err(_) => {
                            let _ = fs::remove_file(&target);
                        }
                    }
                } else {
                    return Ok(target);
                }
            }
        }
    }

    let part_path = dir.join(format!("{filename}.part"));
    // Remove stale part file if any
    let _ = fs::remove_file(&part_path);

    if cancel() {
        return Err(ModelError::Cancelled);
    }

    tracing::info!("Downloading model {} from {}", filename, url);
    let resp = ureq::get(url)
        .call()
        .map_err(|e| ModelError::Http(e.to_string()))?;

    let total_bytes = resp
        .header("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default_bytes);

    let mut reader = resp.into_reader();
    let mut file = match File::create(&part_path) {
        Ok(f) => f,
        Err(e) => return Err(ModelError::Io(e)),
    };

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 64 * 1024];

    loop {
        if cancel() {
            drop(file);
            let _ = fs::remove_file(&part_path);
            return Err(ModelError::Cancelled);
        }

        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(file);
                let _ = fs::remove_file(&part_path);
                return Err(ModelError::Io(e));
            }
        };

        if let Err(e) = file.write_all(&buf[..n]) {
            drop(file);
            let _ = fs::remove_file(&part_path);
            return Err(ModelError::Io(e));
        }

        hasher.update(&buf[..n]);
        downloaded += n as u64;
        progress(downloaded, total_bytes);
    }

    if let Err(e) = file.flush() {
        drop(file);
        let _ = fs::remove_file(&part_path);
        return Err(ModelError::Io(e));
    }
    drop(file);

    let got_sha = format!("{:x}", hasher.finalize());
    if !sha256.is_empty() && !got_sha.eq_ignore_ascii_case(sha256) {
        let _ = fs::remove_file(&part_path);
        return Err(ModelError::ShaMismatch {
            filename: filename.to_string(),
            expected: sha256.to_string(),
            got: got_sha,
        });
    }

    // Rename part file to final file
    if let Err(e) = fs::rename(&part_path, &target) {
        let _ = fs::remove_file(&part_path);
        return Err(ModelError::Io(e));
    }

    tracing::info!("Model {} verified and ready at {:?}", filename, target);
    Ok(target)
}
