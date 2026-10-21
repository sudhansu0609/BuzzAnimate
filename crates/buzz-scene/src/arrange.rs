//! **Arrange Layers** (L3 of `docs/LAYER_MANAGEMENT_PLAN.md`): file the
//! top-level layers of a timeline into folders by what they hold — one per
//! character, Background, FX, Props, and Audio pinned to the bottom —
//! without ever changing what is drawn.
//!
//! The one rule that keeps the picture identical: a layer may move past
//! another only if their content bounds (union over every frame, through the
//! scene's memoised resolved-bounds table, tweens and layer-parenting motion
//! included) don't intersect. A layer that can't reach its group stays where
//! it is and is counted. Mask and guide runs and existing folders move only
//! as whole blocks (and never into a group); a light's anchor layer is never
//! moved or passed. Runs to a fixpoint, and reuses folders by name, so a
//! second run changes nothing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use buzz_geom::Rect;

use crate::cleanup::CleanupOptions;
use crate::layer::{Layer, LayerId, LayerKind, LayerStack};
use crate::object::{ObjectKind, transform_rect};
use crate::symbol::Symbol;
use crate::{BoundsTable, Scene};

/// How Arrange runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrangeOptions {
    /// The active layer: its new folder is left open, and it is never deleted
    /// by the empty-layer pass.
    pub active: Option<LayerId>,
    /// Run Clean Up's "delete empty layers" on the same timeline first.
    pub delete_empty: bool,
}

impl Default for ArrangeOptions {
    fn default() -> Self {
        Self {
            active: None,
            delete_empty: true,
        }
    }
}

/// What [`Scene::arrange_layers`] did (or [`Scene::arrange_preview`] would do).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArrangeReport {
    pub folders_made: usize,
    /// Layers filed into a folder (or moved with the Audio folder).
    pub layers_moved: usize,
    /// Classified layers that stayed outside their folder because moving
    /// them would have changed the drawing order of overlapping artwork.
    pub layers_left_in_place: usize,
    /// Empty layers deleted by the cleanup pass.
    pub emptied: usize,
}

impl ArrangeReport {
    /// Did the run change the timeline?
    pub fn changed(&self) -> bool {
        self.folders_made > 0 || self.layers_moved > 0 || self.emptied > 0
    }
}

/// Which folder a layer belongs in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LayerGroup {
    /// A character, by name (its `Cast/<name>` folder, or the symbol's name).
    Character(String),
    Background,
    Fx,
    Props,
    Audio,
}

impl LayerGroup {
    /// The folder's name.
    pub fn folder_name(&self) -> String {
        match self {
            Self::Character(name) => name.clone(),
            Self::Background => "Background".into(),
            Self::Fx => "FX".into(),
            Self::Props => "Props".into(),
            Self::Audio => "Audio".into(),
        }
    }
}

/// Where a layer (or block) can paint, over its whole span.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Extent {
    /// Paints nothing (sound only, folder rows).
    Empty,
    Area(Rect),
    /// Can't be bounded (filters, cast shadows, a guide run).
    Everywhere,
}

/// Bounds closer than this count as touching: antialiasing bleeds.
const MARGIN: f64 = 2.0;

impl Extent {
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Everywhere, _) | (_, Self::Everywhere) => Self::Everywhere,
            (Self::Empty, x) | (x, Self::Empty) => x,
            (Self::Area(a), Self::Area(b)) => Self::Area(a.union(b)),
        }
    }

    fn add(self, r: Rect) -> Self {
        self.union(Self::Area(r))
    }

    fn meets(self, other: Self) -> bool {
        match (self, other) {
            (Self::Empty, _) | (_, Self::Empty) => false,
            (Self::Everywhere, _) | (_, Self::Everywhere) => true,
            (Self::Area(a), Self::Area(b)) => {
                a.x0 - MARGIN < b.x1
                    && b.x0 - MARGIN < a.x1
                    && a.y0 - MARGIN < b.y1
                    && b.y0 - MARGIN < a.y1
            }
        }
    }
}

struct Ctx<'a> {
    scene: &'a Scene,
    table: Arc<BoundsTable>,
    /// Layers a light sits above (stage only): never moved, never passed.
    anchors: HashSet<LayerId>,
    /// A lit stage with a shadow-casting light: shadow casters are unbounded.
    shadows: bool,
    stage: Rect,
}

/// A run of rows that moves as one: a top-level layer, a mask or guide with
/// its run, or a folder with everything filed under it.
struct Unit {
    rows: Vec<Arc<Layer>>,
    extent: Extent,
    group: Option<LayerGroup>,
    barrier: bool,
    /// Lower-cased name, when the head row is a folder.
    folder: Option<String>,
}

