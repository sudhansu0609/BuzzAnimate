//! Stage H of `docs/AI_AGENT_PLAN.md`: the Assistant's view of the AI jobs it
//! starts, and the two tools that ride on them.
//!
//! The AI commands (`ai_rig_vector_character`, `ai_direct_story`,
//! `ai_character_from_image`, Scene from Image, and `transcribe`) all run off
//! the UI thread on the app's own task registry, so a person sees the same
//! progress in the Tasks panel. Each used to answer `{started:true}` and leave
//! the model blind; now each answers with a `job` id, and **`wait_for_jobs`**
//! reports how they ended.
//!
//! *Waiting* is not done here. `execute` runs on the UI thread and must never
//! block, so `wait_for_jobs` through the executor is a snapshot; the Assistant's
//! worker thread (`agent_bridge`) does the waiting by polling that snapshot
//! between sleeps, so Stop interrupts it and the window never stops painting.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::AgentResult;
use crate::app::{App, TranscribeSource};
use crate::tasks::TaskId;

/// How a job stands.
#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Running,
    Done(Value),
    Failed(String),
}

/// One AI job the Assistant started in this run.
#[derive(Debug, Clone)]
pub struct AgentJob {
    /// What the model sees: the task's own id.
    pub id: u64,
    /// The tool that started it.
    pub kind: String,
    pub task: TaskId,
    pub state: JobState,
    /// The finished result has been handed to the model once. It is not
    /// repeated: a transcript is long, and the model already has it.
    pub delivered: bool,
}

/// Added to every `{started:true}` so a model that has forgotten is reminded.
pub const WAIT_NOTE: &str = "runs in the background; call wait_for_jobs to get its result";

/// The default wait, and the longest one a call may ask for.
pub const DEFAULT_WAIT_S: u64 = 120;
pub const MAX_WAIT_S: u64 = 300;

/// `{jobs:[{id, kind, status, result?, error?}], running}`. With `deliver`, a
/// finished job's result is marked as handed over, so later calls say
/// `result_delivered` instead of repeating it.
pub fn snapshot(jobs: &mut [AgentJob], deliver: bool) -> Value {
    let running = jobs.iter().filter(|j| j.state == JobState::Running).count();
    let list: Vec<Value> = jobs
        .iter_mut()
        .map(|j| {
            let mut v = json!({"id": j.id, "kind": j.kind});
            let finished = j.state != JobState::Running;
            match &j.state {
                JobState::Running => v["status"] = json!("running"),
                JobState::Done(result) => {
                    v["status"] = json!("done");
                    if j.delivered {
                        v["result_delivered"] = json!(true);
                    } else {
                        v["result"] = result.clone();
                    }
                }
                JobState::Failed(why) => {
                    v["status"] = json!("failed");
                    v["error"] = json!(why);
                }
            }
            if deliver && finished {
                j.delivered = true;
            }
            v
        })
        .collect();
    let mut out = json!({"jobs": list, "running": running});
    if jobs.is_empty() {
        out["note"] = json!("no AI jobs have been started in this run");
    }
    out
}

/// `{timeout_s?: 1..300}` -> seconds. Unknown keys are refused, as in the dialogs.
pub fn parse_timeout(args: &Value) -> Result<u64, String> {
    if let Some(map) = args.as_object() {
        for key in map.keys() {
            if key != "timeout_s" {
                return Err(format!("unknown arg '{key}'; valid: timeout_s"));
            }
        }
    }
    match args.get("timeout_s").filter(|v| !v.is_null()) {
        None => Ok(DEFAULT_WAIT_S),
        Some(v) => {
            let n = v.as_f64().filter(|n| n.is_finite()).ok_or("'timeout_s' needs a number")?;
            if n.fract() != 0.0 || !(1.0..=MAX_WAIT_S as f64).contains(&n) {
                return Err(format!("'timeout_s' must be a whole number from 1 to {MAX_WAIT_S}"));
            }
            Ok(n as u64)
        }
    }
}

