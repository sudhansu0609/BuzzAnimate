//! **The user's own Rig panel parts**, `rig_parts.json` beside the workspace
//! file (as `rig_naming.json` is): a nose, a dupatta, a bag, added to a
//! template once and there in every session after (user 2026-11-18: "I shall
//! have the option to add more rigs and tag them ... make it the default
//! template"). A missing or unreadable file is no parts.

use std::path::PathBuf;

use buzz_ui::rig_panel::SavedPart;

/// Where the parts live: `rig_parts.json` beside `workspace.json`. A test's
/// scratch workspace gets a file of its own beside it, named for it, so a
/// test never writes the user's file or reads another run's parts.
pub fn path() -> PathBuf {
    let workspace = buzz_ui::workspace::workspace_path();
    match workspace.file_stem().and_then(|s| s.to_str()) {
        Some("workspace") | None => workspace.with_file_name("rig_parts.json"),
        Some(stem) => workspace.with_file_name(format!("{stem}-rig_parts.json")),
    }
}

/// The saved parts, oldest first.
pub fn load() -> Vec<SavedPart> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Keep `parts` as the user's.
pub fn save(parts: &[SavedPart]) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(parts).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}
