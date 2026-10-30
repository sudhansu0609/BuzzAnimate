//! **Series ComfyUI profiles** (ANIMATED_CHANNELS_PLAN AN1): a series may carry
//! the creator's own ComfyUI workflows (`assets/comfy_profiles/<Series>/`),
//! saved in the UI format (`nodes` + `links`). They are converted once to the
//! API format (widget names from ComfyUI's `/object_info`), cached as
//! `<name>.api.json` next to the copy, and bound by node **class and wiring**
//! (never ids): the prompt is the CLIPTextEncode feeding a sampler's
//! `positive`, the negative the one feeding `negative` (kept as saved), the
//! size the latent, the seed the sampler's, the prefix the save nodes'.
//! LoRA loaders stay as saved. No profile for a series = the Z-Image default.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::comfy::ComfyClient;

static ACTIVE_SERIES: Mutex<String> = Mutex::new(String::new());

/// The series of the run in progress (set by the producer).
pub fn set_series(series: &str) {
    if let Ok(mut s) = ACTIVE_SERIES.lock() {
        *s = series.trim().to_string();
    }
}

fn active_series() -> String {
    ACTIVE_SERIES.lock().map(|s| s.clone()).unwrap_or_default()
}

/// `BUZZ_COMFY_PROFILES`, else `<repo>/assets/comfy_profiles`, else next to the exe.
pub fn profiles_root() -> PathBuf {
    if let Ok(d) = std::env::var("BUZZ_COMFY_PROFILES") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/comfy_profiles");
    if dev.is_dir() {
        return dev;
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("assets").join("comfy_profiles")))
        .unwrap_or_else(|| PathBuf::from("assets/comfy_profiles"))
}

/// The series name a profile / casting / settings folder is kept under:
/// "Chirailiya" and "Originals" are the same series (the Originals profile).
pub fn canonical_series(series: &str) -> String {
    let s = series.trim();
    if s.eq_ignore_ascii_case("chirailiya") || s.eq_ignore_ascii_case("originals") { "Originals".to_string() } else { s.to_string() }
}

/// Where a series' characters come from (`cast_sources` in `profile.json`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CastSources {
    /// Library folders (`Characters`, `Cast`) and their subfolders.
    pub library: Vec<String>,
    /// FLA files imported into the library for this series.
    pub fla: Vec<String>,
    /// Whether a story's own `animate/` folder is imported.
    pub story_animate_folder: bool,
    /// Asset-name globs left out (`Ana *`).
    pub exclude: Vec<String>,
    /// Extra story folders holding character pictures (read by the producer; informational here).
    pub extra_folders: Vec<String>,
}

/// What `profile.json` says.
#[derive(Debug, Clone)]
pub struct Profile {
    pub dir: PathBuf,
    pub background_workflow: String,
    pub character_workflow: String,
    /// F4: the camera-angle flow (`multiple_scene_angles-v1.0.json`); informational,
    /// the Qwen edit graph itself is built by [`qwen_angle_graph`].
    pub angle_workflow: String,
    pub base_size: (u32, u32),
    pub style_text: String,
    /// AN3: cut the background's objects into symbols (`None`: the series default).
    pub objects_as_symbols: Option<bool>,
    /// AN13: captions on the episode (`None`: the series default).
    pub captions: Option<bool>,
    /// Which characters this series casts from (`None`: the whole library).
    pub cast_sources: Option<CastSources>,
}

/// The profile of `series`, if it has one.
pub fn profile_for(series: &str) -> Option<Profile> {
    let series = series.trim();
    if series.is_empty() {
        return None;
    }
    let dir = profiles_root().join(canonical_series(series));
    let text = std::fs::read_to_string(dir.join("profile.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let size = v
        .get("base_size")
        .and_then(Value::as_array)
        .and_then(|a| Some((a.first()?.as_u64()? as u32, a.get(1)?.as_u64()? as u32)));
    Some(Profile {
        dir,
        background_workflow: s("background_workflow"),
        character_workflow: s("character_workflow"),
        angle_workflow: s("angle_workflow"),
        base_size: size.unwrap_or((1280, 720)),
        style_text: s("style_text"),
        objects_as_symbols: v.get("objects_as_symbols").and_then(Value::as_bool),
        captions: v.get("captions").and_then(Value::as_bool),
        cast_sources: v.get("cast_sources").map(|c| {
            let list = |k: &str| -> Vec<String> {
                c.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
            };
            CastSources {
                library: list("library"),
                fla: list("fla"),
                story_animate_folder: c.get("story_animate_folder").and_then(Value::as_bool).unwrap_or(false),
                exclude: list("exclude"),
                extra_folders: list("extra_folders"),
            }
        }),
    })
}

/// The LoRA file names a workflow file uses (ComfyUI UI or API format).
pub fn lora_names(workflow: &Value) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        if !n.trim().is_empty() && !found.iter().any(|f| f == n) {
            found.push(n.to_string());
        }
    };
    if let Some(nodes) = workflow.get("nodes").and_then(Value::as_array) {
        for n in nodes {
            if n.get("type").and_then(Value::as_str).is_some_and(|t| t.starts_with("LoraLoader"))
                && let Some(name) = n.pointer("/widgets_values/0").and_then(Value::as_str)
            {
                add(name);
            }
        }
    } else if let Some(map) = workflow.as_object() {
        for n in map.values() {
            if n.get("class_type").and_then(Value::as_str).is_some_and(|t| t.starts_with("LoraLoader"))
                && let Some(name) = n.pointer("/inputs/lora_name").and_then(Value::as_str)
            {
                add(name);
            }
        }
    }
    found
}

