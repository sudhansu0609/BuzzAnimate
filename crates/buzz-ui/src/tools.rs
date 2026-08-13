//! The tool set, its shortcuts, and how the toolbar is grouped.
//!
//! This module is the *catalogue*: what tools exist, what they are called, what
//! key selects them, and how they group in the toolbar. The behaviour lives in
//! `buzz-app`, so the shortcut map can be tested without a window.
//!
//! Letters follow Animate exactly. Muscle memory is the whole point — a user
//! who presses `V` expects the Selection tool, and getting that wrong is worse
//! than having fewer tools.

use egui::Key;

/// Every tool in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolId {
    Selection,
    Subselection,
    FreeTransform,
    GradientTransform,
    Lasso,
    Pen,
    Text,
    Line,
    Rectangle,
    Oval,
    PolyStar,
    Pencil,
    Brush,
    Bone,
    /// Animate's Asset Warp: handles dropped on artwork, dragged to deform it.
    AssetWarp,
    PaintBucket,
    InkBottle,
    Eyedropper,
    Eraser,
    Camera,
    Hand,
    Zoom,
}

/// Whether a tool is usable yet.
///
/// Shown honestly in the toolbar. A tool that looks available but silently does
/// nothing is worse than one that says it is not ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Ready,
    /// Arrives in a later phase; the tooltip says which.
    Planned(&'static str),
}

impl ToolId {
    /// Name as shown in the tooltip.
    pub fn name(self) -> &'static str {
        use ToolId::*;
        match self {
            Selection => "Selection",
            Subselection => "Subselection",
            FreeTransform => "Free Transform",
            GradientTransform => "Gradient Transform",
            Lasso => "Lasso",
            Pen => "Pen",
            Text => "Text",
            Line => "Line",
            Rectangle => "Rectangle",
            Oval => "Oval",
            PolyStar => "PolyStar",
            Pencil => "Pencil",
            Brush => "Brush",
            Bone => "Bone",
            AssetWarp => "Asset Warp",
            PaintBucket => "Paint Bucket",
            InkBottle => "Ink Bottle",
            Eyedropper => "Eyedropper",
            Eraser => "Eraser",
            Camera => "Camera",
            Hand => "Hand",
            Zoom => "Zoom",
        }
    }

    /// Animate's single-letter shortcut.
    pub fn shortcut(self) -> Option<Key> {
        use ToolId::*;
        match self {
            Selection => Some(Key::V),
            Subselection => Some(Key::A),
            FreeTransform => Some(Key::Q),
            Lasso => Some(Key::L),
            Pen => Some(Key::P),
            Text => Some(Key::T),
            Line => Some(Key::N),
            Rectangle => Some(Key::R),
            Oval => Some(Key::O),
            Pencil => Some(Key::Y),
            Brush => Some(Key::B),
            Bone => Some(Key::M),
            // Animate's own letter for Asset Warp.
            AssetWarp => Some(Key::W),
            PaintBucket => Some(Key::K),
            InkBottle => Some(Key::S),
            Eyedropper => Some(Key::I),
            Eraser => Some(Key::E),
            Camera => Some(Key::C),
            Hand => Some(Key::H),
            Zoom => Some(Key::Z),
            // Animate leaves these two without a letter.
            GradientTransform | PolyStar => None,
        }
    }

    /// The button's glyph.
    ///
    /// # Why these are letters
    ///
    /// Adobe's tool icons cannot be used, and egui's bundled fonts cover only a
    /// scattering of the symbols a tool palette wants — measured on the real
    /// window, `▭ ◯ ╱ ➤ ⇱ ⤢ ✒ ▨` all rendered as empty boxes, which is worse
    /// than no icon at all.
    ///
    /// So each button shows its **shortcut letter**. It always renders, and it
    /// teaches the keyboard shortcut every time the user looks at the palette.
    /// The two tools Animate leaves without a letter use symbols confirmed to
    /// render. Full names are in the tooltip.
    pub fn glyph(self) -> &'static str {
        use ToolId::*;
        match self {
            Selection => "V",
            Subselection => "A",
            FreeTransform => "Q",
            // No shortcut in Animate; both of these render correctly.
            GradientTransform => "◑",
            PolyStar => "☆",
            Lasso => "L",
            Pen => "P",
            Text => "T",
            Line => "N",
            Rectangle => "R",
            Oval => "O",
            Pencil => "Y",
            Brush => "B",
            Bone => "M",
            AssetWarp => "W",
            PaintBucket => "K",
            InkBottle => "S",
            Eyedropper => "I",
            Eraser => "E",
            Camera => "C",
            Hand => "H",
            Zoom => "Z",
        }
    }

    /// Is this tool implemented?
    pub fn status(self) -> ToolStatus {
        use ToolId::*;
        match self {
            Selection | Subselection | FreeTransform | Line | Rectangle | Oval | PolyStar
            | Pencil | Brush | Eraser | PaintBucket | InkBottle | Eyedropper | Hand | Zoom
            | Pen | Camera | Bone | AssetWarp => ToolStatus::Ready,
            Text => ToolStatus::Planned("Text arrives with Phase 2 follow-up"),
            Lasso => ToolStatus::Planned("Lasso arrives with Phase 2 follow-up"),
            GradientTransform => ToolStatus::Planned("Gradients arrive with the Color panel"),
        }
    }

    pub fn is_ready(self) -> bool {
        matches!(self.status(), ToolStatus::Ready)
    }

    /// Does this tool draw a new shape, rather than edit existing ones?
    pub fn is_drawing_tool(self) -> bool {
        use ToolId::*;
        matches!(
            self,
            Line | Rectangle | Oval | PolyStar | Pencil | Brush | Pen
        )
    }

    /// Does the tool navigate rather than modify the document?
    ///
    /// Navigation tools stay usable on a locked layer.
    pub fn is_navigation(self) -> bool {
        matches!(self, ToolId::Hand | ToolId::Zoom)
    }

    /// Does the tool edit the camera rather than artwork?
    ///
    /// The camera belongs to the document, so this is *not* navigation even
    /// though it looks like panning: moving it changes the exported result.
    pub fn is_camera(self) -> bool {
        matches!(self, ToolId::Camera)
    }
}

