//! The user's private AI settings: `%APPDATA%/BuzzAnimate/ai.json`.
//!
//! Secrets (the Claude API server key) live here and only here — never in
//! `workspace.json`, which is shared layout, and never in the repo. The file
//! is written owner-only where the OS allows it (`0600` on Unix, an ACL with
//! inheritance removed on Windows via `icacls`).
//!
//! The Claude proxy key itself lives in `CLAUDE_API_KEY=` in the `.env` at the
//! BuzzAnimate folder root (git-ignored, like Buzzcaf Studio's); `ai.json`
//! keeps url/model/switch and is only a legacy fallback for the key.
//!
//! The environment variable `BUZZ_CLAUDE_API_KEY` overrides the stored key,
//! `BUZZ_AI_CONFIG` points at another file and `BUZZ_HOME` at another
//! BuzzAnimate folder (tests and CI).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default base URL of the user's local Claude API server (OpenAI shape).
pub const CLAUDE_API_DEFAULT_URL: &str = "http://127.0.0.1:8787/v1";

/// The Claude API server entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeApiConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub key: String,
    /// Model sent to the server when the caller names none ("opus").
    #[serde(default = "default_claude_model")]
    pub claude_model: String,
}

/// The default Claude model alias: Opus (5.5), the user's choice
/// (2026-10-06). The alias, not the full id: the proxy's Claude Code refused
/// "claude-opus-5-5" ("Couldn't confirm model") while resolving "opus" fine.
pub const CLAUDE_DEFAULT_MODEL: &str = "opus";

/// Reasoning effort asked of Claude per request, overriding the proxy's
/// server-wide default (which is "low"). The user's choice (2026-10-06).
pub const CLAUDE_DEFAULT_EFFORT: &str = "medium";

fn default_claude_model() -> String {
    CLAUDE_DEFAULT_MODEL.to_string()
}

impl Default for ClaudeApiConfig {
    fn default() -> Self {
        Self { base_url: String::new(), key: String::new(), claude_model: default_claude_model() }
    }
}

impl std::fmt::Debug for ClaudeApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeApiConfig")
            .field("base_url", &self.base_url)
            .field("key", &mask_key(&self.key))
            .field("claude_model", &self.claude_model)
            .finish()
    }
}

/// Everything in `ai.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub claude_api_server: Option<ClaudeApiConfig>,
    /// Explicit switch: the Assistant may use Claude (prompts leave the PC).
    #[serde(default)]
    pub use_claude: bool,
    /// Voice commands (Stage V): the mic mode. Default off.
    #[serde(default)]
    pub voice_mode: VoiceMode,
    /// Closes the mic in every mode until unmuted.
    #[serde(default)]
    pub voice_muted: bool,
    /// Older files: `voice_listen: true` meant "Hey Buzz" listening. Read by
    /// [`AiConfig::effective_voice_mode`]; kept in step by `set_voice_mode`.
    #[serde(default)]
    pub voice_listen: bool,
    /// Input device name for voice commands (`None` = system default).
    #[serde(default)]
    pub voice_device: Option<String>,
    /// "Allow all": voice commands run at once, agent confirmations are
    /// auto-approved and the agent's excluded commands are unlocked.
    #[serde(default)]
    pub allow_all: bool,
}

/// How the microphone is used. Persisted lower-case in `ai.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceMode {
    /// Mic never opens.
    #[default]
    Off,
    /// "Hey Buzz": always listening while on.
    Wake,
    /// Press the mic button / hotkey to record one command.
    Push,
}

impl VoiceMode {
    pub const ALL: [VoiceMode; 3] = [VoiceMode::Off, VoiceMode::Wake, VoiceMode::Push];

    pub fn label(self) -> &'static str {
        match self {
            VoiceMode::Off => "Off",
            VoiceMode::Wake => "Hey Buzz (always listening)",
            VoiceMode::Push => "Push to talk",
        }
    }
}

