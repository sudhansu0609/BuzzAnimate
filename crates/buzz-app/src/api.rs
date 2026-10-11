//! **BuzzAnimate's local API**: the door Buzzcaf Studio and BuzzEdit come in by
//! (step E1 of `docs/ECOSYSTEM_BRIDGE_PLAN.md`).
//!
//! A small synchronous HTTP server (`tiny_http`) on its own thread, bound to
//! `127.0.0.1` only. Port 8840 is a wish, not a fact (ECOSYSTEM.md §3): the
//! first free port from there (`BUZZANIMATE_PORT` overrides the wish) is taken,
//! and once `/health` answers the port is published in the shared ledger as
//! `buzzanimate` ([`crate::ports`]), then withdrawn on a clean exit.
//!
//! **No document is touched off the UI thread.** The server thread answers
//! `/health` itself and sends everything else over a channel to
//! [`App::api_pump`], which the frame loop calls once a frame exactly as it
//! calls `agent_pump`; the server thread asks egui for a repaint so an idle
//! window still answers. A request the window does not answer within 60 s gets
//! a clear error rather than a hang.
//!
//! The routes, JSON in and JSON out:
//!
//! * `GET /health` — `{app:"buzzanimate", status:"ok", port, pid, version}`.
//! * `GET /api/state` — the document: path, title, dirty, frame, frames, fps,
//!   scene names, selection count.
//! * `GET /api/catalog` — [`crate::agent::catalog`].
//! * `POST /api/call {tool, args}` — [`crate::agent::execute`], answered
//!   `{ok, value, error}`, one undo step per call labelled `API: <tool>`. A call
//!   the Assistant would stop to confirm (quit, close, overwriting a file) is
//!   refused here: nobody is at the window to say yes.
//! * `POST /api/open {path}`, `POST /api/save {path?}`.
//! * `POST /api/export {path, format, from?, to?}` — a job on the app's own
//!   export queue; `GET /api/jobs`, `GET /api/jobs/{id}` say how it is going.
//! * `POST /api/episode {...}` — a screenplay built into an episode off the UI
//!   thread (the `examples/episode_build.rs` flow), optionally rendered.
//!
//! **Security.** Localhost only, and a request carrying a web page's `Origin`
//! is refused: a page open in a browser must not be able to drive the app.
//! Bodies are capped at 16 MB. **Never fatal**: a port or ledger failure is a
//! note on the status line and the app carries on without the API.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::{Map, Value, json};

use crate::agent::{self, AgentCall};
use crate::app::{App, Pick};
use crate::ports;
use crate::tasks::TaskId;

/// The port asked for when `BUZZANIMATE_PORT` does not say.
pub const PREFERRED_PORT: u16 = 8840;
/// How long a request waits for the UI thread before giving up.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest request body accepted.
pub const MAX_BODY: usize = 16 * 1024 * 1024;

// ------------------------------------------------------------------ config

/// Where the API listens and publishes.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub preferred: u16,
    pub span: u16,
    /// The ledger, resolved once at start so the withdrawal goes to the same
    /// file the entry went into.
    pub ledger: PathBuf,
}

impl ApiConfig {
    /// `BUZZANIMATE_PORT` (else 8840), the family's span of 20, and the ledger
    /// `BUZZCAF_PORTS_FILE` names (else `%LOCALAPPDATA%\Buzzcaf\ports.json`).
    pub fn from_env() -> Self {
        let preferred = std::env::var("BUZZANIMATE_PORT")
            .ok()
            .and_then(|v| v.trim().parse::<u16>().ok())
            .filter(|p| *p > 0)
            .unwrap_or(PREFERRED_PORT);
        Self {
            preferred,
            span: ports::SPAN,
            ledger: ports::ledger_path(),
        }
    }
}

/// Is this `Origin` header one the API will serve?
///
/// Absent (a script, curl, another app's backend) is fine, and so is a page
/// served from this machine (`http://127.0.0.1:*`, `http://localhost:*`,
/// which is where the Studio's and BuzzEdit's own front-ends live). Anything
/// else is a web page somewhere else, and is refused.
///
/// **`null` is refused.** It is what a sandboxed iframe or a `file://` page
/// sends, and any website can put a sandboxed iframe on its page — allowing
/// `null` would let a web page open, save and export files through this API.
/// No backend client sends it.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim) else {
        return true;
    };
    if origin.is_empty() {
        return true;
    }
    let origin = origin.to_ascii_lowercase();
    ["http://127.0.0.1", "http://localhost"].iter().any(|host| {
        origin
            .strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || (rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()) && rest.len() > 1))
    })
}

/// Is this `Host` header this machine, on the port we listen on?
///
/// The defence against **DNS rebinding**: a web page on `evil.example` can
/// re-point its own name at 127.0.0.1, and its requests then reach this
/// listener *same-origin*, so the `Origin` check alone would let it read the
/// answers. Its `Host` header still says `evil.example`, so only
/// `127.0.0.1`, `localhost` and `[::1]` (with or without our port) are served.
/// A request with no `Host` (HTTP/1.0, a raw socket) has no name to rebind.
pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host.map(str::trim) else {
        return true;
    };
    let host = host.to_ascii_lowercase();
    let port = port.to_string();
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|name| {
        host.strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.strip_prefix(':') == Some(port.as_str()))
    })
}

/// The `/health` body ECOSYSTEM.md §3 rule 4 requires.
pub fn health_body(port: u16) -> Value {
    json!({
        "app": ports::APP,
        "status": "ok",
        "port": port,
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

// ------------------------------------------------------------------ messages

/// What a request asks the UI thread to do.
#[derive(Debug, Clone)]
enum Route {
    State,
    Catalog,
    Call(Value),
    Open(Value),
    Save(Value),
    Export(Value),
    Jobs,
    Job(u64),
    Episode(Value),
}

/// An HTTP status and a JSON body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    fn err(status: u16, error: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({"ok": false, "error": error.into()}),
        }
    }
}

/// Everything the server's threads tell the UI thread.
enum ApiMsg {
    Request { route: Route, reply: Sender<Reply> },
    /// A line for the status bar: where the API landed, or why it did not.
    Note(String),
}

