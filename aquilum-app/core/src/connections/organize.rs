use super::{Connections, Imported};
use crate::{
    app_core::{Core, CoreEvent},
    files::{document::hash_file, gate},
};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

fn legacy_name(name: &str) -> bool {
    name.strip_prefix("Imported-")
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn folders(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_str().is_some_and(legacy_name)
            && entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            folders.push(entry.path());
        }
    }
    folders.sort();
    Ok(folders)
}

pub fn legacy_count(root: &Path) -> usize {
    folders(root).map_or(0, |v| v.len())
}

/// Refuse links and unexpected files before changing a legacy folder. Its full
/// original contents are kept in Aquilum Trash once all notes are verified.
fn contents(folder: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(folder).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_symlink() {
            return Err("An old import contains a link; it was left in place".into());
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if entry.metadata().map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
            return Err("An old imported note exceeds 8 MiB; its folder was left in place".into());
        }
        let relative = entry
            .path()
            .strip_prefix(folder)
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("An old import has a non-Unicode path")?
            .replace('\\', "/");
        if !entry.file_type().is_file() || super::client::note_path(&relative).is_err() {
            return Err("An old import contains files other than supported Markdown notes; it was left in place".into());
        }
        files.insert(
            relative,
            hash_file(entry.path()).map_err(|e| e.to_string())?,
        );
    }
    Ok(files)
}

impl Connections {
    pub(super) fn organize(&self, core: &Core, workspace: &Path) -> Result<(), String> {
        let root = workspace
            .canonicalize()
            .map_err(|_| "The local vault is unavailable")?;
        let folders = folders(&root)?;
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .destination = Some(root.clone());
        core.settle();
        for folder in folders {
            self.check_cancelled()?;
            if folder.canonicalize().map_err(|e| e.to_string())? != folder {
                return Err("An old import folder is a link; it was left in place".into());
            }
            let original = contents(&folder)?;
            for (relative, expected_hash) in &original {
                self.check_cancelled()?;
                let mut bytes = Vec::new();
                std::fs::File::open(folder.join(relative))
                    .map_err(|e| e.to_string())?
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err(
                        "An old imported note exceeds 8 MiB; its folder was left in place".into(),
                    );
                }
                if blake3::hash(&bytes).to_hex().as_str() != expected_hash {
                    return Err(
                        "An old import changed while being organized; its folder was left in place"
                            .into(),
                    );
                }
                let content = String::from_utf8(bytes)
                    .map_err(|_| "An old import is not UTF-8; its folder was left in place")?;
                let result = super::transfer::import(core, &root, relative, &content)?;
                self.record_import(result);
                core.emit(CoreEvent::ConnectionsChanged);
            }
            core.settle();
            self.check_cancelled()?;
            if contents(&folder)? != original {
                return Err(
                    "An old import changed while being organized; its folder was left in place"
                        .into(),
                );
            }
            gate::trash(core, &root, &folder).map_err(|e| e.to_string())?;
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .organized += 1;
            core.emit(CoreEvent::ConnectionsChanged);
        }
        Ok(())
    }

    pub(super) fn record_import(&self, result: Imported) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Imported::Created { conflict } => {
                state.copied += 1;
                state.conflicts += usize::from(conflict);
            }
            Imported::Unchanged => state.skipped += 1,
        }
    }
}