impl AiConfig {
    /// The mode in force: an old `voice_listen: true` with no `voice_mode`
    /// means [`VoiceMode::Wake`].
    pub fn effective_voice_mode(&self) -> VoiceMode {
        if self.voice_mode == VoiceMode::Off && self.voice_listen {
            VoiceMode::Wake
        } else {
            self.voice_mode
        }
    }

    pub fn set_voice_mode(&mut self, mode: VoiceMode) {
        self.voice_mode = mode;
        self.voice_listen = mode == VoiceMode::Wake;
    }
}

/// Masks a key for display: `sk-local-…` for the local server's keys, the
/// first three characters and an ellipsis otherwise, `(none)` when empty.
pub fn mask_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        "(none)".into()
    } else if key.starts_with("sk-local-") {
        "sk-local-…".into()
    } else {
        let head: String = key.chars().take(3).collect();
        format!("{head}…")
    }
}

/// Where `ai.json` lives.
pub fn ai_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BUZZ_AI_CONFIG") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("ai.json")
}

impl AiConfig {
    /// Reads `ai.json`; a missing or damaged file gives the empty config.
    pub fn load() -> Self {
        Self::load_from(&ai_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&ai_config_path())
    }

    /// Writes the file and restricts it to the current user.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, text)?;
        restrict_to_owner(path);
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(windows)]
fn restrict_to_owner(path: &Path) {
    use std::os::windows::process::CommandExt;
    let Ok(user) = std::env::var("USERNAME") else { return };
    // CREATE_NO_WINDOW: no console flash from the GUI app.
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:F"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}

#[cfg(not(any(unix, windows)))]
fn restrict_to_owner(_path: &Path) {}

/// The BuzzAnimate folder: `BUZZ_HOME` when it is a directory, else the first
/// ancestor of the executable, then of the working directory, that holds
/// `crates/buzz-app/Cargo.toml`. `None` when installed elsewhere.
pub fn buzz_home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("BUZZ_HOME").map(PathBuf::from) {
        if d.is_dir() {
            return Some(d);
        }
    }
    let starts = [std::env::current_exe().ok(), std::env::current_dir().ok()];
    starts
        .into_iter()
        .flatten()
        .flat_map(|s| s.ancestors().map(Path::to_path_buf).collect::<Vec<_>>())
        .find(|a| a.join("crates").join("buzz-app").join("Cargo.toml").is_file())
}

/// The `.env` at the BuzzAnimate folder root.
pub fn env_file() -> Option<PathBuf> {
    buzz_home().map(|h| h.join(".env"))
}

const ENV_KEY_NAME: &str = "CLAUDE_API_KEY";

