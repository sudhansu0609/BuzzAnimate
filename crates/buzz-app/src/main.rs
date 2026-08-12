//! BuzzAnimate — GPU-accelerated vector animation.
//!
//! Phase 0 shell. It exists to demonstrate the three properties the whole
//! project is built around, before any authoring features are layered on:
//!
//! 1. **Unbounded zoom** — no 2000% ceiling, and no precision collapse.
//! 2. **True multicore** — a work-stealing pool across every available thread.
//! 3. **GPU rasterisation** — Vello compute shaders on the discrete adapter.

use anyhow::Result;
use buzz_app::app;
use buzz_render::GpuPreference;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "buzz_app=info,buzz_render=info,buzz_jobs=info".into()),
        )
        .init();

    // `--gpu <substring>` forces a specific adapter; `--gpu-list` is handled by
    // the report printed at startup.
    let preference = parse_gpu_preference(std::env::args().skip(1));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(preference);

    // A trailing path argument opens that file, so the app can be associated
    // with `.buzz` and with every format it can import.
    if let Some(path) = std::env::args().skip(1).find(|a| !a.starts_with("--")) {
        let path = std::path::PathBuf::from(path);
        let is_document = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(buzz_doc::EXTENSION));

        if is_document {
            match buzz_doc::Document::open(&path) {
                Ok(doc) => app = app.with_document(doc),
                Err(e) => eprintln!("could not open {}: {e}", path.display()),
            }
        } else {
            // Importing at startup opens a *new, unsaved* document, so Ctrl+S
            // cannot overwrite the file that was imported from.
            match buzz_app::import::read(&path) {
                Ok(imported) => {
                    println!("imported {}: {}", path.display(), imported.summary);
                    for line in &imported.unsupported {
                        println!("  did not come across: {line}");
                    }
                    app = app.with_document(buzz_doc::Document::new(imported.scene));
                }
                Err(e) => eprintln!("could not import {}: {e}", path.display()),
            }
        }
    }

    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Read the adapter override from the command line.
fn parse_gpu_preference(args: impl Iterator<Item = String>) -> GpuPreference {
    let args: Vec<String> = args.collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--gpu" if i + 1 < args.len() => {
                let v = &args[i + 1];
                return match v.parse::<usize>() {
                    Ok(n) => GpuPreference::ByIndex(n),
                    Err(_) => GpuPreference::ByName(v.clone()),
                };
            }
            "--integrated" => return GpuPreference::PreferIntegrated,
            _ => {}
        }
        i += 1;
    }
    GpuPreference::Automatic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> GpuPreference {
        parse_gpu_preference(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_to_automatic_selection() {
        assert_eq!(parse(&[]), GpuPreference::Automatic);
        assert_eq!(parse(&["--unrelated"]), GpuPreference::Automatic);
    }

    #[test]
    fn gpu_flag_accepts_a_name_or_an_index() {
        assert_eq!(parse(&["--gpu", "NVIDIA"]), GpuPreference::ByName("NVIDIA".into()));
        assert_eq!(parse(&["--gpu", "2"]), GpuPreference::ByIndex(2));
    }

    #[test]
    fn integrated_flag_is_recognised() {
        assert_eq!(parse(&["--integrated"]), GpuPreference::PreferIntegrated);
    }

    #[test]
    fn a_trailing_gpu_flag_with_no_value_is_ignored() {
        assert_eq!(parse(&["--gpu"]), GpuPreference::Automatic);
    }
}