// ------------------------------------------------------------------ the server

/// The running server. Dropping it stops the listener and withdraws the
/// ledger entry, so a clean exit cleans up even without an explicit call.
struct ServerHandle {
    port: u16,
    ledger: PathBuf,
    stop: Arc<AtomicBool>,
    published: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    publisher: Option<JoinHandle<()>>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // The publisher gives up within one probe once `stop` is set; joined so
        // it cannot publish after the withdrawal below.
        if let Some(t) = self.publisher.take() {
            let _ = t.join();
        }
        if self.published.load(Ordering::Relaxed) {
            // Only our own entry: a second BuzzAnimate that published since owns
            // the name now, and its entry is not ours to remove.
            let ours = ports::entry_at(&self.ledger, ports::APP)
                .and_then(|e| e.get("pid").and_then(Value::as_u64))
                == Some(std::process::id() as u64);
            if ours {
                ports::withdraw_at(&self.ledger, ports::APP);
            }
        }
    }
}

/// Bind, start answering, and publish once `/health` answers.
fn start_server(
    config: &ApiConfig,
    tx: Sender<ApiMsg>,
    ctx: Option<egui::Context>,
) -> Result<ServerHandle, String> {
    let host = "127.0.0.1";
    let wanted = ports::pick_port(config.preferred, config.span, host);
    let server = match tiny_http::Server::http(SocketAddr::from((Ipv4Addr::LOCALHOST, wanted))) {
        Ok(s) => s,
        // Somebody took it in the gap since `pick_port` looked: let the OS
        // choose rather than give up.
        Err(_) => tiny_http::Server::http(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .map_err(|e| format!("could not listen on {host}: {e}"))?,
    };
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or("the listener has no IP address")?;

    let stop = Arc::new(AtomicBool::new(false));
    let published = Arc::new(AtomicBool::new(false));

    let thread = {
        let stop = stop.clone();
        let tx = tx.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("buzz-api".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let request = match server.recv_timeout(Duration::from_millis(100)) {
                        Ok(Some(r)) => r,
                        Ok(None) => continue,
                        Err(_) => break,
                    };
                    // A thread per request: a call waiting on a busy window
                    // must not hold up `/health`.
                    let tx = tx.clone();
                    let ctx = ctx.clone();
                    let _ = std::thread::Builder::new()
                        .name("buzz-api-request".into())
                        .spawn(move || serve(request, &tx, ctx.as_ref(), port));
                }
            })
            .map_err(|e| format!("could not start the API thread: {e}"))?
    };

    let publisher = {
        let stop = stop.clone();
        let published = published.clone();
        let ledger = config.ledger.clone();
        std::thread::Builder::new()
            .name("buzz-api-publish".into())
            .spawn(move || {
                // Publish once the listener answers: an entry written before
                // that is a promise nobody can keep.
                let mut answered = false;
                for _ in 0..50 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if probe_health(port) {
                        answered = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if !answered {
                    let _ = tx.send(ApiMsg::Note(format!(
                        "Local API on port {port} did not answer its own health check, so it was not published"
                    )));
                    return;
                }
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                ports::publish_at(&ledger, ports::APP, port, &ports::health_url("127.0.0.1", port), Map::new());
                if ports::published_port(&ledger, ports::APP) == Some(port) {
                    published.store(true, Ordering::Relaxed);
                    tracing::info!("local API on http://127.0.0.1:{port}, published in {}", ledger.display());
                } else {
                    let _ = tx.send(ApiMsg::Note(format!(
                        "Local API on port {port}, but the port ledger {} could not be written",
                        ledger.display()
                    )));
                }
            })
            .map_err(|e| format!("could not start the API publisher: {e}"))?
    };

    Ok(ServerHandle {
        port,
        ledger: config.ledger.clone(),
        stop,
        published,
        thread: Some(thread),
        publisher: Some(publisher),
    })
}

/// Does our own `/health` answer with our own name?
fn probe_health(port: u16) -> bool {
    match http_request(port, "GET", "/health", None, None, Duration::from_millis(500)) {
        Ok((200, body)) => body.get("app").and_then(Value::as_str) == Some(ports::APP),
        _ => false,
    }
}

/// A minimal HTTP/1.1 request to this machine: the publisher's self-check, and
/// the tests' client. `Connection: close`, so the body runs to the end.
pub fn http_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&Value>,
    origin: Option<&str>,
    timeout: Duration,
) -> std::io::Result<(u16, Value)> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(origin) = origin {
        head.push_str(&format!("Origin: {origin}\r\n"));
    }
    if body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", payload.len()));
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("");
    Ok((status, serde_json::from_str(body).unwrap_or(Value::Null)))
}

