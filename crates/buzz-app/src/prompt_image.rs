//! **Generate Image**: a positive and a negative prompt, the workflow's LoRA
//! strengths, a seed and a size, sent to the local ComfyUI; the picture comes
//! back into the Library, or onto the stage.
//!
//! Built from the pieces the producer already uses — [`ComfyClient`] to find
//! ComfyUI and run the job, [`comfy_profile::load_api`] to turn a workflow
//! saved from ComfyUI's UI into the API form, [`comfy_profile::bind`] to find
//! the prompt, latent and sampler nodes by their wiring — so any ordinary
//! text-to-image workflow works, not one shape of graph.
//!
//! The workflow file is never changed. What is sent is a copy with this
//! request's values in it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::comfy::{ComfyClient, Output};
use crate::comfy_profile;

/// How long one picture may take, a cold model load included: Z-Image Turbo
/// reads ~20 GB of weights the first time.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(1200);
/// A cold ComfyUI start: Python, PyTorch and every custom node to import.
const START_TIMEOUT: Duration = Duration::from_secs(600);

/// The rgthree stack's four slots: `lora_01`/`strength_01` … `_04`.
const STACK: &str = "Lora Loader Stack (rgthree)";

/// One LoRA the workflow loads, and the strength to load it at.
#[derive(Debug, Clone, PartialEq)]
pub struct LoraSlot {
    /// The node it lives on (the same id in UI and API form).
    pub node: String,
    /// Which of a stack node's slots (1–4); `None` for a single-LoRA loader.
    pub slot: Option<u8>,
    /// The file, as the workflow names it.
    pub name: String,
    pub strength: f32,
}

/// The LoRAs a workflow loads, UI or API form, in node order. Empty slots
/// (`None`) are left out: there is nothing there to set a strength for.
pub fn lora_slots(workflow: &Value) -> Vec<LoraSlot> {
    let mut out = Vec::new();
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let named = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty() && *n != "None")
            .map(str::to_string)
    };
    if let Some(nodes) = workflow.get("nodes").and_then(Value::as_array) {
        // UI form: the values are positional `widgets_values`.
        for n in nodes {
            // Bypassed or muted nodes never run.
            if matches!(n.get("mode").and_then(Value::as_u64), Some(2 | 4)) {
                continue;
            }
            let id = n.get("id").map(|v| v.to_string()).unwrap_or_default();
            let class = n.get("type").and_then(Value::as_str).unwrap_or("");
            let w = n.get("widgets_values");
            let at = |i: usize| w.and_then(|w| w.get(i));
            if class == STACK {
                for slot in 0..4u8 {
                    if let Some(name) = named(at(slot as usize * 2)) {
                        out.push(LoraSlot { node: id.clone(), slot: Some(slot + 1), name, strength: num(at(slot as usize * 2 + 1)) });
                    }
                }
            } else if class.starts_with("LoraLoader") {
                if let Some(name) = named(at(0)) {
                    out.push(LoraSlot { node: id, slot: None, name, strength: num(at(1)) });
                }
            }
        }
    } else if let Some(map) = workflow.as_object() {
        // API form: named inputs.
        let mut ids: Vec<&String> = map.keys().collect();
        ids.sort_by_key(|k| k.parse::<u64>().unwrap_or(u64::MAX));
        for id in ids {
            let n = &map[id];
            let class = n.get("class_type").and_then(Value::as_str).unwrap_or("");
            let inputs = n.get("inputs");
            let get = |k: &str| inputs.and_then(|i| i.get(k));
            if class == STACK {
                for slot in 1..=4u8 {
                    if let Some(name) = named(get(&format!("lora_{slot:02}"))) {
                        out.push(LoraSlot { node: id.clone(), slot: Some(slot), name, strength: num(get(&format!("strength_{slot:02}"))) });
                    }
                }
            } else if class.starts_with("LoraLoader") {
                if let Some(name) = named(get("lora_name")) {
                    out.push(LoraSlot { node: id.clone(), slot: None, name, strength: num(get("strength_model")) });
                }
            }
        }
    }
    out
}

/// Put each slot's strength into an API workflow. A single loader takes it for
/// both the model and the text encoder, as its one slider in ComfyUI is
/// usually used.
pub fn apply_loras(api: &mut Value, loras: &[LoraSlot]) {
    for l in loras {
        let Some(inputs) = api.get_mut(&l.node).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) else {
            continue;
        };
        let s = Value::from((l.strength as f64 * 1000.0).round() / 1000.0);
        match l.slot {
            Some(slot) => {
                inputs.insert(format!("strength_{slot:02}"), s);
            }
            None => {
                inputs.insert("strength_model".into(), s.clone());
                if inputs.contains_key("strength_clip") {
                    inputs.insert("strength_clip".into(), s);
                }
            }
        }
    }
}

