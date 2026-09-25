//! Unsaved edits, kept on disk until they are saved or thrown away.
//!
//! An edit lives in the window until it is saved, and a window is easy to
//! lose: a link followed, the document closed, the app quit, the machine
//! restarted. So the buffer is written here as it changes, one file per
//! document, and the editor offers it back the next time that document is
//! edited.
//!
//! A draft also carries the fingerprint of the version it was started from.
//! Coming back to a draft of a file that has since changed on disk is then
//! recognised as the conflict it is, instead of the draft quietly winning.
//!
//! Drafts are Arto's own files, in its data directory, so unlike the document
//! itself they are written the atomic way: to a temporary file beside the
//! draft, then renamed over it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::disk::Fingerprint;
use super::source::{LineEnding, SourceFormat};

/// Where this installation keeps its drafts.
pub static DRAFTS: LazyLock<DraftStore> = LazyLock::new(|| DraftStore::new(default_dir()));

fn default_dir() -> PathBuf {
    if let Some(mut path) = dirs::data_local_dir() {
        path.push("arto");
        path.push("drafts");
        return path;
    }
    if let Some(mut path) = dirs::home_dir() {
        path.push(".arto");
        path.push("drafts");
        return path;
    }
    PathBuf::from("arto-drafts")
}

/// An edit of `path` that has not been saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub path: PathBuf,
    /// The version of the file the edit started from; `None` when the file
    /// did not exist.
    pub base: Option<Fingerprint>,
    pub text: String,
    pub format: SourceFormat,
    /// Seconds since the Unix epoch.
    pub saved_at: u64,
}

impl Draft {
    pub fn new(
        path: impl Into<PathBuf>,
        base: Option<Fingerprint>,
        text: impl Into<String>,
        format: SourceFormat,
    ) -> Self {
        Self {
            path: path.into(),
            base,
            text: text.into(),
            format,
            saved_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// The draft file's layout. Versioned so a later format can refuse an older
/// one rather than misread it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftFile {
    version: u32,
    path: PathBuf,
    base: Option<String>,
    text: String,
    crlf: bool,
    bom: bool,
    mixed: bool,
    saved_at: u64,
}

const VERSION: u32 = 1;

#[derive(Debug)]
pub struct DraftStore {
    dir: PathBuf,
}

impl DraftStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The draft file for `path`: named by a hash of the path, so any path
    /// maps to a valid file name and two documents never share one.
    fn file_for(&self, path: &Path) -> PathBuf {
        let digest = Sha256::digest(path.as_os_str().as_encoded_bytes());
        let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.json"))
    }

    /// Write `draft` durably, replacing any earlier draft of the same file.
    pub fn save(&self, draft: &Draft) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let file = DraftFile {
            version: VERSION,
            path: draft.path.clone(),
            base: draft.base.map(Fingerprint::to_hex),
            text: draft.text.clone(),
            crlf: draft.format.line_ending == LineEnding::Crlf,
            bom: draft.format.bom,
            mixed: draft.format.mixed,
            saved_at: draft.saved_at,
        };
        let json = serde_json::to_vec(&file).map_err(io::Error::other)?;

        let target = self.file_for(&draft.path);
        let temp = target.with_extension(format!("json.{}.tmp", std::process::id()));
        {
            let mut out = fs::File::create(&temp)?;
            out.write_all(&json)?;
            out.sync_all()?;
        }
        fs::rename(&temp, &target).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }

    /// The draft of `path`, if there is a readable one.
    ///
    /// A draft that cannot be read — another version's format, a truncated
    /// file — is moved aside rather than deleted, so that a clean edit of the
    /// same file, which removes the draft, cannot throw away something that
    /// might still be recovered by hand.
    pub fn load(&self, path: &Path) -> Option<Draft> {
        let draft_file = self.file_for(path);
        let bytes = fs::read(&draft_file).ok()?;
        let parsed = serde_json::from_slice::<DraftFile>(&bytes)
            .ok()
            .filter(|file| file.version == VERSION && file.path == path)
            .and_then(
                |file| match file.base.as_deref().map(Fingerprint::from_hex) {
                    Some(None) => None,
                    Some(Some(base)) => Some((file, Some(base))),
                    None => Some((file, None)),
                },
            );
        let Some((file, base)) = parsed else {
            let aside = draft_file.with_extension("unreadable.json");
            tracing::warn!(?path, ?aside, "Moving an unreadable draft aside");
            let _ = fs::rename(&draft_file, &aside);
            return None;
        };
        Some(Draft {
            path: file.path,
            base,
            text: file.text,
            format: SourceFormat {
                bom: file.bom,
                line_ending: if file.crlf {
                    LineEnding::Crlf
                } else {
                    LineEnding::Lf
                },
                mixed: file.mixed,
            },
            saved_at: file.saved_at,
        })
    }

    #[cfg(test)]
    pub fn exists(&self, path: &Path) -> bool {
        self.file_for(path).exists()
    }

    /// Forget the draft of `path`. A draft that is already gone is fine.
    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(self.file_for(path)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, DraftStore) {
        let dir = TempDir::new().unwrap();
        let store = DraftStore::new(dir.path().join("drafts"));
        (dir, store)
    }

    #[test]
    fn a_draft_comes_back_as_it_was_saved() {
        let (_dir, store) = store();
        let path = Path::new("/docs/設計書.md");
        let draft = Draft::new(
            path,
            Some(Fingerprint::of(b"base")),
            "# 設計\n",
            SourceFormat {
                bom: true,
                line_ending: LineEnding::Crlf,
                mixed: false,
            },
        );
        store.save(&draft).unwrap();
        assert!(store.exists(path));
        assert_eq!(store.load(path), Some(draft));
    }

    #[test]
    fn a_later_draft_replaces_the_earlier_one() {
        let (_dir, store) = store();
        let path = Path::new("/docs/a.md");
        store
            .save(&Draft::new(path, None, "one", SourceFormat::default()))
            .unwrap();
        store
            .save(&Draft::new(path, None, "two", SourceFormat::default()))
            .unwrap();
        assert_eq!(store.load(path).unwrap().text, "two");
    }

    #[test]
    fn drafts_of_different_files_are_kept_apart() {
        let (_dir, store) = store();
        let a = Path::new("/docs/a.md");
        let b = Path::new("/docs/b.md");
        store
            .save(&Draft::new(a, None, "a", SourceFormat::default()))
            .unwrap();
        assert_eq!(store.load(b), None);
    }

    #[test]
    fn removing_a_draft_that_is_not_there_is_fine() {
        let (_dir, store) = store();
        let path = Path::new("/docs/a.md");
        store.remove(path).unwrap();
        store
            .save(&Draft::new(path, None, "a", SourceFormat::default()))
            .unwrap();
        store.remove(path).unwrap();
        assert!(!store.exists(path));
    }

    #[test]
    fn an_unreadable_draft_is_ignored_but_kept_aside() {
        let (_dir, store) = store();
        let path = Path::new("/docs/a.md");
        fs::create_dir_all(&store.dir).unwrap();
        fs::write(store.file_for(path), b"{ not json").unwrap();

        assert_eq!(store.load(path), None);
        // Removing the draft, as a clean edit does, leaves the unreadable one.
        store.remove(path).unwrap();
        let aside = store.file_for(path).with_extension("unreadable.json");
        assert_eq!(fs::read(aside).unwrap(), b"{ not json");
    }
}
