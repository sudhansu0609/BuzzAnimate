//! What a script run is given besides the document (JSFL_COMMANDS_PLAN J6):
//! Trace Bitmap, the Bold trace the editor's own Trace Bitmap makes.
//!
//! The dialogs are the other half of [`buzz_script::Hooks`]; they come from the
//! app's window ([`crate::jsfl_dialog`]) or from a test.

use std::sync::Arc;

/// `document.traceBitmap()`: the selected pictures traced with
/// `TraceOptions::bold()` (keeps the pictures' textures) and the user's saved
/// tuning, as Trace Bitmap does.
pub fn trace_hook() -> buzz_script::TraceFn {
    Arc::new(|scene, _layer, frame, ids| {
        crate::editor::trace_pictures_in_scene(
            scene,
            frame,
            ids,
            buzz_scene::TraceOptions::bold().with_saved_tuning(),
        )
    })
}

/// Hooks for a run with no window to ask in: Trace Bitmap, and `ask` if given.
pub fn hooks(ask: Option<buzz_script::AskFn>) -> buzz_script::Hooks {
    buzz_script::Hooks { ask, trace: Some(trace_hook()) }
}
