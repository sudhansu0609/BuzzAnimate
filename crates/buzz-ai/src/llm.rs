//! LLM backends (OpenAI-compatible / LM Studio, and Ollama) with structured JSON generation.

use std::time::Duration;
use base64::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

/// Trait for interacting with a local language and vision model.
pub trait Llm: Send + Sync {
    /// Generates structured JSON matching `schema`, validated and deserialized into `T`.
    ///
    /// Automatically retries once if the model returns invalid JSON.
    fn chat_json<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        user: &str,
        images: &[&[u8]],
        schema: serde_json::Value,
    ) -> Result<T, LlmError>;

    /// [`Llm::chat_json`] that gives up when `cancel` says so (P8-C2): a
    /// backend that can runs the call on a worker and polls `cancel` every
    /// quarter second, returning `LlmError::Other("cancelled")` within about
    /// that long and abandoning the call. The default just calls
    /// `chat_json` (cancel is checked before it starts).
    fn chat_json_cancellable<T: DeserializeOwned + Send + 'static>(
        &self,
        model: &str,
        system: &str,
        user: &str,
        images: &[&[u8]],
        schema: serde_json::Value,
        cancel: &dyn Fn() -> bool,
    ) -> Result<T, LlmError> {
        if cancel() {
            return Err(LlmError::Other("cancelled".into()));
        }
        self.chat_json(model, system, user, images, schema)
    }

    /// Generates vector embeddings for a batch of text strings.
    fn embed(
        &self,
        model: &str,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, LlmError>;
}

/// The local LLM backend to connect to.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Backend {
    /// LM Studio or any local OpenAI-compatible server (default port 1234).
    OpenAiCompatible { base_url: String },
    /// Ollama server (default port 11434).
    Ollama { base_url: String },
    /// The user's local "Claude API server" (OpenAI shape, default
    /// `http://127.0.0.1:8787/v1`), which needs a key.
    ///
    /// **The key is never serialised** (so it never reaches `workspace.json`);
    /// it lives in `ai.json` (see [`crate::ai_config`]). An empty `key` here
    /// means "read it from `ai.json` / `BUZZ_CLAUDE_API_KEY` at call time".
    ClaudeApiServer {
        base_url: String,
        #[serde(default, skip_serializing)]
        key: String,
    },
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAiCompatible { base_url } => f
                .debug_struct("OpenAiCompatible")
                .field("base_url", base_url)
                .finish(),
            Self::Ollama { base_url } => f.debug_struct("Ollama").field("base_url", base_url).finish(),
            Self::ClaudeApiServer { base_url, key } => f
                .debug_struct("ClaudeApiServer")
                .field("base_url", base_url)
                .field("key", &crate::ai_config::mask_key(key))
                .finish(),
        }
    }
}

