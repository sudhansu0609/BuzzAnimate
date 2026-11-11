//! **A small ComfyUI client** (PRODUCER_PLAN § P3): find the local ComfyUI,
//! start the portable install when asked, queue a workflow, wait for it and
//! fetch its pictures.
//!
//! Mirrors BuzzEdit's Python client (`backend/comfyui_bridge/client.py`):
//! probe `/system_stats`; scan 8188..8208 and 8000 (ComfyUI Desktop) at 0.5 s per port; `POST /prompt`
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
/// Where Comfy Desktop listens unless that port is taken.
const DESKTOP_PORT: u16 = 8000;
/// How long one probe may take.
const PROBE: Duration = Duration::from_millis(500);
/// `POST /prompt`: ComfyUI may not answer while it loads a large model.
const QUEUE: Duration = Duration::from_secs(120);
/// Connection refused for longer than this while waiting = the job died.
const REFUSED_GIVE_UP: Duration = Duration::from_secs(20);
/// The portable install, unless `COMFYUI_DIR` says otherwise.
pub const DEFAULT_COMFYUI_DIR: &str = r"B:\ComfyUI_windows_portable_nvidia_cu118_or_cpu\ComfyUI_windows_portable";

/// The folder to start ComfyUI from when `COMFYUI_DIR` is not set: the portable
/// install on Windows, `~/ComfyUI` elsewhere.
fn default_comfyui_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(DEFAULT_COMFYUI_DIR)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("ComfyUI")).unwrap_or_else(|| PathBuf::from("ComfyUI"))
    }
}
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
    /// `127.0.0.1:8188..8208`, then `:8000` — Comfy Desktop's own default.
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
            .chain(std::iter::once(DESKTOP_PORT))
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
            return Err("ComfyUI is not running (nothing answered /system_stats on COMFYUI_URL, 127.0.0.1:8188-8208 or :8000)".into());
        }
        let started = launch_local()?;
        let began = Instant::now();
        while began.elapsed() < wait {
            std::thread::sleep(Duration::from_secs(2));
            if let Some(port) = started.port {
                let client = Self::new(format!("http://127.0.0.1:{port}"));
                if client.alive() {
                    return Ok(client);
                }
            } else if let Some(found) = Self::discover() {
                return Ok(found);
            }
            if let Some(died) = started.exited() {
                return Err(died);
            }
        }
        Err(format!("ComfyUI was started but did not answer within {} s", wait.as_secs()))
    }

    /// `POST /free`: ask ComfyUI to unload its models and free memory (best effort).
    pub fn free_memory(&self) -> Result<(), String> {
        if !is_local(&self.base_url) {
            return Err(format!("{} is not a localhost URL", self.base_url));
        }
        agent(Duration::from_secs(30))
            .post(&format!("{}/free", self.base_url))
            .send_json(json!({"unload_models": true, "free_memory": true}))
            .map(|_| ())
            .map_err(|e| format!("ComfyUI /free: {e}"))
    }

    /// `GET /object_info`: every node class with its input choices.
    pub fn object_info(&self) -> Result<Value, String> {
        agent(Duration::from_secs(60))
            .get(&format!("{}/object_info", self.base_url))
            .call()
            .map_err(|e| format!("ComfyUI /object_info: {e}"))?
            .into_json()
            .map_err(|e| format!("ComfyUI /object_info: {e}"))
    }

    /// [`Self::queue`] **without a client id**, so ComfyUI broadcasts the
    /// job's progress to every open page — its own web UI shows the run live
    /// (the running node outlined, the sampler's steps) as well as
    /// [`Self::watch`] here. `extra` goes in as `extra_data`, which is where
    /// a UI-form copy of the workflow rides so the saved PNG carries it.
    pub fn queue_shared(&self, workflow: Value, extra: Option<Value>) -> Result<String, String> {
        if !is_local(&self.base_url) {
            return Err(format!("{} is not a localhost URL", self.base_url));
        }
        let mut body = json!({ "prompt": workflow });
        if let Some(extra) = extra {
            body["extra_data"] = extra;
        }
        match agent(QUEUE).post(&format!("{}/prompt", self.base_url)).send_json(body) {
            Ok(r) => {
                let v: Value = r.into_json().map_err(|e| format!("POST /prompt: not JSON ({e})"))?;
                v.get("prompt_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("POST /prompt: no prompt_id in {v}"))
            }
            Err(ureq::Error::Status(code, r)) => {
                let text: String = r.into_string().unwrap_or_default().chars().take(600).collect();
                Err(format!("ComfyUI rejected the workflow ({code}): {text}"))
            }
            Err(e) => Err(format!("ComfyUI queue prompt failed: {e}")),
        }
    }

    /// **ComfyUI's live feed** (`/ws`): every JSON message it sends —
    /// `execution_start`, `executing`, `progress`, `execution_cached`,
    /// `execution_success`, `execution_error` — handed to `on_message` until
    /// `stop` is set or the connection closes. A plain WebSocket client over
    /// a `TcpStream`, enough for ComfyUI's unmasked server frames; binary
    /// frames (preview pictures) are skipped.
    pub fn watch(
        &self,
        stop: &std::sync::atomic::AtomicBool,
        mut on_message: impl FnMut(Value),
    ) -> Result<(), String> {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        if !is_local(&self.base_url) {
            return Err(format!("{} is not a localhost URL", self.base_url));
        }
        let host = self.base_url.trim_start_matches("http://");
        let mut stream = std::net::TcpStream::connect(host).map_err(|e| format!("ComfyUI /ws: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|e| format!("ComfyUI /ws: {e}"))?;
        // Unique per connection: ComfyUI keeps one socket per client id, so a
        // second watcher under the same id silently replaced the first.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let watcher = format!(
            "buzzanimate_watch_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let request = format!(
            "GET /ws?clientId={watcher} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: YnV6emFuaW1hdGUtd3RjaA==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(|e| format!("ComfyUI /ws: {e}"))?;

        // Bytes read but not yet consumed: the handshake's reply, then frames.
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 16384];
        let mut fill = |buf: &mut Vec<u8>, stream: &mut std::net::TcpStream| -> Result<bool, String> {
            match stream.read(&mut chunk) {
                Ok(0) => Ok(false),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    Ok(true)
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(true),
                Err(e) => Err(format!("ComfyUI /ws: {e}")),
            }
        };
        let header_end = loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            if !fill(&mut buf, &mut stream)? {
                return Err("ComfyUI /ws closed during the handshake".into());
            }
        };
        if !buf.starts_with(b"HTTP/1.1 101") {
            let head = String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).lines().next().unwrap_or("").to_string();
            return Err(format!("ComfyUI /ws refused: {head}"));
        }
        buf.drain(..header_end);

        // A message split over several frames is gathered here.
        let mut message: Vec<u8> = Vec::new();
        let mut message_is_text = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            // One whole frame in `buf`?
            let frame = (|| {
                if buf.len() < 2 {
                    return None;
                }
                let (fin, opcode, masked) = (buf[0] & 0x80 != 0, buf[0] & 0x0F, buf[1] & 0x80 != 0);
                let mut at = 2usize;
                let mut len = (buf[1] & 0x7F) as u64;
                if len == 126 {
                    if buf.len() < 4 {
                        return None;
                    }
                    len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
                    at = 4;
                } else if len == 127 {
                    if buf.len() < 10 {
                        return None;
                    }
                    len = u64::from_be_bytes(buf[2..10].try_into().ok()?);
                    at = 10;
                }
                let mask = if masked {
                    let m: [u8; 4] = buf.get(at..at + 4)?.try_into().ok()?;
                    at += 4;
                    Some(m)
                } else {
                    None
                };
                let end = at.checked_add(len as usize)?;
                if buf.len() < end {
                    return None;
                }
                let mut payload = buf[at..end].to_vec();
                if let Some(m) = mask {
                    for (i, b) in payload.iter_mut().enumerate() {
                        *b ^= m[i % 4];
                    }
                }
                Some((fin, opcode, payload, end))
            })();
            let Some((fin, opcode, payload, used)) = frame else {
                if !fill(&mut buf, &mut stream)? {
                    return Ok(());
                }
                continue;
            };
            buf.drain(..used);
            match opcode {
                0x1 | 0x2 => {
                    message_is_text = opcode == 0x1;
                    message = payload;
                }
                0x0 => message.extend_from_slice(&payload),
                0x8 => return Ok(()),
                0x9 => {
                    // Ping: answer with a masked pong carrying the same bytes.
                    let mut pong = vec![0x8A, 0x80 | payload.len().min(125) as u8, 0, 0, 0, 0];
                    pong.extend_from_slice(&payload[..payload.len().min(125)]);
                    let _ = stream.write_all(&pong);
                    continue;
                }
                _ => continue,
            }
            if fin && message_is_text {
                if let Ok(v) = serde_json::from_slice::<Value>(&message) {
                    on_message(v);
                }
                message.clear();
            }
        }
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

    /// `POST /upload/image` (multipart, overwrite): the name ComfyUI keeps the
    /// picture under, for a `LoadImage` node.
    pub fn upload_image(&self, name: &str, png: &[u8]) -> Result<String, String> {
        if !is_local(&self.base_url) {
            return Err(format!("{} is not a localhost URL", self.base_url));
        }
        let boundary = "----buzzanimateboundary7d3f";
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(png);
        body.extend_from_slice(
            format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"overwrite\"\r\n\r\ntrue\r\n--{boundary}--\r\n").as_bytes(),
        );
        let r = agent(Duration::from_secs(120))
            .post(&format!("{}/upload/image", self.base_url))
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(|e| format!("ComfyUI upload failed: {e}"))?;
        let v: Value = r.into_json().map_err(|e| format!("ComfyUI upload: not JSON ({e})"))?;
        Ok(v.get("name").and_then(Value::as_str).unwrap_or(name).to_string())
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

