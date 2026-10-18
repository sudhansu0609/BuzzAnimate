//! **A small ComfyUI client** (PRODUCER_PLAN § P3): find the local ComfyUI,
//! start the portable install when asked, queue a workflow, wait for it and
//! fetch its pictures.
//!
//! Mirrors BuzzEdit's Python client (`backend/comfyui_bridge/client.py`):
//! probe `/system_stats`; scan 8188..8208 at 0.5 s per port; `POST /prompt`
//! `{prompt, client_id}` with a generous timeout (ComfyUI blocks its HTTP
//! server while loading a model); poll `/history/{id}`; read every output
//! entry; fetch through `GET /view`. A job that sees "connection refused" for
//! more than 20 s is given up: nothing is listening, so it died with the
//! process. Only localhost URLs are ever used, and no `Origin` header is sent.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The ports scanned when `COMFYUI_URL` does not answer.
const PORTS: std::ops::RangeInclusive<u16> = 8188..=8208;
/// How long one probe may take.
const PROBE: Duration = Duration::from_millis(500);
/// `POST /prompt`: ComfyUI may not answer while it loads a large model.
const QUEUE: Duration = Duration::from_secs(120);
/// Connection refused for longer than this while waiting = the job died.
const REFUSED_GIVE_UP: Duration = Duration::from_secs(20);
/// The portable install, unless `COMFYUI_DIR` says otherwise.
pub const DEFAULT_COMFYUI_DIR: &str = r"B:\ComfyUI_windows_portable_nvidia_cu118_or_cpu\ComfyUI_windows_portable";
const CLIENT_ID: &str = "buzzanimate_client";

/// Where one finished picture is, as ComfyUI's history lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub filename: String,
    pub subfolder: String,
    /// `output` / `temp` / `input`: ComfyUI's `type`.
    pub kind: String,
}

/// A ComfyUI on this machine.
#[derive(Debug, Clone)]
pub struct ComfyClient {
    pub base_url: String,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(timeout.min(Duration::from_secs(2)))
        .timeout(timeout)
        .build()
}

/// Only `http://127.0.0.1…` / `http://localhost…` are accepted.
fn is_local(url: &str) -> bool {
    let rest = url.strip_prefix("http://").unwrap_or("");
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost")
}

