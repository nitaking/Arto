//! The window's side of editing: starting and stopping, saving, and hearing
//! about the file.
//!
//! Every decision about the buffer is [`EditSession`]'s. What is here is the
//! plumbing around it — reading and writing the disk off the UI's back, the
//! draft kept beside each unsaved edit, and the scroll position that should
//! survive the page switching between the file and the buffer.
//!
//! # Testing note
//!
//! These methods read and write Dioxus `Signal`s, so they are exercised
//! through the app; the behaviour they route to is tested in `crate::editor`.

use dioxus::document;
use dioxus::prelude::*;
use std::io;
use std::path::PathBuf;

use crate::editor::{self, EditSession, SaveError, SaveRequest, DRAFTS};
use crate::keybindings::dispatcher::show_action_feedback;
use crate::state::{AppState, DocumentContent};
use crate::utils::task::spawn_detached;

impl AppState {
    /// Edit the document, or ask to go back to reading it.
    pub fn toggle_editing(&mut self) {
        if self.editor.peek().is_some() {
            self.stop_editing();
        } else {
            self.start_editing();
        }
    }

    /// Open the document's source beside it, picking up a draft left from
    /// an earlier edit.
    pub fn start_editing(&mut self) {
        if self.editor.peek().is_some() {
            return;
        }
        let file = match &self.document.peek().content {
            DocumentContent::File(file) => file.clone(),
            _ => {
                show_action_feedback("Nothing to edit");
                return;
            }
        };
        let this_window = dioxus::desktop::window().id();
        if let Some((mut other, visible)) = crate::window::main::window_editing(&file, this_window)
        {
            if visible {
                if let Some(id) = crate::window::main::window_of(&other) {
                    crate::window::main::focus_window(id);
                }
                show_action_feedback("Already being edited in another window");
                return;
            }
            // A closed first window is only hidden on macOS. Its edit is
            // handed over as a draft, which this window then picks up.
            other.keep_draft();
            other.editor.set(None);
        }
        match editor::read_snapshot(&file) {
            Ok(Some(disk)) => {
                let draft = DRAFTS.load(&file);
                tracing::info!(?file, restored = draft.is_some(), "Editing document");
                self.editor.set(Some(EditSession::open(file, disk, draft)));
            }
            Ok(None) => show_action_feedback("The file is no longer there"),
            Err(e) => {
                tracing::warn!(?file, %e, "Cannot edit document");
                show_action_feedback(&format!("Cannot edit: {e}"));
            }
        }
    }

    /// Go back to reading. With unsaved edits this only asks: the session
    /// shows the question, and [`Self::save_and_stop_editing`] or
    /// [`Self::discard_edits`] answer it.
    ///
    /// The editor's text is collected first, so a keystroke still on its way
    /// is not mistaken for no change at all.
    pub fn stop_editing(&mut self) {
        let mut state = *self;
        spawn_detached(async move {
            state.flush_editor().await;
            state.stop_editing_now();
        });
    }

    fn stop_editing_now(&mut self) {
        let can_close = match self.editor.write().as_mut() {
            Some(session) if session.conflict().is_some() => {
                show_action_feedback("Choose which version to keep first");
                return;
            }
            Some(session) => session.request_close(),
            None => return,
        };
        if can_close {
            self.end_editing(true);
        }
    }

    /// Throw the unsaved edits away and go back to reading the file.
    pub fn discard_edits(&mut self) {
        let path = self.editing_path();
        if let Some(path) = path {
            if let Err(e) = DRAFTS.remove(&path) {
                tracing::warn!(?path, %e, "Failed to remove draft");
            }
        }
        self.editor.set(None);
        self.return_to_reading_here();
    }

    /// The document moved on to another file while it was being edited.
    ///
    /// Nothing is asked: the edit is kept as a draft and offered back the next
    /// time that file is edited, which loses nothing and does not stand
    /// between the reader and the link they followed.
    pub fn suspend_editing(&mut self) {
        let mut state = *self;
        spawn_detached(async move {
            state.flush_editor().await;
            state.suspend_editing_now();
        });
    }

    fn suspend_editing_now(&mut self) {
        self.keep_draft();
        let dirty = self
            .editor
            .peek()
            .as_ref()
            .is_some_and(EditSession::is_dirty);
        self.editor.set(None);
        if dirty {
            show_action_feedback("Unsaved edits kept as a draft");
        }
    }

    pub fn save_document(&mut self) {
        let mut state = *self;
        spawn_detached(async move {
            state.flush_editor().await;
            state.save_then(false);
        });
    }

    pub fn save_and_stop_editing(&mut self) {
        let mut state = *self;
        spawn_detached(async move {
            state.flush_editor().await;
            state.save_then(true);
        });
    }

    /// The editor mounted under `view_key` changed its text.
    ///
    /// A key that is not the session's current one is a view that no longer
    /// shows this buffer — an earlier generation, or another session's — and
    /// what it says is dropped rather than written into the wrong buffer.
    pub fn edit_buffer(&mut self, view_key: &str, text: String) {
        let current = self.editor.peek().as_ref().map(EditSession::view_key);
        if current.as_deref() != Some(view_key) {
            tracing::debug!(view_key, "Ignoring an edit from a stale editor view");
            return;
        }
        if let Some(session) = self.editor.write().as_mut() {
            session.edit(text);
        }
    }