fn env_var_key() -> Option<String> {
    std::env::var("BUZZ_CLAUDE_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

fn dotenv_key() -> Option<String> {
    dotenv_value(&env_file()?, ENV_KEY_NAME)
}

fn json_key() -> Option<String> {
    AiConfig::load()
        .claude_api_server
        .map(|c| c.key.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// The Claude API server key: `BUZZ_CLAUDE_API_KEY`, then `CLAUDE_API_KEY` in
/// BuzzAnimate's `.env`, then the legacy key in `ai.json`.
/// (Studio's key is only ever copied in by [`import_from_studio`].)
pub fn claude_api_key() -> Option<String> {
    env_var_key().or_else(dotenv_key).or_else(json_key)
}

/// Where the key in use comes from: `"environment"`, `".env"` or `"ai.json"`.
/// When no key exists yet, where one would be saved.
pub fn key_location() -> &'static str {
    if env_var_key().is_some() {
        "environment"
    } else if dotenv_key().is_some() {
        ".env"
    } else if json_key().is_some() || env_file().is_none() {
        "ai.json"
    } else {
        ".env"
    }
}

/// Sets `CLAUDE_API_KEY=<key>` in the `.env` of `home`: updates the line in
/// place, else appends; every other line is kept. Creates the file (with a
/// comment header) when missing and restricts it to the owner.
fn write_env_key(home: &Path, key: &str) -> std::io::Result<()> {
    let path = home.join(".env");
    let existing = std::fs::read_to_string(&path).ok();
    let mut out = String::new();
    match &existing {
        Some(text) => {
            let mut done = false;
            for raw in text.split_inclusive('\n') {
                let body = raw.trim_end_matches(['\r', '\n']);
                let eol = &raw[body.len()..];
                let t = body.trim();
                let t = t.strip_prefix("export ").unwrap_or(t);
                let is_key = !done
                    && !t.starts_with('#')
                    && t.split_once('=').is_some_and(|(k, _)| k.trim() == ENV_KEY_NAME);
                if is_key {
                    out.push_str(&format!("{ENV_KEY_NAME}={key}"));
                    out.push_str(if eol.is_empty() { "\n" } else { eol });
                    done = true;
                } else {
                    out.push_str(raw);
                }
            }
            if !done {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&format!("{ENV_KEY_NAME}={key}\n"));
            }
        }
        None => {
            out.push_str("# BuzzAnimate secrets (git-ignored). Never commit this file.\n");
            out.push_str(&format!("{ENV_KEY_NAME}={key}\n"));
        }
    }
    std::fs::write(&path, out)?;
    restrict_to_owner(&path);
    Ok(())
}

/// Stores the key in `home`'s `.env` and blanks it in `ai.json` (url/model
/// kept); with no `home` the key stays in `ai.json` as before.
fn store_key_in(home: Option<&Path>, key: &str) -> std::io::Result<()> {
    let mut cfg = AiConfig::load();
    let mut entry = cfg.claude_api_server.take().unwrap_or_default();
    match home {
        Some(h) => {
            write_env_key(h, key)?;
            entry.key.clear();
        }
        None => entry.key = key.to_string(),
    }
    cfg.claude_api_server = Some(entry);
    cfg.save()
}

/// The Claude API server base URL from `ai.json`, else the default.
pub fn claude_api_base_url() -> String {
    AiConfig::load()
        .claude_api_server
        .map(|c| c.base_url)
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| CLAUDE_API_DEFAULT_URL.to_string())
}

/// The saved Claude model, "sonnet" when none.
pub fn claude_model() -> String {
    AiConfig::load()
        .claude_api_server
        .map(|c| c.claude_model)
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(default_claude_model)
}

/// Whether the user switched Claude on for the Assistant.
pub fn use_claude() -> bool {
    AiConfig::load().use_claude
}

/// What an import copied. Never holds the raw key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub masked_key: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
}

/// Copies Buzzcaf Studio's Claude key into BuzzAnimate's `.env` (url/model,
/// when set, into `ai.json`).
pub fn import_from_studio() -> Result<ImportReport, String> {
    import_from_studio_into(buzz_home().as_deref())
}

fn import_from_studio_into(home: Option<&Path>) -> Result<ImportReport, String> {
    let dir = studio_dir().ok_or_else(|| "Buzzcaf Studio not found".to_string())?;
    let key = studio_claude_key(&dir)
        .ok_or_else(|| "Buzzcaf Studio has no Claude API key set".to_string())?;
    let url = studio_config_str(&dir, "claude_url");
    let model = studio_config_str(&dir, "claude_model");
    let mut cfg = AiConfig::load();
    let mut entry = cfg.claude_api_server.take().unwrap_or_default();
    if let Some(u) = &url {
        entry.base_url = u.clone();
    }
    if let Some(m) = &model {
        entry.claude_model = m.clone();
    }
    cfg.claude_api_server = Some(entry);
    cfg.save().map_err(|e| format!("could not save ai.json: {e}"))?;
    store_key_in(home, &key).map_err(|e| format!("could not save the key: {e}"))?;
    Ok(ImportReport { masked_key: mask_key(&key), base_url: url, model })
}

/// Imports from Studio only when neither `.env` nor `ai.json` has a key yet.
/// File IO only.
pub fn import_from_studio_if_missing() -> Option<ImportReport> {
    if dotenv_key().is_some() || json_key().is_some() {
        return None;
    }
    import_from_studio().ok()
}

/// Startup: makes sure the key lives in `.env`. A legacy `ai.json` key moves
/// into `.env` (and is blanked in `ai.json`); with no key anywhere, Studio's is
/// imported. File IO only; the key is never logged.
pub fn ensure_claude_key() {
    ensure_claude_key_in(buzz_home().as_deref());
}

