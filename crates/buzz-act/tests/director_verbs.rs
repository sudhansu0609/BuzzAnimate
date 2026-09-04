//! **What the director understands, and what it still hands back.**
//!
//! Every sentence a writer types that the parser cannot read is listed in the
//! report for them to rephrase — which is honest, and is also the measure of
//! how much of a brief lands without a person. These tests are that measure:
//! the beats a brief produces, and the sentences it does not.
//!
//! The ordering cases matter most. "Ben stands up" contains the idling stem
//! *stand* and means the opposite of standing about; "Ana walks over and
//! points" is a walk with a point in it. Getting either wrong is not a crash,
//! it is a character doing the wrong thing — which is far harder to notice.

use buzz_act::perform::Action;
use buzz_scene::Scene;

/// Direct a brief and report the actions, in order.
///
/// **The trailing idles are dropped.** Everyone left standing when their part
/// ends idles quietly to the end of the shot — that is deliberate, and it is
/// tested where it belongs; here it is noise, because what is being checked is
/// what the *sentences* produced.
fn actions(story: &str) -> Vec<Action> {
    let mut scene = Scene::default();
    let directed = buzz_act::direct(&mut scene, story).expect("the brief should direct");
    let mut beats: Vec<Action> = directed.beats.iter().map(|b| b.action).collect();
    let asked = directed.beats.len() - trailing_idles(&directed);
    beats.truncate(asked);
    beats
}

/// How many idles at the end were added to keep the cast alive rather than
/// asked for. They are the ones that run to the very end of the shot.
fn trailing_idles(directed: &buzz_act::DirectedScene) -> usize {
    directed
        .beats
        .iter()
        .rev()
        .take_while(|b| b.action == Action::Idle && b.frames.end >= directed.frames)
        .count()
}

/// Direct a brief and report what it could not read.
fn ignored(story: &str) -> Vec<String> {
    let mut scene = Scene::default();
    match buzz_act::direct(&mut scene, story) {
        Ok(d) => d.ignored,
        Err(_) => vec!["<nothing understood>".into()],
    }
}

/// The first beat of a one-line brief, which is how most of these are checked.
fn first(line: &str) -> Option<Action> {
    let mut scene = Scene::default();
    let story = format!("Day. {line}");
    let directed = buzz_act::direct(&mut scene, &story).expect("the brief should direct");
    directed.beats.first().map(|b| b.action)
}

/// **The six new verbs land on the six new actions.**
#[test]
fn the_new_verbs_are_understood() {
    assert_eq!(first("Ana sits down."), Some(Action::Sit));
    assert_eq!(first("Ana stands up."), Some(Action::Stand));
    assert_eq!(first("Ana turns."), Some(Action::Turn));
    assert_eq!(first("Ana points at the door."), Some(Action::Point));
    assert_eq!(first("Ana reaches for the lamp."), Some(Action::Reach));
    assert_eq!(first("Ana flinches."), Some(Action::React));
}

/// **A verb has more than one form**, because a writer will not use the one
/// the parser happens to have been written around.
#[test]
fn the_words_a_writer_would_actually_use() {
    for (line, want) in [
        ("Ana sat down.", Action::Sit),
        ("Ana slumps into the chair.", Action::Sit),
        ("Ana perches there.", Action::Sit),
        ("Ana rises.", Action::Stand),
        ("Ana gets up.", Action::Stand),
        ("Ana spins around.", Action::Turn),
        ("Ana whirls.", Action::Turn),
        ("Ana gestures at the window.", Action::Point),
        ("Ana beckons.", Action::Point),
        ("Ana grabs the lamp.", Action::Reach),
        ("Ana snatches it.", Action::Reach),
        ("Ana picks up the book.", Action::Reach),
        ("Ana recoils.", Action::React),
        ("Ana is startled.", Action::React),
        ("Ana gasps.", Action::React),
    ] {
        assert_eq!(first(line), Some(want), "{line:?}");
    }
}

/// **"Stands up" is not "stands about."**
///
/// The idling vocabulary already contained the stem *stand*, so without the
/// ordering this reads as the opposite of what it says — a character who idles
/// where they should have got to their feet.
#[test]
fn standing_up_beats_standing_about() {
    assert_eq!(first("Ben stands up."), Some(Action::Stand));
    assert_eq!(first("Ben stands."), Some(Action::Idle));
    assert_eq!(first("Ben stands and waits."), Some(Action::Idle));
}

/// **Travelling wins over a gesture in the same sentence.**
///
/// "Ana walks over and points" is a walk with a point in it: the walk is the
/// beat that moves the story, and the gesture is decoration on the end of it.
#[test]
fn going_somewhere_wins_over_a_gesture() {
    assert_eq!(first("Ana walks over and points."), Some(Action::Walk));
    assert_eq!(first("Ana runs and grabs it."), Some(Action::Run));
    // And talking wins too — a line of dialogue is the beat.
    assert_eq!(first("Ana points and says hello."), Some(Action::Talk));
}

