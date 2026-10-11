//! The Assistant's bridge between the agent loop and the app (Stage E2 of
//! `docs/AI_AGENT_PLAN.md`).
//!
//! `buzz_ai::agent::run_agent` runs on a background thread (LLM calls take
//! seconds). Its [`AgentHost`] here sends every tool call over a channel to the
//! UI thread and blocks on the reply; [`App::agent_pump`], called once per frame,
//! executes the call with `crate::agent::execute` (the same code the menus and
//! tools use) and answers. No document is touched off the UI thread.
//!
//! One run is one undo step: the document's history is checkpointed at the start
//! and, when the run ends, collapsed into a single `AI: <request>` entry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine as _;
use buzz_ai::agent::{
    AgentEvent, AgentHost, AgentLimits, AgentRunResult, AgentStatus, ChatTransport, ToolCall,
    ToolOutcome, ToolSpec, probe_native_tools, run_agent, run_agent_with,
};
use buzz_ai::llm::Backend;
use buzz_ui::ai_panel::StepKind;
use buzz_ui::workspace::AiSettings;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::{Value, json};

use crate::agent::{self, AgentCall};
use crate::app::App;

// ------------------------------------------------------------------ messages

/// Everything the worker thread tells the UI thread, in order.
pub enum BridgeMsg {
    Event(AgentEvent),
    /// Run this call, answer on `reply`.
    Call { call: ToolCall, reply: Sender<ToolOutcome> },
    /// `wait_for_jobs` polling (H3): answer on `reply` with the jobs' state,
    /// marking finished results as handed over when `deliver` is set. The
    /// worker thread does the waiting and sends this between sleeps, so the UI
    /// thread only ever answers a cheap question.
    Jobs { deliver: bool, reply: Sender<Value> },
    /// Ask the user, answer on `reply`.
    Confirm { call: ToolCall, reply: Sender<bool> },
    /// A line for the Assistant's log (e.g. an auto-approval).
    Note(String),
    Finished(AgentRunResult),
}

/// Builds the chat transport on the worker thread (a test passes a scripted one).
pub type TransportFactory = Box<dyn FnOnce() -> Box<dyn ChatTransport> + Send>;

/// Where a run's model comes from.
pub enum Source {
    Real(BackendChoice),
    Custom(TransportFactory),
}

// ----------------------------------------------------------- backend choice

/// The backend and model a run would use.
#[derive(Clone)]
pub struct BackendChoice {
    pub backend: Backend,
    pub model: String,
    pub label: String,
    /// The Claude proxy: native tool calling without probing.
    pub claude: bool,
}

const GUIDANCE_NO_BACKEND: &str = "The Assistant needs a model. Start the Claude proxy (Buzzcaf Studio's server at 127.0.0.1:8787) and tick \"Use Claude for the Assistant\" in AI Models, or pick a local model (LM Studio or Ollama) in AI Models.";

const GUIDANCE_NO_KEY: &str = "\"Use Claude for the Assistant\" is on but there is no key. In AI Models, click \"Copy key from Buzzcaf Studio\" (and start the proxy).";

/// Claude when the switch is on and a key exists; else the AI panel's selected
/// local backend and model. Never a silent fallback from Claude to local, and
/// never Claude when the switch is off.
pub fn choose_backend_from(
    use_claude: bool,
    has_key: bool,
    claude_model: &str,
    settings: &AiSettings,
) -> Result<BackendChoice, String> {
    if use_claude {
        if !has_key {
            return Err(GUIDANCE_NO_KEY.to_string());
        }
        let model = if claude_model.trim().is_empty() { buzz_ai::ai_config::CLAUDE_DEFAULT_MODEL } else { claude_model.trim() };
        return Ok(BackendChoice {
            backend: Backend::default_claude_api(),
            model: model.to_string(),
            label: format!("Claude ({model})"),
            claude: true,
        });
    }
    let (Some(backend), Some(model)) = (settings.backend.clone(), settings.model.clone()) else {
        return Err(GUIDANCE_NO_BACKEND.to_string());
    };
    if model.trim().is_empty() || matches!(backend, Backend::ClaudeApiServer { .. }) {
        return Err(GUIDANCE_NO_BACKEND.to_string());
    }
    let label = format!("{}: {model}", backend.display_name());
    Ok(BackendChoice { backend, model, label, claude: false })
}

/// [`choose_backend_from`] with the saved `ai.json` choices.
pub fn choose_backend(settings: &AiSettings) -> Result<BackendChoice, String> {
    choose_backend_from(
        buzz_ai::ai_config::use_claude(),
        buzz_ai::ai_config::claude_api_key().is_some(),
        &buzz_ai::ai_config::claude_model(),
        settings,
    )
}

/// `probe_native_tools` once per backend + model (it is a real request).
fn native_tools_cached(backend: &Backend, model: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = format!("{}|{}", backend.base_url(), model);
    if let Some(known) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return known;
    }
    let result = probe_native_tools(backend, model);
    if let Ok(mut c) = cache.lock() {
        c.insert(key, result);
    }
    result
}

// -------------------------------------------------------------------- tools

/// The tool list the model sees, built from the executor's own catalog so it
/// cannot drift. `command`'s first description line lists every runnable id
/// (the JSON protocol only shows first lines); later lines carry argument hints.
pub fn build_tools() -> Vec<ToolSpec> {
    build_tools_with(false)
}

