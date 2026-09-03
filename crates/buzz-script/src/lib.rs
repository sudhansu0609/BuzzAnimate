//! Scripting: BuzzAnimate's equivalent of Animate's JSFL.
//!
//! A script is JavaScript that drives the document the way a person would —
//! `fl.getDocumentDOM()`, `document.addNewRectangle(...)`, `fl.trace(...)`.
//! Animate's own vocabulary is used throughout, so somebody with a shelf of
//! JSFL commands recognises what they are reading.
//!
//! # Two properties that are not optional
//!
//! **A script cannot hang the application.** QuickJS is given an interrupt
//! handler and a deadline, so `while (true) {}` stops with an error instead of
//! wedging the window. It also gets a memory ceiling and a stack limit, so a
//! runaway allocation or an infinite recursion fails the script rather than
//! the process. Scripting is the one feature where the user runs arbitrary
//! code inside the editor; it has to be the feature least able to break it.
//!
//! **A script is one undo step.** The whole run happens against a single
//! [`Scene`], and the caller commits that scene in one edit — so a script that
//! draws four hundred rectangles is one Ctrl+Z, not four hundred.
//!
//! # How the API is built
//!
//! Rust exposes a flat set of primitives on a hidden `__host` object, and a
//! JavaScript prelude shapes those into the `fl` and `document` objects the
//! user actually calls. That split is deliberate: the Rust side stays plain
//! functions with no lifetime gymnastics, and the *shape* of the API — the
//! part that has to match Animate — is written in readable JavaScript that can
//! be diffed against Animate's documentation.
//!
//! # Deviation from the plan
//!
//! PROGRESS.md's CP-8.1 said scripts would "submit through the same command
//! queue". They do not: they mutate a working copy of the scene directly. A
//! queue cannot answer a read *after* a write — `doc.addNewRectangle(); doc.
//! selection.length` would be wrong — and read-after-write is most of what a
//! script does. One undo step, which is what the queue was for, is achieved by
//! committing the working copy in a single edit.

mod host;
mod samples;

pub use samples::{Sample, samples};

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use buzz_scene::{LayerId, ObjectId, Scene};

/// The JavaScript the prelude runs to build `fl` and `document`.
const PRELUDE: &str = include_str!("prelude.js");

/// What the script is allowed to consume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Wall-clock budget. A script that exceeds it is interrupted.
    pub time: Duration,
    /// Bytes the engine may allocate.
    pub memory: usize,
    /// Stack bytes, which is what bounds runaway recursion.
    pub stack: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Long enough for a script that builds a few thousand shapes,
            // short enough that a mistake is an annoyance rather than a
            // reason to kill the application.
            time: Duration::from_secs(5),
            memory: 64 * 1024 * 1024,
            stack: 1024 * 1024,
        }
    }
}

/// Editor state a script can read and change, beyond the document itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptContext {
    pub current_frame: u32,
    pub selection: Vec<ObjectId>,
    pub active_layer: Option<LayerId>,
    /// The Configuration folder `fl.configURI` points at, and the only place
    /// `fl.runScript` will read from.
    ///
    /// A shelf of JSFL commands is not 71 separate programs: they open by
    /// pulling in a shared file of settings —
    /// `fl.runScript(fl.configURI + "Commands/commonVariables.jsfl")` — and
    /// everything after that line depends on the variables it defines. Without
    /// somewhere to read that from, a script either fails on its first line or,
    /// worse, swallows the error and runs on with every setting undefined.
    ///
    /// **This is also the sandbox boundary.** Reading files is a capability a
    /// script otherwise does not have, so it is confined to this one directory:
    /// a script may read the scripts that live beside it and nothing else.
    /// `None` disables reading altogether.
    pub config_dir: Option<std::path::PathBuf>,
}

/// Where a shelf of JSFL commands lives on this machine.
///
/// BuzzAnimate's own folder first, then Animate's — because somebody with a
/// shelf of commands built up over years has it in Animate's Configuration
/// directory, and asking them to move it to use it here would be the whole
/// cost of switching for no benefit. Newest Animate first, since a script
/// shelf follows the version it was written against.
///
/// `None` when neither exists, which turns `fl.runScript` off rather than
/// pointing it somewhere arbitrary.
pub fn default_config_dir() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    // Ours: `%APPDATA%/BuzzAnimate/Configuration`, beside the assets library.
    if let Some(dir) = dirs_next_config() {
        candidates.push(dir);
    }

    // Animate's, newest first.
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let adobe = std::path::Path::new(&local).join("Adobe");
        let mut versions: Vec<std::path::PathBuf> = std::fs::read_dir(&adobe)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("Animate"))
            })
            .collect();
        versions.sort();
        for version in versions.into_iter().rev() {
            candidates.push(version.join("en_US").join("Configuration"));
        }
    }

    candidates.into_iter().find(|path| path.is_dir())
}

