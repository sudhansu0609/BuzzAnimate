//! **A head's own mouth and eyelid charts** (SIMPLE_EPISODE_FLOW_PLAN E4b).
//!
//! The way Animate animators build a head: the head symbol holds a layer named
//! `Mouth` (an instance of the mouth chart, one frame per shape) and one named
//! `Eyelid` (the lid chart: open, half shut, shut), and they are animated by
//! keying the chart instance's frame on those layers, inside the head. A
//! character on the stage reaches them through the rig's head part.
//!
//! [`fork_face`] gives one placed character its own copy of its head (and what
//! it holds) and plays that copy on the stage's frame
//! ([`buzz_scene::SymbolInstance::sync_to_root`]), so keys on the inner layers
//! are keys at the scene's frames and no other character, scene or Cast asset
//! sees them. [`put_chart_keys`] writes them, [`chart_track`] reads them back.

use std::sync::Arc;

use buzz_audio::Viseme;
use buzz_scene::{EditOrigin, LayerId, LayerTimeline, LoopMode, ObjectId, ObjectKind, Scene, SymbolId};

use crate::layer_rig::LayerRigView;
use crate::pose::{is_lid_layer_name, lid_chart_frames};

/// One chart layer inside a head: the symbol that holds it and its layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceSpot {
    pub symbol: SymbolId,
    pub layer: LayerId,
}

/// The nested face of a layer rig's head part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    /// The head part's layer, object and symbol.
    pub head_layer: LayerId,
    pub head: ObjectId,
    pub head_symbol: SymbolId,
    /// The `Mouth` layer (a chart with a frame for every shape).
    pub mouth: Option<FaceSpot>,
    /// The `Eyelid` layer, with its chart's `[open, half, shut]` frames.
    pub lid: Option<(FaceSpot, [u32; 3])>,
}

/// Is `name` a head's mouth layer ("Mouth", any case)?
pub fn is_mouth_layer_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("mouth")
}

