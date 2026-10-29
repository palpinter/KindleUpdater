use std::collections::BTreeSet;

use crate::state::{FolderState, Record};
use crate::tree::{SIDECAR_SUFFIX, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    New,
    SizeChanged,
    /// Same size on the Kindle, but the source changed since the last upload.
    Modified,
}

#[derive(Debug)]
pub struct Upload {
    pub rel: String,
    pub size: u64,
    pub old_size: Option<u64>,
    pub reason: Reason,
}

/// What it takes to make the target an exact copy of the source.
/// Sidecars of books that stay on the Kindle are kept; orphaned ones are deleted.
#[derive(Debug, Default)]
pub struct Plan {
    pub uploads: Vec<Upload>,
    pub delete_files: Vec<(String, u64)>,
    /// Deleted recursively; nothing inside them is listed separately.
    pub delete_dirs: Vec<String>,
    pub delete_sidecars: Vec<String>,
    pub create_dirs: Vec<String>,
    pub unchanged: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.uploads.is_empty()
            && self.delete_files.is_empty()
            && self.delete_dirs.is_empty()
            && self.delete_sidecars.is_empty()
            && self.create_dirs.is_empty()
    }

    pub fn upload_bytes(&self) -> u64 {
        self.uploads.iter().map(|u| u.size).sum()
    }

    /// Space released by deleted and overwritten files.
    pub fn freed_bytes(&self) -> u64 {
        let deleted: u64 = self.delete_files.iter().map(|(_, size)| size).sum();
        let replaced: u64 = self.uploads.iter().filter_map(|u| u.old_size).sum();
        deleted + replaced
    }

    pub fn new_uploads(&self) -> impl Iterator<Item = &Upload> {
        self.uploads.iter().filter(|u| u.reason == Reason::New)
    }

    pub fn updates(&self) -> impl Iterator<Item = &Upload> {
        self.uploads.iter().filter(|u| u.reason != Reason::New)
    }
}

pub fn build(source: &Tree, target: &Tree, state: &FolderState) -> Plan {
    let mut plan = Plan::default();

    for (rel, src) in &source.files {
        let old = target.files.get(rel);
        let reason = match old {
            None => Some(Reason::New),
            Some(dst) if dst.size != src.size => Some(Reason::SizeChanged),
            Some(_) => match state.get(rel) {
                Some(record) if *record != Record::from(*src) => Some(Reason::Modified),
                _ => None,
            },
        };
        match reason {
            Some(reason) => plan.uploads.push(Upload {
                rel: rel.clone(),
                size: src.size,
                old_size: old.map(|f| f.size),
                reason,
            }),
            None => plan.unchanged += 1,
        }
    }

    // BTreeSet order puts every directory before its contents.
    for dir in &target.dirs {
        if !source.dirs.contains(dir) && !within_any(dir, &plan.delete_dirs) {
            plan.delete_dirs.push(dir.clone());
        }
    }
    for (rel, dst) in &target.files {
        if !source.files.contains_key(rel) && !within_any(rel, &plan.delete_dirs) {
            plan.delete_files.push((rel.clone(), dst.size));
        }
    }

    let books: BTreeSet<&str> = source
        .files
        .keys()
        .map(|rel| strip_extension(rel))
        .collect();
    for sidecar in &target.sidecars {
        let book = &sidecar[..sidecar.len() - SIDECAR_SUFFIX.len()];
        if !books.contains(book) && !within_any(sidecar, &plan.delete_dirs) {
            plan.delete_sidecars.push(sidecar.clone());
        }
    }

    plan.create_dirs = source.dirs.difference(&target.dirs).cloned().collect();
    plan
}

