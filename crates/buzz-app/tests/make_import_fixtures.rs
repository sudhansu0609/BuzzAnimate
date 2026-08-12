//! Writes one file per importable format, for opening by hand.
//!
//! Run with `--ignored`; it prints the paths it wrote. These exist so the
//! importers can be checked on screen — the same way the Phase 2 and Phase 4
//! font defects were found, which no test would have caught.

use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("buzzanimate-fixtures");
    std::fs::create_dir_all(&dir).expect("the fixture directory is created");
    dir
}

#[test]
#[ignore = "writes files for manual inspection"]
fn write_importable_fixtures() {
    let dir = out_dir();

    // -- .fla ---------------------------------------------------------------
    const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="640" height="480"
             frameRate="24" backgroundColor="#FFFFFF">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Art Folder" layerType="folder"/>
        <DOMLayer name="Background" parentLayerIndex="0" color="#4FFF4F">
          <frames>
            <DOMFrame index="0" duration="12">
              <elements>
                <DOMShape>
                  <fills><FillStyle index="1"><SolidColor color="#3366CC"/></FillStyle></fills>
                  <edges>
                    <Edge fillStyle0="1" edges="!400 400|8000 400|8000 5000|400 5000|400 400"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="Hero">
          <frames>
            <DOMFrame index="0" duration="6" tweenType="motion">
              <elements>
                <DOMSymbolInstance libraryItemName="hero">
                  <matrix><Matrix a="1" d="1" tx="60" ty="60"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="6" duration="6">
              <elements>
                <DOMSymbolInstance libraryItemName="hero">
                  <matrix><Matrix a="2" d="2" tx="300" ty="200"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    const SYMBOL: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="characters/hero" symbolType="graphic">
  <timeline>
    <DOMTimeline name="hero">
      <layers>
        <DOMLayer name="body">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <fills><FillStyle index="1"><SolidColor color="#CC3333"/></FillStyle></fills>
                  <edges>
                    <Edge fillStyle0="1" edges="!0 0|1600 0|1600 2400|0 2400|0 0"/>
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

    let fla = dir.join("fixture.fla");
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&fla).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("DOMDocument.xml", options).unwrap();
        zip.write_all(DOCUMENT.as_bytes()).unwrap();
        zip.start_file("LIBRARY/characters/hero.xml", options).unwrap();
        zip.write_all(SYMBOL.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    println!("wrote {}", fla.display());

    // -- .swf ---------------------------------------------------------------
    let swf_path = dir.join("fixture.swf");
    {
        use swf::{Twips, *};

        let bounds = |w: f64, h: f64| Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(w),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(h),
        };
        let d = |dx: f64, dy: f64| PointDelta::new(Twips::from_pixels(dx), Twips::from_pixels(dy));

        let shape = |id: u16, size: f64, colour: swf::Color| {
            Tag::DefineShape(Shape {
                version: 1,
                id,
                shape_bounds: bounds(size, size),
                edge_bounds: bounds(size, size),
                flags: ShapeFlag::empty(),
                styles: ShapeStyles {
                    fill_styles: vec![FillStyle::Color(colour)],
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
                    ShapeRecord::StraightEdge { delta: d(size, 0.0) },
                    ShapeRecord::StraightEdge { delta: d(0.0, size) },
                    ShapeRecord::StraightEdge { delta: d(-size, 0.0) },
                    ShapeRecord::StraightEdge { delta: d(0.0, -size) },
                ],
            })
        };

        // The character comes from `action`; the id is named for readability
        // at the call sites below.
        let place = |depth: u16, _id: u16, x: f64, y: f64, action: PlaceObjectAction| {
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action,
                depth,
                matrix: Some(Matrix::translate(
                    Twips::from_pixels(x),
                    Twips::from_pixels(y),
                )),
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
            stage_size: bounds(400.0, 300.0),
            frame_rate: Fixed8::from_f32(12.0),
            num_frames: 4,
        };

        let tags = vec![
            Tag::SetBackgroundColor(swf::Color { r: 0xEE, g: 0xEE, b: 0xF4, a: 255 }),
            shape(1, 60.0, swf::Color { r: 0xCC, g: 0x33, b: 0x33, a: 255 }),
            shape(2, 40.0, swf::Color { r: 0x33, g: 0xAA, b: 0x55, a: 255 }),
            place(1, 1, 20.0, 20.0, PlaceObjectAction::Place(1)),
            place(2, 2, 200.0, 40.0, PlaceObjectAction::Place(2)),
            Tag::ShowFrame,
            place(1, 1, 80.0, 60.0, PlaceObjectAction::Modify),
            Tag::ShowFrame,
            place(1, 1, 140.0, 100.0, PlaceObjectAction::Modify),
            Tag::ShowFrame,
            Tag::RemoveObject(RemoveObject { depth: 2, character_id: None }),
            Tag::ShowFrame,
        ];

        let mut bytes = Vec::new();
        swf::write_swf(&header, &tags, &mut bytes).unwrap();
        std::fs::write(&swf_path, bytes).unwrap();
    }
    println!("wrote {}", swf_path.display());

    // -- .pdf ---------------------------------------------------------------
    let pdf = dir.join("fixture.pdf");
    {
        let content = "\
0.2 0.4 0.8 rg
20 20 160 80 re
f
0.9 0.3 0.2 rg
40 120 m
100 220 160 220 220 120 c
h
f
0 0 0 RG
4 w
20 240 m
260 240 l
S";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 280] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];

        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        std::fs::write(&pdf, out).unwrap();
    }
    println!("wrote {}", pdf.display());
}
