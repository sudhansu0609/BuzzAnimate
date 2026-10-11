//! R2 + R3 of `docs/SERIES_EP1_PLAN.md`: one rigged character per view image of
//! a series episode, arms relaxed down from the T-pose, saved to the user's Cast.
//!
//! ```text
//! cargo run --release -p buzz-app --example series_setup -- \
//!     --ep B:\youtubeProjects\Originals\series\ep1 --out B:\youtubeProjects\Originals\series\ep1\buzz \
//!     [--replace] [--only "Guddu,Guddu Back"] [--hang 75] [--no-cast] [--tuning <file>] [--bold <px>]
//!     [--rig bones|layers]   (default layers: the baked rig taken apart into followed layers; bones keeps the armature)
//! ```
//!
//! For each row of [`VIEWS`] it drives a headless `App` through the same agent
//! tools the Assistant uses: `ai_character_from_image` (wait for the job), then
//! `bones get` and `set_property bone.<n>.angle` (R1) to swing each upper-arm
//! bone down to `--hang` degrees below horizontal on its own side. The pose is
//! then **baked**: `Set Rest Pose` alone only re-measures the bones against the
//! artwork as drawn (the arms would snap back to the T-pose on screen), so the
//! example loads the saved document, replaces every part's artwork with its
//! posed copy (`ArmatureData::posed`), adopts the pose as the rest pose and
//! rebinds. The result is saved to `<out>/rigs/<name>.buzz` with a preview PNG
//! and, unless `--no-cast`, into `Cast/<name>` of the user's asset library.
//!
//! The pipeline's own save (a T-pose `Cast` asset) goes to a scratch library, so
//! the only thing written to the user's Cast is the finished rig. If a Cast
//! asset of the exact name exists it is skipped, or overwritten with
//! `--replace`. **No other asset is ever deleted or touched.**
//!
//! After the rigs (R4) it builds Scene 5, "Raju arrives": daylight and a village,
//! the five front rigs placed from Cast on one ground line at one height, idle /
//! talk / walk performances, saved as `<out>/ep1_scene5_setup.buzz` with preview
//! PNGs. `--scene-only` skips the rigs, `--rigs-only` skips the scene.
//! Diagnostics: `--skeleton <image>` (DWPose joints) and `--parts <Cast name>`.
//!
//! The rigs are traced with the saved tracer tuning (`trace_tuning.json`).
//! `--tuning <file>` sets `BUZZ_TRACE_TUNING` and `--bold <px>` sets
//! `BUZZ_TRACE_BOLD` for this process before anything traces, so the
//! Assistant's `ai_character_from_image` and the corrected-wrist rebuild both
//! take them (`TraceOptions::with_saved_tuning`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use buzz_app::agent::{AgentCall, AgentResult, execute};
use buzz_app::app::App;
use buzz_render::GpuPreference;
use buzz_scene::{ObjectId, ObjectKind, Scene};
use serde_json::{Value, json};

/// `(character folder, image file, Cast name)`: the plan's image to view table.
const VIEWS: &[(&str, &str, &str)] = &[
    ("guddu", "Goofy Indian Character in T-Pose.png", "Guddu"),
    ("guddu", "Guddu in a T-Pose Pose Sheet.png", "Guddu 3-4"),
    ("guddu", "Back View Character in T-Pose.png", "Guddu Back"),
    ("lalten", "Lalten in T-Pose Character Design.png", "Lalten"),
    ("lalten", "Cartoon Man in T-Pose with Scarf.png", "Lalten Back"),
    ("munna", "Anxious T-Pose Character in Checked Shirt.png", "Munna"),
    ("munna", "Munna Back-View Character Turnaround.png", "Munna Back"),
    ("raju", "ChatGPT Image Oct 6, 2026, 03_31_26 AM-1.png", "Raju"),
    ("raju", "ChatGPT Image Oct 6, 2026, 03_31_27 AM-2.png", "Raju Back"),
    ("woman1", "Cartoon Indian Woman in Traditional Sari.png", "Woman1"),
    ("woman1", "Indian Sari Character in T-Pose.png", "Woman1 3-4"),
    ("woman1", "Back View of Woman in Mustard Sari.png", "Woman1 Back"),
];

/// `(Cast name, character folder, mouth chart image)`: a rig built for this Cast
/// name gets the chart's drawn mouths as its mouth-symbol frames (M2,
/// `ai_mouth_chart`) before it is saved, so a rerun keeps them. Only Raju has a
/// chart so far.
const MOUTH_CHARTS: &[(&str, &str, &str)] = &[("Raju", "raju", "ChatGPT Image Oct 6, 2026, 03_29_03 AM.png")];

