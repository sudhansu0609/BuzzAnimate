//! **BuzzAnimate's clients for the rest of the estate**: BuzzEdit and Buzzcaf
//! Studio (steps E2 and E3 of `docs/ECOSYSTEM_BRIDGE_PLAN.md`).
//!
//! * **File ▸ Send to BuzzEdit…** (agent tool `send_to_buzzedit`): an MP4 on
//!   the app's own export queue — the path `POST /api/export` takes — handed
//!   to BuzzEdit as a new project (`POST /api/projects/import_path`) or into
//!   one of its projects (`POST /api/projects/{id}/media/import`, the list
//!   from `GET /api/projects/list`, tool `buzzedit_projects`).
//! * **Commands ▸ Transcribe with BuzzEdit…** (tool `transcribe_with_buzzedit`):
//!   a library sound goes to BuzzEdit as a project of its own, BuzzEdit's
//!   Whisper hears it (`POST /api/transcription/transcribe`, followed through
//!   `GET /api/transcription/status/{job}`), and its words
//!   (`GET /api/transcription/{id}/transcript`) become captions and the
//!   speaker's lip sync, through the join Lip Sync from Captions uses
//!   (`Editor::apply_transcript`).
//! * **File ▸ Import Script from Studio…** (tools `studio_projects`,
//!   `studio_build_episode`): the Studio's projects (`GET /api/projects`), one
//!   project's script (`GET /api/projects/{id}/asset/script`), built into an
//!   episode by the same function `POST /api/episode` runs
//!   (`crate::api::build_episode_files`), into `<project folder>/animation/`.
//!
//! **Finding them.** The port ledger first ([`crate::ports`]), then the
//! preferred port and the twenty after it, and in both cases the app's health
//! route has to *name* it: BuzzEdit and the Studio both prefer 8099, and an
//! answer from the wrong one is no answer. This is the rule
//! `buzzcaf_ports.discover` follows in Python. A peer that is not running is
//! said so, and offered a start: its `.vbs` launcher in the estate root
//! (BuzzAnimate's parent folder, found from the executable or the working
//! folder, never written down), then its health route is waited on.
//!
//! **Nothing here runs on the UI thread** except the bookkeeping. Every flow
//! is one task in the Tasks panel; the only thing it asks of the UI thread is
//! an export, which goes on the queue like any other and is answered when it
//! ends ([`App::estate_pump`], once a frame).

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded, unbounded};
use serde_json::{Value, json};

use crate::agent::AgentResult;
use crate::agent::jobs::{AgentJob, JobState};
use crate::app::App;
use crate::ports;
use crate::tasks::{TaskCtx, TaskId, TaskKind, TaskOutcome};

/// A connect to a port nobody listens on is refused at once on most systems;
/// Windows retries for about a second, so the scan does not wait for that.
const CONNECT: Duration = Duration::from_millis(250);
/// The Studio's health route asks LM Studio how it is, so it gets some room.
const HEALTH: Duration = Duration::from_secs(3);
/// A project list, a script, a transcript.
const READ: Duration = Duration::from_secs(30);
/// BuzzEdit copies a recording into its projects folder before it answers.
const IMPORT: Duration = Duration::from_secs(600);
/// BuzzEdit answers the transcription POST when Whisper has finished.
const TRANSCRIBE: Duration = Duration::from_secs(3600);
/// How long a just-launched app gets to answer its health route.
pub const START_WAIT: Duration = Duration::from_secs(180);

// ------------------------------------------------------------------ peers

/// One of the other apps: where it usually listens, how it names itself, and
/// how it is started.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// Its ledger name, which its health route must answer as `app`.
    pub app: &'static str,
    /// What the status line calls it.
    pub label: &'static str,
    pub preferred: u16,
    pub span: u16,
    pub health: &'static str,
    /// Its folder in the estate root, and its launcher there.
    pub folder: &'static str,
    pub launcher: &'static str,
}

pub const BUZZEDIT: Peer = Peer {
    app: "buzzedit",
    label: "BuzzEdit",
    preferred: 8099,
    span: ports::SPAN,
    health: "/api/health",
    folder: "BuzzEdit",
    launcher: "START_APP.vbs",
};

pub const STUDIO: Peer = Peer {
    app: "buzzcaf",
    label: "Buzzcaf Studio",
    preferred: 8099,
    span: ports::SPAN,
    health: "/health",
    folder: "BuzzcafStudio",
    launcher: "Buzzcaf Studio.vbs",
};

/// Where the other apps are looked for.
#[derive(Debug, Clone)]
pub struct Estate {
    pub ledger: PathBuf,
    /// The folder holding BuzzAnimate, BuzzEdit and BuzzcafStudio.
    pub root: Option<PathBuf>,
    pub buzzedit: Peer,
    pub studio: Peer,
    pub start_wait: Duration,
}

impl Estate {
    /// The machine's ledger and the estate root above this program.
    pub fn from_env() -> Self {
        Self {
            ledger: ports::ledger_path(),
            root: estate_root(),
            buzzedit: BUZZEDIT,
            studio: STUDIO,
            start_wait: START_WAIT,
        }
    }
}

/// The estate root: the first folder above the executable (else the working
/// folder) that holds BuzzEdit or BuzzcafStudio.
pub fn estate_root() -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.push(exe);
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.iter().find_map(|s| root_above(s))
}

/// The first folder at or above `from` that holds a sibling app.
pub fn root_above(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|dir| [BUZZEDIT.folder, STUDIO.folder].iter().any(|f| dir.join(f).is_dir()))
        .map(Path::to_path_buf)
}

// ------------------------------------------------------------------ http

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn http(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(timeout)
        .build()
}

/// A JSON answer, or what went wrong with the server's own words in it
/// (FastAPI puts them in `detail`).
fn answer(result: Result<ureq::Response, ureq::Error>, what: &str) -> Result<Value, String> {
    match result {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|e| format!("{what}: the answer was not JSON ({e})")),
        Err(ureq::Error::Status(code, response)) => {
            let body: Value = response.into_json().unwrap_or(Value::Null);
            let detail = body
                .get("detail")
                .or_else(|| body.get("error"))
                .map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string()))
                .unwrap_or_default();
            if detail.is_empty() {
                Err(format!("{what} answered {code}"))
            } else {
                Err(format!("{what} answered {code}: {detail}"))
            }
        }
        Err(e) => Err(format!("{what}: {e}")),
    }
}

pub fn get_json(port: u16, path: &str, timeout: Duration) -> Result<Value, String> {
    answer(http(timeout).get(&url(port, path)).call(), &format!("GET {path}"))
}

pub fn post_json(port: u16, path: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    answer(http(timeout).post(&url(port, path)).send_json(body.clone()), &format!("POST {path}"))
}

/// An id that is safe in a URL path and as a folder name: what the Studio's
/// own `_SAFE_ID` allows.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

// ------------------------------------------------------------------ discovery

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), CONNECT).is_ok()
}

/// Does the server on `port` say it is `peer`?
pub fn identifies(port: u16, peer: &Peer) -> bool {
    get_json(port, peer.health, HEALTH)
        .ok()
        .and_then(|v| v.get("app").and_then(Value::as_str).map(str::to_string))
        .as_deref()
        == Some(peer.app)
}

/// Where `peer` is listening: its ledger entry, else its preferred port and
/// the `span` after it — and only where its health route names it.
pub fn discover(estate: &Estate, peer: &Peer) -> Option<u16> {
    let mut tried = Vec::new();
    if let Some(port) = ports::published_port(&estate.ledger, peer.app) {
        tried.push(port);
        if listening(port) && identifies(port, peer) {
            return Some(port);
        }
    }
    if peer.preferred == 0 {
        return None;
    }
    let last = peer.preferred.saturating_add(peer.span);
    (peer.preferred..=last)
        .filter(|p| !tried.contains(p))
        .find(|&port| listening(port) && identifies(port, peer))
}

/// What went wrong in a flow, and whether it was only that the app is not up.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub message: String,
    pub not_running: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, not_running: false }
    }
}

pub fn not_running(peer: &Peer) -> String {
    format!(
        "{} is not running (nothing answered as '{}' on the port ledger or ports {}\u{2013}{})",
        peer.label,
        peer.app,
        peer.preferred,
        peer.preferred.saturating_add(peer.span)
    )
}

/// Start `peer` with its launcher in the estate root.
pub fn launch(estate: &Estate, peer: &Peer) -> Result<PathBuf, String> {
    let Some(root) = estate.root.as_ref() else {
        return Err(format!(
            "{} cannot be started from here: the folder holding the Buzzcaf apps was not found above this program",
            peer.label
        ));
    };
    let launcher = root.join(peer.folder).join(peer.launcher);
    if !launcher.is_file() {
        return Err(format!("{} cannot be started: its launcher is not at {}", peer.label, launcher.display()));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("wscript.exe")
            .arg(&launcher)
            .current_dir(launcher.parent().unwrap_or(root))
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", peer.label))?;
        Ok(launcher)
    }
    #[cfg(not(windows))]
    {
        Err(format!(
            "{} cannot be started from here: its launcher ({}) is a Windows script",
            peer.label,
            launcher.display()
        ))
    }
}