/// The Look step's "Pictures" line for `series`, and the workflow files its profile folder holds
/// (empty when the series has no profile).
pub fn pictures_line(series: &str) -> (String, Vec<String>) {
    let Some(p) = profile_for(series) else { return ("Pictures: the default Z-Image flow".into(), Vec::new()) };
    let file = p.background_workflow.clone();
    let loras = std::fs::read_to_string(p.dir.join(&file))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| lora_names(&v))
        .unwrap_or_default();
    let with = if loras.is_empty() { "no LoRA".to_string() } else { loras.join(", ") };
    let mut flows: Vec<String> = std::fs::read_dir(&p.dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".json") && n != "profile.json" && !n.ends_with(".api.json"))
                .collect()
        })
        .unwrap_or_default();
    flows.sort();
    (format!("Pictures: {file} ({with})"), flows)
}

/// Make `file` (a workflow in the series' profile folder) the flow for backgrounds and characters.
pub fn set_workflows(series: &str, file: &str) -> Result<(), String> {
    let path = profiles_root().join(canonical_series(series)).join("profile.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    v["background_workflow"] = Value::from(file);
    v["character_workflow"] = Value::from(file);
    std::fs::write(&path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))
}

/// AN13: whether the episode carries captions: the request's flag, else the
/// series profile's `captions`, else off for the story channels, on otherwise.
pub fn captions_enabled(request: Option<bool>, series: &str) -> bool {
    if let Some(b) = request {
        return b;
    }
    if let Some(b) = profile_for(series).and_then(|p| p.captions) {
        return b;
    }
    !matches!(series.trim().to_lowercase().as_str(), "khayal3baje" | "raat3baje" | "beyond3baje")
}

/// Background or character picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Background,
    Character,
}

/// The bound API workflow for the running series: `Ok(None)` when the series
/// has no profile (use the Z-Image default); `Err` when it has one but it
/// cannot be used (the caller falls back and logs).
pub fn bound_for_active(
    client: &ComfyClient,
    kind: Kind,
    prompt: &str,
    seed: u64,
    size: Option<(u32, u32)>,
) -> Result<Option<Value>, String> {
    let series = active_series();
    let Some(p) = profile_for(&series) else { return Ok(None) };
    let name = match kind {
        Kind::Background => &p.background_workflow,
        Kind::Character => &p.character_workflow,
    };
    if name.is_empty() {
        return Ok(None);
    }
    let mut api = load_api(&p.dir.join(name), Some(client))?;
    // F3: a model file the flow names that ComfyUI does not have is reported by
    // name, not as a 400 from ComfyUI.
    let missing = missing_models(&client.object_info()?, &api);
    if !missing.is_empty() {
        return Err(format!("{MISSING_PREFIX}{}", missing.join("; ")));
    }
    let full = if p.style_text.is_empty() { prompt.to_string() } else { format!("{prompt}, {}", p.style_text) };
    let prefix = if kind == Kind::Background { "buzzanimate-bg" } else { "buzzanimate-char" };
    bind(&mut api, &full, seed, size.unwrap_or(p.base_size), prefix)?;
    strip_upscale(&mut api);
    saves_into_comfy_output(&mut api, prefix);
    Ok(Some(api))
}

/// Start of the error `bound_for_active` returns when model files are missing
/// (the caller must not fall back to another flow then).
pub const MISSING_PREFIX: &str = "missing models: ";