/// When true (the default), requests ask the model for no hidden reasoning:
/// LM Studio gets `reasoning_effort: "none"`, Ollama gets `think: false`. Reasoning models
/// otherwise spend minutes on a thumbnail batch (F6a: 364 s for C8).
static LOW_REASONING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Sets the process-wide reasoning preference (the AI panel's switch).
pub fn set_low_reasoning(low: bool) {
    LOW_REASONING.store(low, std::sync::atomic::Ordering::Relaxed);
}

/// The current reasoning preference.
pub fn low_reasoning() -> bool {
    LOW_REASONING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Models known to answer without a long hidden chain of thought, recommended
/// in the Models section for C8/C10/C11 speed. Matched as substrings.
pub const RECOMMENDED_FAST_MODELS: &[&str] = &["gemma-4-12b", "gemma-4-e4b", "haiku", "sonnet"];

/// Pulls the JSON value out of a reply that may wrap it in prose or fences.
pub fn extract_json(raw: &str) -> &str {
    let s = clean_json_markdown(raw);
    if serde_json::from_str::<serde_json::Value>(s).is_ok() {
        return s;
    }
    let first = s.find(['{', '[']);
    let last = s.rfind(['}', ']']);
    match (first, last) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        _ => s,
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::OpenAiCompatible {
            base_url: "http://127.0.0.1:1234/v1".into(),
        }
    }
}

impl Backend {
    pub fn default_openai() -> Self {
        Self::OpenAiCompatible {
            base_url: "http://127.0.0.1:1234/v1".into(),
        }
    }

    pub fn default_ollama() -> Self {
        Self::Ollama {
            base_url: "http://127.0.0.1:11434".into(),
        }
    }

    /// The Claude API server at the URL from `ai.json` (or the default); the
    /// key stays empty here and is resolved from `ai.json` at call time.
    pub fn default_claude_api() -> Self {
        Self::ClaudeApiServer {
            base_url: crate::ai_config::claude_api_base_url(),
            key: String::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        match self {
            Self::OpenAiCompatible { base_url } => base_url.trim_end_matches('/'),
            Self::Ollama { base_url } => base_url.trim_end_matches('/'),
            Self::ClaudeApiServer { base_url, .. } => base_url.trim_end_matches('/'),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible { .. } => "LM Studio / OpenAI-Compatible",
            Self::Ollama { .. } => "Ollama",
            Self::ClaudeApiServer { .. } => "Claude API server",
        }
    }

    /// The key to send, if this backend needs one: the inline key, else
    /// `BUZZ_CLAUDE_API_KEY`, else `ai.json`.
    pub fn resolved_key(&self) -> Option<String> {
        match self {
            Self::ClaudeApiServer { key, .. } if !key.trim().is_empty() => Some(key.trim().to_string()),
            Self::ClaudeApiServer { .. } => crate::ai_config::claude_api_key(),
            _ => None,
        }
    }

    /// Adds the auth headers this backend needs.
    pub(crate) fn authorize(&self, req: ureq::Request) -> ureq::Request {
        match self.resolved_key() {
            Some(k) => req
                .set("Authorization", &format!("Bearer {k}"))
                .set("x-api-key", &k),
            None => req,
        }
    }

    /// Lists the models the server offers (`GET /models`, or Ollama's
    /// `/api/tags`). This is also the "Test connection" call.
    pub fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(LLM_CONNECT_TIMEOUT)
            .timeout(Duration::from_secs(20))
            .build();
        let (url, key) = match self {
            Self::Ollama { .. } => (format!("{}/api/tags", self.base_url()), "models"),
            _ => (format!("{}/models", self.base_url()), "data"),
        };
        if matches!(self, Self::ClaudeApiServer { .. }) && self.resolved_key().is_none() {
            return Err(LlmError::Other(
                "No key for the Claude API server: enter it in AI ▸ Models (stored in ai.json).".into(),
            ));
        }
        let resp = self
            .authorize(agent.get(&url))
            .call()
            .map_err(map_ureq_error)?;
        let json: serde_json::Value = resp.into_json().map_err(|e| LlmError::InvalidJson {
            raw: String::new(),
            error: e.to_string(),
        })?;
        let field = if key == "models" { "name" } else { "id" };
        Ok(json[key]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|m| m.get(field).and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
}

pub(crate) fn map_ureq_error(e: ureq::Error) -> LlmError {
    match e {
        ureq::Error::Status(status, resp) => {
            let body = resp.into_string().unwrap_or_default();
            LlmError::Http { status, message: body }
        }
        ureq::Error::Transport(t) => map_transport_error(t),
    }
}

/// Errors occurring during LLM queries.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Connection failed: {0}")]
    Connection(String),
    #[error("Request timed out: {0}")]
    Timeout(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("Failed to parse JSON response: {error}. Raw text was: {raw}")]
    InvalidJson { raw: String, error: String },
    #[error("Retry failed after {attempts} attempts. Last error: {last_error}")]
    RetryFailed {
        attempts: usize,
        last_error: String,
    },
    #[error("No response message returned from model")]
    EmptyResponse,
    #[error("Other error: {0}")]
    Other(String),
}

/// How long to wait for the TCP connection itself. Local inference servers
/// (LM Studio, Ollama) are on localhost, so this only needs to cover the
/// server actually being up and listening — not model load or generation.
///
/// A `pub const` rather than hardcoded inline so the AI Models settings
/// panel can read (and later let the user override) it; there is no UI for
/// that yet.
pub const LLM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for a whole chat/embedding request to finish, including
/// generation.
///
/// Local "reasoning" models (e.g. the qwen/gemma variants BuzzAnimate's own
/// prompts are written for) emit a hidden `reasoning_content` chain-of-thought
/// before the final answer — measured at tens of seconds for a single trivial
/// prompt, and minutes for a real 8-image identify-asset batch or a
/// multi-beat director script — so this has to be generous rather than the
/// 60s an interactive chat reply would need.
///
/// A `pub const` rather than hardcoded inline so the AI Models settings
/// panel can read (and later let the user override) it; there is no UI for
/// that yet.
pub const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

/// True if a `ureq::Transport`'s message (from [`map_transport_error`])
/// describes the local socket giving up on a slow model rather than a real
/// connectivity problem (connection refused, DNS failure, and so on).
///
/// Matched on the message text rather than `Transport::kind()` because ureq
/// 2.x has no dedicated timeout `ErrorKind` — a read/connect timeout still
/// surfaces as `ErrorKind::Io`, wrapping a platform socket error whose text
/// is the only place "this was a timeout" shows up.
fn looks_like_a_timeout(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("did not properly respond")
        || lower.contains("os error 10060")
        || lower.contains("os error 110")
}

/// Turns a `ureq::Transport` into an `LlmError`, giving a slow local model a
/// clear "it's still thinking" message instead of the raw OS socket error
/// text (`os error 10060` / `ETIMEDOUT` and friends mean nothing to someone
/// picking an AI backend).
fn map_transport_error(t: ureq::Transport) -> LlmError {
    let msg = t.to_string();
    if looks_like_a_timeout(&msg) {
        LlmError::Timeout(format!(
            "The model took longer than {}s to respond. Local reasoning models can take minutes to answer a large image batch or a complex schema — this is not a connectivity problem.",
            LLM_REQUEST_TIMEOUT.as_secs()
        ))
    } else {
        LlmError::Connection(msg)
    }
}

/// Strips markdown fences (e.g. ```json ... ```) from LLM output.
pub fn clean_json_markdown(raw: &str) -> &str {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    if let Some(rest) = s.strip_prefix("```") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    s
}

/// Probes localhost ports (:1234 for LM Studio and :11434 for Ollama)
/// with a 200 ms timeout each.
///
/// Returns detected backends and their loaded/installed models.
/// LM Studio is returned first when both are available.
pub fn detect() -> Vec<(Backend, Vec<String>)> {
    let mut detected = Vec::new();

    // 1. Probe LM Studio / OpenAI-compatible (:1234)
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(200))
        .build();

    if let Ok(resp) = agent.get("http://127.0.0.1:1234/v1/models").call() {
        if resp.status() == 200 {
            if let Ok(json) = resp.into_json::<serde_json::Value>() {
                let mut models = Vec::new();
                if let Some(list) = json.get("data").and_then(|v| v.as_array()) {
                    for m in list {
                        if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                            models.push(id.to_string());
                        }
                    }
                }
                detected.push((
                    Backend::OpenAiCompatible {
                        base_url: "http://127.0.0.1:1234/v1".into(),
                    },
                    models,
                ));
            }
        }
    }

    // 2. Probe Ollama (:11434)
    if let Ok(resp) = agent.get("http://127.0.0.1:11434/api/tags").call() {
        if resp.status() == 200 {
            if let Ok(json) = resp.into_json::<serde_json::Value>() {
                let mut models = Vec::new();
                if let Some(list) = json.get("models").and_then(|v| v.as_array()) {
                    for m in list {
                        if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                            models.push(name.to_string());
                        }
                    }
                }
                detected.push((
                    Backend::Ollama {
                        base_url: "http://127.0.0.1:11434".into(),
                    },
                    models,
                ));
            }
        }
    }

    // 3. The Claude API server, only when the user has stored a key for it
    //    (it answers 401 without one) AND switched Claude on: no silent fallback.
    if crate::ai_config::use_claude() && crate::ai_config::claude_api_key().is_some() {
        let claude = Backend::default_claude_api();
        let probe = ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(1500))
            .build();
        if let Ok(resp) = claude
            .authorize(probe.get(&format!("{}/models", claude.base_url())))
            .call()
        {
            if let Ok(json) = resp.into_json::<serde_json::Value>() {
                let models = json["data"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(|m| m["id"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                detected.push((claude, models));
            }
        }
    }

    detected
}

impl Llm for Backend {
    fn chat_json_cancellable<T: DeserializeOwned + Send + 'static>(
        &self,
        model: &str,
        system: &str,
        user: &str,
        images: &[&[u8]],
        schema: serde_json::Value,
        cancel: &dyn Fn() -> bool,
    ) -> Result<T, LlmError> {
        use std::sync::mpsc::{RecvTimeoutError, channel};
        if cancel() {
            return Err(LlmError::Other("cancelled".into()));
        }
        // The call runs on a worker; this thread polls `cancel`. On cancel
        // the worker is abandoned: its answer (or its timeout) is dropped.
        let me = self.clone();
        let (model, system, user) = (model.to_string(), system.to_string(), user.to_string());
        let images: Vec<Vec<u8>> = images.iter().map(|i| i.to_vec()).collect();
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("llm-call".into())
            .spawn(move || {
                let refs: Vec<&[u8]> = images.iter().map(Vec::as_slice).collect();
                let _ = tx.send(me.chat_json::<T>(&model, &system, &user, &refs, schema));
            })
            .map_err(|e| LlmError::Other(format!("could not start the model call: {e}")))?;
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) => {
                    if cancel() {
                        return Err(LlmError::Other("cancelled".into()));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(LlmError::Other("the model call ended without an answer".into()));
                }
            }
        }
    }

    fn chat_json<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        user: &str,
        images: &[&[u8]],
        schema: serde_json::Value,
    ) -> Result<T, LlmError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(LLM_CONNECT_TIMEOUT)
            .timeout(LLM_REQUEST_TIMEOUT)
            .build();

        // First attempt
        let mut last_error = String::new();
        for attempt in 1..=2 {
            let user_prompt = if attempt == 1 {
                user.to_string()
            } else {
                format!(
                    "{user}\n\nNotice: Your previous response could not be parsed according to the required schema: {last_error}. Please output strictly valid JSON conforming to the schema and nothing else."
                )
            };

            let raw_text = match self {
                Backend::OpenAiCompatible { .. } | Backend::ClaudeApiServer { .. } => {
                    let url = format!("{}/chat/completions", self.base_url());
                    let is_claude = matches!(self, Backend::ClaudeApiServer { .. });
                    // The Claude API server ignores `response_format`, so the
                    // schema also goes into the system prompt.
                    let system_text = if is_claude {
                        format!(
                            "{system}\n\nReply with one JSON value that matches this JSON Schema, and nothing else (no prose, no code fences):\n{schema}"
                        )
                    } else {
                        system.to_string()
                    };
                    let mut user_content_parts = Vec::new();
                    user_content_parts.push(serde_json::json!({
                        "type": "text",
                        "text": user_prompt,
                    }));
                    for img in images {
                        let b64 = BASE64_STANDARD.encode(img);
                        user_content_parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:image/png;base64,{b64}"),
                            }
                        }));
                    }

                    // No explicit model: the saved Claude model (default "opus").
                    let model_name = if is_claude && model.trim().is_empty() {
                        crate::ai_config::claude_model()
                    } else {
                        model.to_string()
                    };
                    let mut payload = serde_json::json!({
                        "model": model_name,
                        "messages": [
                            {"role": "system", "content": system_text},
                            {"role": "user", "content": user_content_parts},
                        ],
                        "response_format": {
                            "type": "json_schema",
                            "json_schema": {
                                "name": "response",
                                "strict": true,
                                "schema": schema,
                            }
                        }
                    });
                    if is_claude {
                        // The proxy defaults to "low"; ask for the user's choice.
                        payload["reasoning_effort"] = serde_json::json!(crate::ai_config::CLAUDE_DEFAULT_EFFORT);
                    } else if low_reasoning() {
                        // Measured on LM Studio + qwen3.8-27b (2026-09-26):
                        // "none" gives 0 reasoning tokens; "low", `/no_think`
                        // and `chat_template_kwargs.enable_thinking=false`
                        // all still reasoned.
                        payload["reasoning_effort"] = serde_json::json!("none");
                    }

                    let resp = self.authorize(agent.post(&url))
                        .send_json(payload)
                        .map_err(|e| match e {
                            ureq::Error::Status(status, resp) => {
                                let body = resp.into_string().unwrap_or_default();
                                LlmError::Http { status, message: body }
                            }
                            ureq::Error::Transport(t) => map_transport_error(t),
                        })?;

                    let res_val: serde_json::Value = resp.into_json()
                        .map_err(|e| LlmError::InvalidJson {
                            raw: String::new(),
                            error: e.to_string(),
                        })?;

                    res_val["choices"][0]["message"]["content"]
                        .as_str()
                        .ok_or(LlmError::EmptyResponse)?
                        .to_string()
                }
                Backend::Ollama { .. } => {
                    let url = format!("{}/api/chat", self.base_url());
                    let mut images_b64 = Vec::new();
                    for img in images {
                        images_b64.push(BASE64_STANDARD.encode(img));
                    }

                    let user_msg = if images_b64.is_empty() {
                        serde_json::json!({
                            "role": "user",
                            "content": user_prompt,
                        })
                    } else {
                        serde_json::json!({
                            "role": "user",
                            "content": user_prompt,
                            "images": images_b64,
                        })
                    };

                    let mut payload = serde_json::json!({
                        "model": model,
                        "stream": false,
                        "messages": [
                            {"role": "system", "content": system},
                            user_msg,
                        ],
                        "format": schema,
                    });
                    if low_reasoning() {
                        payload["think"] = serde_json::json!(false);
                    }

                    let resp = agent.post(&url)
                        .send_json(payload)
                        .map_err(|e| match e {
                            ureq::Error::Status(status, resp) => {
                                let body = resp.into_string().unwrap_or_default();
                                LlmError::Http { status, message: body }
                            }
                            ureq::Error::Transport(t) => map_transport_error(t),
                        })?;

                    let res_val: serde_json::Value = resp.into_json()
                        .map_err(|e| LlmError::InvalidJson {
                            raw: String::new(),
                            error: e.to_string(),
                        })?;

                    res_val["message"]["content"]
                        .as_str()
                        .ok_or(LlmError::EmptyResponse)?
                        .to_string()
                }
            };

            let cleaned = extract_json(&raw_text);
            match serde_json::from_str::<T>(cleaned) {
                Ok(val) => return Ok(val),
                Err(err) => {
                    last_error = err.to_string();
                    tracing::warn!(
                        "LLM chat_json attempt {attempt} failed to deserialize: {err}. Raw output was: {raw_text}"
                    );
                    if attempt == 2 {
                        return Err(LlmError::InvalidJson {
                            raw: raw_text,
                            error: last_error,
                        });
                    }
                }
            }
        }

        Err(LlmError::RetryFailed {
            attempts: 2,
            last_error,
        })
    }

    fn embed(
        &self,
        model: &str,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, LlmError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(LLM_CONNECT_TIMEOUT)
            .timeout(LLM_REQUEST_TIMEOUT)
            .build();

        match self {
            Backend::OpenAiCompatible { .. } | Backend::ClaudeApiServer { .. } => {
                let url = format!("{}/embeddings", self.base_url());
                let payload = serde_json::json!({
                    "model": model,
                    "input": texts,
                });

                let resp = self.authorize(agent.post(&url))
                    .send_json(payload)
                    .map_err(|e| match e {
                        ureq::Error::Status(status, resp) => {
                            let body = resp.into_string().unwrap_or_default();
                            LlmError::Http { status, message: body }
                        }
                        ureq::Error::Transport(t) => map_transport_error(t),
                    })?;

                let res_val: serde_json::Value = resp.into_json()
                    .map_err(|e| LlmError::InvalidJson {
                        raw: String::new(),
                        error: e.to_string(),
                    })?;

                let mut embeddings = Vec::new();
                if let Some(data) = res_val["data"].as_array() {
                    for item in data {
                        if let Some(arr) = item["embedding"].as_array() {
                            let vec: Vec<f32> = arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect();
                            embeddings.push(vec);
                        }
                    }
                }
                Ok(embeddings)
            }
            Backend::Ollama { .. } => {
                let url = format!("{}/api/embed", self.base_url());
                let payload = serde_json::json!({
                    "model": model,
                    "input": texts,
                });

                let resp = agent.post(&url).send_json(payload);
                match resp {
                    Ok(r) => {
                        let res_val: serde_json::Value = r.into_json().map_err(|e| LlmError::InvalidJson {
                            raw: String::new(),
                            error: e.to_string(),
                        })?;
                        let mut embeddings = Vec::new();
                        if let Some(arr) = res_val["embeddings"].as_array() {
                            for sub in arr {
                                if let Some(vals) = sub.as_array() {
                                    let vec: Vec<f32> = vals.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect();
                                    embeddings.push(vec);
                                }
                            }
                        }
                        Ok(embeddings)
                    }
                    Err(_) => {
                        // Fallback to legacy /api/embeddings endpoint per text
                        let legacy_url = format!("{}/api/embeddings", self.base_url());
                        let mut embeddings = Vec::new();
                        for t in texts {
                            let legacy_payload = serde_json::json!({
                                "model": model,
                                "prompt": t,
                            });
                            let r = agent.post(&legacy_url).send_json(legacy_payload).map_err(|e| match e {
                                ureq::Error::Status(status, resp) => {
                                    let body = resp.into_string().unwrap_or_default();
                                    LlmError::Http { status, message: body }
                                }
                                ureq::Error::Transport(t) => map_transport_error(t),
                            })?;
                            let res_val: serde_json::Value = r.into_json().map_err(|e| LlmError::InvalidJson {
                                raw: String::new(),
                                error: e.to_string(),
                            })?;
                            if let Some(arr) = res_val["embedding"].as_array() {
                                let vec: Vec<f32> = arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect();
                                embeddings.push(vec);
                            }
                        }
                        Ok(embeddings)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod timeout_mapping_tests {
    use super::*;

    /// The Windows text for a read that never got a byte back (what
    /// `chat_json` actually saw against a real, slow local model before this
    /// fix: `.timeout(60s)` firing mid-response).
    #[test]
    fn windows_connection_timeout_text_is_recognised() {
        assert!(looks_like_a_timeout(
            "Error encountered in the status line: A connection attempt failed because the \
             connected party did not properly respond after a period of time, or established \
             connection failed because connected host has failed to respond. (os error 10060)"
        ));
    }

    /// The Unix/Linux equivalent (`ETIMEDOUT`), so the same message doesn't
    /// silently stop working off Windows.
    #[test]
    fn unix_timeout_text_is_recognised() {
        assert!(looks_like_a_timeout("Connection timed out (os error 110)"));
        assert!(looks_like_a_timeout("Read timed out"));
    }

    /// A real connectivity problem — nothing listening on the port — must
    /// not be relabelled as "the model is thinking".
    #[test]
    fn connection_refused_is_not_a_timeout() {
        assert!(!looks_like_a_timeout("Connection refused (os error 61)"));
        assert!(!looks_like_a_timeout("dns error: failed to lookup address information"));
    }

    #[test]
    fn timeout_message_names_the_configured_duration() {
        match map_transport_error_message_for_test(
            "Error encountered in the status line: ... did not properly respond ... (os error 10060)",
        ) {
            LlmError::Timeout(msg) => {
                assert!(msg.contains(&LLM_REQUEST_TIMEOUT.as_secs().to_string()), "{msg}");
            }
            other => panic!("expected LlmError::Timeout, got {other:?}"),
        }
    }

    #[test]
    fn non_timeout_message_stays_a_connection_error() {
        match map_transport_error_message_for_test("Connection refused (os error 61)") {
            LlmError::Connection(msg) => assert_eq!(msg, "Connection refused (os error 61)"),
            other => panic!("expected LlmError::Connection, got {other:?}"),
        }
    }

    /// `map_transport_error` takes an actual `ureq::Transport`, which this
    /// crate cannot construct outside `ureq` itself — so this mirrors its
    /// (trivial) message-classification body directly, against the same
    /// `looks_like_a_timeout` the real function calls.
    fn map_transport_error_message_for_test(msg: &str) -> LlmError {
        if looks_like_a_timeout(msg) {
            LlmError::Timeout(format!(
                "The model took longer than {}s to respond. Local reasoning models can take minutes to answer a large image batch or a complex schema — this is not a connectivity problem.",
                LLM_REQUEST_TIMEOUT.as_secs()
            ))
        } else {
            LlmError::Connection(msg.to_string())
        }
    }
}