/// A ComfyUI this process started: the port it was told to listen on (none
/// for a Windows launcher script, which picks its own) and its log.
struct Started {
    port: Option<u16>,
    log: Option<PathBuf>,
}

/// The one ComfyUI this process has started, so a second request while it is
/// still loading waits for it instead of starting another.
static CHILD: std::sync::Mutex<Option<(std::process::Child, Started)>> = std::sync::Mutex::new(None);

impl Started {
    /// `Some(why)` once the process we started has quit.
    fn exited(&self) -> Option<String> {
        let mut guard = CHILD.lock().unwrap_or_else(|e| e.into_inner());
        let (child, _) = guard.as_mut()?;
        let status = child.try_wait().ok()??;
        *guard = None;
        let tail = self
            .log
            .as_ref()
            .and_then(|l| std::fs::read_to_string(l).ok())
            .map(|text| {
                let lines: Vec<&str> = text.lines().collect();
                lines[lines.len().saturating_sub(6)..].join("\n")
            })
            .unwrap_or_default();
        let log = self.log.as_ref().map(|l| format!(" — log: {}", l.display())).unwrap_or_default();
        Some(format!("ComfyUI quit while starting ({status}){log}\n{tail}"))
    }
}

/// Start the local ComfyUI, detached: a running start of ours is reused,
/// otherwise `COMFYUI_DIR` (a folder holding `main.py` or the Windows
/// launcher), else Comfy Desktop's installation, else the portable default.
fn launch_local() -> Result<Started, String> {
    {
        let mut guard = CHILD.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((child, started)) = guard.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                return Ok(Started { port: started.port, log: started.log.clone() });
            }
            *guard = None;
        }
    }
    let override_dir = std::env::var("COMFYUI_DIR").ok().filter(|d| !d.trim().is_empty()).map(PathBuf::from);
    if let Some(dir) = &override_dir {
        let bat = dir.join("run_nvidia_gpu.bat");
        if bat.is_file() {
            start_detached(&bat)?;
            return Ok(Started { port: None, log: None });
        }
        if let Some(install) = Install::from_dir(dir) {
            return install.spawn();
        }
        return Err(format!("ComfyUI cannot be started: {} has neither main.py nor run_nvidia_gpu.bat", dir.display()));
    }
    if let Some(install) = Install::from_desktop() {
        return install.spawn();
    }
    let bat = PathBuf::from(DEFAULT_COMFYUI_DIR).join("run_nvidia_gpu.bat");
    if bat.is_file() {
        start_detached(&bat)?;
        return Ok(Started { port: None, log: None });
    }
    Err("ComfyUI cannot be started: no Comfy Desktop installation was found and COMFYUI_DIR is not set".into())
}