/// F3: file inputs (`*_name` of a loader) of `api` whose value is not among
/// the choices ComfyUI's `/object_info` lists, as "<file> is not installed".
pub fn missing_models(info: &Value, api: &Value) -> Vec<String> {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for node in api.as_object().into_iter().flat_map(|o| o.values()) {
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (key, val) in inputs {
            let Some(file) = val.as_str() else { continue };
            if !key.ends_with("_name") || key == "sampler_name" {
                continue;
            }
            let spec = ["required", "optional"].iter().find_map(|g| info.get(class)?.get("input")?.get(*g)?.get(key));
            let Some(choices) = spec.and_then(|s| s.get(0)).and_then(Value::as_array) else { continue };
            if !choices.iter().filter_map(Value::as_str).any(|c| norm(c) == norm(file)) {
                let line = format!("{file} is not installed");
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
    }
    out
}

/// F3: the missing model files of the active series' workflow for `kind`
/// (empty = fine, or no profile).
pub fn missing_for_active(client: &ComfyClient, kind: Kind) -> Result<Vec<String>, String> {
    let Some(p) = profile_for(&active_series()) else { return Ok(Vec::new()) };
    let name = if kind == Kind::Background { &p.background_workflow } else { &p.character_workflow };
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let api = load_api(&p.dir.join(name), Some(client))?;
    Ok(missing_models(&client.object_info()?, &api))
}

// ------------------------------------------------------------------ angles (F4)

pub const QWEN_UNET: &str = "qwen_image_edit_2509_fp8_e4m3fn.safetensors";
pub const QWEN_VAE: &str = "qwen_image_vae.safetensors";
pub const QWEN_CLIP: &str = "qwen_2.5_vl_7b_fp8_scaled.safetensors";
pub const QWEN_LORA_ANGLES: &str = "Qwen-Edit-2509-Multiple-angles.safetensors";
pub const QWEN_LORA_LIGHTNING: &str = "Qwen-Image-Edit-2509-Lightning-4steps-V1.0-bf16.safetensors";

/// The camera-angle prompt of his `multiple_scene_angles` flow for a shot
/// camera: low_angle -> low-angle view, high_angle -> aerial view, over_shoulder
/// -> 45 degrees, reverse -> 90 degrees (left or right). `None`: the camera
/// has no angle picture (wide, medium, close use the wide picture).
pub fn angle_prompt(camera: &str, left: bool) -> Option<&'static str> {
    Some(match (camera, left) {
        ("low_angle", _) => "Turn the camera to a low-angle view.",
        ("high_angle", _) => "Turn the camera to an aerial view.",
        ("over_shoulder", true) => "Rotate the camera 45 degrees to the left.",
        ("over_shoulder", false) => "Rotate the camera 45 degrees to the right.",
        ("reverse", true) => "Rotate the camera 90 degrees to the left.",
        ("reverse", false) => "Rotate the camera 90 degrees to the right.",
        _ => return None,
    })
}

/// The Qwen Image Edit 2509 API graph of one "Generate ..." subgraph of his
/// flow, built from that subgraph's own node settings (Lightning then
/// Multiple-angles LoRA, ModelSamplingAuraFlow 3, CFGNorm 1, picture scaled to
/// 1 MP, KSampler euler/simple 4 steps cfg 1, an empty negative prompt).
/// `image` is the name of the uploaded wide picture.
pub fn qwen_angle_graph(image: &str, prompt: &str, seed: u64, prefix: &str) -> Value {
    json!({
        "1": {"class_type": "VAELoader", "inputs": {"vae_name": QWEN_VAE}},
        "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": QWEN_CLIP, "type": "qwen_image", "device": "default"}},
        "3": {"class_type": "UNETLoader", "inputs": {"unet_name": QWEN_UNET, "weight_dtype": "default"}},
        "4": {"class_type": "LoraLoaderModelOnly", "inputs": {"model": ["3", 0], "lora_name": QWEN_LORA_LIGHTNING, "strength_model": 1}},
        "5": {"class_type": "LoraLoaderModelOnly", "inputs": {"model": ["4", 0], "lora_name": QWEN_LORA_ANGLES, "strength_model": 1}},
        "6": {"class_type": "LoadImage", "inputs": {"image": image}},
        "7": {"class_type": "ImageScaleToTotalPixels", "inputs": {"image": ["6", 0], "upscale_method": "nearest-exact", "megapixels": 1, "resolution_steps": 1}},
        "8": {"class_type": "ModelSamplingAuraFlow", "inputs": {"model": ["5", 0], "shift": 3}},
        "9": {"class_type": "CFGNorm", "inputs": {"model": ["8", 0], "strength": 1}},
        "10": {"class_type": "VAEEncode", "inputs": {"pixels": ["7", 0], "vae": ["1", 0]}},
        "11": {"class_type": "TextEncodeQwenImageEditPlus", "inputs": {"clip": ["2", 0], "vae": ["1", 0], "image1": ["7", 0], "prompt": prompt}},
        "12": {"class_type": "TextEncodeQwenImageEditPlus", "inputs": {"clip": ["2", 0], "vae": ["1", 0], "image1": ["7", 0], "prompt": ""}},
        "13": {"class_type": "KSampler", "inputs": {"model": ["9", 0], "positive": ["11", 0], "negative": ["12", 0], "latent_image": ["10", 0], "seed": seed, "steps": 4, "cfg": 1, "sampler_name": "euler", "scheduler": "simple", "denoise": 1}},
        "14": {"class_type": "VAEDecode", "inputs": {"samples": ["13", 0], "vae": ["1", 0]}},
        "15": {"class_type": "SaveImage", "inputs": {"images": ["14", 0], "filename_prefix": prefix}}
    })
}

