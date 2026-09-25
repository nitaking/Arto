//! Reading a file for editing, and writing it back without losing anything.
//!
//! # What a save promises
//!
//! 1. **It never overwrites what it has not seen.** Every save names the
//!    version it was derived from by [`Fingerprint`]; the file is read again
//!    immediately before writing, and a file that no longer has that
//!    fingerprint is a [`SaveError::Conflict`], not a write. Another editor, a
//!    `git checkout`, a sync client — whatever changed the file, the change is
//!    shown to the writer rather than silently replaced.
//! 2. **It does not touch a file that would not change.** Saving the version
//!    already on disk writes nothing, so the file's modification time, and
//!    every tool that watches it, is left alone.
//! 3. **It is read back before it is reported.** The file is synced and read
//!    again, and a save whose bytes did not arrive is an error, not a success.
//!
//! The write goes into the existing file rather than replacing it with a new
//! one. Replacing is the usual way to make a write atomic, but it gives the
//! document a new inode: hard links stop pointing at it, extended attributes,
//! ACLs and ownership that were not copied are gone, and a watcher holding the
//! old inode — Arto's own, on Linux — stops hearing about the file at all.
//! What replacing buys, a file that is never half written, is covered instead
//! by the draft (see `drafts`), which is on disk before the write starts and is
//! removed only after the write has been read back.

use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::source::{self, SourceFormat};

/// The SHA-256 of a file's bytes: which version of it an edit started from.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 || !hex.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({}…)", &self.to_hex()[..12])
    }
}

/// A version of a file as it was read: its text for the editor, how to write
/// that text back, and which bytes it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub fingerprint: Fingerprint,
    pub text: String,
    pub format: SourceFormat,
}

impl Snapshot {
    /// A snapshot of `bytes`, or an error if they are not UTF-8 text.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let raw = std::str::from_utf8(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the file is not UTF-8 text, so it cannot be edited without changing it",
            )
        })?;
        let (text, format) = source::decode(raw);
        Ok(Self {
            fingerprint: Fingerprint::of(bytes),
            text,
            format,
        })
    }
}

