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
}

/// What a run produced.
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    /// Everything the script passed to `fl.trace`, in order.
    pub trace: Vec<String>,
    /// The failure, already formatted for a person, or `None` on success.
    pub error: Option<String>,
    /// Editor state as the script left it.
    pub context: ScriptContext,
    /// Did the document actually change?
    pub changed: bool,
    pub elapsed: Duration,
}

impl ScriptOutcome {
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }

    /// A line for the status bar.
    pub fn summary(&self) -> String {
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
    let started = Instant::now();
    let revision_before = scene.revision();

    let state = Rc::new(RefCell::new(State {
        // Cheap: the scene is copy-on-write, so this shares its artwork until
        // the script actually changes something.
        scene: scene.clone(),
        context,
        trace: Vec::new(),
    }));

    let error = evaluate(&state, source, limits);

    // Take the work back out, whether or not the script finished. A partial
    // result is still the user's work.
    let finished = Rc::try_unwrap(state)
        .unwrap_or_else(|rc| RefCell::new(rc.borrow().clone_state()))
        .into_inner();

    let changed = finished.scene.revision() != revision_before;
    *scene = finished.scene;

    ScriptOutcome {
        trace: finished.trace,
        error,
        context: finished.context,
        changed,
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
        }
    }
}

/// Build the engine, install the API, and evaluate.
fn evaluate(state: &Rc<RefCell<State>>, source: &str, limits: &Limits) -> Option<String> {
    let runtime = match rquickjs::Runtime::new() {
        Ok(r) => r,
        Err(e) => return Some(format!("could not start the script engine: {e}")),
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
    let fired = Arc::clone(&timed_out);
    let deadline = Instant::now() + limits.time;
    runtime.set_interrupt_handler(Some(Box::new(move || {
        if Instant::now() >= deadline {
            fired.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    })));

    let context = match rquickjs::Context::full(&runtime) {
        Ok(c) => c,
        Err(e) => return Some(format!("could not start the script context: {e}")),
    };

    context.with(|ctx| {
        if let Err(e) = host::install(&ctx, state) {
            return Some(format!("could not install the script API: {e}"));
        }

        // The prelude shapes the host primitives into Animate's API. A failure
        // here is our bug, not the user's, so it says so.
        if let Err(e) = ctx.eval::<(), _>(PRELUDE) {
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
                Some(if timed_out.load(Ordering::Relaxed) {
                    TIMED_OUT.to_string()
                } else {
                    described
                })
            }
        }
    })
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
