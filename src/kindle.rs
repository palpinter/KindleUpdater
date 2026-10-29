use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use gio::prelude::*;

pub struct Kindle {
    pub name: String,
    pub documents: gio::File,
}

/// Finds the Kindle's `documents` directory, either under `explicit` or among the
/// devices known to GVfs (MTP and USB mass storage alike), mounting it if needed.
pub fn locate(explicit: Option<&Path>) -> Result<Kindle> {
    if let Some(path) = explicit {
        let root = gio::File::for_path(path);
        let is_documents = root
            .basename()
            .is_some_and(|name| name.as_os_str() == "documents");
        return documents_dir(&root)
            .or_else(|| (is_documents && is_dir(&root)).then(|| root.clone()))
            .map(|documents| Kindle {
                name: path.display().to_string(),
                documents,
            })
            .ok_or_else(|| anyhow!("nem találok documents mappát itt: {}", path.display()));
    }

    let monitor = gio::VolumeMonitor::get();
    let volumes = monitor.volumes();
    // The raw GVfs mount of an MTP device is just called "mtp"; its volume has the real name.
    let volume_name = |root: &gio::File| {
        volumes
            .iter()
            .find(|v| v.activation_root().is_some_and(|r| r.uri() == root.uri()))
            .map(|v| v.name().to_string())
    };

    let mut found: Vec<Kindle> = Vec::new();
    for mount in monitor.mounts() {
        let root = mount.root();
        let name = volume_name(&root).unwrap_or_else(|| mount.name().to_string());
        add_if_kindle(&mut found, name, &root);
    }

    // Plugged in, but nothing (e.g. a file manager) has mounted it yet.
    if found.is_empty() {
        for volume in &volumes {
            let name = volume.name().to_string();
            let Some(root) = volume.activation_root() else {
                continue;
            };
            if volume.get_mount().is_some()
                || !volume.can_mount()
                || !looks_like_kindle(&name, &root)
            {
                continue;
            }
            println!("A Kindle csatolása: {name}…");
            gio::glib::MainContext::default()
                .block_on(
                    volume.mount_future(gio::MountMountFlags::NONE, gio::MountOperation::NONE),
                )
                .with_context(|| format!("nem sikerült csatolni: {name}"))?;
            add_if_kindle(&mut found, name, &root);
        }
    }

    match found.len() {
        0 => bail!(
            "nem találom a Kindle-t. Csatlakoztasd USB-n, vagy add meg az útvonalát \
             a --kindle kapcsolóval."
        ),
        1 => Ok(found.remove(0)),
        _ => {
            let list: Vec<String> = found
                .iter()
                .map(|k| k.documents.uri().to_string())
                .collect();
            bail!(
                "több Kindle is csatlakoztatva van, a --kindle kapcsolóval válassz közülük:\n  {}",
                list.join("\n  ")
            )
        }
    }
}

fn looks_like_kindle(name: &str, root: &gio::File) -> bool {
    format!("{name} {}", root.uri())
        .to_lowercase()
        .contains("kindle")
}

fn add_if_kindle(found: &mut Vec<Kindle>, name: String, root: &gio::File) {
    if !looks_like_kindle(&name, root) {
        return;
    }
    let Some(documents) = documents_dir(root) else {
        return;
    };
    if found.iter().any(|k| k.documents.uri() == documents.uri()) {
        return;
    }
    found.push(Kindle { name, documents });
}

/// `<root>/documents` on USB mass storage, `<root>/<storage>/documents` on MTP.
fn documents_dir(root: &gio::File) -> Option<gio::File> {
    let direct = root.child("documents");
    if is_dir(&direct) {
        return Some(direct);
    }
    let storages = root
        .enumerate_children(
            "standard::name,standard::type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    storages
        .flatten()
        .filter(|info| info.file_type() == gio::FileType::Directory)
        .map(|info| root.child(info.name()).child("documents"))
        .find(is_dir)
}

fn is_dir(file: &gio::File) -> bool {
    file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        == gio::FileType::Directory
}