/// `%APPDATA%/BuzzAnimate/Configuration`, if the platform has such a place.
fn dirs_next_config() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(
        std::path::PathBuf::from(base)
            .join("BuzzAnimate")
            .join("Configuration"),
    )
}

/// A way for the host to stop a script that is already running.
///
/// Shared and `'static` because it ends up inside QuickJS's interrupt handler,
/// which the engine owns; `Send + Sync` because the script may be running on a
/// thread while the Stop button is pressed on another.
pub type StopSignal = Arc<dyn Fn() -> bool + Send + Sync>;

/// What a run produced.
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    /// Everything the script passed to `fl.trace`, in order.
    pub trace: Vec<String>,
    /// Everything the script raised through `alert`, `prompt` or `confirm`.
    ///
    /// A script runs without a person watching it, so a modal question has
    /// nobody to answer it. Rather than fail — which is what an undefined
    /// `alert` did, and it stopped twenty of the commands on a real shelf at
    /// their first line of error handling — the question is recorded and
    /// answered with its own default, and the host shows the list afterwards.
    pub alerts: Vec<String>,
    /// The failure, already formatted for a person, or `None` on success.
    pub error: Option<String>,
    /// Editor state as the script left it.
    pub context: ScriptContext,
    /// Did the document actually change?
    pub changed: bool,
    /// True when the host asked it to stop, rather than it finishing or
    /// failing on its own.
    pub stopped: bool,
    pub elapsed: Duration,
}

impl ScriptOutcome {
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }

    /// A line for the status bar.
    pub fn summary(&self) -> String {
        if self.stopped {
            return "Script stopped".to_string();
        }
        match &self.error {
            Some(_) => "Script failed".to_string(),
            None if self.changed => format!(
                "Script finished in {:.0} ms",
                self.elapsed.as_secs_f64() * 1000.0
            ),
            None => "Script finished; the document is unchanged".to_string(),
        }
    }
}

/// Everything the host functions read and write during a run.
pub(crate) struct State {
    pub scene: Scene,
    pub context: ScriptContext,
    pub trace: Vec<String>,
    pub alerts: Vec<String>,
}

/// Run `source` against `scene`.
///
/// The scene is modified in place, so the caller can commit it in one edit.
/// A failing script leaves whatever it managed to do before the error —
/// exactly as Animate does, and far more useful than silently discarding an
/// hour of generated artwork because the last line had a typo.
pub fn run(
    scene: &mut Scene,
    context: ScriptContext,
    source: &str,
    limits: &Limits,
) -> ScriptOutcome {
    run_until(scene, context, source, limits, None)
}

/// Run `source`, with a way to stop it.
///
/// `stop` is consulted from inside QuickJS's interrupt handler, so it is asked
/// the same way the time limit is — often, and between bytecodes rather than
/// between statements. That is what makes Stop take effect on the spot instead
/// of at the end of whatever loop the script is in.
pub fn run_until(
    scene: &mut Scene,
    context: ScriptContext,
    source: &str,
    limits: &Limits,
    stop: Option<StopSignal>,
) -> ScriptOutcome {
    let started = Instant::now();
    let revision_before = scene.revision();

    let state = Rc::new(RefCell::new(State {
        // Cheap: the scene is copy-on-write, so this shares its artwork until
        // the script actually changes something.
        scene: scene.clone(),
        context,
        trace: Vec::new(),
        alerts: Vec::new(),
    }));

    let (error, stopped) = evaluate(&state, source, limits, stop);

    // Take the work back out, whether or not the script finished. A partial
    // result is still the user's work.
    let finished = Rc::try_unwrap(state)
        .unwrap_or_else(|rc| RefCell::new(rc.borrow().clone_state()))
        .into_inner();

    let changed = finished.scene.revision() != revision_before;
    *scene = finished.scene;

    ScriptOutcome {
        trace: finished.trace,
        alerts: finished.alerts,
        error,
        context: finished.context,
        changed,
        stopped,
        elapsed: started.elapsed(),
    }
}

impl State {
    /// Used only if a reference escaped into a closure the engine still holds.
    fn clone_state(&self) -> Self {
        Self {
            scene: self.scene.clone(),
            context: self.context.clone(),
            trace: self.trace.clone(),
            alerts: self.alerts.clone(),
        }
    }
}