/// `peer`'s port: found, or (with `start`) started and waited for.
pub fn reach(estate: &Estate, peer: &Peer, start: bool, ctx: Option<&TaskCtx>) -> Result<u16, Failure> {
    if let Some(ctx) = ctx {
        ctx.progress.detail(format!("looking for {}", peer.label));
    }
    if let Some(port) = discover(estate, peer) {
        return Ok(port);
    }
    if !start {
        return Err(Failure { message: not_running(peer), not_running: true });
    }
    launch(estate, peer)?;
    let began = Instant::now();
    while began.elapsed() < estate.start_wait {
        if let Some(ctx) = ctx {
            ctx.progress
                .detail(format!("starting {} ({} s)", peer.label, began.elapsed().as_secs()));
        }
        // Two seconds between looks, in steps short enough for Stop.
        let nap = Instant::now();
        while nap.elapsed() < Duration::from_secs(2) {
            if ctx.is_some_and(TaskCtx::cancelled) {
                return Err(format!("cancelled while {} was starting", peer.label).into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(port) = discover(estate, peer) {
            return Ok(port);
        }
    }
    Err(format!(
        "{} was started but did not answer within {} s; try again once its window is up",
        peer.label,
        estate.start_wait.as_secs()
    )
    .into())
}

// ------------------------------------------------------------------ BuzzEdit

/// `GET /api/projects/list`: `[{id, name, status}]`.
pub fn buzzedit_projects(port: u16) -> Result<Vec<Value>, String> {
    let v = get_json(port, "/api/projects/list", READ)?;
    let list = v.get("projects").and_then(Value::as_array).ok_or("BuzzEdit's project list had no 'projects'")?;
    Ok(list
        .iter()
        .map(|p| json!({"id": p.get("id"), "name": p.get("name"), "status": p.get("status")}))
        .collect())
}

/// Hand `video` to BuzzEdit: a new project from it, or added to `project`'s
/// media (a copy kept in the project, so the file here may move).
pub fn buzzedit_import(port: u16, video: &Path, project: Option<&str>) -> Result<Value, String> {
    let path = video.display().to_string();
    match project {
        None => {
            let p = post_json(port, "/api/projects/import_path", &json!({"path": path}), IMPORT)?;
            let id = p.get("id").and_then(Value::as_str).ok_or("BuzzEdit made a project but did not say its id")?;
            Ok(json!({"project_id": id, "project_name": p.get("name"), "new_project": true}))
        }
        Some(id) => {
            if !safe_id(id) {
                return Err(format!("'{id}' is not a BuzzEdit project id"));
            }
            let r = post_json(
                port,
                &format!("/api/projects/{id}/media/import"),
                &json!({"paths": [path], "copy": true}),
                IMPORT,
            )?;
            let added = r.get("added").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            Ok(json!({"project_id": id, "project_name": null, "new_project": false, "added": added}))
        }
    }
}

/// One heard word: start and end in seconds, and the word.
pub type Word = (f64, f64, String);

/// BuzzEdit's transcription of `audio`: the project it made, the words, and
/// the language it settled on. BuzzEdit answers the POST only when Whisper is
/// done, so the POST waits on a thread of its own while the job's stage is
/// followed here and Stop still works.
pub fn buzzedit_transcribe(port: u16, audio: &Path, ctx: Option<&TaskCtx>) -> Result<(String, Vec<Word>, Option<String>), String> {
    let detail = |what: String| {
        if let Some(ctx) = ctx {
            ctx.progress.detail(what);
        }
    };
    detail("handing the sound to BuzzEdit".into());
    let project = post_json(port, "/api/projects/import_path", &json!({"path": audio.display().to_string()}), IMPORT)?;
    let id = project
        .get("id")
        .and_then(Value::as_str)
        .ok_or("BuzzEdit made a project but did not say its id")?
        .to_string();
    if !safe_id(&id) {
        return Err(format!("BuzzEdit answered with an odd project id '{id}'"));
    }

    // `precut`: every word as heard, with no auto-edit cut and no second
    // pass to verify a cut.
    let body = json!({"project_id": id, "precut": true});
    let (send, receive) = bounded(1);
    std::thread::Builder::new()
        .name("buzz-estate-transcribe".into())
        .spawn(move || {
            let _ = send.send(post_json(port, "/api/transcription/transcribe", &body, TRANSCRIBE));
        })
        .map_err(|e| format!("could not wait for BuzzEdit: {e}"))?;
    let status = format!("/api/transcription/status/trans_{id}");
    let mut looked = Instant::now();
    detail("BuzzEdit is transcribing".into());
    loop {
        if ctx.is_some_and(TaskCtx::cancelled) {
            return Err(format!("cancelled; BuzzEdit carries on transcribing its project {id}"));
        }
        match receive.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => {
                result?;
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("the wait for BuzzEdit stopped unexpectedly".into()),
        }
        if looked.elapsed() > Duration::from_secs(2) {
            looked = Instant::now();
            if let Ok(job) = get_json(port, &status, HEALTH) {
                if let Some(stage) = job.get("stage").and_then(Value::as_str) {
                    detail(format!("BuzzEdit: {stage}"));
                }
            }
        }
    }

    let transcript = get_json(port, &format!("/api/transcription/{id}/transcript"), READ)?;
    let mut words: Vec<Word> = Vec::new();
    for segment in transcript.get("segments").and_then(Value::as_array).into_iter().flatten() {
        for w in segment.get("words").and_then(Value::as_array).into_iter().flatten() {
            let text = w.get("word").and_then(Value::as_str).unwrap_or("").trim();
            let (Some(a), Some(b)) = (w.get("start").and_then(Value::as_f64), w.get("end").and_then(Value::as_f64)) else {
                continue;
            };
            if !text.is_empty() {
                words.push((a, b.max(a), text.to_string()));
            }
        }
    }
    let language = transcript.get("language").and_then(Value::as_str).map(str::to_string);
    Ok((id, words, language))
}

/// Words into caption lines: a new line after a pause of more than 0.6 s,
/// after twelve words, and after a sentence ends.
pub fn words_to_lines(words: &[Word]) -> Vec<buzz_ai::transcribe::Segment> {
    const PAUSE: f64 = 0.6;
    const MOST: usize = 12;
    let mut lines = Vec::new();
    let mut line: Vec<&Word> = Vec::new();
    let flush = |line: &mut Vec<&Word>, lines: &mut Vec<buzz_ai::transcribe::Segment>| {
        if let (Some(first), Some(last)) = (line.first(), line.last()) {
            lines.push(buzz_ai::transcribe::Segment {
                start: first.0,
                end: last.1,
                text: line.iter().map(|w| w.2.as_str()).collect::<Vec<_>>().join(" "),
            });
        }
        line.clear();
    };
    for word in words {
        if let Some(last) = line.last() {
            if word.0 - last.1 > PAUSE || line.len() >= MOST {
                flush(&mut line, &mut lines);
            }
        }
        line.push(word);
        if word.2.ends_with(['.', '?', '!', '\u{964}']) {
            flush(&mut line, &mut lines);
        }
    }
    flush(&mut line, &mut lines);
    lines
}

// ------------------------------------------------------------------ Studio

/// `GET /api/projects`, condensed: `[{id, name, brand, status, current_step, has_script}]`.
pub fn studio_projects(port: u16) -> Result<Vec<Value>, String> {
    let v = get_json(port, "/api/projects", READ)?;
    let list = v.as_array().ok_or("the Studio's project list was not a list")?;
    Ok(list
        .iter()
        .map(|p| {
            json!({
                "id": p.get("id"),
                "name": p.get("name"),
                "brand": p.get("brand"),
                "status": p.get("status"),
                "current_step": p.get("current_step"),
                "has_script": p.get("assets").and_then(|a| a.get("script")).is_some(),
            })
        })
        .collect())
}

/// A project's script text: `GET /api/projects/{id}/asset/{asset}`, which
/// answers `{content}` for a text file (and the object itself for JSON).
pub fn studio_script(port: u16, project: &str, asset: &str) -> Result<String, String> {
    if !safe_id(project) {
        return Err(format!("'{project}' is not a Studio project id"));
    }
    if !safe_id(asset) {
        return Err(format!("'{asset}' is not an asset name"));
    }
    let v = get_json(port, &format!("/api/projects/{project}/asset/{asset}"), READ)?;
    v.get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("the Studio's '{asset}' for {project} is JSON, not a script"))
}

/// **A Studio script, as a screenplay the episode builder can read.**
///
/// The Studio writes narrated videos, not screenplays: one voice, no
/// character cues, and none of the `## SCENE n — TITLE` headings
/// `buzz_act::screenplay` starts a scene on — so its scripts used to build
/// nothing at all ("no scenes"). It writes three shapes, read off the projects
/// under `BuzzcafStudio/backend/projects/*/script/`:
///
/// * `draft.md` — `## [SCENE 01: TITLE]` blocks of labelled lines:
///   `**V.O. (Narrator):** …` is spoken, `**VISUAL:**`, `**SOUND:**`,
///   `**TEXT OVERLAY:**` and `[Visual: …]` are direction.
/// * `buzzedit_script.md` — `# SECTION` headings over plain narration with
///   BuzzEdit's inline directives (`[broll: …]`, `[fx: …]`, `[pills: …]`).
/// * `short_script.md` — a `# Title` over paragraphs carrying delivery cues:
///   `(pause)`, `(beat)`, `(slow)`, `(normal)`.
///
/// Each becomes scenes of `**NARRATOR (V.O.)**` lines, with direction kept as
/// action, inline directives taken out of what is said, and `(pause)` /
/// `(beat)` turned into the screenplay's own `Beat.`. A script that is
/// already a screenplay is passed through untouched.
pub fn studio_to_screenplay(text: &str) -> String {
    if text.lines().any(|l| l.trim_start().starts_with("## SCENE ")) {
        return text.to_string();
    }
    enum Block {
        Action(String),
        Line(String),
        Beat,
    }
    // `draft.md` has metadata headings above its first scene, so headings
    // only start scenes in the other two shapes.
    let draft = text.lines().any(|l| draft_scene_title(l.trim()).is_some());
    let mut scenes: Vec<(String, Vec<Block>)> = Vec::new();
    let push = |scenes: &mut Vec<(String, Vec<Block>)>, block: Block| {
        if scenes.is_empty() {
            scenes.push(("SCENE".into(), Vec::new()));
        }
        scenes.last_mut().unwrap().1.push(block);
    };
    let speak = |scenes: &mut Vec<(String, Vec<Block>)>, raw: &str| {
        for (i, part) in split_on_pauses(raw).into_iter().enumerate() {
            if i > 0 {
                push(scenes, Block::Beat);
            }
            let said = clean_spoken(&part);
            if !said.is_empty() {
                push(scenes, Block::Line(said));
            }
        }
    };

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(title) = draft_scene_title(t) {
            scenes.push((title, Vec::new()));
            continue;
        }
        if t.starts_with('#') {
            let level = t.chars().take_while(|c| *c == '#').count();
            let heading = t.trim_start_matches('#').trim();
            if !draft && level == 1 && !heading.is_empty() {
                scenes.push((heading.to_string(), Vec::new()));
            }
            continue;
        }
        if t.chars().all(|c| c == '-') && t.len() >= 3 {
            if !draft && !scenes.is_empty() {
                push(&mut scenes, Block::Beat);
            }
            continue;
        }
        // `**LABEL:** rest` (or `**LABEL**: rest`).
        if let Some(rest) = t.strip_prefix("**")
            && let Some(end) = rest.find("**")
        {
            let label = rest[..end].trim().trim_end_matches(':').to_ascii_uppercase();
            let body = rest[end + 2..].trim_start_matches(':').trim();
            let spoken = ["V.O", "VO", "NARRAT", "VOICE"].iter().any(|k| label.contains(k));
            if spoken {
                speak(&mut scenes, body);
            } else if !body.is_empty() {
                let what = clean_direction(body);
                if !what.is_empty() {
                    push(&mut scenes, Block::Action(what));
                }
            }
            continue;
        }
        // A whole line of direction: `[Visual: …]`.
        if t.starts_with('[') && t.ends_with(']') {
            let what = clean_direction(t);
            if !what.is_empty() {
                push(&mut scenes, Block::Action(what));
            }
            continue;
        }
        if draft {
            // Unlabelled prose in a draft is the writer describing, not the
            // voice speaking.
            push(&mut scenes, Block::Action(clean_direction(t)));
        } else {
            speak(&mut scenes, t);
        }
    }

    let mut out = String::new();
    for (index, (title, blocks)) in scenes.iter().enumerate() {
        out.push_str(&format!("## SCENE {} \u{2014} {}\n\n", index + 1, title));
        for block in blocks {
            match block {
                Block::Action(what) => out.push_str(&format!("{what}\n\n")),
                Block::Line(said) => out.push_str(&format!("**NARRATOR (V.O.)**\n{said}\n\n")),
                Block::Beat => out.push_str("Beat.\n\n"),
            }
        }
        out.push_str("---\n\n");
    }
    out
}

/// `## [SCENE 01: THE HOOK]` → `THE HOOK`.
fn draft_scene_title(line: &str) -> Option<String> {
    let inner = line.strip_prefix("##")?.trim().strip_prefix('[')?.strip_suffix(']')?;
    let rest = inner.trim().strip_prefix("SCENE")?.trim_start();
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
    let title = rest.trim_start_matches([':', '-', '\u{2014}']).trim();
    Some(if title.is_empty() { "SCENE".into() } else { title.to_string() })
}

/// Split narration at its `(pause)` / `(beat)` / `(long pause)` cues.
fn split_on_pauses(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')').map(|c| open + c) else {
            break;
        };
        let cue = rest[open + 1..close].trim().to_ascii_lowercase();
        parts.last_mut().unwrap().push_str(&rest[..open]);
        if matches!(cue.as_str(), "pause" | "beat" | "long pause" | "short pause") {
            parts.push(String::new());
        } else {
            // Kept for `clean_spoken` to judge: a delivery cue goes, words in
            // brackets that are part of the sentence stay.
            parts.last_mut().unwrap().push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    parts.last_mut().unwrap().push_str(rest);
    parts
}

/// What the narrator actually says: inline `[directive: …]` spans out, one-word
/// delivery cues like `(slow)` out, markdown emphasis out, spaces collapsed.
fn clean_spoken(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '*' => {}
            _ => out.push(c),
        }
    }
    let mut kept = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')').map(|c| open + c) else {
            break;
        };
        let inner = rest[open + 1..close].trim();
        let cue = !inner.is_empty()
            && !inner.contains(' ')
            && inner.chars().all(|c| c.is_ascii_lowercase());
        kept.push_str(&rest[..open]);
        if !cue {
            kept.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    kept.push_str(rest);
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Direction as an action line: brackets and a leading `Visual:`-style label
/// dropped, emphasis out.
fn clean_direction(text: &str) -> String {
    let t = text.trim().trim_start_matches('[').trim_end_matches(']').trim();
    let t = match t.split_once(':') {
        Some((label, rest)) if !label.is_empty() && label.len() <= 16 && !label.contains(' ') => rest,
        _ => t,
    };
    t.replace('*', "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Studio project's own folder, where the Studio keeps it by default.
pub fn studio_project_dir(root: &Path, project: &str) -> Option<PathBuf> {
    let dir = root.join(STUDIO.folder).join("backend").join("projects").join(project);
    (safe_id(project) && dir.is_dir()).then_some(dir)
}

// ------------------------------------------------------------------ flows

/// Who started a flow, which decides how "not running" is put.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Origin {
    /// The Assistant, or the local API's `/api/call`.
    Agent,
    /// A dialog asking what there is (it shows a Start button).
    Dialog,
    /// A dialog's OK button.
    Menu,
}

/// One flow's work, made ready on the UI thread.
pub(crate) enum Work {
    BuzzEditProjects,
    Send {
        /// The `POST /api/export` body.
        export: Value,
        video: PathBuf,
        project: Option<String>,
    },
    Transcribe {
        sound: String,
        speaker: String,
        file_name: String,
        bytes: Arc<Vec<u8>>,
    },
    StudioProjects,
    StudioBuild {
        project: String,
        asset: String,
        out_dir: Option<PathBuf>,
    },
}

impl Work {
    fn tool(&self) -> &'static str {
        match self {
            Work::BuzzEditProjects => "buzzedit_projects",
            Work::Send { .. } => "send_to_buzzedit",
            Work::Transcribe { .. } => "transcribe_with_buzzedit",
            Work::StudioProjects => "studio_projects",
            Work::StudioBuild { .. } => "studio_build_episode",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Work::BuzzEditProjects => "BuzzEdit: Projects",
            Work::Send { .. } => "BuzzEdit: Send",
            Work::Transcribe { .. } => "BuzzEdit: Transcribe",
            Work::StudioProjects => "Studio: Projects",
            Work::StudioBuild { .. } => "Studio: Build Episode",
        }
    }

    fn menu(&self) -> &'static str {
        match self {
            Work::BuzzEditProjects | Work::Send { .. } => "File \u{25b8} Send to BuzzEdit\u{2026}",
            Work::Transcribe { .. } => "Commands \u{25b8} Transcribe with BuzzEdit\u{2026}",
            Work::StudioProjects | Work::StudioBuild { .. } => "File \u{25b8} Import Script from Studio\u{2026}",
        }
    }

    fn peer(&self, estate: &Estate) -> Peer {
        match self {
            Work::StudioProjects | Work::StudioBuild { .. } => estate.studio.clone(),
            _ => estate.buzzedit.clone(),
        }
    }
}

