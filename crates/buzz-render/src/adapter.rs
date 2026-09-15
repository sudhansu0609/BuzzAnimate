//! Choosing the right GPU.
//!
//! On a clean laptop `PowerPreference::HighPerformance` is enough. On a real
//! workstation it is not. The reference machine for this project enumerates
//! four display adapters:
//!
//! ```text
//! NVIDIA GeForce RTX 5060 Ti     <- the one we want
//! Intel(R) UHD Graphics 770      <- integrated, much slower
//! Meta Virtual Monitor           <- virtual display driver
//! MrIdd Device                   <- virtual display driver
//! ```
//!
//! Virtual display drivers installed by VR software, capture tools and tablet
//! utilities present themselves as adapters. Picking one silently gives
//! software rasterisation, and the user experiences "the GPU version is slower
//! than Animate" with no indication why.
//!
//! So BuzzAnimate enumerates every adapter, scores them explicitly, logs the
//! full table, and lets the user override the result. Being wrong here is both
//! easy and invisible, which is exactly why it gets real code instead of one
//! call to `request_adapter`.

use std::fmt;

use serde::{Deserialize, Serialize};
use wgpu::{Adapter, AdapterInfo, Backends, DeviceType, Instance};

/// PCI vendor IDs, used to recognise hardware whose device type is ambiguous.
mod vendor {
    pub const NVIDIA: u32 = 0x10DE;
    pub const AMD: u32 = 0x1002;
    pub const INTEL: u32 = 0x8086;
    /// Microsoft Basic Render Driver / WARP — a CPU rasteriser.
    pub const MICROSOFT: u32 = 0x1414;
}

/// How the user wants the GPU chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuPreference {
    /// Score every adapter and take the best. The default.
    #[default]
    Automatic,
    /// Take the first adapter whose name contains this substring,
    /// case-insensitively. Set from the settings UI.
    ByName(String),
    /// Take the adapter at this index in the enumeration order.
    ByIndex(usize),
    /// Prefer integrated graphics — useful on a laptop running on battery.
    PreferIntegrated,
}

/// One adapter and why it did or did not win.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub index: usize,
    pub info: AdapterInfo,
    /// Higher is better. `None` means disqualified.
    pub score: Option<i64>,
    /// Human-readable justification, shown in diagnostics.
    pub reason: String,
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let score = match self.score {
            Some(s) => format!("{s:>6}"),
            None => "  n/a".to_string(),
        };
        write!(
            f,
            "[{}] {score}  {:<34} {:?}/{:?}  {}",
            self.index, self.info.name, self.info.device_type, self.info.backend, self.reason
        )
    }
}

/// The outcome of adapter selection, including the adapters that lost.
pub struct Selection {
    pub adapter: Adapter,
    pub chosen: Candidate,
    pub candidates: Vec<Candidate>,
}

impl Selection {
    /// One-line summary for the HUD.
    pub fn summary(&self) -> String {
        let i = &self.chosen.info;
        format!("{} ({:?}, {:?})", i.name, i.device_type, i.backend)
    }

    /// Multi-line diagnostic table.
    pub fn report(&self) -> String {
        let mut s = String::from("GPU adapters:\n");
        for c in &self.candidates {
            let marker = if c.index == self.chosen.index {
                "->"
            } else {
                "  "
            };
            s.push_str(&format!("{marker} {c}\n"));
        }
        s
    }
}

/// Why no adapter could be used.
#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    #[error("no GPU adapters found; a DX12 or Vulkan capable driver is required")]
    NoAdapters,
    #[error("every adapter was disqualified:\n{0}")]
    AllDisqualified(String),
    #[error("no adapter matched the requested override ({0}); available:\n{1}")]
    OverrideNotFound(String, String),
}

/// The only adapter properties scoring actually depends on.
///
/// Deliberately not `AdapterInfo`: that struct gains fields between wgpu
/// releases, and tests that had to construct one would break on every bump
/// without the scoring logic having changed at all.
#[derive(Debug, Clone, Copy)]
struct Traits<'a> {
    name: &'a str,
    vendor: u32,
    device_type: DeviceType,
    backend: wgpu::Backend,
}

impl<'a> From<&'a AdapterInfo> for Traits<'a> {
    fn from(i: &'a AdapterInfo) -> Self {
        Self {
            name: &i.name,
            vendor: i.vendor,
            device_type: i.device_type,
            backend: i.backend,
        }
    }
}

