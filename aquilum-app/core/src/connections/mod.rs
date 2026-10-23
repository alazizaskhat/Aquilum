//! Read-only device connections. Credentials stay in the user-selected token file.
mod client;
#[cfg(test)]
mod tests;

use crate::{
    app_core::{Core, CoreEvent},
    files::{document::write_content_atomic, gate},
    history::Source,
};
use client::Client;
pub use client::{Note, PAGE, PREVIEW};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Config {
    pub address: String,
    pub token_file: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct Status {
    pub config: Config,
    pub busy: bool,
    pub connected: bool,
    pub query: String,
    pub notes: Vec<String>,
    pub total: usize,
    pub offset: usize,
    pub next: Option<usize>,
    pub preview: Option<Note>,
    pub preview_offset: usize,
    pub error: Option<String>,
    pub copied: usize,
    pub destination: Option<PathBuf>,
}

pub enum Job {
    Browse(usize),
    Read(String, usize),
    Copy {
        workspace: PathBuf,
        note: Option<String>,
    },
}

pub struct Connections {
    path: PathBuf,
    state: Mutex<Status>,
    cancelled: AtomicBool,
    closed: AtomicBool,
}

impl Connections {
    pub fn new(data: &Path) -> Self {
        let path = data.join("connections.json");
        let config = std::fs::read(&path)
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(Status {
                config,
                ..Default::default()
            }),
            cancelled: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }
    }

    pub fn status(&self) -> Status {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn configure(&self, change: impl FnOnce(&mut Config)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.busy {
            return;
        }
        let mut config = state.config.clone();
        change(&mut config);
        let result = serde_json::to_string_pretty(&config)
            .map_err(|e| e.to_string())
            .and_then(|json| {
                write_content_atomic(&self.path, &json, None)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(()) => {
                *state = Status {
                    config,
                    ..Default::default()
                }
            }
            Err(e) => state.error = Some(e),
        }
    }

    pub fn query(&self, query: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.busy {
            state.query = query;
        }
    }

    pub fn back(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.busy {
            state.preview = None;
        }
    }

    pub fn disconnect(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.cancelled.store(true, Ordering::SeqCst);
        if !state.busy {
            *state = Status {
                config: state.config.clone(),
                ..Default::default()
            };
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            Err("Operation cancelled".into())
        } else {
            Ok(())
        }
    }

    pub fn start(&self, core: &Arc<Core>, job: Job) {
        let snapshot = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.busy || self.closed.load(Ordering::SeqCst) {
                return;
            }
            self.cancelled.store(false, Ordering::SeqCst);
            state.busy = true;
            state.error = None;
            state.clone()
        };
        core.emit(CoreEvent::ConnectionsChanged);
        let core = Arc::clone(core);
        std::thread::spawn(move || {
            let result = (|| {
                let token = snapshot
                    .config
                    .token_file
                    .as_deref()
                    .ok_or("Choose a client token file first")?;
                let client = Client::new(&snapshot.config.address, token)?;
                core.connections.run(&core, &client, job, &snapshot.query)
            })();
            let mut state = core
                .connections
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.busy = false;
            if let Err(error) = result {
                state.error = Some(error);
                state.connected = false;
            }
            drop(state);
            core.emit(CoreEvent::ConnectionsChanged);
        });
    }

    fn run(&self, core: &Core, client: &Client, job: Job, query: &str) -> Result<(), String> {
        match job {
            Job::Browse(offset) => {
                let listing = client.list(query, offset)?;
                self.check_cancelled()?;
                validate_listing(&listing, offset)?;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.notes = listing.notes;
                state.total = listing.total;
                state.offset = offset;
                state.next = listing.next;
                state.preview = None;
                state.connected = true;
            }
            Job::Read(path, offset) => {
                client::note_path(&path)?;
                let note = client.read(&path, offset)?;
                self.check_cancelled()?;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.preview = Some(note);
                state.preview_offset = offset;
                state.connected = true;
            }
            Job::Copy { workspace, note } => {
                let root = workspace
                    .canonicalize()
                    .map_err(|_| "The local vault is unavailable")?;
                let mut paths = Vec::new();
                if let Some(path) = note {
                    client::note_path(&path)?;
                    paths.push(path);
                } else {
                    let mut offset = 0;
                    loop {
                        self.check_cancelled()?;
                        if offset >= 100_000 {
                            return Err("Too many notes to copy at once".into());
                        }
                        // Copy all notes regardless of the browser's title filter.
                        let listing = client.list("", offset)?;
                        validate_listing(&listing, offset)?;
                        paths.extend(listing.notes);
                        if paths.len() > 100_000 {
                            return Err("Too many notes to copy at once".into());
                        }
                        match listing.next {
                            Some(next) => offset = next,
                            None => break,
                        }
                    }
                }
                if paths.is_empty() {
                    return Err("There are no notes to copy".into());
                }
                self.check_cancelled()?;
                let destination = root.join(format!("Imported-{}", uuid::Uuid::new_v4().simple()));
                std::fs::create_dir(&destination).map_err(|e| e.to_string())?;
                {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.copied = 0;
                    state.destination = Some(destination.clone());
                }
                let mut seen = HashSet::new();
                let mut folded = HashSet::new();
                for path in paths {
                    self.check_cancelled()?;
                    if !seen.insert(path.clone()) {
                        continue;
                    }
                    if !folded.insert(path.to_lowercase()) {
                        return Err("Two remote notes differ only by letter case; copy them separately to preserve both".into());
                    }
                    let note = client.export(&path)?;
                    self.check_cancelled()?;
                    let target = destination.join(&path);
                    safe_parents(&destination, &target)?;
                    gate::create(core, &target, &note.content, Source::Me, None)
                        .map_err(|e| e.to_string())?;
                    self.state.lock().unwrap_or_else(|e| e.into_inner()).copied += 1;
                    core.emit(CoreEvent::ConnectionsChanged);
                }
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .connected = true;
            }
        }
        Ok(())
    }
}

fn validate_listing(list: &client::Listing, offset: usize) -> Result<(), String> {
    if list.notes.len() > PAGE
        || list.next.is_some_and(|next| {
            // The API may filter inaccessible paths out of an otherwise full page.
            next <= offset
                || next > offset.saturating_add(PAGE)
                || next < offset.saturating_add(list.notes.len())
        })
    {
        return Err("Invalid note pagination from the other PC".into());
    }
    for path in &list.notes {
        client::note_path(path)?;
    }
    Ok(())
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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
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
            return Err("Copy destination contains a link outside the import folder".into());
        }
    }
    Ok(())
}
