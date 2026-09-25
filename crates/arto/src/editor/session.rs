//! One document being edited: the buffer, the version it came from, and every
//! way the two can part.
//!
//! This is the one place that decides what happens to an edit. The window
//! calls in with what happened — the reader typed, a save finished, the file
//! changed on disk — and reads back what to show. Keeping those decisions in
//! a plain struct is what lets them be tested without a window, and what
//! stops a second path through the UI from forgetting one of them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::disk::{Fingerprint, SaveError, Saved, Snapshot};
use super::drafts::Draft;
use super::source::{LineEnding, SourceFormat};

/// Why the buffer and the file disagree in a way only the writer can settle.
#[derive(Debug, Clone, PartialEq)]
pub enum Conflict {
    /// The file on disk is a version the buffer was not derived from.
    Changed(Snapshot),
    /// The file is no longer there.
    Removed,
}

/// Something worth telling the writer that needs no decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// The buffer was restored from an unsaved draft.
    RestoredDraft,
    /// The file changed on disk and, having no edits of its own, the buffer
    /// took the new version.
    ReloadedFromDisk,
    /// The file mixes line endings; saving will write this one throughout.
    MixedLineEndings(LineEnding),
    Saved,
    /// The save did not happen, and why.
    SaveFailed(String),
    /// The file could not be read to check it against the buffer.
    ReadFailed(String),
}

/// What a save needs, taken from the session when the save starts.
///
/// The text is copied rather than borrowed: the reader goes on typing while
/// the file is written, and what the session records as saved has to be what
/// was actually written, not what the buffer holds when the write returns.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveRequest {
    pub path: PathBuf,
    pub expected: Option<Fingerprint>,
    pub text: String,
    pub format: SourceFormat,
}

/// Where session ids come from: unique for the life of the process, so that
/// two sessions — of two files, or of one file edited twice — are never
/// mistaken for each other by a view still holding the earlier one.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
pub struct EditSession {
    id: u64,
    path: PathBuf,
    /// The version on disk the buffer is an edit of; `None` for a file that
    /// is not there.
    base: Option<Fingerprint>,
    /// That version's text, where it is known. It is not known for a draft
    /// restored over a file that has changed since, and a buffer with no
    /// known base is always unsaved.
    base_text: Option<String>,
    format: SourceFormat,
    buffer: String,
    /// Bumped on every change to the buffer, so a preview or a draft writer
    /// can tell whether what it rendered or wrote is still current.
    revision: u64,
    /// Bumped when the buffer is replaced from outside the editor — a reload,
    /// taking the disk's version — so the editor knows to show the new text
    /// rather than keep what it has.
    generation: u64,
    conflict: Option<Conflict>,
    notice: Option<Notice>,
    saving: bool,
    /// The file changed while a save was being written. What was read then
    /// may be the save itself, half-written; the check is made again once the
    /// save has returned.
    recheck: bool,
    /// Leaving was asked for with unsaved edits, and is waiting on an answer.
    confirm_close: bool,
}

impl EditSession {
    /// Begin editing `path`, as it is on disk now, picking up `draft` if one
    /// was left behind.
    pub fn open(path: impl Into<PathBuf>, disk: Snapshot, draft: Option<Draft>) -> Self {
        let mut session = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            path: path.into(),
            base: Some(disk.fingerprint),
            base_text: Some(disk.text.clone()),
            format: disk.format,
            buffer: disk.text.clone(),
            revision: 0,
            generation: 0,
            conflict: None,
            notice: disk
                .format
                .mixed
                .then_some(Notice::MixedLineEndings(disk.format.line_ending)),
            saving: false,
            recheck: false,
            confirm_close: false,
        };

