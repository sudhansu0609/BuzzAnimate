//! **Stage pins** (AN4c): what the creator moved or posed on the BuzzAnimate
//! stage survives a rebuild.
//!
//! At build time [`written_for`] records, for every character instance, a
//! small hash of its placement and its pose parts (in `produce.json`). Before
//! the next build [`find_pins`] reads the existing `.buzz` and compares: an
//! instance whose placement or pose differs from what the last build wrote is
//! the creator's, becomes a [`Pin`] (kept in `pins.json`), and [`apply_pins`]
//! writes it into the casting as `"pinned": true` with its position, so the
//! rebuild puts the character back where he left it.
//!
//! A moved or resized character is replayed through its mark (`x`, `y`). Joint
//! edits of the pose are detected and the character is pinned where it
//! stands, but the edited joints themselves are not replayed: save a posed rig
//! as a pose picture in the character's folder for that.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use buzz_act::layer_rig::{self, CharacterRig};
use buzz_doc::Document;
use buzz_scene::{ObjectId, Scene};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A placement the creator made on the stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub scene: u32,
    /// The character's key, upper-case.
    pub name: String,
    /// Feet across the stage, `0..1`.
    pub x: f64,
    /// Feet down the stage, `0..1`; `None` keeps the scene's ground line.
    pub y: Option<f64>,
    /// The pose parts differed too.
    pub posed: bool,
}

/// One character instance as a build wrote it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub scene: u32,
    pub name: String,
    /// The body object (`ObjectId`) in the built scene.
    pub body: u64,
    /// Hash of the placement and the pose parts.
    pub hash: String,
    /// The placement affine's coefficients.
    pub placement: [f64; 6],
    /// Hash of the pose parts alone.
    pub pose: String,
    /// The casting mark the build used.
    pub x: f64,
    pub y: f64,
}