/// The size the workflow's empty latent asks for, UI or API form.
pub fn latent_size(workflow: &Value) -> Option<(u32, u32)> {
    let pick = |w: Option<&Value>, h: Option<&Value>| Some((w?.as_u64()? as u32, h?.as_u64()? as u32));
    if let Some(nodes) = workflow.get("nodes").and_then(Value::as_array) {
        nodes.iter().find_map(|n| {
            let class = n.get("type").and_then(Value::as_str)?;
            (class.starts_with("Empty") && class.contains("Latent"))
                .then(|| pick(n.pointer("/widgets_values/0"), n.pointer("/widgets_values/1")))?
        })
    } else {
        workflow.as_object()?.values().find_map(|n| {
            let class = n.get("class_type").and_then(Value::as_str)?;
            (class.starts_with("Empty") && class.contains("Latent"))
                .then(|| pick(n.pointer("/inputs/width"), n.pointer("/inputs/height")))?
        })
    }
}

/// Everything one picture needs.
#[derive(Debug, Clone)]
pub struct Request {
    pub workflow: PathBuf,
    /// `http://127.0.0.1:PORT`; empty finds ComfyUI the usual way.
    pub url: String,
    pub positive: String,
    pub negative: String,
    pub loras: Vec<LoraSlot>,
    pub seed: u64,
    pub size: (u32, u32),
    /// Drop the workflow's own upscaler: a 4× RealESRGAN pass turns 1280×720
    /// into 5120×2880, far more than a stage picture needs.
    pub skip_upscale: bool,
    /// Start the local ComfyUI when none answers.
    pub start_comfy: bool,
    /// Open ComfyUI's own page on this workflow, with this request's values
    /// in it, so the run can be watched there node by node.
    pub show_in_comfy: bool,
    /// Load a GGUF copy of each model when one is installed: Q8 is about
    /// 60% of the memory at nearly the same picture. See [`use_gguf`].
    pub small_models: bool,
}

/// The ComfyUI-GGUF loaders, and the input that names the file.
const GGUF_LOADERS: [(&str, &str, &str); 2] = [("UNETLoader", "UnetLoaderGGUF", "unet_name"), ("CLIPLoader", "CLIPLoaderGGUF", "clip_name")];

/// What a model file is, without its precision: `z_image_turbo_bf16.safetensors`
/// and `z_image_turbo-Q8_0.gguf` are both `zimageturbo`; `qwen_3_4b` and
/// `Qwen3-4B-UD-Q8_K_XL` both `qwen34b`.
fn model_key(file: &str) -> String {
    let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file).to_ascii_lowercase();
    let marker = |t: &str| {
        matches!(t, "bf16" | "fp16" | "fp32" | "f16" | "f32" | "ud" | "gguf")
            || ["fp8", "e4m3", "e5m2"].iter().any(|p| t.starts_with(p))
            || quant_bits(t).is_some()
    };
    stem.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).take_while(|t| !marker(t)).collect()
}

/// `q8` → 8, `iq4` → 4: the bits of a GGUF quant token.
fn quant_bits(token: &str) -> Option<u8> {
    let t = token.strip_prefix("iq").or_else(|| token.strip_prefix('q'))?;
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !t[digits.len()..].chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    digits.parse().ok()
}

/// The best GGUF copy of `full` among `choices`: the same model, the most
/// bits up to Q8 (an F16 GGUF saves nothing).
fn gguf_for(full: &str, choices: &[&str]) -> Option<String> {
    let key = model_key(full);
    choices
        .iter()
        .filter(|c| c.to_ascii_lowercase().ends_with(".gguf") && !key.is_empty() && model_key(c) == key)
        .filter_map(|c| {
            let stem = Path::new(c).file_stem()?.to_str()?.to_ascii_lowercase();
            let bits = stem.split(|ch: char| !ch.is_ascii_alphanumeric()).find_map(quant_bits)?;
            (bits <= 8).then_some((bits, c.len(), *c))
        })
        // Most bits; of equals, the plainest name (Q8_0 before UD-Q8_K_XL).
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, _, c)| c.to_string())
}

