//! L7 in `docs/RIG_SCENE_LAG_PLAN.md`: the lighting-detail ladder
//! (`DrawCache::reconsider`) must not step up into a level it has just seen
//! over budget at the same buffer scale in the same frame, must use the ratio
//! it measured between levels once it has one, and must be able to name a
//! level that fit when a frame's last pass did not. Pure bookkeeping, no GPU:
//! each test plays the part of `buzz-app`'s encode loop by hand.

use buzz_render::document::{DrawCache, LightDetail, line_budget_at, segment_ceiling_at};

const PIXELS: f64 = 1920.0 * 1080.0;
/// The scale the film this was measured on settles at (its `max_scale`).
const SCALE: u32 = 8;

fn cache_at_max_scale() -> DrawCache {
    let mut cache = DrawCache::new();
    cache.set_scale(SCALE);
    cache.set_max_scale(SCALE);
    cache
}

/// One screen frame: `begin`, then up to four passes of note -> reconsider,
/// exactly the shape of `stage::build_plain_scene`'s loop. `cost` answers the
/// `(segments, lines)` a pass encoded at the given level measures. Returns the
/// level of each pass encoded.
fn frame(
    cache: &mut DrawCache,
    cost: impl Fn(LightDetail) -> (u32, u64),
) -> Vec<LightDetail> {
    cache.begin();
    let mut passes = Vec::new();
    for _ in 0..4 {
        let level = cache.detail();
        passes.push(level);
        let (segments, lines) = cost(level);
        cache.note_lines(lines);
        if !cache.reconsider(segments, PIXELS) {
            break;
        }
    }
    passes
}

/// The rim frame: Full costs ten times NoModelling and does not fit; the
/// others do.
fn rim_cost(level: LightDetail) -> (u32, u64) {
    match level {
        LightDetail::Full => (200_000, 15_000_000),
        LightDetail::NoModelling => (150_000, 1_500_000),
        LightDetail::Flat | LightDetail::Unsealed => (100_000, 1_000_000),
    }
}

/// (a) Full = 10x NoModelling, NoModelling fits: Full -> NoModelling -> stop.
/// Today's fixed margin (a sixth of the budget, 2.4 M lines here) says Full
/// would fit from 1.5 M lines and steps up again; the pass just measured says
/// otherwise.
#[test]
fn a_level_refused_this_frame_is_not_stepped_back_up_into() {
    assert!(15_000_000 > line_budget_at(u64::from(SCALE)), "Full must be over");
    assert!(1_500_000 < line_budget_at(u64::from(SCALE)) / 6, "the fixed margin would step up");
    let mut cache = cache_at_max_scale();

    let passes = frame(&mut cache, rim_cost);
    assert_eq!(
        passes,
        [LightDetail::Full, LightDetail::NoModelling],
        "two passes, no oscillation"
    );
    assert_eq!(cache.detail(), LightDetail::NoModelling);
    assert!(!cache.over_budget_last_pass(), "the last pass fit");
    assert_eq!(cache.fitting_fallback(), None, "nothing to fall back from");
    assert_eq!(cache.last_pass_detail(), LightDetail::NoModelling);

    // The next frame of the same content: the ratio measured above (10x) keeps
    // the fixed margin from sending it up to Full to find out again.
    let passes = frame(&mut cache, rim_cost);
    assert_eq!(passes, [LightDetail::NoModelling], "one pass: the measured ratio refuses");
    assert_eq!(cache.detail(), LightDetail::NoModelling);
}

/// (b) A pass the line refusal cannot see: Full is over the *segment* ceiling
/// while every level is under the line budget. The ladder alternates, and the
/// fourth pass is a Full over budget — which is what the post-loop fallback is
/// for: the frame knows NoModelling fit.
#[test]
fn a_loop_that_ends_over_budget_names_the_level_that_fit() {
    let ceiling = segment_ceiling_at(PIXELS, u64::from(SCALE));
    let cost = move |level: LightDetail| match level {
        LightDetail::Full => (ceiling + 1_000, 900_000),
        _ => (ceiling / 20, 700_000),
    };
    let mut cache = cache_at_max_scale();
    // Start the frame on NoModelling (the level the last frame settled on), so
    // the four passes end on a Full.
    cache.pin_detail(Some(LightDetail::NoModelling));
    cache.pin_detail(None);

    let passes = frame(&mut cache, cost);
    assert_eq!(
        passes,
        [
            LightDetail::NoModelling,
            LightDetail::Full,
            LightDetail::NoModelling,
            LightDetail::Full
        ],
        "the segment-driven oscillation the line refusal cannot see"
    );
    assert!(cache.over_budget_last_pass(), "the fourth pass was a Full over the ceiling");
    assert_eq!(cache.last_pass_detail(), LightDetail::Full);
    assert_eq!(cache.fitting_fallback(), Some(LightDetail::NoModelling));

    // What `build_plain_scene` then does: settle on it, encode once, record it.
    cache.settle_on(LightDetail::NoModelling);
    assert_eq!(cache.detail(), LightDetail::NoModelling);
    let (segments, lines) = cost(LightDetail::NoModelling);
    cache.note_settled_pass(segments, lines, PIXELS);
    assert!(!cache.over_budget_last_pass());
    assert!(!cache.over_budget(), "the submitted pass fits");
    assert_eq!(cache.last_pass_detail(), LightDetail::NoModelling);
    assert_eq!(cache.fitting_fallback(), None);
}

/// (c) A frame that later shrinks — Full now only 2x NoModelling — climbs back
/// to Full, even with the 10x ratio remembered from the heavy frame, because
/// the room it needs by that ratio is there.
#[test]
fn a_frame_that_shrinks_steps_back_up() {
    let mut cache = cache_at_max_scale();
    assert_eq!(
        frame(&mut cache, rim_cost),
        [LightDetail::Full, LightDetail::NoModelling]
    );

    let light = |level: LightDetail| match level {
        LightDetail::Full => (120_000, 2_000_000),
        LightDetail::NoModelling => (100_000, 1_000_000),
        LightDetail::Flat | LightDetail::Unsealed => (80_000, 800_000),
    };
    let passes = frame(&mut cache, light);
    assert_eq!(passes, [LightDetail::NoModelling, LightDetail::Full]);
    assert_eq!(cache.detail(), LightDetail::Full);
    assert!(!cache.over_budget_last_pass());

    // And it stays there: nothing above Full to climb to.
    assert_eq!(frame(&mut cache, light), [LightDetail::Full]);
}

/// The measurements belong to one buffer scale: a growth leaves nothing to
/// fall back to, and says so.
#[test]
fn growing_the_buffer_scale_forgets_what_was_measured_at_the_old_one() {
    let mut cache = DrawCache::new();
    cache.set_max_scale(SCALE);
    cache.begin();
    cache.note_lines(5_000_000);
    assert!(cache.reconsider(100_000, PIXELS), "grows");
    assert!(cache.scale() > 1);
    assert!(cache.over_budget_last_pass());
    assert_eq!(cache.fitting_fallback(), None);
}

/// Pinned: the ladder is not judging, so there is nothing to settle.
#[test]
fn a_pinned_level_has_no_fallback() {
    let mut cache = cache_at_max_scale();
    cache.begin();
    cache.pin_detail(Some(LightDetail::Full));
    cache.note_lines(30_000_000);
    assert!(!cache.reconsider(100_000, PIXELS));
    assert!(!cache.over_budget_last_pass());
    assert_eq!(cache.fitting_fallback(), None);
}
