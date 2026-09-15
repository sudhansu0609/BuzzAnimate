//! **The layout on this machine, migrated.**
//!
//! A copy of a real `workspace.json` in which the Story panel had been stranded
//! at row ten of the right column, alone in a section of its own — open,
//! selected, unrolled, and below the fold. Every measure the code had said the
//! panel was showing; the person who asked for it saw an unchanged screen.
//!
//! Kept as a fixture rather than rebuilt in code, because the point of it is
//! that it came off a disk rather than out of an assumption.

use buzz_ui::{PanelId, Workspace};

#[test]
fn a_real_saved_layout_puts_the_story_panel_back_in_view() {
    let text = include_str!("fixtures/stranded-story-workspace.json");
    let saved: Workspace = serde_json::from_str(text).expect("the fixture parses");
    // As it was: stranded.
    let before = saved.slot(PanelId::Story).expect("a Story slot");
    assert_eq!(before.order, 10, "the fixture is not the stranded layout");

    // As `Workspace::load_from` does on the way in.
    let mut migrated = saved;
    migrated.fill_gaps();

    let section = migrated
        .section_of(PanelId::Story)
        .expect("the Story panel is on screen");
    assert!(
        section.panels.contains(&PanelId::Depth),
        "still alone at the bottom: {:?}",
        section.panels
    );
    assert_eq!(section.front, PanelId::Story, "put back behind another tab");
    assert!(!section.collapsed, "put back into a rolled-up section");

    let last = migrated
        .slots
        .iter()
        .filter(|s| s.dock == buzz_ui::Dock::Right)
        .map(|s| s.order)
        .max()
        .expect("a right column");
    let mine = migrated.slot(PanelId::Story).expect("a slot").order;
    assert!(mine < last, "still at the bottom of the column ({mine} of {last})");
}