/// Build the engine, install the API, and evaluate.
///
/// Returns the failure, if any, and whether the host is what stopped it.
fn evaluate(
    state: &Rc<RefCell<State>>,
    source: &str,
    limits: &Limits,
    stop: Option<StopSignal>,
) -> (Option<String>, bool) {
    let runtime = match rquickjs::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            return (
                Some(format!("could not start the script engine: {e}")),
                false,
            );
        }
    };

    runtime.set_memory_limit(limits.memory);
    runtime.set_max_stack_size(limits.stack);

    // The deadline is what makes `while (true) {}` survivable. QuickJS calls
    // this periodically while executing; returning true aborts.
    //
    // The flag records that the abort *was* ours. QuickJS reports an interrupt
    // as an exception reading only "interrupted", which is indistinguishable
    // from a script that threw that word itself — so the cause is recorded at
    // the point it is known rather than guessed at from the message later.
    let timed_out = Arc::new(AtomicBool::new(false));
    let asked_to_stop = Arc::new(AtomicBool::new(false));
    let fired = Arc::clone(&timed_out);
    let asked = Arc::clone(&asked_to_stop);
    let deadline = Instant::now() + limits.time;
    runtime.set_interrupt_handler(Some(Box::new(move || {
        // The host first: a person who pressed Stop is owed an answer sooner
        // than a clock is.
        if let Some(stop) = &stop
            && stop()
        {
            asked.store(true, Ordering::Relaxed);
            return true;
        }
        if Instant::now() >= deadline {
            fired.store(true, Ordering::Relaxed);
            return true;
        }
        false
    })));

    let context = match rquickjs::Context::full(&runtime) {
        Ok(c) => c,
        Err(e) => {
            return (
                Some(format!("could not start the script context: {e}")),
                false,
            );
        }
    };

    let error = context.with(|ctx| {
        if let Err(e) = host::install(&ctx, state) {
            return Some(format!("could not install the script API: {e}"));
        }

        // The prelude shapes the host primitives into Animate's API. A failure
        // here is our bug, not the user's, so it says so.
        if let Err(e) = ctx.eval::<(), _>(PRELUDE) {
            // Stop can land while the prelude is still running — the user
            // pressed it the moment they pressed Run. Blaming our own prelude
            // for that would be both wrong and alarming.
            if asked_to_stop.load(Ordering::Relaxed) {
                return None;
            }
            return Some(format!(
                "the built-in script prelude failed, which is a defect in \
                 BuzzAnimate rather than in your script: {}",
                describe(&ctx, e)
            ));
        }

        match ctx.eval::<rquickjs::Value, _>(source) {
            Ok(_) => None,
            Err(e) => {
                // `describe` also clears the pending exception, so it runs
                // either way and its answer is only preferred when the engine
                // stopped for a reason other than our deadline.
                let described = describe(&ctx, e);
                if asked_to_stop.load(Ordering::Relaxed) {
                    // Not a failure. The user asked, and what the script
                    // managed before then is kept, as it is for a timeout.
                    return None;
                }
                Some(if timed_out.load(Ordering::Relaxed) {
                    TIMED_OUT.to_string()
                } else {
                    described
                })
            }
        }
    });

    (error, asked_to_stop.load(Ordering::Relaxed))
}

/// Said whenever the deadline is what stopped the script. QuickJS's own word
/// for it — "interrupted" — does not tell the user what to change.
const TIMED_OUT: &str = "the script was stopped: it ran longer than the time \
                         limit allows, which usually means a loop that never ends";