/// Answer one request, on its own thread.
fn serve(mut request: tiny_http::Request, tx: &Sender<ApiMsg>, ctx: Option<&egui::Context>, port: u16) {
    let reply = answer(&mut request, tx, ctx, port);
    let data = serde_json::to_vec(&reply.body).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = tiny_http::Response::from_data(data).with_status_code(reply.status);
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]) {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn answer(request: &mut tiny_http::Request, tx: &Sender<ApiMsg>, ctx: Option<&egui::Context>, port: u16) -> Reply {
    let origin = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Origin"))
        .map(|h| h.value.as_str().to_string());
    if !origin_allowed(origin.as_deref()) {
        return Reply::err(
            403,
            format!(
                "refused: requests from {} are not allowed; only this machine (no Origin, http://127.0.0.1:*, http://localhost:*) may drive BuzzAnimate",
                origin.unwrap_or_default()
            ),
        );
    }
    let host = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.as_str().to_string());
    if !host_allowed(host.as_deref(), port) {
        return Reply::err(
            403,
            format!(
                "refused: Host {} is not this machine; address BuzzAnimate as 127.0.0.1:{port} or localhost:{port}",
                host.unwrap_or_default()
            ),
        );
    }

    let path = request.url().split('?').next().unwrap_or("").trim_end_matches('/').to_string();
    let get = *request.method() == tiny_http::Method::Get;
    let post = *request.method() == tiny_http::Method::Post;

    let route = match path.as_str() {
        "/health" if get => return Reply::ok(health_body(port)),
        "/api/state" if get => Route::State,
        "/api/catalog" if get => Route::Catalog,
        "/api/jobs" if get => Route::Jobs,
        p if get && p.starts_with("/api/jobs/") => match p["/api/jobs/".len()..].parse::<u64>() {
            Ok(id) => Route::Job(id),
            Err(_) => return Reply::err(400, "a job id is a whole number"),
        },
        "/api/call" | "/api/open" | "/api/save" | "/api/export" | "/api/episode" if post => {
            let body = match read_body(request) {
                Ok(b) => b,
                Err(r) => return r,
            };
            match path.as_str() {
                "/api/call" => Route::Call(body),
                "/api/open" => Route::Open(body),
                "/api/save" => Route::Save(body),
                "/api/export" => Route::Export(body),
                _ => Route::Episode(body),
            }
        }
        "/health" | "/api/state" | "/api/catalog" | "/api/jobs" | "/api/call" | "/api/open"
        | "/api/save" | "/api/export" | "/api/episode" => {
            return Reply::err(405, format!("{} is not allowed on {path}", request.method()));
        }
        p if p.starts_with("/api/jobs/") => {
            return Reply::err(405, format!("{} is not allowed on {path}", request.method()));
        }
        _ => return Reply::err(404, format!("no route {path}")),
    };

    let (reply, answer) = bounded(1);
    if tx.send(ApiMsg::Request { route, reply }).is_err() {
        return Reply::err(503, "BuzzAnimate is closing");
    }
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
    match answer.recv_timeout(REQUEST_TIMEOUT) {
        Ok(r) => r,
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => Reply::err(
            504,
            "BuzzAnimate did not answer within 60 s: the window may be busy (a modal dialog, a long import); try again",
        ),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Reply::err(503, "BuzzAnimate is closing"),
    }
}

/// The request body as a JSON object (an empty body is `{}`), at most 16 MB.
fn read_body(request: &mut tiny_http::Request) -> Result<Value, Reply> {
    if request.body_length().is_some_and(|n| n > MAX_BODY) {
        return Err(Reply::err(413, "the body is larger than 16 MB"));
    }
    let mut raw = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut raw) {
        return Err(Reply::err(400, format!("could not read the body: {e}")));
    }
    if raw.len() > MAX_BODY {
        return Err(Reply::err(413, "the body is larger than 16 MB"));
    }
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(&raw) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err(Reply::err(400, "the body must be a JSON object")),
        Err(e) => Err(Reply::err(400, format!("the body is not JSON: {e}"))),
    }
}

// ------------------------------------------------------------------ jobs

#[derive(Debug, Clone, PartialEq)]
enum JobState {
    /// Waiting its turn in the export queue.
    Queued,
    Running,
    Done,
    Failed(String),
}

/// What an episode build hands back from its thread.
pub(crate) struct EpisodeDone {
    pub(crate) files: Vec<PathBuf>,
    /// Each built scene, for rendering: (file stem, scene, length in frames).
    pub(crate) scenes: Vec<(String, buzz_scene::Scene, u32)>,
    pub(crate) warnings: Vec<String>,
}

/// One job an API caller started.
struct ApiJob {
    id: u64,
    kind: &'static str,
    /// The file (or folder) it writes.
    output: PathBuf,
    state: JobState,
    /// The task running it, once the export queue has started it.
    task: Option<TaskId>,
    /// An episode build's answer, while its thread runs.
    episode: Option<Receiver<Result<EpisodeDone, String>>>,
    /// Queue a preview per scene when the build is done.
    render: bool,
    result: Value,
}

/// The app's side of the local API.
#[derive(Default)]
pub struct ApiState {
    /// Start a server at the next frame (the window sets this; `App::new`
    /// alone, as every test builds it, never listens).
    wanted: bool,
    tried: bool,
    server: Option<ServerHandle>,
    rx: Option<Receiver<ApiMsg>>,
    ctx: Option<egui::Context>,
    jobs: Vec<ApiJob>,
    next_job: u64,
}

impl ApiState {
    /// The port the API is listening on, if it is.
    pub fn port(&self) -> Option<u16> {
        self.server.as_ref().map(|s| s.port)
    }

    /// Is a request waiting, or a job of ours still queued or running?
    ///
    /// The frame loop asks this while the window is **minimised**: Windows
    /// sends a minimised window no paint messages, so `request_redraw` never
    /// turns into a frame, and without one a caller would wait out its
    /// timeout and an export or episode build would sit still.
    pub fn has_work(&self) -> bool {
        self.rx.as_ref().is_some_and(|rx| !rx.is_empty())
            || self
                .jobs
                .iter()
                .any(|j| matches!(j.state, JobState::Queued | JobState::Running))
    }

    /// Has the ledger entry been written?
    pub fn published(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.published.load(Ordering::Relaxed))
    }

    fn new_job(&mut self, kind: &'static str, output: PathBuf, state: JobState) -> u64 {
        self.next_job += 1;
        let id = self.next_job;
        self.jobs.push(ApiJob {
            id,
            kind,
            output,
            state,
            task: None,
            episode: None,
            render: false,
            result: Value::Null,
        });
        id
    }

    /// The export queue has started a task writing `reveal`: if an API export
    /// is waiting for exactly that file, this is it. First come, first served,
    /// as the queue is.
    pub(crate) fn export_started(&mut self, task: TaskId, reveal: &Path) {
        if let Some(job) = self
            .jobs
            .iter_mut()
            .find(|j| j.kind == "export" && j.state == JobState::Queued && j.output == reveal)
        {
            job.state = JobState::Running;
            job.task = Some(task);
        }
    }

    /// An export task ended (finished, failed or cancelled).
    pub(crate) fn export_finished(&mut self, task: TaskId, ok: bool, message: &str) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.task == Some(task) && j.state == JobState::Running) {
            job.state = if ok { JobState::Done } else { JobState::Failed(message.to_string()) };
            job.result = json!({"message": message});
        }
    }
}

// ------------------------------------------------------------------ the UI side