/// `wait_for_jobs` through the executor: what the jobs are doing right now.
/// The Assistant's worker thread turns this into a wait (see `agent_bridge`);
/// a caller with no worker (a test, a script) polls it itself.
pub fn wait_for_jobs(app: &mut App, args: &Value) -> AgentResult {
    if let Err(e) = parse_timeout(args) {
        return AgentResult::err(e);
    }
    app.agent_poll_tasks();
    AgentResult::ok(snapshot(&mut app.agent_jobs, true))
}

/// `transcribe {sound?, path?}`: start Whisper on a library sound or a file.
pub fn transcribe(app: &mut App, args: &Value) -> AgentResult {
    if let Some(map) = args.as_object() {
        for key in map.keys() {
            if key != "sound" && key != "path" {
                return AgentResult::err(format!("unknown arg '{key}'; valid: sound, path"));
            }
        }
    }
    let given = |key: &str| args.get(key).filter(|v| !v.is_null());
    let text = |key: &str| -> Result<Option<String>, String> {
        match given(key) {
            None => Ok(None),
            Some(v) => match v.as_str().map(str::trim) {
                Some(s) if !s.is_empty() => Ok(Some(s.to_string())),
                _ => Err(format!("'{key}' needs text")),
            },
        }
    };
    let (sound, path) = match (text("sound"), text("path")) {
        (Err(e), _) | (_, Err(e)) => return AgentResult::err(e),
        (Ok(s), Ok(p)) => (s, p),
    };
    let source = match (sound, path) {
        (Some(s), None) => TranscribeSource::Sound(s),
        (None, Some(p)) => TranscribeSource::File(PathBuf::from(p)),
        (None, None) => return AgentResult::err("missing required args: sound | path (exactly one)"),
        (Some(_), Some(_)) => return AgentResult::err("pass exactly one of 'sound' and 'path', not both"),
    };
    match app.agent_transcribe_async(source) {
        Ok(started) => AgentResult::ok(started),
        Err(e) => AgentResult::err(e),
    }
}

/// The `{started:true, job, note}` every async tool answers with.
pub fn started(id: &str, job: u64, what: &str) -> Value {
    json!({"id": id, "started": true, "job": job, "note": format!("{what}; {WAIT_NOTE}")})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, state: JobState) -> AgentJob {
        AgentJob { id, kind: "stub".into(), task: TaskId(id), state, delivered: false }
    }

    #[test]
    fn the_snapshot_lists_running_done_and_failed_and_delivers_a_result_once() {
        let mut jobs = vec![
            job(1, JobState::Running),
            job(2, JobState::Done(json!({"text": "hello"}))),
            job(3, JobState::Failed("no model".into())),
        ];
        let first = snapshot(&mut jobs, true);
        assert_eq!(first["running"], 1);
        let list = first["jobs"].as_array().unwrap();
        assert_eq!(list[0]["status"], "running");
        assert_eq!((list[1]["status"].clone(), list[1]["result"]["text"].clone()), (json!("done"), json!("hello")));
        assert_eq!((list[2]["status"].clone(), list[2]["error"].clone()), (json!("failed"), json!("no model")));
        let again = snapshot(&mut jobs, true);
        let list = again["jobs"].as_array().unwrap();
        assert!(list[1].get("result").is_none() && list[1]["result_delivered"] == true, "{list:?}");
        assert_eq!(list[2]["error"], "no model", "a failure is always repeated");
        // A polling snapshot does not use up the delivery.
        let mut fresh = vec![job(2, JobState::Done(json!(1)))];
        let _ = snapshot(&mut fresh, false);
        assert!(snapshot(&mut fresh, true)["jobs"][0].get("result").is_some());
        assert!(snapshot(&mut [], true)["note"].is_string());
    }

    #[test]
    fn the_timeout_is_validated() {
        assert_eq!(parse_timeout(&json!({})), Ok(DEFAULT_WAIT_S));
        assert_eq!(parse_timeout(&json!({"timeout_s": 5})), Ok(5));
        assert_eq!(parse_timeout(&json!({"timeout_s": 300})), Ok(300));
        for bad in [json!({"timeout_s": 0}), json!({"timeout_s": 301}), json!({"timeout_s": 2.5}), json!({"timeout_s": "5"})] {
            assert!(parse_timeout(&bad).is_err(), "{bad}");
        }
        assert!(parse_timeout(&json!({"seconds": 5})).unwrap_err().contains("unknown arg"));
    }
}
