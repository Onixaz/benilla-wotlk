//! The host half of text input: [`keymap`] maps chords per OS and [`clipboard`] holds the OS
//! pasteboard, both under the UI's EditBox dispatcher, which fires the Lua handlers.

use bevy::prelude::*;

pub(crate) mod clipboard;
pub(crate) mod keymap;

pub(crate) use clipboard::{wayland_display, HostClipboard};

/// Adds the process-wide OS pasteboard.
pub(crate) struct TextInputPlugin;

impl Plugin for TextInputPlugin {
    fn build(&self, app: &mut App) {
        // Held for the whole run: on X11 dropping the handle clears the clipboard. `NonSend`: no
        // backend is `Sync`, and NSPasteboard is main-thread-only.
        app.init_non_send_resource::<HostClipboard>();
    }
}