fn hex(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The pose parts of a rig, as text: each part's transform to 0.01.
fn pose_text(scene: &Scene, body: ObjectId) -> String {
    let mut out = String::new();
    let mut add = |id: ObjectId| {
        if let Some((_, o)) = scene.find_object(id) {
            let c = o.transform.as_coeffs();
            out.push_str(&format!("{:.2},{:.2},{:.2},{:.2},{:.2},{:.2};", c[0], c[1], c[2], c[3], c[4], c[5]));
        }
    };
    match layer_rig::rig_of(scene, body) {
        Some(CharacterRig::Layers(view)) => {
            for &(_, _, part) in &view.parts {
                add(part);
            }
        }
        _ => add(body),
    }
    out
}

/// The instances of a built document: for each `(scene number, name, body,
/// x, y)` its hash, placement and pose hash. Scenes are found by their name,
/// `Scene <n> — <title>`; an instance whose body is gone is left out.
pub fn written_for(doc: &Document, bodies: &[(u32, String, u64, f64, f64)]) -> Vec<Written> {
    let names = doc.scene_names();
    let film = doc.film();
    let mut out = Vec::new();
    for (number, name, body, x, y) in bodies {
        let Some(index) = names.iter().position(|n| {
            n.strip_prefix("Scene ").and_then(|r| r.split_whitespace().next()).and_then(|t| t.parse::<u32>().ok()) == Some(*number)
        }) else {
            continue;
        };
        let Some(scene) = film.get(index) else { continue };
        let id = ObjectId(*body);
        let Some(placement) = layer_rig::placement(scene, id) else { continue };
        let c = placement.as_coeffs();
        let pose = hex(&pose_text(scene, id));
        let hash = hex(&format!("{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}|{pose}", c[0] * 100.0, c[1] * 100.0, c[2] * 100.0, c[3] * 100.0, c[4], c[5]));
        out.push(Written { scene: *number, name: name.clone(), body: *body, hash, placement: c, pose, x: *x, y: *y });
    }
    out
}

/// **The pins**: every instance of `last` whose `now` differs (moved by more
/// than half a pixel, scaled, or posed differently). Pure, so it needs no
/// document. `stage` is the stage's width and height in pixels.
pub fn diff_pins(last: &[Written], now: &[Written], stage: (f64, f64)) -> Vec<Pin> {
    let mut pins = Vec::new();
    for w in last {
        let Some(n) = now.iter().find(|n| n.scene == w.scene && n.name == w.name) else { continue };
        if n.hash == w.hash {
            continue;
        }
        let (dx, dy) = (n.placement[4] - w.placement[4], n.placement[5] - w.placement[5]);
        let scaled = (n.placement[0] - w.placement[0]).abs() > 0.005 || (n.placement[3] - w.placement[3]).abs() > 0.005;
        let moved = dx.abs() > 0.5 || dy.abs() > 0.5 || scaled;
        let posed = n.pose != w.pose;
        if !moved && !posed {
            continue;
        }
        let x = (w.x + if stage.0 > 0.0 { dx / stage.0 } else { 0.0 }).clamp(0.0, 1.0);
        let y = (dy.abs() > 0.5 && stage.1 > 0.0).then(|| (w.y + dy / stage.1).clamp(0.0, 1.0));
        pins.push(Pin { scene: w.scene, name: w.name.clone(), x, y, posed });
    }
    pins
}

/// [`diff_pins`] of the saved document against what the last build wrote.
pub fn find_pins(doc: &Document, last: &[Written], stage: (f64, f64)) -> Vec<Pin> {
    let bodies: Vec<(u32, String, u64, f64, f64)> =
        last.iter().map(|w| (w.scene, w.name.clone(), w.body, w.x, w.y)).collect();
    diff_pins(last, &written_for(doc, &bodies), stage)
}

/// New pins over old ones: the newest of each `(scene, name)` stays.
pub fn merge_pins(old: Vec<Pin>, new: Vec<Pin>) -> Vec<Pin> {
    let mut out: Vec<Pin> = old.into_iter().filter(|o| !new.iter().any(|n| n.scene == o.scene && n.name == o.name)).collect();
    out.extend(new);
    out
}

/// Writes the pins into a casting: `scenes.<n>.place.<NAME>` gets the pin's
/// `x` (and `y`) and `"pinned": true`, over whatever the breakdown chose.
pub fn apply_pins(casting: &mut Value, pins: &[Pin]) {
    for p in pins {
        let scene = &mut casting["scenes"][p.scene.to_string()];
        if !scene.is_object() {
            continue;
        }
        if !scene["place"].is_object() {
            scene["place"] = json!({});
        }
        let spec = &mut scene["place"][p.name.as_str()];
        if !spec.is_object() {
            *spec = json!({});
        }
        spec["x"] = json!((p.x * 1000.0).round() / 1000.0);
        if let Some(y) = p.y {
            spec["y"] = json!((y * 1000.0).round() / 1000.0);
        }
        spec["pinned"] = json!(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(x: f64, tx: f64, pose: &str) -> Written {
        Written {
            scene: 1,
            name: "GUDDU".into(),
            body: 7,
            hash: hex(&format!("{tx}|{pose}")),
            placement: [1.0, 0.0, 0.0, 1.0, tx, 500.0],
            pose: hex(pose),
            x,
            y: 0.9,
        }
    }

    #[test]
    fn a_pinned_instance_survives_a_rebuild() {
        let last = vec![w(0.3, 576.0, "rest")];
        // He dragged Guddu 192 px right on a 1920 px stage and left Raju alone.
        let now = vec![w(0.3, 768.0, "rest")];
        let pins = diff_pins(&last, &now, (1920.0, 1080.0));
        assert_eq!(pins.len(), 1);
        assert!((pins[0].x - 0.4).abs() < 1e-9 && pins[0].y.is_none());
        // Nothing moved: no pin.
        assert!(diff_pins(&last, &last, (1920.0, 1080.0)).is_empty());
        // The rebuilt casting (the breakdown's own marks) keeps the pin.
        let mut casting = json!({"scenes": {"1": {"place": {"GUDDU": {"x": 0.3}, "RAJU": {"x": 0.7}}}}});
        apply_pins(&mut casting, &pins);
        assert_eq!(casting["scenes"]["1"]["place"]["GUDDU"]["x"], json!(0.4));
        assert_eq!(casting["scenes"]["1"]["place"]["GUDDU"]["pinned"], json!(true));
        assert_eq!(casting["scenes"]["1"]["place"]["RAJU"]["x"], json!(0.7));
    }
}