/// **Smaller models**: each UNET / CLIP loader of the API copy is swapped for
/// ComfyUI-GGUF's when a GGUF of the same model is installed. `info` gives a
/// GGUF loader class's `/object_info` entry (`{}` when it is not installed).
/// Returns what to tell the user.
pub fn use_gguf(api: &mut Value, info: impl Fn(&str) -> Value) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(map) = api.as_object_mut() else {
        return notes;
    };
    for (full_class, gguf_class, input) in GGUF_LOADERS {
        let wanted: Vec<&mut Value> = map.values_mut().filter(|n| n.get("class_type").and_then(Value::as_str) == Some(full_class)).collect();
        if wanted.is_empty() {
            continue;
        }
        let entry = info(gguf_class);
        let Some(files) = entry.pointer(&format!("/{gguf_class}/input/required/{input}/0")).and_then(Value::as_array) else {
            notes.push(format!("{gguf_class} is not installed in ComfyUI (ComfyUI-GGUF), so the full-size models were used"));
            continue;
        };
        let files: Vec<&str> = files.iter().filter_map(Value::as_str).collect();
        for node in wanted {
            let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
            let Some(full) = inputs.get(input).and_then(Value::as_str).map(str::to_string) else { continue };
            let Some(small) = gguf_for(&full, &files) else {
                notes.push(format!("no GGUF copy of {full} is installed, so it was loaded full size"));
                continue;
            };
            // The GGUF loaders take only the file (and a CLIP's type).
            inputs.retain(|k, _| k == input || (full_class == "CLIPLoader" && k == "type"));
            inputs.insert(input.into(), Value::from(small));
            node["class_type"] = Value::from(gguf_class);
        }
    }
    notes
}

/// A finished picture.
#[derive(Debug, Clone)]
pub struct Generated {
    pub bytes: Vec<u8>,
    pub file: PathBuf,
    pub seed: u64,
}

/// Where a step of the job has got to, for the window to show.
/// Where a job is, for the window: what it is doing, and how far through
/// the whole picture it is when that is known (`0.0..=1.0`). `Sync`, because
/// ComfyUI's live feed reports from its own thread.
pub type Progress<'a> = &'a (dyn Fn(&str, Option<f32>) + Sync);

/// The ComfyUI this request names, or the first one that answers.
pub fn connect(url: &str, start: bool, progress: Progress<'_>) -> Result<ComfyClient, String> {
    let url = url.trim().trim_end_matches('/');
    if !url.is_empty() {
        let client = ComfyClient::new(url);
        if client.alive() {
            return Ok(client);
        }
        if !start {
            return Err(format!("nothing answers at {url} — is ComfyUI running?"));
        }
    }
    if let Some(found) = ComfyClient::discover() {
        return Ok(found);
    }
    if !start {
        return Err("ComfyUI is not running (nothing answered on 127.0.0.1:8188-8208 or :8000). Start it, or tick \"Start ComfyUI if needed\"".into());
    }
    progress("Starting ComfyUI — the first start takes a minute or two…", None);
    ComfyClient::ensure_running(true, START_TIMEOUT)
}

/// Turn the workflow file into the API form this request will send.
pub fn prepare(req: &Request, client: &ComfyClient) -> Result<(Value, Vec<String>, comfy_profile::Bound), String> {
    let mut api = load_api_fresh(&req.workflow, client)?;
    let mut notes = Vec::new();
    let bound = comfy_profile::bind(&mut api, &req.positive, req.seed, req.size, "buzzanimate/generate")?;

    // A negative that is really the positive zeroed out (Z-Image's usual
    // wiring) leads back to the *positive* text node; writing there would
    // replace the prompt. Only nodes that are the negative's alone are set.
    // An empty box keeps the workflow's own negative rather than blanking it.
    let negatives: Vec<&String> = bound.negative.iter().filter(|n| !bound.prompt.contains(n)).collect();
    if !req.negative.trim().is_empty() {
        if negatives.is_empty() {
            notes.push("this workflow has no negative prompt node, so the negative prompt was not used".into());
        }
        for id in negatives {
            if let Some(inputs) = api.get_mut(id.as_str()).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
                if inputs.contains_key("text_g") {
                    inputs.insert("text_g".into(), Value::from(req.negative.clone()));
                    inputs.insert("text_l".into(), Value::from(req.negative.clone()));
                } else {
                    inputs.insert("text".into(), Value::from(req.negative.clone()));
                }
            }
        }
    }
    apply_loras(&mut api, &req.loras);
    if req.skip_upscale {
        comfy_profile::strip_upscale(&mut api);
    }
    if req.small_models {
        notes.extend(use_gguf(&mut api, |class| client.node_info(class).unwrap_or_default()));
    }
    // A WAS "Image Save" writes wherever its own path says; as a plain
    // SaveImage the picture lands in ComfyUI's output and is read back here.
    comfy_profile::saves_into_comfy_output(&mut api, "buzzanimate/generate");
    Ok((api, notes, bound))
}

/// The workflow in API form. A UI-form file is converted through ComfyUI's
/// `/object_info` (as [`comfy_profile::load_api`] does), and the conversion is
/// kept in BuzzAnimate's own config folder keyed on the file's bytes — not
/// beside the workflow, where ComfyUI's workflow list would show it as one.
fn load_api_fresh(workflow: &Path, client: &ComfyClient) -> Result<Value, String> {
    let bytes = std::fs::read(workflow).map_err(|e| format!("{}: {e}", workflow.display()))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", workflow.display()))?;
    if v.get("nodes").is_none() && v.as_object().is_some_and(|o| o.values().any(|n| n.get("class_type").is_some())) {
        // Already API form ("Export (API)" in ComfyUI).
        return Ok(v);
    }
    let cache = cache_dir().join(format!("{}.api.json", comfy_profile::short_hash(&bytes)));
    if let Some(api) = std::fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        return Ok(api);
    }
    let info = client.object_info()?;
    let api = comfy_profile::ui_to_api(&v, &info)?;
    if std::fs::create_dir_all(cache_dir()).is_ok() {
        let _ = std::fs::write(&cache, serde_json::to_string(&api).unwrap_or_default());
    }
    Ok(api)
}

