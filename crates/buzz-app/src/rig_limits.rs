//! **The user's own joint-limit defaults**, `rig_limits.json`: the per-slot
//! table every rig builder reads, so an elbow the animator has decided bends
//! one way bends that way on every character built after.
//!
//! `%APPDATA%/BuzzAnimate/rig_limits.json`, beside the export presets;
//! `BUZZ_RIG_LIMITS` overrides the path. A missing or unreadable file is the
//! built-in anatomical table, and a file that names only some slots overrides
//! only those.

use std::path::PathBuf;

use buzz_rig::{JointLimitDefaults, RigPattern};

/// Where the table lives.
pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("BUZZ_RIG_LIMITS") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("rig_limits.json")
}

/// The saved table laid over [`JointLimitDefaults::anatomical`]: a slot the
/// file names takes the file's row, every other slot keeps the built-in one.
pub fn load_or_default() -> JointLimitDefaults {
    let mut table = JointLimitDefaults::anatomical();
    if let Ok(text) = std::fs::read_to_string(path())
        && let Ok(saved) = serde_json::from_str::<JointLimitDefaults>(&text)
    {
        table.rows.extend(saved.rows);
    }
    table
}

/// Write `table` as the user's defaults.
pub fn save(table: &JointLimitDefaults) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(table).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

/// **The one way a rig builder gets its pattern**: the built-in pattern called
/// `name` (the biped when there is none by that name) with the saved joint
/// limits on its slots.
pub fn pattern_with_saved_limits(name: &str) -> RigPattern {
    let mut pattern = RigPattern::named(name).unwrap_or_else(RigPattern::biped);
    pattern.apply_limits(&load_or_default());
    pattern
}
