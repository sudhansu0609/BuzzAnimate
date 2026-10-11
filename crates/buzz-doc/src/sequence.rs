//! Reusable setups and the shots stamped from them.
//!
//! See `docs/SEQUENCE_SETUPS_PLAN.md` ("Design", "Setups are stamped, never
//! linked") for the rule these types encode: a shot stamped from a setup is
//! an independent copy from the moment it exists. Nothing here creates a
//! live link back to the setup — `from_setup` and `setup_revision` are kept
//! only so the UI can show where a shot came from and whether its setup has
//! since changed (offering *Add Missing Setup Layers*), never to propagate
//! an edit into a shot that already exists.

use buzz_scene::LayerId;
use serde::{Deserialize, Serialize};

/// Stable identity for a scene, independent of its position in the scene
/// list.
///
/// Scenes are reordered, duplicated and deleted, and a shot's `from_setup`,
/// `take_of` and `continue_from`, along with the document's `sequence`, all
/// need to keep pointing at the same scene through that — a plain index would
/// silently start pointing at whatever scene now sits there instead.
///
/// Allocated from a per-document counter (see `Document::allocate_scene_key`).
/// A file saved before this existed has no keys at all; one is made up for
/// each scene from its position on load, which is why `Document::open` on an
/// old file hands out `SceneKey(0)`, `SceneKey(1)`, … in file order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SceneKey(pub u32);

/// What kind of scene this is. See the table in
/// `docs/SEQUENCE_SETUPS_PLAN.md` "Design".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum SceneRole {
    /// Today's scene: nothing linked, no rules. Plays in the exported film,
    /// in scene order, exactly as it always has.
    #[default]
    Normal,
    /// A reusable arrangement — background, props, lights, camera framing,
    /// cast placed. A source to stamp shots from; never itself in the
    /// exported film.
    Setup,
    /// One cut of the film: a stamped copy of a setup, sliced to a stretch of
    /// the master audio. Plays in `Document::sequence` order.
    Shot(ShotInfo),
}

/// What a [`SceneRole::Shot`] remembers about where it came from and which
/// stretch of the master audio it plays.
///
/// Every field here is display/bookkeeping only. The shot's layers are its
/// own from the moment `Document::stamp_shot` makes them — nothing reads
/// `from_setup` to reach back into the setup and pull anything across, except
/// the one explicit, user-triggered exception:
/// `Document::add_missing_setup_layers`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ShotInfo {
    /// The setup this shot was stamped from, for display only (the Sequence
    /// strip's colour and label). `None` once that setup has been deleted,
    /// or for a shot that never had one.
    pub from_setup: Option<SceneKey>,
    /// The setup's own revision at the moment of stamping, so the UI can
    /// tell whether the setup has changed since and offer *Add Missing Setup
    /// Layers* — never to re-stamp or replay anything automatically.
    pub setup_revision: u64,
    /// Frame on the master audio this shot's frame 0 plays.
    pub audio_in: u32,
    /// Frame on the master audio this shot ends at. The shot's own length is
    /// `audio_out - audio_in`.
    pub audio_out: u32,
    /// The shot this is an alternative take of, if it is one. Set by
    /// `Document::duplicate_shot`; a fresh take is never itself placed in
    /// `Document::sequence` until `Document::set_active_take` puts it there
    /// in place of another key.
    pub take_of: Option<SceneKey>,
    /// Seed this shot's local layers from another shot's last pose at frame
    /// 0 — for a cut that picks up where an earlier shot left off. Not yet
    /// acted on; recorded here for the stage that reads it.
    pub continue_from: Option<SceneKey>,
    /// The shot's own **Dialogue** layer, if one has been made — the master
    /// audio's clip, copied into this shot's own `SoundLibrary` and trimmed
    /// to play `audio_in..audio_out`. See "The audio is the timeline" in
    /// `docs/SEQUENCE_SETUPS_PLAN.md`.
    ///
    /// The one thing `Document` ever rewrites on an existing shot, and only
    /// when this shot's own `audio_in`/`audio_out` change
    /// (`Document::set_shot_range`, `Document::move_cut`) — every other
    /// layer is the animator's, untouched. `#[serde(default)]` so a file
    /// saved before this field existed opens with every shot's Dialogue
    /// layer simply unmade yet, rather than failing to load.
    #[serde(default)]
    pub dialogue_layer: Option<LayerId>,
}

