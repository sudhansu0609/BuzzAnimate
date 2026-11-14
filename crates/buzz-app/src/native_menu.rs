//! **The menus in the macOS menu bar**, at the top of the screen beside the
//! camera notch, as Animate has them — not in a row inside the window.
//!
//! Built from the same description the window's own menu row is drawn from
//! (`buzz_ui::panels::build_menus`, recorded by `buzz_ui::menu_ui`), so the two
//! never list different things. Rebuilt only when the menus change — an item
//! enabled or disabled, a switch ticked, a template saved — which
//! [`NativeMenu::update`] decides from the recorded shape each frame.
//!
//! # No key equivalents
//!
//! The items show no shortcut and own none. A menu that owned ⌘C or ⌘V would
//! take them from every text field in the window; the window's own keymap
//! (`Workspace::keymap`) stays the one place keys are handled, exactly as it
//! was with the menus inside the window.

use std::cell::RefCell;

use buzz_ui::menu_ui::{MenuAction, MenuNode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

thread_local! {
    /// The tags of the items chosen since the window last asked.
    static CHOSEN: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// The window to wake when one is: a menu click arrives with no egui
    /// event behind it.
    static WAKE: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BuzzAnimateMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(buzzMenuChosen:))]
        fn chosen(&self, sender: &NSMenuItem) {
            CHOSEN.with(|c| c.borrow_mut().push(sender.tag()));
            WAKE.with(|w| {
                if let Some(ctx) = w.borrow().as_ref() {
                    ctx.request_repaint();
                }
            });
        }
    }
);

/// The installed menu bar: what it was built from, and each item's action by
/// its tag.
pub struct NativeMenu {
    target: Retained<MenuTarget>,
    shape: Option<Vec<MenuNode>>,
    actions: Vec<MenuAction>,
}

impl NativeMenu {
    /// `None` off the main thread, where AppKit cannot be touched.
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        WAKE.with(|w| *w.borrow_mut() = Some(ctx.clone()));
        let target: Retained<MenuTarget> = unsafe { msg_send![MenuTarget::alloc(mtm), init] };
        Some(Self { target, shape: None, actions: Vec::new() })
    }

    /// Is `shape` what the menus were last built from?
    pub fn is_current(&self, shape: &[MenuNode]) -> bool {
        self.shape.as_deref() == Some(shape)
    }

    /// Build the menu bar from `menus` (whose blanked shape is `shape`).
    pub fn install(&mut self, shape: Vec<MenuNode>, menus: &[MenuNode]) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        self.actions.clear();
        let bar = new_menu(mtm, "");

        // The application menu, first: macOS names it after the app.
        let app_menu = new_menu(mtm, "BuzzAnimate");
        if let Some(about) = self.action_item(mtm, "About BuzzAnimate", true, false, Some(MenuAction::Command(buzz_ui::Command::About))) {
            app_menu.addItem(&about);
        }
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        for (title, action) in [("Hide BuzzAnimate", sel!(hide:)), ("Hide Others", sel!(hideOtherApplications:)), ("Show All", sel!(unhideAllApplications:))] {
            // Sent up the responder chain to the application itself.
            app_menu.addItem(&system_item(mtm, title, action));
        }
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        if let Some(quit) = self.action_item(mtm, "Quit BuzzAnimate", true, false, Some(MenuAction::Command(buzz_ui::Command::Quit))) {
            app_menu.addItem(&quit);
        }
        bar.addItem(&submenu_item(mtm, "BuzzAnimate", &app_menu));

        for node in menus {
            if let MenuNode::Menu { title, items, .. } = node {
                let menu = new_menu(mtm, title);
                self.fill(mtm, &menu, items);
                bar.addItem(&submenu_item(mtm, title, &menu));
            }
        }
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
        self.shape = Some(shape);
    }

    /// What was chosen since the last call.
    pub fn take_chosen(&self) -> Vec<MenuAction> {
        CHOSEN.with(|c| std::mem::take(&mut *c.borrow_mut()))
            .into_iter()
            .filter_map(|tag| self.actions.get(usize::try_from(tag).ok()?).cloned())
            .collect()
    }

    fn fill(&mut self, mtm: MainThreadMarker, menu: &NSMenu, items: &[MenuNode]) {
        for node in items {
            match node {
                MenuNode::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuNode::Note(text) => {
                    let item = system_item(mtm, text, sel!(buzzNothing:));
                    item.setEnabled(false);
                    menu.addItem(&item);
                }
                MenuNode::Item { label, enabled, checked, action } => {
                    if let Some(item) = self.action_item(mtm, label, *enabled, *checked, action.clone()) {
                        menu.addItem(&item);
                    }
                }
                MenuNode::Menu { title, enabled, items } => {
                    let sub = new_menu(mtm, title);
                    self.fill(mtm, &sub, items);
                    let item = submenu_item(mtm, title, &sub);
                    item.setEnabled(*enabled);
                    menu.addItem(&item);
                }
            }
        }
    }

    /// An item that reports itself to [`MenuTarget`] by tag. One with nothing
    /// to do (a Shot item with no shot open) is drawn disabled.
    fn action_item(&mut self, mtm: MainThreadMarker, label: &str, enabled: bool, checked: bool, action: Option<MenuAction>) -> Option<Retained<NSMenuItem>> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(buzzMenuChosen:)),
                &NSString::from_str(""),
            )
        };
        unsafe { item.setTarget(Some(&self.target as &AnyObject)) };
        match action {
            Some(action) => {
                item.setTag(self.actions.len() as isize);
                self.actions.push(action);
                item.setEnabled(enabled);
            }
            None => item.setEnabled(false),
        }
        if checked {
            item.setState(1);
        }
        Some(item)
    }
}

fn new_menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    // Enabled as recorded, not as AppKit would guess from the responder chain.
    menu.setAutoenablesItems(false);
    menu
}

fn submenu_item(mtm: MainThreadMarker, title: &str, menu: &NSMenu) -> Retained<NSMenuItem> {
    let item = system_item(mtm, title, sel!(buzzNothing:));
    unsafe { item.setAction(None::<Sel>) };
    item.setSubmenu(Some(menu));
    item
}

/// An item with no target, sending `action` up the responder chain.
fn system_item(mtm: MainThreadMarker, title: &str, action: Sel) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    }
}
