//! **Clean Up Layers** (L2 of `docs/LAYER_MANAGEMENT_PLAN.md`): delete empty
//! layers, trim trailing blank keyframes, remove empty folders.
//!
//! One rule for preview and apply: [`LayerStack::cleanup_plan`] says what
//! would change, [`LayerStack::apply_cleanup`] does exactly that, and
//! [`Scene::cleanup_preview`] / [`Scene::cleanup_layers`] run the same plans
//! over the chosen scope.

use std::collections::HashSet;

use crate::layer::{Layer, LayerId, LayerKind, LayerStack};
use crate::symbol::SymbolId;
use crate::timeline::LayerTimeline;
use crate::Scene;

/// Which passes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupOptions {
    /// Delete layers with no artwork and no sound in any keyframe.
    pub empty_layers: bool,
    /// Trim the run of blank keyframes after a layer's last content.
    pub trim: bool,
    /// Remove folders with no children.
    pub folders: bool,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self {
            empty_layers: true,
            trim: true,
            folders: true,
        }
    }
}

/// Which timelines a cleanup covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanupScope {
    /// The timeline being edited (the open symbol, or the stage).
    #[default]
    Timeline,
    /// The stage plus every library symbol.
    WholeScene,
}

/// What a cleanup would do to one layer stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupPlan {
    /// Non-folder layers to delete, front to back.
    pub empty_layers: Vec<LayerId>,
    /// Layers to shorten, with their new length in frames.
    pub trims: Vec<(LayerId, u32)>,
    /// Folders to delete (empty, or emptied by this plan), front to back.
    pub empty_folders: Vec<LayerId>,
}

impl CleanupPlan {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.empty_layers.is_empty() && self.trims.is_empty() && self.empty_folders.is_empty()
    }

    /// Frames the trims remove from `stack` (the stack the plan was made for).
    pub fn frames_trimmed(&self, stack: &LayerStack) -> usize {
        self.trims
            .iter()
            .filter_map(|(id, len)| stack.get(*id).map(|l| l.length().saturating_sub(*len) as usize))
            .sum()
    }
}

/// What [`Scene::cleanup_layers`] did (or [`Scene::cleanup_preview`] would do).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub layers_deleted: usize,
    pub frames_trimmed: usize,
    pub folders_removed: usize,
    /// Library symbols changed.
    pub symbols_touched: usize,
}

impl CleanupReport {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.layers_deleted == 0 && self.frames_trimmed == 0 && self.folders_removed == 0
    }
}

/// Does this keyframe hold anything worth keeping? Sound counts, and so does
/// a frame label (scripts and the timeline read labels off blank keyframes).
fn keyframe_has_content(k: &crate::timeline::Keyframe) -> bool {
    !k.objects.is_empty() || k.sound.is_some() || k.label.is_some()
}

/// Empty for cleanup purposes: [`Layer::is_empty_layer`] and no labels.
fn cleanup_empty(layer: &Layer) -> bool {
    layer.is_empty_layer() && layer.frames.keyframes().iter().all(|k| k.label.is_none())
}

/// Where a layer's content ends: the start of the blank run after its last
/// content keyframe, or its length when it has no such run. `None` with no
/// content at all.
fn content_end(layer: &Layer) -> Option<u32> {
    let keys = layer.frames.keyframes();
    let last = keys.iter().rposition(keyframe_has_content)?;
    Some(keys.get(last + 1).map_or(layer.length(), |k| k.start))
}

/// Mask/guide runs are positional, so these kinds belong to the run below a
/// mask or guide.
fn is_run_member(kind: LayerKind) -> bool {
    matches!(kind, LayerKind::Masked | LayerKind::Guided)
}

impl LayerStack {
    /// The full cleanup (every pass on) for this stack.
    pub fn cleanup_plan(&self) -> CleanupPlan {
        self.cleanup_plan_with(CleanupOptions::default(), &HashSet::new())
    }