/// Toolbar groups, separated by a divider, following Animate's arrangement.
pub const TOOL_GROUPS: &[&[ToolId]] = &[
    &[
        ToolId::Selection,
        ToolId::Subselection,
        ToolId::FreeTransform,
        ToolId::GradientTransform,
        ToolId::Lasso,
    ],
    &[ToolId::Pen, ToolId::Text],
    &[
        ToolId::Line,
        ToolId::Rectangle,
        ToolId::Oval,
        ToolId::PolyStar,
    ],
    &[ToolId::Pencil, ToolId::Brush],
    &[ToolId::Bone, ToolId::AssetWarp],
    &[
        ToolId::PaintBucket,
        ToolId::InkBottle,
        ToolId::Eyedropper,
        ToolId::Eraser,
    ],
    &[ToolId::Camera, ToolId::Hand, ToolId::Zoom],
];

/// Every tool, in toolbar order.
pub fn all_tools() -> Vec<ToolId> {
    TOOL_GROUPS.iter().flat_map(|g| g.iter().copied()).collect()
}

/// Which tool a bare letter selects.
pub fn tool_for_key(key: Key) -> Option<ToolId> {
    all_tools().into_iter().find(|t| t.shortcut() == Some(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_toolbar_contains_every_tool_exactly_once() {
        let tools = all_tools();
        let unique: HashSet<ToolId> = tools.iter().copied().collect();
        assert_eq!(
            tools.len(),
            unique.len(),
            "a tool appears in more than one group"
        );
        // 21 through Phase 5, plus Asset Warp when rigging landed in Phase 7.
        assert_eq!(tools.len(), 22, "unexpected tool count");
    }

    /// Muscle memory: these letters must do what an Animate user expects.
    #[test]
    fn letter_shortcuts_match_animate() {
        let cases = [
            (Key::V, ToolId::Selection),
            (Key::A, ToolId::Subselection),
            (Key::Q, ToolId::FreeTransform),
            (Key::L, ToolId::Lasso),
            (Key::P, ToolId::Pen),
            (Key::T, ToolId::Text),
            (Key::N, ToolId::Line),
            (Key::R, ToolId::Rectangle),
            (Key::O, ToolId::Oval),
            (Key::Y, ToolId::Pencil),
            (Key::B, ToolId::Brush),
            (Key::M, ToolId::Bone),
            (Key::W, ToolId::AssetWarp),
            (Key::K, ToolId::PaintBucket),
            (Key::S, ToolId::InkBottle),
            (Key::I, ToolId::Eyedropper),
            (Key::E, ToolId::Eraser),
            (Key::C, ToolId::Camera),
            (Key::H, ToolId::Hand),
            (Key::Z, ToolId::Zoom),
        ];
        for (key, expected) in cases {
            assert_eq!(
                tool_for_key(key),
                Some(expected),
                "{key:?} should select {expected:?}"
            );
        }
    }

    #[test]
    fn no_two_tools_share_a_letter() {
        let mut seen: HashSet<Key> = HashSet::new();
        for tool in all_tools() {
            if let Some(key) = tool.shortcut() {
                assert!(seen.insert(key), "{key:?} is bound by two tools");
            }
        }
    }

    #[test]
    fn every_tool_has_a_name_and_a_glyph() {
        for tool in all_tools() {
            assert!(!tool.name().is_empty(), "{tool:?} has no name");
            assert!(!tool.glyph().is_empty(), "{tool:?} has no glyph");
        }
    }

    /// The glyph must be the shortcut letter wherever there is one, so the
    /// palette teaches the keyboard. Regression guard for the empty-box
    /// symbols this replaced.
    #[test]
    fn glyphs_show_the_shortcut_letter() {
        for tool in all_tools() {
            let Some(key) = tool.shortcut() else { continue };
            assert_eq!(
                tool.glyph(),
                key.name(),
                "{tool:?} should show its shortcut letter"
            );
        }
    }

    /// A tool that is not implemented must say so rather than look available.
    #[test]
    fn unimplemented_tools_declare_themselves() {
        assert!(matches!(ToolId::Text.status(), ToolStatus::Planned(_)));
        assert!(matches!(ToolId::Lasso.status(), ToolStatus::Planned(_)));
        assert!(ToolId::Selection.is_ready());
        assert!(ToolId::Rectangle.is_ready());
        assert!(ToolId::Camera.is_ready(), "the camera arrived in Phase 3");
        assert!(ToolId::Bone.is_ready(), "rigging arrived in Phase 7");
        assert!(ToolId::AssetWarp.is_ready(), "and so did the warp");
    }

    /// The camera edits the document, so it must not be classed as navigation
    /// — navigation tools are allowed on locked layers and are not undoable.
    #[test]
    fn the_camera_is_not_a_navigation_tool() {
        assert!(ToolId::Camera.is_camera());
        assert!(!ToolId::Camera.is_navigation());
        assert!(!ToolId::Hand.is_camera());
    }

    #[test]
    fn most_of_the_toolbar_is_usable() {
        let ready = all_tools().iter().filter(|t| t.is_ready()).count();
        assert!(
            ready >= 15,
            "only {ready} of {} tools are ready",
            all_tools().len()
        );
    }

    #[test]
    fn drawing_and_navigation_tools_are_classified() {
        assert!(ToolId::Rectangle.is_drawing_tool());
        assert!(ToolId::Pencil.is_drawing_tool());
        assert!(!ToolId::Selection.is_drawing_tool());

        assert!(ToolId::Hand.is_navigation());
        assert!(ToolId::Zoom.is_navigation());
        assert!(!ToolId::Brush.is_navigation());
    }
}
