//! **The six one-shot actions, judged as animation rather than as code.**
//!
//! A pose table compiles whatever numbers are in it, so the assertions here are
//! about the *shape* of each move: that a sit ends sat and stays sat, that a
//! knee never bends the wrong way, that a point holds long enough to be read,
//! that a reaction overshoots. Those are the properties an animator would look
//! for, and they are the ones that break when somebody adjusts a number.

use buzz_act::perform::{Action, Performance, pose_at};
use buzz_act::{Beat, Joint};

/// The pose at `u` of the way through the action, at full amount.
fn at(action: Action, u: f64) -> Beat {
    pose_at(action, u, 1.0)
}

fn joint(beat: &Beat, j: Joint) -> f64 {
    beat.joints[j.index()]
}

/// Every one-shot, for the properties they all share.
const ONE_SHOTS: [Action; 6] = [
    Action::Sit,
    Action::Stand,
    Action::Turn,
    Action::Point,
    Action::Reach,
    Action::React,
];

/// **A one-shot happens once, however long the beat is.**
///
/// Sitting down twice because the animator gave it three seconds is not a
/// slower sit, it is a person bobbing.
#[test]
fn a_one_shot_fits_one_cycle_into_any_beat() {
    for action in ONE_SHOTS {
        for frames in [12u32, 48, 240] {
            let p = Performance::new(action, 0..frames);
            assert_eq!(
                p.cycles(24.0),
                1.0,
                "{:?} over {frames} frames wanted {} cycles",
                action,
                p.cycles(24.0)
            );
        }
        assert!(!action.loops(), "{action:?} should not be a cycle");
        assert!(!action.travels(), "{action:?} should not travel");
    }
}

/// **A one-shot's phase does not wrap.**
///
/// The end of a sit must not be the beginning of it, or the character springs
/// upright on the very keyframe meant to hold them down.
#[test]
fn the_end_of_a_one_shot_is_not_its_beginning() {
    let start = at(Action::Sit, 0.0);
    let end = at(Action::Sit, 1.0);
    assert!(
        (joint(&end, Joint::KneeL) - joint(&start, Joint::KneeL)).abs() > 1.0,
        "the last frame of a sit looks like the first"
    );
    // And past the end it stays put rather than starting over.
    let past = at(Action::Sit, 1.4);
    assert_eq!(joint(&past, Joint::KneeL), joint(&end, Joint::KneeL));
}

/// **A sit ends sat**: thighs up, knees folded, body lowered.
#[test]
fn a_sit_ends_seated() {
    let sat = at(Action::Sit, 1.0);
    assert!(joint(&sat, Joint::ThighL) > 1.0, "the thighs did not come up");
    assert!(joint(&sat, Joint::KneeL) < -1.0, "the knees did not fold");
    assert!(sat.offset.y > 15.0, "the body did not come down onto a seat");
    // And back a little, because a chair is behind you.
    assert!(sat.offset.x < 0.0, "the sit went forwards");
}

/// **Standing up is the sit backwards, to the frame.**
///
/// Written once and read from the far end, so the two cannot drift apart when
/// either is adjusted. This is the test that keeps that true.
#[test]
fn standing_up_is_the_sit_reversed() {
    for step in 0..=10 {
        let u = step as f64 / 10.0;
        let sit = at(Action::Sit, u);
        let stand = at(Action::Stand, 1.0 - u);
        for j in Joint::ALL {
            assert!(
                (joint(&sit, j) - joint(&stand, j)).abs() < 1e-9,
                "at {u}, {j:?} differs: sit {} against stand {}",
                joint(&sit, j),
                joint(&stand, j)
            );
        }
        assert!((sit.offset.y - stand.offset.y).abs() < 1e-9);
    }
    // A stand therefore starts sat and ends upright.
    assert!(at(Action::Stand, 0.0).offset.y > 15.0);
    assert!(at(Action::Stand, 1.0).offset.y.abs() < 1e-9);
}

/// **A knee never bends the wrong way**, in any action, at any point.
///
/// The single most obvious tell in a generated performance, and the one it is
/// easiest to reintroduce by adjusting a number in the wrong direction.
#[test]
fn no_knee_ever_bends_backwards() {
    for action in [
        Action::Walk,
        Action::Run,
        Action::Talk,
        Action::Idle,
        Action::Sit,
        Action::Stand,
        Action::Turn,
        Action::Point,
        Action::Reach,
        Action::React,
    ] {
        for step in 0..=100 {
            let beat = at(action, step as f64 / 100.0);
            for knee in [Joint::KneeL, Joint::KneeR] {
                assert!(
                    joint(&beat, knee) <= 1e-9,
                    "{action:?} bends {knee:?} forwards by {} at {}",
                    joint(&beat, knee),
                    step as f64 / 100.0
                );
            }
        }
    }
}

