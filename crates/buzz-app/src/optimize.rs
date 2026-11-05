//! **Optimize Shapes…** in the editor (`docs/TRACE_PICTURE_PLAN.md` T13): the
//! scope (the selection's shapes, else the active layer's frame), the dialog's
//! preview, and Apply as one undo step. The geometry is
//! [`crate::rigging::optimize_shapes`]; it runs off the UI thread like Even
//! Out Lines (T12): [`OptimizeState::work`] is run by the app on a task
//! thread and [`Editor::finish_optimize`] takes the result in.

use crate::editor::Editor;
use crate::rigging::{OptimizeSettings, Optimized, optimize_shapes};
use buzz_geom::Affine;
use buzz_scene::{LayerId, Object, ObjectId, ObjectKind, Paint, Scene, ShapeData, SymbolId};
use std::sync::Arc;

/// One keyframe with shapes to optimize.
struct OptKey {
    owner: Option<SymbolId>,
    layer: LayerId,
    start: u32,
    original: Arc<Vec<Arc<Object>>>,
    /// Positions in `original` of the shapes optimized (in paint order).
    members: Vec<usize>,
    inputs: Vec<(Affine, ShapeData)>,
}

/// What Optimize Shapes works on.
pub struct OptimizePlan {
    keys: Vec<OptKey>,
}

impl OptimizePlan {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Optimize work for a task thread.
pub struct OptimizeWork {
    pub plan: Arc<OptimizePlan>,
    pub settings: OptimizeSettings,
}

/// What [`OptimizeWork`] came to: one result per keyframe of the plan.
pub struct OptimizeDone(pub Vec<Optimized>);

/// Run the work on the calling thread.
pub fn run_optimize_work(work: &OptimizeWork, cancelled: &dyn Fn() -> bool) -> OptimizeDone {
    OptimizeDone(work.plan.keys.iter().map(|k| optimize_shapes(&k.inputs, &work.settings, cancelled)).collect())
}

/// A shape this can work on: solid fill or a stroke, no filters on it.
fn eligible(object: &Object) -> Option<&ShapeData> {
    let ObjectKind::Shape(sd) = &object.kind else { return None };
    if !object.filters.is_empty() || object.locked {
        return None;
    }
    let paint_ok = |p: &Paint| matches!(p, Paint::Solid(_));
    let fill_ok = sd.fill.as_ref().is_none_or(|f| paint_ok(&f.paint));
    let stroke_ok = sd.stroke.as_ref().is_none_or(|s| paint_ok(&s.paint));
    (fill_ok && stroke_ok && (sd.fill.is_some() || sd.stroke.is_some())).then_some(sd)
}

/// The shapes of `ids` (else of `layer`'s keyframe at `frame`) on the
/// timeline being edited.
fn plan_optimize(scene: &Scene, ids: &[ObjectId], layer: Option<LayerId>, frame: u32) -> OptimizePlan {
    let owner = scene.editing_symbol();
    let mut keys: Vec<OptKey> = Vec::new();
    for l in scene.layers().iter() {
        let Some(kf) = l.frames.keyframe_at(frame) else { continue };
        let chosen = |o: &Object| if ids.is_empty() { layer == Some(l.id) } else { ids.contains(&o.id) };
        let mut members = Vec::new();
        let mut inputs = Vec::new();
        for (i, o) in kf.objects.iter().enumerate() {
            if !chosen(o) {
                continue;
            }
            if let Some(sd) = eligible(o) {
                members.push(i);
                inputs.push((o.transform, sd.clone()));
            }
        }
        if !members.is_empty() {
            keys.push(OptKey { owner, layer: l.id, start: kf.start, original: kf.objects.clone(), members, inputs });
        }
    }
    OptimizePlan { keys }
}

/// Write `done` into `scene`: each keyframe is the plan's original with its
/// optimized shapes in place of the ones they came from (the first source's
/// slot and id; the other sources of a merge are gone).
fn apply_optimize(scene: &mut Scene, plan: &OptimizePlan, done: &OptimizeDone) {
    for (key, result) in plan.keys.iter().zip(&done.0) {
        if result.pieces.is_empty() && result.shapes_before > 0 && result.segments_before > 0 {
            continue; // cancelled run: nothing to write
        }
        let mut first: std::collections::HashMap<usize, &crate::rigging::Piece> = std::collections::HashMap::new();
        for p in &result.pieces {
            first.insert(key.members[p.sources[0]], p);
        }
        let member: std::collections::HashSet<usize> = key.members.iter().copied().collect();
        let mut out: Vec<Arc<Object>> = Vec::with_capacity(key.original.len());
        for (i, o) in key.original.iter().enumerate() {
            if !member.contains(&i) {
                out.push(o.clone());
            } else if let Some(p) = first.get(&i) {
                let mut object = (**o).clone();
                object.transform = p.transform;
                object.kind = ObjectKind::Shape(p.shape.clone());
                out.push(Arc::new(object));
            }
        }
        let start = key.start;
        let write = move |layer: &mut buzz_scene::Layer| {
            if let Some(kf) = layer.frames.keyframes_mut().iter_mut().find(|kf| kf.start == start) {
                kf.objects = Arc::new(out);
            }
        };
        match key.owner {
            None => {
                scene.update_stage_layer(key.layer, write);
            }
            Some(symbol) => {
                scene.library_mut().update(symbol, |s| {
                    s.layers.update(key.layer, write);
                });
            }
        }
    }
}

fn counts(done: &OptimizeDone) -> (usize, usize, usize, usize) {
    done.0.iter().fold((0, 0, 0, 0), |a, r| {
        (a.0 + r.shapes_before, a.1 + r.pieces.len(), a.2 + r.segments_before, a.3 + r.segments_after)
    })
}

/// What Optimize did, for the agent and the probes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OptimizeReport {
    pub shapes_before: usize,
    pub shapes_after: usize,
    pub segments_before: usize,
    pub segments_after: usize,
}

