use std::f64::consts::FRAC_PI_4;
use buzz_ai::character::{
    assemble_character, complete_parts, part_masks, skeleton, trace_character_whole,
};
use buzz_ai::pose::Person;
use buzz_ai::segment::matte;
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;

#[test]
fn assemble_character_creates_symbols_layers_and_rigged_armature() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        eprintln!("Skipping test: {} not found", png_path);
        return;
    };
    let img = img.to_rgba8();
    let matte = matte(&img, None).expect("matte");

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced = trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace character");

    // 1. Assemble the character
    let assembled = assemble_character(
        &skel,
        &traced,
        "Pujari",
        Some((img.width() as f64, img.height() as f64)),
        None,
    )
    .expect("assemble character");

    // 2. Verify part symbols and character symbol
    assert_eq!(
        assembled.part_symbols.len(),
        traced.parts.len(),
        "Each traced part must have a corresponding Graphic symbol"
    );
    assert_eq!(
        assembled.character_symbol.layers.len(),
        traced.parts.len(),
        "Character symbol must contain one layer per part"
    );

    // 3. Verify armature contains the 17 `biped_full` bones (T10: the 11 standard
    // biped bones, then Wrist L/R, Ankle L/R, Brow L/R)
    assert_eq!(
        assembled.armature.armature.len(),
        17,
        "Armature should have 17 humanoid biped bones"
    );
    let biped = buzz_rig::RigPattern::biped();
    for (i, slot) in biped.slots.iter().enumerate() {
        let bone = &assembled.armature.armature.bones[i];
        assert_eq!((bone.name.as_str(), bone.parent), (slot.name.as_str(), slot.parent), "bone {i}");
    }
    assert!(
        !assembled.armature.parts.is_empty(),
        "Armature must have rigidly bound parts"
    );

    // 4. Verify Library contains all symbols
    assert!(
        assembled.scene.library().get(assembled.character_symbol.id).is_some(),
        "Character symbol must be present in scene library"
    );
    for sym in &assembled.part_symbols {
        assert!(
            assembled.scene.library().get(sym.id).is_some(),
            "Part symbol {} must be present in scene library",
            sym.name
        );
    }

    // 5. Test Posing: rotate forearm bone 45°
    let elbow_idx = assembled
        .armature
        .armature
        .bones
        .iter()
        .position(|b| b.name == "Elbow L")
        .expect("Elbow L bone must exist in biped armature");

    let mut posed_armature = assembled.armature.clone();
    let rest_transform = posed_armature.armature.pose_transform(elbow_idx);
    posed_armature.armature.bones[elbow_idx].angle += FRAC_PI_4;
    let rotated_transform = posed_armature.armature.pose_transform(elbow_idx);

    assert_ne!(
        rest_transform, rotated_transform,
        "Rotating bone angle must alter pose transform"
    );

    let posed_parts = posed_armature.posed();
    assert_eq!(
        posed_parts.len(),
        assembled.armature.parts.len(),
        "Posed parts count must match bound parts count"
    );

    // Find the forearm part among posed parts and check that its transform changed
    let forearm_part_opt = posed_armature.parts.iter().enumerate().find(|(_, p)| {
        match p.binding {
            buzz_scene::RigBinding::Rigid(b_idx) => b_idx == elbow_idx,
            _ => false,
        }
    });

    let rest_parts = assembled.armature.posed();
    if let Some((part_idx, _)) = forearm_part_opt {
        let rest_part = &rest_parts[part_idx];
        let posed_part = &posed_parts[part_idx];
        assert_ne!(
            rest_part.transform, posed_part.transform,
            "Posed forearm part transform must differ from rest pose"
        );
    }

    // 6. Test Save to Cast AssetLibrary
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut asset_library = buzz_doc::AssetLibrary::at(temp_dir.path());
    let cast_asset = assembled
        .save_to_cast(&mut asset_library)
        .expect("save to cast");
    assert_eq!(cast_asset.folder, "Cast");
    assert!(cast_asset.name.starts_with("Pujari"));

    // 7. Headless rendering test via GPU exporter if available
    if let Ok(mut exporter) = Exporter::new(&GpuPreference::Automatic) {
        let settings = ExportSettings::for_stage(&assembled.scene);
        let frame = exporter.render(&assembled.scene, 0, &settings);
        assert!(frame.is_ok(), "Rendering assembled character scene must succeed");
    } else {
        eprintln!("Notice: skipping GPU export render check (no GPU available)");
    }
}

#[test]
#[ignore]
fn assemble_character_end_to_end_with_models() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();

    let persons = match buzz_ai::pose::detect(&img) {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("Models not available or detection returned empty, skipping");
            return;
        }
    };

    let matte = buzz_ai::segment::matte(&img, None).expect("matte");
    let skel = skeleton(&persons[0]);

    let parts = part_masks(&img, &matte, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced = trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace");
    let assembled = assemble_character(
        &skel,
        &traced,
        "Pujari",
        Some((img.width() as f64, img.height() as f64)),
        None,
    )
    .expect("assemble");

    assert_eq!(assembled.part_symbols.len(), traced.parts.len());
    assert_eq!(assembled.armature.armature.len(), 17);
}