// ------------------------------------------------------------------ cache

fn fnv(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// A short content hash (file names).
pub fn short_hash(bytes: &[u8]) -> String {
    fnv(bytes)
}

/// The API-format workflow for a UI-format copy. Reuses `<name>.api.json` when
/// the copy is unchanged; otherwise converts through `/object_info`.
pub fn load_api(copy: &Path, client: Option<&ComfyClient>) -> Result<Value, String> {
    let bytes = std::fs::read(copy).map_err(|e| format!("{}: {e}", copy.display()))?;
    let hash = fnv(&bytes);
    let cache = copy.with_extension("api.json");
    if let Ok(text) = std::fs::read_to_string(&cache) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if v.get("source_hash").and_then(Value::as_str) == Some(hash.as_str()) {
                if let Some(api) = v.get("api") {
                    return Ok(api.clone());
                }
            }
        }
    }
    let client = client.ok_or_else(|| {
        format!("{} has no converted copy and ComfyUI is not reachable to convert it", copy.display())
    })?;
    let info: Value = ureq::get(&format!("{}/object_info", client.base_url))
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| format!("ComfyUI /object_info: {e}"))?
        .into_json()
        .map_err(|e| format!("ComfyUI /object_info: {e}"))?;
    let ui: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", copy.display()))?;
    let api = ui_to_api(&ui, &info)?;
    let wrapped = json!({"source_hash": hash, "api": api});
    let _ = std::fs::write(&cache, serde_json::to_string_pretty(&wrapped).unwrap_or_default());
    Ok(api)
}

// ------------------------------------------------------------------ UI -> API

const UI_ONLY: &[&str] = &["Note", "MarkdownNote", "Reroute", "PrimitiveNode"];

fn is_widget_type(spec: &Value) -> bool {
    match spec.get(0) {
        Some(Value::Array(_)) => true,
        Some(Value::String(t)) => matches!(t.as_str(), "INT" | "FLOAT" | "STRING" | "BOOLEAN" | "COMBO"),
        _ => false,
    }
}

/// Widget input names of a class in ComfyUI's order, with whether a
/// control-after-generate value follows each (the UI saves it as an extra value).
fn widget_names(info: &Value, class: &str) -> Result<Vec<(String, bool)>, String> {
    let c = info.get(class).ok_or_else(|| format!("ComfyUI does not know the node class '{class}'"))?;
    let mut out = Vec::new();
    for group in ["required", "optional"] {
        let order = c.get("input_order").and_then(|o| o.get(group)).and_then(Value::as_array);
        let Some(specs) = c.get("input").and_then(|i| i.get(group)).and_then(Value::as_object) else { continue };
        let Some(order) = order else {
            return Err(format!("ComfyUI /object_info gives no input_order for '{class}' (ComfyUI too old to convert a workflow)"));
        };
        for name in order.iter().filter_map(Value::as_str) {
            let Some(spec) = specs.get(name) else { continue };
            if is_widget_type(spec) {
                let ctl = spec.get(1).and_then(|o| o.get("control_after_generate")).and_then(Value::as_bool).unwrap_or(false)
                    || matches!(name, "seed" | "noise_seed");
                out.push((name.to_string(), ctl));
            }
        }
    }
    Ok(out)
}

fn node_by_id(ui: &Value, id: u64) -> Option<&Value> {
    ui.get("nodes")?.as_array()?.iter().find(|n| n.get("id").and_then(Value::as_u64) == Some(id))
}