fn name_matches(name: &str, words: &[&str]) -> bool {
    name.to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|t| !t.is_empty() && words.iter().any(|w| t.starts_with(w)))
}

const BACKGROUND_WORDS: &[&str] = &["bg", "background", "sky", "backdrop", "floor", "ground"];
const FX_WORDS: &[&str] = &["fx", "effect", "light", "glow", "shadow", "particle"];

/// The character a symbol stands for: filed under `Cast/<name>/…` → `name`;
/// under `Cast`, or tagged `character` → the symbol's own name.
fn character_name(symbol: &Symbol) -> Option<String> {
    let own = || symbol.name.rsplit('/').next().unwrap_or(&symbol.name).to_string();
    if let Some(folder) = symbol.folder.as_deref() {
        if folder == "Cast" {
            return Some(own());
        }
        if let Some(rest) = folder.strip_prefix("Cast/") {
            let first = rest.split('/').next().unwrap_or("");
            return Some(if first.is_empty() { own() } else { first.to_string() });
        }
    }
    symbol
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("character"))
        .then(own)
}

/// A rig part: a symbol filed in the library's `Cast parts` folder.
fn is_cast_part(symbol: &Symbol) -> bool {
    symbol
        .folder
        .as_deref()
        .is_some_and(|f| f == "Cast parts" || f.starts_with("Cast parts/"))
}

/// The character a set of rig-part names share: the common prefix of the
/// names before " part" ("LALTEN part 11" → "LALTEN").
fn part_stem(names: &[String]) -> Option<String> {
    let stems: Vec<&str> = names
        .iter()
        .filter_map(|n| n.find(" part").map(|i| &n[..i]))
        .collect();
    let first = *stems.first()?;
    let mut len = first.len();
    for s in &stems[1..] {
        len = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(len);
    }
    let stem = first[..len].trim_end_matches(|c: char| c.is_whitespace() || c == '_' || c == '-');
    (!stem.is_empty()).then(|| stem.to_string())
}

impl Ctx<'_> {
    fn layer_extent(&self, stack: &LayerStack, layer: &Layer) -> Extent {
        if !layer.kind.holds_artwork() || layer.all_objects().next().is_none() {
            return Extent::Empty;
        }
        if !layer.filters.is_empty()
            || (self.shadows && layer.casts_shadow && !layer.unlit)
            || layer.all_objects().any(|o| !o.filters.is_empty())
        {
            return Extent::Everywhere;
        }
        let follows = layer.follows.is_some();
        let mut out = Extent::Empty;
        if !follows {
            for o in layer.all_objects() {
                out = out.add(self.scene.resolved_bounds_with(o, &self.table));
            }
        }
        // Frames where what's drawn can differ from a stored keyframe: inside
        // tweens, and (for a follower) every keyframe and tween up the chain.
        let mut chain: Vec<&Layer> = vec![layer];
        if follows {
            let mut next = layer.follows;
            for _ in 0..stack.len() {
                let Some(id) = next else { break };
                let Some(l) = stack.get(id) else { break };
                if chain.iter().any(|c| c.id == l.id) {
                    break;
                }
                chain.push(l);
                next = l.follows;
            }
        }
        let len = layer.length();
        let mut frames: BTreeSet<u32> = BTreeSet::new();
        for (n, l) in chain.iter().enumerate() {
            let keys = l.frames.keyframes();
            for (i, k) in keys.iter().enumerate() {
                if follows {
                    frames.insert(k.start);
                }
                if k.tween.is_active()
                    && let Some(next) = keys.get(i + 1)
                    && (n == 0 || follows)
                {
                    frames.extend((k.start + 1..next.start).take_while(|f| *f < len));
                }
                if frames.len() > 50_000 {
                    return Extent::Everywhere;
                }
            }
        }
        for f in frames.into_iter().filter(|f| *f < len) {
            let resolved = stack.resolved_at_rigged(layer.id, f);
            let mut here: Option<Rect> = None;
            for o in resolved.iter() {
                let r = self.scene.resolved_bounds_with(o, &self.table);
                here = Some(here.map_or(r, |h| h.union(r)));
            }
            if let Some(mut r) = here {
                if follows {
                    r = transform_rect(stack.inherited_transform(layer.id, f), r);
                }
                out = out.add(r);
            }
        }
        out
    }

    /// Group by content, ignoring layer parenting.
    fn classify(&self, stack: &LayerStack, layer: &Layer) -> Option<LayerGroup> {
        let has_objects = layer.all_objects().next().is_some();
        if !has_objects {
            let has_sound = layer.frames.keyframes().iter().any(|k| k.sound.is_some());
            return has_sound.then_some(LayerGroup::Audio);
        }
        for o in layer.all_objects() {
            if let Some(i) = o.instance()
                && let Some(sym) = self.scene.library.get(i.symbol)
                && let Some(name) = character_name(sym)
            {
                return Some(LayerGroup::Character(name));
            }
        }
        if name_matches(&layer.name, BACKGROUND_WORDS) || self.covering_bitmap(stack, layer) {
            return Some(LayerGroup::Background);
        }
        if name_matches(&layer.name, FX_WORDS) {
            return Some(LayerGroup::Fx);
        }
        Some(LayerGroup::Props)
    }

    /// Only bitmaps, covering at least 60% of the stage.
    fn covering_bitmap(&self, _stack: &LayerStack, layer: &Layer) -> bool {
        let all_bitmaps = layer.all_objects().all(|o| match &o.kind {
            ObjectKind::Shape(s) => s.fill.as_ref().is_some_and(|f| f.paint.is_image()),
            _ => false,
        });
        if !all_bitmaps {
            return false;
        }
        let stage_area = self.stage.area();
        if stage_area <= 0.0 {
            return false;
        }
        let Some(b) = layer
            .all_objects()
            .map(|o| self.scene.resolved_bounds_with(o, &self.table))
            .reduce(|a, b| a.union(b))
        else {
            return false;
        };
        let covered = b.intersect(self.stage);
        covered.width() > 0.0 && covered.height() > 0.0 && covered.area() >= 0.6 * stage_area
    }
}

