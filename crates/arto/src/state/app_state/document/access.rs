//! Reading and updating the document a window is showing.
//!
//! # Testing note
//!
//! These methods read and write Dioxus `Signal`s, which panic outside a
//! running Dioxus runtime, so they are exercised through the app rather than
//! by unit tests. What can be tested without one lives on [`Document`]
//! itself, in the parent module.

use super::Document;
use crate::state::AppState;
use dioxus::prelude::*;
use std::path::PathBuf;

impl AppState {
    /// The document on screen, as a clone.
    pub fn document(&self) -> Document {
        self.document.read().clone()
    }

    /// The file on screen, if what is shown came from one.
    pub fn current_file(&self) -> Option<PathBuf> {
        self.document.read().file().map(|file| file.to_path_buf())
    }

    /// Change the document in place.
    pub fn update_document<F>(&mut self, update_fn: F)
    where
        F: FnOnce(&mut Document),
    {
        update_fn(&mut self.document.write());
    }

    /// Read the document again from disk.
    ///
    /// Nothing about the document changes here; the viewer watches the
    /// counter and re-reads the file when it moves.
    ///
    /// While the source is being edited the page shows the buffer, and a
    /// reload is a check of the buffer against the file instead: the session
    /// takes the new version if nothing is unsaved, and asks otherwise.
    pub fn reload_document(&mut self) {
        if self.editor.peek().is_some() {
            self.check_editor_against_disk();
            return;
        }
        let current = *self.reload_trigger.read();
        self.reload_trigger.set(current + 1);
    }
}