/// [`build_tools`], with the excluded commands listed as runnable when
/// "Allow all" is on.
pub fn build_tools_with(allow_all: bool) -> Vec<ToolSpec> {
    let entries = agent::catalog_with(allow_all);
    let entries = entries.as_array().cloned().unwrap_or_default();
    let mut ids = Vec::new();
    let mut hints = Vec::new();
    for e in &entries {
        let how = e["how"].as_str().unwrap_or("");
        if how == "tool" || how == "excluded" || how == "needs_dialog" {
            continue;
        }
        let id = e["id"].as_str().unwrap_or("");
        ids.push(id.to_string());
        if let Some(args) = e["args"].as_str().filter(|a| !a.is_empty()) {
            hints.push(format!("{id} {args}"));
        }
    }
    let command_description = format!(
        "Run an app command by id (like a menu item or shortcut): {}\nData and file commands take arguments:\n{}",
        ids.join(", "),
        hints.join("\n")
    );
    let hint_of = |name: &str| {
        agent::AGENT_TOOLS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, h)| *h)
            .unwrap_or("")
    };
    let tool = |name: &str, parameters: Value| ToolSpec {
        name: name.to_string(),
        description: format!("{name}: {}", hint_of(name)),
        parameters,
    };
    let mut tools = vec![
        ToolSpec {
            name: "command".into(),
            description: command_description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "command id"},
                    "args": {"type": "object", "description": "arguments for data commands"},
                    "path": {"type": "string", "description": "file path for file commands"}
                },
                "required": ["name"]
            }),
        },
        ToolSpec {
            name: "inspect".into(),
            description: "Look at the document: what = document|timeline|objects (everything on stage, with bounds)|selection|library|assets|tools|properties|catalog (properties takes target)".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "what": {"type": "string", "enum": ["document", "timeline", "objects", "selection", "library", "assets", "tools", "properties", "catalog"]},
                    "target": {"type": "string"}
                },
                "required": ["what"]
            }),
        },
        tool(
            "gesture",
            json!({
                "type": "object",
                "properties": {
                    "tool": {"type": "string"},
                    "points": {"type": "array", "items": {"type": "object", "properties": {
                        "x": {"type": "number"}, "y": {"type": "number"},
                        "pressure": {"type": "number"}, "t": {"type": "number"}}, "required": ["x", "y"]}},
                    "mods": {"type": "object", "properties": {
                        "shift": {"type": "boolean"}, "alt": {"type": "boolean"}, "ctrl": {"type": "boolean"}}}
                },
                "required": ["tool", "points"]
            }),
        ),
        // **Every field spelled out.** This was a bare "any object", which
        // Claude reads past but a local server enforces: LM Studio constrains
        // the model's arguments to the schema, so the only thing it could
        // produce was `{}` — and the live run spent its whole turn budget
        // calling `set_style {}` and never got a red circle.
        tool(
            "set_style",
            json!({
                "type": "object",
                "properties": {
                    "stroke": {"type": "string", "description": "#rrggbb or #rrggbbaa; also turns the stroke on"},
                    "fill": {"type": "string", "description": "#rrggbb or #rrggbbaa; also turns a solid fill on"},
                    "stroke_enabled": {"type": "boolean"},
                    "fill_enabled": {"type": "boolean"},
                    "stroke_width": {"type": "number"},
                    "brush_size": {"type": "number"},
                    "brush_kind": {"type": "string"},
                    "effect_brush": {"type": "string"},
                    "brush_smoothing": {"type": "number", "description": "0..1"},
                    "eraser_size": {"type": "number"},
                    "eraser_mode": {"type": "string"},
                    "hairline": {"type": "boolean"},
                    "stroke_kind": {"type": "string"},
                    "drawing_mode": {"type": "string"},
                    "auto_keyframe": {"type": "boolean"}
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "script",
            json!({"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}),
        ),
        tool(
            "snapshot",
            json!({"type": "object", "properties": {"frame": {"type": "integer"}}}),
        ),
        tool(
            "set_property",
            json!({
                "type": "object",
                "properties": {"target": {"type": "string"}, "field": {"type": "string"}, "value": {}},
                "required": ["target", "field", "value"]
            }),
        ),
    ];
    // Tools the executor gained since (kept in step with AGENT_TOOLS).
    for (name, _) in agent::AGENT_TOOLS {
        if tools.iter().any(|t| t.name == *name) {
            continue;
        }
        let schema = match *name {
            "edit_list" => json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string"}, "list": {"type": "string"}, "op": {"type": "string"},
                    "index": {"type": "integer"}, "to": {}, "item": {"type": "object"}
                },
                "required": ["target", "list", "op"]
            }),
            "wait_for_jobs" => json!({
                "type": "object",
                "properties": {
                    "timeout_s": {"type": "integer", "minimum": 1, "maximum": 300, "description": "longest wait in seconds (default 120)"}
                }
            }),
            "transcribe" => json!({
                "type": "object",
                "properties": {
                    "sound": {"type": "string", "description": "name of a sound in the library"},
                    "path": {"type": "string", "description": "an audio file on disk"}
                }
            }),
            // BuzzEdit and Buzzcaf Studio (`crate::estate`).
            "send_to_buzzedit" => json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["document", "scene"]},
                    "project": {"type": "string", "description": "a BuzzEdit project id (default: a new project)"},
                    "path": {"type": "string", "description": "the .mp4 to write"},
                    "start": {"type": "boolean", "description": "launch BuzzEdit if it is not running"}
                }
            }),
            "buzzedit_projects" | "studio_projects" => json!({
                "type": "object",
                "properties": {
                    "start": {"type": "boolean", "description": "launch the app if it is not running"}
                }
            }),
            "transcribe_with_buzzedit" => json!({
                "type": "object",
                "properties": {
                    "sound": {"type": "string", "description": "name of a sound in the library"},
                    "speaker": {"type": "string", "description": "who is talking: the mouth symbol to lip-sync"},
                    "start": {"type": "boolean", "description": "launch BuzzEdit if it is not running"}
                },
                "required": ["sound", "speaker"]
            }),
            "dress_set_from_assets" => json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "the scene's words: slugline and what happens"},
                    "style": {"type": "string", "description": "name of an asset whose style to follow"}
                },
                "required": ["query"]
            }),
            "arrange_assets" => json!({
                "type": "object",
                "properties": {
                    "dry_run": {"type": "boolean", "description": "true (default): only report the plan; false: move the files"}
                }
            }),
            "studio_build_episode" => json!({
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "a Studio project id"},
                    "asset": {"type": "string", "description": "the script asset (default script)"},
                    "out_dir": {"type": "string", "description": "default <the Studio project's folder>/animation"},
                    "start": {"type": "boolean", "description": "launch the Studio if it is not running"}
                },
                "required": ["project"]
            }),
            _ => json!({"type": "object", "additionalProperties": true}),
        };
        tools.push(tool(name, schema));
    }
    tools
}

// ------------------------------------------------------------- worker thread

struct ChannelHost {
    tools: Vec<ToolSpec>,
    tx: Sender<BridgeMsg>,
    stop: Arc<AtomicBool>,
    /// "Allow all": every confirmation is answered yes without asking.
    allow_all: Arc<AtomicBool>,
}

impl AgentHost for ChannelHost {
    fn tools(&self) -> Vec<ToolSpec> {
        self.tools.clone()
    }

    fn execute(&mut self, call: &ToolCall) -> ToolOutcome {
        if call.tool == "wait_for_jobs" {
            return match agent::jobs::parse_timeout(&call.args) {
                Ok(secs) => wait_for_jobs(&self.tx, &self.stop, Duration::from_secs(secs), JOB_POLL),
                Err(e) => ToolOutcome::err(e),
            };
        }
        let (reply, answer) = bounded(1);
        if self.tx.send(BridgeMsg::Call { call: call.clone(), reply }).is_err() {
            return ToolOutcome::err("the app closed");
        }
        answer.recv().unwrap_or_else(|_| ToolOutcome::err("the app closed"))
    }

    fn confirm(&mut self, call: &ToolCall) -> bool {
        if self.allow_all.load(Ordering::Relaxed) {
            let _ = self.tx.send(BridgeMsg::Note(format!("{}: auto-approved (Allow all)", describe_call(call))));
            return true;
        }
        let (reply, answer) = bounded(1);
        if self.tx.send(BridgeMsg::Confirm { call: call.clone(), reply }).is_err() {
            return false;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(yes) => return yes,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if self.stop.load(Ordering::Relaxed) {
                        return false;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }
}

/// How often `wait_for_jobs` asks the UI thread how the jobs are doing, and so
/// how soon Stop is noticed.
const JOB_POLL: Duration = Duration::from_millis(100);

/// **`wait_for_jobs`, on the worker thread** (H3). Polls the jobs through the
/// bridge until none is running or `timeout` passes, then asks once more for
/// the result. Sleeping here and not on the UI thread is the point: the window
/// keeps painting and the jobs keep making progress while the model waits, and
/// the Stop flag ends the wait within one poll.
fn wait_for_jobs(tx: &Sender<BridgeMsg>, stop: &AtomicBool, timeout: Duration, poll: Duration) -> ToolOutcome {
    let ask = |deliver: bool| -> Option<Value> {
        let (reply, answer) = bounded(1);
        tx.send(BridgeMsg::Jobs { deliver, reply }).ok()?;
        answer.recv().ok()
    };
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return ToolOutcome::err("stopped by user");
        }
        let Some(state) = ask(false) else { return ToolOutcome::err("the app closed") };
        let running = state["running"].as_u64().unwrap_or(0);
        if running == 0 || started.elapsed() >= timeout {
            let Some(mut result) = ask(true) else { return ToolOutcome::err("the app closed") };
            let still = result["running"].as_u64().unwrap_or(0) > 0;
            result["timed_out"] = json!(still);
            if still {
                result["note"] = json!("timed out; these jobs are still running: call wait_for_jobs again");
            }
            return ToolOutcome::ok(result);
        }
        std::thread::sleep(poll);
    }
}