/// What a flow's thread tells the UI thread.
enum ToUi {
    /// Queue this export and say when it has ended.
    Export { body: Value, done: Sender<Result<(), String>> },
    Done(Result<Answer, Failure>),
}

enum Answer {
    Value(Value),
    /// Words to lay down as captions and lip sync, on the UI thread.
    Transcript {
        value: Value,
        sound: String,
        speaker: String,
        lines: Vec<buzz_ai::transcribe::Segment>,
    },
}

/// The flow's thread, start to end.
fn run(work: Work, estate: &Estate, start: bool, origin: Origin, ctx: &TaskCtx, to_ui: &Sender<ToUi>) -> Result<Answer, Failure> {
    let peer = work.peer(estate);
    let port = match reach(estate, &peer, start, Some(ctx)) {
        Ok(port) => port,
        Err(mut failure) => {
            if failure.not_running {
                match origin {
                    Origin::Agent => failure
                        .message
                        .push_str(&format!("; call {} again with start:true to launch it", work.tool())),
                    Origin::Menu => failure.message.push_str(&format!("; {} offers to start it", work.menu())),
                    Origin::Dialog => {}
                }
            }
            return Err(failure);
        }
    };
    match work {
        Work::BuzzEditProjects => {
            ctx.progress.detail("asking BuzzEdit for its projects");
            Ok(Answer::Value(json!({"projects": buzzedit_projects(port)?, "port": port})))
        }
        Work::StudioProjects => {
            ctx.progress.detail("asking the Studio for its projects");
            Ok(Answer::Value(json!({"projects": studio_projects(port)?, "port": port})))
        }
        Work::Send { export, video, project } => {
            ctx.progress.detail("rendering the MP4");
            let (done, ended) = bounded(1);
            to_ui
                .send(ToUi::Export { body: export, done })
                .map_err(|_| "the app stopped listening".to_string())?;
            loop {
                if ctx.cancelled() {
                    return Err("cancelled; the export carries on in the Tasks panel".to_string().into());
                }
                match ended.recv_timeout(Duration::from_millis(200)) {
                    Ok(Ok(())) => break,
                    Ok(Err(e)) => return Err(format!("the MP4 export failed: {e}").into()),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err("the export was dropped".to_string().into()),
                }
            }
            ctx.progress.detail("handing the MP4 to BuzzEdit");
            let mut value = buzzedit_import(port, &video, project.as_deref())?;
            value["video"] = json!(video.display().to_string());
            Ok(Answer::Value(value))
        }
        Work::Transcribe { sound, speaker, file_name, bytes } => {
            let dir = std::env::temp_dir().join("buzzanimate-buzzedit");
            std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
            let file = dir.join(&file_name);
            std::fs::write(&file, bytes.as_slice()).map_err(|e| format!("writing {}: {e}", file.display()))?;
            let heard = buzzedit_transcribe(port, &file, Some(ctx));
            // BuzzEdit has its own copy by now (or failed without one).
            let _ = std::fs::remove_file(&file);
            let (project, words, language) = heard?;
            let lines = words_to_lines(&words);
            if lines.is_empty() {
                return Err(format!("BuzzEdit heard no words in '{sound}' (its project {project})").into());
            }
            Ok(Answer::Transcript {
                value: json!({"buzzedit_project": project, "words": words.len(), "language": language}),
                sound,
                speaker,
                lines,
            })
        }
        Work::StudioBuild { project, asset, out_dir } => {
            ctx.progress.detail("fetching the script");
            let script = studio_script(port, &project, &asset)?;
            let out_dir = match out_dir {
                Some(dir) => dir,
                None => estate
                    .root
                    .as_deref()
                    .and_then(|root| studio_project_dir(root, &project))
                    .map(|dir| dir.join("animation"))
                    .ok_or_else(|| {
                        format!("the Studio project '{project}' has no folder under BuzzcafStudio/backend/projects here; pass out_dir")
                    })?,
            };
            ctx.progress.detail("building the episode");
            let done = crate::api::build_episode_files(crate::api::EpisodeRequest {
                script: crate::api::ScriptSource::Text(studio_to_screenplay(&script)),
                casting: None,
                scenes: None,
                out_dir: out_dir.clone(),
            })?;
            let files: Vec<String> = done.files.iter().map(|p| p.display().to_string()).collect();
            Ok(Answer::Value(json!({
                "project": project,
                "out_dir": out_dir.display().to_string(),
                "files": files,
                "warnings": done.warnings,
            })))
        }
    }
}

