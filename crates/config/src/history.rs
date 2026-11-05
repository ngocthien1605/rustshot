//! The capture history: the screenshots, annotated copies and recordings RustShot saved, newest
//! first, kept in `%APPDATA%\RustShot\history.toml` for the History window. Pure data and file
//! I/O, so it is unit-tested on any platform; the app records entries and shows the list.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// What kind of file an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Screenshot,
    /// A copy saved from the annotation editor.
    Annotated,
    Video,
    Gif,
}

impl EntryKind {
    /// The kind of a finished recording, from its file name.
    pub fn for_recording(path: &Path) -> EntryKind {
        let gif = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
        if gif {
            EntryKind::Gif
        } else {
            EntryKind::Video
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EntryKind::Screenshot => "Screenshot",
            EntryKind::Annotated => "Annotated",
            EntryKind::Video => "Video",
            EntryKind::Gif => "GIF",
        }
    }

    /// A still image that can be previewed, annotated, pinned and copied (a GIF shows its first
    /// frame).
    pub fn is_image(self) -> bool {
        !matches!(self, EntryKind::Video)
    }
}

/// One saved capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Local time it was saved, `YYYY-MM-DD HH:MM:SS`.
    pub taken: String,
}

impl Entry {
    /// The line shown in the History list: time, kind, file name.
    pub fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        format!("{}   {:<10}   {name}", self.taken, self.kind.label())
    }
}

/// The history, newest entry first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    #[serde(default, rename = "entry")]
    pub entries: Vec<Entry>,
}

impl History {
    /// Put `entry` first. An older entry for the same file is dropped (the file was saved
    /// again), and only the newest `max` entries are kept (at least one).
    pub fn add(&mut self, entry: Entry, max: usize) {
        self.entries.retain(|e| e.path != entry.path);
        self.entries.insert(0, entry);
        self.entries.truncate(max.max(1));
    }

    /// Forget the entry for `path` (the file is untouched). Returns true if there was one.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.path != path);
        self.entries.len() != before
    }

    /// Drop the entries whose file no longer exists (moved or deleted outside RustShot).
    /// `exists` is injected so this is testable without files. Returns how many were dropped.
    pub fn retain_existing(&mut self, exists: impl Fn(&Path) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| exists(&e.path));
        before - self.entries.len()
    }

    pub fn from_toml(s: &str, path: &Path) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load from `path`; a missing file is an empty history.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Save to `path` atomically (temp file, then rename), creating the folder if needed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        crate::write_atomically(path, &self.to_toml()?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            path: PathBuf::from(format!("/shots/{name}")),
            kind,
            taken: "2026-11-05 14:03:12".into(),
        }
    }

    #[test]
    fn newest_first_capped_and_without_duplicates() {
        let mut h = History::default();
        for n in ["a.png", "b.png", "c.png"] {
            h.add(entry(n, EntryKind::Screenshot), 10);
        }
        let names = |h: &History| -> Vec<String> {
            h.entries
                .iter()
                .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&h), ["c.png", "b.png", "a.png"]);
        h.add(entry("a.png", EntryKind::Annotated), 10); // saved again: moves to the top
        assert_eq!(names(&h), ["a.png", "c.png", "b.png"]);
        assert_eq!(h.entries[0].kind, EntryKind::Annotated);
        h.add(entry("d.png", EntryKind::Screenshot), 2);
        assert_eq!(names(&h), ["d.png", "a.png"], "only the newest 2 kept");
        h.add(entry("e.png", EntryKind::Screenshot), 0);
        assert_eq!(names(&h), ["e.png"], "never fewer than one");
    }

    #[test]
    fn remove_and_retain_existing() {
        let mut h = History::default();
        for n in ["a.png", "b.mp4", "c.gif"] {
            h.add(entry(n, EntryKind::Screenshot), 10);
        }
        assert!(h.remove(Path::new("/shots/b.mp4")));
        assert!(!h.remove(Path::new("/shots/b.mp4")), "already gone");
        let dropped = h.retain_existing(|p| p.ends_with("c.gif"));
        assert_eq!(dropped, 1);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].path, PathBuf::from("/shots/c.gif"));
    }

    #[test]
    fn toml_round_trip_and_readable_layout() {
        let mut h = History::default();
        h.add(entry("a.png", EntryKind::Screenshot), 10);
        h.add(entry("clip.gif", EntryKind::Gif), 10);
        let text = h.to_toml().unwrap();
        assert!(text.contains("[[entry]]"), "{text}");
        assert!(text.contains("kind = \"gif\""), "{text}");
        assert_eq!(History::from_toml(&text, Path::new("h.toml")).unwrap(), h);
        assert_eq!(
            History::from_toml("", Path::new("h.toml")).unwrap(),
            History::default()
        );
        assert!(History::from_toml("entry = 5", Path::new("h.toml")).is_err());
    }

    #[test]
    fn save_then_load_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("rustshot_history_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("history.toml");
        assert_eq!(History::load_from(&path).unwrap(), History::default());
        let mut h = History::default();
        h.add(entry("a.png", EntryKind::Video), 10);
        h.save_to(&path).unwrap();
        assert_eq!(History::load_from(&path).unwrap(), h);
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "no temp file left"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinds_labels_and_recording_kind() {
        assert_eq!(
            EntryKind::for_recording(Path::new("x/clip.GIF")),
            EntryKind::Gif
        );
        assert_eq!(
            EntryKind::for_recording(Path::new("x/clip.mp4")),
            EntryKind::Video
        );
        assert!(EntryKind::Gif.is_image() && EntryKind::Annotated.is_image());
        assert!(!EntryKind::Video.is_image());
        let label = entry("RustShot_1.png", EntryKind::Screenshot).label();
        assert_eq!(label, "2026-11-05 14:03:12   Screenshot   RustShot_1.png");
    }
}