/// `~/.config/BuzzAnimate/comfy_api_cache`.
fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join("BuzzAnimate")
        .join("comfy_api_cache")
}

/// Run one picture to the end, reporting each stage as ComfyUI reaches it.
pub fn generate(req: &Request, out_dir: &Path, progress: Progress<'_>) -> Result<(Generated, Vec<String>), String> {
    progress("Connecting to ComfyUI…", None);
    let client = connect(&req.url, req.start_comfy, progress)?;
    progress("Preparing the workflow…", None);
    let (api, mut notes, bound) = prepare(req, &client)?;
    if req.show_in_comfy {
        match show_in_comfy(req, &client, &bound) {
            Ok(Some(note)) => notes.push(note),
            Ok(None) => {}
            Err(e) => notes.push(format!("could not show the workflow in ComfyUI: {e}")),
        }
    }
    let stages = Stages::of(&api);
    progress("Queued — waiting for ComfyUI…", Some(0.0));

    // ComfyUI's live feed on a thread of its own while the job is queued and
    // waited for; `wait` (history polling) stays the word on when it is done.
    let stop = std::sync::atomic::AtomicBool::new(false);
    let prompt_id: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let outputs = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut run = RunState::default();
            let _ = client.watch(&stop, |message| {
                let mine = prompt_id.lock().map(|p| p.clone()).unwrap_or(None);
                if let Some((text, fraction)) = run.read(&message, mine.as_deref(), &stages) {
                    progress(&text, fraction);
                }
            });
        });
        let result = (|| {
            let id = client.queue_shared(api, None)?;
            if let Ok(mut p) = prompt_id.lock() {
                *p = Some(id.clone());
            }
            client.wait(&id, GENERATE_TIMEOUT)
        })();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        result
    })?;
    let pick = best_output(&outputs).ok_or("ComfyUI finished but returned no picture")?;
    progress("Fetching the picture…", Some(1.0));
    let bytes = client.view(pick)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let ext = Path::new(&pick.filename).extension().and_then(|e| e.to_str()).unwrap_or("png");
    // Never over an earlier picture: the name keeps only five digits of the
    // seed, so a second take can land on a name already used.
    let stem = file_stem(&req.positive, req.seed);
    let mut file = out_dir.join(format!("{stem}.{ext}"));
    let mut take = 2;
    while file.exists() {
        file = out_dir.join(format!("{stem}_{take}.{ext}"));
        take += 1;
    }
    std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok((Generated { bytes, file, seed: req.seed }, notes))
}

/// What each node of the job is, in words a person reads in a progress
/// line: "Loading the model", "Sampling", "Decoding", by node class, with the
/// workflow's own title for anything else.
struct Stages {
    names: std::collections::HashMap<String, String>,
    total: usize,
}

impl Stages {
    fn of(api: &Value) -> Self {
        let mut names = std::collections::HashMap::new();
        if let Some(map) = api.as_object() {
            for (id, node) in map {
                let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
                let title = node
                    .pointer("/_meta/title")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .unwrap_or(class);
                names.insert(id.clone(), describe(class, title));
            }
        }
        let total = names.len().max(1);
        Self { names, total }
    }

    fn name(&self, node: &str) -> String {
        self.names.get(node).cloned().unwrap_or_else(|| format!("node {node}"))
    }
}

/// A node class, as a stage of making a picture.
fn describe(class: &str, title: &str) -> String {
    let c = class.to_ascii_lowercase();
    if c.contains("lora") {
        "Loading the LoRA".into()
    } else if c.contains("vaeloader") {
        "Loading the VAE".into()
    } else if c.contains("cliploader") || c.contains("textencoderloader") {
        "Loading the text encoder".into()
    } else if c.contains("unetloader") || c.contains("checkpoint") || c.contains("diffusion") {
        "Loading the model".into()
    } else if c.contains("loader") {
        format!("Loading {title}")
    } else if c.starts_with("ksampler") || c.contains("sampler") {
        "Sampling".into()
    } else if c.contains("textencode") || c.contains("cliptext") {
        "Reading the prompt".into()
    } else if c.contains("vaedecode") {
        "Decoding the picture".into()
    } else if c.contains("upscale") {
        "Upscaling".into()
    } else if c.contains("save") || c.contains("preview") {
        "Saving".into()
    } else if c.contains("latent") {
        "Setting up the canvas".into()
    } else {
        title.to_string()
    }
}

