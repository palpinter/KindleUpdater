//! Remembers which version of each source file was last uploaded.
//!
//! MTP does not preserve modification times (the Kindle stamps the upload time),
//! so the target alone only tells us the file size. The state lets us notice a
//! source file that changed without changing its size.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::tree::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub size: u64,
    pub mtime: u64,
}

impl From<FileEntry> for Record {
    fn from(entry: FileEntry) -> Self {
        Record {
            size: entry.size,
            mtime: entry.mtime,
        }
    }
}

/// Relative path → the source file version that is on the Kindle.
pub type FolderState = BTreeMap<String, Record>;

/// Folder states keyed by the target directory URI (which includes the device serial).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    folders: BTreeMap<String, FolderState>,
}

impl State {
    pub fn path() -> Option<PathBuf> {
        dirs::state_dir().map(|dir| dir.join("kindle-updater").join("state.json"))
    }

    pub fn load() -> Result<State> {
        let Some(path) = Self::path() else {
            return Ok(State::default());
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(err) => return Err(err).with_context(|| format!("{}", path.display())),
        };
        serde_json::from_str(&text).with_context(|| format!("{}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        let dir = path.parent().expect("state path has a parent");
        fs::create_dir_all(dir).with_context(|| format!("{}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("{}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("{}", path.display()))
    }

    pub fn folder(&self, key: &str) -> FolderState {
        self.folders.get(key).cloned().unwrap_or_default()
    }

    pub fn set_folder(&mut self, key: &str, folder: FolderState) {
        self.folders.insert(key.to_owned(), folder);
    }
}