fn ensure_claude_key_in(home: Option<&Path>) {
    let Some(h) = home else {
        let _ = import_from_studio_if_missing();
        return;
    };
    if dotenv_value(&h.join(".env"), ENV_KEY_NAME).is_some() {
        return;
    }
    match json_key() {
        Some(k) => {
            let _ = store_key_in(Some(h), &k);
        }
        None => {
            let _ = import_from_studio_into(Some(h));
        }
    }
}

/// Buzzcaf Studio's folder: `BUZZCAF_STUDIO_DIR`, else a `BuzzcafStudio`
/// folder beside one of the folders this program runs from (the two apps sit
/// side by side in the workspace). `None` when Studio is not installed.
pub fn studio_dir() -> Option<PathBuf> {
    let is_studio = |p: &Path| p.join("backend").join("config").is_dir();
    if let Some(d) = std::env::var_os("BUZZCAF_STUDIO_DIR").map(PathBuf::from) {
        if is_studio(&d) {
            return Some(d);
        }
    }
    let starts = [std::env::current_exe().ok(), std::env::current_dir().ok()];
    starts
        .into_iter()
        .flatten()
        .flat_map(|s| s.ancestors().map(Path::to_path_buf).collect::<Vec<_>>())
        .map(|a| a.join("BuzzcafStudio"))
        .find(|d| is_studio(d))
}

/// The Claude proxy key the way Buzzcaf Studio resolves it
/// (`integrations/llm.py::claude_settings`): `claude_api_key` in
/// `backend/config/config.json`, else `CLAUDE_API_KEY` from the environment,
/// the repo-root `.env`, then `backend/.env`.
pub fn studio_claude_key(studio: &Path) -> Option<String> {
    let non_empty = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    studio_config_str(studio, "claude_api_key")
        .or_else(|| std::env::var("CLAUDE_API_KEY").ok().and_then(non_empty))
        .or_else(|| dotenv_value(&studio.join(".env"), "CLAUDE_API_KEY"))
        .or_else(|| dotenv_value(&studio.join("backend").join(".env"), "CLAUDE_API_KEY"))
}

/// One non-empty string field of Studio's `backend/config/config.json`.
fn studio_config_str(studio: &Path, field: &str) -> Option<String> {
    let path = studio.join("backend").join("config").join("config.json");
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let v = json.get(field)?.as_str()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// `KEY=value` from a `.env` file (`export `, quotes and `#` comments allowed).
fn dotenv_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim();
        let v = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(" #").next().unwrap_or(v).trim(),
        };
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Stores the URL in `ai.json` and the key in BuzzAnimate's `.env` (in
/// `ai.json` when there is no BuzzAnimate folder). An empty key leaves the
/// stored key alone.
pub fn store_claude_api(base_url: &str, key: &str) -> std::io::Result<()> {
    store_claude_api_in(buzz_home().as_deref(), base_url, key)
}

fn store_claude_api_in(home: Option<&Path>, base_url: &str, key: &str) -> std::io::Result<()> {
    let mut cfg = AiConfig::load();
    let mut entry = cfg.claude_api_server.take().unwrap_or_default();
    entry.base_url = base_url.trim().to_string();
    cfg.claude_api_server = Some(entry);
    cfg.save()?;
    let key = key.trim();
    if key.is_empty() {
        return Ok(());
    }
    store_key_in(home, key)
}

/// Saves the Claude model, keeping url and key.
pub fn store_claude_model(model: &str) -> std::io::Result<()> {
    let mut cfg = AiConfig::load();
    let mut entry = cfg.claude_api_server.take().unwrap_or_default();
    entry.claude_model = model.trim().to_string();
    cfg.claude_api_server = Some(entry);
    cfg.save()
}

