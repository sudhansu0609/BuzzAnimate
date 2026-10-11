//! Backend-agnostic agent loop (Stage E1 of docs/AI_AGENT_PLAN.md).
//!
//! The loop talks to the LLM through [`ChatTransport`] (native tool calling or the
//! JSON fallback) and to the app only through [`AgentHost`]. No UI types here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::llm::{
    low_reasoning, Backend, Llm, LlmError, LLM_CONNECT_TIMEOUT, LLM_REQUEST_TIMEOUT,
};

// ---------------------------------------------------------------- host side

/// One tool the model may call (name + JSON-schema parameters).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// One call the model made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub tool: String,
    pub args: Value,
}

/// What the app returns for a call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutcome {
    pub ok: bool,
    pub value: Value,
    pub error: Option<String>,
    pub image_png: Option<Vec<u8>>,
}

impl ToolOutcome {
    pub fn ok(value: Value) -> Self {
        Self { ok: true, value, error: None, image_png: None }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, value: Value::Null, error: Some(msg.into()), image_png: None }
    }
}

/// The app side of the loop. The app implements it (via a channel to the UI thread).
pub trait AgentHost: Send {
    fn tools(&self) -> Vec<ToolSpec>;
    fn execute(&mut self, call: &ToolCall) -> ToolOutcome;
    /// Asks the user yes/no for a call that needs confirmation; blocks until answered.
    fn confirm(&mut self, call: &ToolCall) -> bool;
}

// ------------------------------------------------------------ limits/events

#[derive(Clone, Debug)]
pub struct AgentLimits {
    pub max_calls: usize,
    pub max_turns: usize,
    pub max_calls_per_turn: usize,
    pub result_budget_chars: usize,
    /// Use native tool calling on backends other than the Claude proxy
    /// (the app sets it from [`probe_native_tools`]).
    pub native_tools: bool,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_calls: 60,
            // Local models make one call per turn, so a cap of 15 stopped
            // every live LM Studio run before it could use a quarter of its
            // call budget. Claude batches calls and never reached it. The
            // call cap is what bounds a run; this only stops a model that
            // keeps talking without calling anything.
            max_turns: 40,
            max_calls_per_turn: 8,
            result_budget_chars: 4000,
            native_tools: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    Thinking(String),
    Call(ToolCall),
    Result { id: String, ok: bool, summary: String },
    NeedsConfirm(ToolCall),
    Reply(String),
    Stopped(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Done,
    Stopped(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunResult {
    pub status: AgentStatus,
    pub reply: String,
    pub calls: usize,
    pub turns: usize,
}

// ------------------------------------------------------------- conversation

/// Transport-neutral conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    ToolResult { call_id: String, tool: String, content: String, image_png: Option<Vec<u8>> },
}

/// One model turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssistantTurn {
    /// Free text: the model's thought, or its final reply when it made no calls.
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// JSON protocol's `done` flag (native: true iff there are no calls).
    pub done: bool,
    /// JSON protocol's `reply` (final answer).
    pub reply: String,
}

pub trait ChatTransport {
    fn name(&self) -> &'static str;
    /// True if tool results may carry images for this backend.
    fn accepts_images(&self) -> bool;
    fn complete(&mut self, system: &str, msgs: &[Msg], tools: &[ToolSpec]) -> Result<AssistantTurn, LlmError>;
}

/// ClaudeApiServer -> Native; others -> Json unless `native` says otherwise.
pub fn transport_for(backend: &Backend, model: &str, native: bool) -> Box<dyn ChatTransport> {
    if matches!(backend, Backend::ClaudeApiServer { .. }) || native {
        Box::new(NativeTools { backend: backend.clone(), model: model.to_string() })
    } else {
        Box::new(JsonProtocol { backend: backend.clone(), model: model.to_string() })
    }
}

fn backend_takes_images(backend: &Backend) -> bool {
    matches!(backend, Backend::ClaudeApiServer { .. })
}

fn effective_model(backend: &Backend, model: &str) -> String {
    if matches!(backend, Backend::ClaudeApiServer { .. }) && model.trim().is_empty() {
        crate::ai_config::claude_model()
    } else {
        model.to_string()
    }
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(LLM_CONNECT_TIMEOUT)
        .timeout(LLM_REQUEST_TIMEOUT)
        .build()
}

fn post_json(backend: &Backend, url: &str, payload: Value, timeout: Option<Duration>) -> Result<Value, LlmError> {
    let agent = match timeout {
        Some(t) => ureq::AgentBuilder::new().timeout_connect(LLM_CONNECT_TIMEOUT).timeout(t).build(),
        None => http_agent(),
    };
    let resp = backend
        .authorize(agent.post(url))
        .send_json(payload)
        .map_err(crate::llm::map_ureq_error)?;
    resp.into_json::<Value>()
        .map_err(|e| LlmError::InvalidJson { raw: String::new(), error: e.to_string() })
}

// ------------------------------------------------------------ native tools

pub struct NativeTools {
    backend: Backend,
    model: String,
}

fn tools_json(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                json!({"type":"function","function":{
                    "name": t.name, "description": t.description,
                    "parameters": strict_parameters(&t.parameters)}})
            })
            .collect(),
    )
}