/// One running flow, from the UI thread's side.
struct Flow {
    task: TaskId,
    tool: &'static str,
    origin: Origin,
    rx: Receiver<ToUi>,
    /// The export it is waiting on: the API job, and who to tell.
    export: Option<(u64, Sender<Result<(), String>>)>,
}

/// The app's side of the estate.
#[derive(Default)]
pub struct EstateState {
    /// Resolved on first use (tests put their own here).
    config: Option<Estate>,
    flows: Vec<Flow>,
    dialog: Option<EstateDialog>,
}

impl EstateState {
    /// Is a BuzzEdit or Studio flow still running?
    pub fn busy(&self) -> bool {
        !self.flows.is_empty()
    }
}

/// Which of the three dialogs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DialogKind {
    Send,
    Transcribe,
    Studio,
}

/// What the dialog knows of the other app.
#[derive(Debug, Clone, PartialEq)]
enum Listing {
    Asking(TaskId),
    Starting(TaskId),
    NotRunning(String),
    Failed(String),
    /// `(id, what the list shows)`.
    Ready(Vec<(String, String)>),
}

pub(crate) struct EstateDialog {
    kind: DialogKind,
    listing: Listing,
    /// Send: 0 is a new project, `i + 1` the `i`th listed one. Studio: the
    /// listed project.
    choice: usize,
    whole: bool,
    sound: usize,
    speaker: String,
    out_dir: String,
}

enum DialogAction {
    Start,
    Retry,
    Go,
}

fn sanitize(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' })
        .collect();
    let clean = clean.trim().trim_matches('.').to_string();
    if clean.is_empty() { "untitled".into() } else { clean }
}

/// Beside the document when it has a file, else in Videos/BuzzAnimate.
fn default_video_path(document: Option<&Path>, title: &str) -> PathBuf {
    let stamp = humantime::format_rfc3339_seconds(SystemTime::now()).to_string().replace(':', "-");
    let dir = match document.and_then(Path::parent).filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.to_path_buf(),
        None => std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(|home| PathBuf::from(home).join("Videos").join("BuzzAnimate"))
            .unwrap_or_else(|| std::env::temp_dir().join("BuzzAnimate")),
    };
    dir.join(format!("{} - BuzzEdit {stamp}.mp4", sanitize(title)))
}

impl App {
    fn estate_config(&mut self) -> Estate {
        self.estate.config.get_or_insert_with(Estate::from_env).clone()
    }

    /// Start a flow on a task of its own.
    pub(crate) fn estate_start(&mut self, work: Work, start: bool, origin: Origin) -> TaskId {
        let estate = self.estate_config();
        let (tx, rx) = unbounded();
        let tool = work.tool();
        let label = work.label();
        let task = self.tasks.spawn_thread(TaskKind::Ai, label, move |ctx| {
            let answer = run(work, &estate, start, origin, &ctx, &tx);
            let outcome = match &answer {
                Ok(_) => TaskOutcome::Finished(String::new()),
                Err(_) if ctx.cancelled() => TaskOutcome::Cancelled,
                Err(failure) => TaskOutcome::Failed(failure.message.clone()),
            };
            let _ = tx.send(ToUi::Done(answer));
            outcome
        });
        self.estate.flows.push(Flow { task, tool, origin, rx, export: None });
        task
    }

