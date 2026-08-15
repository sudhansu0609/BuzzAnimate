//! Asking for a file without stopping the program.
//!
//! # What was wrong with calling `rfd` directly
//!
//! `rfd::FileDialog::pick_file()` blocks the thread it is called on until the
//! user chooses. Called from the UI thread — which every one of the nine call
//! sites did — that thread stops pumping events and stops drawing. Windows
//! notices within a few seconds and paints the "Not Responding" ghost over the
//! window; a running export stops reporting progress and its Cancel button
//! stops working; and if the export finishes while the picker is up, nothing
//! notices until the picker closes.
//!
//! So the picker runs on a thread of its own and the answer comes back over a
//! channel, which the frame loop reads like anything else.
//!
//! # It is still modal, and that is on purpose
//!
//! The dialog is parented to the main window, so Windows disables input to the
//! app while it is up — which is exactly what File ▸ Open should do, and what
//! an unparented dialog would fail to do while also being free to disappear
//! *behind* the window it belongs to. What changes is the part that was
//! actually broken: the window keeps painting, background work keeps running
//! and keeps reporting, and the process never looks hung.
//!
//! # Why one at a time
//!
//! Two pickers open at once is two modal dialogs fighting over the same owner
//! window. [`Pending::busy`] is how the caller declines the second.

use std::path::PathBuf;

use crossbeam_channel::Receiver;

/// A file-type filter, as it appears in the picker's dropdown.
#[derive(Debug, Clone)]
pub struct Filter {
    pub label: String,
    pub extensions: Vec<String>,
}

impl Filter {
    pub fn new(label: &str, extensions: &[&str]) -> Self {
        Self {
            label: label.to_string(),
            extensions: extensions.iter().map(|e| (*e).to_string()).collect(),
        }
    }
}

/// Which of the three pickers to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    OpenFile,
    SaveFile,
    Folder,
}

/// What to open, and how it should look when it does.
#[derive(Debug, Clone)]
pub struct Request {
    ask: Ask,
    title: Option<String>,
    filters: Vec<Filter>,
    directory: Option<PathBuf>,
    file_name: Option<String>,
}

impl Request {
    /// Pick an existing file.
    pub fn open_file() -> Self {
        Self::of(Ask::OpenFile)
    }

    /// Choose where to write one.
    pub fn save_file() -> Self {
        Self::of(Ask::SaveFile)
    }

    /// Pick a directory.
    pub fn folder() -> Self {
        Self::of(Ask::Folder)
    }

    fn of(ask: Ask) -> Self {
        Self {
            ask,
            title: None,
            filters: Vec::new(),
            directory: None,
            file_name: None,
        }
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    pub fn filter(mut self, label: &str, extensions: &[&str]) -> Self {
        self.filters.push(Filter::new(label, extensions));
        self
    }

    pub fn directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// Run it, on the calling thread. Not for the UI thread — see [`ask`].
    fn show(self, parent: Parent) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new();
        if let Some(title) = &self.title {
            dialog = dialog.set_title(title);
        }
        for filter in &self.filters {
            let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&filter.label, &extensions);
        }
        // Only if it is still there: a remembered folder that has since been
        // deleted makes some backends open on nothing at all.
        if let Some(directory) = self.directory.as_ref().filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(name) = &self.file_name {
            dialog = dialog.set_file_name(name);
        }
        if let Some(owner) = parent.owner() {
            dialog = dialog.set_parent(&owner);
        }

        match self.ask {
            Ask::OpenFile => dialog.pick_file(),
            Ask::SaveFile => dialog.save_file(),
            Ask::Folder => dialog.pick_folder(),
        }
    }
}

/// The window the picker should belong to.
///
/// Held as a bare handle rather than as the window itself, because the window
/// is not `Send` and the picker runs somewhere else. The handle is read on the
/// UI thread, while the window certainly exists, and used before the window
/// can close — a close request is only acted on between frames, and a frame
/// cannot start while a modal dialog owned by that window is up.
#[derive(Debug, Clone, Copy, Default)]
pub struct Parent {
    #[cfg(windows)]
    hwnd: Option<std::num::NonZeroIsize>,
}