        if let Some(draft) = draft.filter(|draft| draft.text != disk.text) {
            session.buffer = draft.text;
            session.notice = Some(Notice::RestoredDraft);
            if draft.base != Some(disk.fingerprint) {
                // The draft was an edit of some other version. Its text is
                // what the writer last saw, so it is what is shown; the file
                // as it is now is the conflict to settle before saving.
                session.base = draft.base;
                session.base_text = None;
                session.format = draft.format;
                session.conflict = Some(Conflict::Changed(disk));
            }
        }
        session
    }

    #[cfg(test)]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The name the editor view showing this buffer is mounted under: the
    /// session and the generation of its buffer. A change reported under any
    /// other key is from a view that is no longer this buffer's, and is
    /// ignored.
    pub fn view_key(&self) -> String {
        format!("{}-{}", self.id, self.generation)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn format(&self) -> SourceFormat {
        self.format
    }

    pub fn conflict(&self) -> Option<&Conflict> {
        self.conflict.as_ref()
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn is_confirming_close(&self) -> bool {
        self.confirm_close
    }

    /// Whether the buffer holds anything the file does not.
    pub fn is_dirty(&self) -> bool {
        self.base_text.as_deref() != Some(self.buffer.as_str())
    }

    /// The reader changed the text.
    pub fn edit(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text != self.buffer {
            self.buffer = text;
            self.revision += 1;
            if matches!(self.notice, Some(Notice::Saved | Notice::ReloadedFromDisk)) {
                self.notice = None;
            }
        }
    }

    /// The file on disk changed; `None` is a file that was removed.
    pub fn disk_changed(&mut self, disk: Option<Snapshot>) {
        if self.saving {
            self.recheck = true;
            return;
        }
        match disk {
            // What this session last wrote or read. A watcher hears the
            // editor's own saves too, and those are not news; a conflict
            // raised by a version that has since gone away is not either.
            Some(disk) if Some(disk.fingerprint) == self.base => self.conflict = None,
            // Someone wrote exactly what the buffer holds. Nothing to settle.
            Some(disk) if disk.text == self.buffer => self.adopt(disk, false),
            Some(disk) if !self.is_dirty() => {
                self.adopt(disk, true);
                self.notice = Some(if self.format.mixed {
                    Notice::MixedLineEndings(self.format.line_ending)
                } else {
                    Notice::ReloadedFromDisk
                });
            }
            Some(disk) => self.conflict = Some(Conflict::Changed(disk)),
            None => {
                if self.base.is_some() {
                    self.conflict = Some(Conflict::Removed);
                }
            }
        }
    }

    /// Settle a conflict in the disk's favour: the buffer becomes the file.
    pub fn take_theirs(&mut self) {
        match self.conflict.take() {
            Some(Conflict::Changed(disk)) => {
                self.adopt(disk, true);
                self.notice = None;
            }
            // A removed file has no version to take.
            other => self.conflict = other,
        }
    }

    /// Settle a conflict in the buffer's favour: the next save may overwrite
    /// the file as it is now (or put a removed one back).
    ///
    /// The format is the file's current one, so that a change of line
    /// endings made on disk is not reverted by an edit of the words.
    pub fn keep_mine(&mut self) {
        match self.conflict.take() {
            Some(Conflict::Changed(disk)) => {
                self.base = Some(disk.fingerprint);
                self.base_text = Some(disk.text);
                self.format = disk.format;
                if disk.format.mixed {
                    self.notice = Some(Notice::MixedLineEndings(disk.format.line_ending));
                }
            }
            Some(Conflict::Removed) => {
                self.base = None;
                self.base_text = None;
            }
            None => {}
        }
    }

    /// Start a save. `None` while a conflict is unsettled or a save is
    /// already under way: a save must never be what settles a conflict.
    pub fn begin_save(&mut self) -> Option<SaveRequest> {
        if self.conflict.is_some() || self.saving {
            return None;
        }
        self.saving = true;
        Some(SaveRequest {
            path: self.path.clone(),
            expected: self.base,
            text: self.buffer.clone(),
            format: self.format,
        })
    }

    /// A save started by [`Self::begin_save`] came back.
    pub fn finish_save(&mut self, request: SaveRequest, result: Result<Saved, SaveError>) {
        self.saving = false;
        match result {
            Ok(saved) => {
                self.base = Some(saved.fingerprint);
                self.base_text = Some(request.text);
                self.format.mixed = false;
                self.notice = Some(Notice::Saved);
                if matches!(&self.conflict, Some(Conflict::Changed(disk)) if disk.fingerprint == saved.fingerprint)
                {
                    self.conflict = None;
                }
            }
            Err(SaveError::Conflict(Some(disk))) if disk.text == request.text => {
                // Something else wrote the same text: the save is done, and
                // the file is theirs — line endings included.
                self.adopt(disk, false);
                self.notice = Some(Notice::Saved);
            }
            Err(SaveError::Conflict(Some(disk))) => {
                self.conflict = Some(Conflict::Changed(disk));
            }
            Err(SaveError::Conflict(None)) => self.conflict = Some(Conflict::Removed),
            Err(SaveError::Io(e)) => self.notice = Some(Notice::SaveFailed(e.to_string())),
        }
    }

    /// Whether the file has to be checked again now that a save is done.
    pub fn take_recheck(&mut self) -> bool {
        std::mem::take(&mut self.recheck)
    }

    /// The unsaved edit to keep on disk, if there is one.
    pub fn draft(&self) -> Option<Draft> {
        self.is_dirty()
            .then(|| Draft::new(&self.path, self.base, &self.buffer, self.format))
    }

    /// Ask to stop editing. `true` when there is nothing to lose; otherwise
    /// the session waits for [`Self::cancel_close`] or the caller discarding.
    pub fn request_close(&mut self) -> bool {
        if self.is_dirty() {
            self.confirm_close = true;
            false
        } else {
            true
        }
    }

    pub fn cancel_close(&mut self) {
        self.confirm_close = false;
    }

    /// Throw the edits away: the buffer becomes the version it was an edit
    /// of. Only possible while that version's text is known and nothing is
    /// in conflict; otherwise settling the conflict is the way back.
    pub fn revert(&mut self) -> bool {
        if self.conflict.is_some() {
            return false;
        }
        let Some(base) = self.base_text.clone() else {
            return false;
        };
        if base != self.buffer {
            self.buffer = base;
            self.revision += 1;
            self.generation += 1;
        }
        self.notice = None;
        self.confirm_close = false;
        true
    }

    /// The file could not be read to compare it with the buffer. The buffer is
    /// left as it is; the next check or save tries again.
    pub fn read_failed(&mut self, reason: impl Into<String>) {
        self.notice = Some(Notice::ReadFailed(reason.into()));
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    fn adopt(&mut self, disk: Snapshot, replace_buffer: bool) {
        self.base = Some(disk.fingerprint);
        self.format = disk.format;
        if replace_buffer && self.buffer != disk.text {
            self.buffer = disk.text.clone();
            self.revision += 1;
            self.generation += 1;
        }
        self.base_text = Some(disk.text);
        self.conflict = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn snap(text: &str) -> Snapshot {
        Snapshot::from_bytes(text.as_bytes()).unwrap()
    }

    fn open(text: &str) -> EditSession {
        EditSession::open("/docs/design.md", snap(text), None)
    }

    fn saved(text: &str) -> Saved {
        Saved {
            fingerprint: Fingerprint::of(text.as_bytes()),
            written: true,
        }
    }

    #[test]
    fn a_fresh_session_is_the_file() {
        let s = open("# A\n");
        assert_eq!(s.buffer(), "# A\n");
        assert!(!s.is_dirty());
        assert_eq!(s.conflict(), None);
        assert_eq!(s.draft(), None);
    }

    #[test]
    fn typing_makes_it_dirty_and_undoing_makes_it_clean() {
        let mut s = open("a\n");
        s.edit("ab\n");
        assert!(s.is_dirty());
        assert_eq!(s.revision(), 1);
        s.edit("a\n");
        assert!(!s.is_dirty());
        // The same text again is not a change.
        s.edit("a\n");
        assert_eq!(s.revision(), 2);
    }

    #[test]
    fn a_save_records_what_was_written_not_what_was_typed_since() {
        let mut s = open("a\n");
        s.edit("b\n");
        let request = s.begin_save().unwrap();
        assert!(s.begin_save().is_none(), "one save at a time");
        s.edit("bc\n");
        s.finish_save(request, Ok(saved("b\n")));

        assert!(!s.is_saving());
        assert!(s.is_dirty(), "the text typed during the save is not saved");
        let next = s.begin_save().unwrap();
        assert_eq!(next.expected, Some(Fingerprint::of(b"b\n")));
    }

    #[test]
    fn the_echo_of_its_own_save_is_ignored() {
        let mut s = open("a\n");
        s.edit("b\n");
        let request = s.begin_save().unwrap();
        s.finish_save(request, Ok(saved("b\n")));
        s.edit("bc\n");

        s.disk_changed(Some(snap("b\n")));
        assert_eq!(s.buffer(), "bc\n");
        assert_eq!(s.conflict(), None);
    }

    #[test]
    fn a_clean_buffer_follows_the_disk() {
        let mut s = open("a\n");
        let generation = s.generation();
        s.disk_changed(Some(snap("changed\n")));
        assert_eq!(s.buffer(), "changed\n");
        assert!(!s.is_dirty());
        assert_eq!(s.generation(), generation + 1);
        assert_eq!(s.notice(), Some(&Notice::ReloadedFromDisk));
    }

    #[test]
    fn a_dirty_buffer_is_never_replaced_by_the_disk() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\n")));
        assert_eq!(s.buffer(), "mine\n");
        assert_eq!(s.conflict(), Some(&Conflict::Changed(snap("theirs\n"))));
        assert!(s.begin_save().is_none(), "a conflict blocks saving");
    }

    #[test]
    fn a_disk_change_to_the_buffers_own_text_is_no_conflict() {
        let mut s = open("a\n");
        s.edit("same\n");
        s.disk_changed(Some(snap("same\n")));
        assert_eq!(s.conflict(), None);
        assert!(!s.is_dirty());
    }

    #[test]
    fn taking_theirs_replaces_the_buffer() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\n")));
        s.take_theirs();
        assert_eq!(s.buffer(), "theirs\n");
        assert!(!s.is_dirty());
        assert_eq!(s.conflict(), None);
    }

    #[test]
    fn keeping_mine_lets_the_next_save_overwrite_exactly_that_version() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\n")));
        s.keep_mine();
        assert_eq!(s.buffer(), "mine\n");
        assert!(s.is_dirty());
        let request = s.begin_save().unwrap();
        assert_eq!(request.expected, Some(Fingerprint::of(b"theirs\n")));
    }

    #[test]
    fn keeping_mine_adopts_the_disks_line_endings() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\r\n")));
        s.keep_mine();
        assert_eq!(s.format().line_ending, LineEnding::Crlf);
    }

    #[test]
    fn a_removed_file_must_be_put_back_on_purpose() {
        let mut s = open("a\n");
        s.disk_changed(None);
        assert_eq!(s.conflict(), Some(&Conflict::Removed));
        assert!(s.begin_save().is_none());

        s.take_theirs();
        assert_eq!(s.conflict(), Some(&Conflict::Removed), "nothing to take");

        s.keep_mine();
        let request = s.begin_save().unwrap();
        assert_eq!(request.expected, None);
    }

    #[test]
    fn a_file_that_comes_back_as_it_was_settles_its_removal() {
        let mut s = open("a\n");
        s.disk_changed(None);
        s.disk_changed(Some(snap("a\n")));
        assert_eq!(s.conflict(), None);
    }

    #[test]
    fn a_save_that_finds_a_changed_file_becomes_a_conflict() {
        let mut s = open("a\n");
        s.edit("mine\n");
        let request = s.begin_save().unwrap();
        s.finish_save(request, Err(SaveError::Conflict(Some(snap("theirs\n")))));
        assert_eq!(s.conflict(), Some(&Conflict::Changed(snap("theirs\n"))));
        assert!(s.is_dirty());
    }

    #[test]
    fn a_failed_write_keeps_the_edit_and_says_why() {
        let mut s = open("a\n");
        s.edit("mine\n");
        let request = s.begin_save().unwrap();
        s.finish_save(request, Err(SaveError::Io(io::Error::other("disk full"))));
        assert!(s.is_dirty());
        assert_eq!(s.notice(), Some(&Notice::SaveFailed("disk full".into())));
        assert!(s.draft().is_some());
    }

    #[test]
    fn a_draft_of_the_same_version_is_restored() {
        let disk = snap("a\n");
        let draft = Draft::new(
            "/docs/design.md",
            Some(disk.fingerprint),
            "draft\n",
            disk.format,
        );
        let s = EditSession::open("/docs/design.md", disk, Some(draft));
        assert_eq!(s.buffer(), "draft\n");
        assert!(s.is_dirty());
        assert_eq!(s.conflict(), None);
        assert_eq!(s.notice(), Some(&Notice::RestoredDraft));
    }

    #[test]
    fn a_draft_of_an_older_version_is_restored_as_a_conflict() {
        let old = Fingerprint::of(b"old\n");
        let draft = Draft::new("/docs/design.md", Some(old), "draft\n", Default::default());
        let s = EditSession::open("/docs/design.md", snap("new\n"), Some(draft));
        assert_eq!(s.buffer(), "draft\n");
        assert_eq!(s.conflict(), Some(&Conflict::Changed(snap("new\n"))));
    }

    #[test]
    fn a_draft_that_matches_the_disk_is_not_news() {
        let disk = snap("same\n");
        let draft = Draft::new("/docs/design.md", None, "same\n", disk.format);
        let s = EditSession::open("/docs/design.md", disk, Some(draft));
        assert!(!s.is_dirty());
        assert_eq!(s.notice(), None);
    }

    #[test]
    fn leaving_with_unsaved_edits_asks_first() {
        let mut s = open("a\n");
        assert!(s.request_close());
        s.edit("b\n");
        assert!(!s.request_close());
        assert!(s.is_confirming_close());
        s.cancel_close();
        assert!(!s.is_confirming_close());
    }

    #[test]
    fn reverting_returns_to_the_version_the_edit_started_from() {
        let disk = snap("a\n");
        let draft = Draft::new(
            "/docs/design.md",
            Some(disk.fingerprint),
            "draft\n",
            disk.format,
        );
        let mut s = EditSession::open("/docs/design.md", disk, Some(draft));
        let generation = s.generation();
        assert!(s.revert());
        assert_eq!(s.buffer(), "a\n");
        assert!(!s.is_dirty());
        assert_eq!(s.generation(), generation + 1);
    }

    #[test]
    fn reverting_is_not_a_way_around_a_conflict() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\n")));
        assert!(!s.revert());
        assert_eq!(s.buffer(), "mine\n");
    }

    #[test]
    fn a_change_heard_during_a_save_is_checked_after_it() {
        let mut s = open("a\n");
        s.edit("b\n");
        let request = s.begin_save().unwrap();
        s.edit("bc\n");
        // The watcher reads the file mid-write: empty, or the save itself.
        s.disk_changed(Some(snap("")));
        assert_eq!(s.conflict(), None, "nothing is decided while writing");
        s.finish_save(request, Ok(saved("b\n")));
        assert!(s.take_recheck());
        assert!(!s.take_recheck());

        // The recheck then finds the save, which is no conflict.
        s.disk_changed(Some(snap("b\n")));
        assert_eq!(s.conflict(), None);
        assert_eq!(s.buffer(), "bc\n");
    }

    #[test]
    fn a_conflict_goes_away_when_the_file_returns_to_the_base() {
        let mut s = open("a\n");
        s.edit("mine\n");
        s.disk_changed(Some(snap("theirs\n")));
        s.disk_changed(Some(snap("a\n")));
        assert_eq!(s.conflict(), None);
        assert_eq!(s.buffer(), "mine\n");
    }

    #[test]
    fn a_save_that_meets_the_same_text_takes_the_files_format() {
        let mut s = open("a\n");
        s.edit("b\n");
        let request = s.begin_save().unwrap();
        s.finish_save(request, Err(SaveError::Conflict(Some(snap("b\r\n")))));
        assert_eq!(s.conflict(), None);
        assert_eq!(s.format().line_ending, LineEnding::Crlf);
        assert_eq!(
            s.begin_save().map(|r| r.expected),
            Some(Some(Fingerprint::of(b"b\r\n")))
        );
    }

    #[test]
    fn every_session_has_its_own_id() {
        assert_ne!(open("a\n").id(), open("a\n").id());
    }

    #[test]
    fn a_buffer_replaced_from_outside_has_a_new_view_key() {
        let mut s = open("a\n");
        let key = s.view_key();
        s.edit("b\n");
        assert_eq!(s.view_key(), key, "typing keeps the view");
        s.disk_changed(Some(snap("c\n")));
        s.take_theirs();
        assert_ne!(s.view_key(), key, "the disk's text is a new view");
    }

    #[test]
    fn mixed_line_endings_are_announced() {
        let s = open("a\r\nb\n");
        assert_eq!(s.notice(), Some(&Notice::MixedLineEndings(LineEnding::Lf)));
    }
}