/// A tool's parameters as strict OpenAI-compatible servers insist on them:
/// an object schema that always carries `properties`.
///
/// A tool that takes free-form arguments describes them as a bare
/// `{"type":"object"}`. JSON Schema allows that and Claude accepts it, but
/// LM Studio validates every tool and rejects the **whole request** with
/// HTTP 400 when one is missing `properties` — so one loose schema left the
/// local Assistant unable to do anything at all.
fn strict_parameters(parameters: &Value) -> Value {
    let mut p = match parameters {
        Value::Object(_) => parameters.clone(),
        _ => json!({"type": "object"}),
    };
    if let Value::Object(map) = &mut p {
        map.entry("type").or_insert_with(|| json!("object"));
        map.entry("properties").or_insert_with(|| json!({}));
    }
    p
}

fn image_part(png: &[u8]) -> Value {
    json!({"type":"image_url","image_url":{"url": format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png))}})
}

/// OpenAI-style request body (LM Studio / Claude proxy). Pure; tested without HTTP.
/// Tool results are `role:"tool"` with `tool_call_id`; images (when `images` is true)
/// follow as a user message after the tool messages, since tool content is text only.
pub fn build_openai_request(model: &str, system: &str, msgs: &[Msg], tools: &[ToolSpec], images: bool) -> Value {
    let mut messages = vec![json!({"role":"system","content":system})];
    let mut pending: Vec<Value> = Vec::new();
    let flush = |messages: &mut Vec<Value>, pending: &mut Vec<Value>| {
        if !pending.is_empty() {
            let mut parts = vec![json!({"type":"text","text":"Snapshot(s) from the tool call(s) above."})];
            parts.append(pending);
            messages.push(json!({"role":"user","content":parts}));
        }
    };
    for m in msgs {
        match m {
            Msg::ToolResult { call_id, content, image_png, .. } => {
                messages.push(json!({"role":"tool","tool_call_id":call_id,"content":content}));
                if let (true, Some(png)) = (images, image_png) {
                    pending.push(image_part(png));
                }
            }
            other => {
                flush(&mut messages, &mut pending);
                match other {
                    Msg::User(t) => messages.push(json!({"role":"user","content":t})),
                    Msg::Assistant { text, calls } => {
                        let mut a = json!({"role":"assistant",
                            "content": if text.is_empty() { Value::Null } else { json!(text) }});
                        if !calls.is_empty() {
                            a["tool_calls"] = Value::Array(
                                calls
                                    .iter()
                                    .map(|c| {
                                        json!({"id":c.id,"type":"function","function":{
                                            "name":c.tool,"arguments":c.args.to_string()}})
                                    })
                                    .collect(),
                            );
                        }
                        messages.push(a);
                    }
                    Msg::ToolResult { .. } => unreachable!(),
                }
            }
        }
    }
    flush(&mut messages, &mut pending);
    let mut body = json!({"model":model,"messages":messages});
    if !tools.is_empty() {
        body["tools"] = tools_json(tools);
    }
    body
}

/// Ollama `/api/chat` request body with the `tools` field.
pub fn build_ollama_request(model: &str, system: &str, msgs: &[Msg], tools: &[ToolSpec], images: bool) -> Value {
    let mut messages = vec![json!({"role":"system","content":system})];
    for m in msgs {
        match m {
            Msg::User(t) => messages.push(json!({"role":"user","content":t})),
            Msg::Assistant { text, calls } => {
                let mut a = json!({"role":"assistant","content":text});
                if !calls.is_empty() {
                    a["tool_calls"] = Value::Array(
                        calls
                            .iter()
                            .map(|c| json!({"function":{"name":c.tool,"arguments":c.args}}))
                            .collect(),
                    );
                }
                messages.push(a);
            }
            Msg::ToolResult { tool, content, image_png, .. } => {
                messages.push(json!({"role":"tool","tool_name":tool,"content":content}));
                if let (true, Some(png)) = (images, image_png) {
                    messages.push(json!({"role":"user","content":"Snapshot from the tool call above.",
                        "images":[BASE64_STANDARD.encode(png)]}));
                }
            }
        }
    }
    let mut body = json!({"model":model,"stream":false,"messages":messages});
    if !tools.is_empty() {
        body["tools"] = tools_json(tools);
    }
    body
}

fn parse_args_string(s: &str) -> Value {
    if s.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({"_invalid_arguments": s}))
}

/// Parses `choices[0].message` of an OpenAI-style reply.
pub fn parse_openai_response(v: &Value, id_base: usize) -> Result<AssistantTurn, LlmError> {
    let msg = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")).ok_or(LlmError::EmptyResponse)?;
    let text = msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let mut calls = Vec::new();
    if let Some(list) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, c) in list.iter().enumerate() {
            let f = &c["function"];
            let Some(name) = f.get("name").and_then(|n| n.as_str()) else { continue };
            let args = match &f["arguments"] {
                Value::String(s) => parse_args_string(s),
                Value::Null => json!({}),
                other => other.clone(),
            };
            let id = c
                .get("id")
                .and_then(|i| i.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{}", id_base + i));
            calls.push(ToolCall { id, tool: name.to_string(), args });
        }
    }
    if calls.is_empty() && text.trim().is_empty() {
        return Err(LlmError::EmptyResponse);
    }
    let done = calls.is_empty();
    Ok(AssistantTurn { reply: if done { text.clone() } else { String::new() }, text, calls, done })
}

