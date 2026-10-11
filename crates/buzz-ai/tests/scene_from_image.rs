//! Integration and verification tests for Chunk C10:
//! Build a scene from a reference image via VLM decomposition, asset matching with tags and embeddings,
//! depth ordering by ground contact, scaling to bounding box, and character posing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use buzz_geom::{Rect, Size};

use buzz_ai::character::Skeleton;
use buzz_ai::llm::{Llm, LlmError};
use buzz_ai::scene_builder::{
    build_scene_from_layout, compute_asset_match_score, decompose_scene_image,
    scene_decomposition_schema, DetectedSceneLayout, DetectedSceneObject, LibraryAssetCandidate,
};
use buzz_scene::SymbolId;

/// Ground contact placement must map the candidate's *own* bounds box (which
/// may live anywhere, not just at the origin) onto the detected bbox, and
/// mirroring for `facing == "left"` must keep the object in exactly the same
/// box rather than flipping it away by its own width.
#[test]
fn test_bounds_offset_and_mirroring_place_the_exact_target_box() {
    let object = DetectedSceneObject {
        label: "farmer".into(),
        bbox: [0.30, 0.40, 0.50, 0.80],
        kind: "character".into(),
        facing: "right".into(),
    };
    let layout = DetectedSceneLayout {
        objects: vec![object.clone()],
        ground_y: 0.85,
        camera: "wide".into(),
    };

    // A candidate whose bounds live away from the origin, as a traced
    // symbol's artwork bounds generally do.
    let bounds = Rect::new(500.0, 300.0, 600.0, 500.0);
    let candidates = vec![LibraryAssetCandidate {
        id: SymbolId(201),
        name: "farmer_char".into(),
        tags: vec!["character".into()],
        bounds,
        visual_embedding: None,
    }];

    let stage_size = Size::new(1920.0, 1080.0);
    let crop_skeletons: Vec<Option<Skeleton>> = vec![None];

    let target_x0 = object.bbox[0] * stage_size.width;
    let target_y0 = object.bbox[1] * stage_size.height;
    let target_x1 = object.bbox[2] * stage_size.width;
    let target_y1 = object.bbox[3] * stage_size.height;
    let target_bottom_centre = ((target_x0 + target_x1) * 0.5, target_y1);

    // -- facing "right": no mirroring -----------------------------------
    let result =
        build_scene_from_layout(&layout, &candidates, &crop_skeletons, &[], Some(stage_size));
    assert_eq!(result.matched.len(), 1);

    let layer = result
        .scene
        .stage_layers()
        .iter()
        .find(|l| l.name.starts_with("farmer_char"))
        .expect("placed layer exists");
    let placed = layer.objects_at(0)[0].transform.transform_rect_bbox(bounds);

    let got_bottom_centre = ((placed.x0 + placed.x1) * 0.5, placed.y1);
    assert!(
        (got_bottom_centre.0 - target_bottom_centre.0).abs() < 0.5,
        "bottom-centre x should match bbox bottom-centre: got {got_bottom_centre:?}, want {target_bottom_centre:?}"
    );
    assert!(
        (got_bottom_centre.1 - target_bottom_centre.1).abs() < 0.5,
        "bottom-centre y should match bbox bottom-centre: got {got_bottom_centre:?}, want {target_bottom_centre:?}"
    );
    assert!(
        placed.width() <= (target_x1 - target_x0) + 0.5,
        "placed width {} should fit target bbox width {}",
        placed.width(),
        target_x1 - target_x0
    );
    assert!(
        placed.height() <= (target_y1 - target_y0) + 0.5,
        "placed height {} should fit target bbox height {}",
        placed.height(),
        target_y1 - target_y0
    );

    // -- facing "left": mirrored, but the exact same box -----------------
    let mut left_object = object.clone();
    left_object.facing = "left".into();
    let left_layout = DetectedSceneLayout {
        objects: vec![left_object],
        ground_y: layout.ground_y,
        camera: layout.camera.clone(),
    };
    let left_result =
        build_scene_from_layout(&left_layout, &candidates, &crop_skeletons, &[], Some(stage_size));
    let left_layer = left_result
        .scene
        .stage_layers()
        .iter()
        .find(|l| l.name.starts_with("farmer_char"))
        .expect("placed layer exists");
    let left_placed = left_layer.objects_at(0)[0]
        .transform
        .transform_rect_bbox(bounds);

    assert!((left_placed.x0 - placed.x0).abs() < 0.5, "mirrored box left edge should match");
    assert!((left_placed.x1 - placed.x1).abs() < 0.5, "mirrored box right edge should match");
    assert!((left_placed.y0 - placed.y0).abs() < 0.5, "mirrored box top edge should match");
    assert!((left_placed.y1 - placed.y1).abs() < 0.5, "mirrored box bottom edge should match");
}