struct Args {
    ep: PathBuf,
    out: PathBuf,
    replace: bool,
    only: Option<Vec<String>>,
    hang: f64,
    cast: bool,
    rigs: bool,
    scene: bool,
    tuning: Option<PathBuf>,
    bold: Option<f64>,
    rig: buzz_rig::RigKind,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

/// [`parse_args`] over any argument list (the program name already skipped).
fn parse_args_from(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        ep: PathBuf::new(),
        out: PathBuf::new(),
        replace: false,
        only: None,
        hang: 75.0,
        cast: true,
        rigs: true,
        scene: true,
        tuning: None,
        bold: None,
        // LR2: layer rigs unless `--rig bones`.
        rig: buzz_rig::RigKind::Layers,
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ep" => args.ep = it.next().ok_or("--ep needs a folder")?.into(),
            "--out" => args.out = it.next().ok_or("--out needs a folder")?.into(),
            "--replace" => args.replace = true,
            "--no-cast" => args.cast = false,
            // Only the scene (R4), from the rigs already in Cast; or only the rigs (R2/R3).
            "--scene-only" => args.rigs = false,
            "--rigs-only" => args.scene = false,
            "--only" => {
                args.only = Some(
                    it.next()
                        .ok_or("--only needs names")?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect(),
                )
            }
            "--hang" => args.hang = it.next().ok_or("--hang needs degrees")?.parse().map_err(|_| "--hang needs a number")?,
            "--tuning" => args.tuning = Some(it.next().ok_or("--tuning needs a file")?.into()),
            "--rig" => {
                args.rig = buzz_rig::RigKind::parse(&it.next().ok_or("--rig needs bones or layers")?)
                    .ok_or("--rig needs bones or layers")?
            }
            "--bold" => args.bold = Some(it.next().ok_or("--bold needs px")?.parse().map_err(|_| "--bold needs a number")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if args.ep.as_os_str().is_empty() || args.out.as_os_str().is_empty() {
        return Err("usage: series_setup --ep <ep dir> --out <dir> [--replace] [--only a,b] [--hang 75] [--no-cast] [--scene-only|--rigs-only] [--tuning <file>] [--bold <px>] [--rig bones|layers]".into());
    }
    Ok(args)
}

fn call(app: &mut App, tool: &str, args: Value) -> AgentResult {
    execute(app, &AgentCall { tool: tool.to_string(), args })
}

fn must(r: AgentResult, what: &str) -> Result<Value, String> {
    if r.ok { Ok(r.value) } else { Err(format!("{what}: {}", r.error.unwrap_or_default())) }
}

/// Wait for the one job in flight and hand back its listing.
fn wait_job(app: &mut App, secs: u64) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let r = must(call(app, "wait_for_jobs", json!({"timeout_s": 30})), "wait_for_jobs")?;
        if r["running"] == json!(0) {
            let job = r["jobs"].as_array().and_then(|j| j.first()).cloned().ok_or("no job was listed")?;
            return Ok(job);
        }
        if started.elapsed() > Duration::from_secs(secs) {
            return Err(format!("the job was still running after {secs}s"));
        }
    }
}

fn wrap_deg(mut a: f64) -> f64 {
    while a > 180.0 {
        a -= 360.0;
    }
    while a <= -180.0 {
        a += 360.0;
    }
    a
}

#[derive(Debug, Clone)]
struct BoneInfo {
    name: String,
    parent: Option<usize>,
    angle: f64,
}

fn read_bones(app: &mut App, element: u64) -> Result<Vec<BoneInfo>, String> {
    let target = format!("element:{element}");
    let list = must(
        call(app, "edit_list", json!({"target": target, "list": "bones", "op": "get"})),
        "bones get",
    )?;
    let props = must(call(app, "inspect", json!({"what": "properties", "target": target})), "inspect properties")?;
    let angle_of = |i: usize| {
        props["fields"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["field"] == json!(format!("bone.{i}.angle"))))
            .and_then(|f| f["value"].as_f64())
    };
    let items = list["items"].as_array().ok_or("bones get returned no items")?;
    items
        .iter()
        .enumerate()
        .map(|(i, b)| {
            Ok(BoneInfo {
                name: b["name"].as_str().unwrap_or("").to_string(),
                parent: b["parent"].as_u64().map(|p| p as usize),
                angle: angle_of(i).ok_or_else(|| format!("bone.{i}.angle is not listed"))?,
            })
        })
        .collect()
}

fn world_angle(bones: &[BoneInfo], index: usize) -> f64 {
    let mut total = 0.0;
    let mut current = Some(index);
    let mut guard = 0;
    while let Some(i) = current {
        total += bones[i].angle;
        current = bones[i].parent;
        guard += 1;
        if guard > bones.len() {
            break;
        }
    }
    wrap_deg(total)
}

fn is_upper_arm(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("shoulder") || n.contains("upper arm") || n.contains("upperarm") || n.contains("bicep")
}

/// R2: swing each upper arm to `hang` degrees below horizontal on its own side.
/// Returns what was done, for the report.
fn relax_arms(app: &mut App, element: u64, hang: f64) -> Result<Vec<Value>, String> {
    let target = format!("element:{element}");
    let bones = read_bones(app, element)?;
    let mut done = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        if !is_upper_arm(&bone.name) {
            continue;
        }
        let world = world_angle(&bones, i);
        // y points down: +90 is straight down. An arm on the right (cos > 0)
        // hangs at +hang; one on the left at 180 - hang.
        let goes_right = world.to_radians().cos() >= 0.0;
        let wanted_world = if goes_right { hang } else { 180.0 - hang };
        let local = wrap_deg(bone.angle + wrap_deg(wanted_world - world));
        let mut r = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.angle"), "value": local}));
        let mut note = String::new();
        let mut now = read_bones(app, element)?;
        if (wrap_deg(world_angle(&now, i) - wanted_world)).abs() > 0.5 {
            // The joint's limits kept it from hanging: lift them (an arm that
            // cannot hang is no use to anyone) and say so.
            let lifted = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.limited"), "value": false}));
            if lifted.ok {
                r = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.angle"), "value": local}));
                note = "joint limits lifted".into();
                now = read_bones(app, element)?;
            }
        }
        if !r.ok {
            return Err(format!("bone.{i}.angle: {}", r.error.unwrap_or_default()));
        }
        done.push(json!({
            "bone": i, "name": bone.name,
            "world_before": world, "world_after": world_angle(&now, i),
            "local_before": bone.angle, "local_after": now[i].angle,
            "side": if goes_right { "right" } else { "left" }, "note": note,
        }));
    }
    if done.is_empty() {
        return Err(format!(
            "no upper-arm bone found among {:?}",
            bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>()
        ));
    }
    Ok(done)
}