/// Split the stack into units, front to back, and classify the movable ones.
fn units_of(ctx: &Ctx, stack: &LayerStack) -> Vec<Unit> {
    let mut groups: Vec<Vec<Arc<Layer>>> = Vec::new();
    for row in stack.iter() {
        let joins = row.parent.is_some() || matches!(row.kind, LayerKind::Masked | LayerKind::Guided);
        match groups.last_mut() {
            Some(g) if joins => g.push(Arc::clone(row)),
            _ => groups.push(vec![Arc::clone(row)]),
        }
    }
    // Basic class of every single, plain, top-level layer.
    let mut class: HashMap<LayerId, LayerGroup> = HashMap::new();
    for rows in &groups {
        let head = &rows[0];
        if rows.len() == 1
            && head.parent.is_none()
            && head.kind == LayerKind::Normal
            && !ctx.anchors.contains(&head.id)
            && let Some(g) = ctx.classify(stack, head)
        {
            class.insert(head.id, g);
        }
    }
    // A layer-parenting rig stays together: the whole tree (root and every
    // transitive follower) takes its first character (the root's own first);
    // else, when any of its layers holds a `Cast parts` symbol, it is one
    // character named by the parts ("LALTEN part 11" → "LALTEN"), or by the
    // root layer; else the root's class.
    let roots = stack.follow_roots();
    if !roots.is_empty() {
        let mut trees: HashMap<LayerId, Vec<LayerId>> = HashMap::new();
        for l in stack.iter() {
            if let Some(r) = roots.get(&l.id) {
                trees.entry(*r).or_default().push(l.id);
            }
        }
        for (root, members) in trees {
            let chain: Vec<LayerId> = std::iter::once(root).chain(members.iter().copied()).collect();
            let character = chain.iter().find_map(|id| match class.get(id) {
                Some(g @ LayerGroup::Character(_)) => Some(g.clone()),
                _ => None,
            });
            let character = character.or_else(|| {
                let mut parts: Vec<String> = Vec::new();
                for l in chain.iter().filter_map(|id| stack.get(*id)) {
                    for o in l.all_objects() {
                        if let Some(i) = o.instance()
                            && let Some(sym) = ctx.scene.library.get(i.symbol)
                            && is_cast_part(sym)
                        {
                            parts.push(sym.name.rsplit('/').next().unwrap_or(&sym.name).to_string());
                        }
                    }
                }
                if parts.is_empty() {
                    return None;
                }
                let name = part_stem(&parts)
                    .or_else(|| stack.get(root).map(|l| l.name.clone()))
                    .unwrap_or_default();
                Some(LayerGroup::Character(name))
            });
            let tree_class = character
                .or_else(|| class.get(&root).cloned())
                .or_else(|| members.iter().find_map(|id| class.get(id).cloned()));
            if let Some(g) = tree_class {
                for id in chain {
                    if let Some(c) = class.get_mut(&id) {
                        *c = g.clone();
                    }
                }
            }
        }
    }
    groups
        .into_iter()
        .map(|rows| {
            let head = Arc::clone(&rows[0]);
            let barrier = rows.iter().any(|r| ctx.anchors.contains(&r.id));
            let extent = if head.kind == LayerKind::Guide && rows.len() > 1 {
                Extent::Everywhere
            } else {
                rows.iter()
                    .fold(Extent::Empty, |e, r| e.union(ctx.layer_extent(stack, r)))
            };
            let folder = (head.kind == LayerKind::Folder && head.parent.is_none())
                .then(|| head.name.to_lowercase());
            Unit {
                group: if rows.len() == 1 { class.get(&head.id).cloned() } else { None },
                rows,
                extent,
                barrier,
                folder,
            }
        })
        .collect()
}

