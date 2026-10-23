use crate::{
    api::{self, ApiError, AppState},
    auth,
    config::Config,
    NoteBackend, NoteFuture,
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{oneshot, Semaphore};

pub type ToolCaller = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

pub struct LocalBackend {
    call: ToolCaller,
    workers: Arc<Semaphore>,
}

impl LocalBackend {
    pub fn new(call: ToolCaller) -> Self {
        Self {
            call,
            workers: Arc::new(Semaphore::new(4)),
        }
    }
}

impl NoteBackend for LocalBackend {
    fn call(&self, tool: &'static str, arguments: Value) -> NoteFuture<'_> {
        Box::pin(async move {
            if !matches!(
                tool,
                "list_notes"
                    | "read_note"
                    | "note_outline"
                    | "search_notes"
                    | "backlinks"
                    | "get_index_status"
            ) {
                return Err(ApiError::forbidden());
            }
            let call = self.call.clone();
            let permit = self
                .workers
                .clone()
                .try_acquire_owned()
                .map_err(|_| ApiError::limited())?;
            // The worker owns the permit even if the HTTP request times out.
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                call(tool, &arguments)
            });
            match tokio::time::timeout(Duration::from_secs(15), task).await {
                Ok(Ok(Ok(value))) => Ok(value),
                Ok(Ok(Err(_))) => Err(ApiError::tool_error()),
                Ok(Err(_)) => Err(ApiError::unavailable()),
                Err(_) => Err(ApiError::timeout()),
            }
        })
    }
}

/// One background runtime inside the existing Aquilum process; no subprocess or MCP client.
pub struct RunningApi {
    address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl RunningApi {
    pub fn start(config: Config, call: ToolCaller) -> Result<Self> {
        ensure!(config.enabled, "Client API is disabled");
        ensure!(
            config.bind.ip().is_loopback(),
            "Client API must bind to loopback"
        );
        auth::load_tokens(&config.tokens_file).context("Invalid client token registry")?;
        let listener = TcpListener::bind(config.bind).context("Cannot bind client API port")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(4)
            .build()?;
        let state = Arc::new(AppState::new(config, Arc::new(LocalBackend::new(call))));
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("aquilum-client-api".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let _ = axum::serve(listener, api::router(state))
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await;
                });
            })?;
        Ok(Self {
            address,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RunningApi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Client, TokenFile};
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn direct_backend_rejects_write_tools_before_calling_core() {
        let backend = LocalBackend::new(Arc::new(|_, _| panic!("Write reached core")));
        assert!(backend
            .call("delete_note", serde_json::json!({}))
            .await
            .is_err());
    }

    #[test]
    fn embedded_server_calls_core_without_mcp_and_releases_its_port() {
        use std::io::{Read, Write};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let token = format!("aqg_{}", "ab".repeat(32));
        let tokens_file = dir.path().join("tokens.json");
        std::fs::write(
            &tokens_file,
            serde_json::to_vec(&TokenFile {
                clients: vec![Client {
                    name: "test".into(),
                    sha256: hex::encode(Sha256::digest(token.as_bytes())),
                    enabled: true,
                }],
            })
            .unwrap(),
        )
        .unwrap();
        let config = Config {
            enabled: true,
            bind: "127.0.0.1:0".parse().unwrap(),
            workspace: root.clone(),
            aquilum_settings: "not-used".into(),
            tokens_file,
            allowed_origins: vec![],
            requests_per_minute: 120,
            burst: 20,
        };
        let server = RunningApi::start(
            config,
            Arc::new(move |tool, arguments| {
                assert_eq!(tool, "get_index_status");
                assert_eq!(arguments["workspace"], root.to_string_lossy().as_ref());
                Ok(serde_json::json!({"indexing": false}))
            }),
        )
        .unwrap();
        let address = server.address();
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET /api/health HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("\"indexing\":false"));
        server.stop();
        assert!(TcpListener::bind(address).is_ok());
    }
}