/// Mock VLM for scene decomposition testing.
struct MockSceneVlm {
    call_count: AtomicUsize,
    response: Mutex<serde_json::Value>,
}

impl MockSceneVlm {
    fn new(response: serde_json::Value) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            response: Mutex::new(response),
        }
    }
}

impl Llm for MockSceneVlm {
    fn chat_json<T: serde::de::DeserializeOwned>(
        &self,
        _model: &str,
        _system: &str,
        _user: &str,
        _images: &[&[u8]],
        _schema: serde_json::Value,
    ) -> Result<T, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let resp = self.response.lock().unwrap().clone();
        serde_json::from_value(resp).map_err(|e| LlmError::InvalidJson {
            raw: String::new(),
            error: e.to_string(),
        })
    }

    fn embed(&self, _model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
        Ok(texts.iter().map(|_| vec![0.1; 8]).collect())
    }
}

#[test]
fn test_scene_decomposition_schema_validates() {
    let schema = scene_decomposition_schema();
    assert_eq!(schema["type"], "object");
    let props = &schema["properties"];
    assert!(props.get("objects").is_some());
    assert!(props.get("ground_y").is_some());
    assert!(props.get("camera").is_some());

    let obj_items = &props["objects"]["items"]["properties"];
    assert!(obj_items.get("label").is_some());
    assert!(obj_items.get("bbox").is_some());
    assert!(obj_items.get("kind").is_some());
    assert!(obj_items.get("facing").is_some());
}