/// Adopt the pose the bones are in as the rig's drawn pose, in the artwork too.
///
/// **And hold the mouth.** A face-rig mouth is an instance of a multi-frame viseme
/// symbol, and a looping one flickers through the visemes on any timeline. The
/// pipeline now builds it holding Rest (`assemble_character`); a face part with
/// more than one frame is still set to hold its first here, as a guard.
fn bake_pose(scene: &mut Scene) -> usize {
    let lengths: std::collections::HashMap<buzz_scene::SymbolId, u32> =
        scene.library().iter().map(|s| (s.id, s.length())).collect();
    let ids: Vec<ObjectId> = scene
        .layers()
        .iter()
        .flat_map(|l| l.all_objects().filter(|o| matches!(o.kind, ObjectKind::Armature(_))).map(|o| o.id).collect::<Vec<_>>())
        .collect();
    for id in &ids {
        scene.update_object(*id, |o| {
            if let ObjectKind::Armature(rig) = &mut o.kind {
                let posed = rig.posed();
                for (part, p) in rig.parts.iter_mut().zip(posed) {
                    part.artwork = p;
                    let long = matches!(&part.artwork.kind, ObjectKind::Instance(i) if lengths.get(&i.symbol).is_some_and(|n| *n > 1));
                    if long && let ObjectKind::Instance(inst) = &mut std::sync::Arc::make_mut(&mut part.artwork).kind {
                        inst.loop_mode = buzz_scene::LoopMode::SingleFrame;
                        inst.first_frame = 0;
                    }
                }
                rig.armature.set_rest_here();
                rig.rebind();
            }
        });
    }
    ids.len()
}

/// A PNG of the character, framed on its own bounds (not the stage).
fn preview(scene: &Scene, path: &Path) -> Result<(), String> {
    let mut union: Option<buzz_geom::Rect> = None;
    for layer in scene.layers().iter() {
        for o in layer.objects_at(0).iter() {
            let b = scene.resolved_bounds(o);
            union = Some(union.map_or(b, |u| u.union(b)));
        }
    }
    let b = union.ok_or("the document has no artwork to preview")?;
    let margin = b.width().max(b.height()) * 0.06;
    let region = b.inflate(margin, margin);
    let longest = region.width().max(region.height());
    let scale = 900.0 / longest;
    let settings = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: false,
        region: Some(region),
    };
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::default()).map_err(|e| format!("{e}"))?;
    let frame = exporter.render(scene, 0, &settings).map_err(|e| format!("{e}"))?;
    std::fs::write(path, frame.to_png().map_err(|e| format!("{e}"))?).map_err(|e| format!("{e}"))
}

/// The agent's own `ai_character_from_image`, waited for.
fn build_with_agent(image: &Path, name: &str, scratch_assets: &Path) -> Result<(App, u64), String> {
    let mut app = App::new(GpuPreference::default());
    app.live_use_assets_at(scratch_assets);
    must(
        // Bones here whatever `--rig` says: the arms are swung and baked on the
        // armature, and `--rig layers` converts afterwards.
        call(&mut app, "command", json!({"name": "ai_character_from_image", "args": {"path": image.to_string_lossy(), "character_name": name, "rig": "bones"}})),
        "ai_character_from_image",
    )?;
    let job = wait_job(&mut app, 1800)?;
    if job["status"] != json!("done") {
        return Err(format!("the character job did not finish: {job}"));
    }
    let element = job["result"]["armature_element"].as_u64().ok_or_else(|| format!("no armature in {}", job["result"]))?;
    Ok((app, element))
}

/// How far the worst elbow bends off its upper arm, if it is past what a
/// drawn T-pose or hanging arm ever does (60 degrees).
fn crossed_arm(bones: &[BoneInfo]) -> Option<f64> {
    bones
        .iter()
        .filter(|b| {
            let n = b.name.to_lowercase();
            (n.contains("elbow") || n.contains("forearm")) && b.parent.is_some_and(|p| is_upper_arm(&bones[p].name))
        })
        .map(|b| wrap_deg(b.angle).abs())
        .filter(|a| *a > 60.0)
        .reduce(f64::max)
}

/// DWPose's wrists, put back on the arm whose elbow they belong to: if a wrist
/// lies on the far side of the shoulder-elbow line's own direction, swap it (and
/// its hand keypoints) with the other wrist.
fn swap_crossed_wrists(person: &mut buzz_ai::pose::Person) -> bool {
    let k = &mut person.keypoints;
    // COCO-WholeBody: 5/6 shoulders, 7/8 elbows, 9/10 wrists, 91.. left hand, 112.. right hand.
    let along = |sh: (f32, f32, f32), el: (f32, f32, f32), wr: (f32, f32, f32)| {
        let (ax, ay) = (el.0 - sh.0, el.1 - sh.1);
        let (bx, by) = (wr.0 - el.0, wr.1 - el.1);
        ax * bx + ay * by
    };
    let straight = |k: &[(f32, f32, f32); 133], w9: usize, w10: usize| {
        along(k[5], k[7], k[w9]) > 0.0 && along(k[6], k[8], k[w10]) > 0.0
    };
    if straight(k, 9, 10) {
        return false;
    }
    if !straight(k, 10, 9) {
        return false;
    }
    k.swap(9, 10);
    for i in 0..21 {
        k.swap(91 + i, 112 + i);
    }
    true
}

