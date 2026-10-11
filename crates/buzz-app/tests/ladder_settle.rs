//! L7 in `docs/RIG_SCENE_LAG_PLAN.md`: what the stage's encode loops do once
//! the detail ladder's passes are spent. The decision is a pure function so it
//! needs no encode.

use buzz_app::stage::settle_after_ladder;
use buzz_render::document::LightDetail::{Flat, Full, NoModelling};

#[test]
fn re_encodes_once_at_the_fallback_when_the_last_pass_was_over_budget() {
    assert_eq!(settle_after_ladder(true, Some(NoModelling), Full), Some(NoModelling));
    assert_eq!(settle_after_ladder(true, Some(Flat), NoModelling), Some(Flat));
}

#[test]
fn leaves_a_pass_that_fit_alone() {
    assert_eq!(settle_after_ladder(false, Some(NoModelling), Full), None);
    assert_eq!(settle_after_ladder(false, None, Full), None);
}

#[test]
fn has_nothing_to_do_without_a_fitting_level() {
    assert_eq!(settle_after_ladder(true, None, Flat), None);
}

#[test]
fn never_re_encodes_the_level_the_last_pass_already_used() {
    assert_eq!(settle_after_ladder(true, Some(NoModelling), NoModelling), None);
}