fn probe(url: &str, timeout: Duration) -> bool {
    agent(timeout)
        .get(&format!("{url}/system_stats"))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

impl ComfyClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into().trim_end_matches('/').to_string() }
    }

    /// The first ComfyUI that answers: `COMFYUI_URL` (when local), then
    /// `127.0.0.1:8188..8208`.
    pub fn discover() -> Option<Self> {
        let mut tried = Vec::new();
        if let Ok(url) = std::env::var("COMFYUI_URL") {
            let url = url.trim().trim_end_matches('/').to_string();
            if !url.is_empty() && is_local(&url) {
                if probe(&url, PROBE) {
                    return Some(Self::new(url));
                }
                tried.push(url);
            }
        }
        PORTS
            .map(|p| format!("http://127.0.0.1:{p}"))
            .filter(|u| !tried.contains(u))
            .find(|u| probe(u, PROBE))
            .map(Self::new)
    }

    /// Is this one up?
    pub fn alive(&self) -> bool {
        probe(&self.base_url, PROBE)
    }

    /// A running ComfyUI: found, or (with `launch`) the portable install
    /// started detached and polled for up to `wait`.
    pub fn ensure_running(launch: bool, wait: Duration) -> Result<Self, String> {
        if let Some(found) = Self::discover() {
            return Ok(found);
        }
        if !launch {
            return Err("ComfyUI is not running (nothing answered /system_stats on COMFYUI_URL or 127.0.0.1:8188-8208)".into());
        }
        let dir = std::env::var("COMFYUI_DIR")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_COMFYUI_DIR));
        let bat = dir.join("run_nvidia_gpu.bat");
        if !bat.is_file() {
            return Err(format!("ComfyUI cannot be started: {} is not there", bat.display()));
        }
        start_detached(&bat)?;
        let began = Instant::now();
        while began.elapsed() < wait {
            std::thread::sleep(Duration::from_secs(2));
            if let Some(found) = Self::discover() {
                return Ok(found);
            }
        }
        Err(format!("ComfyUI was started but did not answer within {} s", wait.as_secs()))
    }

    /// `POST /prompt`: the prompt id.
    pub fn queue(&self, workflow: Value) -> Result<String, String> {
        if !is_local(&self.base_url) {
            return Err(format!("{} is not a localhost URL", self.base_url));
        }
        let body = json!({ "prompt": workflow, "client_id": CLIENT_ID });
        match agent(QUEUE).post(&format!("{}/prompt", self.base_url)).send_json(body) {
            Ok(r) => {
                let v: Value = r.into_json().map_err(|e| format!("POST /prompt: not JSON ({e})"))?;
                v.get("prompt_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("POST /prompt: no prompt_id in {v}"))
            }
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                let text: String = text.chars().take(600).collect();
                Err(format!("ComfyUI rejected the workflow ({code}): {text}"))
            }
            Err(e) => Err(format!("ComfyUI queue prompt failed: {e}")),
        }
    }

    /// Poll `/history/{id}` every second until the job has outputs, failed,
    /// or `timeout` passed.
    pub fn wait(&self, prompt_id: &str, timeout: Duration) -> Result<Vec<Output>, String> {
        let began = Instant::now();
        let mut refused_since: Option<Instant> = None;
        let url = format!("{}/history/{prompt_id}", self.base_url);
        while began.elapsed() < timeout {
            match agent(Duration::from_secs(10)).get(&url).call() {
                Ok(r) => {
                    refused_since = None;
                    let v: Value = r.into_json().unwrap_or(Value::Null);
                    if let Some(entry) = v.get(prompt_id) {
                        let status = entry.get("status");
                        if status.and_then(|s| s.get("status_str")).and_then(Value::as_str) == Some("error") {
                            let messages = status.and_then(|s| s.get("messages")).cloned().unwrap_or(Value::Null);
                            let text: String = messages.to_string().chars().take(600).collect();
                            return Err(format!("ComfyUI job {prompt_id} failed: {text}"));
                        }
                        let outputs = read_outputs(entry);
                        let done = status.and_then(|s| s.get("completed")).and_then(Value::as_bool).unwrap_or(false);
                        if !outputs.is_empty() {
                            return Ok(outputs);
                        }
                        if done {
                            return Err(format!("ComfyUI job {prompt_id} finished with no readable outputs"));
                        }
                    }
                }
                Err(ureq::Error::Status(_, _)) => {
                    refused_since = None;
                }
                Err(_) => {
                    let since = *refused_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > REFUSED_GIVE_UP {
                        return Err(format!(
                            "ComfyUI stopped answering at {} for over {} s; the job died with it",
                            self.base_url,
                            REFUSED_GIVE_UP.as_secs()
                        ));
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        Err(format!("ComfyUI job {prompt_id} did not finish within {} s", timeout.as_secs()))
    }

    /// `GET /view`: the file's bytes.
    pub fn view(&self, output: &Output) -> Result<Vec<u8>, String> {
        let r = agent(Duration::from_secs(60))
            .get(&format!("{}/view", self.base_url))
            .query("filename", &output.filename)
            .query("subfolder", &output.subfolder)
            .query("type", &output.kind)
            .call()
            .map_err(|e| format!("GET /view {}: {e}", output.filename))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut bytes)
            .map_err(|e| format!("GET /view {}: {e}", output.filename))?;
        Ok(bytes)
    }
}

/// Every `images` / `gifs` / `videos` / `audio` entry of a history entry's
/// outputs, node by node.
fn read_outputs(entry: &Value) -> Vec<Output> {
    let mut found = Vec::new();
    let Some(outputs) = entry.get("outputs").and_then(Value::as_object) else {
        return found;
    };
    for node in outputs.values() {
        for key in ["images", "gifs", "videos", "audio"] {
            let Some(list) = node.get(key).and_then(Value::as_array) else {
                continue;
            };
            for item in list {
                let Some(filename) = item.get("filename").and_then(Value::as_str) else {
                    continue;
                };
                found.push(Output {
                    filename: filename.to_string(),
                    subfolder: item.get("subfolder").and_then(Value::as_str).unwrap_or("").to_string(),
                    kind: item.get("type").and_then(Value::as_str).unwrap_or("output").to_string(),
                });
            }
        }
    }
    found
}

/// Start the launcher in its own folder, detached from this process.
fn start_detached(bat: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let dir = bat.parent().unwrap_or(std::path::Path::new("."));
        let name = bat.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        std::process::Command::new("cmd")
            .args(["/c", "start", "/min", "", &name])
            .current_dir(dir)
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start ComfyUI ({}): {e}", bat.display()))
    }
    #[cfg(not(windows))]
    {
        Err(format!("ComfyUI cannot be started from here: {} is a Windows script", bat.display()))
    }
}