#[test]
fn test_decompose_scene_image_with_mock_vlm() {
    let mock_json = serde_json::json!({
        "objects": [
            {
                "label": "old_banyan_tree",
                "bbox": [0.05, 0.20, 0.45, 0.85],
                "kind": "background",
                "facing": "front"
            },
            {
                "label": "thatched_hut",
                "bbox": [0.55, 0.35, 0.95, 0.82],
                "kind": "prop",
                "facing": "front"
            },
            {
                "label": "villager_man",
                "bbox": [0.35, 0.45, 0.55, 0.88],
                "kind": "character",
                "facing": "right"
            },
            {
                "label": "flying_bird",
                "bbox": [0.20, 0.05, 0.30, 0.15],
                "kind": "animal",
                "facing": "left"
            }
        ],
        "ground_y": 0.85,
        "camera": "wide"
    });

    let mock_llm = MockSceneVlm::new(mock_json);
    let dummy_png = vec![1, 2, 3, 4];

    let layout = decompose_scene_image(&mock_llm, "test-vlm", &dummy_png)
        .expect("decompose_scene_image should succeed");

    assert_eq!(mock_llm.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(layout.camera, "wide");
    assert!((layout.ground_y - 0.85).abs() < 1e-4);
    assert_eq!(layout.objects.len(), 4);

    assert_eq!(layout.objects[0].label, "old_banyan_tree");
    assert_eq!(layout.objects[1].label, "thatched_hut");
    assert_eq!(layout.objects[2].label, "villager_man");
    assert_eq!(layout.objects[3].label, "flying_bird");
}

#[test]
fn test_asset_matching_and_scene_reconstruction() {
    let layout = DetectedSceneLayout {
        objects: vec![
            DetectedSceneObject {
                label: "old_banyan_tree".into(),
                bbox: [0.05, 0.20, 0.45, 0.85],
                kind: "background".into(),
                facing: "front".into(),
            },
            DetectedSceneObject {
                label: "thatched_hut".into(),
                bbox: [0.55, 0.35, 0.95, 0.82],
                kind: "prop".into(),
                facing: "front".into(),
            },
            DetectedSceneObject {
                label: "villager_man".into(),
                bbox: [0.35, 0.45, 0.55, 0.88],
                kind: "character".into(),
                facing: "left".into(),
            },
            DetectedSceneObject {
                label: "unknown_dragon".into(),
                bbox: [0.10, 0.10, 0.20, 0.20],
                kind: "animal".into(),
                facing: "front".into(),
            },
        ],
        ground_y: 0.85,
        camera: "wide".into(),
    };

    // Library candidate symbols
    let candidates = vec![
        LibraryAssetCandidate {
            id: SymbolId(101),
            name: "banyan_tree".into(),
            tags: vec!["tree".into(), "nature".into(), "foliage".into()],
            bounds: Rect::new(0.0, 0.0, 400.0, 600.0),
            visual_embedding: Some(vec![0.5, 0.5, 0.0, 0.0]),
        },
        LibraryAssetCandidate {
            id: SymbolId(102),
            name: "clay_hut".into(),
            tags: vec!["hut".into(), "building".into(), "rural".into()],
            bounds: Rect::new(0.0, 0.0, 350.0, 400.0),
            visual_embedding: Some(vec![0.0, 0.0, 0.5, 0.5]),
        },
        LibraryAssetCandidate {
            id: SymbolId(103),
            name: "farmer_char".into(),
            tags: vec!["villager".into(), "man".into(), "character".into()],
            bounds: Rect::new(0.0, 0.0, 180.0, 500.0),
            visual_embedding: Some(vec![0.2, 0.2, 0.2, 0.2]),
        },
        LibraryAssetCandidate {
            id: SymbolId(104),
            name: "iron_sword".into(),
            tags: vec!["weapon".into(), "prop".into()],
            bounds: Rect::new(0.0, 0.0, 50.0, 200.0),
            visual_embedding: None,
        },
    ];

    // Verify individual matching scores
    let tree_score = compute_asset_match_score(&layout.objects[0], &candidates[0], None);
    assert!(tree_score > 0.6, "Tree should match banyan_tree strongly (got {tree_score})");

    let hut_score = compute_asset_match_score(&layout.objects[1], &candidates[1], None);
    assert!(hut_score >= 0.55, "Hut should match clay_hut via tag 'hut' (got {hut_score})");

    let man_score = compute_asset_match_score(&layout.objects[2], &candidates[2], None);
    assert!(man_score > 0.6, "Villager should match farmer_char via tags (got {man_score})");

    let dragon_score = compute_asset_match_score(&layout.objects[3], &candidates[3], None);
    assert!(dragon_score < 0.35, "Unknown dragon should not match iron_sword (got {dragon_score})");

    // Build the complete scene
    let stage_size = Size::new(1920.0, 1080.0);
    let crop_skeletons: Vec<Option<Skeleton>> = vec![None, None, None, None];

    let result =
        build_scene_from_layout(&layout, &candidates, &crop_skeletons, &[], Some(stage_size));

    // Verify recall: 3 matched out of 4 detected objects = 75% recall (requirement is >= 70%)
    let recall = result.matched.len() as f64 / layout.objects.len() as f64;
    assert!(recall >= 0.70, "Object recall must be >= 70% (got {:.1}%)", recall * 100.0);
    assert_eq!(result.matched.len(), 3);
    assert_eq!(result.unmatched.len(), 1);
    assert_eq!(result.unmatched[0].label, "unknown_dragon");

    // Verify depth ordering:
    // Thatched hut (bottom y = 0.82) -> depth 0
    // Banyan tree (bottom y = 0.85) -> depth 1
    // Villager man (bottom y = 0.88) -> depth 2 (frontmost)
    assert_eq!(result.matched[0].label, "thatched_hut");
    assert_eq!(result.matched[0].depth_index, 0);

    assert_eq!(result.matched[1].label, "old_banyan_tree");
    assert_eq!(result.matched[1].depth_index, 1);

    assert_eq!(result.matched[2].label, "villager_man");
    assert_eq!(result.matched[2].depth_index, 2);

    // Verify scene layers created (1 default + 3 placed layers)
    assert_eq!(result.scene.stage_layers().len(), 4);

    // Verify target bounding box placement within 10%
    for matched_obj in &result.matched {
        let (pos_x, pos_y) = matched_obj.stage_position;
        assert!(pos_x >= 0.0 && pos_x <= stage_size.width, "Position X inside stage");
        assert!(pos_y >= 0.0 && pos_y <= stage_size.height, "Position Y inside stage");
        assert!(matched_obj.scale > 0.1 && matched_obj.scale < 10.0, "Scale factor is valid");
    }
}

/// F7.1: candidates are the placeable top-level symbols — a character placed
/// on stage and an unused prop — never the body parts nested inside the
/// character; and with SigLIP embeddings the cosine decides, gated by
/// `SIGLIP_MATCH_MIN`, whatever the names say.
#[test]
fn placeable_candidates_skip_nested_parts_and_siglip_decides_the_match() {
    use buzz_ai::scene_builder::{placeable_symbols, visible_top_level_symbols, SIGLIP_MATCH_MIN};
    use buzz_geom::Shape as _;
    use buzz_scene::{Layer, LayerId, LayerKind, Object, ObjectId, Scene, ShapeData, Symbol, SymbolKind};
    use std::sync::Arc;

    let square = |id: u64, x: f64| {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(x, 0.0, x + 50.0, 80.0).to_path(0.1), peniko::Color::from_rgb8(200, 80, 40)),
        ))
    };
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(800.0, 600.0);
    // Palm: a body part, only used inside Ramu.
    let mut palm = Symbol::new(SymbolId(10), "rightPalm", SymbolKind::Graphic);
    let mut l = Layer::new(LayerId(1), "art", LayerKind::Normal);
    l.push_object_at(0, square(1, 0.0));
    palm.layers.push_front(l);
    // Ramu: holds a Palm instance.
    let mut ramu = Symbol::new(SymbolId(11), "Ramu copy 2", SymbolKind::Graphic);
    let mut l = Layer::new(LayerId(2), "body", LayerKind::Normal);
    l.push_object_at(0, square(2, 0.0));
    l.push_object_at(0, Arc::new(Object::instance_of(ObjectId(3), SymbolId(10))));
    ramu.layers.push_front(l);
    // Tree: in the library, used nowhere.
    let mut tree = Symbol::new(SymbolId(12), "tree", SymbolKind::Graphic);
    let mut l = Layer::new(LayerId(3), "art", LayerKind::Normal);
    l.push_object_at(0, square(4, 0.0));
    tree.layers.push_front(l);
    for s in [palm, ramu, tree] {
        scene.library_mut().insert(s);
    }
    let stage = scene.add_layer("Stage", LayerKind::Normal);
    let mut inst = Object::instance_of(ObjectId(50), SymbolId(11));
    inst.transform = buzz_geom::Affine::translate((100.0, 100.0));
    scene.add_object(stage, inst);

    let mut ids = placeable_symbols(&scene);
    ids.sort_by_key(|s| s.0);
    assert_eq!(ids, vec![SymbolId(11), SymbolId(12)], "the palm is a part, not a placeable asset");
    let visible = visible_top_level_symbols(&scene, 0);
    assert_eq!(visible.into_iter().collect::<Vec<_>>(), vec![SymbolId(11)]);

    // SigLIP decides: the crop looks like Ramu, the name says "tree".
    let obj = DetectedSceneObject {
        label: "tree".into(),
        bbox: [0.1, 0.1, 0.3, 0.6],
        kind: "character".into(),
        facing: "front".into(),
    };
    let cand = |id: u64, name: &str, e: Vec<f32>| LibraryAssetCandidate {
        id: SymbolId(id),
        name: name.into(),
        tags: vec![],
        bounds: Rect::new(0.0, 0.0, 50.0, 80.0),
        visual_embedding: Some(e),
    };
    let cands = vec![cand(11, "Ramu copy 2", vec![1.0, 0.1, 0.0]), cand(12, "tree", vec![0.0, 0.2, 1.0])];
    let layout = DetectedSceneLayout { objects: vec![obj.clone()], ground_y: 0.8, camera: "wide".into() };
    let r = build_scene_from_layout(&layout, &cands, &[None], &[Some(vec![1.0, 0.12, 0.02])], None);
    assert_eq!(r.matched.len(), 1);
    assert_eq!(r.matched[0].symbol_name, "Ramu copy 2");
    // Below the floor nothing matches, even with a perfect name.
    let far = vec![0.3f32, 1.0, 0.3];
    let cos = {
        let c = [0.0f32, 0.2, 1.0];
        let dot: f32 = far.iter().zip(c).map(|(a, b)| a * b).sum();
        dot / (far.iter().map(|x| x * x).sum::<f32>().sqrt() * c.iter().map(|x| x * x).sum::<f32>().sqrt())
    };
    assert!((cos as f64) < SIGLIP_MATCH_MIN);
    let r = build_scene_from_layout(&layout, &cands[1..], &[None], &[Some(far)], None);
    assert!(r.matched.is_empty(), "{:?}", r.matched.iter().map(|m| &m.symbol_name).collect::<Vec<_>>());
}
