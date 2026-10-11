//! Integration and verification tests for Chunk C8: Identify & Rename Assets.
//!
//! Validates:
//! - Local VLM batching of up to 8 thumbnails with JSON schema.
//! - Strict schema compliance for `id`, `name(snake_case)`, `category`, `view`, and `tags`.
//! - Review proposal generation, progress tracking, and batching constraints.
//! - One-edit application of names and tags to `Scene` and `AssetLibrary`.
//! - Persistence and roundtripping of `tags` on `Symbol` through document serialization.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use buzz_ai::llm::{Llm, LlmError};
use buzz_ai::naming::{
    apply_renames_to_scene, asset_identification_schema, identify_all_assets, identify_batch,
    AssetCategory, AssetThumbnailItem, AssetView, RenameProposal,
};
use buzz_scene::{Scene, Symbol, SymbolId, SymbolKind};

/// Mock VLM implementation for testing structured JSON chat and batch constraints.
struct MockVlm {
    batch_sizes: Mutex<Vec<usize>>,
    call_count: AtomicUsize,
    responses: Mutex<Vec<serde_json::Value>>,
}

impl MockVlm {
    fn new(responses: Vec<serde_json::Value>) -> Self {
        Self {
            batch_sizes: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            responses: Mutex::new(responses),
        }
    }
}

impl Llm for MockVlm {
    fn chat_json<T: serde::de::DeserializeOwned>(
        &self,
        _model: &str,
        _system: &str,
        _user: &str,
        images: &[&[u8]],
        _schema: serde_json::Value,
    ) -> Result<T, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.batch_sizes.lock().unwrap().push(images.len());

        let mut resp_lock = self.responses.lock().unwrap();
        if !resp_lock.is_empty() {
            let val = resp_lock.remove(0);
            serde_json::from_value(val).map_err(|e| LlmError::InvalidJson {
                raw: String::new(),
                error: e.to_string(),
            })
        } else {
            Err(LlmError::EmptyResponse)
        }
    }

    fn embed(&self, _model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
        Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
    }
}

#[test]
fn test_asset_identification_schema_structure() {
    let schema = asset_identification_schema();
    assert_eq!(schema["type"], "array");
    let items = &schema["items"];
    assert_eq!(items["type"], "object");
    let props = &items["properties"];
    assert!(props.get("id").is_some());
    assert!(props.get("name").is_some());
    assert!(props.get("category").is_some());
    assert!(props.get("view").is_some());
    assert!(props.get("tags").is_some());

    let categories = props["category"]["enum"].as_array().expect("category enum array");
    let cat_strings: Vec<&str> = categories.iter().filter_map(|c| c.as_str()).collect();
    assert!(cat_strings.contains(&"character"));
    assert!(cat_strings.contains(&"prop"));
    assert!(cat_strings.contains(&"background"));
    assert!(cat_strings.contains(&"vehicle"));
    assert!(cat_strings.contains(&"animal"));
    assert!(cat_strings.contains(&"effect"));
    assert!(cat_strings.contains(&"other"));

    let views = props["view"]["enum"].as_array().expect("view enum array");
    let view_strings: Vec<&str> = views.iter().filter_map(|v| v.as_str()).collect();
    assert!(view_strings.contains(&"front"));
    assert!(view_strings.contains(&"side"));
    assert!(view_strings.contains(&"back"));
    assert!(view_strings.contains(&"3q"));
    assert!(view_strings.contains(&"na"));
}