impl Parent {
    /// Read the handle off the main window.
    #[cfg(windows)]
    pub fn of(window: &winit::window::Window) -> Self {
        use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        let hwnd = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd),
            _ => None,
        };
        Self { hwnd }
    }

    /// Elsewhere the picker runs unparented — there is no portable way to hand
    /// a window handle to another thread, and the backends that matter here
    /// are the Windows ones.
    #[cfg(not(windows))]
    pub fn of(_window: &winit::window::Window) -> Self {
        Self::default()
    }

    #[cfg(windows)]
    fn owner(self) -> Option<Owner> {
        self.hwnd.map(Owner)
    }

    #[cfg(not(windows))]
    fn owner(self) -> Option<Owner> {
        None
    }
}

/// The handle, wearing the traits `rfd` wants.
#[cfg(windows)]
struct Owner(std::num::NonZeroIsize);

#[cfg(not(windows))]
struct Owner;

#[cfg(windows)]
impl winit::raw_window_handle::HasWindowHandle for Owner {
    fn window_handle(
        &self,
    ) -> Result<winit::raw_window_handle::WindowHandle<'_>, winit::raw_window_handle::HandleError>
    {
        use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle, WindowHandle};
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // Safe because the window is alive for as long as this borrow: the
        // handle was taken this frame and the picker owns the window until it
        // closes, which is before anything can destroy it.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(windows)]
impl winit::raw_window_handle::HasDisplayHandle for Owner {
    fn display_handle(
        &self,
    ) -> Result<winit::raw_window_handle::DisplayHandle<'_>, winit::raw_window_handle::HandleError>
    {
        use winit::raw_window_handle::{DisplayHandle, RawDisplayHandle, WindowsDisplayHandle};
        let raw = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

#[cfg(not(windows))]
impl winit::raw_window_handle::HasWindowHandle for Owner {
    fn window_handle(
        &self,
    ) -> Result<winit::raw_window_handle::WindowHandle<'_>, winit::raw_window_handle::HandleError>
    {
        Err(winit::raw_window_handle::HandleError::NotSupported)
    }
}

#[cfg(not(windows))]
impl winit::raw_window_handle::HasDisplayHandle for Owner {
    fn display_handle(
        &self,
    ) -> Result<winit::raw_window_handle::DisplayHandle<'_>, winit::raw_window_handle::HandleError>
    {
        Err(winit::raw_window_handle::HandleError::NotSupported)
    }
}

/// A picker that is open, and what the answer is for.
///
/// `P` is the caller's own "why did I ask": an enum of everything a path might
/// be wanted for. Keeping it out here means this module knows nothing about
/// exports or imports, and the frame loop gets its answer already labelled.
pub struct Pending<P> {
    open: Option<Open<P>>,
}

struct Open<P> {
    answer: Receiver<Option<PathBuf>>,
    purpose: P,
}

impl<P> Default for Pending<P> {
    fn default() -> Self {
        Self { open: None }
    }
}

impl<P> Pending<P> {
    /// Is a picker already up? Two modal dialogs on one owner window is not a
    /// state worth having.
    pub fn busy(&self) -> bool {
        self.open.is_some()
    }

    /// Open a picker, unless one is already open.
    ///
    /// Returns false when it declined, so the caller can say why.
    pub fn ask(&mut self, request: Request, parent: Parent, purpose: P) -> bool {
        if self.busy() {
            return false;
        }

        let (send, answer) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("buzz-picker".into())
            .spawn(move || {
                let _ = send.send(request.show(parent));
            })
            .is_ok();

        if !spawned {
            return false;
        }

        self.open = Some(Open { answer, purpose });
        true
    }

    /// The answer, once there is one.
    ///
    /// `Some((purpose, None))` is the user pressing Cancel, which is a normal
    /// thing to do and must not be confused with the picker still being open.
    pub fn poll(&mut self) -> Option<(P, Option<PathBuf>)> {
        let open = self.open.as_ref()?;
        match open.answer.try_recv() {
            Ok(path) => {
                let open = self.open.take()?;
                Some((open.purpose, path))
            }
            // The thread died without answering. Treat it as a cancel: the
            // alternative is a picker that is for ever "already open".
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                let open = self.open.take()?;
                Some((open.purpose, None))
            }
            Err(crossbeam_channel::TryRecvError::Empty) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picker cannot be opened in a test — it would wait for a person. What
    /// *can* be tested is the state machine around it, which is where the
    /// "already open" and "cancelled" bugs would live.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Why {
        Open,
        Save,
    }

    /// Stand in for the picker thread: hand `Pending` a channel we control.
    fn opened(purpose: Why) -> (Pending<Why>, crossbeam_channel::Sender<Option<PathBuf>>) {
        let (send, answer) = crossbeam_channel::bounded(1);
        (
            Pending {
                open: Some(Open { answer, purpose }),
            },
            send,
        )
    }

    #[test]
    fn nothing_is_pending_to_begin_with() {
        let mut pending: Pending<Why> = Pending::default();
        assert!(!pending.busy());
        assert!(pending.poll().is_none());
    }

    #[test]
    fn a_chosen_path_comes_back_with_its_purpose() {
        let (mut pending, send) = opened(Why::Open);
        assert!(pending.busy());
        assert!(pending.poll().is_none(), "not answered yet");

        send.send(Some(PathBuf::from("film.buzz"))).unwrap();
        let (why, path) = pending.poll().expect("an answer");
        assert_eq!(why, Why::Open);
        assert_eq!(path, Some(PathBuf::from("film.buzz")));
        assert!(!pending.busy(), "it should be free again");
    }

    /// Cancel is an answer, not silence — otherwise the picker stays "open"
    /// for ever and File ▸ Open never works again.
    #[test]
    fn cancelling_frees_the_picker() {
        let (mut pending, send) = opened(Why::Save);
        send.send(None).unwrap();

        let (why, path) = pending.poll().expect("an answer");
        assert_eq!(why, Why::Save);
        assert_eq!(path, None);
        assert!(!pending.busy());
    }

    /// Same for a picker thread that dies without saying anything.
    #[test]
    fn a_lost_picker_frees_itself() {
        let (mut pending, send) = opened(Why::Open);
        drop(send);

        let (_, path) = pending.poll().expect("an answer");
        assert_eq!(path, None);
        assert!(!pending.busy());
    }

    /// The second Ctrl+O while the first picker is up is declined, not queued.
    #[test]
    fn a_second_ask_is_declined_while_one_is_open() {
        let (mut pending, _send) = opened(Why::Open);
        let accepted = pending.ask(Request::open_file(), Parent::default(), Why::Save);
        assert!(!accepted);
        assert!(pending.busy());
    }

    /// The answer is delivered once. A frame loop polls every frame and must
    /// not act on the same choice sixty times a second.
    #[test]
    fn an_answer_is_delivered_once() {
        let (mut pending, send) = opened(Why::Open);
        send.send(Some(PathBuf::from("a.buzz"))).unwrap();
        assert!(pending.poll().is_some());
        assert!(pending.poll().is_none());
    }

    /// A directory that has been deleted since it was remembered must not be
    /// handed to the backend.
    #[test]
    fn a_missing_start_folder_is_dropped() {
        let request = Request::open_file().directory("no-such-folder-anywhere-here");
        assert!(
            request.directory.is_some(),
            "the request still records what was asked for"
        );
        // `show` filters it; this asserts the predicate that does so, since
        // `show` itself cannot run without a person to answer it.
        assert!(!request.directory.unwrap().is_dir());
    }
}