/// The layer in `symbol` (or in a symbol it shows, up to `depth` deep) whose name
/// says `is_wanted`, holding an instance of a chart `chart_ok` accepts.
fn find_layer(scene: &Scene, symbol: SymbolId, is_wanted: &dyn Fn(&str) -> bool, chart_ok: &dyn Fn(SymbolId) -> bool, depth: u32) -> Option<FaceSpot> {
    let s = scene.library().get(symbol)?;
    for l in s.layers.iter() {
        if !is_wanted(&l.name) {
            continue;
        }
        let chart = l.frames.keyframes().iter().find_map(|k| k.objects.first()).and_then(|o| o.instance()).map(|i| i.symbol);
        if chart.is_some_and(chart_ok) {
            return Some(FaceSpot { symbol, layer: l.id });
        }
    }
    if depth == 0 {
        return None;
    }
    for l in s.layers.iter() {
        for k in l.frames.keyframes().iter().take(1) {
            for o in k.objects.iter() {
                if let Some(i) = o.instance()
                    && i.symbol != symbol
                    && let Some(found) = find_layer(scene, i.symbol, is_wanted, chart_ok, depth - 1)
                {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn length_of(scene: &Scene, symbol: SymbolId) -> u32 {
    scene.library().get(symbol).map_or(0, |s| s.length())
}

fn chart_of(scene: &Scene, spot: FaceSpot) -> Option<SymbolId> {
    let layer = scene.library().get(spot.symbol)?.layers.get(spot.layer)?;
    layer.frames.keyframes().iter().find_map(|k| k.objects.first())?.instance().map(|i| i.symbol)
}

/// **The nested face of a layer rig**: the first part that is an instance of a
/// head symbol holding a `Mouth` and/or an `Eyelid` layer (a mouth chart of at
/// least [`Viseme::COUNT`] frames, a lid chart that [`lid_chart_frames`] can read).
pub fn face_of(scene: &Scene, rig: &LayerRigView) -> Option<Face> {
    for &(_, layer, id) in &rig.parts {
        let Some((_, object)) = scene.find_object(id) else { continue };
        let Some(instance) = object.instance() else { continue };
        let head_symbol = instance.symbol;
        let mouth = find_layer(scene, head_symbol, &is_mouth_layer_name, &|c| length_of(scene, c) >= Viseme::COUNT, 3);
        let lid = find_layer(scene, head_symbol, &is_lid_layer_name, &|c| lid_chart_frames(scene, c).is_some(), 3)
            .and_then(|spot| Some((spot, lid_chart_frames(scene, chart_of(scene, spot)?)?)));
        if mouth.is_some() || lid.is_some() {
            return Some(Face { head_layer: layer, head: id, head_symbol, mouth, lid });
        }
    }
    None
}

/// **Give the placed character its own head, playing on the stage's frame.**
/// The head part's symbol and everything it shows are copied
/// ([`Scene::fork_instance_subtree`]), the part now shows the copy, looping from
/// frame 0 locked to the stage frame. Idempotent: a head that already plays on
/// the stage frame is left alone. Returns the face, found again in the copy.
pub fn fork_face(scene: &mut Scene, rig: &LayerRigView) -> Option<Face> {
    let face = face_of(scene, rig)?;
    let synced = scene.find_object(face.head).and_then(|(_, o)| o.instance().map(|i| i.sync_to_root)).unwrap_or(false);
    if !synced {
        let start = scene
            .layers()
            .get(face.head_layer)?
            .frames
            .keyframes()
            .iter()
            .find(|k| k.objects.iter().any(|o| o.id == face.head))?
            .start;
        let report = scene.fork_instance_subtree(EditOrigin { layer: face.head_layer, keyframe_start: start, object: face.head })?;
        // The copy plays on the stage's frame, so whatever the FLA's long face timelines do
        // by themselves (pupils darting, a lid blinking) would play too: every layer of the
        // copies is held on its first key (the Mouth and Eyelid layers are keyed by the
        // writers, and the lid starts open).
        // Nothing inside plays by itself on the stage's frame either: every looping instance of a
        // short chart (2 to 99 frames) in the copies shows the frame it starts on, held; the long
        // face timelines that hold the Mouth and Eyelid layers keep following the stage's frame.
        let lengths: std::collections::HashMap<SymbolId, u32> = report.copied.iter().map(|(_, c)| (*c, length_of(scene, *c))).chain(scene.library().iter().map(|s| (s.id, s.length()))).collect();
        for (_, copy) in &report.copied {
            let Some(stack) = scene.library_mut().layers_mut(*copy) else { continue };
            let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
            for id in ids {
                stack.update(id, |l| {
                    for key in l.frames.keyframes_mut() {
                        for object in Arc::make_mut(&mut key.objects).iter_mut() {
                            if let ObjectKind::Instance(i) = &Arc::make_mut(object).kind
                                && i.loop_mode != LoopMode::SingleFrame
                                && (2..100).contains(&lengths.get(&i.symbol).copied().unwrap_or(1))
                                && let ObjectKind::Instance(i) = &mut Arc::make_mut(object).kind
                            {
                                i.loop_mode = LoopMode::SingleFrame;
                            }
                        }
                    }
                });
            }
        }
        for (_, copy) in &report.copied {
            // (Charts - mouths, lids, pupils, whites: a few frames - keep every frame.)
            if length_of(scene, *copy) < 100 {
                continue;
            }
            let Some(stack) = scene.library_mut().layers_mut(*copy) else { continue };
            let ids: Vec<LayerId> = stack.iter().filter(|l| l.frames.keyframe_count() > 1).map(|l| l.id).collect();
            for id in ids {
                stack.update(id, |l| {
                    let first: Vec<_> = l.frames.keyframes().iter().take(1).cloned().collect();
                    let length = l.frames.length();
                    l.frames = LayerTimeline::from_parts(first, length);
                });
            }
        }
        scene.update_object_across(0, u32::MAX, face.head, |o| {
            if let ObjectKind::Instance(i) = &mut o.kind {
                i.sync_to_root = true;
                i.loop_mode = LoopMode::Loop;
                i.first_frame = 0;
            }
        });
    }
    let face = face_of(scene, rig)?;
    if let Some(chart) = mouth_chart(scene, &face) {
        // This character's own chart carries its own viseme map.
        store_viseme_map(scene, chart);
    }
    if let Some((spot, frames)) = face.lid {
        put_chart_keys(scene, &face, spot, &[(0, frames[0])]);
    }
    if let Some(spot) = face.mouth {
        // The mouth rests closed until a lip sync says otherwise.
        let rest = mouth_chart(scene, &face).map_or(Viseme::Rest.frame(), |c| viseme_map_of(scene, c)[Viseme::Rest.frame() as usize]);
        put_chart_keys(scene, &face, spot, &[(0, rest)]);
    }
    Some(face)
}

/// Make every layer of `symbol` at least `n` frames long, holding its last key.
fn ensure_length(scene: &mut Scene, symbol: SymbolId, n: u32) {
    let Some(stack) = scene.library_mut().layers_mut(symbol) else { return };
    let ids: Vec<LayerId> = stack.iter().filter(|l| l.frames.length() < n).map(|l| l.id).collect();
    for id in ids {
        stack.update(id, |l| {
            let keys = l.frames.keyframes().to_vec();
            l.frames = LayerTimeline::from_parts(keys, n);
        });
    }
}

/// **Key a chart layer**: at each `(frame, chart frame)` the layer's instance
/// shows that frame of its chart, held from that key on. The head, and the
/// symbol holding the layer, are lengthened to cover the last frame.
pub fn put_chart_keys(scene: &mut Scene, face: &Face, spot: FaceSpot, keys: &[(u32, u32)]) {
    let Some(&last) = keys.iter().map(|(f, _)| f).max() else { return };
    ensure_length(scene, face.head_symbol, last + 2);
    if spot.symbol != face.head_symbol {
        ensure_length(scene, spot.symbol, last + 2);
    }
    let Some(stack) = scene.library_mut().layers_mut(spot.symbol) else { return };
    stack.update(spot.layer, |l| {
        let Some(template) = l.frames.keyframes().iter().find_map(|k| k.objects.first()).cloned() else { return };
        for &(frame, chart) in keys {
            l.frames.insert_keyframe(frame);
            let mut object = (*template).clone();
            if let ObjectKind::Instance(i) = &mut object.kind {
                i.first_frame = chart;
                i.loop_mode = LoopMode::SingleFrame;
            }
            l.frames.set_objects(frame, vec![Arc::new(object)]);
            l.frames.set_tween(frame, buzz_scene::Tween::default());
        }
    });
}

/// Remove the keys of a chart layer in `from..until` (never frame 0), so keys written again replace them.
pub fn clear_chart_keys(scene: &mut Scene, spot: FaceSpot, from: u32, until: u32) {
    let Some(stack) = scene.library_mut().layers_mut(spot.symbol) else { return };
    stack.update(spot.layer, |l| {
        let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).filter(|s| *s > 0 && *s >= from && *s < until).collect();
        for s in starts {
            l.frames.clear_keyframe(s);
        }
    });
}

/// The chart frame shown by a chart layer at each change over `from..until`:
/// `(frame, chart frame)`, starting with the frame shown at `from`.
pub fn chart_track(scene: &Scene, spot: FaceSpot, from: u32, until: u32) -> Vec<(u32, u32)> {
    let Some(layer) = scene.library().get(spot.symbol).and_then(|s| s.layers.get(spot.layer)) else { return Vec::new() };
    let mut out: Vec<(u32, u32)> = Vec::new();
    for f in from..until {
        let Some(shown) = layer.frames.objects_at(f).first().and_then(|o| o.instance().map(|i| i.first_frame)) else { continue };
        if out.last().is_none_or(|(_, s)| *s != shown) {
            out.push((f, shown));
        }
    }
    out
}

// -- the chart's own viseme map ---------------------------------------------------------------

/// The frame of a mouth chart that shows each viseme, indexed by [`Viseme::frame`] (Rest, Ai, E, O, U,
/// L, WQ, MBP, FV, Etc). Every chart has its own: charts are drawn in any order.
pub type VisemeMap = [u32; Viseme::COUNT as usize];

/// The tag a mouth chart symbol carries once its map is stored on it (`visemes:0,3,5,...`).
const MAP_TAG: &str = "visemes:";

/// What each frame of a chart draws: `(width, height)` of everything shown on it, `None` for a blank frame.
fn frame_sizes(scene: &Scene, chart: SymbolId) -> Vec<Option<(f64, f64)>> {
    let Some(symbol) = scene.library().get(chart) else { return Vec::new() };
    (0..symbol.length().min(32))
        .map(|f| {
            let mut union: Option<buzz_geom::Rect> = None;
            for l in symbol.layers.iter() {
                for o in l.objects_at(f).iter() {
                    let r = scene.resolved_bounds(o);
                    if r.width() > 0.0 || r.height() > 0.0 {
                        union = Some(union.map_or(r, |u| u.union(r)));
                    }
                }
            }
            union.map(|r| (r.width(), r.height()))
        })
        .collect()
}

/// **Which frame of this chart shows each viseme**, read from the drawings themselves: Rest is the most
/// closed mouth drawn (the flattest), MBP the next flattest, Ai the widest open one, and the rest by how
/// open, wide and round they are (E spread, O round, U small and round, WQ the narrowest, FV low and
/// wide, L and Etc between). Each viseme takes a frame of its own while the chart has enough of them.
pub fn analyse_mouth_chart(scene: &Scene, chart: SymbolId) -> VisemeMap {
    let sizes = frame_sizes(scene, chart);
    let drawn: Vec<(u32, f64, f64)> = sizes.iter().enumerate().filter_map(|(f, s)| s.map(|(w, h)| (f as u32, w, h))).collect();
    let mut map: VisemeMap = std::array::from_fn(|i| i as u32);
    if drawn.is_empty() {
        return map;
    }
    let max_h = drawn.iter().map(|d| d.2).fold(f64::MIN, f64::max).max(1e-6);
    let max_w = drawn.iter().map(|d| d.1).fold(f64::MIN, f64::max).max(1e-6);
    let mut used: Vec<u32> = Vec::new();
    // `score` is minimised over the frames not used yet (all of them once every drawn frame is used).
    let mut pick = |wanted: Viseme, score: &dyn Fn(f64, f64) -> f64| {
        let free: Vec<&(u32, f64, f64)> = drawn.iter().filter(|d| !used.contains(&d.0)).collect();
        let pool: Vec<&(u32, f64, f64)> = if free.is_empty() { drawn.iter().collect() } else { free };
        if let Some(best) = pool.iter().min_by(|a, b| score(a.1 / max_w, a.2 / max_h).total_cmp(&score(b.1 / max_w, b.2 / max_h)).then(a.0.cmp(&b.0))) {
            map[wanted.frame() as usize] = best.0;
            used.push(best.0);
        }
    };
    // Flat first (then the wider one: a closed mouth is a wide line), tall for Ai.
    pick(Viseme::Rest, &|w, h| h * 10.0 - w);
    pick(Viseme::Ai, &|w, h| -h * 10.0 - w * 0.1);
    pick(Viseme::MBP, &|w, h| h * 10.0 - w);
    pick(Viseme::E, &|w, h| if h < 0.25 { 5.0 } else { -(w / h.max(0.05)) });
    pick(Viseme::O, &|w, h| if h < 0.3 { 5.0 } else { (w / h.max(0.05) - 1.0).abs() });
    pick(Viseme::U, &|w, h| if h < 0.15 { 5.0 } else { w });
    pick(Viseme::L, &|_, h| (h - 0.6).abs());
    pick(Viseme::FV, &|w, h| (h - 0.3).abs() - w * 0.3);
    pick(Viseme::WQ, &|w, _| w);
    pick(Viseme::Etc, &|_, h| (h - 0.45).abs());
    map
}

/// The chart's map: the one stored on it, else read from its drawings ([`analyse_mouth_chart`]).
pub fn viseme_map_of(scene: &Scene, chart: SymbolId) -> VisemeMap {
    stored_viseme_map(scene, chart).unwrap_or_else(|| analyse_mouth_chart(scene, chart))
}

/// **The animator's own mapping** of Adobe Animate's twelve visemes
/// ([`buzz_audio::ANIMATE_VISEMES`]) to the chart's frames, 0-based — what
/// Animate's Lip Syncing dialog stores on a mouth symbol, set here in the Lip
/// Sync window's Set Visemes. Stored on the chart symbol as a tag.
pub type AnimateChart = [u32; 12];

/// The tag an Animate-style viseme mapping is stored under (`animate-visemes:3,0,6,...`).
const ANIMATE_TAG: &str = "animate-visemes:";

/// The chart's Animate-style mapping, when one was set on it.
pub fn animate_chart_of(scene: &Scene, chart: SymbolId) -> Option<AnimateChart> {
    scene.library().get(chart).and_then(|s| {
        s.tags.iter().find_map(|t| t.strip_prefix(ANIMATE_TAG)).and_then(|list| {
            let v: Vec<u32> = list.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            <AnimateChart>::try_from(v).ok()
        })
    })
}

/// The map a stored mapping gives: the Animate-style one when set, else the
/// older per-shape one; `None` when the chart carries neither.
pub fn stored_viseme_map(scene: &Scene, chart: SymbolId) -> Option<VisemeMap> {
    if let Some(animate) = animate_chart_of(scene, chart) {
        return Some(map_from_animate(&animate));
    }
    scene.library().get(chart).and_then(|s| {
        s.tags.iter().find_map(|t| t.strip_prefix(MAP_TAG)).and_then(|list| {
            let v: Vec<u32> = list.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            <VisemeMap>::try_from(v).ok()
        })
    })
}

const SHAPES: [Viseme; 10] = [Viseme::Rest, Viseme::Ai, Viseme::E, Viseme::O, Viseme::U, Viseme::L, Viseme::WQ, Viseme::MBP, Viseme::FV, Viseme::Etc];

/// Each shape's chart frame, from an Animate-style mapping.
pub fn map_from_animate(chart: &AnimateChart) -> VisemeMap {
    let mut map: VisemeMap = std::array::from_fn(|i| i as u32);
    for shape in SHAPES {
        map[shape.frame() as usize] = chart[shape.animate_slot()];
    }
    map
}

/// The Animate-style mapping a per-shape map amounts to (D, R and S all on
/// the consonant frame, L and Uh on the neutral one).
pub fn animate_from_map(map: &VisemeMap) -> AnimateChart {
    let shape_of_slot = [
        Viseme::Rest, Viseme::Ai, Viseme::Etc, Viseme::E, Viseme::FV, Viseme::L,
        Viseme::MBP, Viseme::O, Viseme::Etc, Viseme::Etc, Viseme::L, Viseme::U,
    ];
    std::array::from_fn(|slot| map[shape_of_slot[slot].frame() as usize])
}

/// **Set the chart's visemes**, Animate's way: the mapping is stored on the
/// chart symbol and every lip sync onto it uses it from then on.
pub fn set_animate_chart(scene: &mut Scene, chart: SymbolId, mapping: AnimateChart) {
    let list = mapping.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let derived = map_from_animate(&mapping).iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    scene.library_mut().update(chart, |s| {
        s.tags.retain(|t| !t.starts_with(ANIMATE_TAG) && !t.starts_with(MAP_TAG));
        s.tags.push(format!("{ANIMATE_TAG}{list}"));
        s.tags.push(format!("{MAP_TAG}{derived}"));
    });
}

/// Store the chart's map on the chart symbol (a tag), unless it already has one. Used on the copy a
/// placed character owns, so the Cast asset's chart needs no reordering and is not touched.
pub fn store_viseme_map(scene: &mut Scene, chart: SymbolId) {
    if scene.library().get(chart).is_none_or(|s| s.tags.iter().any(|t| t.starts_with(MAP_TAG))) {
        return;
    }
    let map = analyse_mouth_chart(scene, chart);
    let tag = format!("{MAP_TAG}{}", map.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    scene.library_mut().update(chart, |s| s.tags.push(tag));
}

/// The mouth chart a face's `Mouth` layer shows.
pub fn mouth_chart(scene: &Scene, face: &Face) -> Option<SymbolId> {
    chart_of(scene, face.mouth?)
}

/// The chart frames that show each shape of a track's runs: `(frame, chart frame)`.
pub fn mouth_keys(scene: &Scene, face: &Face, runs: &[(u32, Viseme, u32)]) -> Vec<(u32, u32)> {
    let map = mouth_chart(scene, face).map(|c| viseme_map_of(scene, c));
    runs.iter().map(|(first, shape, _)| (*first, map.map_or(shape.frame(), |m| m[shape.frame() as usize]))).collect()
}

/// The viseme a chart frame shows in `map` (the first one mapped to it; Rest for a frame none shows).
pub fn viseme_of_frame(map: &VisemeMap, frame: u32) -> Viseme {
    const ALL: [Viseme; 10] = [Viseme::Rest, Viseme::Ai, Viseme::E, Viseme::O, Viseme::U, Viseme::L, Viseme::WQ, Viseme::MBP, Viseme::FV, Viseme::Etc];
    ALL.iter().copied().find(|v| map[v.frame() as usize] == frame).unwrap_or(Viseme::Rest)
}

/// The blink state a lid chart frame shows (0 open, 1 half, 2 shut), by the chart's frames.
pub fn lid_state_of(frames: [u32; 3], shown: u32) -> usize {
    if shown == frames[2] {
        2
    } else if shown == frames[1] {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer_rig::rig_of_layer;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{Layer, LayerKind, Object, ShapeData, SymbolKind};

    /// A chart symbol whose frame `f` draws a bar `heights[f]` tall.
    fn chart(scene: &mut Scene, name: &str, heights: &[f64]) -> SymbolId {
        let id = scene.add_symbol(name, SymbolKind::Graphic, None);
        let layer = scene.library().get(id).unwrap().layers.iter().next().unwrap().id;
        let objects: Vec<_> = heights
            .iter()
            .map(|h| Arc::new(Object::shape(scene.next_object_id(), ShapeData::filled(Rect::new(0.0, 0.0, 20.0, h.max(0.5)).to_path(1e-9), peniko::Color::WHITE))))
            .collect();
        scene.library_mut().update(id, |s| {
            s.layers.update(layer, |l| {
                for (f, o) in objects.into_iter().enumerate() {
                    l.frames.insert_keyframe(f as u32);
                    l.frames.set_objects(f as u32, vec![o]);
                }
            });
        });
        id
    }

    /// A head with a `Mouth` layer (10 frames) and an `Eyelid` layer (7: open, half, shut, half...), on a rig.
    fn rig(scene: &mut Scene) -> (LayerRigView, SymbolId, SymbolId, SymbolId) {
        let mouth = chart(scene, "mouths", &[1.0, 8.0, 6.0, 10.0, 7.0, 5.0, 4.0, 2.0, 3.0, 6.0]);
        let lid = chart(scene, "lids", &[0.5, 5.0, 8.0, 15.0, 8.0, 5.0, 0.5]);
        let head = scene.add_symbol("Head sym", SymbolKind::Graphic, None);
        let first = scene.library().get(head).unwrap().layers.iter().next().unwrap().id;
        let (m_id, l_id) = (LayerId(scene.next_object_id().0), LayerId(scene.next_object_id().0));
        let (mo, lo) = (Object::instance_of(scene.next_object_id(), mouth), Object::instance_of(scene.next_object_id(), lid));
        scene.library_mut().update(head, |s| {
            s.layers.update(first, |l| l.name = "base".into());
            for (id, name, o) in [(m_id, "mouth", mo), (l_id, "eyeLid", lo)] {
                let mut layer = Layer::new(id, name, LayerKind::Normal);
                layer.push_object_at(0, Arc::new(o));
                s.layers.push_front(layer);
            }
        });
        let hips = scene.add_layer("Hips", LayerKind::Normal);
        let head_layer = scene.add_layer("Head", LayerKind::Normal);
        scene.add_instance_at(head_layer, 0, head, buzz_geom::Affine::IDENTITY);
        scene.add_instance_at(hips, 0, mouth, buzz_geom::Affine::IDENTITY);
        scene.set_follows(head_layer, Some(hips), 0);
        (rig_of_layer(scene, head_layer).expect("a rig"), head, mouth, lid)
    }

    #[test]
    fn a_heads_mouth_and_eyelid_layers_are_found_and_the_lid_chart_is_read() {
        let mut scene = Scene::default();
        let (view, head, _, _) = rig(&mut scene);
        let face = face_of(&scene, &view).expect("a face");
        assert_eq!(face.head_symbol, head);
        assert!(face.mouth.is_some(), "the Mouth layer (any case) is found");
        let (_, frames) = face.lid.expect("the Eyelid layer is found");
        assert_eq!(frames, [0, 2, 3], "open, half (second tallest), shut (tallest)");
    }

    #[test]
    fn a_placed_heads_keys_are_its_own_and_play_on_the_stage_frame() {
        let mut scene = Scene::default();
        let (view, head, _, _) = rig(&mut scene);
        let face = fork_face(&mut scene, &view).expect("forked");
        assert_ne!(face.head_symbol, head, "the character has its own head");
        let on_stage = scene.find_object(face.head).map(|(_, o)| o.instance().cloned()).flatten().expect("the head part");
        assert!(on_stage.sync_to_root && on_stage.loop_mode == LoopMode::Loop);
        let (mouth, (lid, frames)) = (face.mouth.unwrap(), face.lid.unwrap());
        put_chart_keys(&mut scene, &face, mouth, &[(4, 1), (8, 3)]);
        put_chart_keys(&mut scene, &face, lid, &[(6, frames[1]), (8, frames[2]), (10, frames[0])]);
        assert_eq!(chart_track(&scene, mouth, 0, 12), vec![(0, 0), (4, 1), (8, 3)]);
        assert_eq!(chart_track(&scene, lid, 0, 12).iter().map(|(f, _)| *f).collect::<Vec<_>>(), vec![0, 6, 8, 10]);
        // The Cast asset's own head (the symbol the forked one was copied from) is untouched.
        let original = FaceSpot { symbol: head, layer: scene.library().get(head).unwrap().layers.iter().find(|l| l.name == "mouth").unwrap().id };
        assert_eq!(chart_track(&scene, original, 0, 12), vec![(0, 0)]);
        // Forking again does nothing: the head already plays on the stage.
        assert_eq!(fork_face(&mut scene, &view).map(|f| f.head_symbol), Some(face.head_symbol));
    }

    #[test]
    fn a_chart_drawn_in_its_own_order_gets_its_own_viseme_map_with_rest_the_most_closed_mouth() {
        let mut scene = Scene::default();
        // Frame 0 is an open mouth; the flattest drawings are 5 and 10.
        let chart = chart(&mut scene, "odd mouths", &[9.0, 6.0, 3.0, 8.0, 5.0, 0.5, 7.0, 4.0, 2.0, 10.0, 1.0]);
        let map = analyse_mouth_chart(&scene, chart);
        assert_eq!(map[Viseme::Rest.frame() as usize], 5, "Rest = the most closed mouth drawn");
        assert_eq!(map[Viseme::MBP.frame() as usize], 10, "MBP = the next flattest");
        assert_eq!(map[Viseme::Ai.frame() as usize], 9, "Ai = the widest open");
        let mut seen = map.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 10, "every shape has a frame of its own: {map:?}");
        assert_eq!(viseme_of_frame(&map, 5), Viseme::Rest);
        // Stored on the chart, then read back from there.
        store_viseme_map(&mut scene, chart);
        assert_eq!(viseme_map_of(&scene, chart), map);
        assert!(scene.library().get(chart).unwrap().tags.iter().any(|t| t.starts_with("visemes:")));
    }

    #[test]
    fn a_placed_heads_mouth_rests_on_its_closed_frame_and_lip_sync_keys_use_the_charts_map() {
        let mut scene = Scene::default();
        let (view, _, mouth_chart_id, _) = rig(&mut scene);
        let face = fork_face(&mut scene, &view).expect("forked");
        let mouth = face.mouth.unwrap();
        // The chart in the placed copy carries a map.
        let chart = super::mouth_chart(&scene, &face).unwrap();
        assert!(scene.library().get(chart).unwrap().tags.iter().any(|t| t.starts_with("visemes:")));
        assert_eq!(chart_track(&scene, mouth, 0, 4), vec![(0, viseme_map_of(&scene, chart)[0])]);
        let keys = mouth_keys(&scene, &face, &[(0, Viseme::Rest, 4), (4, Viseme::Ai, 4)]);
        let map = viseme_map_of(&scene, chart);
        assert_eq!(keys, vec![(0, map[0]), (4, map[Viseme::Ai.frame() as usize])]);
        let _ = mouth_chart_id;
    }
}