#[test]
fn test_identify_batch_with_mock_vlm() {
    let mock_json = serde_json::json!([
        {
            "id": "10",
            "name": "hero_sword_side",
            "category": "prop",
            "view": "side",
            "tags": ["weapon", "iron", "hero"]
        },
        {
            "id": "20",
            "name": "warrior_head_front",
            "category": "character",
            "view": "front",
            "tags": ["warrior", "head", "face"]
        },
        {
            "id": "30",
            "name": "pine_tree_3q",
            "category": "background",
            "view": "3q",
            "tags": ["tree", "nature", "foliage"]
        }
    ]);

    let mock_llm = MockVlm::new(vec![mock_json]);

    let items = vec![
        AssetThumbnailItem {
            id: "10".into(),
            old_name: "Symbol 10".into(),
            png_bytes: vec![1, 2, 3],
        },
        AssetThumbnailItem {
            id: "20".into(),
            old_name: "layer_2_copy".into(),
            png_bytes: vec![4, 5, 6],
        },
        AssetThumbnailItem {
            id: "30".into(),
            old_name: "background_obj".into(),
            png_bytes: vec![7, 8, 9],
        },
    ];

    let identified = identify_batch(&mock_llm, "test-vlm-model", &items)
        .expect("identify_batch should succeed");

    assert_eq!(identified.len(), 3);
    assert_eq!(mock_llm.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(mock_llm.batch_sizes.lock().unwrap().as_slice(), &[3]);

    assert_eq!(identified[0].id, "10");
    assert_eq!(identified[0].name, "hero_sword_side");
    assert_eq!(identified[0].category, AssetCategory::Prop);
    assert_eq!(identified[0].view, AssetView::Side);
    assert_eq!(identified[0].tags, vec!["weapon", "iron", "hero"]);

    assert_eq!(identified[1].id, "20");
    assert_eq!(identified[1].name, "warrior_head_front");
    assert_eq!(identified[1].category, AssetCategory::Character);
    assert_eq!(identified[1].view, AssetView::Front);
    assert_eq!(identified[1].tags, vec!["warrior", "head", "face"]);

    assert_eq!(identified[2].id, "30");
    assert_eq!(identified[2].name, "pine_tree_3q");
    assert_eq!(identified[2].category, AssetCategory::Background);
    assert_eq!(identified[2].view, AssetView::ThreeQuarter);
    assert_eq!(identified[2].tags, vec!["tree", "nature", "foliage"]);
}

#[test]
fn test_identify_all_assets_batches_in_eights_and_reports_progress() {
    // 19 items -> chunks of 8, 8, 3
    let mut items = Vec::new();
    for i in 0..19 {
        items.push(AssetThumbnailItem {
            id: format!("{i}"),
            old_name: format!("asset_{i}"),
            png_bytes: vec![i as u8],
        });
    }

    let mut responses = Vec::new();
    // Batch 1 (8 items)
    let batch1: Vec<serde_json::Value> = (0..8)
        .map(|i| {
            serde_json::json!({
                "id": format!("{i}"),
                "name": format!("classified_{i}"),
                "category": "prop",
                "view": "front",
                "tags": ["tag"]
            })
        })
        .collect();
    responses.push(serde_json::Value::Array(batch1));

    // Batch 2 (8 items)
    let batch2: Vec<serde_json::Value> = (8..16)
        .map(|i| {
            serde_json::json!({
                "id": format!("{i}"),
                "name": format!("classified_{i}"),
                "category": "character",
                "view": "side",
                "tags": ["tag"]
            })
        })
        .collect();
    responses.push(serde_json::Value::Array(batch2));

    // Batch 3 (3 items)
    let batch3: Vec<serde_json::Value> = (16..19)
        .map(|i| {
            serde_json::json!({
                "id": format!("{i}"),
                "name": format!("classified_{i}"),
                "category": "background",
                "view": "na",
                "tags": ["tag"]
            })
        })
        .collect();
    responses.push(serde_json::Value::Array(batch3));

    let mock_llm = MockVlm::new(responses);

    let mut progress_reports = Vec::new();
    let proposals = identify_all_assets(
        &mock_llm,
        "test-vlm",
        &items,
        |processed, total| {
            progress_reports.push((processed, total));
        },
    )
    .expect("identify_all_assets should succeed");

    assert_eq!(proposals.len(), 19);
    assert_eq!(mock_llm.call_count.load(Ordering::SeqCst), 3);
    assert_eq!(
        mock_llm.batch_sizes.lock().unwrap().as_slice(),
        &[8, 8, 3],
        "VLM calls must be strictly batched in chunks of at most 8"
    );

    assert_eq!(
        progress_reports,
        vec![(8, 19), (16, 19), (19, 19)],
        "Progress callback should accurately track batches"
    );

    for (i, prop) in proposals.iter().enumerate() {
        assert_eq!(prop.id, format!("{i}"));
        assert_eq!(prop.new_name, format!("classified_{i}"));
        assert!(prop.selected, "Default proposals should be pre-selected for review");
    }
}

#[test]
fn test_apply_renames_to_scene_updates_names_and_tags_and_roundtrips() {
    let mut scene = Scene::default();

    // Insert 3 symbols into library
    let sym1 = Symbol::new(SymbolId(1), "old_sword", SymbolKind::Graphic);
    let sym2 = Symbol::new(SymbolId(2), "old_hero", SymbolKind::Graphic);
    let sym3 = Symbol::new(SymbolId(3), "old_tree", SymbolKind::Graphic);

    scene.library_mut().insert(sym1);
    scene.library_mut().insert(sym2);
    scene.library_mut().insert(sym3);

    let proposals = vec![
        RenameProposal {
            id: "1".into(),
            old_name: "old_sword".into(),
            new_name: "hero_sword_side".into(),
            category: AssetCategory::Prop,
            view: AssetView::Side,
            tags: vec!["prop".into(), "weapon".into(), "iron".into()],
            selected: true,
        },
        RenameProposal {
            id: "2".into(),
            old_name: "old_hero".into(),
            new_name: "unselected_rename".into(),
            category: AssetCategory::Character,
            view: AssetView::Front,
            tags: vec!["ignored".into()],
            selected: false, // User unchecked this in review dialog
        },
        RenameProposal {
            id: "3".into(),
            old_name: "old_tree".into(),
            new_name: "pine_tree_3q".into(),
            category: AssetCategory::Background,
            view: AssetView::ThreeQuarter,
            tags: vec!["scenery".into(), "nature".into()],
            selected: true,
        },
    ];

    let changed = apply_renames_to_scene(&mut scene, &proposals);
    assert_eq!(changed, 2, "Only selected proposals should be applied");

    let s1 = scene.library().get(SymbolId(1)).expect("sym 1 exists");
    assert_eq!(s1.name, "hero_sword_side");
    assert_eq!(s1.tags, vec!["prop", "weapon", "iron"]);

    let s2 = scene.library().get(SymbolId(2)).expect("sym 2 exists");
    assert_eq!(s2.name, "old_hero", "Unselected symbol should keep original name");
    assert!(s2.tags.is_empty(), "Unselected symbol should have empty tags");

    let s3 = scene.library().get(SymbolId(3)).expect("sym 3 exists");
    assert_eq!(s3.name, "pine_tree_3q");
    assert_eq!(s3.tags, vec!["scenery", "nature"]);

    // Test document saving and loading to verify 'tags' persist across disk format
    let tmp_doc = tempfile::NamedTempFile::new().expect("temp file");
    let doc_path = tmp_doc.path().with_extension("buzz");
    buzz_doc::format::save(&scene, &doc_path).expect("buzz_doc format::save");

    let reconstructed_scene = buzz_doc::format::load(&doc_path).expect("buzz_doc format::load");

    let r1 = reconstructed_scene
        .library()
        .get(SymbolId(1))
        .expect("reconstructed sym 1");
    assert_eq!(r1.name, "hero_sword_side");
    assert_eq!(r1.tags, vec!["prop", "weapon", "iron"]);

    let r3 = reconstructed_scene
        .library()
        .get(SymbolId(3))
        .expect("reconstructed sym 3");
    assert_eq!(r3.name, "pine_tree_3q");
    assert_eq!(r3.tags, vec!["scenery", "nature"]);
}

/// C8 fix: renaming used to also touch on-disk assets that merely shared a
/// symbol's old name, outside undo — `apply_renames_to_asset_library` is
/// gone, and `apply_renames_to_scene` is the only path renames take now.
#[test]
fn test_apply_renames_to_scene_makes_new_names_unique() {
    let mut scene = Scene::default();

    // One symbol already named "tree", plus two more the VLM both proposed
    // renaming to "tree".
    scene
        .library_mut()
        .insert(Symbol::new(SymbolId(1), "tree", SymbolKind::Graphic));
    scene
        .library_mut()
        .insert(Symbol::new(SymbolId(2), "old_oak", SymbolKind::Graphic));
    scene
        .library_mut()
        .insert(Symbol::new(SymbolId(3), "old_pine", SymbolKind::Graphic));

    let proposals = vec![
        RenameProposal {
            id: "2".into(),
            old_name: "old_oak".into(),
            new_name: "tree".into(),
            category: AssetCategory::Background,
            view: AssetView::Na,
            tags: vec![],
            selected: true,
        },
        RenameProposal {
            id: "3".into(),
            old_name: "old_pine".into(),
            new_name: "tree".into(),
            category: AssetCategory::Background,
            view: AssetView::Na,
            tags: vec![],
            selected: true,
        },
    ];

    let changed = apply_renames_to_scene(&mut scene, &proposals);
    assert_eq!(changed, 2);

    let names: std::collections::HashSet<String> = scene
        .library()
        .iter()
        .map(|s| s.name.clone())
        .collect();

    // The pre-existing "tree" is untouched, and the two proposals landed on
    // distinct, non-colliding names.
    assert!(names.contains("tree"));
    assert_eq!(names.len(), 3, "no two symbols should share a name: {names:?}");

    let s2 = scene.library().get(SymbolId(2)).expect("sym 2 exists");
    let s3 = scene.library().get(SymbolId(3)).expect("sym 3 exists");
    assert_ne!(s2.name, s3.name);
    assert_eq!(s2.name, "tree 2");
    assert_eq!(s3.name, "tree 3");
}