/// What has happened so far in one run, from ComfyUI's messages.
#[derive(Default)]
struct RunState {
    cached: usize,
    done: usize,
    current: Option<String>,
}

impl RunState {
    /// One message from `/ws`: the line and fraction to show, if it is ours
    /// and says something new. `mine` is the job's id once it is queued.
    fn read(&mut self, message: &Value, mine: Option<&str>, stages: &Stages) -> Option<(String, Option<f32>)> {
        let kind = message.get("type").and_then(Value::as_str)?;
        let data = message.get("data")?;
        let theirs = data.get("prompt_id").and_then(Value::as_str);
        if theirs.is_some() && mine.is_some() && theirs != mine {
            return None;
        }
        let to_run = stages.total.saturating_sub(self.cached).max(1);
        let of = |done: f32| Some((done / to_run as f32).clamp(0.0, 1.0));
        match kind {
            "execution_start" => Some(("Started".into(), Some(0.0))),
            "execution_cached" => {
                self.cached = data.get("nodes").and_then(Value::as_array).map_or(0, Vec::len);
                None
            }
            "executing" => {
                let node = data.get("node").and_then(Value::as_str)?;
                if self.current.replace(node.to_string()).is_some() {
                    self.done += 1;
                }
                let step = (self.done + 1).min(to_run);
                Some((format!("{} \u{2014} step {step} of {to_run}", stages.name(node)), of(self.done as f32)))
            }
            "progress" => {
                let value = data.get("value").and_then(Value::as_f64)?;
                let max = data.get("max").and_then(Value::as_f64).filter(|m| *m > 0.0)?;
                let node = data.get("node").and_then(Value::as_str).map(str::to_string).or_else(|| self.current.clone())?;
                Some((
                    format!("{} \u{2014} {value:.0} of {max:.0}", stages.name(&node)),
                    of(self.done as f32 + (value / max) as f32),
                ))
            }
            // Newer ComfyUI: one message holding every node's state —
            // running or finished, and the running one's own steps.
            "progress_state" => {
                let nodes = data.get("nodes").and_then(Value::as_object)?;
                let finished = nodes.values().filter(|n| n.get("state").and_then(Value::as_str) == Some("finished")).count();
                self.done = finished;
                let running = nodes
                    .iter()
                    .filter(|(_, n)| n.get("state").and_then(Value::as_str) == Some("running"))
                    .max_by(|a, b| {
                        let max = |n: &Value| n.get("max").and_then(Value::as_f64).unwrap_or(1.0);
                        max(a.1).total_cmp(&max(b.1))
                    });
                // Between two nodes: the bar moves on, the line keeps the
                // stage just finished rather than flickering to a count.
                let Some((id, node)) = running else {
                    let last = self.current.as_deref().map(|n| stages.name(n)).unwrap_or_else(|| "Working".into());
                    return Some((format!("{last} \u{2014} done"), of(finished as f32)));
                };
                let id = node.get("display_node_id").and_then(Value::as_str).unwrap_or(id);
                self.current = Some(id.to_string());
                let value = node.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                let max = node.get("max").and_then(Value::as_f64).unwrap_or(1.0).max(1.0);
                let text = if max > 1.0 {
                    format!("{} \u{2014} {value:.0} of {max:.0}", stages.name(id))
                } else {
                    format!("{} \u{2014} step {} of {to_run}", stages.name(id), (finished + 1).min(to_run))
                };
                Some((text, of(finished as f32 + (value / max) as f32)))
            }
            "execution_error" => {
                let what = data.get("exception_message").and_then(Value::as_str).unwrap_or("error");
                let node = data.get("node_type").and_then(Value::as_str).unwrap_or("a node");
                Some((format!("ComfyUI stopped in {node}: {}", what.lines().next().unwrap_or(what)), None))
            }
            _ => None,
        }
    }
}

/// Open ComfyUI's page on the workflow, carrying this request's prompt,
/// seed, size and LoRA strengths, so the run shows there as it happens. A
/// note for the window when the page can only open on its own default.
fn show_in_comfy(req: &Request, client: &ComfyClient, bound: &comfy_profile::Bound) -> Result<Option<String>, String> {
    let text = std::fs::read_to_string(&req.workflow).map_err(|e| format!("{}: {e}", req.workflow.display()))?;
    let mut ui: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if ui.get("nodes").is_none() {
        crate::comfy::open_in_browser(&client.base_url);
        return Ok(Some("the workflow is in API form, so ComfyUI opened on its own last workflow".into()));
    }
    fill_ui_workflow(&mut ui, req, bound);
    let checkout = crate::comfy::checkout_for(&req.workflow).ok_or("no ComfyUI folder found to show it from")?;
    let (page, served) = crate::comfy::publish_workflow(client, &checkout, "buzzanimate_generate", &ui)?;
    crate::comfy::open_in_browser(&page);
    Ok((!served).then(|| {
        "ComfyUI opened, but it needs one restart to load BuzzAnimate's workflow bridge — after that it opens on this workflow".into()
    }))
}