/// Read `path` as it is now.
///
/// `Ok(None)` is a file that does not exist, which is a state an edit has to
/// know about (it was deleted under the editor) rather than a failure.
pub fn read_snapshot(path: &Path) -> io::Result<Option<Snapshot>> {
    match fs::read(path) {
        Ok(bytes) => Snapshot::from_bytes(&bytes).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A save that went through.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The fingerprint of what is on disk now.
    pub fingerprint: Fingerprint,
    /// Whether the file had to be written at all.
    pub written: bool,
}

#[derive(Debug)]
pub enum SaveError {
    /// The file is not the version the edit started from. `None` is a file
    /// that is no longer there.
    Conflict(Option<Snapshot>),
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(Some(_)) => f.write_str("the file was changed by something else"),
            Self::Conflict(None) => f.write_str("the file was removed"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Write `text` to `path`, provided the file is still the version `expected`.
///
/// `expected` is `None` when the edit knows the file is absent (it was removed
/// and the writer chose to put it back); the file is then created, and a file
/// that has appeared in the meantime is a conflict like any other.
pub fn save(
    path: &Path,
    expected: Option<Fingerprint>,
    text: &str,
    format: &SourceFormat,
) -> Result<Saved, SaveError> {
    let current = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };

    let current_fingerprint = current.as_deref().map(Fingerprint::of);
    if current_fingerprint != expected {
        let theirs = current.as_deref().map(Snapshot::from_bytes).transpose()?;
        return Err(SaveError::Conflict(theirs));
    }

    let bytes = source::encode(text, format);
    let fingerprint = Fingerprint::of(&bytes);
    if current_fingerprint == Some(fingerprint) {
        return Ok(Saved {
            fingerprint,
            written: false,
        });
    }

    {
        // A file that was expected to be absent is created only if it still
        // is: one that appeared since the check above is not truncated.
        let mut file = if expected.is_none() {
            OpenOptions::new().write(true).create_new(true).open(path)
        } else {
            OpenOptions::new().write(true).truncate(true).open(path)
        }?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }

    let written = fs::read(path)?;
    if written != bytes {
        return Err(SaveError::Io(io::Error::other(
            "the file did not read back as it was written",
        )));
    }

    Ok(Saved {
        fingerprint,
        written: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::source::LineEnding;
    use tempfile::TempDir;

    fn setup(content: &[u8]) -> (TempDir, std::path::PathBuf, Snapshot) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("design.md");
        fs::write(&path, content).unwrap();
        let snapshot = read_snapshot(&path).unwrap().unwrap();
        (dir, path, snapshot)
    }

    #[test]
    fn fingerprints_round_trip_through_hex() {
        let fp = Fingerprint::of(b"hello");
        assert_eq!(Fingerprint::from_hex(&fp.to_hex()), Some(fp));
        assert_eq!(Fingerprint::from_hex("zz"), None);
        assert_eq!(Fingerprint::from_hex(&"g".repeat(64)), None);
    }

    #[test]
    fn a_save_writes_the_text_in_the_files_own_format() {
        let (_dir, path, snap) = setup(b"a\r\nb\r\n");
        assert_eq!(snap.format.line_ending, LineEnding::Crlf);

        let saved = save(&path, Some(snap.fingerprint), "a\nchanged\n", &snap.format).unwrap();
        assert!(saved.written);
        assert_eq!(fs::read(&path).unwrap(), b"a\r\nchanged\r\n");
        assert_eq!(saved.fingerprint, Fingerprint::of(b"a\r\nchanged\r\n"));
    }

    #[test]
    fn saving_what_is_already_there_writes_nothing() {
        let (_dir, path, snap) = setup(b"same\n");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let saved = save(&path, Some(snap.fingerprint), &snap.text, &snap.format).unwrap();
        assert!(!saved.written);
        assert_eq!(saved.fingerprint, snap.fingerprint);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn a_file_changed_since_it_was_read_is_not_overwritten() {
        let (_dir, path, snap) = setup(b"original\n");
        fs::write(&path, b"someone else\n").unwrap();

        let err = save(&path, Some(snap.fingerprint), "mine\n", &snap.format).unwrap_err();
        match err {
            SaveError::Conflict(Some(theirs)) => assert_eq!(theirs.text, "someone else\n"),
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(fs::read(&path).unwrap(), b"someone else\n");
    }

    #[test]
    fn a_removed_file_is_a_conflict_not_a_recreation() {
        let (_dir, path, snap) = setup(b"original\n");
        fs::remove_file(&path).unwrap();

        let err = save(&path, Some(snap.fingerprint), "mine\n", &snap.format).unwrap_err();
        assert!(matches!(err, SaveError::Conflict(None)));
        assert!(!path.exists());
    }

    #[test]
    fn a_removed_file_is_recreated_only_when_asked() {
        let (_dir, path, snap) = setup(b"original\n");
        fs::remove_file(&path).unwrap();

        save(&path, None, "mine\n", &snap.format).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"mine\n");
    }

    #[test]
    fn a_file_that_appeared_is_a_conflict_for_a_writer_expecting_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.md");
        fs::write(&path, b"surprise\n").unwrap();

        let err = save(&path, None, "mine\n", &SourceFormat::default()).unwrap_err();
        assert!(matches!(err, SaveError::Conflict(Some(_))));
    }

    #[test]
    fn a_non_utf8_file_cannot_be_opened_for_editing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bin.md");
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let err = read_snapshot(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_missing_file_reads_as_none() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_snapshot(&dir.path().join("nope.md")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_keeps_its_identity_and_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let (_dir, path, snap) = setup(b"x\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&path).unwrap();

        save(&path, Some(snap.fingerprint), "y\n", &snap.format).unwrap();

        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.permissions().mode() & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_document_is_written_through_the_link() {
        let (dir, target, snap) = setup(b"x\n");
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        save(&link, Some(snap.fingerprint), "y\n", &snap.format).unwrap();

        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"y\n");
    }
}