/// **Scene indices, in film order** — the rule the exported/previewed film
/// follows, from the raw parallel `roles`/`keys` a document (or a freshly
/// loaded `format::LoadedScenes`) carries, plus its `sequence`. See
/// `docs/SEQUENCE_SETUPS_PLAN.md` S6.
///
/// - A `Setup` never plays: dropped.
/// - A `Shot` not named in `sequence` is a take not in the cut: dropped.
/// - Every `Shot` that IS named in `sequence` plays, together, in
///   `sequence`'s order — inserted as one run at the single position where
///   the first of them (by document order) sits among the `Normal` scenes.
/// - `Normal` scenes otherwise keep their own relative order untouched, so a
///   normal scene before the sequence and one after it stay exactly there.
///
/// A document with only `Normal` scenes (no shots in `sequence` at all)
/// comes back exactly as `(0..roles.len())` — today's plain scene order.
pub fn film_order(roles: &[SceneRole], keys: &[SceneKey], sequence: &[SceneKey]) -> Vec<usize> {
    let normal_indices: Vec<usize> = (0..roles.len())
        .filter(|&i| matches!(roles.get(i), Some(SceneRole::Normal)))
        .collect();

    let is_active_shot = |i: usize| -> bool {
        matches!(roles.get(i), Some(SceneRole::Shot(_)))
            && keys.get(i).is_some_and(|k| sequence.contains(k))
    };
    let first_shot = (0..roles.len()).filter(|&i| is_active_shot(i)).min();

    let Some(first_shot) = first_shot else {
        return normal_indices;
    };

    let insert_at = normal_indices.iter().filter(|&&i| i < first_shot).count();
    let sequence_indices: Vec<usize> = sequence
        .iter()
        .filter_map(|key| keys.iter().position(|k| k == key))
        .collect();

    let mut order = Vec::with_capacity(normal_indices.len() + sequence_indices.len());
    order.extend_from_slice(&normal_indices[..insert_at]);
    order.extend_from_slice(&sequence_indices);
    order.extend_from_slice(&normal_indices[insert_at..]);
    order
}

#[cfg(test)]
mod film_order_tests {
    use super::*;

    #[test]
    fn only_normal_scenes_keep_plain_scene_order() {
        let roles = vec![SceneRole::Normal, SceneRole::Normal, SceneRole::Normal];
        let keys = vec![SceneKey(0), SceneKey(1), SceneKey(2)];
        assert_eq!(film_order(&roles, &keys, &[]), vec![0, 1, 2]);
    }

    /// Setups are dropped, and the shots in `sequence` land together, in
    /// `sequence`'s own order, at the position their first (by document
    /// order) sits among the Normal scenes.
    #[test]
    fn setups_are_dropped_and_shots_play_in_sequence_order() {
        // 0: Normal A, 1: Setup, 2: Shot X, 3: Shot Y, 4: Normal B,
        // 5: Shot Z (an inactive take, never in `sequence`).
        let roles = vec![
            SceneRole::Normal,
            SceneRole::Setup,
            SceneRole::Shot(ShotInfo::default()),
            SceneRole::Shot(ShotInfo::default()),
            SceneRole::Normal,
            SceneRole::Shot(ShotInfo::default()),
        ];
        let keys: Vec<SceneKey> = (0..6).map(|i| SceneKey(i as u32)).collect();
        // Cut order: Y before X.
        let sequence = vec![SceneKey(3), SceneKey(2)];

        assert_eq!(
            film_order(&roles, &keys, &sequence),
            vec![0, 3, 2, 4],
            "A, then Y, then X (sequence order), then B; the setup and the inactive take never appear"
        );
    }
}
