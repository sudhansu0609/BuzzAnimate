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
    // Poll rather than Wait: the HUD animates live CPU utilisation.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new(preference);
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