/// The dialog's preview.
pub struct OptimizeSession {
    plan: Arc<OptimizePlan>,
    done: Option<OptimizeDone>,
    checkpoint: buzz_doc::RunCheckpoint,
}

/// The editor's Optimize Shapes state.
#[derive(Default)]
pub struct OptimizeState {
    pub dialog: buzz_ui::OptimizeDialogState,
    session: Option<OptimizeSession>,
    /// Work waiting for the app to run it on a task thread, with the
    /// generation it answers.
    pub work: Option<(u64, OptimizeWork)>,
    /// Bumped by every request and by Cancel; older answers are dropped.
    pub generation: u64,
    queued: bool,
    revision: u64,
}

impl OptimizeState {
    fn settings(&self) -> OptimizeSettings {
        let d = &self.dialog;
        OptimizeSettings { smoothing: d.smoothing, merge_same_colour: d.merge_same_colour, remove_tiny: d.remove_tiny }
    }
}

impl Editor {
    fn optimize_plan(&mut self, element: Option<ObjectId>) -> OptimizePlan {
        let ids: Vec<ObjectId> = match element {
            Some(id) => vec![id],
            None => self.selection.iter().collect(),
        };
        let scene = self.doc.scene().clone();
        let layer = self.selection.ensure_active_layer(&scene);
        plan_optimize(&scene, &ids, layer, self.current_frame)
    }

    /// **Optimize Shapes…**: open the dialog; the preview is made off the UI
    /// thread (the app runs [`OptimizeState::work`]).
    pub fn open_optimize_shapes(&mut self) {
        if self.optimize.session.is_some() || self.optimize.dialog.open {
            return;
        }
        let plan = self.optimize_plan(None);
        if plan.is_empty() {
            self.status = Some("Optimize Shapes: no shapes here (select some, or pick a layer with artwork)".into());
            return;
        }
        self.doc.end_gesture();
        self.optimize.session = Some(OptimizeSession { plan: Arc::new(plan), done: None, checkpoint: self.doc.run_checkpoint() });
        self.optimize.revision = self.doc.scene().revision();
        self.optimize.dialog.open = true;
        self.optimize_request();
    }

    fn optimize_request(&mut self) {
        if self.optimize.dialog.working {
            self.optimize.queued = true;
            return;
        }
        let Some(session) = self.optimize.session.as_ref() else { return };
        self.optimize.generation += 1;
        self.optimize.work = Some((
            self.optimize.generation,
            OptimizeWork { plan: session.plan.clone(), settings: self.optimize.settings() },
        ));
        self.optimize.dialog.working = true;
    }