/// `(origin_id, origin_slot)` of a link id.
fn link_origin(ui: &Value, link: u64) -> Option<(u64, u64)> {
    ui.get("links")?.as_array()?.iter().find_map(|l| {
        let a = l.as_array()?;
        if a.first()?.as_u64()? == link { Some((a.get(1)?.as_u64()?, a.get(2)?.as_u64()?)) } else { None }
    })
}

/// What a link really comes from: reroutes are followed; a primitive node
/// yields its value.
enum Source {
    Node(u64, u64),
    Value(Value),
}

fn resolve(ui: &Value, link: u64) -> Result<Source, String> {
    let mut link = link;
    for _ in 0..32 {
        let (id, slot) = link_origin(ui, link).ok_or_else(|| format!("link {link} has no origin"))?;
        let node = node_by_id(ui, id).ok_or_else(|| format!("link {link}: node {id} missing"))?;
        match node.get("type").and_then(Value::as_str).unwrap_or("") {
            "Reroute" => {
                link = node
                    .get("inputs")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(|i| i.get("link"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("reroute {id} has no input"))?;
            }
            "PrimitiveNode" => {
                let v = node
                    .get("widgets_values")
                    .and_then(|w| w.get(0))
                    .cloned()
                    .ok_or_else(|| format!("primitive {id} has no value"))?;
                return Ok(Source::Value(v));
            }
            _ => return Ok(Source::Node(id, slot)),
        }
    }
    Err("reroute loop".into())
}

/// Convert a UI-format graph to API format using `/object_info` (`info`).
pub fn ui_to_api(ui: &Value, info: &Value) -> Result<Value, String> {
    let nodes = ui.get("nodes").and_then(Value::as_array).ok_or("not a UI-format workflow (no nodes)")?;
    let mut api = Map::new();
    for node in nodes {
        let class = node.get("type").and_then(Value::as_str).unwrap_or("");
        let id = node.get("id").and_then(Value::as_u64).ok_or("a node has no id")?;
        let mode = node.get("mode").and_then(Value::as_u64).unwrap_or(0);
        if UI_ONLY.contains(&class) || mode == 2 || mode == 4 {
            continue;
        }
        let mut inputs = Map::new();
        let mut linked = std::collections::HashSet::new();
        for inp in node.get("inputs").and_then(Value::as_array).into_iter().flatten() {
            let (Some(name), Some(link)) = (inp.get("name").and_then(Value::as_str), inp.get("link").and_then(Value::as_u64))
            else {
                continue;
            };
            match resolve(ui, link)? {
                Source::Node(src, slot) => {
                    inputs.insert(name.into(), json!([src.to_string(), slot]));
                }
                Source::Value(v) => {
                    inputs.insert(name.into(), v);
                }
            }
            linked.insert(name.to_string());
        }
        let values = node.get("widgets_values").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut vi = 0;
        for (name, ctl) in widget_names(info, class)? {
            let Some(v) = values.get(vi) else { break };
            vi += 1;
            if ctl {
                vi += 1; // the UI saves "randomize" / "fixed" after a seed
            }
            if !linked.contains(&name) {
                inputs.insert(name, v.clone());
            }
        }
        api.insert(id.to_string(), json!({"class_type": class, "inputs": inputs}));
    }
    Ok(Value::Object(api))
}

// ------------------------------------------------------------------ binder

/// Which nodes were bound (ids), for logs and tests.
#[derive(Debug, Default, Clone)]
pub struct Bound {
    pub prompt: Vec<String>,
    pub negative: Vec<String>,
    pub latent: Vec<String>,
    pub samplers: Vec<String>,
    pub saves: Vec<String>,
}

fn class_of<'a>(api: &'a Value, id: &str) -> &'a str {
    api.get(id).and_then(|n| n.get("class_type")).and_then(Value::as_str).unwrap_or("")
}

/// The CLIPTextEncode a conditioning input comes from (walking through
/// conditioning-only nodes).
fn text_node(api: &Value, start: &Value) -> Option<String> {
    let mut cur = start.as_array()?.first()?.as_str()?.to_string();
    for _ in 0..8 {
        if class_of(api, &cur).starts_with("CLIPTextEncode") {
            return Some(cur);
        }
        let next = api.get(&cur)?.get("inputs")?.as_object()?.values().find_map(|v| {
            let a = v.as_array()?;
            let id = a.first()?.as_str()?;
            if a.len() == 2 && a[1].is_u64() && !class_of(api, id).is_empty() { Some(id.to_string()) } else { None }
        })?;
        cur = next;
    }
    None
}

fn set(api: &mut Value, id: &str, key: &str, v: Value) {
    if let Some(inputs) = api.get_mut(id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        inputs.insert(key.into(), v);
    }
}