fn spawn_worker(
    request: String,
    source: Source,
    tools: Vec<ToolSpec>,
    stop: Arc<AtomicBool>,
    allow_all: Arc<AtomicBool>,
    tx: Sender<BridgeMsg>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name("assistant".into()).spawn(move || {
        let mut host = ChannelHost { tools, tx: tx.clone(), stop: stop.clone(), allow_all };
        let events = tx.clone();
        let mut on_event = move |e: AgentEvent| {
            let _ = events.send(BridgeMsg::Event(e));
        };
        let limits = AgentLimits::default();
        let result = match source {
            Source::Custom(factory) => {
                let mut transport = factory();
                run_agent_with(&request, transport.as_mut(), &mut host, limits, &stop, &mut on_event)
            }
            Source::Real(choice) => {
                let native = choice.claude || {
                    on_event(AgentEvent::Thinking("checking whether the model supports tool calls".into()));
                    native_tools_cached(&choice.backend, &choice.model)
                };
                let limits = AgentLimits { native_tools: native, ..limits };
                run_agent(&request, &choice.backend, &choice.model, &mut host, limits, &stop, &mut on_event)
            }
        };
        let _ = tx.send(BridgeMsg::Finished(result));
    })
}

// ---------------------------------------------------------------- UI side

/// A call held for one frame so "running script..." can be painted first.
struct HeldCall {
    call: ToolCall,
    reply: Sender<ToolOutcome>,
}

struct ActiveRun {
    rx: Receiver<BridgeMsg>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    checkpoint: buzz_doc::RunCheckpoint,
    label: String,
    held: Option<HeldCall>,
    confirm_reply: Option<Sender<bool>>,
    calls: usize,
}

/// The app's side of the Assistant: at most one run at a time.
#[derive(Default)]
pub struct AgentBridge {
    run: Option<ActiveRun>,
    label_checked: Option<std::time::Instant>,
    /// (calls, turns) of the last finished run, for the live-acceptance harness.
    last_run: Option<(usize, usize)>,
    /// "Allow all", shared with the worker thread's host.
    pub(crate) allow_all: Arc<AtomicBool>,
    /// Tests: the next `agent_start` uses this scripted source instead of a real backend.
    #[cfg(test)]
    pub(crate) test_source: Option<(Source, String)>,
}

fn short(text: &str, max: usize) -> String {
    let t: String = text.chars().take(max).collect();
    if text.chars().count() > max { format!("{t}...") } else { t }
}

fn describe_call(call: &ToolCall) -> String {
    format!("{} {}", call.tool, short(&call.args.to_string(), 110))
}

fn is_long(call: &ToolCall) -> Option<&'static str> {
    match call.tool.as_str() {
        "script" => Some("running script..."),
        "snapshot" => Some("taking a snapshot..."),
        _ => None,
    }
}

/// Run one call on the UI thread and shape the answer for the model.
fn run_call(app: &mut App, call: &ToolCall) -> ToolOutcome {
    let result = agent::execute(app, &AgentCall { tool: call.tool.clone(), args: call.args.clone() });
    let mut outcome = ToolOutcome {
        ok: result.ok,
        value: result.value,
        error: result.error,
        image_png: None,
    };
    if call.tool == "snapshot" {
        if let Some(b64) = outcome.value.get("png_base64").and_then(|v| v.as_str()) {
            outcome.image_png = base64::engine::general_purpose::STANDARD.decode(b64).ok();
        }
        if let Some(map) = outcome.value.as_object_mut() {
            map.remove("png_base64");
        }
    }
    outcome
}

impl App {
    /// True while a run is in flight.
    pub(crate) fn agent_active(&self) -> bool {
        self.agent_bridge.run.is_some()
    }

    /// The running run's stop flag, for the `script` tool's interrupt handler.
    /// "Allow all" is on.
    pub(crate) fn agent_allow_all(&self) -> bool {
        self.agent_bridge.allow_all.load(Ordering::Relaxed)
    }

    pub(crate) fn agent_stop_flag(&self) -> Option<Arc<AtomicBool>> {
        self.agent_bridge.run.as_ref().map(|r| r.stop.clone())
    }

    /// Keep the panel's backend label current (cheap: re-read at most every 3 s).
    pub(crate) fn agent_refresh_label(&mut self) {
        if self.agent_bridge.run.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if self.agent_bridge.label_checked.is_some_and(|t| now.duration_since(t) < Duration::from_secs(3)) {
            return;
        }
        self.agent_bridge.label_checked = Some(now);
        self.ai_panel.assistant.backend_label = match choose_backend(&self.agent_editor().workspace.ai) {
            Ok(c) => c.label,
            Err(_) => "no model available".to_string(),
        };
    }

    /// Start a run on the chosen backend, or explain what is missing.
    pub(crate) fn agent_start(&mut self, request: String) {
        if self.agent_active() {
            return;
        }
        #[cfg(test)]
        if let Some((source, label)) = self.agent_bridge.test_source.take() {
            self.agent_launch(request, source, label);
            return;
        }
        match choose_backend(&self.agent_editor().workspace.ai) {
            Ok(choice) => {
                let label = choice.label.clone();
                self.agent_launch(request, Source::Real(choice), label);
            }
            Err(guidance) => {
                self.ai_panel.assistant.guidance = Some(guidance);
                self.ai_panel.assistant.backend_label = "no model available".into();
            }
        }
    }

    /// Start a run from any source (tests pass a scripted transport).
    pub(crate) fn agent_launch(&mut self, request: String, source: Source, label: String) {
        if self.agent_active() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = unbounded();
        let tools = build_tools_with(self.agent_allow_all());
        let checkpoint = self.agent_editor().doc.run_checkpoint();
        // `wait_for_jobs` reports the jobs of *this* run.
        self.agent_jobs.clear();
        let handle = match spawn_worker(request.clone(), source, tools, stop.clone(), self.agent_bridge.allow_all.clone(), tx) {
            Ok(h) => h,
            Err(e) => {
                self.ai_panel.assistant.guidance = Some(format!("Could not start the Assistant: {e}"));
                return;
            }
        };
        let undo_label = format!("AI: {}", request.trim().chars().take(40).collect::<String>().replace('\n', " "));
        let a = &mut self.ai_panel.assistant;
        a.log.clear();
        a.reply.clear();
        a.guidance = None;
        a.busy_note = None;
        a.confirm = None;
        a.running = true;
        a.backend_label = label;
        self.agent_editor_mut().status = Some("Assistant running...".into());
        self.agent_bridge.run = Some(ActiveRun {
            rx,
            stop,
            handle: Some(handle),
            checkpoint,
            label: undo_label,
            held: None,
            confirm_reply: None,
            calls: 0,
        });
    }

    /// The Stop button: no further call runs; a pending confirmation is a No.
    pub(crate) fn agent_stop(&mut self) {
        let Some(run) = self.agent_bridge.run.as_mut() else { return };
        run.stop.store(true, Ordering::Relaxed);
        if let Some(reply) = run.confirm_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(held) = run.held.take() {
            let _ = held.reply.send(ToolOutcome::err("stopped by user"));
        }
        self.ai_panel.assistant.confirm = None;
        self.ai_panel.assistant.busy_note = None;
    }

    /// The user's Yes/No on a pending confirmation.
    pub(crate) fn agent_confirm(&mut self, yes: bool) {
        if let Some(reply) = self.agent_bridge.run.as_mut().and_then(|r| r.confirm_reply.take()) {
            let _ = reply.send(yes);
        }
        self.ai_panel.assistant.confirm = None;
    }

    /// Stop and end the run now, without waiting for the worker (it may be
    /// inside an LLM request): for closing the document or the app. Everything
    /// done so far still collapses into one undo step.
    pub(crate) fn agent_abort(&mut self, why: &str) {
        if self.agent_bridge.run.is_none() {
            return;
        }
        self.agent_stop();
        self.ai_panel.assistant.push(StepKind::Note, why.to_string());
        self.agent_end(None);
    }