    /// Send to BuzzEdit's work: the export it asks for and where it goes.
    pub(crate) fn estate_send_work(&mut self, scene_only: bool, project: Option<String>, path: Option<PathBuf>) -> Result<Work, String> {
        if !buzz_export::ffmpeg_available() {
            return Err("Send to BuzzEdit renders an MP4, which needs ffmpeg, and none was found on this machine".into());
        }
        if let Some(id) = project.as_deref() {
            if !safe_id(id) {
                return Err(format!("'{id}' is not a BuzzEdit project id"));
            }
        }
        let doc = &self.agent_editor().doc;
        let (from, to) = if scene_only {
            let active = doc.active_scene();
            if !doc.film_order().contains(&active) {
                return Err("the scene being edited is not part of the film (a setup or an unused take); send the whole document".into());
            }
            let from = doc.film_start_of(active);
            let length = doc.scene().rendered_frame_count().max(1);
            (Some(from), Some(from + length - 1))
        } else {
            (None, None)
        };
        let video = match path {
            Some(p) => {
                if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
                    return Err("'path' is the .mp4 to write".into());
                }
                if p.exists() {
                    return Err(format!("there is already a file at {}; pick another path", p.display()));
                }
                p
            }
            None => {
                let p = default_video_path(doc.path(), &doc.title());
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
                }
                p
            }
        };
        let export = json!({"path": video.display().to_string(), "format": "mp4", "from": from, "to": to});
        Ok(Work::Send { export, video, project })
    }

    /// Transcribe with BuzzEdit's work: the sound's bytes, and who speaks.
    pub(crate) fn estate_transcribe_work(&mut self, sound: &str, speaker: &str) -> Result<Work, String> {
        let speaker = speaker.trim();
        if speaker.is_empty() {
            return Err("'speaker' names who is talking: the mouth symbol to lip-sync".into());
        }
        let scene = self.agent_editor().doc.scene();
        let Some(asset) = scene.sounds().iter().find(|s| s.name == sound).cloned() else {
            let names: Vec<String> = scene.sounds().iter().map(|s| s.name.clone()).collect();
            return Err(format!("there is no sound called '{sound}' in the library; sounds: [{}]", names.join(", ")));
        };
        let stem = Path::new(&asset.name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| asset.name.clone());
        Ok(Work::Transcribe {
            sound: asset.name.clone(),
            speaker: speaker.to_string(),
            file_name: format!("{}.{}", sanitize(&stem), sanitize(&asset.format)),
            bytes: asset.data.clone(),
        })
    }

    /// Once a frame (and before the Assistant reads its jobs): queue the
    /// exports flows ask for, tell them when those end, and land what
    /// finished flows produced.
    pub(crate) fn estate_pump(&mut self) {
        let mut i = 0;
        while i < self.estate.flows.len() {
            let mut finished = None;
            let mut gone = false;
            loop {
                match self.estate.flows[i].rx.try_recv() {
                    Ok(ToUi::Export { body, done }) => match self.api_queue_export(&body) {
                        Ok(job) => self.estate.flows[i].export = Some((job, done)),
                        Err(e) => {
                            let _ = done.send(Err(e));
                        }
                    },
                    Ok(ToUi::Done(result)) => {
                        finished = Some(result);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        gone = true;
                        break;
                    }
                }
            }
            let waiting = self.estate.flows[i].export.as_ref().map(|(job, _)| *job);
            if let Some(job) = waiting {
                let state = self.api_job_state(job);
                let ended = match state.as_ref().and_then(|s| s.get("state")).and_then(Value::as_str) {
                    Some("done") => Some(Ok(())),
                    Some("failed") => Some(Err(state
                        .as_ref()
                        .and_then(|s| s.get("error"))
                        .and_then(Value::as_str)
                        .unwrap_or("the export failed")
                        .to_string())),
                    None => Some(Err("the export job is gone".to_string())),
                    _ => None,
                };
                if let Some(ended) = ended {
                    if let Some((_, done)) = self.estate.flows[i].export.take() {
                        let _ = done.send(ended);
                    }
                }
            }
            if let Some(result) = finished {
                let flow = self.estate.flows.remove(i);
                self.estate_finish(flow, result);
                continue;
            }
            if gone {
                // Ended with no answer: `poll_tasks` reports how.
                self.estate.flows.remove(i);
                continue;
            }
            i += 1;
        }
    }

    fn estate_settle_job(&mut self, task: TaskId, state: JobState) {
        if let Some(job) = self.agent_jobs.iter_mut().find(|j| j.task == task) {
            job.state = state;
            job.delivered = false;
        }
    }

    fn estate_finish(&mut self, flow: Flow, result: Result<Answer, Failure>) {
        let listed = matches!(flow.tool, "buzzedit_projects" | "studio_projects");
        // A dialog waiting on this flow's list takes the answer.
        if let Some(dialog) = self.estate.dialog.as_mut() {
            if matches!(dialog.listing, Listing::Asking(t) | Listing::Starting(t) if t == flow.task) {
                dialog.listing = match &result {
                    Ok(Answer::Value(v)) => Listing::Ready(listing_items(flow.tool, v)),
                    Ok(Answer::Transcript { .. }) => Listing::Ready(Vec::new()),
                    Err(f) if f.not_running => Listing::NotRunning(f.message.clone()),
                    Err(f) => Listing::Failed(f.message.clone()),
                };
            }
        }
        match result {
            Err(failure) => {
                if !(listed && flow.origin == Origin::Dialog && failure.not_running) {
                    self.agent_editor_mut().status = Some(failure.message.clone());
                }
                self.estate_settle_job(flow.task, JobState::Failed(failure.message));
            }
            Ok(Answer::Value(mut value)) => {
                let status = match flow.tool {
                    "send_to_buzzedit" => {
                        let id = value["project_id"].as_str().unwrap_or("?");
                        if value["new_project"] == true {
                            let name = value["project_name"].as_str().unwrap_or("");
                            format!("Sent to BuzzEdit: new project '{name}' ({id})")
                        } else {
                            format!("Sent to BuzzEdit: added to project {id}")
                        }
                    }
                    "studio_build_episode" => {
                        let buzz = value["files"]
                            .as_array()
                            .and_then(|f| f.iter().filter_map(Value::as_str).find(|f| f.ends_with(".buzz")))
                            .unwrap_or("")
                            .to_string();
                        let warnings = value["warnings"].as_array().map(Vec::len).unwrap_or(0);
                        format!("Episode built from the Studio's script: {buzz} ({warnings} warning(s) in report.md)")
                    }
                    "buzzedit_projects" => {
                        format!("BuzzEdit: {} project(s)", value["projects"].as_array().map(Vec::len).unwrap_or(0))
                    }
                    _ => format!("Buzzcaf Studio: {} project(s)", value["projects"].as_array().map(Vec::len).unwrap_or(0)),
                };
                if !(listed && flow.origin == Origin::Dialog) {
                    self.agent_editor_mut().status = Some(status.clone());
                }
                if let Some(map) = value.as_object_mut() {
                    map.remove("port");
                    map.insert("status".into(), json!(status));
                }
                self.estate_settle_job(flow.task, JobState::Done(value));
            }
            Ok(Answer::Transcript { mut value, sound, speaker, lines }) => match self.estate_lay_transcript(&sound, &speaker, &lines) {
                Ok((captions, keys)) => {
                    let lip = self.agent_editor_mut().status.take().unwrap_or_default();
                    let status = format!(
                        "{captions} caption(s) from '{sound}' for {speaker}, heard by BuzzEdit; {keys} lip-sync key(s). {lip}"
                    );
                    self.agent_editor_mut().status = Some(status.clone());
                    value["captions"] = json!(captions);
                    value["lip_sync_keys"] = json!(keys);
                    value["status"] = json!(status);
                    self.estate_settle_job(flow.task, JobState::Done(value));
                }
                Err(e) => {
                    let message = format!("BuzzEdit heard '{sound}', but its words could not be laid down: {e}");
                    self.agent_editor_mut().status = Some(message.clone());
                    self.estate_settle_job(flow.task, JobState::Failed(message));
                }
            },
        }
    }

    /// Lines in the sound's own seconds → captions and lip sync where the
    /// sound plays on the stage (its first frame, less any trimmed in-point).
    fn estate_lay_transcript(
        &mut self,
        sound: &str,
        speaker: &str,
        lines: &[buzz_ai::transcribe::Segment],
    ) -> Result<(usize, usize), String> {
        let scene = self.agent_editor().doc.scene();
        let fps = scene.stage().frame_rate.max(1.0);
        let id = scene.sounds().iter().find(|s| s.name == sound).map(|s| s.id);
        let cue = id.and_then(|id| scene.stage_cues().into_iter().find(|c| c.sound == id));
        let (start_frame, trim) = cue.map(|c| (c.start_frame, c.trim_start as f64)).unwrap_or((0, 0.0));
        let shifted: Vec<buzz_ai::transcribe::Segment> = lines
            .iter()
            .filter(|s| s.end > trim)
            .map(|s| buzz_ai::transcribe::Segment {
                start: (s.start - trim).max(0.0),
                end: s.end - trim,
                text: s.text.clone(),
            })
            .collect();
        let cues = buzz_ai::transcribe::to_dialogue_cues(&shifted, speaker, fps, start_frame);
        let keys = self.agent_editor_mut().apply_transcript(sound, &cues).map_err(|e| e.to_string())?;
        Ok((cues.len(), keys))
    }

    // ------------------------------------------------------------------ the dialog

    /// The menu's way in: a dialog that first asks the other app what there is.
    pub(crate) fn estate_open_dialog(&mut self, kind: DialogKind) {
        let work = match kind {
            DialogKind::Studio => Work::StudioProjects,
            DialogKind::Send | DialogKind::Transcribe => Work::BuzzEditProjects,
        };
        let task = self.estate_start(work, false, Origin::Dialog);
        self.estate.dialog = Some(EstateDialog {
            kind,
            listing: Listing::Asking(task),
            choice: 0,
            whole: true,
            sound: 0,
            speaker: String::new(),
            out_dir: String::new(),
        });
    }

    /// The dialog, if one is open.
    pub(crate) fn estate_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.estate.dialog.take() else {
            return;
        };
        let peer = if d.kind == DialogKind::Studio { STUDIO.label } else { BUZZEDIT.label };
        let title = match d.kind {
            DialogKind::Send => "Send to BuzzEdit",
            DialogKind::Transcribe => "Transcribe with BuzzEdit",
            DialogKind::Studio => "Import Script from Studio",
        };
        let sounds: Vec<String> = self.agent_editor().doc.scene().sounds().iter().map(|s| s.name.clone()).collect();
        let mut open = true;
        let mut action = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| match &d.listing {
                Listing::Asking(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Asking {peer}\u{2026}"));
                    });
                }
                Listing::Starting(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Starting {peer}\u{2026} this can take a minute or two."));
                    });
                }
                Listing::NotRunning(_) => {
                    ui.label(format!("{peer} is not running."));
                    if ui.button(format!("Start {peer}")).clicked() {
                        action = Some(DialogAction::Start);
                    }
                }
                Listing::Failed(why) => {
                    ui.label(why.as_str());
                    if ui.button("Try Again").clicked() {
                        action = Some(DialogAction::Retry);
                    }
                }
                Listing::Ready(items) => {
                    let items = items.clone();
                    match d.kind {
                        DialogKind::Send => {
                            ui.radio_value(&mut d.whole, true, "The whole document");
                            ui.radio_value(&mut d.whole, false, "The scene being edited");
                            let shown = match d.choice {
                                0 => "A new BuzzEdit project".to_string(),
                                n => items.get(n - 1).map(|i| i.1.clone()).unwrap_or_default(),
                            };
                            egui::ComboBox::from_id_salt("estate_send_project")
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut d.choice, 0, "A new BuzzEdit project");
                                    for (n, (_, label)) in items.iter().enumerate() {
                                        ui.selectable_value(&mut d.choice, n + 1, label.as_str());
                                    }
                                });
                            if ui.button("Send").clicked() {
                                action = Some(DialogAction::Go);
                            }
                        }
                        DialogKind::Transcribe => {
                            if sounds.is_empty() {
                                ui.label("The library has no sounds: File \u{25b8} Import Sound first.");
                            } else {
                                egui::ComboBox::from_id_salt("estate_transcribe_sound")
                                    .selected_text(sounds.get(d.sound).cloned().unwrap_or_default())
                                    .show_ui(ui, |ui| {
                                        for (n, name) in sounds.iter().enumerate() {
                                            ui.selectable_value(&mut d.sound, n, name.as_str());
                                        }
                                    });
                                ui.horizontal(|ui| {
                                    ui.label("Speaker");
                                    ui.text_edit_singleline(&mut d.speaker);
                                });
                                let ready = d.sound < sounds.len() && !d.speaker.trim().is_empty();
                                if ui.add_enabled(ready, egui::Button::new("Transcribe")).clicked() {
                                    action = Some(DialogAction::Go);
                                }
                            }
                        }
                        DialogKind::Studio => {
                            if items.is_empty() {
                                ui.label("The Studio has no projects yet.");
                            } else {
                                egui::ComboBox::from_id_salt("estate_studio_project")
                                    .selected_text(items.get(d.choice).map(|i| i.1.clone()).unwrap_or_default())
                                    .show_ui(ui, |ui| {
                                        for (n, (_, label)) in items.iter().enumerate() {
                                            ui.selectable_value(&mut d.choice, n, label.as_str());
                                        }
                                    });
                                ui.horizontal(|ui| {
                                    ui.label("Into");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut d.out_dir)
                                            .hint_text("the Studio project's animation folder"),
                                    );
                                });
                                if ui.add_enabled(d.choice < items.len(), egui::Button::new("Build Episode")).clicked() {
                                    action = Some(DialogAction::Go);
                                }
                            }
                        }
                    }
                }
            });
        let mut keep = open;
        match action {
            None => {}
            Some(DialogAction::Start) | Some(DialogAction::Retry) => {
                let start = matches!(action, Some(DialogAction::Start));
                let work = if d.kind == DialogKind::Studio { Work::StudioProjects } else { Work::BuzzEditProjects };
                let task = self.estate_start(work, start, Origin::Dialog);
                d.listing = if start { Listing::Starting(task) } else { Listing::Asking(task) };
            }
            Some(DialogAction::Go) => {
                let items = match &d.listing {
                    Listing::Ready(items) => items.clone(),
                    _ => Vec::new(),
                };
                let work = match d.kind {
                    DialogKind::Send => {
                        let project = d.choice.checked_sub(1).and_then(|n| items.get(n)).map(|i| i.0.clone());
                        self.estate_send_work(!d.whole, project, None)
                    }
                    DialogKind::Transcribe => match sounds.get(d.sound) {
                        Some(sound) => self.estate_transcribe_work(sound, &d.speaker),
                        None => Err("pick a sound".into()),
                    },
                    DialogKind::Studio => match items.get(d.choice) {
                        Some((id, _)) => Ok(Work::StudioBuild {
                            project: id.clone(),
                            asset: "script".into(),
                            out_dir: Some(d.out_dir.trim()).filter(|s| !s.is_empty()).map(PathBuf::from),
                        }),
                        None => Err("pick a project".into()),
                    },
                };
                match work {
                    Ok(work) => {
                        let label = work.label();
                        self.estate_start(work, false, Origin::Menu);
                        self.agent_editor_mut().status = Some(format!("{label}: started (see the Tasks panel)"));
                        keep = false;
                    }
                    Err(e) => self.agent_editor_mut().status = Some(e),
                }
            }
        }
        if keep {
            self.estate.dialog = Some(d);
        }
    }
}