/// The wizard's own stages (`character_from_image`'s body) with the skeleton
/// corrected between detection and masking, opened in a fresh `App`.
fn build_corrected(image: &Path, name: &str, scratch_assets: &Path) -> Result<(App, u64), String> {
    use buzz_ai::character::{
        assemble_character, complete_parts, part_masks, skeleton, trace_character_whole_with, TraceCharacterOptions,
    };
    let rgba = image::open(image).map_err(|e| format!("decode: {e}"))?.to_rgba8();
    let matte = buzz_ai::segment::matte(&rgba, None).map_err(|e| format!("matte: {e}"))?;
    let mut persons = buzz_ai::pose::detect(&rgba).map_err(|e| format!("pose: {e}"))?;
    if persons.is_empty() {
        return Err("no person detected".into());
    }
    if !swap_crossed_wrists(&mut persons[0]) {
        return Err("the wrists could not be put back on their own arms".into());
    }
    let skel = skeleton(&persons[0]);
    let parts = part_masks(&rgba, &matte, &skel).map_err(|e| format!("parts: {e}"))?;
    let completed = complete_parts(&rgba, &matte, &skel, &parts).map_err(|e| format!("completion: {e}"))?;
    // The wizard's own options: its stroke width, and its trace options with
    // the saved tuning (`--tuning` / `--bold` reach them through the env).
    let wizard = buzz_ui::AiCharacterState::default();
    let trace_options = TraceCharacterOptions { uniform_width: wizard.uniform_width, ..wizard.trace_options };
    let traced = trace_character_whole_with(&rgba, &completed, &skel, &matte, &trace_options)
        .map_err(|e| format!("trace: {e}"))?;
    // The saved joint limits (`rig_limits.json`), as every rig builder uses.
    let limits = buzz_app::rig_limits::load_or_default();
    let assembled = assemble_character(&skel, &traced, name, Some((rgba.width() as f64, rgba.height() as f64)), Some(&limits))
        .map_err(|e| format!("assembly: {e}"))?;

    let file = scratch_assets.join(format!("{}.corrected.buzz", name.replace(' ', "_")));
    buzz_doc::format::save(&assembled.scene, &file).map_err(|e| format!("save corrected rig: {e}"))?;
    let mut app = App::new(GpuPreference::default());
    app.live_use_assets_at(scratch_assets);
    must(call(&mut app, "command", json!({"name": "open", "path": file.to_string_lossy()})), "open")?;
    // Opening reads the file on a task of its own; `wait_for_jobs` runs the
    // finished tasks, so poll it until the document has arrived.
    let started = Instant::now();
    loop {
        let _ = call(&mut app, "wait_for_jobs", json!({"timeout_s": 1}));
        let objects = must(call(&mut app, "inspect", json!({"what": "objects"})), "inspect objects")?;
        if let Some(element) = objects["objects"]
            .as_array()
            .and_then(|o| o.iter().find(|o| o["kind"] == json!("armature")))
            .and_then(|o| o["id"].as_u64())
        {
            return Ok((app, element));
        }
        if started.elapsed() > Duration::from_secs(120) {
            return Err("the corrected rig did not open with an armature on stage".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn one_view(
    args: &Args,
    folder: &str,
    file: &str,
    name: &str,
    scratch: &Path,
) -> Result<Value, String> {
    let image = args.ep.join(folder).join(file);
    let rigs = args.out.join("rigs");
    std::fs::create_dir_all(&rigs).map_err(|e| e.to_string())?;

    // The pipeline saves a T-pose asset to Cast as it finishes; that goes to a
    // scratch library so the user's Cast only ever gets the finished rig.
    let scratch_assets = scratch.join(name.replace(' ', "_"));
    std::fs::create_dir_all(&scratch_assets).map_err(|e| e.to_string())?;

    let started = Instant::now();
    let (mut app, mut element) = build_with_agent(&image, name, &scratch_assets)?;
    let mut route = "ai_character_from_image";
    let mut bones = read_bones(&mut app, element)?;
    // **A crossed skeleton.** DWPose sometimes hands a back view's wrists to the
    // wrong arm, which pulls each forearm across the body and cuts the parts
    // wrongly. The agent command takes the detection as it comes, so a rig whose
    // forearms bend sharply off their upper arms is rebuilt through the same
    // public stages with the wrists put back on their own side.
    if let Some(bend) = crossed_arm(&bones) {
        println!("[{name}] an elbow bends {bend:.0} degrees off its upper arm: the detection crossed the arms; rebuilding with corrected wrists");
        let (fixed, fixed_element) = build_corrected(&image, name, &scratch_assets)?;
        app = fixed;
        element = fixed_element;
        bones = read_bones(&mut app, element)?;
        route = "stages with crossed wrists swapped back";
        if let Some(bend) = crossed_arm(&bones) {
            return Err(format!("the corrected skeleton still has an elbow bent {bend:.0} degrees"));
        }
    }
    let pipeline_secs = started.elapsed().as_secs_f64();

    let bone_names: Vec<String> = bones.iter().map(|b| b.name.clone()).collect();
    println!("[{name}] rig element {element} ({route}), bones: {bone_names:?}");
    let before: Vec<f64> = (0..bones.len()).map(|i| world_angle(&bones, i)).collect();

    let moves = relax_arms(&mut app, element, args.hang)?;
    println!("[{name}] arms: {}", json!(moves));

    // The character's own mouth chart, if it has one: the drawn mouths become the
    // frames of the rig's mouth symbol (the ten visemes first, so lip sync keeps
    // working). Before the save below, so the file and the Cast asset carry them.
    // Addressed by the rig's element, not by the character's name: the name finds
    // the library's `<name>_mouth`, which is not the symbol this rig's mouth
    // instance uses (the document holds a second copy, `<name>_mouth 1`).
    let mut mouth_chart = Value::Null;
    if let Some((_, chart_folder, chart_file)) = MOUTH_CHARTS.iter().find(|(n, _, _)| n == &name) {
        let chart = args.ep.join(chart_folder).join(chart_file);
        let done = must(
            call(
                &mut app,
                "command",
                json!({"name": "ai_mouth_chart", "args": {"element": element, "path": chart.to_string_lossy()}}),
            ),
            "ai_mouth_chart",
        )?;
        println!("[{name}] mouth chart: {done}");
        mouth_chart = done;
    }

    // The posed document, then the baked one.
    let posed_path = rigs.join(format!("{name}.posed.buzz"));
    must(
        call(&mut app, "command", json!({"name": "save_as", "path": posed_path.to_string_lossy()})),
        "save_as",
    )?;
    let mut scene = buzz_doc::format::load(&posed_path).map_err(|e| format!("reload: {e}"))?;
    let baked = bake_pose(&mut scene);
    if baked == 0 {
        return Err("the saved document holds no armature".into());
    }
    // `--rig layers`: the baked rig taken apart into followed layers.
    if args.rig == buzz_rig::RigKind::Layers {
        let (converted, refused) = buzz_app::rigging::convert_all_to_layers(&mut scene, 0);
        if refused > 0 || converted == 0 {
            return Err(format!(
                "--rig layers: {refused} rig(s) have skinned artwork and cannot be one layer per bone"
            ));
        }
        println!("[{name}] converted to a layer rig");
    }
    let buzz_path = rigs.join(format!("{name}.buzz"));
    buzz_doc::format::save(&scene, &buzz_path).map_err(|e| format!("save rig: {e}"))?;
    let _ = std::fs::remove_file(&posed_path);
    preview(&scene, &rigs.join(format!("{name}.png")))?;

    // The user's Cast.
    let mut cast_state = "not written (--no-cast)".to_string();
    if args.cast {
        let mut library = buzz_doc::AssetLibrary::user();
        let exists = library.assets().iter().any(|a| a.folder == "Cast" && a.name == name);
        if exists && !args.replace {
            cast_state = "skipped: a Cast asset of that name exists (use --replace)".into();
        } else {
            let saved = library.save(name, "Cast", &scene).map_err(|e| format!("Cast save: {e}"))?;
            cast_state = format!("{} {}", if exists { "replaced" } else { "saved" }, saved.path.display());
        }
    }
    println!("[{name}] Cast: {cast_state}");

    Ok(json!({
        "name": name, "image": image.to_string_lossy(), "element": element,
        "bones": bone_names, "world_angles_before": before, "arms": moves,
        "mouth_chart": mouth_chart, "cast": cast_state, "rig": args.rig.label(), "file": buzz_path.to_string_lossy(), "pipeline_s": pipeline_secs,
    }))
}

// ------------------------------------------------------------------- R4: the scene

/// Scene 5 ("Raju arrives"): the five front rigs, left to right, with where each
/// stands as a fraction of the stage width. Raju's is where he ends up.
const SCENE5_CAST: &[(&str, f64)] =
    &[("Lalten", 0.12), ("Guddu", 0.28), ("Woman1", 0.44), ("Munna", 0.60), ("Raju", 0.82)];
const SCENE5_FRAMES: u32 = 144;
/// Feet on one ground line, as a fraction of the stage height.
const GROUND: f64 = 0.9;
/// Every figure's drawn height, as a fraction of the stage height.
const FIGURE_HEIGHT: f64 = 0.45;

/// Finds an element on frame 0 by the id the executor reports.
const FIND_ELEMENT: &str = "function findEl(id){var tl=fl.getDocumentDOM().getTimeline();\
for(var li=0;li<tl.layers.length;li++){var fr=tl.layers[li].frames[0];if(!fr)continue;\
var els=fr.elements;for(var k=0;k<els.length;k++){if(els[k].__objectId===id)return els[k];}}return null;}";

/// Run a JSFL script; its last `fl.trace` line, parsed as JSON.
fn js(app: &mut App, code: &str) -> Result<Value, String> {
    let r = call(app, "script", json!({"code": code}));
    if !r.ok {
        return Err(format!(
            "script failed: {} (in: {})",
            r.error.unwrap_or_default().lines().next().unwrap_or(""),
            code.chars().take(160).collect::<String>()
        ));
    }
    let trace = match &r.value["trace"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    let last = trace.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    Ok(serde_json::from_str(last).unwrap_or_else(|_| json!({"trace": trace})))
}

fn stage_objects(app: &mut App) -> Result<Vec<Value>, String> {
    let r = must(call(app, "inspect", json!({"what": "objects"})), "inspect objects")?;
    Ok(r["objects"].as_array().cloned().unwrap_or_default())
}

fn bounds_of(o: &Value) -> Result<[f64; 4], String> {
    let b = o["bounds"].as_array().ok_or("an object without bounds")?;
    let n = |i: usize| b.get(i).and_then(Value::as_f64).ok_or("bad bounds");
    Ok([n(0)?, n(1)?, n(2)?, n(3)?])
}

/// **What a Cast rig really covers**, in the document coordinates it is placed
/// at: the asset rendered on transparent and its opaque pixels measured. The
/// executor's `bounds` for a rig take a symbol instance's placeholder box as
/// its extent, which stretches the width well past the drawing.
fn measure_asset(name: &str) -> Result<[f64; 4], String> {
    let library = buzz_doc::AssetLibrary::user();
    let asset = library
        .assets()
        .iter()
        .find(|a| a.folder == "Cast" && a.name == name)
        .ok_or_else(|| format!("no Cast asset called {name}"))?;
    let scene = library.load(asset).map_err(|e| format!("load {name}: {e}"))?;
    let mut union: Option<buzz_geom::Rect> = None;
    for layer in scene.layers().iter() {
        for o in layer.objects_at(0).iter() {
            let b = scene.resolved_bounds(o);
            union = Some(union.map_or(b, |u| u.union(b)));
        }
    }
    let b = union.ok_or("the asset has no artwork")?;
    let region = b.inflate(b.width() * 0.1, b.height() * 0.1);
    let scale = 1400.0 / region.width().max(region.height());
    let settings = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: true,
        region: Some(region),
    };
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::default()).map_err(|e| format!("{e}"))?;
    let frame = exporter.render(&scene, 0, &settings).map_err(|e| format!("{e}"))?;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..frame.height {
        for x in 0..frame.width {
            if frame.pixel(x, y)[3] > 24 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 == 0 {
        return Err(format!("{name} rendered nothing"));
    }
    let to_doc = |px: u32, py: u32| (region.x0 + px as f64 / scale, region.y0 + py as f64 / scale);
    let (ax, ay) = to_doc(x0, y0);
    let (bx, by) = to_doc(x1, y1);
    Ok([ax, ay, bx, by])
}

/// Set an element's matrix on frame 0.
fn set_matrix(app: &mut App, id: u64, m: [f64; 6]) -> Result<(), String> {
    js(
        app,
        &format!(
            "{FIND_ELEMENT} var e=findEl({id}); e.matrix={{a:{},b:{},c:{},d:{},tx:{},ty:{}}}; fl.trace('{{}}');",
            m[0], m[1], m[2], m[3], m[4], m[5]
        ),
    )
    .map(|_| ())
}

/// Save the document as `<out>/ep1_scene5_setup.buzz`, and write the previews.
fn scene5_previews(app: &mut App, out: &Path, frames: &[u32]) -> Result<Vec<String>, String> {
    let file = out.join("ep1_scene5_setup.buzz");
    must(call(app, "command", json!({"name": "save_as", "path": file.to_string_lossy()})), "save_as")?;
    let mut written = Vec::new();
    // The executor's own snapshot (longest side 768), as the plan asks.
    for &f in frames {
        let r = must(call(app, "snapshot", json!({"frame": f})), "snapshot")?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(r["png_base64"].as_str().ok_or("no png")?)
            .map_err(|e| e.to_string())?;
        let path = out.join(format!("ep1_scene5_f{:03}.png", f + 1));
        std::fs::write(&path, png).map_err(|e| e.to_string())?;
        written.push(path.display().to_string());
    }
    // And the saved file itself, loaded back and rendered larger, so what was
    // written to disk is what is looked at.
    let scene = buzz_doc::format::load(&file).map_err(|e| format!("reload: {e}"))?;
    let settings = buzz_export::ExportSettings::scaled(&scene, 1280.0 / scene.stage().size.width.max(1.0));
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::default()).map_err(|e| format!("{e}"))?;
    for &f in frames {
        let frame = exporter.render(&scene, f, &settings).map_err(|e| format!("{e}"))?;
        let path = out.join(format!("ep1_scene5_f{:03}_hd.png", f + 1));
        std::fs::write(&path, frame.to_png().map_err(|e| format!("{e}"))?).map_err(|e| format!("{e}"))?;
        written.push(path.display().to_string());
    }
    Ok(written)
}

/// The document as it stands in `app`, read back through a scratch save (the
/// agent tools have no other way to hand over the scene).
fn current_scene(app: &mut App) -> Result<Scene, String> {
    let file = std::env::temp_dir().join(format!("series_setup_probe_{}.buzz", std::process::id()));
    must(call(app, "command", json!({"name": "save_as", "path": file.to_string_lossy()})), "save_as")?;
    let scene = buzz_doc::format::load(&file).map_err(|e| format!("reload: {e}"));
    let _ = std::fs::remove_file(&file);
    scene
}

/// What placing a Cast asset brought on stage.
struct CastPlacement {
    /// The element standing for the character: an armature, or the root part
    /// of a layer rig (`CharacterRig::body`).
    id: u64,
    /// That element's `inspect objects` entry.
    rig: Value,
    /// Every object that arrived, as `kind:id`.
    kinds: Vec<String>,
}

/// Place the Cast asset `name` and find the character it brought — a bone rig
/// or a layer rig (LR2), asked the one way (`buzz_act::layer_rig::rig_of`).
fn place_cast(app: &mut App, name: &str) -> Result<CastPlacement, String> {
    let before: Vec<u64> = stage_objects(app)?.iter().filter_map(|o| o["id"].as_u64()).collect();
    js(app, &format!("fl.getDocumentDOM().assets.place({name:?},'Cast'); fl.trace('{{}}');"))?;
    let now = stage_objects(app)?;
    let arrived: Vec<&Value> =
        now.iter().filter(|o| o["id"].as_u64().is_some_and(|id| !before.contains(&id))).collect();
    let kinds: Vec<String> =
        arrived.iter().map(|o| format!("{}:{}", o["kind"].as_str().unwrap_or("?"), o["id"])).collect();
    let scene = current_scene(app)?;
    let id = arrived
        .iter()
        .filter_map(|o| o["id"].as_u64())
        .find_map(|id| buzz_act::layer_rig::rig_of(&scene, ObjectId(id)).map(|rig| rig.body().0))
        .ok_or_else(|| format!("{name}: placing the asset brought no rig ({kinds:?})"))?;
    let rig = arrived
        .iter()
        .find(|o| o["id"].as_u64() == Some(id))
        .map(|o| (*o).clone())
        .ok_or_else(|| format!("{name}: the rig's element {id} is not on frame 0"))?;
    Ok(CastPlacement { id, rig, kinds })
}

fn build_scene5(args: &Args) -> Result<Value, String> {
    let mut app = App::new(GpuPreference::default());
    let doc = must(call(&mut app, "inspect", json!({"what": "document"})), "inspect document")?;
    let (w, h) = (doc["width"].as_f64().unwrap_or(1920.0), doc["height"].as_f64().unwrap_or(1080.0));
    let ground = h * GROUND;
    let figure_h = h * FIGURE_HEIGHT;

    // The set: daylight and a village on the horizon.
    let set = js(
        &mut app,
        &format!(
            "var d=fl.getDocumentDOM(); var set=d.setTheScene({{setting:'daylight',cast:0,frames:{SCENE5_FRAMES},horizon:0.6}}); \
             var sc=d.layScenery('village', set.horizonY, set.backdrop); \
             fl.trace(JSON.stringify({{horizonY:set.horizonY,backdrop:set.backdrop,pieces:sc.pieces,message:set.message}}));"
        ),
    )?;
    println!("[scene 5] set: {set}");

    let mut placed: Vec<Value> = Vec::new();
    let warnings: Vec<String> = Vec::new();
    for (name, x_frac) in SCENE5_CAST {
        let CastPlacement { id, rig, kinds } = place_cast(&mut app, name)?;
        let reported = bounds_of(&rig)?;
        let b = measure_asset(name)?;

        // Scale about the soles to the common height, then stand on the ground line.
        let s = figure_h / (b[3] - b[1]).max(1.0);
        let (ax, ay) = ((b[0] + b[2]) / 2.0, b[3]);
        let (px, py) = (w * x_frac, ground);
        let m = js(&mut app, &format!("{FIND_ELEMENT} var e=findEl({id}); fl.trace(JSON.stringify(e.matrix));"))?;
        let g = |k: &str| m[k].as_f64().ok_or_else(|| format!("{name}: matrix lacks {k}"));
        let (a, bb, c, d, tx, ty) = (g("a")?, g("b")?, g("c")?, g("d")?, g("tx")?, g("ty")?);
        let new = [s * a, s * bb, s * c, s * d, s * (tx - ax) + px, s * (ty - ay) + py];
        set_matrix(&mut app, id, new)?;
        // Where the measured drawing now sits: the placement is a scale about
        // the soles and a move, so the box follows the same two steps.
        let nb = [px + s * (b[0] - ax), py + s * (b[1] - ay), px + s * (b[2] - ax), py];
        println!(
            "[scene 5] {name}: rig {id}, arrived {kinds:?}; drawn {:.0}x{:.0} (executor said {:.0}x{:.0}) -> {:.0}x{:.0}, soles y {:.0} (ground {ground:.0}), centre x {:.0}",
            b[2] - b[0],
            b[3] - b[1],
            reported[2] - reported[0],
            reported[3] - reported[1],
            nb[2] - nb[0],
            nb[3] - nb[1],
            nb[3],
            (nb[0] + nb[2]) / 2.0
        );
        placed.push(json!({"name": name, "element": id, "scale": s, "matrix": new, "bounds": nb, "arrived": kinds}));
    }

    // Performances. A walk's travel is written through the object's own matrix,
    // so a scaled figure travels `scale` times the distance asked: ask for
    // (wanted / scale). Raju's talk is written first, at his final mark, and then
    // the walk from off-stage: `perform` measures a walk from the first keyframe.
    let mut timings: Vec<Value> = Vec::new();
    let mut perform = |app: &mut App, who: &str, id: u64, action: &str, from: u32, to: u32, distance: f64| -> Result<(), String> {
        let code = if distance != 0.0 {
            format!("var n=fl.getDocumentDOM().performFully({id},{action:?},{from},{to},{{distance:{distance}}}); fl.trace(JSON.stringify({{keys:n}}));")
        } else {
            format!("var n=fl.getDocumentDOM().perform({id},{action:?},{from},{to}); fl.trace(JSON.stringify({{keys:n}}));")
        };
        let r = js(app, &code)?;
        timings.push(json!({"who": who, "action": action, "frames_1_based": [from + 1, to], "keyframes": r["keys"]}));
        Ok(())
    };
    let find = |name: &str| {
        placed.iter().find(|p| p["name"] == json!(name)).cloned().ok_or_else(|| format!("{name} was not placed"))
    };
    for who in ["Lalten", "Woman1", "Munna"] {
        let id = find(who)?["element"].as_u64().unwrap();
        perform(&mut app, who, id, "idle", 0, SCENE5_FRAMES, 0.0)?;
    }
    let guddu = find("Guddu")?["element"].as_u64().unwrap();
    perform(&mut app, "Guddu", guddu, "idle", 0, SCENE5_FRAMES, 0.0)?;
    perform(&mut app, "Guddu", guddu, "talk", 96, 130, 0.0)?;

    let raju = find("Raju")?;
    let raju_id = raju["element"].as_u64().unwrap();
    let raju_scale = raju["scale"].as_f64().unwrap();
    perform(&mut app, "Raju", raju_id, "talk", 60, 120, 0.0)?;
    // Back to the wings on frame 1: off-stage right, clear of the frame by most of a figure.
    let m: Vec<f64> = raju["matrix"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
    let rb: Vec<f64> = raju["bounds"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
    let figure_w = rb[2] - rb[0];
    let final_x = w * 0.82;
    let start_x = w + figure_w * 0.75;
    let travel = start_x - final_x;
    set_matrix(&mut app, raju_id, [m[0], m[1], m[2], m[3], m[4] + travel, m[5]])?;
    perform(&mut app, "Raju", raju_id, "walk", 0, 60, -travel / raju_scale)?;

    // Every layer to the length of the shot.
    js(&mut app, &format!("fl.getDocumentDOM().scenes.setLength({SCENE5_FRAMES}); fl.trace('{{}}');"))?;

    let files = scene5_previews(&mut app, &args.out, &[0u32, 29, 59, 119])?;
    let layers = must(call(&mut app, "inspect", json!({"what": "timeline"})), "inspect timeline")?;
    Ok(json!({
        "scene_file": args.out.join("ep1_scene5_setup.buzz").display().to_string(),
        "previews": files,
        "stage": [w, h], "ground_y": ground, "figure_height": figure_h,
        "placed": placed, "timings": timings, "warnings": warnings,
        "raju": {"final_x": final_x, "start_x": start_x, "walk_distance_asked": -travel / raju_scale},
        "layers": layers["layers"].as_array().map(|l| l.iter().map(|x| json!({"name": x["name"], "id": x["id"]})).collect::<Vec<_>>()),
    }))
}

/// `--skeleton <image>`: print the joints DWPose finds, to see why a rig went wrong.
fn dump_skeleton(path: &str) {
    let img = image::open(path).expect("image").to_rgba8();
    match buzz_ai::pose::detect(&img) {
        Ok(persons) if !persons.is_empty() => {
            let skel = buzz_ai::character::skeleton(&persons[0]);
            println!("image {}x{}, {} person(s)", img.width(), img.height(), persons.len());
            for j in &skel.joints {
                println!("{:<10} x {:7.1} y {:7.1} conf {:.2}", j.name, j.x, j.y, j.confidence);
            }
        }
        other => println!("no person detected: {:?}", other.map(|p| p.len())),
    }
}

/// `--parts <Cast name>`: what the rig is made of, for diagnosing a rig.
fn dump_parts(name: &str) {
    let library = buzz_doc::AssetLibrary::user();
    let asset = library.assets().iter().find(|a| a.folder == "Cast" && a.name == name).expect("a Cast asset").clone();
    let scene = library.load(&asset).expect("load");
    for layer in scene.layers().iter() {
        for o in layer.objects_at(0).iter() {
            if let ObjectKind::Armature(rig) = &o.kind {
                println!("armature {} on layer {}: {} bones, {} parts, {} modifiers", o.id.0, layer.name, rig.armature.bones.len(), rig.parts.len(), o.modifiers.len());
                for (i, part) in rig.parts.iter().enumerate() {
                    match &part.artwork.kind {
                        ObjectKind::Instance(inst) => {
                            let sym = scene.library().get(inst.symbol);
                            println!("  part {i}: instance of {:?} kind {:?} length {:?} loop {:?} first {} bound {:?}",
                                sym.map(|s| s.name.clone()), sym.map(|s| s.kind), sym.map(|s| s.length()), inst.loop_mode, inst.first_frame, part.binding);
                        }
                        other => println!("  part {i}: {} bound {:?}", match other { ObjectKind::Shape(_) => "shape", ObjectKind::Group(_) => "group", _ => "other" }, part.binding),
                    }
                }
            } else if let Some(buzz_act::layer_rig::CharacterRig::Layers(view)) =
                buzz_act::layer_rig::rig_of(&scene, o.id)
            {
                let slot = view.parts.iter().find(|(_, _, id)| *id == o.id).map(|(s, _, _)| *s);
                println!("layer {}: object {} (layer-rig part, slot {slot:?}, root {})", layer.name, o.id.0, view.body.0);
            } else {
                println!("layer {}: object {} (not an armature)", layer.name, o.id.0);
            }
        }
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--parts") {
        dump_parts(&std::env::args().nth(2).expect("a Cast name"));
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--skeleton") {
        dump_skeleton(&std::env::args().nth(2).expect("an image path"));
        return;
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    // `--tuning` / `--bold` reach every trace — the Assistant's command and
    // `build_corrected` — through `TraceOptions::with_saved_tuning`.
    if let Some(file) = &args.tuning {
        if let Err(e) = buzz_scene::TraceTuning::load(file) {
            eprintln!("--tuning: {e}");
            std::process::exit(2);
        }
        // SAFETY: set before any App, model or worker thread exists.
        unsafe { std::env::set_var("BUZZ_TRACE_TUNING", file) };
    }
    if let Some(bold) = args.bold {
        // SAFETY: as above.
        unsafe { std::env::set_var("BUZZ_TRACE_BOLD", bold.to_string()) };
    }
    std::fs::create_dir_all(&args.out).expect("output folder");
    let scratch = std::env::temp_dir().join(format!("series_setup_{}", std::process::id()));
    // The report keeps what earlier runs found; a run replaces only its own views.
    let report_path = args.out.join("series_setup_report.json");
    let mut report: Vec<Value> = std::fs::read_to_string(&report_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let upsert = |report: &mut Vec<Value>, entry: Value| match report.iter_mut().find(|e| e["name"] == entry["name"]) {
        Some(slot) => *slot = entry,
        None => report.push(entry),
    };
    let mut failures = 0;
    for (folder, file, name) in VIEWS {
        if !args.rigs {
            break;
        }
        if args.only.as_ref().is_some_and(|only| !only.iter().any(|n| n == name)) {
            continue;
        }
        if args.cast && !args.replace {
            let library = buzz_doc::AssetLibrary::user();
            if library.assets().iter().any(|a| a.folder == "Cast" && a.name == *name) {
                println!("[{name}] skipped: a Cast asset called '{name}' already exists (use --replace)");
                continue;
            }
        }
        match one_view(&args, folder, file, name, &scratch) {
            Ok(entry) => upsert(&mut report, entry),
            Err(e) => {
                failures += 1;
                eprintln!("[{name}] FAILED: {e}");
                upsert(&mut report, json!({"name": name, "error": e}));
            }
        }
        let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap_or_default());
    }
    let _ = std::fs::remove_dir_all(&scratch);
    println!("done: {} views, {failures} failed", report.len());
    if args.scene {
        match build_scene5(&args) {
            Ok(entry) => {
                println!("scene 5: {}", entry);
                let _ = std::fs::write(args.out.join("ep1_scene5_report.json"), serde_json::to_string_pretty(&entry).unwrap_or_default());
            }
            Err(e) => {
                failures += 1;
                eprintln!("scene 5 FAILED: {e}");
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Args, String> {
        parse_args_from(list.iter().map(|s| s.to_string()))
    }

    /// LR2: layer rigs unless `--rig bones` is given.
    #[test]
    fn the_rig_kind_defaults_to_layers() {
        let args = parse(&["--ep", "e", "--out", "o"]).expect("parsed");
        assert_eq!(args.rig, buzz_rig::RigKind::Layers);
        let args = parse(&["--ep", "e", "--out", "o", "--rig", "bones"]).expect("parsed");
        assert_eq!(args.rig, buzz_rig::RigKind::Bones);
        assert!(parse(&["--ep", "e", "--out", "o", "--rig", "sticks"]).is_err());
    }
}