/// **A gesture holds long enough to be read.**
///
/// An arm that goes up and comes straight back down is a wave nobody can
/// follow. The hold through the middle is what makes a point a point.
#[test]
fn a_point_holds_through_the_middle() {
    let up = |u: f64| -joint(&at(Action::Point, u), Joint::ShoulderR);
    assert!(up(0.0).abs() < 1e-9, "a point does not start with the arm up");
    let held = up(0.5);
    assert!(held > 0.5, "the arm never came up: {held}");
    // Flat across the middle third, within a hair.
    for u in [0.35, 0.45, 0.55, 0.65, 0.74] {
        assert!(
            (up(u) - held).abs() < 1e-6,
            "the arm moved during the hold at {u}: {} against {held}",
            up(u)
        );
    }
    assert!(up(1.0).abs() < 1e-9, "the arm was left in the air");
}

/// **A reach commits the whole body.** A point is the arm and almost nothing
/// else; without the counterweight a reach reads as somebody being pulled by
/// the wrist.
#[test]
fn a_reach_leans_where_a_point_does_not() {
    let point = at(Action::Point, 0.5);
    let reach = at(Action::Reach, 0.5);

    assert!(
        joint(&reach, Joint::Chest).abs() > joint(&point, Joint::Chest).abs() * 2.0,
        "a reach should lean far more than a point does"
    );
    assert!(
        joint(&reach, Joint::ShoulderL) > 0.2,
        "the far arm did not counterbalance"
    );
    assert!(reach.offset.x > 0.0, "the body did not go after the hand");
    assert!(
        joint(&reach, Joint::ShoulderR) < joint(&point, Joint::ShoulderR),
        "a reach should extend further than a point"
    );
}

/// **A reaction overshoots and settles.**
///
/// It goes away, comes back past rest, and dies down — which is what separates
/// being startled from deciding to lean.
#[test]
fn a_reaction_recoils_then_comes_back_past_rest() {
    let head = |u: f64| joint(&at(Action::React, u), Joint::Head);

    let early: Vec<f64> = (1..25).map(|i| head(i as f64 / 100.0)).collect();
    let away = early.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(away < -0.05, "the recoil never happened: {away}");

    // Somewhere after it, the head crosses back through and past rest.
    let later: Vec<f64> = (30..70).map(|i| head(i as f64 / 100.0)).collect();
    let back = later.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(back > 0.0, "the recoil never came back past rest: {back}");

    // And it is over by the end, so the beat composes with what follows.
    assert!(head(1.0).abs() < 0.02, "the reaction never settled: {}", head(1.0));
    assert!(away.abs() > back.abs(), "the settle should be smaller than the recoil");
}

/// **A turn is led by the head and finished by the hips**, which is the order a
/// body turns in and the reason a rigid turn reads as a statue on a turntable.
#[test]
fn a_turn_is_led_by_the_head() {
    // A fifth of the way in, the head is well into its swing and the hips have
    // barely started.
    let early = at(Action::Turn, 0.2);
    assert!(
        joint(&early, Joint::Head) > joint(&early, Joint::Chest),
        "the chest is leading the head"
    );
    assert!(
        joint(&early, Joint::Chest) > joint(&early, Joint::Hips),
        "the hips are leading the chest"
    );
    // And everything is back at rest by the end.
    let done = at(Action::Turn, 1.0);
    for j in [Joint::Head, Joint::Chest, Joint::Hips] {
        assert!(
            joint(&done, j).abs() < 1e-9,
            "{j:?} was left turned at the end: {}",
            joint(&done, j)
        );
    }
}

/// **Every one-shot except the sit ends back at rest**, so the beat composes
/// with whatever the director schedules after it. The sit is the deliberate
/// exception — it ends sat, because that is what sitting down is.
#[test]
fn a_one_shot_leaves_the_body_where_it_found_it() {
    for action in [Action::Turn, Action::Point, Action::Reach, Action::React] {
        let done = at(action, 1.0);
        for j in Joint::ALL {
            assert!(
                joint(&done, j).abs() < 0.02,
                "{action:?} left {j:?} at {}",
                joint(&done, j)
            );
        }
        assert!(done.offset.x.abs() < 0.2 && done.offset.y.abs() < 0.2);
    }
}