/// **A gesture wins over a bare idle**, because it is the more specific thing
/// the sentence actually says.
#[test]
fn a_gesture_beats_an_idle() {
    assert_eq!(first("Ana waits and then points."), Some(Action::Point));
    assert_eq!(first("Ana stops and turns."), Some(Action::Turn));
}

/// **A noun that is also a verb still wins as a verb.**
///
/// "Ana perches on the step" comes out as a *walk*, because `step` is in the
/// travelling vocabulary and the parser has no idea it is a noun here. This is
/// a real limitation of a keyword grammar and it is written down rather than
/// hidden behind a luckier example: the fix is to rephrase, and knowing that
/// is worth more than pretending it does not happen.
///
/// It is deliberately not "fixed" by making gestures beat travelling, because
/// that would break "Ana walks over and points", which is far more common.
#[test]
fn a_noun_that_is_also_a_verb_is_a_known_limitation() {
    assert_eq!(
        first("Ana perches on the step."),
        Some(Action::Walk),
        "if this now reads as a sit, the grammar got cleverer and the note above \
         should be rewritten rather than deleted"
    );
    // Rephrased, it lands.
    assert_eq!(first("Ana perches there."), Some(Action::Sit));
}

/// **Sentences these verbs turn up in without meaning them are not misread.**
///
/// The risk of adding vocabulary is that it starts eating prose. "The sun
/// rises" names nobody, and a sentence about nobody is scenery.
#[test]
fn prose_that_merely_contains_a_verb_is_not_a_beat() {
    // Nobody is named, so there is no actor and nothing is performed: "rises"
    // would be a Stand if it had a subject, and does not get one.
    assert_eq!(
        actions("Day. The sun rises over the hill.\nAna waits."),
        vec![Action::Idle],
        "scenery was performed as a beat"
    );
}

/// **A brief that used to be handed back now lands.**
///
/// The measure of the whole change: this is a perfectly ordinary paragraph
/// that produced nothing but complaints before the one-shots existed.
#[test]
fn an_ordinary_paragraph_now_lands() {
    let story = "\
Interior. Ana walks in from the left.
Ana sits down.
Ben points at the door.
Ana stands up.
Ana turns.
Ben reaches for the lamp.
Ana flinches.";
    let got = actions(story);
    assert_eq!(
        got,
        vec![
            Action::Walk,
            Action::Sit,
            Action::Point,
            Action::Stand,
            Action::Turn,
            Action::Reach,
            Action::React,
        ],
        "the brief did not come out as written"
    );
    assert!(
        ignored(story).is_empty(),
        "sentences were handed back: {:?}",
        ignored(story)
    );
}

/// **A beat takes as long as the action naturally takes**, unless the writer
/// said otherwise — and when they do say, they are obeyed.
#[test]
fn a_gesture_is_as_long_as_the_action_takes() {
    let mut scene = Scene::default();
    let directed = buzz_act::direct(&mut scene, "Day. Ana points at the door.").expect("directs");
    let beat = &directed.beats[0];
    let seconds = beat.frames.len() as f64 / 24.0;
    assert!(
        (seconds - Action::Point.cycle_seconds()).abs() < 0.2,
        "a point ran {seconds:.2}s, not about {}s",
        Action::Point.cycle_seconds()
    );

    let mut scene = Scene::default();
    let directed =
        buzz_act::direct(&mut scene, "Day. Ana points for three seconds.").expect("directs");
    let seconds = directed.beats[0].frames.len() as f64 / 24.0;
    assert!((seconds - 3.0).abs() < 0.2, "the writer asked for 3s and got {seconds:.2}s");
}

/// **A gesture does not move the character**, so the staging stays where the
/// director put it.
#[test]
fn a_gesture_does_not_travel() {
    let mut scene = Scene::default();
    let directed =
        buzz_act::direct(&mut scene, "Day. Ana sits down.\nAna stands up.").expect("directs");
    for beat in &directed.beats {
        assert_eq!(beat.travel, 0.0, "{:?} travelled", beat.action);
    }
}

/// **The old vocabulary still means what it did.** New words must not have
/// taken any of it.
#[test]
fn the_original_four_are_unchanged() {
    assert_eq!(first("Ana walks in from the left."), Some(Action::Walk));
    assert_eq!(first("Ana runs away."), Some(Action::Run));
    assert_eq!(first("Ana talks to Ben."), Some(Action::Talk));
    assert_eq!(first("Ana waits."), Some(Action::Idle));
    assert_eq!(first("Ana listens."), Some(Action::Idle));
}