fn can_pass(mover: &Unit, passed: &Unit) -> bool {
    if mover.extent == Extent::Empty {
        return true;
    }
    !passed.barrier && !mover.extent.meets(passed.extent)
}

/// Members that can join a block anchored at `anchor`: (from above, from below).
fn plan_group(units: &[Unit], group: &LayerGroup, anchor: usize) -> (Vec<usize>, Vec<usize>) {
    let members: Vec<usize> = (0..units.len())
        .filter(|&i| i != anchor && units[i].group.as_ref() == Some(group))
        .collect();
    let mut below: Vec<usize> = Vec::new();
    for &m in members.iter().filter(|&&m| m > anchor) {
        let ok = (anchor + 1..m)
            .filter(|j| !below.contains(j))
            .all(|j| can_pass(&units[m], &units[j]));
        if ok {
            below.push(m);
        }
    }
    let mut above: Vec<usize> = Vec::new();
    for &m in members.iter().rev().filter(|&&m| m < anchor) {
        let ok = (m + 1..anchor)
            .filter(|j| !above.contains(j))
            .all(|j| can_pass(&units[m], &units[j]));
        if ok {
            above.push(m);
        }
    }
    above.sort_unstable();
    (above, below)
}

fn filed_under(unit: &Unit, folder: LayerId) -> impl Iterator<Item = Arc<Layer>> + '_ {
    unit.rows.iter().map(move |r| {
        if r.parent.is_none() {
            let mut l = (**r).clone();
            l.parent = Some(folder);
            Arc::new(l)
        } else {
            Arc::clone(r)
        }
    })
}

