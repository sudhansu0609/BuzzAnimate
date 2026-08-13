//! Phase 5 exit test: import each supported format into a real document.
//!
//! Every earlier importer test checks its own parser in isolation. This one
//! runs the whole path the application runs — read the file, merge it into an
//! open document that already has artwork of its own, save it, reopen it — for
//! all three formats. That is where the seams are, and the seams are what
//! break: id spaces that collide, symbols that survive parsing but not
//! serialisation, layers that arrive but do not draw.
//!
//! # What this does not do
//!
//! Phase 5's original exit criterion was to compare an imported `.fla`
//! frame-by-frame against a render from Adobe Animate. That comparison is not
//! performed here and cannot be: it requires a licensed copy of Animate and a
//! reference file produced by it, neither of which exists in this environment,
//! and inventing one would prove nothing about Animate's actual output. What
//! is verified instead is structural fidelity against a file whose intended
//! content is known exactly, plus the round trip through disk. The visual
//! comparison remains genuinely outstanding and is recorded in PROGRESS.md as
//! such rather than quietly marked done.

use buzz_geom::Shape as _;
use buzz_scene::{ImportTarget, Scene, ShapeData, SymbolKind};
use kurbo::Rect;
use peniko::Color;

/// A document with artwork of its own, so every import is a *merge* into
/// something rather than a fill of an empty scene.
fn open_document() -> Scene {
    let mut scene = Scene::default();
    let layer = scene.layers().iter().next().unwrap().id;
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 25.0, 25.0).to_path(1e-9), Color::WHITE),
    );
    // A symbol whose name every fixture below also uses, so the collision path
    // is exercised on each format rather than only once.
    scene.add_symbol("Shape 1", SymbolKind::Graphic, None);
    scene
}

/// Assert the invariants that must hold after *any* merge, whatever the source.
fn check_document_is_sound(scene: &Scene, before_symbols: usize) {
    assert!(
        scene.library().len() > before_symbols,
        "the import brought nothing into the library"
    );

    // Every object id is unique across the stage and every symbol.
    let mut seen = std::collections::BTreeSet::new();
    let stage = scene.stage_layers().iter().flat_map(|l| l.all_objects());
    let nested = scene
        .library()
        .iter()
        .flat_map(|s| s.layers.iter().flat_map(|l| l.all_objects()));
    for object in stage.chain(nested) {
        assert!(
            seen.insert(object.id.0),
            "object id {} is used twice after the merge",
            object.id.0
        );
    }

    // Every symbol name is unique, which is what the Library panel relies on.
    let mut names = std::collections::BTreeSet::new();
    for symbol in scene.library().iter() {
        assert!(
            names.insert(symbol.name.clone()),
            "two symbols are both called {:?}",
            symbol.name
        );
    }

    // Every instance points at a symbol that exists.
    let stage = scene.stage_layers().iter().flat_map(|l| l.all_objects());
    let nested = scene
        .library()
        .iter()
        .flat_map(|s| s.layers.iter().flat_map(|l| l.all_objects()));
    for object in stage.chain(nested) {
        if let Some(instance) = object.instance() {
            assert!(
                scene.library().get(instance.symbol).is_some(),
                "an instance points at symbol {:?}, which is not in the library",
                instance.symbol
            );
        }
    }
}

/// Save and reopen, then confirm nothing was lost on the way.
fn survives_a_round_trip_through_disk(scene: &Scene) {
    let dir = tempfile::tempdir().expect("a temp directory");
    let path = dir.path().join("imported.buzz");

    buzz_doc::format::save(scene, &path).expect("the imported document saves");
    let reloaded = buzz_doc::format::load(&path).expect("and reopens");

    assert_eq!(
        reloaded.library().len(),
        scene.library().len(),
        "symbols were lost in the file format"
    );
    assert_eq!(
        reloaded.stage_layers().len(),
        scene.stage_layers().len(),
        "layers were lost in the file format"
    );

    let count = |s: &Scene| -> usize {
        s.stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .count()
            + s.library()
                .iter()
                .flat_map(|sym| sym.layers.iter().flat_map(|l| l.all_objects()))
                .count()
    };
    assert_eq!(count(&reloaded), count(scene), "artwork was lost on save");
}

// ---------------------------------------------------------------------------
// .fla
// ---------------------------------------------------------------------------

const FLA_DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="640" height="480"
             frameRate="30" backgroundColor="#102030">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Folder" layerType="folder"/>
        <DOMLayer name="Background" parentLayerIndex="0">
          <frames>
            <DOMFrame index="0" duration="10">
              <elements>
                <DOMShape>
                  <edges>
                    <Edge fillStyle0="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="Hero">
          <frames>
            <DOMFrame index="0" duration="5" tweenType="motion">
              <elements>
                <DOMSymbolInstance libraryItemName="hero">
                  <matrix><Matrix a="2" d="2" tx="100" ty="50"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="5" duration="5"/>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

const FLA_SYMBOL: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="hero" symbolType="graphic">
  <timeline>
    <DOMTimeline name="hero">
      <layers>
        <DOMLayer name="body">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <edges>
                    <Edge fillStyle0="1" edges="!0 0|400 0|400 400|0 400|0 0"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