impl App {
    /// Listen for the local API once the window is up. Only the real window
    /// asks for this, so a headless `App` never opens a port by surprise.
    pub fn with_api(mut self) -> Self {
        self.api.wanted = true;
        self
    }

    /// The local API's state, for the status of the server.
    pub fn api_state(&self) -> &ApiState {
        &self.api
    }

    /// Stop listening and withdraw the ledger entry. The exit path calls it;
    /// dropping the app does the same.
    pub fn api_shutdown(&mut self) {
        self.api.server = None;
        self.api.rx = None;
    }

    fn api_start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = unbounded();
        let config = ApiConfig::from_env();
        match start_server(&config, tx, Some(ctx.clone())) {
            Ok(handle) => {
                self.api.server = Some(handle);
                self.api.rx = Some(rx);
                self.api.ctx = Some(ctx.clone());
            }
            Err(e) => {
                tracing::warn!("local API not started: {e}");
                self.agent_editor_mut().status = Some(format!("Local API not started: {e}"));
            }
        }
    }

    /// Once per frame: start the server if it is wanted and not yet tried,
    /// answer whatever requests have arrived, and settle finished episode
    /// builds. Returns true if it did anything (the caller repaints at once).
    pub(crate) fn api_pump(&mut self, ctx: &egui::Context) -> bool {
        if self.api.wanted && !self.api.tried {
            self.api.tried = true;
            self.api_start(ctx);
        }
        let mut did = self.api_settle_episodes();
        loop {
            let Some(rx) = self.api.rx.as_ref() else { break };
            let Ok(msg) = rx.try_recv() else { break };
            did = true;
            match msg {
                ApiMsg::Note(text) => self.agent_editor_mut().status = Some(text),
                ApiMsg::Request { route, reply } => {
                    let r = self.api_route(route);
                    let _ = reply.send(r);
                }
            }
        }
        did
    }

    fn api_route(&mut self, route: Route) -> Reply {
        match route {
            Route::State => Reply::ok(self.api_document_state()),
            Route::Catalog => Reply::ok(agent::catalog()),
            Route::Call(body) => self.api_call(&body),
            Route::Open(body) => self.api_open(&body),
            Route::Save(body) => self.api_save(&body),
            Route::Export(body) => self.api_export(&body),
            Route::Jobs => {
                self.api_refresh_jobs();
                let list: Vec<Value> = self.api.jobs.iter().map(|j| self.api_job_json(j)).collect();
                Reply::ok(json!({"jobs": list}))
            }
            Route::Job(id) => {
                self.api_refresh_jobs();
                match self.api.jobs.iter().find(|j| j.id == id) {
                    Some(j) => Reply::ok(self.api_job_json(j)),
                    None => Reply::err(404, format!("no job {id}")),
                }
            }
            Route::Episode(body) => self.api_episode(&body),
        }
    }

    fn api_document_state(&self) -> Value {
        let editor = self.agent_editor();
        let scene = editor.doc.scene();
        json!({
            "path": editor.doc.path().map(|p| p.display().to_string()),
            "title": editor.doc.title(),
            "dirty": editor.doc.is_dirty(),
            "current_frame": editor.current_frame,
            "frame_count": scene.frame_count(),
            "film_frames": editor.doc.film_frames(),
            "fps": scene.stage().frame_rate,
            "scenes": editor.doc.scene_names(),
            "active_scene": editor.doc.active_scene(),
            "selection": editor.selection.len(),
        })
    }

    /// `POST /api/call`: one tool call, one undo step.
    fn api_call(&mut self, body: &Value) -> Reply {
        let Some(tool) = body.get("tool").and_then(Value::as_str).map(str::to_string) else {
            return Reply::err(400, "a call needs a 'tool' (GET /api/catalog lists them)");
        };
        let args = body.get("args").cloned().unwrap_or_else(|| json!({}));
        if self.agent_active() {
            return Reply::err(409, "the Assistant is running in the window; try again when it has finished");
        }
        let probe = buzz_ai::agent::ToolCall {
            id: "api".into(),
            tool: tool.clone(),
            args: args.clone(),
        };
        if buzz_ai::agent::needs_confirmation(&probe) {
            return Reply {
                status: 409,
                body: json!({
                    "ok": false,
                    "value": null,
                    "error": format!(
                        "refused: this {tool} call would ask the user to confirm (quit, close, or overwriting an existing file), and the API never waits on a dialog"
                    ),
                }),
            };
        }
        // Undo and redo walk the history; folding them into a step of their
        // own would undo the undo.
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        let walks_history = tool == "command" && matches!(name, "undo" | "redo");
        let checkpoint = (!walks_history).then(|| self.agent_editor().doc.run_checkpoint());
        let result = agent::execute(self, &AgentCall { tool: tool.clone(), args });
        if let Some(checkpoint) = checkpoint {
            let doc = &mut self.agent_editor_mut().doc;
            doc.end_gesture();
            doc.collapse_run(checkpoint, format!("API: {tool}"));
            doc.end_gesture();
        }
        Reply::ok(json!({"ok": result.ok, "value": result.value, "error": result.error}))
    }

    /// `POST /api/open {path}`. Reading happens in the background, as File ▸
    /// Open does; `GET /api/state` shows the document once it is open.
    fn api_open(&mut self, body: &Value) -> Reply {
        let Some(path) = body.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) else {
            return Reply::err(400, "open needs a 'path'");
        };
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Reply::err(404, format!("no file at {}", path.display()));
        }
        self.apply_pick(Pick::Open, path.clone());
        Reply::ok(json!({
            "ok": true,
            "opening": path.display().to_string(),
            "note": "the file is read in the background; GET /api/state shows it once it is open",
        }))
    }

    /// `POST /api/save {path?}`: to `path`, or to the document's own file.
    fn api_save(&mut self, body: &Value) -> Reply {
        let path = match body.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(p) => PathBuf::from(p),
            None => match self.agent_editor().doc.path() {
                Some(p) => p.to_path_buf(),
                None => return Reply::err(400, "the document has no file yet: pass a 'path'"),
            },
        };
        self.apply_pick(Pick::SaveAs, path.clone());
        let doc = &self.agent_editor().doc;
        if doc.path() == Some(path.as_path()) && !doc.is_dirty() {
            Reply::ok(json!({"ok": true, "path": path.display().to_string()}))
        } else {
            let why = self.agent_editor().status.clone().unwrap_or_else(|| "could not save".into());
            Reply::err(500, why)
        }
    }

    /// `POST /api/export {path, format, from?, to?}`: a job on the export queue,
    /// with the export dialog's own settings path (`App::start_export`).
    fn api_export(&mut self, body: &Value) -> Reply {
        let Some(path) = body.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) else {
            return Reply::err(400, "export needs a 'path'");
        };
        let path = PathBuf::from(path);
        let format = body.get("format").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let (kind, container) = match format.as_str() {
            "mp4" => (buzz_ui::ExportKind::Video, Some(buzz_ui::ContainerChoice::Mp4)),
            "mov" => (buzz_ui::ExportKind::Video, Some(buzz_ui::ContainerChoice::Mov)),
            "gif" => (buzz_ui::ExportKind::Gif, None),
            "webp" => (buzz_ui::ExportKind::Webp, None),
            "png" => (buzz_ui::ExportKind::Image, None),
            _ => return Reply::err(400, "'format' is one of mp4, mov, gif, webp, png"),
        };
        let frame = |key: &str| -> Result<Option<u32>, String> {
            match body.get(key).filter(|v| !v.is_null()) {
                None => Ok(None),
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .map(Some)
                    .ok_or_else(|| format!("'{key}' is a frame number (0 is the first)")),
            }
        };
        let (from, to) = match (frame("from"), frame("to")) {
            (Err(e), _) | (_, Err(e)) => return Reply::err(400, e),
            (Ok(a), Ok(b)) => (a, b),
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !parent.is_dir() {
                return Reply::err(400, format!("no folder {}", parent.display()));
            }
        }
        if kind.needs_ffmpeg() && !buzz_export::ffmpeg_available() {
            return Reply::err(422, format!("{format} needs ffmpeg, and none was found on this machine"));
        }

        // The export dialog's state is borrowed for this one export and put
        // back, so an API export neither changes the user's remembered
        // settings nor closes a dialog they have open.
        let saved_state = self.agent_editor().export.clone();
        let saved_frame = self.agent_editor().current_frame;
        {
            let editor = self.agent_editor_mut();
            let size = editor.scene().stage().size;
            let length = editor.doc.film_frames();
            editor.export.open(
                kind,
                (size.width.round().max(1.0) as u32, size.height.round().max(1.0) as u32),
                length,
            );
            editor.export.transparent = false;
            editor.export.region_to_selection = false;
            if let Some(container) = container {
                editor.export.video.container = container;
            }
            let last = length.saturating_sub(1);
            if let Some(f) = from {
                editor.export.from_frame = f.min(last);
            }
            if let Some(t) = to {
                editor.export.to_frame = t.min(last);
            }
            // A still is the frame `from` of the scene being edited, as File ▸
            // Export Image takes the playhead's.
            if kind == buzz_ui::ExportKind::Image {
                if let Some(f) = from {
                    editor.current_frame = f.min(editor.scene().frame_count().saturating_sub(1));
                }
            }
        }
        let id = self.api.new_job("export", path.clone(), JobState::Queued);
        self.apply_pick(Pick::Export(kind), path);
        let editor = self.agent_editor_mut();
        editor.export = saved_state;
        editor.current_frame = saved_frame;
        Reply::ok(json!({"ok": true, "job_id": id}))
    }

    /// `POST /api/episode {script_path | script_text, casting_path?, scenes?, out_dir, render?}`.
    fn api_episode(&mut self, body: &Value) -> Reply {
        let text = |key: &str| body.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string);
        let (script_path, script_text) = (text("script_path"), text("script_text"));
        let script = match (script_path, script_text) {
            (Some(p), None) => ScriptSource::Path(PathBuf::from(p)),
            (None, Some(t)) => ScriptSource::Text(t),
            (Some(_), Some(_)) => return Reply::err(400, "pass one of 'script_path' and 'script_text', not both"),
            (None, None) => return Reply::err(400, "an episode needs 'script_path' or 'script_text'"),
        };
        let Some(out_dir) = text("out_dir").map(PathBuf::from) else {
            return Reply::err(400, "an episode needs an 'out_dir'");
        };
        let casting = text("casting_path").map(PathBuf::from);
        let scenes = match body.get("scenes").filter(|v| !v.is_null()) {
            None => None,
            Some(Value::Array(items)) => {
                let numbers: Option<Vec<u32>> =
                    items.iter().map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok())).collect();
                match numbers {
                    Some(n) => Some(n),
                    None => return Reply::err(400, "'scenes' is a list of the script's scene numbers"),
                }
            }
            Some(_) => return Reply::err(400, "'scenes' is a list of the script's scene numbers"),
        };
        let render = body.get("render").and_then(Value::as_bool).unwrap_or(false);

        let (send, receive) = bounded(1);
        let ctx = self.api.ctx.clone();
        let request = EpisodeRequest { script, casting, scenes, out_dir: out_dir.clone() };
        let spawned = std::thread::Builder::new().name("buzz-api-episode".into()).spawn(move || {
            let _ = send.send(build_episode_files(request));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            return Reply::err(500, format!("could not start the build: {e}"));
        }
        let id = self.api.new_job("episode", out_dir, JobState::Running);
        if let Some(job) = self.api.jobs.iter_mut().find(|j| j.id == id) {
            job.episode = Some(receive);
            job.render = render;
        }
        Reply::ok(json!({"ok": true, "job_id": id}))
    }

    /// Collect finished episode builds; queue their previews when asked to.
    fn api_settle_episodes(&mut self) -> bool {
        let mut finished: Vec<(u64, Result<EpisodeDone, String>, bool)> = Vec::new();
        for job in &mut self.api.jobs {
            let Some(rx) = job.episode.as_ref() else { continue };
            match rx.try_recv() {
                Ok(result) => {
                    job.episode = None;
                    finished.push((job.id, result, job.render));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    job.episode = None;
                    job.state = JobState::Failed("the episode build stopped unexpectedly".into());
                }
            }
        }
        let did = !finished.is_empty();
        for (id, result, render) in finished {
            let (state, value) = match result {
                Err(e) => (JobState::Failed(e), Value::Null),
                Ok(done) => {
                    let mut warnings = done.warnings;
                    let mut render_jobs: Vec<u64> = Vec::new();
                    if render {
                        if buzz_export::ffmpeg_available() {
                            render_jobs = self.api_queue_previews(id, done.scenes);
                        } else {
                            warnings.push("render was asked for, but ffmpeg was not found, so no previews were queued".into());
                        }
                    }
                    let files: Vec<String> = done.files.iter().map(|p| p.display().to_string()).collect();
                    (JobState::Done, json!({"files": files, "render_jobs": render_jobs, "warnings": warnings}))
                }
            };
            if let Some(job) = self.api.jobs.iter_mut().find(|j| j.id == id) {
                job.state = state;
                job.result = value;
            }
        }
        did
    }

    /// One MP4 per built scene, on the export queue, each its own job.
    fn api_queue_previews(&mut self, episode: u64, scenes: Vec<(String, buzz_scene::Scene, u32)>) -> Vec<u64> {
        use crate::export_service::{ExportRequest, ExportTarget};
        let out_dir = self
            .api
            .jobs
            .iter()
            .find(|j| j.id == episode)
            .map(|j| j.output.join("previews"))
            .unwrap_or_default();
        if std::fs::create_dir_all(&out_dir).is_err() {
            return Vec::new();
        }
        let mut ids = Vec::new();
        for (stem, scene, length) in scenes {
            if length == 0 {
                continue;
            }
            let path = out_dir.join(format!("{stem}.mp4"));
            let settings = buzz_export::ExportSettings::for_stage(&scene);
            let id = self.api.new_job("export", path.clone(), JobState::Queued);
            self.api_enqueue_export(ExportRequest {
                scenes: vec![(scene, length)],
                settings,
                range: 0..length,
                target: ExportTarget::Video {
                    path: path.clone(),
                    video: buzz_export::VideoSettings::default(),
                },
                gpu: self.api_gpu(),
                label: format!("{stem}.mp4"),
            });
            ids.push(id);
        }
        ids
    }

    /// Queue an export exactly as `POST /api/export` does, for Send to
    /// BuzzEdit (`crate::estate`): the job's id, or why it was refused.
    pub(crate) fn api_queue_export(&mut self, body: &Value) -> Result<u64, String> {
        let reply = self.api_export(body);
        match reply.body.get("job_id").and_then(Value::as_u64) {
            Some(id) if reply.status == 200 => Ok(id),
            _ => Err(reply
                .body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the export was refused")
                .to_string()),
        }
    }

    /// A job as `GET /api/jobs/{id}` answers it, without polling the tasks
    /// first (the caller is the frame loop, which already has).
    pub(crate) fn api_job_state(&self, id: u64) -> Option<Value> {
        self.api.jobs.iter().find(|j| j.id == id).map(|j| self.api_job_json(j))
    }

    /// Let finished export tasks report before a job list is read.
    fn api_refresh_jobs(&mut self) {
        self.agent_poll_tasks();
        self.api_settle_episodes();
    }

    fn api_job_json(&self, job: &ApiJob) -> Value {
        let (state, error) = match &job.state {
            JobState::Queued => ("queued", None),
            JobState::Running => ("running", None),
            JobState::Done => ("done", None),
            JobState::Failed(e) => ("failed", Some(e.clone())),
        };
        let progress = match (&job.state, job.task) {
            (JobState::Running, Some(task)) => self.api_task_progress(task).map(|p| {
                json!({"done": p.done, "total": p.total, "fraction": p.fraction(), "detail": p.detail})
            }),
            (JobState::Done, _) => Some(json!({"fraction": 1.0})),
            _ => None,
        };
        json!({
            "id": job.id,
            "kind": job.kind,
            "state": state,
            "progress": progress,
            "output": job.output.display().to_string(),
            "error": error,
            "result": job.result,
        })
    }
}