    /// Once per frame: execute the worker's calls and fold its events into the
    /// panel. Returns true if it did anything (the caller repaints at once).
    pub(crate) fn agent_pump(&mut self) -> bool {
        let Some(run) = self.agent_bridge.run.as_ref() else { return false };
        if !run.checkpoint.is_for(&self.agent_editor().doc) {
            self.agent_abort("The document was closed or replaced, so the Assistant stopped.");
            return true;
        }
        let mut did = false;

        // A long call held back last frame so its "running..." note was painted.
        if let Some(held) = self.agent_bridge.run.as_mut().and_then(|r| r.held.take()) {
            let outcome = run_call(self, &held.call);
            let _ = held.reply.send(outcome);
            self.ai_panel.assistant.busy_note = None;
            did = true;
        }

        loop {
            let Some(run) = self.agent_bridge.run.as_mut() else { return true };
            let msg = match run.rx.try_recv() {
                Ok(m) => m,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    // The worker ended without a result (it panicked).
                    self.ai_panel.assistant.push(StepKind::Failed, "the Assistant's worker stopped unexpectedly");
                    self.agent_end(None);
                    return true;
                }
            };
            did = true;
            match msg {
                BridgeMsg::Event(e) => self.agent_event(e),
                BridgeMsg::Call { call, reply } => {
                    if run.stop.load(Ordering::Relaxed) {
                        let _ = reply.send(ToolOutcome::err("stopped by user"));
                        continue;
                    }
                    run.calls += 1;
                    if let Some(note) = is_long(&call) {
                        run.held = Some(HeldCall { call, reply });
                        self.ai_panel.assistant.busy_note = Some(note.to_string());
                        break;
                    }
                    let outcome = run_call(self, &call);
                    let _ = reply.send(outcome);
                }
                BridgeMsg::Jobs { deliver, reply } => {
                    // The tasks are run by the frame loop; polling here too
                    // means a job that has just finished is seen at once, and
                    // a waiting run needs no frame to make progress.
                    self.agent_poll_tasks();
                    let _ = reply.send(agent::jobs::snapshot(&mut self.agent_jobs, deliver));
                }
                BridgeMsg::Confirm { call, reply } => {
                    run.confirm_reply = Some(reply);
                    self.ai_panel.assistant.confirm = Some(describe_call(&call));
                }
                BridgeMsg::Note(text) => self.ai_panel.assistant.push(StepKind::Note, text),
                BridgeMsg::Finished(result) => {
                    self.agent_end(Some(result));
                    return true;
                }
            }
        }
        did
    }

    fn agent_event(&mut self, event: AgentEvent) {
        let a = &mut self.ai_panel.assistant;
        match event {
            AgentEvent::Thinking(t) => a.push(StepKind::Thought, short(&t, 300)),
            AgentEvent::Call(c) => a.push(StepKind::Call, describe_call(&c)),
            AgentEvent::Result { ok, summary, .. } => {
                a.push(if ok { StepKind::Ok } else { StepKind::Failed }, summary)
            }
            AgentEvent::NeedsConfirm(c) => {
                a.push(StepKind::Note, format!("needs your confirmation: {}", describe_call(&c)))
            }
            AgentEvent::Reply(r) => a.reply = r,
            AgentEvent::Stopped(r) => a.push(StepKind::Note, format!("stopped: {r}")),
            AgentEvent::Error(e) => a.push(StepKind::Failed, format!("error: {e}")),
        }
    }

    /// The run is over (finished, stopped, failed or aborted): collapse its
    /// edits into one undo step and clear the running state.
    fn agent_end(&mut self, result: Option<AgentRunResult>) {
        let Some(mut run) = self.agent_bridge.run.take() else { return };
        self.agent_bridge.last_run = result.as_ref().map(|r| (r.calls, r.turns));
        run.stop.store(true, Ordering::Relaxed);
        if let Some(reply) = run.confirm_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(held) = run.held.take() {
            let _ = held.reply.send(ToolOutcome::err("stopped by user"));
        }
        // After a normal finish the worker is done; after an abort it may be
        // inside a request, so it is left to exit on its own.
        if result.is_some() {
            if let Some(h) = run.handle.take() {
                let _ = h.join();
            }
        }
        self.agent_editor_mut().doc.end_gesture();
        let collapsed = self.agent_editor_mut().doc.collapse_run(run.checkpoint, run.label.clone());
        self.agent_editor_mut().doc.end_gesture();
        let a = &mut self.ai_panel.assistant;
        a.running = false;
        a.busy_note = None;
        a.confirm = None;
        if !collapsed {
            a.push(StepKind::Note, "the document changed during the run, so it is not one undo step");
        }
        self.agent_editor_mut().status = Some(match &result {
            Some(r) => match &r.status {
                AgentStatus::Done => format!("Assistant done ({} calls)", r.calls),
                AgentStatus::Stopped(why) => format!("Assistant stopped: {why}"),
                AgentStatus::Error(e) => format!("Assistant failed: {e}"),
            },
            None => "Assistant stopped".to_string(),
        });
    }
}

// ------------------------------------------------- live-acceptance accessors

/// Public, behaviour-free accessors for `examples/agent_live.rs` (Stage G).
impl App {
    /// The label of the backend a run would use, or the guidance text (same
    /// selection as the AI panel's Assistant).
    pub fn live_backend(&self) -> Result<String, String> {
        choose_backend(&self.agent_editor().workspace.ai).map(|c| c.label)
    }

    /// Pick a local OpenAI-compatible model (LM Studio) for this app only, as
    /// choosing one in the AI panel would, without saving it — so a live run
    /// can be pointed at a local model without touching the user's settings.
    pub fn live_use_local(&mut self, base_url: &str, model: &str) {
        let ai = &mut self.agent_editor_mut().workspace.ai;
        ai.backend = Some(buzz_ai::llm::Backend::OpenAiCompatible { base_url: base_url.to_string() });
        ai.model = Some(model.to_string());
    }

    /// Keep the Assets library in `root` instead of the user's own folder, so a
    /// test that makes a character (which is saved to Cast) leaves no file behind.
    pub fn live_use_assets_at(&mut self, root: &std::path::Path) {
        self.agent_editor_mut().assets = buzz_doc::AssetLibrary::at(root);
    }

    /// Start a run exactly as the Assistant's Send button does.
    pub fn live_start(&mut self, request: String) {
        self.agent_start(request);
    }

    pub fn live_active(&self) -> bool {
        self.agent_active()
    }

    pub fn live_pump(&mut self) -> bool {
        self.agent_pump()
    }

    pub fn live_stop(&mut self) {
        self.agent_stop();
    }

    pub fn live_abort(&mut self, why: &str) {
        self.agent_abort(why);
    }

    pub fn live_confirm(&mut self, yes: bool) {
        self.agent_confirm(yes);
    }

    /// The pending confirmation's description, if the run is waiting on one.
    pub fn live_pending_confirm(&self) -> Option<String> {
        self.ai_panel.assistant.confirm.clone()
    }

    /// The step log so far as `(kind, text)`.
    pub fn live_log(&self) -> Vec<(String, String)> {
        self.ai_panel
            .assistant
            .log
            .iter()
            .map(|s| (format!("{:?}", s.kind).to_lowercase(), s.text.clone()))
            .collect()
    }

    /// The model's final reply of the last run.
    pub fn live_reply(&self) -> String {
        self.ai_panel.assistant.reply.clone()
    }

    /// Guidance shown when no backend was available.
    pub fn live_guidance(&self) -> Option<String> {
        self.ai_panel.assistant.guidance.clone()
    }

    /// (calls, turns) of the last finished run.
    pub fn live_last_run(&self) -> Option<(usize, usize)> {
        self.agent_bridge.last_run
    }