    /// Take the editor's text as it is this moment into the session.
    ///
    /// Every change is reported as it happens, but a report is a message, and
    /// a save or a close that is decided on the Rust side could otherwise
    /// overtake the keystroke before it. This asks the editor directly.
    pub async fn flush_editor(&mut self) {
        #[derive(serde::Deserialize)]
        struct EditorSnapshot {
            key: String,
            text: String,
        }
        if self.editor.peek().is_none() {
            return;
        }
        let snapshot = document::eval("return window.Arto?.editor?.snapshot?.() ?? null;")
            .join::<Option<EditorSnapshot>>()
            .await;
        match snapshot {
            Ok(Some(snapshot)) => self.edit_buffer(&snapshot.key, snapshot.text),
            Ok(None) => {}
            Err(e) => tracing::warn!(?e, "Could not read the editor's text before acting on it"),
        }
    }

    /// Compare the buffer with the file as it is now: the watcher heard a
    /// change, or the reader asked to reload.
    pub fn check_editor_against_disk(&mut self) {
        let Some(path) = self.editing_path() else {
            return;
        };
        let result = editor::read_snapshot(&path);
        if let Some(session) = self.editor.write().as_mut() {
            match result {
                Ok(disk) => session.disk_changed(disk),
                Err(e) => session.read_failed(e.to_string()),
            }
        }
    }

    pub fn take_disk_version(&mut self) {
        if let Some(session) = self.editor.write().as_mut() {
            session.take_theirs();
        }
    }

    pub fn keep_my_edits(&mut self) {
        if let Some(session) = self.editor.write().as_mut() {
            session.keep_mine();
        }
    }

    /// Put the buffer back to the file as it was when the edit started, and
    /// forget the draft.
    pub fn revert_edits(&mut self) {
        let reverted = self
            .editor
            .write()
            .as_mut()
            .is_some_and(EditSession::revert);
        if reverted {
            self.keep_draft();
        }
    }

    pub fn cancel_stop_editing(&mut self) {
        if let Some(session) = self.editor.write().as_mut() {
            session.cancel_close();
        }
    }

    pub fn dismiss_editor_notice(&mut self) {
        if let Some(session) = self.editor.write().as_mut() {
            session.dismiss_notice();
        }
    }

    /// Put the unsaved edit on disk, or remove the draft if there is none.
    ///
    /// Called as the buffer settles and when the editor goes away. It uses
    /// `try_peek` because the last call comes while the window is being torn
    /// down, when the signal may already be gone.
    pub fn keep_draft(&self) {
        let Ok(guard) = self.editor.try_peek() else {
            return;
        };
        let Some(session) = guard.as_ref() else {
            return;
        };
        let result = match session.draft() {
            Some(draft) => DRAFTS.save(&draft),
            None => DRAFTS.remove(session.path()),
        };
        if let Err(e) = result {
            tracing::warn!(path = ?session.path(), %e, "Failed to keep draft");
        }
    }

    fn editing_path(&self) -> Option<PathBuf> {
        self.editor
            .peek()
            .as_ref()
            .map(|session| session.path().to_path_buf())
    }

    fn end_editing(&mut self, keep_place: bool) {
        self.keep_draft();
        self.editor.set(None);
        if keep_place {
            self.return_to_reading_here();
        }
    }

    /// The page goes back to the file; ask it to open where it is now rather
    /// than at the top. See `handle_scroll_anchor` in the file viewer.
    fn return_to_reading_here(&mut self) {
        let here = *self.current_scroll_anchor.peek();
        self.pending_scroll_anchor.set(Some(here));
    }

    fn save_then(&mut self, stop: bool) {
        let request: Option<SaveRequest> = {
            let mut guard = self.editor.write();
            let Some(session) = guard.as_mut() else {
                return;
            };
            if session.conflict().is_some() {
                session.cancel_close();
                drop(guard);
                show_action_feedback("Settle the change on disk before saving");
                return;
            }
            if !session.is_dirty() {
                drop(guard);
                if stop {
                    self.end_editing(true);
                }
                return;
            }
            if session.is_saving() {
                drop(guard);
                show_action_feedback("Still saving");
                return;
            }
            // On disk before the file is touched, so that a write that never
            // finishes leaves the edit recoverable.
            if let Some(draft) = session.draft() {
                if let Err(e) = DRAFTS.save(&draft) {
                    tracing::warn!(path = ?session.path(), %e, "Failed to keep draft before saving");
                }
            }
            session.begin_save()
        };
        let Some(request) = request else {
            return;
        };

        let mut state = *self;
        spawn_detached(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let job = request.clone();
            std::thread::spawn(move || {
                let _ = tx.send(editor::save(
                    &job.path,
                    job.expected,
                    &job.text,
                    &job.format,
                ));
            });
            let result = rx.await.unwrap_or_else(|_| {
                Err(SaveError::Io(io::Error::other("the save was interrupted")))
            });
            let saved = result.is_ok();
            if let Err(e) = &result {
                tracing::warn!(path = ?request.path, %e, "Save did not go through");
            }

            let path = request.path.clone();
            let (still_dirty, recheck) = {
                let mut guard = state.editor.write();
                match guard.as_mut() {
                    Some(session) if session.path() == path => {
                        session.finish_save(request, result);
                        if !saved {
                            session.cancel_close();
                        }
                        (session.is_dirty(), session.take_recheck())
                    }
                    // The session ended while the file was being written.
                    // The draft is left: opening the file again finds it
                    // identical to the disk and drops it.
                    _ => return,
                }
            };

            if recheck {
                state.check_editor_against_disk();
            }

            if saved {
                if !still_dirty {
                    if let Err(e) = DRAFTS.remove(&path) {
                        tracing::warn!(?path, %e, "Failed to remove draft after saving");
                    }
                }
                show_action_feedback("Saved");
                if stop && !still_dirty {
                    state.end_editing(true);
                }
            }
        });
    }
}