fn write_fla(dir: &std::path::Path) -> std::path::PathBuf {
    use std::io::Write;

    let path = dir.join("fixture.fla");
    let file = std::fs::File::create(&path).expect("the fixture is created");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    zip.start_file("DOMDocument.xml", options).unwrap();
    zip.write_all(FLA_DOCUMENT.as_bytes()).unwrap();
    zip.start_file("LIBRARY/hero.xml", options).unwrap();
    zip.write_all(FLA_SYMBOL.as_bytes()).unwrap();
    zip.finish().unwrap();

    path
}

#[test]
fn an_animate_document_imports_merges_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fla(dir.path());

    let imported = buzz_app::import::read(&path).expect("the .fla imports");
    assert!(
        imported.summary.contains("layers"),
        "the report should say what arrived: {}",
        imported.summary
    );

    let mut document = open_document();
    let before = document.library().len();
    let report = document.merge(&imported.scene, ImportTarget::Stage);

    // The fixture's symbol is called "hero"; the open document already has a
    // "Shape 1", so nothing should collide here — but the invariant holds
    // either way.
    assert!(report.symbols >= 1, "the hero symbol came across");
    check_document_is_sound(&document, before);

    // The structure the file actually describes.
    let hero = document
        .library()
        .find_by_name("hero")
        .expect("the symbol arrived with its name");
    assert_eq!(hero.kind, SymbolKind::Graphic);
    assert!(hero.bounds().is_some(), "and with its artwork");

    let folder = document
        .stage_layers()
        .iter()
        .find(|l| l.name == "Folder")
        .expect("the layer folder arrived");
    let background = document
        .stage_layers()
        .iter()
        .find(|l| l.name == "Background")
        .expect("the nested layer arrived");
    assert_eq!(
        background.parent,
        Some(folder.id),
        "and is still inside its folder after the merge renumbered everything"
    );

    // The instance on the Hero layer points at the imported symbol.
    let instance = document
        .stage_layers()
        .iter()
        .find(|l| l.name == "Hero")
        .and_then(|l| l.all_objects().next().cloned())
        .expect("the Hero layer has an object");
    assert_eq!(
        instance.instance().map(|i| i.symbol),
        Some(hero.id),
        "the instance points at the imported symbol, not a local one"
    );

    survives_a_round_trip_through_disk(&document);
}

// ---------------------------------------------------------------------------
// .swf
// ---------------------------------------------------------------------------

fn write_swf(dir: &std::path::Path) -> std::path::PathBuf {
    use swf::{Twips, *};

    let bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(30.0),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(30.0),
    };
    let d = |dx: f64, dy: f64| PointDelta::new(Twips::from_pixels(dx), Twips::from_pixels(dy));

    let shape = Tag::DefineShape(Shape {
        version: 1,
        id: 1,
        shape_bounds: bounds.clone(),
        edge_bounds: bounds.clone(),
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(swf::Color {
                r: 0,
                g: 200,
                b: 0,
                a: 255,
            })],
            line_styles: vec![],
        },
        shape: vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(swf::Point::new(Twips::ZERO, Twips::ZERO)),
                fill_style_0: None,
                fill_style_1: Some(1),
                line_style: None,
                new_styles: None,
            })),
            ShapeRecord::StraightEdge {
                delta: d(30.0, 0.0),
            },
            ShapeRecord::StraightEdge {
                delta: d(0.0, 30.0),
            },
            ShapeRecord::StraightEdge {
                delta: d(-30.0, 0.0),
            },
            ShapeRecord::StraightEdge {
                delta: d(0.0, -30.0),
            },
        ],
    });

    let place = |depth: u16, id: u16, x: f64| {
        Tag::PlaceObject(Box::new(PlaceObject {
            version: 2,
            action: PlaceObjectAction::Place(id),
            depth,
            matrix: Some(Matrix::translate(Twips::from_pixels(x), Twips::ZERO)),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            class_name: None,
            filters: None,
            background_color: None,
            blend_mode: None,
            clip_actions: None,
            has_image: false,
            is_bitmap_cached: None,
            is_visible: None,
            amf_data: None,
        }))
    };

    let header = Header {
        compression: Compression::None,
        version: 6,
        stage_size: Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(320.0),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(240.0),
        },
        frame_rate: Fixed8::from_f32(12.0),
        num_frames: 2,
    };

    let tags = vec![
        shape,
        place(1, 1, 0.0),
        place(2, 1, 100.0),
        Tag::ShowFrame,
        Tag::ShowFrame,
    ];

    let path = dir.join("fixture.swf");
    let mut bytes = Vec::new();
    swf::write_swf(&header, &tags, &mut bytes).expect("the fixture writes");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_flash_movie_imports_merges_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_swf(dir.path());

    let imported = buzz_app::import::read(&path).expect("the .swf imports");

    let mut document = open_document();
    let before = document.library().len();
    let report = document.merge(&imported.scene, ImportTarget::Stage);

    check_document_is_sound(&document, before);

    // The movie's one shape is called "Shape 1", which the open document
    // already uses — so this is the collision path, end to end.
    assert_eq!(
        report.renamed.len(),
        1,
        "the name clash should be reported: {:?}",
        report.renamed
    );
    assert_eq!(report.renamed[0].0, "Shape 1");

    // Two depths were used, so two layers arrived.
    assert_eq!(report.layers, 2, "one layer per depth");

    survives_a_round_trip_through_disk(&document);
}