/// Saves the "use Claude for the Assistant" switch.
pub fn store_use_claude(on: bool) -> std::io::Result<()> {
    let mut cfg = AiConfig::load();
    cfg.use_claude = on;
    cfg.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_local_keys_and_others() {
        assert_eq!(mask_key("sk-local-TESTONLY123"), "sk-local-…");
        assert_eq!(mask_key("abcdef"), "abc…");
        assert_eq!(mask_key("  "), "(none)");
    }

    #[test]
    fn debug_never_prints_the_key() {
        let c = ClaudeApiConfig { base_url: "http://x".into(), key: "sk-local-TESTONLY123".into(), ..Default::default() };
        let s = format!("{c:?}");
        assert!(!s.contains("TESTONLY"), "{s}");
    }

    #[test]
    fn voice_settings_default_off_and_old_files_still_load() {
        // A file from before Stage V: no voice fields at all.
        let old: AiConfig = serde_json::from_str(r#"{"use_claude": true}"#).unwrap();
        assert!(old.use_claude);
        assert_eq!(old.voice_mode, VoiceMode::Off);
        assert!(!old.voice_muted && old.voice_device.is_none() && !old.allow_all);
        let on: AiConfig = serde_json::from_str(r#"{"allow_all": true}"#).unwrap();
        assert!(on.allow_all);
        assert_eq!(old.effective_voice_mode(), VoiceMode::Off);
        // The first-draft flag maps to Hey Buzz.
        let draft: AiConfig = serde_json::from_str(r#"{"voice_listen": true}"#).unwrap();
        assert_eq!(draft.effective_voice_mode(), VoiceMode::Wake);
        // Modes round-trip as lower-case words; setting a mode keeps the flag in step.
        let mut c = AiConfig::default();
        c.set_voice_mode(VoiceMode::Push);
        c.voice_muted = true;
        c.voice_device = Some("USB Mic".into());
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains(r#""voice_mode":"push""#), "{text}");
        let back: AiConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back, c);
        c.set_voice_mode(VoiceMode::Wake);
        assert!(c.voice_listen);
        c.set_voice_mode(VoiceMode::Off);
        assert_eq!(c.effective_voice_mode(), VoiceMode::Off);
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("buzz_ai_cfg_{}", std::process::id()));
        let path = dir.join("ai.json");
        let cfg = AiConfig {
            claude_api_server: Some(ClaudeApiConfig {
                base_url: "http://127.0.0.1:9/v1".into(),
                key: "sk-local-TESTONLY".into(),
                claude_model: "opus".into(),
            }),
            use_claude: true,
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(AiConfig::load_from(&path), cfg);
        let _ = std::fs::remove_dir_all(&dir);
        // A missing file loads as empty.
        assert_eq!(AiConfig::load_from(&path), AiConfig::default());
    }

    #[test]
    fn reads_the_key_the_way_buzzcaf_studio_does() {
        let studio = std::env::temp_dir().join(format!("buzz_studio_{}", std::process::id()));
        let config = studio.join("backend").join("config");
        std::fs::create_dir_all(&config).unwrap();

        // `.env` files: the repo root first, `backend/.env` after it.
        std::fs::write(studio.join("backend").join(".env"), "CLAUDE_API_KEY=sk-local-BACKEND\n").unwrap();
        std::fs::write(studio.join(".env"), "# keys\nexport CLAUDE_API_KEY=\"sk-local-ROOT\" # main\n").unwrap();
        if std::env::var_os("CLAUDE_API_KEY").is_none() {
            assert_eq!(studio_claude_key(&studio).as_deref(), Some("sk-local-ROOT"));
        }

        // `config.json` beats both, as in Studio's `claude_settings`.
        std::fs::write(
            config.join("config.json"),
            r#"{"claude_api_key": " sk-local-CONFIG ", "claude_url": "http://127.0.0.1:9/v1"}"#,
        )
        .unwrap();
        assert_eq!(studio_claude_key(&studio).as_deref(), Some("sk-local-CONFIG"));
        assert_eq!(studio_config_str(&studio, "claude_url").as_deref(), Some("http://127.0.0.1:9/v1"));

        // An empty field is not a key.
        std::fs::write(config.join("config.json"), r#"{"claude_api_key": ""}"#).unwrap();
        assert_eq!(studio_config_str(&studio, "claude_api_key"), None);

        let _ = std::fs::remove_dir_all(&studio);
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("buzz_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_studio(root: &Path, key: &str) {
        let config = root.join("backend").join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.json"),
            format!(r#"{{"claude_api_key":"{key}","claude_url":"http://127.0.0.1:9/v1","claude_model":"opus"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn import_copies_and_never_leaks_the_key() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let studio = temp("imp_studio");
        let out = temp("imp_out");
        fake_studio(&studio, "sk-local-FAKEIMPORT");
        unsafe { std::env::set_var("BUZZCAF_STUDIO_DIR", &studio); }
        unsafe { std::env::set_var("BUZZ_AI_CONFIG", out.join("ai.json")); }
        unsafe { std::env::set_var("BUZZ_HOME", &out); }
        let report = import_from_studio().unwrap();
        assert!(!format!("{report:?}").contains("FAKEIMPORT"));
        assert_eq!(report.model.as_deref(), Some("opus"));
        let c = AiConfig::load().claude_api_server.unwrap();
        assert_eq!(c.key, "");
        assert_eq!(dotenv_value(&out.join(".env"), "CLAUDE_API_KEY").as_deref(), Some("sk-local-FAKEIMPORT"));
        assert_eq!(c.base_url, "http://127.0.0.1:9/v1");
        assert_eq!(c.claude_model, "opus");
        assert!(!format!("{c:?}").contains("FAKEIMPORT"));
        unsafe { std::env::remove_var("BUZZCAF_STUDIO_DIR"); }
        unsafe { std::env::remove_var("BUZZ_AI_CONFIG"); }
        unsafe { std::env::remove_var("BUZZ_HOME"); }
        let _ = std::fs::remove_dir_all(&studio);
        let _ = std::fs::remove_dir_all(&out);
    }

    fn with_home<R>(name: &str, f: impl FnOnce(&Path) -> R) -> R {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let out = temp(name);
        let studio = temp(&format!("{name}_studio"));
        unsafe { std::env::set_var("BUZZ_HOME", &out); }
        unsafe { std::env::set_var("BUZZ_AI_CONFIG", out.join("ai.json")); }
        unsafe { std::env::set_var("BUZZCAF_STUDIO_DIR", &studio); }
        unsafe { std::env::remove_var("BUZZ_CLAUDE_API_KEY"); }
        let r = f(&out);
        unsafe { std::env::remove_var("BUZZ_HOME"); }
        unsafe { std::env::remove_var("BUZZ_AI_CONFIG"); }
        unsafe { std::env::remove_var("BUZZCAF_STUDIO_DIR"); }
        unsafe { std::env::remove_var("BUZZ_CLAUDE_API_KEY"); }
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&studio);
        r
    }

    #[test]
    fn import_if_missing_keeps_an_existing_key() {
        with_home("ifm", |home| {
            let studio = std::env::var_os("BUZZCAF_STUDIO_DIR").map(PathBuf::from).unwrap();
            fake_studio(&studio, "sk-local-FAKESTUDIO");
            store_claude_api("http://x/v1", "sk-local-FAKEOWN").unwrap();
            assert!(import_from_studio_if_missing().is_none());
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-FAKEOWN"));
            std::fs::remove_file(home.join(".env")).unwrap();
            assert!(import_from_studio_if_missing().is_some());
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-FAKESTUDIO"));
        });
    }

    #[test]
    fn store_writes_env_preserves_lines_and_updates_in_place() {
        with_home("env_store", |home| {
            let env = home.join(".env");
            std::fs::write(&env, "# mine\nFOO=bar\nCLAUDE_API_KEY=old\nBAZ=1").unwrap();
            store_claude_api("http://u/v1", "sk-local-FAKEA").unwrap();
            assert_eq!(
                std::fs::read_to_string(&env).unwrap(),
                "# mine\nFOO=bar\nCLAUDE_API_KEY=sk-local-FAKEA\nBAZ=1"
            );
            std::fs::write(&env, "# mine\nFOO=bar").unwrap();
            store_claude_api("http://u/v1", "sk-local-FAKEB").unwrap();
            assert_eq!(
                std::fs::read_to_string(&env).unwrap(),
                "# mine\nFOO=bar\nCLAUDE_API_KEY=sk-local-FAKEB\n"
            );
            // Created when missing, with a comment header.
            std::fs::remove_file(&env).unwrap();
            store_claude_api("http://u/v1", "sk-local-FAKEC").unwrap();
            let text = std::fs::read_to_string(&env).unwrap();
            assert!(text.starts_with('#') && text.contains("CLAUDE_API_KEY=sk-local-FAKEC"));
            // ai.json keeps the url but not the key.
            let c = AiConfig::load().claude_api_server.unwrap();
            assert_eq!(c.key, "");
            assert_eq!(c.base_url, "http://u/v1");
            assert_eq!(key_location(), ".env");
        });
    }

    #[test]
    fn key_precedence_env_then_dotenv_then_ai_json() {
        with_home("env_prec", |home| {
            let cfg = AiConfig {
                claude_api_server: Some(ClaudeApiConfig { key: "sk-local-JSONKEY".into(), ..Default::default() }),
                use_claude: false,
                ..Default::default()
            };
            cfg.save().unwrap();
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-JSONKEY"));
            assert_eq!(key_location(), "ai.json");
            std::fs::write(home.join(".env"), "CLAUDE_API_KEY=sk-local-DOTENV\n").unwrap();
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-DOTENV"));
            assert_eq!(key_location(), ".env");
            unsafe { std::env::set_var("BUZZ_CLAUDE_API_KEY", "sk-local-ENVVAR"); }
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-ENVVAR"));
            assert_eq!(key_location(), "environment");
        });
    }

    #[test]
    fn migration_moves_ai_json_key_into_env() {
        with_home("env_migr", |home| {
            let cfg = AiConfig {
                claude_api_server: Some(ClaudeApiConfig {
                    base_url: "http://keep/v1".into(),
                    key: "sk-local-MIGRATE".into(),
                    claude_model: "opus".into(),
                }),
                use_claude: true,
                ..Default::default()
            };
            cfg.save().unwrap();
            ensure_claude_key();
            assert_eq!(dotenv_value(&home.join(".env"), "CLAUDE_API_KEY").as_deref(), Some("sk-local-MIGRATE"));
            let c = AiConfig::load();
            assert!(c.use_claude);
            let s = c.claude_api_server.unwrap();
            assert_eq!(s.key, "");
            assert_eq!(s.base_url, "http://keep/v1");
            assert_eq!(s.claude_model, "opus");
            // A second run changes nothing.
            ensure_claude_key();
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-MIGRATE"));
        });
    }

    #[test]
    fn without_a_buzz_home_the_key_stays_in_ai_json() {
        with_home("env_nohome", |home| {
            store_claude_api_in(None, "http://u/v1", "sk-local-NOHOME").unwrap();
            assert!(!home.join(".env").exists());
            assert_eq!(AiConfig::load().claude_api_server.unwrap().key, "sk-local-NOHOME");
            assert_eq!(claude_api_key().as_deref(), Some("sk-local-NOHOME"));
        });
    }

    #[test]
    fn reports_never_contain_the_raw_key() {
        with_home("env_report", |_home| {
            let studio = std::env::var_os("BUZZCAF_STUDIO_DIR").map(PathBuf::from).unwrap();
            fake_studio(&studio, "sk-local-FAKESECRET");
            let report = import_from_studio().unwrap();
            assert!(!format!("{report:?}").contains("FAKESECRET"));
            assert!(!mask_key("sk-local-FAKESECRET").contains("FAKESECRET"));
            assert!(!key_location().contains("FAKESECRET"));
        });
    }

    #[test]
    fn old_ai_json_still_loads() {
        let out = temp("old_json");
        let p = out.join("ai.json");
        std::fs::write(&p, r#"{"claude_api_server":{"base_url":"http://x","key":"k"}}"#).unwrap();
        let cfg = AiConfig::load_from(&p);
        let c = cfg.claude_api_server.unwrap();
        assert_eq!(c.claude_model, CLAUDE_DEFAULT_MODEL);
        assert!(!cfg.use_claude);
        let _ = std::fs::remove_dir_all(&out);
    }
}