/// Arrange one stack. `None` when nothing would change.
fn arrange_stack(
    ctx: &Ctx,
    stack: &LayerStack,
    active: Option<LayerId>,
    new_id: &mut dyn FnMut() -> LayerId,
    report: &mut ArrangeReport,
) -> Option<LayerStack> {
    let mut units = units_of(ctx, stack);
    // New folders span the whole timeline, like every other layer.
    let span = stack.frame_count();
    let mut changed = false;
    // Bounded: every productive round files at least one unit or makes a folder.
    for _ in 0..=units.len() + 8 {
        let mut round = false;
        let mut order: Vec<LayerGroup> = Vec::new();
        for u in &units {
            if let Some(g) = &u.group
                && !order.contains(g)
            {
                order.push(g.clone());
            }
        }
        if let Some(i) = order.iter().position(|g| *g == LayerGroup::Audio) {
            let audio = order.remove(i);
            order.push(audio);
        }
        for group in order {
            let wanted = group.folder_name().to_lowercase();
            let existing = units.iter().position(|u| u.folder.as_deref() == Some(wanted.as_str()));
            let (anchor, above, below) = match existing {
                Some(a) => {
                    let (ab, be) = plan_group(&units, &group, a);
                    (a, ab, be)
                }
                None => {
                    let mut best: Option<(usize, Vec<usize>, Vec<usize>)> = None;
                    for a in (0..units.len()).filter(|&i| units[i].group.as_ref() == Some(&group)) {
                        let (ab, be) = plan_group(&units, &group, a);
                        if best.as_ref().is_none_or(|(_, x, y)| ab.len() + be.len() > x.len() + y.len()) {
                            best = Some((a, ab, be));
                        }
                    }
                    let Some(b) = best else { continue };
                    b
                }
            };
            if existing.is_some() && above.is_empty() && below.is_empty() {
                continue;
            }
            // Build the block.
            let mut rows: Vec<Arc<Layer>> = Vec::new();
            let folder_id;
            if existing.is_some() {
                let head = Arc::clone(&units[anchor].rows[0]);
                folder_id = head.id;
                rows.push(head);
                for &i in &above {
                    rows.extend(filed_under(&units[i], folder_id));
                }
                rows.extend(units[anchor].rows[1..].iter().cloned());
                for &i in &below {
                    rows.extend(filed_under(&units[i], folder_id));
                }
            } else {
                folder_id = new_id();
                let mut members: Vec<Arc<Layer>> = Vec::new();
                for &i in above.iter().chain(std::iter::once(&anchor)).chain(below.iter()) {
                    members.extend(filed_under(&units[i], folder_id));
                }
                let mut folder = Layer::new(folder_id, group.folder_name(), LayerKind::Folder);
                folder.collapsed = !members.iter().any(|r| Some(r.id) == active);
                if span > 1 {
                    folder.frames.insert_frame(span - 1);
                }
                rows.push(Arc::new(folder));
                rows.extend(members);
                report.folders_made += 1;
                report.layers_moved += units[anchor].rows.iter().filter(|r| r.parent.is_none()).count();
            }
            for &i in above.iter().chain(below.iter()) {
                report.layers_moved += units[i].rows.iter().filter(|r| r.parent.is_none()).count();
            }
            let taken: HashSet<usize> = above.iter().chain(below.iter()).copied().chain([anchor]).collect();
            let extent = taken.iter().fold(Extent::Empty, |e, &i| e.union(units[i].extent));
            let barrier = taken.iter().any(|&i| units[i].barrier);
            let at = anchor - above.len();
            let block = Unit {
                rows,
                extent,
                group: None,
                barrier,
                folder: Some(wanted.clone()),
            };
            let mut next: Vec<Unit> = Vec::with_capacity(units.len());
            for (i, u) in units.into_iter().enumerate() {
                if !taken.contains(&i) {
                    next.push(u);
                }
            }
            next.insert(at, block);
            units = next;
            round = true;
        }
        // Audio is pinned to the bottom when it paints nothing.
        if let Some(i) = units.iter().position(|u| u.folder.as_deref() == Some("audio"))
            && i + 1 < units.len()
            && units[i].extent == Extent::Empty
        {
            let block = units.remove(i);
            report.layers_moved += block.rows.iter().filter(|r| r.kind != LayerKind::Folder).count();
            units.push(block);
            round = true;
        }
        if !round {
            break;
        }
        changed = true;
    }
    report.layers_left_in_place = units.iter().filter(|u| u.group.is_some()).count();
    changed.then(|| LayerStack::from_rows(units.into_iter().flat_map(|u| u.rows).collect()))
}

impl Scene {
    /// What [`Self::arrange_layers`] would do, without changing anything.
    pub fn arrange_preview(&self, opts: ArrangeOptions) -> ArrangeReport {
        let mut copy = self.clone();
        copy.arrange_layers(opts)
    }

    /// **Arrange Layers** on the timeline being edited. Bumps the revision
    /// only when something changed.
    pub fn arrange_layers(&mut self, opts: ArrangeOptions) -> ArrangeReport {
        let symbol = self.editing_symbol().filter(|id| self.library.get(*id).is_some());
        let mut stack = match symbol.and_then(|id| self.library.get(id)) {
            Some(s) => s.layers.clone(),
            None => self.layers.clone(),
        };
        let anchors: HashSet<LayerId> = if symbol.is_none() {
            self.lights.lights.iter().filter_map(|l| l.above).map(LayerId).collect()
        } else {
            HashSet::new()
        };
        let mut report = ArrangeReport::default();
        if opts.delete_empty {
            let mut protected = anchors.clone();
            protected.extend(opts.active);
            let plan = stack.cleanup_plan_with(
                CleanupOptions {
                    empty_layers: true,
                    trim: false,
                    folders: false,
                },
                &protected,
            );
            if !plan.empty_layers.is_empty() {
                report.emptied = plan.empty_layers.len();
                stack.apply_cleanup(&plan);
            }
        }
        let shadows = symbol.is_none()
            && self.lights.enabled
            && self.lights.lights.iter().any(|l| l.enabled && l.shadows);
        let mut ids = self.ids;
        let arranged = {
            let ctx = Ctx {
                scene: self,
                table: self.cached_bounds_table(),
                anchors,
                shadows,
                stage: self.stage.stage_rect(),
            };
            let mut new_id = || LayerId(ids.take());
            arrange_stack(&ctx, &stack, opts.active, &mut new_id, &mut report)
        };
        self.ids = ids;
        let moved = arranged.is_some();
        if let Some(s) = arranged {
            stack = s;
        }
        if moved || report.emptied > 0 {
            match symbol {
                Some(id) => {
                    if let Some(s) = self.library_mut().layers_mut(id) {
                        *s = stack;
                    }
                }
                None => *self.edit_stage_layers() = stack,
            }
        }
        report
    }
}
