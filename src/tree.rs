use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow};
use gio::prelude::*;

/// Kindle keeps reading positions, highlights and notes of sideloaded books
/// in a `<book name without extension>.sdr` directory next to the book.
pub const SIDECAR_SUFFIX: &str = ".sdr";

const ATTRIBUTES: &str = "standard::name,standard::type,standard::size,time::modified";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileEntry {
    pub size: u64,
    pub mtime: u64,
}

/// Snapshot of a directory tree. Paths are relative to the scanned root and use `/`.
/// Hidden entries (names starting with `.`) are ignored on both sides.
#[derive(Debug, Default)]
pub struct Tree {
    pub files: BTreeMap<String, FileEntry>,
    pub dirs: BTreeSet<String>,
    pub sidecars: BTreeSet<String>,
}

impl Tree {
    pub fn total_size(&self) -> u64 {
        self.files.values().map(|f| f.size).sum()
    }
}

/// Scans `root` recursively. A missing root yields an empty tree.
/// With `detect_sidecars`, `.sdr` directories are collected separately and not descended into.
pub fn scan(root: &gio::File, detect_sidecars: bool) -> Result<Tree> {
    let mut tree = Tree::default();
    if !root.query_exists(gio::Cancellable::NONE) {
        return Ok(tree);
    }

    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        let handle = if dir.is_empty() {
            root.clone()
        } else {
            root.resolve_relative_path(&dir)
        };
        let children = handle
            .enumerate_children(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )
            .with_context(|| format!("nem sikerült beolvasni: {}", handle.uri()))?;

        for info in children {
            let info = info.with_context(|| format!("nem sikerült beolvasni: {}", handle.uri()))?;
            let name = info
                .name()
                .into_os_string()
                .into_string()
                .map_err(|name| anyhow!("nem UTF-8 fájlnév itt: {}: {name:?}", handle.uri()))?;
            if name.starts_with('.') {
                continue;
            }
            let rel = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };

            match info.file_type() {
                gio::FileType::Directory if detect_sidecars && name.ends_with(SIDECAR_SUFFIX) => {
                    tree.sidecars.insert(rel);
                }
                gio::FileType::Directory => {
                    tree.dirs.insert(rel.clone());
                    pending.push(rel);
                }
                gio::FileType::Regular => {
                    let entry = FileEntry {
                        size: info.size().max(0) as u64,
                        mtime: info.attribute_uint64("time::modified"),
                    };
                    tree.files.insert(rel, entry);
                }
                // Symlinks, sockets and the like are not books.
                _ => {}
            }
        }
    }
    Ok(tree)
}