/// Turn an engine error into something a person can act on.
fn describe(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> String {
    if error.is_exception() {
        let exception = ctx.catch();
        if let Some(exception) = exception.as_exception() {
            let message = exception.message().unwrap_or_default();
            let stack = exception.stack().unwrap_or_default();

            if message.is_empty() && stack.is_empty() {
                return TIMED_OUT.to_string();
            }

            let mut text = if message.is_empty() {
                "script error".to_string()
            } else {
                message
            };
            // The first stack line carries the line number, which is the only
            // part of a QuickJS stack a user cares about.
            if let Some(first) = stack.lines().find(|l| !l.trim().is_empty()) {
                text.push_str(&format!("\n  {}", first.trim()));
            }
            return text;
        }
        // An exception that is not an Error object: a bare `throw "text"`.
        if let Some(text) = exception.as_string().and_then(|s| s.to_string().ok()) {
            return text;
        }
        return "the script threw a value that is not an error".to_string();
    }

    match error {
        rquickjs::Error::Allocation => {
            "the script ran out of memory, which usually means it built \
             something far larger than it meant to"
                .to_string()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LayerKind;

    fn document() -> Scene {
        let mut scene = Scene::default();
        scene.add_layer("Second", LayerKind::Normal);
        scene
    }

    fn run_source(scene: &mut Scene, source: &str) -> ScriptOutcome {
        run(scene, ScriptContext::default(), source, &Limits::default())
    }

    // -- animation ----------------------------------------------------------
    //
    // The point of these: everything the program learned to do in the last
    // stretch of work was unreachable from a script, so a user could not
    // automate their own repetitive work with the very features built to save
    // them effort. Each of these proves one of them is reachable now, and — the
    // part that matters — that what it leaves behind is ordinary document data
    // the next line of the same script can read.

    #[test]
    fn a_script_can_tween_and_ease() {
        let mut scene = document();
        let outcome = run_source(
            &mut scene,
            r##"
            var doc = fl.getDocumentDOM();
            doc.setFillColor("#3366AA");
            doc.addNewRectangle({left: 0, top: 0, right: 50, bottom: 50});
            doc.setTween(0, 0, "motion");
            doc.setEase(0, 0, 60);
            "##,
        );
        assert!(outcome.error.is_none(), "{:?}", outcome.error);

        let layer = scene.layers().iter().next().expect("a layer").id;
        let tween = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .tween_at(0);
        assert!(tween.is_active(), "the script set a tween");
        assert_eq!(tween.easing, buzz_scene::Easing::Strength(60.0));
    }

    #[test]
    fn a_script_that_tweens_an_empty_frame_says_so() {
        let mut scene = document();
        let outcome = run_source(
            &mut scene,
            r#"fl.getDocumentDOM().setTween(0, 40, "motion");"#,
        );
        let error = outcome.error.expect("tweening frame 40 should fail");
        assert!(
            error.contains("keyframe"),
            "the message should name the reason, got {error}"
        );
    }

    #[test]
    fn a_script_can_shoot_the_scene() {
        let mut scene = document();
        let outcome = run_source(
            &mut scene,
            r#"
            var doc = fl.getDocumentDOM();
            doc.camera.setKey(0, {x: 100, y: 100, zoom: 1});
            doc.camera.setKey(24, {x: 400, y: 100, zoom: 2});
            doc.camera.setShutter(0.5, 12);
            doc.camera.setFocusKey(0, 600, 0.04);
            doc.camera.setFocusKey(24, 0, 0.04);
            "#,
        );
        assert!(outcome.error.is_none(), "{:?}", outcome.error);

        let camera = scene.camera();
        assert!(camera.enabled, "keying the camera switches it on");
        assert_eq!(camera.keys().len(), 2);
        assert!((camera.shutter - 0.5).abs() < 1e-9, "the shutter is open");
        assert_eq!(camera.blur_samples, 12);

        // And it is a real focus pull: the same depth is sharp at one end and
        // soft at the other.
        assert_eq!(camera.dof_blur_at(0u32, 600.0), None);
        assert!(camera.dof_blur_at(24u32, 600.0).is_some());
    }

    #[test]
    fn a_script_can_put_a_modifier_on_the_selection() {
        let mut scene = document();
        let outcome = run_source(
            &mut scene,
            r#"
            var doc = fl.getDocumentDOM();
            doc.addNewRectangle({left: 0, top: 0, right: 50, bottom: 50});
            doc.selectAll();
            doc.addWiggle(6, 3);
            "#,
        );
        assert!(outcome.error.is_none(), "{:?}", outcome.error);

        let modifiers: Vec<_> = scene
            .layers()
            .iter()
            .flat_map(|l| l.frames.resolved_at(0u32).iter().cloned().collect::<Vec<_>>())
            .flat_map(|o| o.modifiers.clone())
            .collect();
        assert_eq!(modifiers.len(), 1, "one wiggle landed");
        assert!(matches!(
            modifiers[0],
            buzz_scene::Modifier::Wiggle { .. }
        ));
    }

    #[test]
    fn a_modifier_with_nothing_selected_says_so() {
        let mut scene = document();
        let outcome = run_source(&mut scene, "fl.getDocumentDOM().addWiggle(4, 2);");
        let error = outcome.error.expect("it should refuse");
        assert!(error.contains("selected"), "got {error}");
    }

    #[test]
    fn a_script_can_set_type() {
        let mut scene = document();
        let outcome = run_source(
            &mut scene,
            r#"fl.getDocumentDOM().addText(20, 60, "Hello", {size: 36});"#,
        );
        // A machine with no font at all cannot draw text; that is a fact about
        // the machine, not a failure of the binding.
        if outcome.error.is_some() {
            eprintln!("skipping: no font on this machine");
            return;
        }
        let text = scene
            .layers()
            .iter()
            .flat_map(|l| l.frames.resolved_at(0u32).iter().cloned().collect::<Vec<_>>())
            .find_map(|o| o.text.clone())
            .expect("the text object records what it was typed as");
        assert_eq!(text.content, "Hello");
        assert_eq!(text.size, 36.0);
    }

    /// **The one that changes what a script is for.** A few lines of prose
    /// become a staged scene with a cast animated onto the timeline — and it is
    /// ordinary layers and keyframes afterwards, which the script goes on to
    /// read.
    #[test]
    fn a_script_can_direct_a_whole_scene() {
        let mut scene = Scene::default();
        let outcome = run_source(
            &mut scene,
            r#"
            var doc = fl.getDocumentDOM();
            var frames = doc.direct("Night. Ana walks in from the left.\nAna talks to Ben.");
            fl.trace("frames=" + frames);
            fl.trace("layers=" + doc.getTimeline().layers.length);
            "#,
        );
        assert!(outcome.error.is_none(), "{:?}", outcome.error);

        let traced = outcome.trace.join("\n");
        let frames: u32 = traced
            .lines()
            .find_map(|l| l.strip_prefix("frames=")?.parse().ok())
            .expect("the shot's length came back to the script");
        assert!(frames > 0, "a directed shot has a length");

        // The cast is really there, as layers with artwork on them.
        assert!(
            scene.layers().len() > 2,
            "a set and a cast is more than one layer, got {}",
            scene.layers().len()
        );
    }

    #[test]
    fn a_story_the_director_cannot_read_says_so() {
        let mut scene = Scene::default();
        let outcome = run_source(&mut scene, r#"fl.getDocumentDOM().direct("");"#);
        assert!(
            outcome.error.is_some(),
            "an empty brief is nothing to direct"
        );
    }

    // -- the script shelf ---------------------------------------------------

    /// **The line every command on a shelf opens with.**
    ///
    /// A JSFL shelf shares its settings through one file, pulled in with
    /// `fl.runScript(fl.configURI + "Commands/...")`, and everything after
    /// that line depends on the variables it declares. Those are `var`
    /// declarations, so the file has to be evaluated in *global* scope or the
    /// caller sees none of them.
    #[test]
    fn a_script_can_pull_in_the_settings_file_beside_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let commands = dir.path().join("Commands");
        std::fs::create_dir_all(&commands).expect("commands dir");
        std::fs::write(
            commands.join("commonVariables.jsfl"),
            "var moveFrames = 6;
var CONFIG = { moveAmount: 5, browFrames: 10 };
",
        )
        .expect("write");

        let context = ScriptContext {
            config_dir: Some(dir.path().to_path_buf()),
            ..ScriptContext::default()
        };
        let mut scene = document();
        let outcome = run(
            &mut scene,
            context,
            r#"
              var file = fl.configURI + "Commands/commonVariables.jsfl";
              fl.runScript(file);
              fl.trace("frames=" + moveFrames);
              fl.trace("amount=" + CONFIG.moveAmount);
              fl.trace("brow=" + CONFIG.browFrames);
            "#,
            &Limits::default(),
        );

        assert_eq!(outcome.error, None, "the include should have run");
        assert_eq!(
            outcome.trace,
            vec!["frames=6", "amount=5", "brow=10"],
            "the settings must land in global scope where the caller can see them"
        );
    }

    /// A shelf may read the scripts beside it and **nothing else**. Reading
    /// files is a capability a script otherwise has none of.
    #[test]
    fn a_script_cannot_read_past_its_own_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let commands = dir.path().join("Commands");
        std::fs::create_dir_all(&commands).expect("commands dir");
        let secret = dir.path().parent().expect("a parent").join("buzz-secret.txt");
        std::fs::write(&secret, "not for scripts").expect("write");

        let context = ScriptContext {
            config_dir: Some(dir.path().to_path_buf()),
            ..ScriptContext::default()
        };
        let mut scene = document();
        let outcome = run(
            &mut scene,
            context,
            &format!(
                "fl.runScript({:?});",
                secret.display().to_string().replace(char::from(92), "/")
            ),
            &Limits::default(),
        );
        assert!(
            outcome.error.is_some(),
            "reading outside the script folder must fail, not succeed quietly"
        );
        let _ = std::fs::remove_file(&secret);
    }

    /// **What a real shelf of JSFL commands does in this engine.**
    ///
    /// Not a pass/fail test — a *report*. Point it at a Configuration folder
    /// and it runs every command in it against an empty document, then prints
    /// how far each got and what stopped it. That turns "do my scripts work"
    /// from an opinion into a list of missing calls in the order they matter.
    ///
    /// Ignored by default because it reads a folder that only exists on a
    /// machine with a shelf on it. Run it deliberately:
    ///
    /// ```text
    /// cargo test -p buzz-script --lib jsfl_shelf -- --ignored --nocapture
    /// ```
    ///
    /// Set `BUZZ_JSFL_DIR` to point it at a Configuration folder; it falls back
    /// to whatever [`default_config_dir`] finds.
    #[test]
    #[ignore = "reads a JSFL shelf that only exists on a machine that has one"]
    fn jsfl_shelf_compatibility_report() {
        let root = std::env::var_os("BUZZ_JSFL_DIR")
            .map(std::path::PathBuf::from)
            .or_else(default_config_dir);
        let Some(root) = root else {
            println!("no Configuration folder found; nothing to report on");
            return;
        };
        let commands = root.join("Commands");
        let Ok(entries) = std::fs::read_dir(&commands) else {
            println!("no Commands folder under {}", root.display());
            return;
        };

        let mut scripts: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("jsfl"))
            })
            .collect();
        scripts.sort();

        // What stopped each script, tallied, so the most valuable thing to
        // implement next is the one at the top.
        let mut blockers: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let mut ran = 0usize;

        println!("\n=== {} scripts in {} ===", scripts.len(), commands.display());
        for path in &scripts {
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let context = ScriptContext {
                config_dir: Some(root.clone()),
                ..ScriptContext::default()
            };
            let mut scene = document();
            let outcome = run(&mut scene, context, &source, &Limits::default());
            match &outcome.error {
                None => {
                    ran += 1;
                    println!("  ok    {name}");
                }
                Some(error) => {
                    let first = error.lines().next().unwrap_or(error).trim().to_string();
                    // "X is not a function" / "cannot read property Y" carry the
                    // missing call; that is the part worth counting.
                    *blockers.entry(first.clone()).or_default() += 1;
                    println!("  stop  {name}: {first}");
                }
            }
        }

        println!("\n{ran} of {} ran to the end", scripts.len());
        let mut ranked: Vec<(&String, &usize)> = blockers.iter().collect();
        ranked.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        println!("\nwhat stopped the rest, most common first:");
        for (reason, count) in ranked.iter().take(25) {
            println!("  {count:>3}  {reason}");
        }
    }

    // -- stopping -----------------------------------------------------------

    use std::sync::atomic::AtomicUsize;

    /// A script that would run for a minute stops when the host says so, and
    /// does it in far less than the time limit — this is what makes the Stop
    /// button a button rather than a suggestion.
    #[test]
    fn the_host_can_stop_a_running_script() {
        let mut scene = document();
        let asked = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&asked);

        // Say stop the moment the engine first asks.
        let stop: StopSignal = Arc::new(move || {
            flag.store(true, Ordering::Relaxed);
            true
        });

        let started = Instant::now();
        let out = run_until(
            &mut scene,
            ScriptContext::default(),
            "while (true) {}",
            &Limits {
                time: Duration::from_secs(60),
                ..Limits::default()
            },
            Some(stop),
        );
        let elapsed = started.elapsed();

        assert!(out.stopped, "it should say the host stopped it");
        assert!(asked.load(Ordering::Relaxed), "nobody was asked");
        assert!(
            elapsed < Duration::from_secs(5),
            "stopping took {elapsed:?}, which is not stopping"
        );
    }

    /// Stopping is not failing. A script that built four hundred shapes and
    /// was then stopped has still built them, and saying "Script failed" over
    /// the top of that would be a lie.
    #[test]
    fn a_stopped_script_is_not_a_failed_one() {
        let mut scene = document();
        // Let it get going first, so this tests a script that was stopped
        // part-way rather than one that never started.
        let asked = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&asked);
        let stop: StopSignal = Arc::new(move || count.fetch_add(1, Ordering::Relaxed) > 200);

        let out = run_until(
            &mut scene,
            ScriptContext::default(),
            "fl.trace('starting'); while (true) {}",
            &Limits::default(),
            Some(stop),
        );

        assert!(out.stopped);
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(out.summary(), "Script stopped");
        // What it managed before being stopped is kept.
        assert_eq!(out.trace, vec!["starting".to_string()]);
    }

    /// Nobody asking is the ordinary case, and it must not change what the
    /// engine does.
    #[test]
    fn a_signal_that_never_fires_changes_nothing() {
        let mut scene = document();
        let stop: StopSignal = Arc::new(|| false);

        let out = run_until(
            &mut scene,
            ScriptContext::default(),
            "fl.trace('hello');",
            &Limits::default(),
            Some(stop),
        );

        assert!(!out.stopped);
        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["hello".to_string()]);
    }

    /// The time limit still applies when a signal is present — the two are not
    /// alternatives, and a runaway script with a signal nobody is watching must
    /// still end.
    #[test]
    fn the_time_limit_still_applies_alongside_a_signal() {
        let mut scene = document();
        let stop: StopSignal = Arc::new(|| false);

        let out = run_until(
            &mut scene,
            ScriptContext::default(),
            "while (true) {}",
            &Limits {
                time: Duration::from_millis(250),
                ..Limits::default()
            },
            Some(stop),
        );

        assert!(!out.stopped, "the clock stopped it, not the host");
        let message = out.error.expect("it should have been interrupted");
        assert!(message.contains("time limit"), "{message}");
    }

    // -- the engine ---------------------------------------------------------

    #[test]
    fn a_script_can_trace_output() {
        let mut scene = document();
        let out = run_source(&mut scene, "fl.trace('hello'); fl.trace(42);");

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["hello".to_string(), "42".to_string()]);
        assert!(!out.changed, "tracing is not a document change");
    }

    /// The property the whole feature rests on: arbitrary user code must not
    /// be able to wedge the editor.
    #[test]
    fn an_endless_loop_is_stopped_rather_than_hanging() {
        let mut scene = document();
        let limits = Limits {
            time: Duration::from_millis(250),
            ..Limits::default()
        };

        let started = Instant::now();
        let out = run(
            &mut scene,
            ScriptContext::default(),
            "while (true) { }",
            &limits,
        );
        let elapsed = started.elapsed();

        assert!(!out.succeeded(), "an endless loop must fail");
        assert!(
            elapsed < Duration::from_secs(3),
            "the interrupt should fire promptly, took {elapsed:?}"
        );
        let message = out.error.unwrap();
        assert!(
            message.contains("time limit") || message.contains("stopped"),
            "the message should name the cause: {message}"
        );
    }

    /// Runaway recursion has to fail the script, not blow the real stack.
    #[test]
    fn runaway_recursion_fails_the_script_and_not_the_process() {
        let mut scene = document();
        let out = run_source(&mut scene, "function f(){ return f(); } f();");
        assert!(!out.succeeded());
    }

    /// QuickJS words an interrupt as an exception reading "interrupted", which
    /// a script can also throw itself. Only the deadline actually firing may
    /// produce the timeout message.
    #[test]
    fn a_script_throwing_the_word_interrupted_is_not_called_a_timeout() {
        let mut scene = document();
        let out = run_source(&mut scene, "throw new Error('interrupted');");

        assert!(!out.succeeded());
        let message = out.error.unwrap();
        assert!(message.contains("interrupted"), "{message}");
        assert!(
            !message.contains("time limit"),
            "a thrown error was reported as a timeout: {message}"
        );
    }

    #[test]
    fn a_syntax_error_is_reported_with_something_useful() {
        let mut scene = document();
        let out = run_source(&mut scene, "this is not javascript(");

        assert!(!out.succeeded());
        let message = out.error.unwrap();
        assert!(!message.is_empty());
        assert!(
            message.to_lowercase().contains("expect")
                || message.to_lowercase().contains("unexpected")
                || message.to_lowercase().contains("syntax"),
            "expected a parse complaint, got: {message}"
        );
    }

    #[test]
    fn a_thrown_error_reports_its_message() {
        let mut scene = document();
        let out = run_source(&mut scene, "throw new Error('deliberate');");

        assert!(!out.succeeded());
        assert!(out.error.unwrap().contains("deliberate"));
    }

    /// Work done before a failure is kept. Discarding an hour of generated
    /// artwork because the last line had a typo would be indefensible.
    #[test]
    fn work_done_before_an_error_survives() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM();
             d.addNewRectangle({left:0, top:0, right:50, bottom:50});
             throw new Error('too late');",
        );

        assert!(!out.succeeded());
        assert!(out.changed, "the rectangle should still be there");
        assert_eq!(scene.shape_count_at(0), 1);
    }

    /// The engine must not hand scripts a way out of the editor.
    #[test]
    fn a_script_cannot_reach_the_file_system_or_the_network() {
        let mut scene = document();
        for probe in [
            "typeof require",
            "typeof fetch",
            "typeof process",
            "typeof globalThis.open",
            "typeof XMLHttpRequest",
        ] {
            let out = run_source(&mut scene, &format!("fl.trace({probe});"));
            assert!(out.succeeded(), "{probe}: {:?}", out.error);
            assert_eq!(
                out.trace,
                vec!["undefined".to_string()],
                "{probe} should not exist"
            );
        }
    }

    // -- the document API ---------------------------------------------------

    #[test]
    fn a_script_reads_the_documents_dimensions() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM(); fl.trace(d.width + 'x' + d.height);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["550x400".to_string()]);
    }

    #[test]
    fn a_script_can_resize_the_document() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM(); d.width = 1920; d.height = 1080;",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert!(out.changed);
        assert_eq!(scene.stage().size.width, 1920.0);
        assert_eq!(scene.stage().size.height, 1080.0);
    }

    #[test]
    fn a_script_draws_rectangles_and_ovals() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM();
             d.setFillColor('#FF0000');
             for (var i = 0; i < 5; i++) {
                 d.addNewRectangle({left: i*20, top: 0, right: i*20+15, bottom: 15});
             }
             d.addNewOval({left: 0, top: 40, right: 30, bottom: 70});",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(scene.shape_count_at(0), 6);
    }

    #[test]
    fn a_script_manages_layers() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var t = fl.getDocumentDOM().getTimeline();
             fl.trace(t.layerCount);
             t.addNewLayer('Scripted');
             fl.trace(t.layerCount);
             t.layers[0].name = 'Renamed';
             t.layers[0].visible = false;
             fl.trace(t.layers[0].name);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["2", "3", "Renamed"]);

        let top = scene.layers().iter().next().unwrap();
        assert_eq!(top.name, "Renamed");
        assert!(!top.visible);
    }

    /// Layer depth is ours rather than Animate's, and a script is exactly how
    /// somebody would want to set up a parallax stack.
    #[test]
    fn a_script_can_set_layer_depth() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var t = fl.getDocumentDOM().getTimeline();
             for (var i = 0; i < t.layerCount; i++) {
                 t.layers[i].depth = i * 500;
             }",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        let depths: Vec<f64> = scene.layers().iter().map(|l| l.depth).collect();
        assert_eq!(depths, vec![0.0, 500.0]);
    }

    #[test]
    fn a_script_works_with_frames() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var t = fl.getDocumentDOM().getTimeline();
             t.insertFrames(10);
             fl.trace(t.frameCount);
             t.currentFrame = 5;
             fl.trace(t.currentFrame);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["11", "5"]);
        assert_eq!(out.context.current_frame, 5);
    }

    #[test]
    fn a_script_can_select_and_move_artwork() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM();
             d.addNewRectangle({left:0, top:0, right:10, bottom:10});
             d.selectAll();
             fl.trace(d.selection.length);
             d.moveSelectionBy({x: 100, y: 50});
             d.selectNone();
             fl.trace(d.selection.length);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["1", "0"]);

        // The rectangle actually moved.
        let bounds = scene.content_bounds().expect("there is artwork");
        assert!((bounds.x0 - 100.0).abs() < 1e-6, "got {bounds:?}");
    }

    #[test]
    fn a_script_can_make_a_symbol_from_the_selection() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM();
             d.addNewRectangle({left:0, top:0, right:40, bottom:40});
             d.selectAll();
             d.convertToSymbol('graphic', 'Scripted Symbol');
             fl.trace(d.library.itemCount);
             fl.trace(d.library.items[0].name);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["1", "Scripted Symbol"]);
        assert_eq!(scene.library().len(), 1);
    }

    #[test]
    fn a_script_that_changes_nothing_reports_no_change() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM(); fl.trace(d.width);",
        );

        assert!(out.succeeded());
        assert!(!out.changed, "reading must not mark the document dirty");
    }

    /// Nothing about the API should panic on an empty document, which is what
    /// a script will meet most often.
    #[test]
    fn the_api_copes_with_an_empty_document() {
        let mut scene = Scene::empty();
        let out = run_source(
            &mut scene,
            "var d = fl.getDocumentDOM();
             var t = d.getTimeline();
             fl.trace(t.layerCount);
             fl.trace(d.selection.length);
             fl.trace(d.library.itemCount);
             d.selectAll();
             d.deleteSelection();",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["0", "0", "0"]);
    }

    /// Out-of-range indices are a scripting mistake, and should say so rather
    /// than panic or silently do the wrong thing.
    #[test]
    fn addressing_a_layer_that_does_not_exist_is_an_error_not_a_crash() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var t = fl.getDocumentDOM().getTimeline(); t.layers[99].name = 'x';",
        );
        assert!(!out.succeeded(), "expected a complaint about the index");
    }

    #[test]
    fn deleting_a_layer_works_and_the_last_one_is_refused() {
        let mut scene = document();
        let out = run_source(
            &mut scene,
            "var t = fl.getDocumentDOM().getTimeline();
             t.deleteLayer(0);
             fl.trace(t.layerCount);
             var refused = false;
             try { t.deleteLayer(0); } catch (e) { refused = true; }
             fl.trace(refused);",
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert_eq!(out.trace, vec!["1", "true"]);
        assert_eq!(
            scene.layers().len(),
            1,
            "a document keeps at least one layer"
        );
    }
}
