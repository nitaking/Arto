//! Editing the document a window is reading.
//!
//! Arto is a reader first, so an edit never goes through the rendered page:
//! the editor works on the Markdown source exactly as it is on disk, and the
//! page beside it is a preview of that source. What a design doc says is what
//! its bytes say, and a round trip through HTML would be a chance to change
//! them without anyone asking.
//!
//! Keeping the file whole is the point of every module here:
//!
//! | Module | What it guarantees |
//! |--------|--------------------|
//! | `source` | The bytes that come back out are the bytes that went in: BOM and line endings are recorded on the way in and restored on the way out |
//! | `disk` | A save never overwrites a version it has not seen, is read back before it is reported, and never touches the disk when nothing changed |
//! | `drafts` | An edit that has not been saved survives leaving the document, closing the window, or the app going away mid-write |
//! | `session` | One place decides what an edit, a save, or a change on disk does to the buffer, so no path through the UI can skip a check |
//!
//! Nothing in this module knows about Dioxus; the window's side of it is
//! `state::app_state::editor` and `components::content::editor_pane`.

mod disk;
mod drafts;
mod session;
mod source;

pub use disk::{read_snapshot, save, SaveError};
pub use drafts::DRAFTS;
pub use session::{Conflict, EditSession, Notice, SaveRequest};