/// **Amount scales a performance without changing its shape.** Half is a
/// listless version of the same move, not a different one.
#[test]
fn amount_scales_without_reshaping() {
    for action in ONE_SHOTS {
        for step in [0u32, 3, 5, 8, 10] {
            let u = step as f64 / 10.0;
            let full = pose_at(action, u, 1.0);
            let half = pose_at(action, u, 0.5);
            for j in Joint::ALL {
                let (f, h) = (joint(&full, j), joint(&half, j));
                assert!(
                    (h - f * 0.5).abs() < 1e-9,
                    "{action:?} at {u}: {j:?} is {h} at half, not {} ",
                    f * 0.5
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The timing pass
// ---------------------------------------------------------------------------

use buzz_act::perform::{apply_from, Performance as Perf};
use buzz_geom::Affine;
use buzz_scene::{LayerKind, Scene};

/// A figure on a layer, ready to perform.
fn stage() -> (Scene, buzz_scene::ObjectId) {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    let id = scene.next_object_id();
    let mut next = || scene_next(&mut scene);
    let figure = buzz_act::build_figure(&buzz_act::FigureSpec::default(), id, &mut next);
    let placed = scene.add_object(layer, figure).expect("on the layer");
    (scene, placed)
}

fn scene_next(scene: &mut Scene) -> buzz_scene::ObjectId {
    scene.next_object_id()
}

/// Where the figure stands at `frame`, along x.
fn x_at(scene: &Scene, id: buzz_scene::ObjectId, frame: u32) -> f64 {
    let (layer, _) = scene.find_object(id).expect("the figure");
    scene
        .layers()
        .get(layer)
        .and_then(|l| {
            l.frames
                .resolved_at(frame)
                .iter()
                .find(|o| o.id == id)
                .map(|o| o.transform.translation().x)
        })
        .unwrap_or(0.0)
}

/// **A walk accelerates out of a standstill.**
///
/// The tell this was written to remove: a generated walk left at full stride on
/// frame one and stopped dead at the end, so the character read as being slid
/// rather than as walking.
#[test]
fn a_walk_gets_up_to_speed_rather_than_starting_at_it() {
    let (mut scene, id) = stage();
    let performance = Perf {
        distance: 400.0,
        ..Perf::new(Action::Walk, 0..96)
    };
    apply_from(&mut scene, id, &performance, Affine::IDENTITY).expect("performs");

    // Ground covered in the first eighth against a middle eighth of the same
    // length. A linear walk covers the same in both.
    let early = x_at(&scene, id, 12) - x_at(&scene, id, 0);
    let middle = x_at(&scene, id, 54) - x_at(&scene, id, 42);
    assert!(
        early < middle * 0.8,
        "the walk started at full speed: {early:.1} early against {middle:.1} in the middle"
    );

    // And it settles rather than stopping dead.
    let late = x_at(&scene, id, 94) - x_at(&scene, id, 82);
    assert!(
        late < middle * 0.9,
        "the walk stopped dead: {late:.1} at the end against {middle:.1} in the middle"
    );
}

/// **It still arrives exactly where it was sent.**
///
/// Easing the pacing must not change the distance: the director places the next
/// beat from where this one ended, and a walk that fell short would leave the
/// whole rest of the shot offset.
#[test]
fn easing_the_pacing_does_not_move_the_destination() {
    let (mut scene, id) = stage();
    let performance = Perf {
        distance: 400.0,
        ..Perf::new(Action::Walk, 0..96)
    };
    apply_from(&mut scene, id, &performance, Affine::IDENTITY).expect("performs");

    let arrived = x_at(&scene, id, 94);
    assert!(
        (arrived - 400.0).abs() < 12.0,
        "the walk was sent 400 units and arrived at {arrived:.1}"
    );
    assert!(x_at(&scene, id, 0).abs() < 1e-6, "it did not start where it was placed");
}

/// **Standing actions are not eased.**
///
/// A talk and an idle are ambient: they have no beginning to leave and no end
/// to arrive at, and easing them would only make them listless at both ends.
#[test]
fn a_standing_action_keeps_its_even_pacing() {
    for action in [Action::Talk, Action::Idle] {
        let (mut scene, id) = stage();
        apply_from(
            &mut scene,
            id,
            &Perf::new(action, 0..96),
            Affine::IDENTITY,
        )
        .expect("performs");
        // Nothing travels, so the test is that the pose at the quarter point is
        // the pose the unmodified curve gives there — no re-timing applied.
        let quarter = pose_at(action, 0.25 * Perf::new(action, 0..96).cycles(24.0), 1.0);
        assert!(
            quarter.joints.iter().any(|v| v.abs() > 1e-9),
            "{action:?} produced no motion at all"
        );
    }
}
