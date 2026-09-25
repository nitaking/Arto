//! The document's source, beside the page that previews it.
//!
//! The pane is a view of `AppState::editor` and nothing more: every button
//! calls an `AppState` method, which asks the session, which decides. What is
//! drawn here is chosen so that the state of the file is never in doubt —
//! whether there is anything unsaved, which format it will be written in, and,
//! when the file and the buffer have parted, a question that has to be
//! answered before the next save.

use dioxus::document;
use dioxus::prelude::*;
use std::time::Duration;

use crate::components::icon::{Icon, IconName};
use crate::editor::{Conflict, EditSession, Notice};
use crate::state::AppState;

/// How long the buffer has to stand still before it is written as a draft.
const DRAFT_SETTLE: Duration = Duration::from_millis(800);
/// The longest an unsaved edit goes without a draft while typing never
/// pauses long enough to settle.
const DRAFT_MAX_WAIT: Duration = Duration::from_secs(3);

/// What the pane draws, taken out of the session so that a keystroke which
/// changes none of it does not redraw the pane.
#[derive(Debug, Clone, PartialEq)]
struct PaneView {
    dirty: bool,
    saving: bool,
    conflict: Option<ConflictView>,
    notice: Option<Notice>,
    confirming_close: bool,
    format: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ConflictView {
    Changed,
    Removed,
}

impl PaneView {
    fn of(session: &EditSession) -> Self {
        let format = session.format();
        let mut parts = vec!["UTF-8"];
        if format.bom {
            parts.push("BOM");
        }
        parts.push(format.line_ending.label());
        Self {
            dirty: session.is_dirty(),
            saving: session.is_saving(),
            conflict: session.conflict().map(|conflict| match conflict {
                Conflict::Changed(_) => ConflictView::Changed,
                Conflict::Removed => ConflictView::Removed,
            }),
            notice: session.notice().cloned(),
            confirming_close: session.is_confirming_close(),
            format: parts.join(" · "),
        }
    }
}

fn notice_text(notice: &Notice) -> Option<String> {
    Some(match notice {
        Notice::RestoredDraft => {
            "Restored unsaved edits from a draft left the last time this file was edited.".into()
        }
        Notice::ReloadedFromDisk => {
            "The file changed on disk; the editor now shows the new version.".into()
        }
        Notice::MixedLineEndings(ending) => format!(
            "This file mixes line endings. Saving will write {} throughout.",
            ending.label()
        ),
        Notice::SaveFailed(reason) => {
            format!("Could not save: {reason}. Your edits are kept as a draft.")
        }
        Notice::ReadFailed(reason) => format!("Could not read the file: {reason}."),
        // The status line already says so.
        Notice::Saved => return None,
    })
}

#[component]
pub fn EditorPane() -> Element {
    let mut state = use_context::<AppState>();

    let view = use_memo(move || state.editor.read().as_ref().map(PaneView::of));
    // Which buffer the editor view is showing: the session, and the
    // generation of its buffer. See `EditSession::view_key`.
    let view_key = use_memo(move || {
        state
            .editor
            .read()
            .as_ref()
            .map(EditSession::view_key)
            .unwrap_or_default()
    });
    let revision = use_memo(move || {
        state
            .editor
            .read()
            .as_ref()
            .map(EditSession::revision)
            .unwrap_or_default()
    });

    // Keep the draft on disk as the buffer settles, and at least every few
    // seconds while it does not.
    let mut last_draft = use_signal(std::time::Instant::now);
    use_effect(move || {
        let revision = revision();
        spawn(async move {
            tokio::time::sleep(DRAFT_SETTLE).await;
            let current = state.editor.peek().as_ref().map(EditSession::revision);
            let settled = current == Some(revision);
            if settled || last_draft.peek().elapsed() >= DRAFT_MAX_WAIT {
                state.keep_draft();
                last_draft.set(std::time::Instant::now());
            }
        });
    });
    // And once more on the way out, whatever the way out is.
    use_drop(move || state.keep_draft());

    let Some(view) = view() else {
        return rsx! {};
    };

    let status = if view.saving {
        "Saving…"
    } else if view.conflict.is_some() {
        "Changed on disk"
    } else if view.dirty {
        "Unsaved changes"
    } else {
        "Saved"
    };
    let can_save = view.dirty && view.conflict.is_none() && !view.saving;

    rsx! {
        section {
            class: "editor-pane",
            "aria-label": "Markdown source",

            div {
                class: "editor-toolbar",
                span {
                    class: "editor-status",
                    class: if view.dirty { "dirty" },
                    class: if view.conflict.is_some() { "conflict" },
                    span { class: "editor-status-dot", "aria-hidden": "true" }
                    "{status}"
                }
                span { class: "editor-format", title: "Encoding and line endings the file is written with", "{view.format}" }
                div { class: "editor-toolbar-spacer" }
                if can_save {
                    button {
                        class: "editor-button primary",
                        title: "Save",
                        onclick: move |_| state.save_document(),
                        Icon { name: IconName::DeviceFloppy }
                        "Save"
                    }
                }
                button {
                    class: "editor-button",
                    title: "Back to reading",
                    onclick: move |_| state.stop_editing(),
                    Icon { name: IconName::Eye }
                    "Done"
                }
            }

            if let Some(conflict) = view.conflict {
                div {
                    class: "editor-banner conflict",
                    role: "alert",
                    p {
                        match conflict {
                            ConflictView::Changed => "This file was changed outside Arto since you started editing. Nothing has been overwritten. Choose which version to keep before saving.",
                            ConflictView::Removed => "This file was removed or renamed outside Arto. Your edits are safe here; keeping them lets the next save write the file again.",
                        }
                    }
                    div {
                        class: "editor-banner-actions",
                        if conflict == ConflictView::Changed {
                            button {
                                class: "editor-button",
                                onclick: move |_| state.take_disk_version(),
                                "Use the file on disk"
                            }
                        }
                        button {
                            class: "editor-button primary",
                            title: "The next save replaces the file on disk with your text",
                            onclick: move |_| state.keep_my_edits(),
                            "Keep my edits"
                        }
                        button {
                            class: "editor-button",
                            title: "Copy your text, to compare or merge by hand",
                            onclick: move |_| {
                                if let Some(session) = state.editor.peek().as_ref() {
                                    crate::utils::clipboard::copy_text(session.buffer());
                                }
                                crate::keybindings::dispatcher::show_action_feedback("Copied");
                            },
                            Icon { name: IconName::Copy }
                            "Copy my text"
                        }
                    }
                }
            } else if view.confirming_close {
                div {
                    class: "editor-banner confirm",
                    role: "alert",
                    p { "You have unsaved changes." }
                    div {
                        class: "editor-banner-actions",
                        button {
                            class: "editor-button primary",
                            onclick: move |_| state.save_and_stop_editing(),
                            "Save"
                        }
                        button {
                            class: "editor-button danger",
                            onclick: move |_| state.discard_edits(),
                            "Discard changes"
                        }
                        button {
                            class: "editor-button",
                            onclick: move |_| state.cancel_stop_editing(),
                            "Keep editing"
                        }
                    }
                }
            }

            if let Some(text) = view.notice.as_ref().and_then(notice_text) {
                div {
                    class: "editor-banner notice",
                    class: if matches!(view.notice, Some(Notice::SaveFailed(_) | Notice::ReadFailed(_))) { "error" },
                    role: "status",
                    p { "{text}" }
                    div {
                        class: "editor-banner-actions",
                        if view.notice == Some(Notice::RestoredDraft) && view.conflict.is_none() {
                            button {
                                class: "editor-button",
                                title: "Throw the draft away and edit the file as it is on disk",
                                onclick: move |_| state.revert_edits(),
                                "Discard draft"
                            }
                        }
                        button {
                            class: "editor-button icon-only",
                            title: "Dismiss",
                            "aria-label": "Dismiss",
                            onclick: move |_| state.dismiss_editor_notice(),
                            Icon { name: IconName::Close }
                        }
                    }
                }
            }

            // Keyed by view: a buffer replaced from outside (the disk's version
            // taken, a draft discarded) is a new editor, and the text the
            // reader is typing is never written over from here.
            for key in std::iter::once(view_key()) {
                div {
                    key: "{key}",
                    class: "editor-host",
                    "data-editor-key": "{key}",
                    onmounted: move |_| {
                        let key = key.clone();
                        // The text to start from, read without subscribing:
                        // between generations the editor owns its text.
                        let text = state
                            .editor
                            .peek()
                            .as_ref()
                            .filter(|session| session.view_key() == key)
                            .map(|session| session.buffer().to_string());
                        if let Some(text) = text {
                            spawn(run_editor_view(state, key, text));
                        }
                    },
                }
            }
        }
    }
}

/// Mount the editor in the host keyed `key`, and carry what is typed into the
/// session for as long as that view lives.
///
/// Each change arrives as the whole text, under the key this channel was
/// opened for; `AppState::edit_buffer` drops it if the session has since
/// moved on to another view.
async fn run_editor_view(mut state: AppState, key: String, text: String) {
    let mut eval = document::eval(
        r#"
        const [key, text] = await dioxus.recv();
        while (!window.Arto?.editor?.mount) {
            await new Promise((resolve) => setTimeout(resolve, 10));
        }
        const host = document.querySelector(`.editor-host[data-editor-key="${key}"]`);
        if (host) {
            window.Arto.editor.mount(host, text, key, (current) => dioxus.send(current));
        }
        "#,
    );
    if eval.send((key.as_str(), text)).is_err() {
        tracing::warn!(key, "Could not hand the text to the editor");
        return;
    }
    while let Ok(text) = eval.recv::<String>().await {
        state.edit_buffer(&key, text);
    }
}