/// Bind prompt, seed, size and prefix by class and wiring.
pub fn bind(api: &mut Value, prompt: &str, seed: u64, size: (u32, u32), prefix: &str) -> Result<Bound, String> {
    let ids: Vec<String> = api.as_object().ok_or("not an API workflow")?.keys().cloned().collect();
    let mut b = Bound::default();
    let mut latents: Vec<String> = Vec::new();
    for id in &ids {
        let class = class_of(api, id).to_string();
        if class == "KSampler" || class == "KSamplerAdvanced" {
            b.samplers.push(id.clone());
            let inputs = api[id]["inputs"].clone();
            if let Some(t) = inputs.get("positive").and_then(|p| text_node(api, p)) {
                if !b.prompt.contains(&t) {
                    b.prompt.push(t);
                }
            }
            if let Some(t) = inputs.get("negative").and_then(|p| text_node(api, p)) {
                if !b.negative.contains(&t) {
                    b.negative.push(t);
                }
            }
            if let Some(l) = inputs.get("latent_image").and_then(|l| l.get(0)).and_then(Value::as_str) {
                latents.push(l.to_string());
            }
        }
    }
    if b.samplers.is_empty() {
        return Err("the workflow has no KSampler".into());
    }
    if b.prompt.is_empty() {
        return Err("no CLIPTextEncode feeds a sampler's positive".into());
    }
    for id in &b.samplers {
        let key = if class_of(api, id) == "KSamplerAdvanced" { "noise_seed" } else { "seed" };
        set(api, id, key, Value::from(seed));
    }
    for id in &b.prompt.clone() {
        if class_of(api, id) == "CLIPTextEncodeSDXL" {
            set(api, id, "text_g", Value::from(prompt));
            set(api, id, "text_l", Value::from(prompt));
        } else {
            set(api, id, "text", Value::from(prompt));
        }
    }
    // The latent wired to a sampler, else any empty-latent node.
    let is_latent = |api: &Value, id: &str| class_of(api, id).starts_with("Empty") && class_of(api, id).contains("Latent");
    let mut targets: Vec<String> = latents.iter().filter(|l| is_latent(api, l)).cloned().collect();
    if targets.is_empty() {
        targets = ids.iter().filter(|i| is_latent(api, i)).cloned().collect();
    }
    for id in targets {
        if !b.latent.contains(&id) {
            set(api, &id, "width", Value::from(size.0));
            set(api, &id, "height", Value::from(size.1));
            b.latent.push(id);
        }
    }
    if b.latent.is_empty() {
        return Err("the workflow has no empty-latent size node".into());
    }
    for id in &ids {
        let class = class_of(api, id);
        if class == "SaveImage" || class == "Image Save" {
            set(api, id, "filename_prefix", Value::from(prefix));
            b.saves.push(id.clone());
        }
    }
    Ok(b)
}

const UPSCALE_CLASSES: &[&str] = &["ImageUpscaleWithModel", "ImageScaleBy", "ImageScale", "UpscaleModelLoader"];

/// AN7: the producer upscales every picture itself (RealESRGAN, then a
/// quality gate), so a flow's own upscale chain (his `4x-UltraSharp` is not
/// installed) is removed and whatever read its result reads the picture before it.
pub fn strip_upscale(api: &mut Value) {
    let ids: Vec<String> = match api.as_object() {
        Some(o) => o.keys().cloned().collect(),
        None => return,
    };
    // Where an image output really comes from once the chain is skipped.
    let source = |api: &Value, start: &Value| -> Option<Value> {
        let mut cur = start.clone();
        for _ in 0..16 {
            let id = cur.get(0)?.as_str()?.to_string();
            let class = class_of(api, &id);
            if !UPSCALE_CLASSES.contains(&class) {
                return Some(cur);
            }
            cur = api.get(&id)?.get("inputs")?.get("image")?.clone();
        }
        None
    };
    for id in &ids {
        if UPSCALE_CLASSES.contains(&class_of(api, id)) {
            continue;
        }
        let inputs = match api[id].get("inputs").and_then(Value::as_object) {
            Some(i) => i.clone(),
            None => continue,
        };
        for (k, v) in inputs {
            let Some(src) = v.get(0).and_then(Value::as_str) else { continue };
            if UPSCALE_CLASSES.contains(&class_of(api, src)) {
                if let Some(new) = source(api, &v) {
                    set(api, id, &k, new);
                }
            }
        }
    }
    if let Some(o) = api.as_object_mut() {
        o.retain(|_, n| !UPSCALE_CLASSES.contains(&n.get("class_type").and_then(Value::as_str).unwrap_or("")));
    }
}

