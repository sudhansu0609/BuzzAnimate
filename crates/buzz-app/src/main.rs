//! BuzzAnimate — GPU-accelerated vector animation.
//!
//! Phase 0 shell. It exists to demonstrate the three properties the whole
//! project is built around, before any authoring features are layered on:
//!
//! 1. **Unbounded zoom** — no 2000% ceiling, and no precision collapse.
//! 2. **True multicore** — a work-stealing pool across every available thread.
//! 3. **GPU rasterisation** — Vello compute shaders on the discrete adapter.
//!
//! # One window, not two
//!
//! A release build is a **Windows GUI application**, so double-clicking it
//! opens the editor and nothing else. Built as an ordinary console program it
//! also opened a black terminal alongside the window — which is what a user
//! reports, reasonably, as the program opening twice.
//!
//! A **debug** build keeps its console, because that is where the adapter
//! table, the tracing output and a panic's backtrace go, and `--dev` on the
//! launcher is how you ask for it. The diagnostics are not lost, they are
//! behind a flag; that is the trade, and it is the right way round for a
//! program whose users are animators rather than its author.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use buzz_app::app;
use buzz_render::GpuPreference;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> Result<()> {
    // **A person launched this, so the layout on screen is theirs to keep.**
    // Nothing else in the workspace claims it, which is what stops a test run
    // rearranging the panels of whoever is running the suite. See
    // `buzz_ui::workspace::claim_user_workspace`.
    buzz_ui::workspace::claim_user_workspace();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "buzz_app=info,buzz_render=info,buzz_jobs=info".into()),
        )
        .init();

    // `--gpu <substring>` forces a specific adapter; `--script <file>` runs a
    // script over the document once it is open.
    let args = Args::parse(std::env::args().skip(1));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(args.gpu);

    // A trailing path argument opens that file, so the app can be associated
    // with `.buzz` and with every format it can import.
    if let Some(path) = args.document {
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

    // The script runs *after* the document is open, so it operates on the file
    // that was asked for rather than on the empty one it replaced.
    if let Some(path) = args.script {
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                app = app.with_script(source);
                let (trace, error) = app.script_report();
                for line in trace {
                    println!("{line}");
                }
                if let Some(error) = error {
                    eprintln!("{}: {error}", path.display());
                }
            }
            Err(e) => eprintln!("could not read {}: {e}", path.display()),
        }
    }

    event_loop.run_app(&mut app)?;
    Ok(())
}

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Args {
    gpu: GpuPreference,
    /// A document to open, or a foreign file to import.
    document: Option<std::path::PathBuf>,
    /// A script to run once the document is open.
    script: Option<std::path::PathBuf>,
}

impl Args {
    /// Parse the command line.
    ///
    /// Flag *values* are skipped when looking for the trailing path, which the
    /// first version did not do: `--gpu NVIDIA` took the adapter name for a
    /// file and reported that it could not be imported.
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let args: Vec<String> = args.collect();
        let mut out = Args {
            gpu: GpuPreference::Automatic,
            document: None,
            script: None,
        };

        let mut i = 0;
        while i < args.len() {
            let value = args.get(i + 1).filter(|v| !v.starts_with("--"));
            match args[i].as_str() {
                "--gpu" => {
                    if let Some(v) = value {
                        out.gpu = match v.parse::<usize>() {
                            Ok(n) => GpuPreference::ByIndex(n),
                            Err(_) => GpuPreference::ByName(v.clone()),
                        };
                        i += 1;
                    }
                }
                "--script" => {
                    if let Some(v) = value {
                        out.script = Some(std::path::PathBuf::from(v));
                        i += 1;
                    }
                }
                "--integrated" => out.gpu = GpuPreference::PreferIntegrated,
                other if !other.starts_with("--") && out.document.is_none() => {
                    out.document = Some(std::path::PathBuf::from(other));
                }
                _ => {}
            }
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Args {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_to_automatic_selection() {
        assert_eq!(parse(&[]).gpu, GpuPreference::Automatic);
        assert_eq!(parse(&["--unrelated"]).gpu, GpuPreference::Automatic);
    }

    #[test]
    fn gpu_flag_accepts_a_name_or_an_index() {
        assert_eq!(
            parse(&["--gpu", "NVIDIA"]).gpu,
            GpuPreference::ByName("NVIDIA".into())
        );
        assert_eq!(parse(&["--gpu", "2"]).gpu, GpuPreference::ByIndex(2));
    }

    #[test]
    fn integrated_flag_is_recognised() {
        assert_eq!(
            parse(&["--integrated"]).gpu,
            GpuPreference::PreferIntegrated
        );
    }

    #[test]
    fn a_trailing_gpu_flag_with_no_value_is_ignored() {
        assert_eq!(parse(&["--gpu"]).gpu, GpuPreference::Automatic);
        assert_eq!(parse(&["--gpu"]).document, None);
    }

    /// The defect this parser replaced: the adapter name was picked up as a
    /// file to open, so `--gpu NVIDIA` reported that it could not import a file
    /// called NVIDIA.
    #[test]
    fn a_flag_value_is_not_mistaken_for_a_file_to_open() {
        let args = parse(&["--gpu", "NVIDIA", "art.buzz"]);
        assert_eq!(args.gpu, GpuPreference::ByName("NVIDIA".into()));
        assert_eq!(args.document, Some(PathBuf::from("art.buzz")));

        assert_eq!(parse(&["--gpu", "NVIDIA"]).document, None);
        assert_eq!(parse(&["--script", "grid.js"]).document, None);
    }

    #[test]
    fn a_script_can_be_given_alongside_a_document() {
        let args = parse(&["art.buzz", "--script", "grid.js"]);
        assert_eq!(args.document, Some(PathBuf::from("art.buzz")));
        assert_eq!(args.script, Some(PathBuf::from("grid.js")));
    }

    /// Two paths is a mistake, and taking the first is at least predictable.
    #[test]
    fn only_the_first_path_is_opened() {
        let args = parse(&["one.buzz", "two.buzz"]);
        assert_eq!(args.document, Some(PathBuf::from("one.buzz")));
    }
}
