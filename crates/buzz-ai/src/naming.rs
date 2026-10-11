//! Identify and rename assets using local VLM (Vision-Language Model).
//!
//! Chunk C8 specifications:
//! - Render thumbnails and send to local VLM in batches of 8 with JSON schema:
//!   `[{id, name(snake_case), category: character|prop|background|vehicle|animal|effect|other, view: front|side|back|3q|na, tags:[..]}]`.
//! - Review dialog listing old name -> new name with checkboxes applied in one undoable edit.
//! - Tags stored on assets (`Symbol::tags`, `Asset::tags`).

use serde::{Deserialize, Serialize};
use buzz_scene::{Scene, SymbolId};

use crate::llm::{Llm, LlmError};

/// Category classification of an asset or symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetCategory {
    Character,
    Prop,
    Background,
    Vehicle,
    Animal,
    Effect,
    Other,
}

impl AssetCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Prop => "prop",
            Self::Background => "background",
            Self::Vehicle => "vehicle",
            Self::Animal => "animal",
            Self::Effect => "effect",
            Self::Other => "other",
        }
    }
}

/// Perspective view angle of an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetView {
    Front,
    Side,
    Back,
    #[serde(rename = "3q")]
    ThreeQuarter,
    #[serde(rename = "na")]
    Na,
}

impl AssetView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Side => "side",
            Self::Back => "back",
            Self::ThreeQuarter => "3q",
            Self::Na => "na",
        }
    }
}

/// Schema-matching output of VLM for a single asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifiedAsset {
    /// Identifier corresponding to the input thumbnail ID.
    pub id: String,
    /// Semantic, descriptive name in snake_case.
    pub name: String,
    pub category: AssetCategory,
    pub view: AssetView,
    pub tags: Vec<String>,
}

/// Input thumbnail for identification.
#[derive(Debug, Clone)]
pub struct AssetThumbnailItem {
    pub id: String,
    pub old_name: String,
    pub png_bytes: Vec<u8>,
}

/// Review item presented to user in the rename dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct RenameProposal {
    pub id: String,
    pub old_name: String,
    pub new_name: String,
    pub category: AssetCategory,
    pub view: AssetView,
    pub tags: Vec<String>,
    pub selected: bool,
}

/// Returns the strict JSON schema required for VLM asset identification.
pub fn asset_identification_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "category": {
                    "type": "string",
                    "enum": ["character", "prop", "background", "vehicle", "animal", "effect", "other"]
                },
                "view": {
                    "type": "string",
                    "enum": ["front", "side", "back", "3q", "na"]
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["id", "name", "category", "view", "tags"],
            "additionalProperties": false
        }
    })
}

/// Identifies a single batch of up to 8 asset thumbnails using a local VLM.
pub fn identify_batch(
    llm: &impl Llm,
    model: &str,
    batch: &[AssetThumbnailItem],
) -> Result<Vec<IdentifiedAsset>, LlmError> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    assert!(batch.len() <= 8, "Batch size must be at most 8");

    let system_prompt = "You are a professional 2D animation asset cataloguer. \
Identify the objects, characters, or scenery shown in the provided image thumbnails. \
Return a JSON array containing an entry for each thumbnail with: \
- id: the exact ID corresponding to the image index \
- name: a concise, descriptive snake_case name (e.g. 'hero_sword_side', 'old_oak_tree', 'warrior_head_front') \
- category: one of character, prop, background, vehicle, animal, effect, other \
- view: one of front, side, back, 3q, na \
- tags: a list of 2-5 relevant keyword tags.";

    let mut user_prompt = String::from("Analyze the following images in sequence:\n");
    for (idx, item) in batch.iter().enumerate() {
        user_prompt.push_str(&format!(
            "Image {}: id='{}', current_name='{}'\n",
            idx + 1,
            item.id,
            item.old_name
        ));
    }
    user_prompt.push_str("\nOutput a valid JSON array according to the schema.");

    let images_refs: Vec<&[u8]> = batch.iter().map(|item| item.png_bytes.as_slice()).collect();
    let schema = asset_identification_schema();

    let identified: Vec<IdentifiedAsset> = llm.chat_json(
        model,
        system_prompt,
        &user_prompt,
        &images_refs,
        schema,
    )?;

    Ok(identified)
}

/// Identifies all given assets by splitting into batches of 8 and querying the VLM.
pub fn identify_all_assets(
    llm: &impl Llm,
    model: &str,
    items: &[AssetThumbnailItem],
    mut progress_callback: impl FnMut(usize, usize),
) -> Result<Vec<RenameProposal>, LlmError> {
    let mut proposals = Vec::with_capacity(items.len());
    let total = items.len();
    let mut processed = 0;

    for chunk in items.chunks(8) {
        match identify_batch(llm, model, chunk) {
            Ok(identified_list) => {
                for item in chunk {
                    if let Some(identified) = identified_list.iter().find(|id_res| id_res.id == item.id) {
                        proposals.push(RenameProposal {
                            id: item.id.clone(),
                            old_name: item.old_name.clone(),
                            new_name: identified.name.clone(),
                            category: identified.category,
                            view: identified.view,
                            tags: identified.tags.clone(),
                            selected: true,
                        });
                    } else {
                        // Fallback: keep old name
                        proposals.push(RenameProposal {
                            id: item.id.clone(),
                            old_name: item.old_name.clone(),
                            new_name: item.old_name.clone(),
                            category: AssetCategory::Other,
                            view: AssetView::Na,
                            tags: Vec::new(),
                            selected: false,
                        });
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Batch identification failed: {e}; falling back for batch");
                for item in chunk {
                    proposals.push(RenameProposal {
                        id: item.id.clone(),
                        old_name: item.old_name.clone(),
                        new_name: item.old_name.clone(),
                        category: AssetCategory::Other,
                        view: AssetView::Na,
                        tags: Vec::new(),
                        selected: false,
                    });
                }
            }
        }
        processed += chunk.len();
        progress_callback(processed, total);
    }

    Ok(proposals)
}

/// Applies rename proposals to symbols in a scene library in one operation.
///
/// New names are made unique against the library first — "tree", "tree 2",
/// and so on — so two symbols the VLM described alike do not collide into one
/// name, and a suggested name does not silently steal another symbol's.
///
/// Returns the count of renamed symbols.
pub fn apply_renames_to_scene(scene: &mut Scene, proposals: &[RenameProposal]) -> usize {
    let mut changed = 0;
    let mut taken: std::collections::HashSet<String> =
        scene.library().iter().map(|s| s.name.clone()).collect();

    for prop in proposals {
        if !prop.selected {
            continue;
        }
        let Ok(sym_num) = prop.id.parse::<u64>() else {
            continue;
        };
        let sym_id = SymbolId(sym_num);

        // The symbol's own current name does not count against itself — it is
        // about to be replaced, not kept alongside the new one.
        taken.remove(&prop.old_name);
        let unique_name = unique_name(&prop.new_name, &taken);

        let updated = scene.library_mut().update(sym_id, |sym| {
            sym.name = unique_name.clone();
            for t in &prop.tags {
                if !sym.tags.contains(t) {
                    sym.tags.push(t.clone());
                }
            }
        });
        if updated {
            taken.insert(unique_name);
            changed += 1;
        } else {
            // Nothing actually changed — the old name is still the one in use.
            taken.insert(prop.old_name.clone());
        }
    }
    changed
}

/// `name`, or `name 2`, `name 3`, … — whichever is not already in `taken`.
fn unique_name(name: &str, taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name} {n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}