/// Score an adapter. Higher is better; `None` disqualifies it.
///
/// Weights are chosen so device type dominates: a discrete GPU on a mediocre
/// backend still beats integrated graphics on the best one.
fn score(info: &Traits<'_>, prefer_integrated: bool) -> (Option<i64>, String) {
    // A CPU rasteriser will technically run Vello, at unusable speed. Refusing
    // it produces a clear error instead of a mysteriously slow application.
    if info.device_type == DeviceType::Cpu || info.vendor == vendor::MICROSOFT {
        return (
            None,
            "software rasteriser (would be far slower than Animate)".into(),
        );
    }

    let mut score = 0i64;
    let mut notes: Vec<&str> = Vec::new();

    score += match info.device_type {
        DeviceType::DiscreteGpu if prefer_integrated => 200,
        DeviceType::DiscreteGpu => 1000,
        DeviceType::IntegratedGpu if prefer_integrated => 1000,
        DeviceType::IntegratedGpu => 300,
        DeviceType::VirtualGpu => 50,
        DeviceType::Other => 10,
        DeviceType::Cpu => unreachable!("filtered above"),
    };

    match info.device_type {
        DeviceType::DiscreteGpu => notes.push("discrete"),
        DeviceType::IntegratedGpu => notes.push("integrated"),
        DeviceType::VirtualGpu => notes.push("virtual"),
        _ => notes.push("unknown class"),
    }

    // Backend quality for Vello's compute pipelines on Windows. DX12 has the
    // most consistent driver support; GL cannot run compute shaders well
    // enough for Vello and is a last resort.
    score += match info.backend {
        wgpu::Backend::Dx12 => 100,
        wgpu::Backend::Vulkan => 90,
        wgpu::Backend::Metal => 90,
        wgpu::Backend::BrowserWebGpu => 60,
        wgpu::Backend::Gl => 5,
        // A no-op backend renders nothing at all; never select it.
        wgpu::Backend::Noop => -1000,
    };

    if info.backend == wgpu::Backend::Gl {
        notes.push("GL backend is a poor fit for Vello compute");
    }

    // A dedicated-graphics vendor reporting a non-discrete type is usually a
    // driver quirk rather than genuinely weak hardware.
    match info.vendor {
        vendor::NVIDIA | vendor::AMD => {
            score += 20;
            notes.push("dedicated-graphics vendor");
        }
        vendor::INTEL => notes.push("Intel"),
        _ => {}
    }

    // Virtual monitor drivers routinely report themselves as ordinary
    // adapters. Demote by name where the device type does not reveal them.
    let lname = info.name.to_ascii_lowercase();
    const VIRTUAL_HINTS: [&str; 6] = [
        "virtual",
        "remote",
        "idd",
        "mirror",
        "basic render",
        "software",
    ];
    if VIRTUAL_HINTS.iter().any(|h| lname.contains(h)) {
        score -= 900;
        notes.push("looks like a virtual display driver");
    }

    (Some(score), notes.join(", "))
}

