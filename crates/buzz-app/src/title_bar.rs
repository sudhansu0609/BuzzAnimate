//! Menus in the title bar.
//!
//! # Why a custom one
//!
//! The bar with the icon and the document's name used to be the OS's own —
//! native decorations, fixed to `BuzzAnimate` — with the menu row as a
//! separate strip of its own underneath. Two rows for what Animate draws in
//! one is the whole of what this closes: on Windows the app now draws the
//! drag area, the menu and the minimise, maximise and close buttons itself,
//! sharing the row the OS used to reserve for its own chrome.
//!
//! # Three modes, one per platform
//!
//! - [`TitleBar::Custom`] (Windows): native decorations are off
//!   (`with_decorations(false)`), and the app draws everything — see
//!   `App::title_bar_row`. Window actions (drag, minimise, maximise, close,
//!   resize) go through [`WindowRequest`], a queue `App::render` drains,
//!   which is what keeps them testable without a real window.
//! - [`TitleBar::MacInset`] (macOS): the native title bar stays, made
//!   transparent and inset, with the traffic lights left where the OS put
//!   them. The app's row sits behind them rather than below a second bar.
//! - [`TitleBar::Native`] (everything else): unchanged, except the menu row
//!   also names the document.
//!
//! # What this does not fix
//!
//! Windows' own Snap Layouts hover flyout on the Maximise button is a
//! feature of the *native* button; an undecorated window has none, and this
//! v1 accepts that loss. Drag-to-edge snapping and Win+arrow still work,
//! because both are handled by the compositor rather than by the button. A
//! `WM_NCHITTEST` subclass that brings the flyout back is a follow-up.

/// Which title bar this platform draws. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBar {
    /// Windows: native decorations off, the app draws the whole row.
    Custom,
    /// macOS: the native bar, transparent and inset, with the app's own row
    /// behind the traffic lights.
    MacInset,
    /// Every other platform: unchanged but for the document's name.
    Native,
}

impl TitleBar {
    /// The mode this platform draws, decided once at start-up.
    pub fn for_platform() -> Self {
        #[cfg(windows)]
        {
            Self::Custom
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacInset
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Self::Native
        }
    }

    /// Whether this mode draws its own drag area and window buttons —
    /// `Custom` only. `MacInset` leaves the buttons to the OS and only
    /// insets around them.
    pub fn is_custom(self) -> bool {
        matches!(self, Self::Custom)
    }
}

/// A window action asked for from inside the title bar row, queued rather
/// than acted on immediately — `App::render` drains it after `run_ui`, which
/// is what lets a test assert the request without a real window to act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowRequest {
    /// The drag area was pressed and dragged: move the window with the
    /// pointer.
    StartDrag,
    /// The drag area, or the maximise button, was double-clicked or
    /// clicked: toggle between maximised and restored.
    ToggleMaximize,
    Minimize,
    /// Asks to close the window — the same `CloseRequested` path the OS's
    /// own close button takes, so unsaved work is still asked about.
    Close,
    /// A border was pressed: resize from that edge or corner.
    Resize(winit::window::ResizeDirection),
}

/// Where the title bar's own hit-targets were drawn this frame — the drag
/// area and the three window buttons — for `window_event` to test the
/// pointer against, and for a test to find them by.
///
/// Each is `None` until the row has actually drawn once; a hit test against
/// an unset rect would otherwise silently accept the origin corner.
#[derive(Debug, Clone, Copy, Default)]
pub struct TitleBarRects {
    pub drag: Option<egui::Rect>,
    pub minimize: Option<egui::Rect>,
    pub maximize: Option<egui::Rect>,
    pub close: Option<egui::Rect>,
}

/// Which edge or corner of the window the pointer is over, within
/// `border_px` of it — or `None` in the interior. `pos` and `size` must be
/// in the same units (`Custom` mode uses physical pixels, to match
/// `WindowEvent::CursorMoved` and `Window::inner_size`).
///
/// Corners are tested before edges: a corner is where two edge bands
/// overlap, and testing edges first would resolve that overlap to whichever
/// axis happened to be checked first, giving a single-axis resize where the
/// pointer is plainly over a corner.
pub fn resize_direction(
    pos: egui::Pos2,
    size: egui::Vec2,
    border_px: f32,
) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection::{
        East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West,
    };

    if pos.x < 0.0 || pos.y < 0.0 || pos.x > size.x || pos.y > size.y {
        return None;
    }

    let west = pos.x < border_px;
    let east = pos.x > size.x - border_px;
    let north = pos.y < border_px;
    let south = pos.y > size.y - border_px;

    match (west, east, north, south) {
        (true, _, true, _) => Some(NorthWest),
        (_, true, true, _) => Some(NorthEast),
        (true, _, _, true) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, false, false, false) => Some(West),
        (false, true, false, false) => Some(East),
        (false, false, true, false) => Some(North),
        (false, false, false, true) => Some(South),
        _ => None,
    }
}
