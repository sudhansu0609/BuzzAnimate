//! **`Insert ▸ Scene ▸ Direct a Story…` puts the Story panel in front of you.**
//!
//! # What a menu item like this owes the user
//!
//! It is not a toggle and it is not a tab. Somebody who picks *Direct a Story*
//! off a menu has said, unambiguously, "show me the thing that directs a
//! story" — and the only acceptable outcome is that they are then looking at
//! it. Three separate things can stop that, and all three are ordinary states a
//! real workspace ends up in:
//!
//! 1. **The panel is hidden.** Someone closed it, or the layout predates it.
//! 2. **It is a background tab.** It shares a section with five other panels,
//!    and an upgraded layout keeps whichever tab the user had at the front.
//! 3. **The section is rolled up.** Selecting a tab deliberately does *not*
//!    unroll a section — that is right for clicking a tab and wrong for a menu
//!    item, which has no other way to show you anything.
//! 4. **It is below the fold.** Open, fronted, unrolled — and sixth in a
//!    right-hand column that is taller than the screen. The column scrolls,
//!    and nothing scrolled it.
//!
//! The third and fourth are the ones that read as "I clicked it and nothing
//! happened", because something *did* happen: the right tab was selected,
//! behind a rolled-up title bar or off the bottom of the window. The fourth
//! was found on this machine, with a layout every other measure called fine.

use buzz_app::editor::Editor;
use buzz_ui::{Command, Dock, PanelId};

/// An editor with a document, as it is after launch.
fn app() -> Editor {
    Editor::new(buzz_doc::Document::default())
}

/// Every way the panel can be out of sight, and the menu item beating it.
#[test]
fn directing_a_story_brings_the_panel_to_the_front() {
    for (name, prepare) in [
        (
            "hidden",
            Box::new(|app: &mut Editor| {
                app.workspace.move_to(PanelId::Story, Dock::Hidden);
            }) as Box<dyn Fn(&mut Editor)>,
        ),
        (
            "a background tab",
            Box::new(|app: &mut Editor| {
                app.workspace.select_tab(PanelId::Depth);
            }),
        ),
        (
            "rolled up",
            Box::new(|app: &mut Editor| {
                app.workspace.select_tab(PanelId::Depth);
                app.workspace.set_collapsed(PanelId::Depth, true);
            }),
        ),
    ] {
        let mut app = app();
        prepare(&mut app);

        app.run(Command::DirectScene);

        let workspace = &app.workspace;
        assert!(
            workspace.is_open(PanelId::Story),
            "{name}: the Story panel is still hidden"
        );
        let section = workspace
            .section_of(PanelId::Story)
            .unwrap_or_else(|| panic!("{name}: the Story panel is not in a section"));
        assert_eq!(
            section.front,
            PanelId::Story,
            "{name}: the Story panel is behind another tab"
        );
        assert!(
            !workspace.is_collapsed(PanelId::Story),
            "{name}: the section is rolled up, so the panel shows nothing"
        );
        assert_eq!(
            workspace.scroll_to,
            Some(PanelId::Story),
            "{name}: the column was not asked to scroll to it"
        );
    }
}

/// **The set and the scenery are named on the menu**, and a menu item that
/// names a section has to open that section — not merely take you to a panel
/// where it might be collapsed.
#[test]
fn each_menu_item_opens_the_part_it_names() {
    for (command, wants_set, wants_scenery) in [
        (Command::SetScene, true, false),
        (Command::SceneryFor, false, true),
        (Command::DirectScene, false, false),
    ] {
        let mut app = app();
        // Both shut, as they would be after somebody collapsed them.
        app.story.show_set = false;
        app.story.show_scenery = false;

        app.run(command);

        assert_eq!(
            app.story.show_set, wants_set,
            "{command:?} left the set section shut"
        );
        assert_eq!(
            app.story.show_scenery, wants_scenery,
            "{command:?} left the scenery section shut"
        );
        assert!(
            app.story.reveal,
            "{command:?} did not ask the panel to obey those"
        );
    }
}

/// **Directing puts the caret where the writing goes.**
///
/// The modal this replaced opened with the cursor in the text field, which is
/// the whole of what an ellipsis on a menu item promises: you asked to write a
/// story, so you are writing one. A panel that merely appears leaves you to
/// find a text box among six tabs in a narrow column first.
#[test]
fn directing_a_story_puts_the_caret_in_the_brief() {
    let mut directing = app();
    directing.run(Command::DirectScene);
    assert!(directing.story.focus_draft, "the brief box was not asked for");

    // The other two are about the panel's other sections, and stealing the
    // keyboard from somebody adjusting a slider would be a nuisance.
    for command in [Command::SetScene, Command::SceneryFor] {
        let mut app = app();
        app.run(command);
        assert!(
            !app.story.focus_draft,
            "{command:?} took the keyboard for the brief"
        );
    }
}

/// **And it says what to do.** A panel appearing in a column is not an
/// instruction; the status bar is where this program tells you one.
#[test]
fn each_menu_item_says_what_to_do_next() {
    for (command, wants) in [
        (Command::DirectScene, "Direct"),
        (Command::SetScene, "Set the scene"),
        (Command::SceneryFor, "Scenery"),
    ] {
        let mut app = app();
        app.run(command);
        let said = app.status.clone().unwrap_or_default();
        assert!(
            said.contains(wants),
            "{command:?} said {said:?}, which does not mention {wants:?}"
        );
    }
}