/// Parses Ollama's `message` (arguments are already objects, no ids).
pub fn parse_ollama_response(v: &Value, id_base: usize) -> Result<AssistantTurn, LlmError> {
    let msg = v.get("message").ok_or(LlmError::EmptyResponse)?;
    let text = msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let mut calls = Vec::new();
    if let Some(list) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, c) in list.iter().enumerate() {
            let f = &c["function"];
            let Some(name) = f.get("name").and_then(|n| n.as_str()) else { continue };
            let args = match &f["arguments"] {
                Value::String(s) => parse_args_string(s),
                Value::Null => json!({}),
                other => other.clone(),
            };
            calls.push(ToolCall { id: format!("call_{}", id_base + i), tool: name.to_string(), args });
        }
    }
    if calls.is_empty() && text.trim().is_empty() {
        return Err(LlmError::EmptyResponse);
    }
    let done = calls.is_empty();
    Ok(AssistantTurn { reply: if done { text.clone() } else { String::new() }, text, calls, done })
}

fn count_calls(msgs: &[Msg]) -> usize {
    msgs.iter().map(|m| if let Msg::Assistant { calls, .. } = m { calls.len() } else { 0 }).sum()
}

impl ChatTransport for NativeTools {
    fn name(&self) -> &'static str {
        "native tools"
    }
    fn accepts_images(&self) -> bool {
        backend_takes_images(&self.backend)
    }
    fn complete(&mut self, system: &str, msgs: &[Msg], tools: &[ToolSpec]) -> Result<AssistantTurn, LlmError> {
        let model = effective_model(&self.backend, &self.model);
        let images = self.accepts_images();
        let id_base = count_calls(msgs);
        match &self.backend {
            Backend::Ollama { .. } => {
                let mut body = build_ollama_request(&model, system, msgs, tools, images);
                if low_reasoning() {
                    body["think"] = json!(false);
                }
                let url = format!("{}/api/chat", self.backend.base_url());
                parse_ollama_response(&post_json(&self.backend, &url, body, None)?, id_base)
            }
            b => {
                let mut body = build_openai_request(&model, system, msgs, tools, images);
                if matches!(b, Backend::ClaudeApiServer { .. }) {
                    // The proxy's own default is "low"; ask for the effort
                    // the user chose for Claude instead.
                    body["reasoning_effort"] = json!(crate::ai_config::CLAUDE_DEFAULT_EFFORT);
                } else if low_reasoning() {
                    body["reasoning_effort"] = json!("none");
                }
                let url = format!("{}/chat/completions", b.base_url());
                parse_openai_response(&post_json(b, &url, body, None)?, id_base)
            }
        }
    }
}

/// One tiny tools request; true iff a tool_call comes back.
pub fn probe_native_tools(backend: &Backend, model: &str) -> bool {
    let tools = [ToolSpec {
        name: "ping".into(),
        description: "Call this tool to say hello.".into(),
        parameters: json!({"type":"object","properties":{"word":{"type":"string"}}}),
    }];
    let msgs = [Msg::User("Call the ping tool now with word \"hi\". Do not answer in text.".into())];
    let model = effective_model(backend, model);
    let (url, body, ollama) = match backend {
        Backend::Ollama { .. } => (
            format!("{}/api/chat", backend.base_url()),
            build_ollama_request(&model, "You use tools.", &msgs, &tools, false),
            true,
        ),
        b => (
            format!("{}/chat/completions", b.base_url()),
            build_openai_request(&model, "You use tools.", &msgs, &tools, false),
            false,
        ),
    };
    let Ok(v) = post_json(backend, &url, body, Some(Duration::from_secs(60))) else { return false };
    let turn = if ollama { parse_ollama_response(&v, 0) } else { parse_openai_response(&v, 0) };
    matches!(turn, Ok(t) if !t.calls.is_empty())
}

// ------------------------------------------------------------ JSON protocol

pub struct JsonProtocol {
    backend: Backend,
    model: String,
}

#[derive(Deserialize, Default)]
struct JsonCall {
    tool: String,
    #[serde(default)]
    args: Value,
}

#[derive(Deserialize, Default)]
struct JsonTurn {
    #[serde(default)]
    thought: String,
    #[serde(default)]
    calls: Vec<JsonCall>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    reply: String,
}

pub fn json_protocol_schema() -> Value {
    json!({
        "type":"object",
        "properties":{
            "thought":{"type":"string"},
            "calls":{"type":"array","items":{"type":"object",
                "properties":{"tool":{"type":"string"},"args":{"type":"object"}},
                "required":["tool","args"]}},
            "done":{"type":"boolean"},
            "reply":{"type":"string"}
        },
        "required":["thought","calls","done","reply"]
    })
}

fn turn_from_json(t: JsonTurn, id_base: usize) -> AssistantTurn {
    let calls: Vec<ToolCall> = t
        .calls
        .into_iter()
        .enumerate()
        .map(|(i, c)| ToolCall {
            id: format!("call_{}", id_base + i),
            tool: c.tool,
            args: if c.args.is_null() { json!({}) } else { c.args },
        })
        .collect();
    let done = t.done || calls.is_empty();
    AssistantTurn { text: t.thought, calls, done, reply: t.reply }
}

/// Parses one raw JSON-protocol reply (fences/prose tolerated via `extract_json`).
pub fn parse_json_turn(raw: &str, id_base: usize) -> Result<AssistantTurn, String> {
    let cleaned = crate::llm::extract_json(raw);
    serde_json::from_str::<JsonTurn>(cleaned)
        .map(|t| turn_from_json(t, id_base))
        .map_err(|e| e.to_string())
}

/// Parses `first`; on failure asks `retry` once (with the parse error) and parses that.
pub fn parse_with_repair(
    first: &str,
    id_base: usize,
    retry: &mut dyn FnMut(&str) -> Result<String, LlmError>,
) -> Result<AssistantTurn, LlmError> {
    match parse_json_turn(first, id_base) {
        Ok(t) => Ok(t),
        Err(e) => {
            let raw = retry(&e)?;
            parse_json_turn(&raw, id_base).map_err(|error| LlmError::InvalidJson { raw, error })
        }
    }
}