/// The custom-node folder BuzzAnimate puts in ComfyUI so its web page can
/// open a workflow sent from here: ComfyUI serves every custom node's
/// `example_workflows/` at `/api/workflow_templates/<node>/<name>.json`, and
/// its page opens one from `?template=<name>&source=<node>`. Holds no nodes.
pub const BRIDGE: &str = "buzzanimate_bridge";

/// The ComfyUI checkout (the folder with `main.py`) that `workflow` lives
/// in — a workflow saved from ComfyUI sits under its `user/` folder — else
/// `COMFYUI_DIR`, else Comfy Desktop's install.
pub fn checkout_for(workflow: &std::path::Path) -> Option<PathBuf> {
    workflow
        .ancestors()
        .find(|d| d.join("main.py").is_file() && d.join("custom_nodes").is_dir())
        .map(std::path::Path::to_path_buf)
        .or_else(|| {
            std::env::var("COMFYUI_DIR")
                .ok()
                .filter(|d| !d.trim().is_empty())
                .and_then(|d| Install::from_dir(std::path::Path::new(&d)))
                .map(|i| i.dir)
        })
        .or_else(|| Install::from_desktop().map(|i| i.dir))
}

/// Put `workflow` (UI form) where ComfyUI's page can open it, installing
/// the bridge folder first if it is not there. The page address to open,
/// and whether ComfyUI already serves the bridge — it reads custom nodes
/// only as it starts, so a bridge installed while it runs needs one restart.
pub fn publish_workflow(client: &ComfyClient, checkout: &std::path::Path, name: &str, workflow: &Value) -> Result<(String, bool), String> {
    let bridge = checkout.join("custom_nodes").join(BRIDGE);
    let examples = bridge.join("example_workflows");
    std::fs::create_dir_all(&examples).map_err(|e| format!("{}: {e}", examples.display()))?;
    let init = bridge.join("__init__.py");
    if !init.is_file() {
        std::fs::write(
            &init,
            "# Installed by BuzzAnimate. It adds no nodes: it only lets ComfyUI's page\n\
             # open the workflows BuzzAnimate sends (example_workflows/). Safe to delete.\n\
             NODE_CLASS_MAPPINGS = {}\nNODE_DISPLAY_NAME_MAPPINGS = {}\n",
        )
        .map_err(|e| format!("{}: {e}", init.display()))?;
    }
    let file = examples.join(format!("{name}.json"));
    let text = serde_json::to_string_pretty(workflow).map_err(|e| e.to_string())?;
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;

    let served = agent(Duration::from_secs(5))
        .get(&format!("{}/api/workflow_templates", client.base_url))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .is_some_and(|v| v.get(BRIDGE).is_some());
    let page = if served {
        format!("{}/?template={name}&source={BRIDGE}", client.base_url)
    } else {
        format!("{}/", client.base_url)
    };
    Ok((page, served))
}