/// This request's values written into a UI-form workflow, by the node ids
/// `bind` found — the same ids in both forms.
fn fill_ui_workflow(ui: &mut Value, req: &Request, bound: &comfy_profile::Bound) {
    let Some(nodes) = ui.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for node in nodes {
        let id = node.get("id").map(|v| v.to_string()).unwrap_or_default();
        let class = node.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let Some(w) = node.get_mut("widgets_values").and_then(Value::as_array_mut) else {
            continue;
        };
        let set = |w: &mut Vec<Value>, i: usize, v: Value| {
            if let Some(slot) = w.get_mut(i) {
                *slot = v;
            }
        };
        if class == "CLIPTextEncode" {
            if bound.prompt.contains(&id) {
                set(w, 0, Value::from(req.positive.clone()));
            } else if bound.negative.contains(&id) && !req.negative.trim().is_empty() {
                set(w, 0, Value::from(req.negative.clone()));
            }
        }
        if bound.samplers.contains(&id) && class.starts_with("KSampler") {
            set(w, 0, Value::from(req.seed));
            set(w, 1, Value::from("fixed"));
        }
        if bound.latent.contains(&id) {
            set(w, 0, Value::from(req.size.0));
            set(w, 1, Value::from(req.size.1));
        }
        for l in req.loras.iter().filter(|l| l.node == id) {
            match l.slot {
                Some(slot) => set(w, (slot as usize - 1) * 2 + 1, Value::from(l.strength)),
                None => {
                    set(w, 1, Value::from(l.strength));
                    set(w, 2, Value::from(l.strength));
                }
            }
        }
    }
}

/// ComfyUI's history lists previews (`temp`) beside saves (`output`); the
/// saved picture is the finished one.
fn best_output(outputs: &[Output]) -> Option<&Output> {
    outputs.iter().find(|o| o.kind == "output").or_else(|| outputs.first())
}

/// Words that say how a picture is shot or painted, or join the words that
/// say what is in it — never what a picture *is*. Left out of its name.
const NOT_A_SUBJECT: &[&str] = &[
    // joining words
    "a", "an", "the", "of", "at", "in", "on", "during", "with", "and", "or", "for", "to", "from",
    "by", "into", "over", "under", "near", "is", "are", "was", "were", "be", "this", "that", "its",
    "their", "his", "her", "some", "very", "while", "as", "s",
    // framing
    "cinematic", "wide", "shot", "close", "closeup", "up", "medium", "long", "establishing", "view",
    "angle", "aerial", "low", "high", "overhead", "scene", "frame", "framing", "composition",
    // medium and quality
    "image", "photo", "photograph", "picture", "illustration", "render", "rendering", "style",
    "styled", "detailed", "highly", "quality", "hd", "4k", "8k", "masterpiece", "realistic",
    "photorealistic", "digital", "painting", "art", "artwork", "concept", "2d", "3d", "cartoon",
    "anime", "lighting", "lit", "beautiful", "stunning", "vibrant", "dramatic", "soft", "natural",
    "epic", "gorgeous", "featuring", "prominently", "showing", "depicting", "depicts",
    // general colour words that describe rather than name
    "rugged", "semi", "rustic", "early", "late", "era",
];

/// **A short name for a picture, from what its prompt is about**:
/// `rural_indian_village_market_lane_night_diwali_28188`.
///
/// Prompts are long descriptions, and their first words are usually how the
/// picture is shot ("Cinematic wide shot of a…"), which named every picture
/// alike. The subject is in the opening sentence, so the name is that
/// sentence's words less [`NOT_A_SUBJECT`] and numbers, up to seven of them,
/// with the seed's last five digits to tell two takes apart. Safe as a file
/// and as a Library name.
pub fn file_stem(prompt: &str, seed: u64) -> String {
    let keywords = |text: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            let word = word.to_lowercase();
            if NOT_A_SUBJECT.contains(&word.as_str())
                || word.chars().all(|c| c.is_ascii_digit())
                || word.chars().next().is_some_and(|c| c.is_ascii_digit())
                || out.contains(&word)
            {
                continue;
            }
            out.push(word);
        }
        out
    };
    let opening = prompt.split(['.', ';', '\n']).find(|s| !s.trim().is_empty()).unwrap_or("");
    let mut words = keywords(opening);
    if words.len() < 2 {
        words = keywords(prompt);
    }
    let mut name = String::new();
    for word in words.iter().take(7) {
        if !name.is_empty() && name.len() + 1 + word.len() > 56 {
            break;
        }
        if !name.is_empty() {
            name.push('_');
        }
        name.push_str(word);
    }
    if name.is_empty() {
        name.push_str("image");
    }
    format!("{name}_{:05}", seed % 100_000)
}