    /// Run the queued work now, on the calling thread (tests, probes, agent).
    pub fn finish_optimize_now(&mut self) {
        while let Some((generation, work)) = self.optimize.work.take() {
            let done = run_optimize_work(&work, &|| false);
            self.finish_optimize(generation, done);
        }
    }

    /// An Optimize task came to `done`: preview it, then run what was asked
    /// meanwhile.
    pub fn finish_optimize(&mut self, generation: u64, done: OptimizeDone) {
        if generation != self.optimize.generation || !self.optimize.dialog.open {
            return;
        }
        self.optimize.dialog.working = false;
        if self.doc.scene().revision() != self.optimize.revision && self.optimize.session.as_ref().is_some_and(|s| s.done.is_none()) {
            // The document changed under the first run: start over from it.
            self.optimize_action(buzz_ui::OptimizeAction::Cancel);
            self.status = Some("Optimize Shapes: the document changed meanwhile, run it again".into());
            return;
        }
        let Some(session) = self.optimize.session.as_mut() else { return };
        let plan = session.plan.clone();
        self.doc.edit("Optimize Shapes", |scene| apply_optimize(scene, &plan, &done));
        self.optimize.revision = self.doc.scene().revision();
        let (sb, sa, gb, ga) = counts(&done);
        let d = &mut self.optimize.dialog;
        (d.shapes_before, d.shapes_after, d.segments_before, d.segments_after) = (sb, sa, gb, ga);
        if let Some(session) = self.optimize.session.as_mut() {
            session.done = Some(done);
        }
        if std::mem::take(&mut self.optimize.queued) {
            self.optimize_request();
        }
    }

    /// What the Optimize Shapes dialog asked for.
    pub fn optimize_action(&mut self, action: buzz_ui::OptimizeAction) {
        use buzz_ui::OptimizeAction as A;
        match action {
            A::None => {}
            A::Changed => self.optimize_request(),
            A::Apply => {
                if self.optimize.dialog.working {
                    return;
                }
                self.optimize.dialog.open = false;
                let Some(session) = self.optimize.session.take() else { return };
                self.doc.collapse_run(session.checkpoint, "Optimize Shapes");
                self.doc.end_gesture();
                self.selection.prune(self.doc.scene());
                let d = &self.optimize.dialog;
                self.status = Some(format!(
                    "Optimize Shapes: {} \u{2192} {} shapes, {} \u{2192} {} segments",
                    d.shapes_before, d.shapes_after, d.segments_before, d.segments_after
                ));
            }
            A::Cancel => {
                self.optimize.dialog.open = false;
                self.optimize.dialog.working = false;
                self.optimize.generation += 1;
                self.optimize.work = None;
                self.optimize.queued = false;
                let Some(session) = self.optimize.session.take() else { return };
                self.doc.restore_checkpoint(session.checkpoint);
                self.doc.end_gesture();
                self.selection.prune(self.doc.scene());
            }
        }
    }

    /// **Optimize Shapes** without the dialog (the agent's `optimize_shapes`):
    /// `element` or the selection, else the active layer's frame; one undo
    /// step. `Err` (and the status line) when there is nothing to optimize.
    pub fn optimize_shapes_now(&mut self, element: Option<ObjectId>, settings: OptimizeSettings) -> Result<OptimizeReport, String> {
        let plan = self.optimize_plan(element);
        if plan.is_empty() {
            let message = "Optimize Shapes: no shapes here (select some, or pick a layer with artwork)".to_string();
            self.status = Some(message.clone());
            return Err(message);
        }
        let work = OptimizeWork { plan: Arc::new(plan), settings };
        let done = run_optimize_work(&work, &|| false);
        let plan = work.plan;
        self.doc.end_gesture();
        self.doc.edit("Optimize Shapes", |scene| apply_optimize(scene, &plan, &done));
        self.doc.end_gesture();
        self.selection.prune(self.doc.scene());
        let (shapes_before, shapes_after, segments_before, segments_after) = counts(&done);
        self.status = Some(format!(
            "Optimize Shapes: {shapes_before} \u{2192} {shapes_after} shapes, {segments_before} \u{2192} {segments_after} segments"
        ));
        Ok(OptimizeReport { shapes_before, shapes_after, segments_before, segments_after })
    }
}