/// Open a page in the default browser.
pub fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// A ComfyUI checkout with its own Python, and the arguments it runs with.
#[derive(Debug, Clone, PartialEq)]
struct Install {
    /// The folder holding `main.py`.
    dir: PathBuf,
    python: PathBuf,
    args: Vec<String>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The Python beside a ComfyUI checkout: its `.venv`, or a standalone
/// environment one folder up (Comfy Desktop's layout).
fn python_for(dir: &std::path::Path) -> Option<PathBuf> {
    let bin = if cfg!(windows) { "Scripts/python.exe" } else { "bin/python" };
    [dir.join(".venv").join(bin), dir.join("venv").join(bin)]
        .into_iter()
        .chain(dir.parent().map(|p| p.join("standalone-env").join(bin)))
        .find(|p| p.is_file())
}

impl Install {
    fn from_dir(dir: &std::path::Path) -> Option<Self> {
        let dir = if dir.join("main.py").is_file() { dir.to_path_buf() } else { dir.join("ComfyUI") };
        if !dir.join("main.py").is_file() {
            return None;
        }
        let python = python_for(&dir)?;
        Some(Self { dir, python, args: Vec::new() })
    }

    /// The first installed instance Comfy Desktop knows, launched the way
    /// Desktop launches it: its launch arguments, its shared model folders
    /// and its input / output folders.
    fn from_desktop() -> Option<Self> {
        let support = home()?.join("Library/Application Support/Comfy Desktop");
        let installs: Value = serde_json::from_str(&std::fs::read_to_string(support.join("installations.json")).ok()?).ok()?;
        let settings: Value = std::fs::read_to_string(support.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        Self::from_desktop_json(&installs, &settings, &support)
    }

    fn from_desktop_json(installs: &Value, settings: &Value, support: &std::path::Path) -> Option<Self> {
        installs.as_array()?.iter().find_map(|inst| {
            if inst.get("status").and_then(Value::as_str).is_some_and(|s| s != "installed") {
                return None;
            }
            let path = PathBuf::from(inst.get("installPath")?.as_str()?);
            let mut found = Self::from_dir(&path)?;
            let args = &mut found.args;
            if let Some(extra) = inst.get("launchArgs").and_then(Value::as_str) {
                args.extend(extra.split_whitespace().map(str::to_string));
            }
            if let Some(id) = inst.get("id").and_then(Value::as_str) {
                let paths = support.join("instance-model-paths").join(format!("{id}.yaml"));
                if paths.is_file() {
                    args.extend(["--extra-model-paths-config".into(), paths.display().to_string()]);
                }
            }
            for (key, flag) in [("inputDir", "--input-directory"), ("outputDir", "--output-directory")] {
                if let Some(d) = settings.get(key).and_then(Value::as_str).filter(|d| std::path::Path::new(d).is_dir()) {
                    args.extend([flag.to_string(), d.to_string()]);
                }
            }
            Some(found)
        })
    }

    fn spawn(self) -> Result<Started, String> {
        let port = PORTS
            .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
            .ok_or("ComfyUI cannot be started: ports 8188-8208 are all taken")?;
        let log = home()
            .map(|h| h.join(".config/BuzzAnimate"))
            .unwrap_or_else(std::env::temp_dir)
            .join("comfyui.log");
        if let Some(parent) = log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| format!("{}: {e}", log.display()))?;
        let mut command = std::process::Command::new(&self.python);
        command
            .arg("main.py")
            .args(["--listen", "127.0.0.1", "--port", &port.to_string()])
            .args(&self.args)
            .current_dir(&self.dir)
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err);
        // Its own process group, so it keeps running when BuzzAnimate quits —
        // the next session finds it already warm.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command
            .spawn()
            .map_err(|e| format!("could not start ComfyUI ({}): {e}", self.python.display()))?;
        let started = Started { port: Some(port), log: Some(log.clone()) };
        *CHILD.lock().unwrap_or_else(|e| e.into_inner()) = Some((child, Started { port: Some(port), log: Some(log) }));
        Ok(started)
    }
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
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        // `bat` is `<dir>/main.py` here: run it with the folder's own Python
        // environment when it has one.
        let dir = bat.parent().unwrap_or(std::path::Path::new("."));
        let python = ["venv/bin/python", ".venv/bin/python"]
            .iter()
            .map(|p| dir.join(p))
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("python3"));
        std::process::Command::new(python)
            .arg("main.py")
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start ComfyUI ({}): {e}", bat.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comfy Desktop's own files are enough to start its instance the way it
    /// does: the checkout's Python, its launch arguments, its model paths.
    #[test]
    fn a_desktop_install_is_launched_with_its_own_settings() {
        let root = std::env::temp_dir().join(format!("buzz-comfy-desktop-{}", std::process::id()));
        let install = root.join("ComfyUI-Installs/ComfyUI");
        let checkout = install.join("ComfyUI");
        std::fs::create_dir_all(checkout.join(".venv/bin")).unwrap();
        std::fs::write(checkout.join("main.py"), "").unwrap();
        std::fs::write(checkout.join(".venv/bin/python"), "").unwrap();
        let support = root.join("support");
        std::fs::create_dir_all(support.join("instance-model-paths")).unwrap();
        std::fs::write(support.join("instance-model-paths/inst-1.yaml"), "").unwrap();
        let output = root.join("out");
        std::fs::create_dir_all(&output).unwrap();

        let installs = json!([
            {"id": "inst-0", "status": "downloading", "installPath": "/nowhere"},
            {"id": "inst-1", "status": "installed", "installPath": install, "launchArgs": "--enable-manager --cache-none"}
        ]);
        let settings = json!({"outputDir": output, "inputDir": "/no/such/folder"});
        let found = Install::from_desktop_json(&installs, &settings, &support).expect("the installed instance");
        assert_eq!(found.dir, checkout);
        assert_eq!(found.python, checkout.join(".venv/bin/python"));
        let model_paths = support.join("instance-model-paths/inst-1.yaml").display().to_string();
        assert_eq!(
            found.args,
            vec![
                "--enable-manager".to_string(),
                "--cache-none".into(),
                "--extra-model-paths-config".into(),
                model_paths,
                "--output-directory".into(),
                output.display().to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