/// Ask ComfyUI to stop the job it is running (best effort).
pub fn interrupt(url: &str) {
    if let Ok(client) = connect(url, false, &|_, _| {}) {
        let _ = ureq::post(&format!("{}/interrupt", client.base_url))
            .timeout(Duration::from_secs(5))
            .send_string("");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ComfyUI's live messages, read as stages: the node that runs named by
    /// what it does, the sampler's steps counted, the fraction rising, and
    /// another job's messages ignored.
    #[test]
    fn live_messages_become_named_stages() {
        let api = json!({
            "1": {"class_type": "UNETLoader", "_meta": {"title": "Load Diffusion Model"}, "inputs": {}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {}},
            "3": {"class_type": "KSampler", "inputs": {}},
            "4": {"class_type": "VAEDecode", "inputs": {}}
        });
        let stages = Stages::of(&api);
        let mut run = RunState::default();
        let mine = Some("job-1");
        let msg = |kind: &str, data: Value| json!({"type": kind, "data": data});

        let (text, f) = run.read(&msg("executing", json!({"node": "1", "prompt_id": "job-1"})), mine, &stages).unwrap();
        assert_eq!(text, "Loading the model \u{2014} step 1 of 4");
        assert_eq!(f, Some(0.0));
        run.read(&msg("executing", json!({"node": "2", "prompt_id": "job-1"})), mine, &stages);
        let (text, _) = run.read(&msg("executing", json!({"node": "3", "prompt_id": "job-1"})), mine, &stages).unwrap();
        assert!(text.starts_with("Sampling"), "{text}");
        let (text, f) = run.read(&msg("progress", json!({"value": 4, "max": 8, "node": "3", "prompt_id": "job-1"})), mine, &stages).unwrap();
        assert_eq!(text, "Sampling \u{2014} 4 of 8");
        assert!((f.unwrap() - 2.5 / 4.0).abs() < 1e-6, "{f:?}");
        assert!(run.read(&msg("executing", json!({"node": "4", "prompt_id": "someone-else"})), mine, &stages).is_none());

        // The newer form: every node's state in one message.
        let mut run = RunState::default();
        let state = json!({"prompt_id": "job-1", "nodes": {
            "1": {"state": "finished", "value": 1.0, "max": 1.0, "display_node_id": "1"},
            "2": {"state": "finished", "value": 1.0, "max": 1.0, "display_node_id": "2"},
            "3": {"state": "running", "value": 2.0, "max": 8.0, "display_node_id": "3"}
        }});
        let (text, f) = run.read(&msg("progress_state", state), mine, &stages).unwrap();
        assert_eq!(text, "Sampling \u{2014} 2 of 8");
        assert!((f.unwrap() - 2.25 / 4.0).abs() < 1e-6, "{f:?}");
    }

    /// The copy shown in ComfyUI carries this request's prompt, seed, size
    /// and LoRA strength, in the nodes `bind` found.
    #[test]
    fn the_shown_workflow_carries_the_request() {
        let mut ui = json!({"nodes": [
            {"id": 7, "type": "CLIPTextEncode", "widgets_values": ["old prompt"]},
            {"id": 8, "type": "CLIPTextEncode", "widgets_values": ["old negative"]},
            {"id": 3, "type": "KSampler", "widgets_values": [1, "randomize", 8, 1.0]},
            {"id": 5, "type": "EmptySD3LatentImage", "widgets_values": [1024, 1024, 1]},
            {"id": 9, "type": "LoraLoader", "widgets_values": ["x.safetensors", 1.0, 1.0]}
        ]});
        let bound = comfy_profile::Bound {
            prompt: vec!["7".into()],
            negative: vec!["8".into()],
            latent: vec!["5".into()],
            samplers: vec!["3".into()],
            saves: vec![],
        };
        let req = Request {
            workflow: PathBuf::new(),
            url: String::new(),
            positive: "a lamp".into(),
            negative: "blur".into(),
            loras: vec![LoraSlot { node: "9".into(), slot: None, name: "x.safetensors".into(), strength: 0.4 }],
            seed: 42,
            size: (640, 360),
            skip_upscale: true,
            start_comfy: false,
            show_in_comfy: true,
            small_models: false,
        };
        fill_ui_workflow(&mut ui, &req, &bound);
        let w = |i: usize| ui["nodes"][i]["widgets_values"].clone();
        assert_eq!(w(0)[0], "a lamp");
        assert_eq!(w(1)[0], "blur");
        assert_eq!((w(2)[0].clone(), w(2)[1].clone()), (json!(42), json!("fixed")));
        assert_eq!((w(3)[0].clone(), w(3)[1].clone()), (json!(640), json!(360)));
        assert!((w(4)[1].as_f64().unwrap() - 0.4).abs() < 1e-6);
    }

    fn ui_stack() -> Value {
        json!({"nodes": [
            {"id": 15, "type": "Lora Loader Stack (rgthree)", "mode": 0,
             "widgets_values": ["style.safetensors", 0.56, "None", 1.2, "extra.safetensors", 0.8, "None", 1]},
            {"id": 30, "type": "LoraLoader", "mode": 0, "widgets_values": ["single.safetensors", 0.7, 0.7]},
            {"id": 31, "type": "LoraLoader", "mode": 4, "widgets_values": ["bypassed.safetensors", 1, 1]},
            {"id": 3, "type": "EmptySD3LatentImage", "mode": 0, "widgets_values": [1280, 720, 1]}
        ]})
    }

    #[test]
    fn stack_and_single_loras_are_found_and_empty_slots_skipped() {
        let slots = lora_slots(&ui_stack());
        let names: Vec<_> = slots.iter().map(|s| (s.node.as_str(), s.slot, s.name.as_str(), s.strength)).collect();
        assert_eq!(
            names,
            vec![
                ("15", Some(1), "style.safetensors", 0.56),
                ("15", Some(3), "extra.safetensors", 0.8),
                ("30", None, "single.safetensors", 0.7),
            ]
        );
        assert_eq!(latent_size(&ui_stack()), Some((1280, 720)));
    }

    #[test]
    fn strengths_land_in_the_api_inputs() {
        let mut api = json!({
            "15": {"class_type": STACK, "inputs": {"lora_01": "style.safetensors", "strength_01": 0.56}},
            "30": {"class_type": "LoraLoader", "inputs": {"lora_name": "single.safetensors", "strength_model": 0.7, "strength_clip": 0.7}}
        });
        let mut slots = lora_slots(&api);
        assert_eq!(slots.len(), 2);
        slots[0].strength = 0.9;
        slots[1].strength = 0.25;
        apply_loras(&mut api, &slots);
        assert_eq!(api["15"]["inputs"]["strength_01"], json!(0.9));
        assert_eq!(api["30"]["inputs"]["strength_model"], json!(0.25));
        assert_eq!(api["30"]["inputs"]["strength_clip"], json!(0.25));
    }

    #[test]
    fn small_models_swap_in_the_q8_gguf_of_the_same_model() {
        let mut api = json!({
            "1": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen_3_4b.safetensors", "type": "lumina2", "device": "default"}},
            "8": {"class_type": "UNETLoader", "inputs": {"unet_name": "z_image_turbo_bf16.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "VAELoader", "inputs": {"vae_name": "ae.safetensors"}}
        });
        let info = |class: &str| match class {
            "UnetLoaderGGUF" => json!({"UnetLoaderGGUF": {"input": {"required": {"unet_name": [[
                "flux-Q8_0.gguf", "z_image_turbo-Q4_K_M.gguf", "z_image_turbo-Q8_0.gguf", "z_image_turbo-F16.gguf"
            ]]}}}}),
            "CLIPLoaderGGUF" => json!({"CLIPLoaderGGUF": {"input": {"required": {"clip_name": [[
                "Qwen3-4B-UD-Q8_K_XL.gguf", "Qwen3-4B-Q8_0.gguf", "t5xxl-Q8_0.gguf"
            ]]}}}}),
            _ => json!({}),
        };
        assert!(use_gguf(&mut api, info).is_empty());
        assert_eq!(api["8"], json!({"class_type": "UnetLoaderGGUF", "inputs": {"unet_name": "z_image_turbo-Q8_0.gguf"}}));
        assert_eq!(api["1"], json!({"class_type": "CLIPLoaderGGUF", "inputs": {"clip_name": "Qwen3-4B-Q8_0.gguf", "type": "lumina2"}}));
        assert_eq!(api["2"]["class_type"], "VAELoader");

        // No ComfyUI-GGUF: the workflow's own loaders, and a note saying so.
        let mut plain = json!({"8": {"class_type": "UNETLoader", "inputs": {"unet_name": "z_image_turbo_bf16.safetensors"}}});
        assert_eq!(use_gguf(&mut plain, |_| json!({})).len(), 1);
        assert_eq!(plain["8"]["class_type"], "UNETLoader");
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(file_stem("A rainy Village, at night!", 42), "rainy_village_night_00042");
        assert_eq!(file_stem("  ", 7), "image_00007");
    }

    /// **The name says what the picture is of**, not how it was shot: the
    /// framing words a prompt opens with are left out, and it comes from the
    /// opening sentence rather than the whole description.
    #[test]
    fn a_long_prompt_is_named_by_its_subject() {
        let prompt = "Cinematic wide shot of a rugged semi-rural Indian village market lane at \
                      night during Diwali, early 2000s era. Rutted dirt road lined with rustic \
                      mud-and-brick shopfronts.";
        assert_eq!(
            file_stem(prompt, 319156850728188),
            "rural_indian_village_market_lane_night_diwali_28188"
        );
    }
}