fn within_any(path: &str, dirs: &[String]) -> bool {
    dirs.iter().any(|dir| {
        path.strip_prefix(dir.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// `a/Book.azw3` → `a/Book`; the extension is only looked for in the file name.
fn strip_extension(rel: &str) -> &str {
    let name_start = rel.rfind('/').map_or(0, |i| i + 1);
    match rel[name_start..].rfind('.') {
        Some(dot) if dot > 0 => &rel[..name_start + dot],
        _ => rel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::FileEntry;

    fn tree(files: &[(&str, u64, u64)], dirs: &[&str], sidecars: &[&str]) -> Tree {
        Tree {
            files: files
                .iter()
                .map(|&(rel, size, mtime)| (rel.to_owned(), FileEntry { size, mtime }))
                .collect(),
            dirs: dirs.iter().map(|d| d.to_string()).collect(),
            sidecars: sidecars.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn mirrors_files_by_name_and_size() {
        let source = tree(
            &[("a.azw3", 10, 1), ("b.azw3", 20, 1), ("c.azw3", 30, 1)],
            &[],
            &[],
        );
        let target = tree(
            &[("a.azw3", 10, 99), ("b.azw3", 21, 99), ("d.azw3", 40, 99)],
            &[],
            &[],
        );
        let plan = build(&source, &target, &FolderState::new());

        let uploads: Vec<_> = plan
            .uploads
            .iter()
            .map(|u| (u.rel.as_str(), u.reason))
            .collect();
        assert_eq!(
            uploads,
            [("b.azw3", Reason::SizeChanged), ("c.azw3", Reason::New)]
        );
        assert_eq!(plan.delete_files, [("d.azw3".to_owned(), 40)]);
        assert_eq!(plan.unchanged, 1);
        assert_eq!(plan.upload_bytes(), 50);
        assert_eq!(plan.freed_bytes(), 61);
    }

    #[test]
    fn detects_same_size_change_from_state() {
        let source = tree(&[("a.azw3", 10, 2), ("b.azw3", 10, 1)], &[], &[]);
        let target = tree(&[("a.azw3", 10, 99), ("b.azw3", 10, 99)], &[], &[]);
        let state = FolderState::from([
            ("a.azw3".to_owned(), Record { size: 10, mtime: 1 }),
            ("b.azw3".to_owned(), Record { size: 10, mtime: 1 }),
        ]);
        let plan = build(&source, &target, &state);

        let uploads: Vec<_> = plan
            .uploads
            .iter()
            .map(|u| (u.rel.as_str(), u.reason))
            .collect();
        assert_eq!(uploads, [("a.azw3", Reason::Modified)]);
        assert_eq!(plan.unchanged, 1);
    }

    #[test]
    fn keeps_sidecars_of_kept_books_only() {
        let source = tree(
            &[("Kept.azw3", 10, 1), ("sub/Other.mobi", 10, 1)],
            &["sub"],
            &[],
        );
        let target = tree(
            &[
                ("Kept.azw3", 10, 1),
                ("sub/Other.mobi", 10, 1),
                ("Gone.azw3", 5, 1),
            ],
            &["sub"],
            &["Kept.sdr", "Gone.sdr", "sub/Other.sdr", "sub/Kept.sdr"],
        );
        let plan = build(&source, &target, &FolderState::new());

        assert_eq!(plan.delete_sidecars, ["Gone.sdr", "sub/Kept.sdr"]);
        assert_eq!(plan.delete_files, [("Gone.azw3".to_owned(), 5)]);
    }

    #[test]
    fn deletes_extra_directories_as_a_whole() {
        let source = tree(&[("a.azw3", 10, 1)], &["keep"], &[]);
        let target = tree(
            &[
                ("old/x.azw3", 1, 1),
                ("old/deep/y.azw3", 1, 1),
                ("keep/z.azw3", 1, 1),
            ],
            &["keep", "old", "old/deep"],
            &["old/x.sdr"],
        );
        let plan = build(&source, &target, &FolderState::new());

        assert_eq!(plan.delete_dirs, ["old"]);
        assert_eq!(plan.delete_files, [("keep/z.azw3".to_owned(), 1)]);
        assert!(plan.delete_sidecars.is_empty());
    }

    #[test]
    fn creates_missing_directories() {
        let source = tree(&[("new/a.azw3", 10, 1)], &["new", "new/deeper"], &[]);
        let plan = build(&source, &Tree::default(), &FolderState::new());

        assert_eq!(plan.create_dirs, ["new", "new/deeper"]);
        assert_eq!(plan.uploads.len(), 1);
    }

    #[test]
    fn up_to_date_plan_is_empty() {
        let source = tree(&[("a.azw3", 10, 1)], &[], &[]);
        let target = tree(&[("a.azw3", 10, 99)], &[], &["a.sdr"]);
        let plan = build(&source, &target, &FolderState::new());

        assert!(plan.is_empty());
        assert_eq!(plan.unchanged, 1);
    }

    #[test]
    fn strips_extension_from_file_name_only() {
        assert_eq!(strip_extension("Book.azw3"), "Book");
        assert_eq!(strip_extension("a.b/Book"), "a.b/Book");
        assert_eq!(strip_extension("Dr. Who - Book.mobi"), "Dr. Who - Book");
        assert_eq!(strip_extension("dir/.hidden"), "dir/.hidden");
    }
}