/// Enumerate, score and pick a GPU.
///
/// Enumerates DX12 and Vulkan only: on Windows those are the backends that can
/// actually run Vello's compute pipelines, and including GL would let a weak
/// fallback win on a machine where the real driver failed to load.
pub async fn select(
    instance: &Instance,
    preference: &GpuPreference,
) -> Result<Selection, SelectionError> {
    let adapters = instance
        .enumerate_adapters(Backends::DX12 | Backends::VULKAN)
        .await;

    if adapters.is_empty() {
        return Err(SelectionError::NoAdapters);
    }

    let prefer_integrated = matches!(preference, GpuPreference::PreferIntegrated);

    let candidates: Vec<Candidate> = adapters
        .iter()
        .enumerate()
        .map(|(index, a)| {
            let info = a.get_info();
            let (score, reason) = score(&Traits::from(&info), prefer_integrated);
            Candidate {
                index,
                info,
                score,
                reason,
            }
        })
        .collect();

    let table = || {
        candidates
            .iter()
            .map(|c| format!("  {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Explicit overrides bypass scoring entirely, including disqualification —
    // if a user insists on a specific adapter, that is their call to make.
    let picked: usize = match preference {
        GpuPreference::ByName(want) => {
            let want_lower = want.to_ascii_lowercase();
            candidates
                .iter()
                .find(|c| c.info.name.to_ascii_lowercase().contains(&want_lower))
                .map(|c| c.index)
                .ok_or_else(|| {
                    SelectionError::OverrideNotFound(format!("name contains {want:?}"), table())
                })?
        }
        GpuPreference::ByIndex(i) => {
            if *i < candidates.len() {
                *i
            } else {
                return Err(SelectionError::OverrideNotFound(
                    format!("index {i}"),
                    table(),
                ));
            }
        }
        GpuPreference::Automatic | GpuPreference::PreferIntegrated => candidates
            .iter()
            .filter_map(|c| c.score.map(|s| (s, c.index)))
            // `max_by_key` keeps the last maximum; comparing on the negated
            // index makes ties resolve to the first adapter, which is the
            // system's own preferred order.
            .max_by_key(|(s, i)| (*s, -(*i as i64)))
            .map(|(_, i)| i)
            .ok_or_else(|| SelectionError::AllDisqualified(table()))?,
    };

    let chosen = candidates[picked].clone();
    let adapter = adapters.into_iter().nth(picked).expect("index in range");

    tracing::info!(
        adapter = %chosen.info.name,
        backend = ?chosen.info.backend,
        device_type = ?chosen.info.device_type,
        driver = %chosen.info.driver_info,
        "selected GPU"
    );

    Ok(Selection {
        adapter,
        chosen,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(
        name: &str,
        device_type: DeviceType,
        backend: wgpu::Backend,
        vendor: u32,
    ) -> Traits<'_> {
        Traits {
            name,
            vendor,
            device_type,
            backend,
        }
    }

    #[test]
    fn discrete_nvidia_outscores_intel_integrated() {
        let (nv, _) = score(
            &info(
                "NVIDIA GeForce RTX 5060 Ti",
                DeviceType::DiscreteGpu,
                wgpu::Backend::Dx12,
                vendor::NVIDIA,
            ),
            false,
        );
        let (intel, _) = score(
            &info(
                "Intel(R) UHD Graphics 770",
                DeviceType::IntegratedGpu,
                wgpu::Backend::Dx12,
                vendor::INTEL,
            ),
            false,
        );
        assert!(
            nv > intel,
            "discrete {nv:?} should beat integrated {intel:?}"
        );
    }

    #[test]
    fn software_rasterisers_are_disqualified_outright() {
        let (cpu, _) = score(
            &info("llvmpipe", DeviceType::Cpu, wgpu::Backend::Vulkan, 0),
            false,
        );
        assert_eq!(cpu, None);

        let (warp, _) = score(
            &info(
                "Microsoft Basic Render Driver",
                DeviceType::DiscreteGpu,
                wgpu::Backend::Dx12,
                vendor::MICROSOFT,
            ),
            false,
        );
        assert_eq!(warp, None, "WARP must never be selected silently");
    }

    /// The specific failure this module exists to prevent.
    #[test]
    fn virtual_display_drivers_lose_to_real_hardware() {
        let (real, _) = score(
            &info(
                "NVIDIA GeForce RTX 5060 Ti",
                DeviceType::DiscreteGpu,
                wgpu::Backend::Dx12,
                vendor::NVIDIA,
            ),
            false,
        );
        for fake in [
            "Meta Virtual Monitor",
            "MrIdd Device",
            "Remote Display Adapter",
            "Generic Software Adapter",
        ] {
            let (s, reason) = score(
                &info(fake, DeviceType::DiscreteGpu, wgpu::Backend::Dx12, 0),
                false,
            );
            assert!(
                s < real,
                "{fake:?} scored {s:?} against real hardware {real:?} ({reason})"
            );
        }
    }

    #[test]
    fn integrated_wins_only_when_explicitly_preferred() {
        let nv = info(
            "NVIDIA GeForce RTX 5060 Ti",
            DeviceType::DiscreteGpu,
            wgpu::Backend::Dx12,
            vendor::NVIDIA,
        );
        let intel = info(
            "Intel(R) UHD Graphics 770",
            DeviceType::IntegratedGpu,
            wgpu::Backend::Dx12,
            vendor::INTEL,
        );
        assert!(score(&intel, true).0 > score(&nv, true).0);
        assert!(score(&intel, false).0 < score(&nv, false).0);
    }

    #[test]
    fn gl_backend_is_a_last_resort() {
        let dx12 = info(
            "GPU",
            DeviceType::DiscreteGpu,
            wgpu::Backend::Dx12,
            vendor::AMD,
        );
        let gl = info(
            "GPU",
            DeviceType::DiscreteGpu,
            wgpu::Backend::Gl,
            vendor::AMD,
        );
        assert!(score(&dx12, false).0 > score(&gl, false).0);
    }

    /// Runs against whatever hardware is actually present.
    #[test]
    fn selects_a_real_adapter_on_this_machine() {
        let instance = Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let selection = pollster::block_on(select(&instance, &GpuPreference::Automatic));

        match selection {
            Ok(sel) => {
                println!("{}", sel.report());
                assert!(sel.chosen.score.is_some(), "chose a disqualified adapter");
                assert_ne!(sel.chosen.info.device_type, DeviceType::Cpu);
            }
            Err(SelectionError::NoAdapters) => {
                eprintln!("skipping: no DX12/Vulkan adapters (headless CI?)");
            }
            Err(e) => panic!("adapter selection failed: {e}"),
        }
    }

    #[test]
    fn by_name_override_is_honoured() {
        let instance = Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let auto = match pollster::block_on(select(&instance, &GpuPreference::Automatic)) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skipping: no adapters");
                return;
            }
        };

        // Ask for the chosen adapter by a fragment of its own name.
        let fragment: String = auto.chosen.info.name.chars().take(6).collect();
        let forced =
            pollster::block_on(select(&instance, &GpuPreference::ByName(fragment.clone())))
                .expect("override should resolve");
        assert!(
            forced
                .chosen
                .info
                .name
                .to_ascii_lowercase()
                .contains(&fragment.to_ascii_lowercase())
        );

        let missing = pollster::block_on(select(
            &instance,
            &GpuPreference::ByName("definitely-not-a-real-gpu".into()),
        ));
        assert!(matches!(missing, Err(SelectionError::OverrideNotFound(..))));
    }
}