/// AN1b: a WAS "Image Save" node writes to its own `output_path` (his story
/// folders). It becomes a plain `SaveImage`, so the picture goes to ComfyUI's
/// own output folder, is read back through `/view`, and nothing lands in the
/// creator's folders.
pub fn saves_into_comfy_output(api: &mut Value, prefix: &str) {
    let ids: Vec<String> = api.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    for id in ids {
        if class_of(api, &id) == "Image Save" {
            let images = api[&id]["inputs"].get("images").cloned().unwrap_or(Value::Null);
            api[&id] = json!({"class_type": "SaveImage", "inputs": {"images": images, "filename_prefix": prefix}});
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chirailiya_is_the_originals_series() {
        assert_eq!(canonical_series("Chirailiya"), "Originals");
        assert_eq!(canonical_series(" originals "), "Originals");
        assert_eq!(canonical_series("Khayal3Baje"), "Khayal3Baje");
    }

    use super::*;

    fn spec(widgets: &[(&str, &str)], links: &[&str]) -> Value {
        let mut req = Map::new();
        for l in links {
            req.insert((*l).into(), json!(["LINK"]));
        }
        for (n, t) in widgets {
            req.insert((*n).into(), json!([t]));
        }
        let order: Vec<&str> = widgets.iter().map(|(n, _)| *n).collect();
        json!({"input": {"required": req}, "input_order": {"required": order}})
    }

    #[test]
    fn missing_models_are_named_and_the_angle_graph_is_wired() {
        let info = json!({
            "UNETLoader": {"input": {"required": {"unet_name": [["a.safetensors"]], "weight_dtype": [["default"]]}}},
            "KSampler": {"input": {"required": {"sampler_name": [["euler"]]}}}
        });
        let api = json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "zImageTurbo_turbo.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "UNETLoader", "inputs": {"unet_name": "A.safetensors"}},
            "3": {"class_type": "KSampler", "inputs": {"sampler_name": "dpm"}}
        });
        assert_eq!(missing_models(&info, &api), vec!["zImageTurbo_turbo.safetensors is not installed"]);
        let g = qwen_angle_graph("wide.png", angle_prompt("reverse", false).unwrap(), 7, "p");
        assert_eq!(g["11"]["inputs"]["prompt"], "Rotate the camera 90 degrees to the right.");
        assert_eq!(g["13"]["inputs"]["seed"], 7);
        assert_eq!(g["6"]["inputs"]["image"], "wide.png");
        assert!(angle_prompt("medium", true).is_none() && angle_prompt("close", true).is_none());
    }

    #[test]
    fn converts_link_through_reroute() {
        let info = json!({
            "A": spec(&[], &[]),
            "B": spec(&[("text", "STRING"), ("seed", "INT"), ("n", "INT")], &["x"]),
        });
        let ui = json!({
            "nodes": [
                {"id": 1, "type": "A", "inputs": [], "widgets_values": []},
                {"id": 2, "type": "Reroute", "inputs": [{"name": "", "link": 1}]},
                {"id": 3, "type": "Note", "widgets_values": ["hi"]},
                {"id": 4, "type": "B", "inputs": [{"name": "x", "link": 2}], "widgets_values": ["hello", 7, "fixed", 3]}
            ],
            "links": [[1, 1, 0, 2, 0, "X"], [2, 2, 0, 4, 0, "X"]]
        });
        let api = ui_to_api(&ui, &info).unwrap();
        assert!(api.get("2").is_none() && api.get("3").is_none());
        assert_eq!(api["4"]["inputs"]["x"], json!(["1", 0]));
        assert_eq!(api["4"]["inputs"]["text"], json!("hello"));
        assert_eq!(api["4"]["inputs"]["seed"], json!(7));
        assert_eq!(api["4"]["inputs"]["n"], json!(3));
    }

    #[test]
    fn binds_the_khayal3baje_workflow_by_wiring() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/comfy_profiles/Khayal3Baje/urbanScenesKhayal3baje.json");
        let ui: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let w = |n: &[(&str, &str)]| spec(n, &[]);
        let info = json!({
            "VAEDecode": w(&[]), "PerturbedAttentionGuidance": w(&[("scale", "FLOAT")]),
            "CLIPTextEncode": w(&[("text", "STRING")]), "UpscaleModelLoader": w(&[("model_name", "STRING")]),
            "ImageUpscaleWithModel": w(&[]), "ImageScaleBy": w(&[("upscale_method", "STRING"), ("scale_by", "FLOAT")]),
            "KSampler": w(&[("seed", "INT"), ("steps", "INT"), ("cfg", "FLOAT"), ("sampler_name", "STRING"), ("scheduler", "STRING"), ("denoise", "FLOAT")]),
            "EmptyLatentImage": w(&[("width", "INT"), ("height", "INT"), ("batch_size", "INT")]),
            "Image Save": w(&[("output_path", "STRING"), ("filename_prefix", "STRING")]),
            "PreviewImage": w(&[]),
            "LoraLoader": w(&[("lora_name", "STRING"), ("strength_model", "FLOAT"), ("strength_clip", "FLOAT")]),
            "CheckpointLoaderSimple": w(&[("ckpt_name", "STRING")]),
        });
        let mut api = ui_to_api(&ui, &info).unwrap();
        let negative_before = api["7"]["inputs"]["text"].clone();
        let b = bind(&mut api, "MY PROMPT", 42, (1000, 600), "pfx").unwrap();
        assert_eq!(b.prompt, vec!["6"]);
        assert_eq!(b.negative, vec!["7"]);
        assert_eq!(api["6"]["inputs"]["text"], "MY PROMPT");
        assert_eq!(api["7"]["inputs"]["text"], negative_before);
        assert_eq!(api["3"]["inputs"]["seed"], 42);
        assert_eq!(api["5"]["inputs"]["width"], 1000);
        assert_eq!(api["5"]["inputs"]["height"], 600);
        assert_eq!(api["72"]["inputs"]["filename_prefix"], "pfx");
        // The primitive node's value reached the LoRA's linked strength.
        assert_eq!(api["10"]["inputs"]["strength_model"], 1.01);
    }

    #[test]
    fn binds_the_z_image_lora_workflow_and_rebinds_the_save() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/comfy_profiles/Khayal3Baje/image_z_image_turboLora2.json");
        let ui: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let w = |n: &[(&str, &str)]| spec(n, &[]);
        let info = json!({
            "VAELoader": w(&[("vae_name", "STRING")]), "VAEDecode": w(&[]),
            "EmptySD3LatentImage": w(&[("width", "INT"), ("height", "INT"), ("batch_size", "INT")]),
            "ConditioningZeroOut": w(&[]), "ImageUpscaleWithModel": w(&[]),
            "ModelSamplingAuraFlow": w(&[("shift", "FLOAT")]),
            "KSampler": w(&[("seed", "INT"), ("steps", "INT"), ("cfg", "FLOAT"), ("sampler_name", "STRING"), ("scheduler", "STRING"), ("denoise", "FLOAT")]),
            "ImageScaleBy": w(&[("upscale_method", "STRING"), ("scale_by", "FLOAT")]),
            "SaveImage": w(&[("filename_prefix", "STRING")]), "UpscaleModelLoader": w(&[("model_name", "STRING")]),
            "Image Save": w(&[("output_path", "STRING"), ("filename_prefix", "STRING")]),
            "PreviewImage": w(&[]), "CLIPLoader": w(&[("clip_name", "STRING"), ("type", "STRING"), ("device", "STRING")]),
            "UNETLoader": w(&[("unet_name", "STRING"), ("weight_dtype", "STRING")]),
            "CLIPTextEncode": w(&[("text", "STRING")]),
            "LoraLoaderModelOnly": w(&[("lora_name", "STRING"), ("strength_model", "FLOAT")]),
        });
        let mut api = ui_to_api(&ui, &info).unwrap();
        let b = bind(&mut api, "MY PROMPT", 99, (1280, 720), "pfx").unwrap();
        assert_eq!(b.prompt, vec!["69"]);
        assert_eq!(b.latent, vec!["70"]);
        assert_eq!(b.samplers, vec!["72"]);
        assert_eq!(api["69"]["inputs"]["text"], "MY PROMPT");
        assert_eq!(api["72"]["inputs"]["seed"], 99);
        assert_eq!(api["70"]["inputs"]["width"], 1280);
        assert_eq!(b.saves, vec!["79"]);
        strip_upscale(&mut api);
        saves_into_comfy_output(&mut api, "pfx");
        assert!(api.get("73").is_none() && api.get("74").is_none() && api.get("75").is_none());
        assert_eq!(api["79"]["class_type"], "SaveImage");
        assert_eq!(api["79"]["inputs"]["images"], json!(["67", 0]));
        assert!(api["79"]["inputs"].get("output_path").is_none());
        assert_eq!(api["76"]["inputs"]["images"], json!(["67", 0]));
    }
}
