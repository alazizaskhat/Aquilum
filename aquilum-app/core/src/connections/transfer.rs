use crate::{
    app_core::Core,
    files::{document::hash_file, error::FileCommandError, gate},
    history::Source,
};
use std::{io::ErrorKind, path::Path};

pub(super) enum Imported {
    Created { conflict: bool },
    Unchanged,
}

/// Keep the source path. Identical notes are skipped; differing local notes are
/// retained and the remote text gets a sibling copy. Retrying is idempotent.
pub(super) fn import(
    core: &Core,
    root: &Path,
    relative: &str,
    content: &str,
) -> Result<Imported, String> {
    super::client::note_path(relative)?;
    let original = root.join(relative);
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid note name")?;
    let extension = original
        .extension()
        .and_then(|s| s.to_str())
        .ok_or("Invalid note extension")?;
    let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
    for number in 0..=1000 {
        let target = match number {
            0 => original.clone(),
            1 => original.with_file_name(format!("{stem} (remote copy).{extension}")),
            number => original.with_file_name(format!("{stem} (remote copy {number}).{extension}")),
        };
        safe_parents(root, &target)?;
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(
                        "A note destination is a symbolic link; no file was replaced".into(),
                    );
                }
                if meta.is_file()
                    && meta.len() == content.len() as u64
                    && hash_file(&target).map_err(|e| e.to_string())? == hash
                {
                    return Ok(Imported::Unchanged);
                }
                continue;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
        match gate::create(core, &target, &content, Source::Me, None) {
            Ok(_) => {
                return Ok(Imported::Created {
                    conflict: number > 0,
                })
            }
            Err(FileCommandError::AlreadyExists { .. }) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Too many differing copies of this note; organize them before copying again".into())
}

fn safe_parents(root: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("Invalid destination")?;
    let mut at = root.to_path_buf();
    for part in parent
        .strip_prefix(root)
        .map_err(|_| "Invalid destination")?
        .components()
    {
        at.push(part);
        match std::fs::create_dir(&at) {
            Ok(()) => (),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
        let meta = std::fs::symlink_metadata(&at).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink()
            || !meta.is_dir()
            || !at
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
        {
            return Err("Copy destination contains a link outside the local vault".into());
        }
    }
    Ok(())
}
