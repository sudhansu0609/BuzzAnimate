//! Import an Animate document and print what came across.
//!
//! Small documents are covered by the unit tests; the ones that find real bugs
//! are somebody's production files, which cannot live in the repository. This
//! points the importer at one on disk:
//!
//! ```text
//! cargo run -p buzz-import-xfl --example report -- "path/to/scene.fla"
//! ```

/// The frame named after `--camera`, for reading the state there.
fn frame_argument() -> u32 {
    std::env::args()
        .skip_while(|a| a != "--camera")
        .nth(1)
        .and_then(|f| f.parse().ok())
        .unwrap_or(0)
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: report <file.fla | folder.xfl>");
        std::process::exit(2);
    };

    let started = std::time::Instant::now();
    match buzz_import_xfl::import(&path) {
        Ok((scene, report)) => {
            println!("{}", report.summary());
            println!(
                "stage {:?} at {} fps, {} frames, read in {:.1}s",
                scene.stage().size,
                scene.stage().frame_rate,
                scene.frame_count(),
                started.elapsed().as_secs_f64()
            );
            // `--layers <frame>` lists the stack front to back with what each
            // layer is drawing there, which is how a misplaced or
            // wrongly-ordered layer is found without opening the window.
            if let Some(frame) = std::env::args()
                .skip_while(|a| a != "--layers")
                .nth(1)
                .and_then(|f| f.parse::<u32>().ok())
            {
                println!("\nlayers at frame {frame}, front to back:");
                for layer in scene.layers().iter() {
                    let objects = layer.objects_at(frame);
                    for object in objects.iter() {
                        if let Some(instance) = object.instance() {
                            let name = scene
                                .library()
                                .get(instance.symbol)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "?".into());
                            let c = object.transform.as_coeffs();
                            println!(
                                "      {:<26} scale {:.3},{:.3} at {:.0},{:.0}  first {} {:?}",
                                name, c[0], c[3], c[4], c[5], instance.first_frame,
                                instance.loop_mode
                            );
                        }
                    }
                    let bounds = layer.bounds_at(frame);
                    println!(
                        "  {:<28} {:<8} {:>3} objects  {}",
                        layer.name,
                        layer.kind.display_name(),
                        objects.len(),
                        match bounds {
                            Some(b) => format!(
                                "{:.0},{:.0} .. {:.0},{:.0}",
                                b.x0, b.y0, b.x1, b.y1
                            ),
                            None => "empty".to_string(),
                        }
                    );
                }
            }

            // `--symbol <name>` looks inside one symbol: its layers, how they
            // are linked, and what each holds at frame 0. A rigged character
            // that arrives in pieces is a question about this, not about the
            // stage.
            if let Some(wanted) = std::env::args()
                .skip_while(|a| a != "--symbol")
                .nth(1)
            {
                let at: u32 = std::env::args()
                    .skip_while(|a| a != "--symbol")
                    .nth(2)
                    .and_then(|f| f.parse().ok())
                    .unwrap_or(0);
                for symbol in scene.library().iter().filter(|s| s.name.contains(&wanted)) {
                    println!(
                        "\nsymbol {} #{} ({:?}), {} frames, bounds {}, at frame {at}:",
                        symbol.path(),
                        symbol.id.0,
                        symbol.kind,
                        symbol.length(),
                        match symbol.bounds() {
                            Some(b) => format!(
                                "{:.0},{:.0} .. {:.0},{:.0}",
                                b.x0, b.y0, b.x1, b.y1
                            ),
                            None => "empty".into(),
                        }
                    );
                    for layer in symbol.layers.iter() {
                        let objects = layer.objects_at(at);
                        let inside: Vec<String> = objects
                            .iter()
                            .filter_map(|o| o.instance())
                            .zip(objects.iter().map(|o| o.transform.as_coeffs()))
                            .map(|(i, c)| {
                                format!(
                                    "[{:.2},{:.2} at {:.0},{:.0}] {}#{}@{}",
                                    c[0],
                                    c[3],
                                    c[4],
                                    c[5],
                                    scene
                                        .library()
                                        .get(i.symbol)
                                        .map(|s| s.path())
                                        .unwrap_or_else(|| "?".into()),
                                    i.symbol.0,
                                    i.first_frame
                                )
                            })
                            .collect();
                        println!(
                            "  {:<24} {:<12} follows {:<6} {:>2} objects  {}",
                            layer.name,
                            layer.kind.display_name(),
                            layer
                                .follows
                                .map(|f| f.0.to_string())
                                .unwrap_or_else(|| "-".into()),
                            objects.len(),
                            inside.join(", ")
                        );
                    }
                }
            }

            if std::env::args().any(|a| a == "--camera") {
                let camera = scene.camera();
                println!(
                    "camera: {} keys, {}",
                    camera.keys().len(),
                    if camera.enabled { "on" } else { "off" }
                );
                for key in camera.keys() {
                    println!(
                        "  frame {:>5}  centre {:.1},{:.1}  zoom {:.3}",
                        key.frame, key.center.x, key.center.y, key.zoom
                    );
                }
                if let Some(state) = camera.state_at(frame_argument()) {
                    println!(
                        "  at frame {}: centre {:.1},{:.1} zoom {:.3}",
                        frame_argument(),
                        state.center.x,
                        state.center.y,
                        state.zoom
                    );
                }
            }

            // A symbol that holds nothing is the quietest import failure there
            // is: the instance places correctly and draws nothing at all.
            let empty: Vec<&str> = scene
                .library()
                .iter()
                .filter(|s| s.bounds().is_none())
                .map(|s| s.name.as_str())
                .collect();
            println!(
                "symbols holding no artwork: {} of {}",
                empty.len(),
                scene.library().iter().count()
            );
            for name in empty.iter().take(15) {
                println!("  - {name}");
            }

            if report.unsupported.is_empty() {
                println!("everything came across");
            } else {
                println!("\nnot imported:");
                for line in &report.unsupported {
                    println!("  - {line}");
                }
            }
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    }
}
