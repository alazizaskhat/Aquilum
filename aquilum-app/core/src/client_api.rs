//! Authenticated REST API and its live settings, hosted by Aquilum itself.
use crate::{
    app_core::{Core, CoreEvent},
    files::document::write_content_atomic,
};
pub use aquilum_gateway::config::Config as ClientApiConfig;
use aquilum_gateway::{
    auth,
    embedded::{RunningApi, ToolCaller},
};
use serde::Serialize;
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientApiStatus {
    pub configured: bool,
    pub running: bool,
    pub busy: bool,
    pub address: Option<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClientToken {
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientApiField {
    Enabled,
    Port,
    Workspace,
    RequestsPerMinute,
    Burst,
    Origins,
}

#[derive(Default)]
struct State {
    running: Option<RunningApi>,
    status: ClientApiStatus,
}

pub struct ClientApi {
    config_path: PathBuf,
    state: Mutex<State>,
    closed: AtomicBool,
}

impl ClientApi {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            config_path: data_dir.join("client-api/config.json"),
            state: Mutex::new(State::default()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn settings(&self, workspace: Option<&Path>) -> ClientApiConfig {
        ClientApiConfig::load(&self.config_path).unwrap_or_else(|_| ClientApiConfig {
            enabled: false,
            bind: "127.0.0.1:8790".parse().expect("loopback address"),
            workspace: workspace.unwrap_or(Path::new("")).to_path_buf(),
            aquilum_settings: PathBuf::new(),
            tokens_file: self.config_path.with_file_name("tokens.json"),
            allowed_origins: Vec::new(),
            requests_per_minute: 120,
            burst: 20,
        })
    }

    pub fn clients(&self) -> Vec<ClientToken> {
        let config = self.settings(None);
        auth::load_tokens(&config.tokens_file)
            .map(|file| {
                file.clients
                    .into_iter()
                    .map(|client| ClientToken {
                        name: client.name,
                        enabled: client.enabled,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn record_error(&self, error: String) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .error = Some(error);
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Validate before changing a working listener. The restart runs off the UI thread.
    pub fn configure(
        &self,
        core: &Arc<Core>,
        change: impl FnOnce(&mut ClientApiConfig),
    ) -> Result<(), String> {
        let result = (|| {
            if self.closed.load(Ordering::SeqCst) {
                return Err("Aquilum is closing".into());
            }
            if self.status().busy {
                return Err("Wait for the backend to finish restarting".into());
            }
            let workspace = core.search.active_root();
            let mut config = self.settings(workspace.as_deref());
            change(&mut config);
            config.validate().map_err(|e| e.to_string())?;
            if config.enabled {
                let known = core
                    .ui_state
                    .workspace_roots()
                    .map_err(|error| format!("{error:?}"))?;
                if !known
                    .iter()
                    .any(|root| crate::search::paths::same_path(root, &config.workspace))
                {
                    return Err(
                        "Open this notes folder in Aquilum first, then select it for the backend"
                            .into(),
                    );
                }
                let tokens = auth::load_tokens(&config.tokens_file)
                    .map_err(|_| "Create a client token before enabling the backend".to_owned())?;
                if !tokens.clients.iter().any(|client| client.enabled) {
                    return Err("Create an active client token before enabling the backend".into());
                }
                let status = self.status();
                if !status.running
                    || status.address.as_deref() != Some(config.bind.to_string().as_str())
                {
                    let _probe = TcpListener::bind(config.bind)
                        .map_err(|_| "The selected port is already in use".to_owned())?;
                }
            }
            std::fs::create_dir_all(self.config_path.parent().expect("profile directory"))
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
            write_content_atomic(&self.config_path, &json, None).map_err(|e| e.to_string())?;
            self.request_apply(core)
        })();
        if let Err(error) = &result {
            self.record_error(error.clone());
        }
        result
    }

    fn begin(&self) -> Result<Option<RunningApi>, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            return Err("Aquilum is closing".into());
        }
        if state.status.busy {
            return Err("Backend is already changing state".into());
        }
        state.status.busy = true;
        state.status.error = None;
        state.status.notice = None;
        Ok(state.running.take())
    }

    pub fn apply(&self, core: &Arc<Core>) {
        if let Ok(previous) = self.begin() {
            self.finish_apply(core, previous, false);
        }
    }

    pub fn request_apply(&self, core: &Arc<Core>) -> Result<(), String> {
        let previous = self.begin()?;
        core.emit(CoreEvent::ClientApiChanged);
        let weak = Arc::downgrade(core);
        std::thread::spawn(move || {
            if let Some(core) = weak.upgrade() {
                core.client_api.finish_apply(&core, previous, true);
            } else if let Some(previous) = previous {
                previous.stop();
            }
        });
        Ok(())
    }

    fn finish_apply(&self, core: &Arc<Core>, previous: Option<RunningApi>, notify: bool) {
        if let Some(previous) = previous {
            previous.stop();
        }
        let configured = self.config_path.is_file();
        let result = (|| {
            if !configured || self.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let config = ClientApiConfig::load(&self.config_path).map_err(|e| e.to_string())?;
            if !config.enabled {
                return Ok(None);
            }
            let weak = Arc::downgrade(core);
            let caller: ToolCaller = Arc::new(move |tool, arguments| {
                if !matches!(
                    tool,
                    "list_notes"
                        | "read_note"
                        | "note_outline"
                        | "search_notes"
                        | "backlinks"
                        | "get_index_status"
                ) {
                    return Err("Only read operations are available".into());
                }
                let core = weak.upgrade().ok_or("Aquilum is shutting down")?;
                core.settle();
                crate::mcp::tools::call(&core, tool, arguments)
            });
            RunningApi::start(config, caller)
                .map(Some)
                .map_err(|e| e.to_string())
        })();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.status = ClientApiStatus {
            configured,
            ..Default::default()
        };
        match result {
            Ok(Some(running)) if !self.closed.load(Ordering::SeqCst) => {
                state.status.running = true;
                state.status.address = Some(running.address().to_string());
                state.running = Some(running);
            }
            Ok(Some(running)) => {
                drop(state);
                running.stop();
                if notify {
                    core.emit(CoreEvent::ClientApiChanged);
                }
                return;
            }
            Ok(None) => {}
            Err(error) => state.status.error = Some(error),
        }
        drop(state);
        if notify {
            core.emit(CoreEvent::ClientApiChanged);
        }
    }

    pub fn status(&self) -> ClientApiStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut status = state.status.clone();
        if !status.busy
            && status.running
            && state
                .running
                .as_ref()
                .is_none_or(|running| !running.is_running())
        {
            status.running = false;
            status.error = Some("The backend stopped unexpectedly; restart it to retry".into());
        }
        status
    }

    pub fn create_token(&self, name: &str, output: &Path) -> Result<(), String> {
        let config = self.settings(None);
        let result =
            auth::create_token(&config.tokens_file, name, output).map_err(|e| e.to_string());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &result {
            Ok(()) => {
                state.status.error = None;
                state.status.notice = Some(output.display().to_string());
            }
            Err(error) => state.status.error = Some(error.clone()),
        }
        result
    }

    pub fn revoke_token(&self, name: &str) -> Result<(), String> {
        let result =
            auth::revoke_token(&self.settings(None).tokens_file, name).map_err(|e| e.to_string());
        if let Err(error) = &result {
            self.record_error(error.clone());
        } else {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status
                .error = None;
        }
        result
    }

    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let previous = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.status.running = false;
            state.status.address = None;
            state.running.take()
        };
        if let Some(previous) = previous {
            previous.stop();
        }
    }
}

/// These commands share exactly the same token registry as Settings > Backend.
pub fn token_command(data_dir: &Path, arguments: &[String]) -> Result<(), String> {
    let api = ClientApi::new(data_dir);
    match arguments.first().map(String::as_str) {
        Some("--api-token-create") if arguments.len() == 3 => api.create_token(&arguments[1], Path::new(&arguments[2])),
        Some("--api-token-revoke") if arguments.len() == 2 => api.revoke_token(&arguments[1]),
        _ => Err("Usage: aquilum-app --api-token-create <client-name> <new-private-file> | --api-token-revoke <client-name>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreEvent, EventSink};
    use serde_json::{json, Value};
    use std::fs;

    fn wait_for_backend(core: &Core) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while core.client_api.status().busy {
            assert!(
                std::time::Instant::now() < deadline,
                "Backend transition did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn settings_enable_disable_and_invalid_changes_preserve_a_working_backend() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes");
        fs::create_dir(&notes).unwrap();
        let sink: Arc<dyn EventSink> = Arc::new(|event| {
            if let CoreEvent::Settle(done) = event {
                let _ = done.send(());
            }
        });
        fs::create_dir(dir.path().join("data")).unwrap();
        let core = Core::open(&dir.path().join("data"), sink);
        core.ui_state
            .resolve_workspace(&notes.to_string_lossy(), 0)
            .unwrap();
        core.search.prepare(&notes.to_string_lossy()).unwrap();
        core.client_api
            .configure(&core, |config| {
                config.workspace = notes;
                config.bind.set_port(0);
            })
            .unwrap();
        wait_for_backend(&core);
        assert!(!core.client_api.status().running);
        assert!(core
            .client_api
            .configure(&core, |config| config.enabled = true)
            .is_err());
        assert!(
            !ClientApiConfig::load(core.client_api.config_path())
                .unwrap()
                .enabled
        );
        let credential = dir.path().join("phone.token");
        core.client_api.create_token("phone", &credential).unwrap();
        core.client_api
            .configure(&core, |config| config.enabled = true)
            .unwrap();
        wait_for_backend(&core);
        let address = core.client_api.status().address.unwrap();
        let before = fs::read(core.client_api.config_path()).unwrap();
        assert!(core
            .client_api
            .configure(&core, |config| config.bind =
                "0.0.0.0:8790".parse().unwrap())
            .is_err());
        assert_eq!(fs::read(core.client_api.config_path()).unwrap(), before);
        assert_eq!(
            core.client_api.status().address.as_deref(),
            Some(address.as_str())
        );
        let token = fs::read_to_string(credential).unwrap();
        let url = format!("http://{address}/api/health");
        assert!(ureq::get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
            .is_ok());
        core.client_api.revoke_token("phone").unwrap();
        assert!(matches!(
            ureq::get(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .call(),
            Err(ureq::Error::Status(401, _))
        ));
        assert!(!core.client_api.clients()[0].enabled);
        core.client_api
            .configure(&core, |config| {
                config.enabled = false;
                config.workspace = PathBuf::from("missing-folder");
            })
            .unwrap();
        wait_for_backend(&core);
        assert!(!core.client_api.status().running);
        assert!(
            !ClientApiConfig::load(core.client_api.config_path())
                .unwrap()
                .enabled
        );
        assert!(TcpListener::bind(address).is_ok());
        core.shutdown();
    }

    #[test]
    fn api_reads_the_core_with_mcp_disabled_and_stops_with_the_app() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let root = directory.path().join("notes");
        fs::create_dir_all(data.join("client-api")).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Example.md"), "# Title\n\nEmbedded API test").unwrap();
        let token_file = data.join("client-api/tokens.json");
        let credential = directory.path().join("test.token");
        auth::create_token(&token_file, "test", &credential).unwrap();
        let token = fs::read_to_string(&credential).unwrap();
        fs::write(data.join("client-api/config.json"), json!({
            "enabled":true, "bind":"127.0.0.1:0", "workspace":root,
            "tokens_file":"tokens.json", "allowed_origins":[], "requests_per_minute":120, "burst":20
        }).to_string()).unwrap();
        let events: Arc<dyn EventSink> = Arc::new(|event| {
            if let CoreEvent::Settle(done) = event {
                let _ = done.send(());
            }
        });
        let core = Core::open(&data, events);
        core.ui_state
            .resolve_workspace(&root.to_string_lossy(), 0)
            .unwrap();
        core.search.prepare(&root.to_string_lossy()).unwrap();
        let status = core.client_api.status();
        assert!(status.running, "{:?}", status.error);
        assert!(!core.mcp.status().running);
        let address = status.address.unwrap();
        let url = format!("http://{address}/api/note?path=Example.md");
        assert!(matches!(
            ureq::get(&url).call(),
            Err(ureq::Error::Status(401, _))
        ));
        let response: Value = serde_json::from_str(
            &ureq::get(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .call()
                .unwrap()
                .into_string()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["path"], "Example.md");
        assert!(response.to_string().contains("Embedded API test"));
        core.shutdown();
        assert!(!core.client_api.status().running);
        assert!(std::net::TcpListener::bind(&address).is_ok());
    }

    #[test]
    fn missing_or_invalid_config_leaves_api_off() {
        let directory = tempfile::tempdir().unwrap();
        let events: Arc<dyn EventSink> = Arc::new(|_: CoreEvent| {});
        let core = Core::open(directory.path(), events);
        assert!(!core.client_api.status().configured);
        fs::create_dir_all(directory.path().join("client-api")).unwrap();
        fs::write(directory.path().join("client-api/config.json"), "invalid").unwrap();
        core.client_api.apply(&core);
        let status = core.client_api.status();
        assert!(status.configured && !status.running && status.error.is_some());
        core.shutdown();
    }
}