    /// A compact JSON picture of the document for the acceptance checks.
    pub fn live_state(&self) -> Value {
        let editor = self.agent_editor();
        let scene = editor.doc.scene();
        let hex = |c: peniko::Color| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            format!("#{r:02x}{g:02x}{b:02x}")
        };
        let layers: Vec<Value> = scene
            .layers()
            .iter()
            .map(|layer| {
                let keys: Vec<Value> = layer
                    .frames
                    .keyframes()
                    .iter()
                    .map(|k| {
                        let mut instances = 0usize;
                        let objects: Vec<Value> = k
                            .objects
                            .iter()
                            .map(|o| {
                                let at = o.transform * buzz_geom::Point::ZERO;
                                match &o.kind {
                                    buzz_scene::ObjectKind::Shape(s) => {
                                        use buzz_geom::Shape as _;
                                        let bb = s.path.bounding_box();
                                        let c = o.transform * buzz_geom::Point::new((bb.x0 + bb.x1) / 2.0, (bb.y0 + bb.y1) / 2.0);
                                        let (mut curves, mut lines) = (0usize, 0usize);
                                        for el in s.path.elements() {
                                            match el {
                                                buzz_geom::PathEl::CurveTo(..) | buzz_geom::PathEl::QuadTo(..) => curves += 1,
                                                buzz_geom::PathEl::LineTo(..) => lines += 1,
                                                _ => {}
                                            }
                                        }
                                        let (w, h) = (bb.width(), bb.height());
                                        let round = w > 1.0 && h > 1.0 && (w / h - 1.0).abs() < 0.2;
                                        json!({
                                            "kind": "shape",
                                            "oval": curves >= 4 && lines == 0 && round,
                                            "fill": s.fill.as_ref().map(|f| hex(f.paint.color())),
                                            "x": c.x,
                                            "y": c.y,
                                        })
                                    }
                                    other => {
                                        if matches!(other, buzz_scene::ObjectKind::Instance(_)) {
                                            instances += 1;
                                        }
                                        json!({"kind": "other", "x": at.x, "y": at.y})
                                    }
                                }
                            })
                            .collect();
                        json!({
                            "start": k.start,
                            "tween": format!("{:?}", k.tween.kind),
                            "sound": k.sound.is_some(),
                            "instances": instances,
                            "objects": objects,
                        })
                    })
                    .collect();
                json!({"name": layer.name, "keyframes": keys})
            })
            .collect();
        let lights: Vec<Value> = scene
            .lights()
            .lights
            .iter()
            .map(|l| {
                let kind = format!("{:?}", l.kind);
                let kind = kind.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("").to_lowercase();
                let (enabled, keys) = match &l.track {
                    Some(t) => (
                        t.enabled,
                        t.keys().iter().map(|k| json!({"frame": k.frame, "intensity": k.intensity})).collect::<Vec<_>>(),
                    ),
                    None => (false, Vec::new()),
                };
                json!({"name": l.name, "kind": kind, "intensity": l.intensity, "track_enabled": enabled, "keys": keys})
            })
            .collect();
        let history = editor.doc.history();
        json!({
            "layers": layers,
            "shapes": scene.shape_count(),
            "lights": lights,
            "sounds": scene.sounds().len(),
            "undo_depth": history.undo_depth(),
            "next_undo_label": history.next_undo_label(),
        })
    }
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ai::agent::AssistantTurn;
    use buzz_render::GpuPreference;
    use std::collections::VecDeque;

    struct Scripted {
        turns: VecDeque<AssistantTurn>,
        /// The second model turn waits for this (a mid-run Stop test).
        gate: Option<Arc<AtomicBool>>,
        served: usize,
    }

    impl ChatTransport for Scripted {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn accepts_images(&self) -> bool {
            false
        }
        fn complete(
            &mut self,
            _system: &str,
            _msgs: &[buzz_ai::agent::Msg],
            _tools: &[ToolSpec],
        ) -> Result<AssistantTurn, buzz_ai::llm::LlmError> {
            self.served += 1;
            if self.served == 2 {
                if let Some(gate) = &self.gate {
                    let start = std::time::Instant::now();
                    while !gate.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(20) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
            Ok(self.turns.pop_front().unwrap_or(AssistantTurn {
                text: "All done.".into(),
                calls: vec![],
                done: true,
                reply: "All done.".into(),
            }))
        }
    }

    fn call(n: usize, tool: &str, args: Value) -> ToolCall {
        ToolCall { id: format!("c{n}"), tool: tool.into(), args }
    }

    fn turn(calls: Vec<ToolCall>) -> AssistantTurn {
        AssistantTurn { text: "working".into(), calls, done: false, reply: String::new() }
    }

    fn source(turns: Vec<AssistantTurn>, gate: Option<Arc<AtomicBool>>) -> Source {
        Source::Custom(Box::new(move || {
            Box::new(Scripted { turns: turns.into(), gate, served: 0 }) as Box<dyn ChatTransport>
        }))
    }

    fn pump_until(app: &mut App, what: &str, mut done: impl FnMut(&App) -> bool) {
        let start = std::time::Instant::now();
        while !done(app) {
            app.agent_pump();
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn finish(app: &mut App) {
        pump_until(app, "the run to end", |a| !a.agent_active());
    }

    fn layers(app: &App) -> usize {
        app.agent_editor().doc.scene().layers().iter().count()
    }

    fn shapes(app: &App) -> usize {
        app.agent_editor().doc.scene().shape_count()
    }

    fn rectangle() -> Value {
        json!({"tool": "rectangle", "points": [{"x": 40.0, "y": 40.0}, {"x": 140.0, "y": 100.0}]})
    }

    #[test]
    fn allow_all_auto_approves_unlocks_excluded_commands_and_keeps_quit_blocked() {
        use std::sync::atomic::AtomicBool;
        // (2) confirm answers yes without any UI, and logs it.
        let (tx, rx) = unbounded();
        let mut host = ChannelHost {
            tools: vec![],
            tx,
            stop: Arc::new(AtomicBool::new(false)),
            allow_all: Arc::new(AtomicBool::new(true)),
        };
        assert!(host.confirm(&call(1, "command", json!({"name": "quit"}))));
        match rx.try_recv() {
            Ok(BridgeMsg::Note(n)) => assert!(n.contains("auto-approved (Allow all)"), "{n}"),
            _ => panic!("expected a log note"),
        }
        // (3) excluded commands: refused when off, run when on.
        let mut app = App::new(GpuPreference::default());
        let run = |app: &mut App, name: &str| {
            agent::execute(app, &AgentCall { tool: "command".into(), args: json!({"name": name}) })
        };
        let off = run(&mut app, "about");
        assert!(!off.ok && !app.agent_editor().about.open, "{off:?}");
        app.set_allow_all(true);
        assert!(app.agent_allow_all());
        let on = run(&mut app, "about");
        assert!(on.ok && app.agent_editor().about.open, "{on:?}");
        // The voice commands too (mute only closes the mic; Off mode's press just explains).
        assert!(run(&mut app, "voice_mute").ok && app.ai_panel.assistant.voice.muted);
        assert!(run(&mut app, "voice_push_to_talk").ok);
        // Quit and close stay excluded: this app has no unsaved-changes prompt to fall back on.
        let quit = run(&mut app, "quit");
        assert!(!quit.ok && !app.agent_editor().should_quit, "{quit:?}");
        assert!(!run(&mut app, "close").ok);
        // The catalog says so.
        let how = |cat: &Value, id: &str| cat.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["how"].clone();
        let cat = agent::catalog_with(true);
        assert_eq!(how(&cat, "about"), "allowed (Allow all)");
        assert_eq!(how(&cat, "voice_mute"), "allowed (Allow all)");
        assert_eq!(how(&cat, "quit"), "excluded");
        assert_eq!(how(&agent::catalog(), "about"), "excluded");
    }

    #[test]
    fn allow_all_runs_a_voice_transcript_at_once_and_off_only_fills_the_box() {
        let mut app = App::new(GpuPreference::default());
        // Off: the box is filled, nothing runs.
        app.agent_bridge.test_source = Some((source(vec![], None), "fake".into()));
        app.voice_handle(vec![crate::voice_bridge::testing::fill("add a red ball")]);
        assert_eq!(app.ai_panel.assistant.request, "add a red ball");
        assert!(!app.agent_active());
        // On: the same transcript starts a run and is logged.
        app.set_allow_all(true);
        app.voice_handle(vec![crate::voice_bridge::testing::fill("make a new layer")]);
        assert!(app.agent_active(), "the transcript started a run");
        assert!(app.ai_panel.assistant.log.iter().any(|s| s.text.contains("heard: make a new layer")));
        finish(&mut app);
    }

    #[test]
    fn a_run_is_one_undo_step_and_one_undo_restores_the_document() {
        let mut app = App::new(GpuPreference::default());
        let (layers0, shapes0) = (layers(&app), shapes(&app));
        let depth0 = app.agent_editor().doc.history().undo_depth();
        let background0 = agent::execute(
            &mut app,
            &AgentCall { tool: "inspect".into(), args: json!({"what": "properties", "target": "document"}) },
        )
        .value;

        let turns = vec![turn(vec![
            call(1, "command", json!({"name": "new_layer"})),
            call(2, "gesture", rectangle()),
            call(3, "set_property", json!({"target": "document", "field": "background", "value": "#336699"})),
        ])];
        app.agent_launch("Draw a box on a new layer and colour the stage".into(), source(turns, None), "fake".into());
        assert!(app.agent_active());
        finish(&mut app);

        assert_eq!(layers(&app), layers0 + 1);
        assert!(shapes(&app) > shapes0, "the rectangle drew something");
        let history = app.agent_editor().doc.history();
        assert_eq!(history.undo_depth(), depth0 + 1, "labels: {:?}", history.labels());
        let expected: String = "Draw a box on a new layer and colour the stage".chars().take(40).collect();
        assert_eq!(history.next_undo_label(), Some(format!("AI: {expected}").as_str()));
        let a = &app.ai_panel.assistant;
        assert!(!a.running && a.reply == "All done.", "{a:?}");
        assert_eq!(a.log.iter().filter(|s| s.kind == StepKind::Ok).count(), 3, "{:?}", a.log);

        let undo = agent::execute(&mut app, &AgentCall { tool: "command".into(), args: json!({"name": "undo"}) });
        assert!(undo.ok, "{:?}", undo.error);
        assert_eq!((layers(&app), shapes(&app)), (layers0, shapes0));
        let background = agent::execute(
            &mut app,
            &AgentCall { tool: "inspect".into(), args: json!({"what": "properties", "target": "document"}) },
        )
        .value;
        assert_eq!(background, background0, "one undo restores the whole document");
        assert_eq!(app.agent_editor().doc.history().undo_depth(), depth0);
    }

    #[test]
    fn stop_mid_run_leaves_one_undo_step_and_a_consistent_document() {
        let mut app = App::new(GpuPreference::default());
        let (layers0, shapes0) = (layers(&app), shapes(&app));
        let depth0 = app.agent_editor().doc.history().undo_depth();
        let gate = Arc::new(AtomicBool::new(false));
        let turns = vec![
            turn(vec![call(1, "command", json!({"name": "new_layer"})), call(2, "gesture", rectangle())]),
            // Never reached: the run is stopped before the second turn's calls.
            turn(vec![call(3, "command", json!({"name": "new_layer"})), call(4, "gesture", rectangle())]),
        ];
        app.agent_launch("stop me".into(), source(turns, Some(gate.clone())), "fake".into());
        pump_until(&mut app, "the first two results", |a| {
            a.ai_panel.assistant.log.iter().filter(|s| s.kind == StepKind::Ok).count() >= 2
        });
        app.agent_stop();
        gate.store(true, Ordering::Relaxed);
        finish(&mut app);

        assert_eq!(layers(&app), layers0 + 1);
        assert!(shapes(&app) > shapes0, "the first turn's rectangle drew something");
        assert_eq!(app.agent_editor().doc.history().undo_depth(), depth0 + 1);
        assert_eq!(app.agent_editor().doc.history().next_undo_label(), Some("AI: stop me"));
        assert!(app.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Note && s.text.contains("stopped")));
        assert!(!app.ai_panel.assistant.running);
        assert!(agent::execute(&mut app, &AgentCall { tool: "command".into(), args: json!({"name": "undo"}) }).ok);
        assert_eq!((layers(&app), shapes(&app)), (layers0, shapes0));
    }

    #[test]
    fn a_run_that_uses_undo_still_leaves_a_consistent_history() {
        let mut app = App::new(GpuPreference::default());
        app.dispatch(buzz_ui::Command::NewLayer);
        let (layers_before, depth_before) = (layers(&app), app.agent_editor().doc.history().undo_depth());
        assert!(depth_before >= 1);

        // The run undoes the user's own New Layer, then makes a different edit.
        let turns = vec![turn(vec![
            call(1, "command", json!({"name": "undo"})),
            call(2, "command", json!({"name": "new_layer"})),
            call(3, "command", json!({"name": "new_layer"})),
            call(4, "command", json!({"name": "redo"})),
        ])];
        app.agent_launch("undo then rebuild".into(), source(turns, None), "fake".into());
        finish(&mut app);

        assert_eq!(layers(&app), layers_before + 1, "undo, +1, +1, redo (nothing to redo)");
        let history = app.agent_editor().doc.history();
        assert_eq!(history.undo_depth(), depth_before + 1, "labels: {:?}", history.labels());
        assert!(!history.can_redo());
        assert_eq!(history.next_undo_label(), Some("AI: undo then rebuild"));

        let undo = || AgentCall { tool: "command".into(), args: json!({"name": "undo"}) };
        assert!(agent::execute(&mut app, &undo()).ok);
        assert_eq!(layers(&app), layers_before, "one undo restores the pre-run document");
        assert!(agent::execute(&mut app, &undo()).ok);
        assert_eq!(layers(&app), layers_before - 1, "and the user's earlier step is still there");
    }

    #[test]
    fn a_declined_confirmation_is_a_tool_result_and_writes_nothing() {
        let mut app = App::new(GpuPreference::default());
        let path = std::env::temp_dir().join(format!("buzz_agent_confirm_{}.buzz", std::process::id()));
        std::fs::write(&path, b"original").unwrap();
        let turns = vec![turn(vec![call(
            1,
            "command",
            json!({"name": "save_as", "path": path.to_string_lossy()}),
        )])];
        app.agent_launch("save over it".into(), source(turns, None), "fake".into());
        pump_until(&mut app, "the confirmation", |a| a.ai_panel.assistant.confirm.is_some());
        assert!(app.ai_panel.assistant.confirm.as_ref().unwrap().contains("save_as"));
        app.agent_confirm(false);
        finish(&mut app);

        assert_eq!(std::fs::read(&path).unwrap(), b"original", "nothing written");
        assert!(app.agent_editor().doc.path().is_none());
        let log = &app.ai_panel.assistant.log;
        assert!(
            log.iter().any(|s| s.kind == StepKind::Failed && s.text.contains("declined")),
            "{log:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn closing_the_app_aborts_the_run_and_still_collapses_it() {
        let mut app = App::new(GpuPreference::default());
        let depth0 = app.agent_editor().doc.history().undo_depth();
        let gate = Arc::new(AtomicBool::new(false));
        let turns = vec![
            turn(vec![call(1, "command", json!({"name": "new_layer"}))]),
            turn(vec![call(2, "command", json!({"name": "new_layer"}))]),
        ];
        app.agent_launch("abort me".into(), source(turns, Some(gate.clone())), "fake".into());
        pump_until(&mut app, "the first result", |a| {
            a.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Ok)
        });
        app.agent_abort("The app is closing.");
        gate.store(true, Ordering::Relaxed);
        assert!(!app.agent_active() && !app.ai_panel.assistant.running);
        assert_eq!(app.agent_editor().doc.history().undo_depth(), depth0 + 1);
        // The detached worker finds the app gone and ends; nothing else runs.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(app.agent_editor().doc.history().undo_depth(), depth0 + 1);
    }

    #[test]
    fn every_tool_schema_names_its_fields() {
        // A local server constrains arguments to the schema, so a tool whose
        // schema lists nothing can only ever be called with `{}`.
        for t in build_tools() {
            let props = t.parameters["properties"].as_object();
            assert!(
                props.is_some_and(|p| !p.is_empty()),
                "tool '{}' has no listed fields: {}",
                t.name,
                t.parameters
            );
        }
        let tools = build_tools();
        let style = tools.iter().find(|t| t.name == "set_style").expect("set_style");
        let listed: Vec<&str> = style.parameters["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for f in crate::agent::STYLE_FIELDS {
            assert!(listed.contains(f), "set_style schema is missing '{f}'");
        }
        assert_eq!(listed.len(), crate::agent::STYLE_FIELDS.len(), "{listed:?}");
    }

    #[test]
    fn backend_choice_follows_the_claude_switch_and_the_panels_local_model() {
        let mut settings = AiSettings::default();
        let none = choose_backend_from(false, false, "sonnet", &settings);
        assert!(none.err().unwrap().contains("Use Claude for the Assistant"));

        let no_key = choose_backend_from(true, false, "sonnet", &settings);
        assert!(no_key.err().unwrap().contains("no key"));

        let claude = choose_backend_from(true, true, "opus", &settings).ok().unwrap();
        assert!(claude.claude && claude.label == "Claude (opus)" && matches!(claude.backend, Backend::ClaudeApiServer { .. }));

        settings.backend = Some(Backend::default_openai());
        settings.model = Some("gemma".into());
        let local = choose_backend_from(false, true, "sonnet", &settings).ok().unwrap();
        assert!(!local.claude && local.model == "gemma" && local.label.ends_with(": gemma"));
        // The switch wins when it is on.
        assert!(choose_backend_from(true, true, "sonnet", &settings).ok().unwrap().claude);
        // Claude selected as the local backend while the switch is off is not used.
        settings.backend = Some(Backend::default_claude_api());
        assert!(choose_backend_from(false, true, "sonnet", &settings).is_err());
    }

    #[test]
    fn the_tool_list_covers_every_catalog_tool() {
        let tools = build_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        for (name, _) in agent::AGENT_TOOLS {
            assert!(names.contains(name), "tool {name} missing from {names:?}");
        }
        assert!(names.contains(&"command") && names.contains(&"inspect"));
        let command = tools.iter().find(|t| t.name == "command").unwrap();
        let first = command.description.lines().next().unwrap();
        assert!(first.contains("new_layer") && first.contains("undo"), "ids listed on the first line");
        assert!(!first.contains("quit,") && !first.contains(" quit"), "excluded commands are not offered");
        assert!(tools.iter().all(|t| t.parameters["type"] == "object"));
    }

    fn ui_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> egui::Rect {
        let input = egui::RawInput {
            events,
            modifiers,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            ..Default::default()
        };
        let mut stage = egui::Rect::NOTHING;
        let _ = ctx.run_ui(input, |ui| stage = app.build_ui(ui));
        stage
    }

    fn user_drag(app: &mut App, ctx: &egui::Context, stage: egui::Rect) {
        let a = stage.center() - egui::vec2(60.0, 40.0);
        let b = stage.center() + egui::vec2(60.0, 40.0);
        let m = egui::Modifiers::default();
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: m };
        ui_frame(app, ctx, vec![egui::Event::PointerMoved(a)], m);
        ui_frame(app, ctx, vec![button(a, true)], m);
        for k in 1..=4 {
            let p = a + (b - a) * (k as f32 / 4.0);
            ui_frame(app, ctx, vec![egui::Event::PointerMoved(p)], m);
        }
        ui_frame(app, ctx, vec![button(b, false)], m);
        ui_frame(app, ctx, vec![], m);
    }

    fn user_undo_key(app: &mut App, ctx: &egui::Context) {
        let key = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: Some(egui::Key::Z),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        ui_frame(app, ctx, vec![key], egui::Modifiers::COMMAND);
        ui_frame(app, ctx, vec![], egui::Modifiers::default());
    }

    #[test]
    fn user_input_is_blocked_while_the_assistant_runs_and_works_again_after() {
        let mut app = App::new(GpuPreference::default());
        app.recovery.found.clear();
        app.agent_editor_mut().set_tool(buzz_ui::ToolId::Rectangle);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let mut stage = ui_frame(&mut app, &ctx, vec![], egui::Modifiers::default());
        stage = ui_frame(&mut app, &ctx, vec![], egui::Modifiers::default()).union(stage);
        app.dispatch(buzz_ui::Command::NewLayer); // the user's own earlier step

        let gate = Arc::new(AtomicBool::new(false));
        let turns = vec![turn(vec![call(1, "command", json!({"name": "new_layer"}))]), turn(vec![])];
        app.agent_launch("hold".into(), source(turns, Some(gate.clone())), "fake".into());
        let start = std::time::Instant::now();
        while !app.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Ok) {
            ui_frame(&mut app, &ctx, vec![], egui::Modifiers::default());
            assert!(start.elapsed() < Duration::from_secs(30), "agent call never ran");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(app.agent_active(), "the run is held open by the gate");
        let (l0, s0, d0) = (layers(&app), shapes(&app), app.agent_editor().doc.history().undo_depth());

        user_drag(&mut app, &ctx, stage);
        user_undo_key(&mut app, &ctx);
        assert!(!app.user_command_allowed(buzz_ui::Command::Delete));
        assert!(app.user_command_allowed(buzz_ui::Command::ZoomIn));
        assert_eq!((layers(&app), shapes(&app)), (l0, s0), "no user edit reached the document");
        assert_eq!(app.agent_editor().doc.history().undo_depth(), d0);

        // The agent's own calls still apply while the user is locked out.
        let more = agent::execute(&mut app, &AgentCall { tool: "command".into(), args: json!({"name": "new_layer"}) });
        assert!(more.ok);
        assert_eq!(layers(&app), l0 + 1);

        gate.store(true, Ordering::Relaxed);
        let start = std::time::Instant::now();
        while app.agent_active() {
            ui_frame(&mut app, &ctx, vec![], egui::Modifiers::default());
            assert!(start.elapsed() < Duration::from_secs(30), "run never ended");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.agent_editor().doc.history().next_undo_label(), Some("AI: hold"));

        // After the run the same input works again.
        let (l1, d1) = (layers(&app), app.agent_editor().doc.history().undo_depth());
        user_undo_key(&mut app, &ctx);
        assert!(app.agent_editor().doc.history().undo_depth() < d1, "undo works after the run");
        assert!(layers(&app) < l1);
        let s1 = shapes(&app);
        user_drag(&mut app, &ctx, stage);
        assert!(shapes(&app) > s1, "a stage drag draws after the run");
    }

    // ---- H3: wait_for_jobs ------------------------------------------------

    /// Run the worker's `wait_for_jobs` on a thread and answer its polls from
    /// this one, as `agent_pump` does while a run is active.
    fn wait_serving(app: &mut App, timeout: Duration, stop: Arc<AtomicBool>) -> ToolOutcome {
        let (tx, rx) = unbounded();
        let worker = std::thread::spawn(move || wait_for_jobs(&tx, &stop, timeout, Duration::from_millis(10)));
        let started = std::time::Instant::now();
        while !worker.is_finished() {
            if let Ok(BridgeMsg::Jobs { deliver, reply }) = rx.recv_timeout(Duration::from_millis(5)) {
                app.agent_poll_tasks();
                let _ = reply.send(agent::jobs::snapshot(&mut app.agent_jobs, deliver));
            }
            assert!(started.elapsed() < Duration::from_secs(30), "wait_for_jobs never returned");
        }
        worker.join().unwrap()
    }

    fn job_of<'a>(outcome: &'a ToolOutcome, id: u64) -> &'a Value {
        outcome.value["jobs"].as_array().unwrap().iter().find(|j| j["id"] == id).expect("the job is listed")
    }

    #[test]
    fn wait_for_jobs_returns_done_and_failed_at_once_and_gives_a_result_only_once() {
        use crate::tasks::TaskOutcome;
        let mut app = App::new(GpuPreference::default());
        let ok = app.test_stub_job("stub_ok", |_| TaskOutcome::Finished("built it".into()));
        let bad = app.test_stub_job("stub_bad", |_| TaskOutcome::Failed("the model is missing".into()));
        let started = std::time::Instant::now();
        let first = wait_serving(&mut app, Duration::from_secs(20), Arc::new(AtomicBool::new(false)));
        assert!(first.ok, "{first:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "returns as soon as nothing runs");
        assert_eq!(first.value["running"], 0);
        assert_eq!(first.value["timed_out"], false);
        let (done, failed) = (job_of(&first, ok), job_of(&first, bad));
        assert_eq!((done["status"].clone(), done["kind"].clone()), (json!("done"), json!("stub_ok")));
        assert_eq!(done["result"]["status"], "built it");
        assert_eq!((failed["status"].clone(), failed["error"].clone()), (json!("failed"), json!("the model is missing")));
        // Asked again, the finished result is not repeated; the failure still is.
        let second = wait_serving(&mut app, Duration::from_secs(20), Arc::new(AtomicBool::new(false)));
        assert_eq!(job_of(&second, ok)["result_delivered"], true);
        assert!(job_of(&second, ok).get("result").is_none());
        assert_eq!(job_of(&second, bad)["error"], "the model is missing");
    }

    #[test]
    fn wait_for_jobs_times_out_with_the_running_job_and_sees_it_finish_later() {
        use crate::tasks::TaskOutcome;
        let mut app = App::new(GpuPreference::default());
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        let slow = app.test_stub_job("stub_slow", move |_| {
            let start = std::time::Instant::now();
            while !gate.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(5));
            }
            TaskOutcome::Finished("late".into())
        });
        let started = std::time::Instant::now();
        let timed_out = wait_serving(&mut app, Duration::from_millis(300), Arc::new(AtomicBool::new(false)));
        assert!(started.elapsed() >= Duration::from_millis(300) && started.elapsed() < Duration::from_secs(5));
        assert_eq!((timed_out.value["timed_out"].clone(), timed_out.value["running"].clone()), (json!(true), json!(1)));
        assert_eq!(job_of(&timed_out, slow)["status"], "running");
        release.store(true, Ordering::Relaxed);
        let later = wait_serving(&mut app, Duration::from_secs(20), Arc::new(AtomicBool::new(false)));
        assert_eq!((later.value["timed_out"].clone(), job_of(&later, slow)["status"].clone()), (json!(false), json!("done")));
    }

    #[test]
    fn wait_for_jobs_with_no_jobs_says_so_and_a_bad_timeout_is_refused() {
        let mut app = App::new(GpuPreference::default());
        let none = wait_serving(&mut app, Duration::from_secs(5), Arc::new(AtomicBool::new(false)));
        assert!(none.ok && none.value["jobs"].as_array().unwrap().is_empty() && none.value["note"].is_string());
        let (tx, _rx) = unbounded();
        let mut host = ChannelHost {
            tools: vec![],
            tx,
            stop: Arc::new(AtomicBool::new(false)),
            allow_all: Arc::new(AtomicBool::new(false)),
        };
        let refused = host.execute(&call(1, "wait_for_jobs", json!({"timeout_s": 999})));
        assert!(!refused.ok && refused.error.unwrap().contains("1 to 300"));
    }

    #[test]
    fn stop_interrupts_wait_for_jobs_promptly() {
        use crate::tasks::TaskOutcome;
        let mut app = App::new(GpuPreference::default());
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        app.test_stub_job("stub_forever", move |_| {
            let start = std::time::Instant::now();
            while !gate.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(5));
            }
            TaskOutcome::Finished(String::new())
        });
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let outcome = wait_serving(&mut app, Duration::from_secs(300), stop);
        assert!(!outcome.ok && outcome.error.as_deref() == Some("stopped by user"), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
        release.store(true, Ordering::Relaxed);
    }

    /// The whole path: the model calls `wait_for_jobs` in a real run, the UI
    /// thread keeps pumping, and the Stop button ends the wait.
    #[test]
    fn a_run_waits_for_a_job_and_stop_ends_the_wait() {
        use crate::tasks::TaskOutcome;
        let mut app = App::new(GpuPreference::default());
        let gate = Arc::new(AtomicBool::new(false));
        let turns = vec![
            turn(vec![call(1, "command", json!({"name": "new_layer"}))]),
            turn(vec![call(2, "wait_for_jobs", json!({"timeout_s": 300}))]),
        ];
        app.agent_launch("build and wait".into(), source(turns, Some(gate.clone())), "fake".into());
        pump_until(&mut app, "the first result", |a| a.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Ok));
        // The job starts once the run is under way (a launch clears the old ones).
        let release = Arc::new(AtomicBool::new(false));
        let hold = release.clone();
        app.test_stub_job("stub_forever", move |_| {
            let start = std::time::Instant::now();
            while !hold.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(5));
            }
            TaskOutcome::Finished(String::new())
        });
        assert_eq!(app.agent_jobs.len(), 1);
        gate.store(true, Ordering::Relaxed);
        pump_until(&mut app, "the wait to begin", |a| {
            a.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Call && s.text.contains("wait_for_jobs"))
        });
        // Keep pumping so the worker's polls are answered: it is waiting, not done.
        let waited = std::time::Instant::now();
        while waited.elapsed() < Duration::from_millis(400) {
            app.agent_pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.agent_active(), "the run is still waiting on the job");
        let stopped = std::time::Instant::now();
        app.agent_stop();
        finish(&mut app);
        assert!(stopped.elapsed() < Duration::from_secs(3), "took {:?}", stopped.elapsed());
        assert!(app.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Failed && s.text.contains("stopped by user")));
        release.store(true, Ordering::Relaxed);
    }

    /// A run whose job ends while it waits gets the result in the tool answer.
    #[test]
    fn a_run_gets_a_finished_jobs_result_from_wait_for_jobs() {
        use crate::tasks::TaskOutcome;
        let mut app = App::new(GpuPreference::default());
        let gate = Arc::new(AtomicBool::new(false));
        let turns = vec![
            turn(vec![call(1, "command", json!({"name": "new_layer"}))]),
            turn(vec![call(2, "wait_for_jobs", json!({"timeout_s": 30}))]),
        ];
        app.agent_launch("wait for it".into(), source(turns, Some(gate.clone())), "fake".into());
        pump_until(&mut app, "the first result", |a| a.ai_panel.assistant.log.iter().any(|s| s.kind == StepKind::Ok));
        app.test_stub_job("stub_short", |_| {
            std::thread::sleep(Duration::from_millis(300));
            TaskOutcome::Finished("all built".into())
        });
        gate.store(true, Ordering::Relaxed);
        finish(&mut app);
        let log = &app.ai_panel.assistant.log;
        let answer = log.iter().filter(|s| s.kind == StepKind::Ok).nth(1).expect("the wait answered");
        assert!(answer.text.contains("\"done\""), "{answer:?}");
        assert_eq!(app.agent_jobs[0].state, agent::jobs::JobState::Done(json!({"status": "all built"})));
    }

    #[test]
    fn stop_interrupts_a_running_script_promptly() {
        let mut app = App::new(GpuPreference::default());
        let depth0 = app.agent_editor().doc.history().undo_depth();
        let turns = vec![turn(vec![
            call(1, "command", json!({"name": "new_layer"})),
            call(2, "script", json!({"code": "while(true){}"})),
        ])];
        app.agent_launch("loop".into(), source(turns, None), "fake".into());
        let flag = app.agent_stop_flag().expect("run is active");
        // The script is held one frame so its note paints; Stop lands while it spins.
        pump_until(&mut app, "the script to be held", |a| a.ai_panel.assistant.busy_note.is_some());
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            flag.store(true, Ordering::Relaxed);
        });
        finish(&mut app);
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        let log = &app.ai_panel.assistant.log;
        assert!(log.iter().any(|s| s.kind == StepKind::Note && s.text.contains("stopped")), "{log:?}");
        assert_eq!(app.agent_editor().doc.history().undo_depth(), depth0 + 1);
        assert_eq!(app.agent_editor().doc.history().next_undo_label(), Some("AI: loop"));
    }
}
