//! Records saved captures in the capture history (`config::history`) and reads it back for the
//! History window. Best effort: the capture itself already worked, so a history that cannot be
//! read or written is logged, never shown to the user.

use std::path::Path;

use config::history::{Entry, EntryKind, History};

/// Add a saved file to the history (when the history is turned on), and refresh the History
/// window if it is open.
pub fn record(path: &Path, kind: EntryKind) {
    let Some((enabled, max)) =
        crate::dispatch::with_app(|a| (a.settings.history.enabled, a.settings.history.max_entries))
    else {
        return;
    };
    if !enabled {
        return;
    }
    let t = imaging::Timestamp::now_local();
    let entry = Entry {
        path: path.to_path_buf(),
        kind,
        taken: format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        ),
    };
    update(|h| h.add(entry, max as usize));
    crate::ui::history::refresh();
}

/// The history as saved; empty (and logged) if it cannot be read.
pub fn load() -> History {
    let loaded = config::paths::history_file().map(|p| History::load_from(&p));
    match loaded {
        Ok(Ok(h)) => h,
        Ok(Err(e)) | Err(e) => {
            tracing::warn!("capture history unavailable: {e}");
            History::default()
        }
    }
}

/// The history without the entries whose file no longer exists; saved back only if any were
/// dropped (so opening the History window does not rewrite the file for nothing).
pub fn load_existing() -> History {
    let mut h = load();
    if h.retain_existing(|p| p.exists()) > 0 {
        let saved = config::paths::history_file().and_then(|p| h.save_to(&p));
        if let Err(e) = saved {
            tracing::warn!("capture history not saved: {e}");
        }
    }
    h
}

/// Load, change and save the history. An unreadable file is replaced by the changed empty one.
pub fn update(change: impl FnOnce(&mut History)) {
    let mut h = load();
    change(&mut h);
    let saved = config::paths::history_file().and_then(|p| h.save_to(&p));
    if let Err(e) = saved {
        tracing::warn!("capture history not saved: {e}");
    }
}