    /// The cleanup for this stack with only the chosen passes. `protected`
    /// layers are never deleted (the stage passes the layers lights sit
    /// above).
    ///
    /// Never deleted: a folder with children, a layer something files under
    /// or follows, a mask or guide with its run beneath it, a layer whose
    /// removal would join the run below it to a mask or guide above, an
    /// `audio_guide` layer, a layer with a frame label — and never every
    /// layer (a timeline keeps at least one). Deletions are planned to a
    /// fixpoint, so a folder emptied by this plan counts as empty.
    pub fn cleanup_plan_with(&self, opts: CleanupOptions, protected: &HashSet<LayerId>) -> CleanupPlan {
        let layers: Vec<&std::sync::Arc<Layer>> = self.iter().collect();
        let mut plan = CleanupPlan::default();
        let mut gone: HashSet<LayerId> = HashSet::new();

        if opts.empty_layers || opts.folders {
            // Bounded: each round removes at least one layer or stops.
            for _ in 0..=layers.len() {
                let mut changed = false;
                for (i, layer) in layers.iter().enumerate() {
                    if gone.contains(&layer.id) || protected.contains(&layer.id) || layer.audio_guide {
                        continue;
                    }
                    let folder = layer.kind == LayerKind::Folder;
                    let wanted = if folder {
                        opts.folders
                    } else {
                        opts.empty_layers && cleanup_empty(layer)
                    };
                    if !wanted {
                        continue;
                    }
                    let depended = layers.iter().any(|o| {
                        o.id != layer.id
                            && !gone.contains(&o.id)
                            && (o.parent == Some(layer.id) || o.follows == Some(layer.id))
                    });
                    if depended {
                        continue;
                    }
                    let below = layers[i + 1..].iter().find(|o| !gone.contains(&o.id));
                    if let Some(below) = below
                        && is_run_member(below.kind)
                        && below.kind != layer.kind
                    {
                        continue;
                    }
                    gone.insert(layer.id);
                    changed = true;
                }
                if !changed {
                    break;
                }
            }
            // A timeline keeps at least one layer: spare the first top-level
            // one (preferring a drawing layer over a folder).
            if !layers.is_empty() && gone.len() == layers.len() {
                let spare = layers
                    .iter()
                    .find(|l| l.parent.is_none() && l.kind != LayerKind::Folder)
                    .or_else(|| layers.iter().find(|l| l.parent.is_none()))
                    .unwrap_or(&layers[0]);
                gone.remove(&spare.id);
            }
            for layer in &layers {
                if gone.contains(&layer.id) {
                    if layer.kind == LayerKind::Folder {
                        plan.empty_folders.push(layer.id);
                    } else {
                        plan.empty_layers.push(layer.id);
                    }
                }
            }
        }

        if opts.trim {
            let stack_end = layers
                .iter()
                .filter(|l| !gone.contains(&l.id) && l.kind != LayerKind::Folder)
                .filter_map(|l| content_end(l))
                .max()
                .unwrap_or(0);
            for layer in &layers {
                if gone.contains(&layer.id) || layer.kind == LayerKind::Folder || layer.audio_guide {
                    continue;
                }
                let blank_from = content_end(layer).unwrap_or(0);
                let new_len = blank_from.max(stack_end).max(1);
                if new_len < layer.length() {
                    plan.trims.push((layer.id, new_len));
                }
            }
        }
        plan
    }