// ------------------------------------------------------------------ episodes

// `pub(crate)` so Import Script from Studio (`crate::estate`, step E3) builds
// through this same path.
pub(crate) enum ScriptSource {
    Path(PathBuf),
    Text(String),
}

pub(crate) struct EpisodeRequest {
    pub(crate) script: ScriptSource,
    pub(crate) casting: Option<PathBuf>,
    pub(crate) scenes: Option<Vec<u32>>,
    pub(crate) out_dir: PathBuf,
}

/// The `examples/episode_build.rs` flow, on a worker thread: parse, cast,
/// build, save the `.buzz` and the report.
pub(crate) fn build_episode_files(request: EpisodeRequest) -> Result<EpisodeDone, String> {
    use crate::episode::{BuildOptions, Casting, build_episode, report_markdown, scene_file_stem};

    let (source, stem) = match &request.script {
        ScriptSource::Path(p) => (
            std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?,
            p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "episode".into()),
        ),
        ScriptSource::Text(t) => (t.clone(), "episode".to_string()),
    };
    // A Studio narration script becomes a narrator's screenplay here, so the
    // Studio calling `/api/episode` with its own script works as well as the
    // in-app import does; a real screenplay passes through untouched.
    let source = crate::estate::studio_to_screenplay(&source);
    let play = buzz_act::screenplay::Screenplay::parse(&source);
    if play.scenes.is_empty() {
        return Err("the script has no scenes in it (a scene starts with '## SCENE n — TITLE')".into());
    }
    let casting = match &request.casting {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
            Casting::from_json(&text)?
        }
        None => Casting::default(),
    };
    let numbers: Vec<u32> = request
        .scenes
        .clone()
        .unwrap_or_else(|| play.scenes.iter().map(|s| s.number).collect());
    let assets = buzz_doc::AssetLibrary::user();
    let opts = BuildOptions::default();
    std::fs::create_dir_all(&request.out_dir)
        .map_err(|e| format!("creating {}: {e}", request.out_dir.display()))?;

    let (mut doc, reports) = build_episode(&play, &casting, &assets, &numbers, &opts);
    let buzz = request.out_dir.join(format!("{stem}.buzz"));
    doc.save_as(&buzz).map_err(|e| format!("saving {}: {e}", buzz.display()))?;
    let report = request.out_dir.join("report.md");
    let title = if play.title.is_empty() { "episode" } else { play.title.as_str() };
    std::fs::write(&report, report_markdown(title, &reports, opts.fps))
        .map_err(|e| format!("writing {}: {e}", report.display()))?;

    // The scenes that were built, in the order they sit in the document.
    let built: Vec<u32> = reports.iter().filter(|r| r.frames > 0).map(|r| r.number).collect();
    let scenes = doc
        .film_with_lengths()
        .into_iter()
        .enumerate()
        .map(|(index, (scene, length))| {
            let number = built.get(index).copied().unwrap_or(index as u32 + 1);
            let title = reports.iter().find(|r| r.number == number).map(|r| r.title.clone()).unwrap_or_default();
            (scene_file_stem(number, &title), scene, length)
        })
        .collect();
    let warnings = reports
        .iter()
        .flat_map(|r| r.warnings.iter().map(move |w| format!("scene {}: {w}", r.number)))
        .collect();
    Ok(EpisodeDone {
        files: vec![buzz, report],
        scenes,
        warnings,
    })
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_render::GpuPreference;

    #[test]
    fn only_this_machine_may_drive_the_api() {
        for ok in [None, Some(""), Some("http://127.0.0.1:5173"), Some("http://localhost:8099"), Some("http://localhost")] {
            assert!(origin_allowed(ok), "{ok:?} should be allowed");
        }
        for bad in [
            // A sandboxed iframe on any website sends this.
            Some("null"),
            Some("https://evil.example"),
            Some("http://localhost.evil.example"),
            Some("http://127.0.0.1.evil.example:80"),
            Some("http://127.0.0.1:80@evil.example"),
            Some("https://localhost:5173"),
        ] {
            assert!(!origin_allowed(bad), "{bad:?} should be refused");
        }

        // DNS rebinding: a page whose own name now points at 127.0.0.1 still
        // sends its own name as the Host.
        for ok in [None, Some("127.0.0.1:8840"), Some("localhost:8840"), Some("LOCALHOST"), Some("[::1]:8840")] {
            assert!(host_allowed(ok, 8840), "{ok:?} should be allowed");
        }
        for bad in [
            Some("evil.example:8840"),
            Some("127.0.0.1.evil.example:8840"),
            Some("localhost.evil.example"),
            Some("127.0.0.1:8841"),
            Some("127.0.0.1:"),
        ] {
            assert!(!host_allowed(bad, 8840), "{bad:?} should be refused");
        }
    }

    fn frame(app: &mut App, ctx: &egui::Context) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
    }

    /// A request from another thread, with frames run until it is answered:
    /// what an idle window does when the server asks it to repaint.
    fn request(
        app: &mut App,
        ctx: &egui::Context,
        port: u16,
        method: &str,
        path: &str,
        body: Option<Value>,
        origin: Option<&str>,
    ) -> (u16, Value) {
        let (method, path, origin) = (method.to_string(), path.to_string(), origin.map(str::to_string));
        let handle = std::thread::spawn(move || {
            http_request(port, &method, &path, body.as_ref(), origin.as_deref(), Duration::from_secs(90))
        });
        let start = std::time::Instant::now();
        while !handle.is_finished() {
            frame(app, ctx);
            assert!(start.elapsed() < Duration::from_secs(90), "no answer");
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.join().unwrap().expect("the request reached the server")
    }

    fn wait_for_job(app: &mut App, ctx: &egui::Context, port: u16, id: u64, limit: Duration) -> Value {
        let start = std::time::Instant::now();
        loop {
            let (status, job) = request(app, ctx, port, "GET", &format!("/api/jobs/{id}"), None, None);
            assert_eq!(status, 200, "{job}");
            if job["state"] != "running" && job["state"] != "queued" {
                return job;
            }
            assert!(start.elapsed() < limit, "job {id} did not finish: {job}");
            for _ in 0..10 {
                frame(app, ctx);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn layers(app: &App) -> usize {
        app.agent_editor().doc.scene().layers().iter().count()
    }

    /// The whole API against a headless app, with a ledger of its own and a
    /// port nobody else wants: identity, publish and withdraw, a call through
    /// to the document and back out with one undo, a refused cross-origin
    /// request, an export and an episode reaching `done`.
    #[test]
    fn the_local_api_answers_publishes_and_drives_the_document() {
        let dir = std::env::temp_dir().join(format!("buzzanimate-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Under a `Buzzcaf` folder, like the real one, so `ports`' own test of
        // `ledger_path()`'s shape still holds while this one runs beside it.
        let ledger = dir.join("Buzzcaf").join("ports.json");
        let wish = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        // SAFETY: the only readers of these two variables in this test binary
        // are `ports::ledger_path` and `ApiConfig::from_env`; nothing else in
        // the process reads or writes them, so a concurrent `getenv` cannot see
        // a half-written value of either.
        unsafe {
            std::env::set_var("BUZZCAF_PORTS_FILE", &ledger);
            std::env::set_var("BUZZANIMATE_PORT", wish.to_string());
        }

        let mut app = App::new(GpuPreference::Automatic).with_api();
        let ctx = egui::Context::default();
        let start = std::time::Instant::now();
        while !app.api_state().published() {
            frame(&mut app, &ctx);
            assert!(start.elapsed() < Duration::from_secs(20), "the API never published");
            std::thread::sleep(Duration::from_millis(10));
        }
        let port = app.api_state().port().unwrap();

        // Identity, and the ledger entry that points at it.
        let (status, health) = request(&mut app, &ctx, port, "GET", "/health", None, None);
        assert_eq!(status, 200);
        assert_eq!(health["app"], "buzzanimate");
        assert_eq!(health["status"], "ok");
        assert_eq!(health["port"], port);
        assert_eq!(health["pid"], std::process::id());
        let entry = ports::entry_at(&ledger, "buzzanimate").expect("published");
        assert_eq!(entry["port"], port);
        assert_eq!(entry["pid"], std::process::id());
        assert_eq!(entry["health"], format!("http://127.0.0.1:{port}/health"));

        // State and catalog.
        let (status, state) = request(&mut app, &ctx, port, "GET", "/api/state", None, Some("http://localhost:5173"));
        assert_eq!(status, 200, "a local page may ask: {state}");
        assert!(state["frame_count"].as_u64().is_some() && state["scenes"].is_array(), "{state}");
        let (status, catalog) = request(&mut app, &ctx, port, "GET", "/api/catalog", None, None);
        assert_eq!(status, 200);
        assert!(catalog.as_array().is_some_and(|c| c.iter().any(|e| e["id"] == "new_layer")), "{catalog}");

        // A call reaches the document as one undo step labelled for the API...
        let before = layers(&app);
        let (status, call) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/call",
            Some(json!({"tool": "command", "args": {"name": "new_layer"}})),
            None,
        );
        assert_eq!(status, 200);
        assert_eq!(call["ok"], true, "{call}");
        assert_eq!(layers(&app), before + 1);
        assert_eq!(app.agent_editor().doc.history().next_undo_label(), Some("API: command"));
        // ...and one undo through the API takes it back out.
        let (_, undo) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/call",
            Some(json!({"tool": "command", "args": {"name": "undo"}})),
            None,
        );
        assert_eq!(undo["ok"], true, "{undo}");
        assert_eq!(layers(&app), before);

        // A web page elsewhere is refused before anything runs.
        let (status, refused) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/call",
            Some(json!({"tool": "command", "args": {"name": "new_layer"}})),
            Some("https://evil.example"),
        );
        assert_eq!(status, 403, "{refused}");
        assert_eq!(layers(&app), before, "a refused request changes nothing");

        // A call that would need a confirmation is refused, not left waiting.
        let (status, quit) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/call",
            Some(json!({"tool": "command", "args": {"name": "quit"}})),
            None,
        );
        assert_eq!(status, 409, "{quit}");
        assert!(!app.agent_editor().should_quit);

        // Save to a file of our own.
        let saved = dir.join("api.buzz");
        let (status, save) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/save",
            Some(json!({"path": saved.display().to_string()})),
            None,
        );
        assert_eq!(status, 200, "{save}");
        assert!(saved.is_file());

        // An export on the queue, to `done`: a still always, and a two-frame
        // MP4 when this machine has ffmpeg.
        let still = dir.join("still.png");
        let (status, export) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/export",
            Some(json!({"path": still.display().to_string(), "format": "png"})),
            None,
        );
        assert_eq!(status, 200, "{export}");
        let job = wait_for_job(&mut app, &ctx, port, export["job_id"].as_u64().unwrap(), Duration::from_secs(120));
        assert_eq!(job["state"], "done", "{job}");
        assert!(still.is_file());
        if buzz_export::ffmpeg_available() {
            let film = dir.join("film.mp4");
            let (status, export) = request(
                &mut app,
                &ctx,
                port,
                "POST",
                "/api/export",
                Some(json!({"path": film.display().to_string(), "format": "mp4", "from": 0, "to": 1})),
                None,
            );
            assert_eq!(status, 200, "{export}");
            let job = wait_for_job(&mut app, &ctx, port, export["job_id"].as_u64().unwrap(), Duration::from_secs(180));
            assert_eq!(job["state"], "done", "{job}");
            assert!(film.is_file());
        }
        let (status, jobs) = request(&mut app, &ctx, port, "GET", "/api/jobs", None, None);
        assert_eq!(status, 200);
        assert!(jobs["jobs"].as_array().is_some_and(|j| !j.is_empty()));

        // An episode from a two-line script.
        let out = dir.join("episode");
        let script = "## SCENE 1 — TEST\n\n**GUDDU**\nHello there.\n";
        let (status, episode) = request(
            &mut app,
            &ctx,
            port,
            "POST",
            "/api/episode",
            Some(json!({"script_text": script, "out_dir": out.display().to_string()})),
            None,
        );
        assert_eq!(status, 200, "{episode}");
        let job = wait_for_job(&mut app, &ctx, port, episode["job_id"].as_u64().unwrap(), Duration::from_secs(120));
        assert_eq!(job["state"], "done", "{job}");
        let files: Vec<String> = job["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap().to_string())
            .collect();
        let buzz = files.iter().find(|f| f.ends_with(".buzz")).expect("a .buzz in the result");
        assert!(Path::new(buzz).is_file());
        // With `render`, each built scene's preview is its own export job.
        if buzz_export::ffmpeg_available() {
            let out = dir.join("episode-rendered");
            let (status, episode) = request(
                &mut app,
                &ctx,
                port,
                "POST",
                "/api/episode",
                Some(json!({"script_text": script, "out_dir": out.display().to_string(), "render": true})),
                None,
            );
            assert_eq!(status, 200, "{episode}");
            let job = wait_for_job(&mut app, &ctx, port, episode["job_id"].as_u64().unwrap(), Duration::from_secs(120));
            assert_eq!(job["state"], "done", "{job}");
            let renders = job["result"]["render_jobs"].as_array().cloned().unwrap_or_default();
            assert!(!renders.is_empty(), "a preview per built scene: {job}");
            for id in renders {
                let render = wait_for_job(&mut app, &ctx, port, id.as_u64().unwrap(), Duration::from_secs(300));
                assert_eq!(render["state"], "done", "{render}");
                assert!(Path::new(render["output"].as_str().unwrap()).is_file());
            }
        }

        // Unknown routes and wrong methods say so.
        let (status, _) = request(&mut app, &ctx, port, "GET", "/api/nothing", None, None);
        assert_eq!(status, 404);
        let (status, _) = request(&mut app, &ctx, port, "GET", "/api/call", None, None);
        assert_eq!(status, 405);

        // A clean stop withdraws the entry, and the port stops answering.
        app.api_shutdown();
        assert!(ports::entry_at(&ledger, "buzzanimate").is_none(), "withdrawn on shutdown");
        assert!(http_request(port, "GET", "/health", None, None, Duration::from_secs(2)).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