/// Flattens the conversation into one user message for the JSON path.
pub fn json_transcript(msgs: &[Msg]) -> String {
    let mut out = String::new();
    for m in msgs {
        match m {
            Msg::User(t) => {
                out.push_str(t);
                out.push_str("\n\n");
            }
            Msg::Assistant { text, calls } => {
                out.push_str(&format!("[You said: {text}]\n"));
                for c in calls {
                    out.push_str(&format!("[You called {} {}]\n", c.tool, c.args));
                }
            }
            Msg::ToolResult { tool, content, .. } => {
                out.push_str(&format!("[Result of {tool}: {content}]\n"));
            }
        }
    }
    out.push_str("\nNow reply with the next JSON step.");
    out
}

impl ChatTransport for JsonProtocol {
    fn name(&self) -> &'static str {
        "json protocol"
    }
    fn accepts_images(&self) -> bool {
        backend_takes_images(&self.backend)
    }
    fn complete(&mut self, system: &str, msgs: &[Msg], _tools: &[ToolSpec]) -> Result<AssistantTurn, LlmError> {
        let model = effective_model(&self.backend, &self.model);
        let user = json_transcript(msgs);
        let image = if self.accepts_images() {
            msgs.iter().rev().find_map(|m| match m {
                Msg::ToolResult { image_png: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
        } else {
            None
        };
        let imgs: Vec<&[u8]> = image.iter().map(|p| p.as_slice()).collect();
        // chat_json already does the schema prompt, extract_json and one repair retry.
        let t: JsonTurn = self.backend.chat_json(&model, system, &user, &imgs, json_protocol_schema())?;
        Ok(turn_from_json(t, count_calls(msgs)))
    }
}

// ------------------------------------------------------------ system prompt

pub fn system_prompt(tools: &[ToolSpec], json_protocol: bool) -> String {
    let mut s = String::from(
        "You operate BuzzAnimate, a 2D animation program, exactly like the user would: with its commands, \
mouse-tool gestures, style settings, scripts and property edits. Prefer `inspect` first to see the document, \
timeline and selection before changing anything. Use `snapshot` to check visual results when you can see images. \
Never invent command ids: use only names from the catalog (`inspect` with what=catalog lists commands). \
Make small steps, check results, and finish with a short plain-text reply saying what you did. \
If a call fails, read the error and try another route.\n\nTools:\n",
    );
    for t in tools {
        let first = t.description.lines().next().unwrap_or("");
        let keys: Vec<&str> = t
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|o| o.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        s.push_str(&format!("- {}({}): {}\n", t.name, keys.join(", "), first));
    }
    if json_protocol {
        s.push_str(
            "\nReply with ONE JSON object per step: {\"thought\":\"short\",\"calls\":[{\"tool\":\"<tool>\",\"args\":{...}}],\
\"done\":false,\"reply\":\"\"}. Up to 8 calls per step run in order. When finished set \"done\":true, \"calls\":[] and put your final answer in \"reply\".",
        );
    }
    s
}

// ------------------------------------------------------------ confirmation

fn effective_call_name(call: &ToolCall) -> String {
    if call.tool == "command" {
        call.args.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string()
    } else {
        call.tool.clone()
    }
}

/// Path argument of a call (`args.path` or `args.args.path`).
fn call_path(call: &ToolCall) -> Option<String> {
    call.args
        .get("path")
        .or_else(|| call.args.get("args").and_then(|a| a.get("path")))
        .and_then(|p| p.as_str())
        .map(str::to_string)
}

/// Rule 4: `quit`/`close`, or a file-writing command whose path already exists.
pub fn needs_confirmation(call: &ToolCall) -> bool {
    let name = effective_call_name(call);
    if name == "quit" || name == "close" {
        return true;
    }
    let writes = name == "save_as" || name.starts_with("export_");
    writes && call_path(call).is_some_and(|p| std::path::Path::new(&p).exists())
}

// ------------------------------------------------------------------- loop

/// Serialises an outcome for the model, truncated to `budget` chars with a marker.
pub fn result_json(outcome: &ToolOutcome, budget: usize) -> String {
    let v = json!({"ok":outcome.ok,"value":outcome.value,"error":outcome.error});
    truncate_chars(&v.to_string(), budget)
}

/// How much of a tool's result the model gets. `wait_for_jobs` carries a
/// finished job's result once (a transcript, say), which is long by nature and
/// cannot be fetched again, so it gets four times the usual room.
pub fn result_budget(call: &ToolCall, limits: &AgentLimits) -> usize {
    if call.tool == "wait_for_jobs" {
        limits.result_budget_chars.saturating_mul(4)
    } else {
        limits.result_budget_chars
    }
}

fn truncate_chars(s: &str, budget: usize) -> String {
    let n = s.chars().count();
    if n <= budget {
        return s.to_string();
    }
    let head: String = s.chars().take(budget).collect();
    format!("{head}...[truncated, {} more chars]", n - budget)
}

fn summarize(outcome: &ToolOutcome) -> String {
    let t = match (&outcome.error, outcome.ok) {
        (Some(e), _) => e.clone(),
        _ => outcome.value.to_string(),
    };
    let t: String = t.chars().take(120).collect();
    if outcome.image_png.is_some() { format!("{t} [image]") } else { t }
}

/// Runs the agent against a real backend.
pub fn run_agent(
    request: &str,
    backend: &Backend,
    model: &str,
    host: &mut dyn AgentHost,
    limits: AgentLimits,
    stop: &AtomicBool,
    on_event: &mut dyn FnMut(AgentEvent),
) -> AgentRunResult {
    let mut transport = transport_for(backend, model, limits.native_tools);
    run_agent_with(request, transport.as_mut(), host, limits, stop, on_event)
}

fn is_retryable(e: &LlmError) -> bool {
    match e {
        LlmError::Connection(_) | LlmError::Timeout(_) => true,
        LlmError::Http { status, .. } => *status >= 500,
        _ => false,
    }
}

/// The loop itself, over any transport (tested with a scripted fake).
pub fn run_agent_with(
    request: &str,
    transport: &mut dyn ChatTransport,
    host: &mut dyn AgentHost,
    limits: AgentLimits,
    stop: &AtomicBool,
    on_event: &mut dyn FnMut(AgentEvent),
) -> AgentRunResult {
    let tools = host.tools();
    let system = system_prompt(&tools, transport.name() == "json protocol");
    let mut msgs = vec![Msg::User(request.to_string())];
    let (mut calls_made, mut turns) = (0usize, 0usize);

    macro_rules! finish {
        ($status:expr, $reply:expr) => {{
            let status: AgentStatus = $status;
            match &status {
                AgentStatus::Stopped(r) => on_event(AgentEvent::Stopped(r.clone())),
                AgentStatus::Error(e) => on_event(AgentEvent::Error(e.clone())),
                AgentStatus::Done => {}
            }
            return AgentRunResult { status, reply: $reply, calls: calls_made, turns };
        }};
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            finish!(AgentStatus::Stopped("stopped by user".into()), String::new());
        }
        if turns >= limits.max_turns {
            finish!(AgentStatus::Stopped(format!("turn limit reached ({})", limits.max_turns)), String::new());
        }
        turns += 1;
        let mut result = transport.complete(&system, &msgs, &tools);
        if let Err(e) = &result {
            if is_retryable(e) && !stop.load(Ordering::Relaxed) {
                result = transport.complete(&system, &msgs, &tools);
            }
        }
        let turn = match result {
            Ok(t) => t,
            Err(e) => finish!(AgentStatus::Error(e.to_string()), String::new()),
        };

        if turn.calls.is_empty() {
            let reply = if turn.reply.trim().is_empty() { turn.text.clone() } else { turn.reply.clone() };
            on_event(AgentEvent::Reply(reply.clone()));
            finish!(AgentStatus::Done, reply);
        }
        if !turn.text.trim().is_empty() {
            on_event(AgentEvent::Thinking(turn.text.clone()));
        }

        msgs.push(Msg::Assistant { text: turn.text.clone(), calls: turn.calls.clone() });
        for (i, call) in turn.calls.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                finish!(AgentStatus::Stopped("stopped by user".into()), String::new());
            }
            if calls_made >= limits.max_calls {
                finish!(AgentStatus::Stopped(format!("call limit reached ({})", limits.max_calls)), String::new());
            }
            if i >= limits.max_calls_per_turn {
                // Every call id must still get an answer on the native path.
                let o = ToolOutcome::err(format!(
                    "skipped: at most {} calls per turn; repeat this call next turn",
                    limits.max_calls_per_turn
                ));
                msgs.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    content: result_json(&o, limits.result_budget_chars),
                    image_png: None,
                });
                continue;
            }
            calls_made += 1;
            on_event(AgentEvent::Call(call.clone()));
            let outcome = if needs_confirmation(call) {
                on_event(AgentEvent::NeedsConfirm(call.clone()));
                if host.confirm(call) {
                    host.execute(call)
                } else {
                    ToolOutcome::err("declined by user")
                }
            } else {
                host.execute(call)
            };
            on_event(AgentEvent::Result { id: call.id.clone(), ok: outcome.ok, summary: summarize(&outcome) });
            let mut content = result_json(&outcome, result_budget(call, &limits));
            let image = match outcome.image_png {
                Some(png) if transport.accepts_images() => Some(png),
                Some(_) => {
                    content.push_str(
                        " (A snapshot was taken but this model cannot see images; use `inspect` instead.)",
                    );
                    None
                }
                None => None,
            };
            msgs.push(Msg::ToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                content,
                image_png: image,
            });
        }
        if turn.done {
            let reply = if turn.reply.trim().is_empty() { turn.text.clone() } else { turn.reply.clone() };
            on_event(AgentEvent::Reply(reply.clone()));
            finish!(AgentStatus::Done, reply);
        }
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FakeTransport {
        script: VecDeque<Result<AssistantTurn, LlmError>>,
        images: bool,
        seen: Vec<Vec<Msg>>,
    }
    impl FakeTransport {
        fn new(script: Vec<Result<AssistantTurn, LlmError>>) -> Self {
            Self { script: script.into(), images: false, seen: vec![] }
        }
    }
    impl ChatTransport for FakeTransport {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn accepts_images(&self) -> bool {
            self.images
        }
        fn complete(&mut self, _s: &str, msgs: &[Msg], _t: &[ToolSpec]) -> Result<AssistantTurn, LlmError> {
            self.seen.push(msgs.to_vec());
            self.script.pop_front().unwrap_or(Err(LlmError::EmptyResponse))
        }
    }

    #[derive(Default)]
    struct FakeHost {
        executed: Vec<ToolCall>,
        confirmed: Vec<ToolCall>,
        answer: bool,
        outcome: Option<ToolOutcome>,
        stop_after: Option<(usize, std::sync::Arc<AtomicBool>)>,
    }
    impl AgentHost for FakeHost {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "command".into(),
                description: "Run a command by id.\nmore".into(),
                parameters: json!({"type":"object","properties":{"name":{"type":"string"},"args":{"type":"object"}}}),
            }]
        }
        fn execute(&mut self, call: &ToolCall) -> ToolOutcome {
            self.executed.push(call.clone());
            if let Some((n, flag)) = &self.stop_after {
                if self.executed.len() >= *n {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            self.outcome.clone().unwrap_or_else(|| ToolOutcome::ok(json!({"n": self.executed.len()})))
        }
        fn confirm(&mut self, call: &ToolCall) -> bool {
            self.confirmed.push(call.clone());
            self.answer
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), tool: "command".into(), args: json!({"name": name}) }
    }
    fn turn_calls(calls: Vec<ToolCall>) -> Result<AssistantTurn, LlmError> {
        Ok(AssistantTurn { text: "thinking".into(), calls, done: false, reply: String::new() })
    }
    fn turn_text(t: &str) -> Result<AssistantTurn, LlmError> {
        Ok(AssistantTurn { text: t.into(), calls: vec![], done: true, reply: t.into() })
    }
    fn run(
        t: &mut FakeTransport,
        host: &mut FakeHost,
        limits: AgentLimits,
        stop: &AtomicBool,
    ) -> (AgentRunResult, Vec<AgentEvent>) {
        let mut ev = Vec::new();
        let r = run_agent_with("do it", t, host, limits, stop, &mut |e| ev.push(e));
        (r, ev)
    }

    /// LM Studio rejected the whole request (HTTP 400, `parameters.properties
    /// Required`) because one tool took free-form arguments. Every tool sent
    /// must carry an object schema with `properties`.
    #[test]
    fn every_native_tool_schema_has_properties() {
        let loose = [
            json!({"type": "object"}),
            json!({}),
            json!(null),
            json!({"type": "object", "properties": {"a": {"type": "string"}}}),
        ];
        for p in loose {
            let s = strict_parameters(&p);
            assert_eq!(s["type"], "object", "{p}");
            assert!(s["properties"].is_object(), "{p}");
        }
        // Existing properties are kept, not replaced.
        let kept = strict_parameters(&json!({"type": "object", "properties": {"a": {"type": "string"}}}));
        assert_eq!(kept["properties"]["a"]["type"], "string");

        let tools = FakeHost::default().tools();
        let b = build_openai_request("m", "sys", &[Msg::User("hi".into())], &tools, true);
        for t in b["tools"].as_array().unwrap() {
            assert!(t["function"]["parameters"]["properties"].is_object(), "{t}");
        }
    }

    #[test]
    fn native_request_body_shape() {
        let tools = FakeHost::default().tools();
        let c = call("call_a", "new_layer");
        let msgs = vec![
            Msg::User("hi".into()),
            Msg::Assistant { text: String::new(), calls: vec![c.clone()] },
            Msg::ToolResult { call_id: "call_a".into(), tool: "command".into(), content: "{\"ok\":true}".into(), image_png: Some(vec![1, 2, 3]) },
        ];
        let b = build_openai_request("m", "sys", &msgs, &tools, true);
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["tools"][0]["function"]["name"], "command");
        assert!(b["tools"][0]["function"]["parameters"].is_object());
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["role"], "assistant");
        assert_eq!(m[2]["tool_calls"][0]["id"], "call_a");
        let args: Value = serde_json::from_str(m[2]["tool_calls"][0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["name"], "new_layer");
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "call_a");
        assert_eq!(m[4]["role"], "user");
        assert_eq!(m[4]["content"][1]["type"], "image_url");
        // without image support: no image message
        let b2 = build_openai_request("m", "sys", &msgs, &tools, false);
        assert_eq!(b2["messages"].as_array().unwrap().len(), 4);
        // ollama shape
        let o = build_ollama_request("m", "sys", &msgs, &tools, true);
        assert_eq!(o["stream"], false);
        assert_eq!(o["messages"][3]["role"], "tool");
        assert_eq!(o["messages"][2]["tool_calls"][0]["function"]["arguments"]["name"], "new_layer");
    }

    #[test]
    fn native_response_parsing() {
        let v = json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"abc","type":"function","function":{"name":"command","arguments":"{\"name\":\"undo\"}"}}]},
            "finish_reason":"tool_calls"}]});
        let t = parse_openai_response(&v, 0).unwrap();
        assert_eq!(t.calls[0].id, "abc");
        assert_eq!(t.calls[0].args["name"], "undo");
        assert!(!t.done);
        let v = json!({"choices":[{"message":{"content":"all done"}}]});
        let t = parse_openai_response(&v, 0).unwrap();
        assert!(t.done && t.reply == "all done");
        assert!(parse_openai_response(&json!({"choices":[{"message":{"content":""}}]}), 0).is_err());
        let o = parse_ollama_response(&json!({"message":{"content":"","tool_calls":[{"function":{"name":"inspect","arguments":{"what":"document"}}}]}}), 3).unwrap();
        assert_eq!(o.calls[0].id, "call_3");
        assert_eq!(o.calls[0].args["what"], "document");
    }

    #[test]
    fn json_protocol_parsing_and_repair() {
        let t = parse_json_turn("```json\n{\"thought\":\"t\",\"calls\":[{\"tool\":\"command\",\"args\":{\"name\":\"undo\"}}],\"done\":false,\"reply\":\"\"}\n```", 2).unwrap();
        assert_eq!(t.calls[0].id, "call_2");
        assert!(!t.done);
        let t = parse_json_turn("{\"done\":true,\"reply\":\"ok\"}", 0).unwrap();
        assert!(t.done && t.reply == "ok");
        assert!(parse_json_turn("not json at all", 0).is_err());

        let mut asked = 0;
        let t = parse_with_repair("{\"calls\": [oops", 0, &mut |err| {
            asked += 1;
            assert!(!err.is_empty());
            Ok("{\"thought\":\"fixed\",\"calls\":[],\"done\":true,\"reply\":\"r\"}".into())
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(t.reply, "r");
        // repair that is still broken fails after exactly one retry
        let mut asked = 0;
        let r = parse_with_repair("bad", 0, &mut |_| {
            asked += 1;
            Ok("still bad".into())
        });
        assert!(r.is_err() && asked == 1);
    }

    #[test]
    fn transport_selection() {
        let claude = Backend::ClaudeApiServer { base_url: "http://x/v1".into(), key: String::new() };
        assert_eq!(transport_for(&claude, "", false).name(), "native tools");
        assert_eq!(transport_for(&Backend::default_openai(), "m", false).name(), "json protocol");
        assert_eq!(transport_for(&Backend::default_openai(), "m", true).name(), "native tools");
        assert_eq!(transport_for(&Backend::default_ollama(), "m", false).name(), "json protocol");
    }

    #[test]
    fn multi_turn_run_ends_on_final_text() {
        let mut t = FakeTransport::new(vec![
            turn_calls(vec![call("1", "new_layer"), call("2", "insert_keyframe")]),
            turn_calls(vec![call("3", "undo")]),
            turn_text("made a layer"),
        ]);
        let mut h = FakeHost::default();
        let (r, ev) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!(r.status, AgentStatus::Done);
        assert_eq!(r.reply, "made a layer");
        assert_eq!((r.calls, r.turns), (3, 3));
        assert_eq!(h.executed.len(), 3);
        assert!(ev.contains(&AgentEvent::Reply("made a layer".into())));
        assert!(matches!(ev[0], AgentEvent::Thinking(_)));
        // the third request carried both prior tool results
        let last = &t.seen[2];
        assert_eq!(last.iter().filter(|m| matches!(m, Msg::ToolResult { .. })).count(), 3);
    }

    #[test]
    fn json_done_with_calls_executes_then_finishes() {
        let mut t = FakeTransport::new(vec![Ok(AssistantTurn {
            text: "x".into(),
            calls: vec![call("1", "new_layer")],
            done: true,
            reply: "bye".into(),
        })]);
        let mut h = FakeHost::default();
        let (r, _) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!((r.status, r.reply.as_str(), h.executed.len()), (AgentStatus::Done, "bye", 1));
    }

    #[test]
    fn max_calls_and_max_turns_stop() {
        let many: Vec<_> = (0..20).map(|_| turn_calls(vec![call("a", "undo")])).collect();
        let mut t = FakeTransport::new(many);
        let mut h = FakeHost::default();
        let lim = AgentLimits { max_calls: 3, ..Default::default() };
        let (r, ev) = run(&mut t, &mut h, lim, &AtomicBool::new(false));
        assert!(matches!(r.status, AgentStatus::Stopped(ref s) if s.contains("call limit")));
        assert_eq!(h.executed.len(), 3);
        assert!(matches!(ev.last(), Some(AgentEvent::Stopped(_))));

        let many: Vec<_> = (0..20).map(|_| turn_calls(vec![call("a", "undo")])).collect();
        let mut t = FakeTransport::new(many);
        let mut h = FakeHost::default();
        let lim = AgentLimits { max_turns: 2, ..Default::default() };
        let (r, _) = run(&mut t, &mut h, lim, &AtomicBool::new(false));
        assert!(matches!(r.status, AgentStatus::Stopped(ref s) if s.contains("turn limit")));
        assert_eq!(r.turns, 2);
    }

    #[test]
    fn per_turn_cap_still_answers_every_call_id() {
        let calls: Vec<_> = (0..10).map(|i| call(&format!("c{i}"), "undo")).collect();
        let mut t = FakeTransport::new(vec![turn_calls(calls), turn_text("ok")]);
        let mut h = FakeHost::default();
        let (r, _) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!(h.executed.len(), 8);
        assert_eq!(r.calls, 8);
        let answered = t.seen[1].iter().filter(|m| matches!(m, Msg::ToolResult { .. })).count();
        assert_eq!(answered, 10);
    }

    #[test]
    fn stop_flag_stops_between_calls_and_before_requests() {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let mut t = FakeTransport::new(vec![turn_calls(vec![call("1", "a"), call("2", "b"), call("3", "c")])]);
        let mut h = FakeHost { stop_after: Some((1, flag.clone())), ..Default::default() };
        let (r, ev) = run(&mut t, &mut h, AgentLimits::default(), &flag);
        assert!(matches!(r.status, AgentStatus::Stopped(_)));
        assert_eq!(h.executed.len(), 1);
        assert!(matches!(ev.last(), Some(AgentEvent::Stopped(_))));

        let mut t = FakeTransport::new(vec![turn_text("never")]);
        let mut h = FakeHost::default();
        let (r, _) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(true));
        assert!(matches!(r.status, AgentStatus::Stopped(_)));
        assert!(t.seen.is_empty());
    }

    #[test]
    fn a_finished_jobs_result_gets_more_room_than_other_results() {
        let limits = AgentLimits::default();
        assert_eq!(result_budget(&call("1", "inspect"), &limits), limits.result_budget_chars);
        let wait = ToolCall { id: "2".into(), tool: "wait_for_jobs".into(), args: json!({}) };
        assert_eq!(result_budget(&wait, &limits), limits.result_budget_chars * 4);
    }

    #[test]
    fn results_are_truncated_with_marker() {
        let big = ToolOutcome::ok(json!({"data": "x".repeat(10_000)}));
        let s = result_json(&big, 4000);
        assert!(s.contains("[truncated") && s.chars().count() < 4100);
        let small = result_json(&ToolOutcome::ok(json!(1)), 4000);
        assert!(!small.contains("truncated"));

        let mut t = FakeTransport::new(vec![turn_calls(vec![call("1", "inspect")]), turn_text("ok")]);
        let mut h = FakeHost { outcome: Some(big), ..Default::default() };
        run(&mut t, &mut h, AgentLimits { result_budget_chars: 100, ..Default::default() }, &AtomicBool::new(false));
        match &t.seen[1][2] {
            Msg::ToolResult { content, .. } => assert!(content.contains("truncated")),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn snapshot_image_only_for_image_transports() {
        let shot = ToolOutcome { ok: true, value: json!({}), error: None, image_png: Some(vec![9]) };
        for (images, expect_image) in [(true, true), (false, false)] {
            let mut t = FakeTransport::new(vec![turn_calls(vec![call("1", "snapshot")]), turn_text("ok")]);
            t.images = images;
            let mut h = FakeHost { outcome: Some(shot.clone()), ..Default::default() };
            run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
            match &t.seen[1][2] {
                Msg::ToolResult { content, image_png, .. } => {
                    assert_eq!(image_png.is_some(), expect_image);
                    assert_eq!(content.contains("cannot see images"), !expect_image);
                }
                m => panic!("{m:?}"),
            }
        }
    }

    #[test]
    fn confirmation_yes_and_no() {
        let quit = || turn_calls(vec![call("1", "quit")]);
        let mut t = FakeTransport::new(vec![quit(), turn_text("ok")]);
        let mut h = FakeHost { answer: true, ..Default::default() };
        let (_, ev) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!((h.confirmed.len(), h.executed.len()), (1, 1));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::NeedsConfirm(_))));

        let mut t = FakeTransport::new(vec![quit(), turn_text("ok")]);
        let mut h = FakeHost { answer: false, ..Default::default() };
        run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!((h.confirmed.len(), h.executed.len()), (1, 0));
        match &t.seen[1][2] {
            Msg::ToolResult { content, .. } => assert!(content.contains("declined by user")),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn existing_file_overwrite_detection() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a.fla");
        std::fs::write(&existing, b"x").unwrap();
        let missing = dir.path().join("none.fla");
        let mk = |name: &str, path: &std::path::Path, nested: bool| ToolCall {
            id: "1".into(),
            tool: "command".into(),
            args: if nested {
                json!({"name":name,"args":{"path":path.to_str().unwrap()}})
            } else {
                json!({"name":name,"path":path.to_str().unwrap()})
            },
        };
        assert!(needs_confirmation(&mk("save_as", &existing, false)));
        assert!(needs_confirmation(&mk("export_fla", &existing, true)));
        assert!(needs_confirmation(&mk("export_captions", &existing, false)));
        assert!(!needs_confirmation(&mk("save_as", &missing, false)));
        assert!(!needs_confirmation(&mk("save_snapshot", &existing, false)));
        assert!(!needs_confirmation(&mk("new_layer", &existing, false)));
        assert!(needs_confirmation(&call("1", "quit")));
        assert!(needs_confirmation(&call("1", "close")));
        assert!(!needs_confirmation(&call("1", "undo")));
    }

    #[test]
    fn llm_errors_retry_once_on_transport_not_on_4xx() {
        let mut t = FakeTransport::new(vec![Err(LlmError::Connection("down".into())), turn_text("hi")]);
        let mut h = FakeHost::default();
        let (r, _) = run(&mut t, &mut h, AgentLimits::default(), &AtomicBool::new(false));
        assert_eq!(r.status, AgentStatus::Done);
        assert_eq!(t.seen.len(), 2);

        let mut t = FakeTransport::new(vec![Err(LlmError::Http { status: 401, message: "no".into() }), turn_text("x")]);
        let (r, ev) = run(&mut t, &mut FakeHost::default(), AgentLimits::default(), &AtomicBool::new(false));
        assert!(matches!(r.status, AgentStatus::Error(_)));
        assert_eq!(t.seen.len(), 1);
        assert!(matches!(ev.last(), Some(AgentEvent::Error(_))));

        let mut t = FakeTransport::new(vec![Err(LlmError::Connection("a".into())), Err(LlmError::Connection("b".into()))]);
        let (r, _) = run(&mut t, &mut FakeHost::default(), AgentLimits::default(), &AtomicBool::new(false));
        assert!(matches!(r.status, AgentStatus::Error(_)));
        assert_eq!(t.seen.len(), 2);
    }

    #[test]
    fn system_prompt_lists_tools() {
        let tools = FakeHost::default().tools();
        let p = system_prompt(&tools, true);
        assert!(p.contains("- command(args, name): Run a command by id."));
        assert!(p.contains("inspect") && p.contains("\"done\""));
        assert!(!system_prompt(&tools, false).contains("\"done\""));
    }
}
