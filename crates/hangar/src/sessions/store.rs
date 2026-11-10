//! On-disk session state: `<home>/sessions/<id>/meta.json` and
//! `events.jsonl`. Files are private to the user (they hold transcripts).

use super::{Event, SessionMeta};
use crate::util::{append_private, make_private, write_private};
use std::path::{Path, PathBuf};

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(home: &Path) -> Store {
        let root = home.join("sessions");
        let _ = std::fs::create_dir_all(&root);
        make_private(&root);
        Store { root }
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn create(&self, id: &str) -> std::io::Result<()> {
        let dir = self.dir(id);
        std::fs::create_dir_all(&dir)?;
        make_private(&dir);
        Ok(())
    }

    pub fn save_meta(&self, meta: &SessionMeta) {
        if let Ok(bytes) = serde_json::to_vec_pretty(meta)
            && let Err(e) = write_private(&self.dir(&meta.id).join("meta.json"), &bytes)
        {
            tracing::warn!(session = %meta.id, "could not save session: {e}");
        }
    }

    pub fn append(&self, id: &str, event: &Event) {
        if let Ok(line) = serde_json::to_string(event)
            && let Err(e) = append_private(&self.dir(id).join("events.jsonl"), &line)
        {
            tracing::warn!(session = %id, "could not save an event: {e}");
        }
    }

    /// Events with `seq > after`, oldest first.
    pub fn events(&self, id: &str, after: u64) -> Vec<Event> {
        let text = std::fs::read_to_string(self.dir(id).join("events.jsonl")).unwrap_or_default();
        text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.seq > after).collect()
    }

    /// Every saved session with the next free event number.
    pub fn load_all(&self) -> Vec<(SessionMeta, u64)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|e| {
                let meta: SessionMeta = serde_json::from_str(&std::fs::read_to_string(e.path().join("meta.json")).ok()?).ok()?;
                // Only trust ids that match their folder (and our id format).
                if meta.id != e.file_name().to_string_lossy() || !super::valid_id(&meta.id) {
                    return None;
                }
                let last = self.events(&meta.id, 0).last().map_or(0, |ev| ev.seq);
                Some((meta, last + 1))
            })
            .collect()
    }

    pub fn remove(&self, id: &str) {
        if super::valid_id(id) {
            let _ = std::fs::remove_dir_all(self.dir(id));
        }
    }
}