/// A project list as the dialog shows it.
fn listing_items(tool: &str, value: &Value) -> Vec<(String, String)> {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    value["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_str)?.to_string();
            let name = text(p, "name");
            let label = if tool == "studio_projects" {
                let script = if p["has_script"] == true { "" } else { ", no script yet" };
                format!("{name} ({}, {}{script})", text(p, "brand"), text(p, "current_step"))
            } else {
                format!("{name} ({})", text(p, "status"))
            };
            Some((id, label))
        })
        .collect()
}

// ------------------------------------------------------------------ agent tools

fn refuse(error: impl Into<String>) -> AgentResult {
    AgentResult { ok: false, value: Value::Null, error: Some(error.into()) }
}

/// `send_to_buzzedit`, `buzzedit_projects`, `transcribe_with_buzzedit`,
/// `studio_projects`, `studio_build_episode`: each starts a flow and answers
/// `{started, job}`; `wait_for_jobs` gives the result.
pub fn tool(app: &mut App, name: &str, args: &Value) -> AgentResult {
    let allowed: &[&str] = match name {
        "send_to_buzzedit" => &["scope", "project", "path", "start"],
        "transcribe_with_buzzedit" => &["sound", "speaker", "start"],
        "studio_build_episode" => &["project", "asset", "out_dir", "start"],
        _ => &["start"],
    };
    if let Some(map) = args.as_object() {
        for key in map.keys() {
            if !allowed.contains(&key.as_str()) {
                return refuse(format!("unknown arg '{key}'; valid: {}", allowed.join(", ")));
            }
        }
    } else if !args.is_null() {
        return refuse("args is an object");
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match args.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => match v.as_str().map(str::trim) {
                Some(s) if !s.is_empty() => Ok(Some(s.to_string())),
                _ => Err(format!("'{key}' needs text")),
            },
        }
    };
    let start = match args.get("start").filter(|v| !v.is_null()) {
        None => false,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => return refuse("'start' is true or false"),
        },
    };
    let work = (|| -> Result<Work, String> {
        Ok(match name {
            "buzzedit_projects" => Work::BuzzEditProjects,
            "studio_projects" => Work::StudioProjects,
            "send_to_buzzedit" => {
                let scene_only = match text("scope")?.as_deref() {
                    None | Some("document") => false,
                    Some("scene") => true,
                    Some(other) => return Err(format!("'scope' is document or scene, not '{other}'")),
                };
                app.estate_send_work(scene_only, text("project")?, text("path")?.map(PathBuf::from))?
            }
            "transcribe_with_buzzedit" => {
                let sound = text("sound")?.ok_or("missing required arg: sound (a sound in the library)")?;
                let speaker = text("speaker")?.ok_or("missing required arg: speaker (who is talking)")?;
                app.estate_transcribe_work(&sound, &speaker)?
            }
            "studio_build_episode" => {
                let project = text("project")?.ok_or("missing required arg: project (an id from studio_projects)")?;
                if !safe_id(&project) {
                    return Err(format!("'{project}' is not a Studio project id"));
                }
                let asset = text("asset")?.unwrap_or_else(|| "script".into());
                if !safe_id(&asset) {
                    return Err(format!("'{asset}' is not an asset name"));
                }
                Work::StudioBuild { project, asset, out_dir: text("out_dir")?.map(PathBuf::from) }
            }
            other => return Err(format!("unknown tool '{other}'")),
        })
    })();
    let work = match work {
        Ok(work) => work,
        Err(e) => return refuse(e),
    };
    let what = match &work {
        Work::BuzzEditProjects => "BuzzEdit is being asked for its projects".to_string(),
        Work::StudioProjects => "Buzzcaf Studio is being asked for its projects".to_string(),
        Work::Send { video, .. } => format!("the MP4 {} is being rendered and sent to BuzzEdit", video.display()),
        Work::Transcribe { sound, .. } => format!("BuzzEdit is transcribing '{sound}'"),
        Work::StudioBuild { project, .. } => format!("the Studio project '{project}' is being built into an episode"),
    };
    let task = app.estate_start(work, start, Origin::Agent);
    app.agent_jobs.push(AgentJob {
        id: task.0,
        kind: name.to_string(),
        task,
        state: JobState::Running,
        delivered: false,
    });
    AgentResult {
        ok: true,
        value: crate::agent::jobs::started(name, task.0, &what),
        error: None,
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod studio_script_tests {
    use super::studio_to_screenplay;
    use buzz_act::screenplay::{Item, Screenplay};

    fn lines(play: &Screenplay) -> Vec<(u32, String, String)> {
        play.scenes
            .iter()
            .flat_map(|s| {
                s.items.iter().filter_map(move |i| match i {
                    Item::Dialogue(d) => Some((s.number, d.speaker.clone(), d.text.clone())),
                    _ => None,
                })
            })
            .collect()
    }

    /// The three shapes the Studio really writes (abridged from projects
    /// under `BuzzcafStudio/backend/projects/*/script/`), each read by the
    /// screenplay parser into scenes the narrator speaks.
    #[test]
    fn every_studio_script_shape_becomes_scenes_the_narrator_speaks() {
        let draft = "# script/draft.md\n\n---\n\n## 🎬 Project: The Art of Starting Small\n### 🗣️ Language: Hinglish\n\n---\n\n\
            ## [SCENE 01: THE HOOK - THE OVERWHELM]\n**VISUAL:** [Fast cuts over a desk of half-finished sketches.]\n\
            **TEXT OVERLAY:** *“Why are we scared to start?”*\n\n\
            **V.O. (Narrator):** Hum sab ke paas bade sapne hote hain, right? Par aksar... hum *darte* hain.\n\n\
            [Visual: Narrator looks into the camera with a sigh.]\n\n---\n\n\
            ## [SCENE 02: THE BIG TRAP]\n**SOUND:** A clock ticking.\n\n**V.O. (Narrator):** Ek baar mere saath bhi aisa hua tha.\n";
        let play = Screenplay::parse(&studio_to_screenplay(draft));
        assert_eq!(play.scenes.len(), 2, "{play:#?}");
        assert_eq!(play.scenes[0].title, "THE HOOK - THE OVERWHELM");
        let said = lines(&play);
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said.iter().all(|(_, who, _)| who == "NARRATOR"));
        assert_eq!(said[0].2, "Hum sab ke paas bade sapne hote hain, right? Par aksar... hum darte hain.");
        assert!(
            play.scenes[0].items.iter().any(|i| matches!(i, Item::Action(a) if a.contains("half-finished sketches"))),
            "direction is kept as action"
        );

        let buzzedit = "# THE HOOK - THE OVERWhelm\n\n\
            [broll: A wide shot of a hilltop, photoreal] Hum sab ke paas bade -bade sapne hote hain, [pills: Build a Brand; Create Masterpiece] apne ek brand banayenge. [fx: push intensity=0.8 dur=0.4] lekin aksar...\n\n\
            # THE BIG TRAP\n\nmere saath bhi ek baar [canvas: Hands typing | My Story] aisa hua tha.\n";
        let play = Screenplay::parse(&studio_to_screenplay(buzzedit));
        assert_eq!(play.scenes.len(), 2);
        let said = lines(&play);
        assert_eq!(said[0].2, "Hum sab ke paas bade -bade sapne hote hain, apne ek brand banayenge. lekin aksar...");
        assert_eq!(said[1], (2, "NARRATOR".into(), "mere saath bhi ek baar aisa hua tha.".into()));
        assert!(said.iter().all(|(_, _, t)| !t.contains('[') && !t.contains(']')), "no directive is spoken");

        let short = "# Kentucky ke 'Green Men' 👽\n\n\
            August 20, 1955. Kentucky ke ek farmhouse par... maut ka sannata tha. (pause) Tabhi kuch ajeeb hua. (normal)\n\n\
            Skeptics ise prank kehte hain (ya hallucination). (slow) Aapko kya lagta hai? (beat)\n";
        let play = Screenplay::parse(&studio_to_screenplay(short));
        assert_eq!(play.scenes.len(), 1);
        assert_eq!(play.scenes[0].title, "Kentucky ke 'Green Men' 👽");
        let said: Vec<String> = lines(&play).into_iter().map(|(_, _, t)| t).collect();
        assert_eq!(
            said,
            vec![
                "August 20, 1955. Kentucky ke ek farmhouse par... maut ka sannata tha.".to_string(),
                "Tabhi kuch ajeeb hua.".into(),
                "Skeptics ise prank kehte hain (ya hallucination). Aapko kya lagta hai?".into(),
            ],
            "(pause) splits the line, (normal)/(slow) go, words in brackets stay"
        );
        assert!(
            play.scenes[0].items.iter().any(|i| matches!(i, Item::Pause(_))),
            "a (pause) is a pause"
        );
    }

    /// A Studio project whose script is already a screenplay is left alone.
    #[test]
    fn a_screenplay_passes_through_untouched() {
        let play = "## SCENE 1 \u{2014} TEST\n\n**GUDDU**\nHello there.\n";
        assert_eq!(studio_to_screenplay(play), play);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_render::GpuPreference;
    use std::sync::Mutex;

    type Log = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// A tiny stand-in for BuzzEdit or the Studio: every request is logged
    /// and answered by `handler`.
    struct Fake {
        port: u16,
        log: Log,
        server: Arc<tiny_http::Server>,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.server.unblock();
        }
    }

    fn fake(handler: impl Fn(&str, &str, &Value) -> (u16, Value) + Send + 'static) -> Fake {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("a local port"));
        let port = server.server_addr().to_ip().unwrap().port();
        let log: Log = Arc::default();
        let (s, l) = (server.clone(), log.clone());
        std::thread::spawn(move || {
            for mut request in s.incoming_requests() {
                let mut raw = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut raw);
                let body: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                let method = request.method().to_string();
                let path = request.url().to_string();
                let (status, answer) = handler(&method, &path, &body);
                l.lock().unwrap().push((method, path, body));
                let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let response = tiny_http::Response::from_string(answer.to_string())
                    .with_status_code(status)
                    .with_header(header);
                let _ = request.respond(response);
            }
        });
        Fake { port, log, server }
    }

    fn requests(fake: &Fake) -> Vec<(String, String, Value)> {
        fake.log.lock().unwrap().clone()
    }

    fn closed_port() -> u16 {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buzzanimate-estate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An estate of its own: a temp ledger, no root unless given, and each
    /// peer looked for on one port only — never the machine's real apps.
    fn estate(dir: &Path, buzzedit: u16, studio: u16) -> Estate {
        Estate {
            ledger: dir.join("Buzzcaf").join("ports.json"),
            root: None,
            buzzedit: Peer { preferred: buzzedit, span: 0, ..BUZZEDIT },
            studio: Peer { preferred: studio, span: 0, ..STUDIO },
            start_wait: Duration::from_secs(2),
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

    /// Frames until the job has ended; its state.
    fn wait(app: &mut App, ctx: &egui::Context, job: u64, limit: Duration) -> JobState {
        let began = Instant::now();
        loop {
            frame(app, ctx);
            app.agent_poll_tasks();
            let state = app.agent_jobs.iter().find(|j| j.id == job).map(|j| j.state.clone()).expect("the job");
            if state != JobState::Running && !app.estate.busy() {
                return state;
            }
            assert!(began.elapsed() < limit, "job {job} did not end");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn started(result: &AgentResult) -> u64 {
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.value["started"], true);
        result.value["job"].as_u64().unwrap()
    }

    fn health(app: &'static str) -> impl Fn(&str, &str, &Value) -> (u16, Value) + Send + 'static {
        move |_, path, _| match path {
            "/health" | "/api/health" => (200, json!({"app": app, "status": "ok"})),
            _ => (404, json!({"detail": "Not Found"})),
        }
    }

    #[test]
    fn an_app_is_known_by_its_name_not_by_its_port() {
        let dir = temp("identity");
        // The Studio sits where BuzzEdit's ledger entry says BuzzEdit is.
        let studio = fake(health("buzzcaf"));
        let mut e = estate(&dir, studio.port, studio.port);
        ports::publish_at(&e.ledger, "buzzedit", studio.port, "", Default::default());
        assert_eq!(discover(&e, &e.buzzedit), None, "a buzzcaf health on BuzzEdit's port is not BuzzEdit");
        assert_eq!(discover(&e, &e.studio), Some(studio.port), "found by the scan, by name");

        // BuzzEdit, found through its ledger entry on a port it does not prefer.
        let buzzedit = fake(health("buzzedit"));
        ports::publish_at(&e.ledger, "buzzedit", buzzedit.port, "", Default::default());
        e.buzzedit.preferred = closed_port();
        assert_eq!(discover(&e, &e.buzzedit), Some(buzzedit.port));

        // Nobody there: said so, and a start needs a launcher.
        let nobody = estate(&dir.join("none"), closed_port(), closed_port());
        let failure = reach(&nobody, &nobody.buzzedit, false, None).unwrap_err();
        assert!(failure.not_running && failure.message.contains("BuzzEdit is not running"), "{failure:?}");
        let mut rooted = nobody.clone();
        rooted.root = Some(dir.clone());
        let failure = reach(&rooted, &rooted.buzzedit, true, None).unwrap_err();
        assert!(!failure.not_running && failure.message.contains("START_APP.vbs"), "{failure:?}");

        assert_eq!(root_above(&dir.join("BuzzAnimate").join("target")), None);
        std::fs::create_dir_all(dir.join("BuzzEdit")).unwrap();
        assert_eq!(root_above(&dir.join("BuzzAnimate").join("target")), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_to_buzzedit_makes_a_project_or_adds_to_one() {
        let dir = temp("send");
        let buzzedit = fake(|method, path, _| match (method, path) {
            (_, "/api/health") => (200, json!({"app": "buzzedit"})),
            ("POST", "/api/projects/import_path") => (200, json!({"id": "ab12cd34", "name": "film.mp4"})),
            ("POST", "/api/projects/ab12cd34/media/import") => (200, json!({"status": "success", "added": [{"id": "m1"}]})),
            ("GET", "/api/projects/list") => (200, json!({"projects": [{"id": "ab12cd34", "name": "film.mp4", "status": "draft"}]})),
            _ => (404, json!({"detail": "Not Found"})),
        });
        let video = dir.join("film.mp4");

        // The call shapes, new and existing.
        let made = buzzedit_import(buzzedit.port, &video, None).unwrap();
        assert_eq!(made["project_id"], "ab12cd34");
        assert_eq!(made["new_project"], true);
        let added = buzzedit_import(buzzedit.port, &video, Some("ab12cd34")).unwrap();
        assert_eq!(added["new_project"], false);
        assert_eq!(added["added"], 1);
        assert!(buzzedit_import(buzzedit.port, &video, Some("../etc")).is_err());
        let log = requests(&buzzedit);
        assert_eq!(log[0].1, "/api/projects/import_path");
        assert_eq!(log[0].2, json!({"path": video.display().to_string()}));
        assert_eq!(log[1].1, "/api/projects/ab12cd34/media/import");
        assert_eq!(log[1].2, json!({"paths": [video.display().to_string()], "copy": true}));
        assert_eq!(log.len(), 2, "a bad id never reaches BuzzEdit");
        assert_eq!(buzzedit_projects(buzzedit.port).unwrap()[0]["id"], "ab12cd34");

        // The whole flow through the agent tool: the export queue, then BuzzEdit.
        if buzz_export::ffmpeg_available() {
            let mut app = App::new(GpuPreference::Automatic);
            app.estate.config = Some(estate(&dir, buzzedit.port, closed_port()));
            let ctx = egui::Context::default();
            let out = dir.join("sent.mp4");
            let r = tool(
                &mut app,
                "send_to_buzzedit",
                &json!({"path": out.display().to_string(), "project": "ab12cd34"}),
            );
            let job = started(&r);
            let state = wait(&mut app, &ctx, job, Duration::from_secs(180));
            let JobState::Done(value) = state else { panic!("{state:?}") };
            assert_eq!(value["project_id"], "ab12cd34", "{value}");
            assert!(out.is_file(), "the MP4 was written");
            let last = requests(&buzzedit).last().cloned().unwrap();
            assert_eq!(last.1, "/api/projects/ab12cd34/media/import");
            assert_eq!(last.2["paths"][0], out.display().to_string());
            assert!(app.agent_editor().status.as_deref().unwrap_or("").contains("Sent to BuzzEdit"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_become_lines_at_pauses_and_sentence_ends() {
        let w = |a: f64, b: f64, t: &str| (a, b, t.to_string());
        let lines = words_to_lines(&[
            w(0.0, 0.3, "Hello"),
            w(0.35, 0.8, "there."),
            w(0.9, 1.1, "How"),
            w(1.15, 1.3, "are"),
            w(2.5, 2.9, "you"),
        ]);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, ["Hello there.", "How are", "you"]);
        assert_eq!((lines[0].start, lines[0].end), (0.0, 0.8));
    }

    fn wav(seconds: f64) -> Vec<u8> {
        let rate = 44_100.0_f64;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut out, spec).unwrap();
            for i in 0..(seconds * rate) as usize {
                let t = i as f64 / rate;
                let v = (t * 180.0 * std::f64::consts::TAU).sin() * 0.5
                    + (t * (700.0 + 400.0 * (t * 3.0).sin()) * std::f64::consts::TAU).sin() * 0.3;
                writer.write_sample((v * 26_000.0) as i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn a_buzzedit_transcript_becomes_captions_and_lip_sync() {
        let dir = temp("transcribe");
        let buzzedit = fake(|method, path, _| match (method, path) {
            (_, "/api/health") => (200, json!({"app": "buzzedit"})),
            ("POST", "/api/projects/import_path") => (200, json!({"id": "tr000001", "name": "vo.wav"})),
            ("POST", "/api/transcription/transcribe") => (200, json!({"status": "completed", "job_id": "trans_tr000001"})),
            ("GET", "/api/transcription/status/trans_tr000001") => (200, json!({"status": "running", "stage": "transcribing"})),
            ("GET", "/api/transcription/tr000001/transcript") => (
                200,
                json!({"language": "en", "duration": 2.6, "segments": [{"id": 0, "start": 0.2, "end": 2.6, "text": "Hello there. How are you?", "words": [
                    {"word": "Hello", "start": 0.2, "end": 0.5},
                    {"word": "there.", "start": 0.55, "end": 0.9},
                    {"word": "How", "start": 1.8, "end": 2.0},
                    {"word": "are", "start": 2.05, "end": 2.2},
                    {"word": "you?", "start": 2.25, "end": 2.6},
                ]}]}),
            ),
            _ => (404, json!({"detail": "Not Found"})),
        });
        let mut app = App::new(GpuPreference::Automatic);
        app.estate.config = Some(estate(&dir, buzzedit.port, closed_port()));
        let path = dir.join("vo.wav");
        std::fs::write(&path, wav(3.0)).unwrap();
        let sound = app.agent_editor_mut().import_sound(&path).expect("the sound imports");
        app.agent_editor_mut().doc.edit("Mouth", |scene| {
            buzz_act::lipsync::placeholder_mouth(scene, "Ana");
        });

        // Arguments are checked before anything starts.
        assert!(!tool(&mut app, "transcribe_with_buzzedit", &json!({"sound": sound})).ok, "a speaker is required");
        assert!(!tool(&mut app, "transcribe_with_buzzedit", &json!({"sound": "nope", "speaker": "Ana"})).ok);

        let ctx = egui::Context::default();
        let job = started(&tool(&mut app, "transcribe_with_buzzedit", &json!({"sound": sound, "speaker": "Ana"})));
        let state = wait(&mut app, &ctx, job, Duration::from_secs(60));
        let JobState::Done(value) = state else { panic!("{state:?}") };
        assert_eq!(value["buzzedit_project"], "tr000001", "{value}");
        assert_eq!(value["words"], 5);
        assert_eq!(value["captions"], 2, "two lines: {value}");
        assert!(value["lip_sync_keys"].as_u64().unwrap_or(0) > 0, "Ana's mouth was keyed: {value}");

        // BuzzEdit was asked for every word as heard.
        let log = requests(&buzzedit);
        let asked = log.iter().find(|r| r.1 == "/api/transcription/transcribe").expect("transcribed");
        assert_eq!(asked.2, json!({"project_id": "tr000001", "precut": true}));
        let sent = log.iter().find(|r| r.1 == "/api/projects/import_path").unwrap();
        assert!(sent.2["path"].as_str().unwrap().ends_with("vo.wav"), "{:?}", sent.2);

        let scene = app.agent_editor().doc.scene();
        let names: Vec<String> = scene.layers().iter().map(|l| l.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Captions"), "{names:?}");
        assert!(names.iter().any(|n| n == "Ana"), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_studio_script_becomes_an_episode_in_the_project_folder() {
        let dir = temp("studio");
        let studio = fake(|method, path, _| match (method, path) {
            (_, "/health") => (200, json!({"app": "buzzcaf", "status": "ok"})),
            ("GET", "/api/projects") => (
                200,
                json!([{"id": "ep1", "name": "Episode 1", "brand": "Chirailiya", "status": "active", "current_step": "script", "assets": {"script": "script/script.md"}}]),
            ),
            ("GET", "/api/projects/ep1/asset/script") => (200, json!({"content": "## SCENE 1 — TEST\n\n**GUDDU**\nHello there.\n"})),
            _ => (404, json!({"detail": "Project not found"})),
        });
        let folder = dir.join("BuzzcafStudio").join("backend").join("projects").join("ep1");
        std::fs::create_dir_all(&folder).unwrap();
        let mut app = App::new(GpuPreference::Automatic);
        let mut e = estate(&dir, closed_port(), studio.port);
        e.root = Some(dir.clone());
        app.estate.config = Some(e);
        let ctx = egui::Context::default();

        let job = started(&tool(&mut app, "studio_projects", &json!({})));
        let JobState::Done(list) = wait(&mut app, &ctx, job, Duration::from_secs(30)) else { panic!() };
        assert_eq!(list["projects"][0]["id"], "ep1", "{list}");
        assert_eq!(list["projects"][0]["has_script"], true);

        let job = started(&tool(&mut app, "studio_build_episode", &json!({"project": "ep1"})));
        let state = wait(&mut app, &ctx, job, Duration::from_secs(120));
        let JobState::Done(built) = state else { panic!("{state:?}") };
        let buzz = built["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .find(|f| f.ends_with(".buzz"))
            .expect("a .buzz")
            .to_string();
        assert!(Path::new(&buzz).is_file());
        assert!(Path::new(&buzz).starts_with(folder.join("animation")), "{buzz}");

        // A project with no script says so.
        let job = started(&tool(&mut app, "studio_build_episode", &json!({"project": "ep2"})));
        let JobState::Failed(why) = wait(&mut app, &ctx, job, Duration::from_secs(30)) else { panic!() };
        assert!(why.contains("404"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_that_is_not_running_is_said_so_and_offered_a_start() {
        let dir = temp("down");
        let mut app = App::new(GpuPreference::Automatic);
        app.estate.config = Some(estate(&dir, closed_port(), closed_port()));
        let ctx = egui::Context::default();

        // The agent is told how to start it.
        let job = started(&tool(&mut app, "studio_projects", &json!({})));
        let JobState::Failed(why) = wait(&mut app, &ctx, job, Duration::from_secs(30)) else { panic!() };
        assert!(why.contains("Buzzcaf Studio is not running") && why.contains("start:true"), "{why}");
        let job = started(&tool(&mut app, "buzzedit_projects", &json!({"start": true})));
        let JobState::Failed(why) = wait(&mut app, &ctx, job, Duration::from_secs(30)) else { panic!() };
        assert!(why.contains("cannot be started"), "no estate root, no launcher: {why}");

        // The menu opens a dialog that asks, finds nobody, and offers Start.
        app.dispatch(buzz_ui::Command::SendToBuzzEdit);
        assert!(app.estate.dialog.is_some());
        let began = Instant::now();
        while app.estate.dialog.as_ref().is_some_and(|d| matches!(d.listing, Listing::Asking(_))) {
            frame(&mut app, &ctx);
            assert!(began.elapsed() < Duration::from_secs(30), "the dialog never heard back");
            std::thread::sleep(Duration::from_millis(20));
        }
        let listing = app.estate.dialog.as_ref().unwrap().listing.clone();
        assert!(matches!(&listing, Listing::NotRunning(m) if m.contains("BuzzEdit is not running")), "{listing:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