    /// Apply a plan made by [`Self::cleanup_plan`] for this stack.
    pub fn apply_cleanup(&mut self, plan: &CleanupPlan) {
        let removed: HashSet<LayerId> =
            plan.empty_layers.iter().chain(&plan.empty_folders).copied().collect();
        for id in &removed {
            self.remove(*id);
        }
        // Nothing is left pointing at a removed layer (the plan never
        // removes a layer that is still depended on, but be safe).
        let dangling: Vec<LayerId> = self
            .iter()
            .filter(|l| {
                l.follows.is_some_and(|f| removed.contains(&f))
                    || l.parent.is_some_and(|p| removed.contains(&p))
            })
            .map(|l| l.id)
            .collect();
        for id in dangling {
            self.update(id, |l| {
                if l.follows.is_some_and(|f| removed.contains(&f)) {
                    l.follows = None;
                    l.follows_bone = None;
                }
                if l.parent.is_some_and(|p| removed.contains(&p)) {
                    l.parent = None;
                }
            });
        }
        for (id, len) in &plan.trims {
            self.update(*id, |l| {
                if *len >= l.length() {
                    return;
                }
                let keys: Vec<_> = l
                    .frames
                    .keyframes()
                    .iter()
                    .filter(|k| k.start < *len)
                    .cloned()
                    .collect();
                l.frames = LayerTimeline::from_parts(keys, *len);
            });
        }
    }
}

/// Whose stack a plan is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Stage,
    Symbol(SymbolId),
}

impl Scene {
    /// The layers lights sit above on the stage: deleting one would move the
    /// light's slot, so they are never cleaned up.
    fn light_anchors(&self) -> HashSet<LayerId> {
        self.lights.lights.iter().filter_map(|l| l.above).map(LayerId).collect()
    }

    fn cleanup_plans(&self, scope: CleanupScope, opts: CleanupOptions) -> Vec<(Target, CleanupPlan)> {
        let stage_plan = || (Target::Stage, self.layers.cleanup_plan_with(opts, &self.light_anchors()));
        let symbol_plan = |id: SymbolId| {
            self.library
                .get(id)
                .map(|s| (Target::Symbol(id), s.layers.cleanup_plan_with(opts, &HashSet::new())))
        };
        let mut plans = Vec::new();
        match scope {
            CleanupScope::Timeline => {
                match self.editing_symbol().filter(|id| self.library.get(*id).is_some()) {
                    Some(id) => plans.extend(symbol_plan(id)),
                    None => plans.push(stage_plan()),
                }
            }
            CleanupScope::WholeScene => {
                plans.push(stage_plan());
                let ids: Vec<SymbolId> = self.library.iter().map(|s| s.id).collect();
                plans.extend(ids.into_iter().filter_map(symbol_plan));
            }
        }
        plans.retain(|(_, p)| !p.is_empty());
        plans
    }

    fn stack_of(&self, target: Target) -> Option<&LayerStack> {
        match target {
            Target::Stage => Some(&self.layers),
            Target::Symbol(id) => self.library.get(id).map(|s| &s.layers),
        }
    }

    fn report_for(&self, plans: &[(Target, CleanupPlan)]) -> CleanupReport {
        let mut report = CleanupReport::default();
        for (target, plan) in plans {
            let Some(stack) = self.stack_of(*target) else { continue };
            report.layers_deleted += plan.empty_layers.len();
            report.folders_removed += plan.empty_folders.len();
            report.frames_trimmed += plan.frames_trimmed(stack);
            if matches!(target, Target::Symbol(_)) {
                report.symbols_touched += 1;
            }
        }
        report
    }

    /// What [`Self::cleanup_layers`] would do, without changing anything.
    pub fn cleanup_preview(&self, scope: CleanupScope, opts: CleanupOptions) -> CleanupReport {
        self.report_for(&self.cleanup_plans(scope, opts))
    }

    /// **Clean Up Layers** over `scope`. Bumps the revision only when
    /// something changed.
    pub fn cleanup_layers(&mut self, scope: CleanupScope, opts: CleanupOptions) -> CleanupReport {
        let plans = self.cleanup_plans(scope, opts);
        let report = self.report_for(&plans);
        for (target, plan) in &plans {
            match target {
                Target::Stage => self.edit_stage_layers().apply_cleanup(plan),
                Target::Symbol(id) => {
                    if let Some(stack) = self.library_mut().layers_mut(*id) {
                        stack.apply_cleanup(plan);
                    }
                }
            }
        }
        report
    }
}