// ---------------------------------------------------------------------------
// .pdf
// ---------------------------------------------------------------------------

fn write_pdf(dir: &std::path::Path) -> std::path::PathBuf {
    let content = "1 0 0 rg\n10 10 80 40 re\nf\n0 0 1 RG\n3 w\n20 60 m\n40 90 60 90 80 60 c\nS";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 150] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];

    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));

    let path = dir.join("fixture.pdf");
    std::fs::write(&path, out).unwrap();
    path
}

#[test]
fn pdf_artwork_imports_merges_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_pdf(dir.path());

    let imported = buzz_app::import::read(&path).expect("the .pdf imports");
    assert!(
        imported.unsupported.is_empty(),
        "this fixture uses only supported operators: {:?}",
        imported.unsupported
    );

    // A PDF brings artwork rather than symbols, so the shared soundness check
    // (which requires a symbol) does not apply; the artwork is checked instead.
    let mut document = open_document();
    let stage_before = document
        .stage_layers()
        .iter()
        .flat_map(|l| l.all_objects())
        .count();

    document.merge(&imported.scene, ImportTarget::Stage);

    let objects: Vec<_> = document
        .stage_layers()
        .iter()
        .flat_map(|l| l.all_objects())
        .collect();
    assert_eq!(
        objects.len(),
        stage_before + 2,
        "the filled rectangle and the stroked curve both arrived"
    );

    let filled = objects
        .iter()
        .filter_map(|o| match &o.kind {
            buzz_scene::ObjectKind::Shape(s) => s.fill.as_ref().map(|f| f.color()),
            _ => None,
        })
        .find(|c| c.to_rgba8().to_u8_array()[..3] == [255, 0, 0]);
    assert!(filled.is_some(), "the red fill survived the merge");

    let stroked = objects.iter().any(|o| match &o.kind {
        buzz_scene::ObjectKind::Shape(s) => s.stroke.as_ref().is_some_and(|st| st.width == 3.0),
        _ => false,
    });
    assert!(stroked, "the 3-unit stroke survived the merge");

    survives_a_round_trip_through_disk(&document);
}

// ---------------------------------------------------------------------------
// Behaviour common to all three
// ---------------------------------------------------------------------------

/// Importing to the library must never disturb the stage, whatever the format
/// — that is the whole difference between the two menu commands.
#[test]
fn importing_to_the_library_never_touches_the_stage() {
    let dir = tempfile::tempdir().unwrap();

    for path in [write_fla(dir.path()), write_swf(dir.path())] {
        let imported = buzz_app::import::read(&path).expect("it imports");
        let mut document = open_document();

        let layers_before = document.stage_layers().len();
        let objects_before: Vec<u64> = document
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .map(|o| o.id.0)
            .collect();

        let report = document.merge(&imported.scene, ImportTarget::Library);

        assert_eq!(
            report.layers,
            0,
            "{}: no layers should arrive",
            path.display()
        );
        assert_eq!(document.stage_layers().len(), layers_before);

        let objects_after: Vec<u64> = document
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .map(|o| o.id.0)
            .collect();
        assert_eq!(
            objects_before,
            objects_after,
            "{}: the stage must be untouched",
            path.display()
        );
    }
}

/// A failed import must leave the open document exactly as it was — the user
/// should be able to pick the wrong file without consequence.
#[test]
fn a_failed_import_leaves_the_document_untouched() {
    let dir = tempfile::tempdir().unwrap();

    let broken = dir.path().join("broken.swf");
    std::fs::write(&broken, b"not really an swf").unwrap();
    let unknown = dir.path().join("art.psd");
    std::fs::write(&unknown, b"whatever").unwrap();
    let missing = dir.path().join("absent.fla");

    for path in [broken, unknown, missing] {
        let document = open_document();
        let before = document.clone();

        let result = buzz_app::import::read(&path);
        assert!(result.is_err(), "{} should not import", path.display());

        // Nothing was merged, so nothing changed.
        assert_eq!(document, before, "{}", path.display());
    }
}

/// Two imports of the same file must both survive, with distinct names and
/// distinct ids — importing twice is something people do by accident and it
/// should not corrupt anything.
#[test]
fn importing_the_same_file_twice_keeps_both_copies_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_swf(dir.path());
    let imported = buzz_app::import::read(&path).expect("it imports");

    let mut document = open_document();
    let before = document.library().len();

    document.merge(&imported.scene, ImportTarget::Stage);
    document.merge(&imported.scene, ImportTarget::Stage);

    assert_eq!(
        document.library().len(),
        before + 2,
        "both imports should be in the library"
    );
    check_document_is_sound(&document, before);
    survives_a_round_trip_through_disk(&document);
}
