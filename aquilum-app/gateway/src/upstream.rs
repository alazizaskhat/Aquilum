use crate::api::ApiError;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::{RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

type Session = RunningService<RoleClient, ClientConfig>;

impl crate::NoteBackend for Aquilum {
    fn call(&self, tool: &'static str, arguments: Value) -> crate::NoteFuture<'_> {
        Box::pin(Aquilum::call(self, tool, arguments))
    }
}

pub struct Aquilum {
    settings: PathBuf,
    workspace: PathBuf,
    session: Mutex<Option<CachedSession>>,
}

struct CachedSession {
    fingerprint: [u8; 32],
    service: Arc<Session>,
}

#[derive(Deserialize)]
struct Settings {
    mcp: McpSettings,
}
#[derive(Deserialize)]
struct McpSettings {
    enabled: bool,
    port: u16,
    token: String,
}

impl Aquilum {
    pub fn new(settings: PathBuf, workspace: PathBuf) -> Self {
        Self {
            settings,
            workspace,
            session: Mutex::new(None),
        }
    }

    pub async fn call(&self, tool: &'static str, mut arguments: Value) -> Result<Value, ApiError> {
        // This allowlist is independent of HTTP routing. No arbitrary MCP calls.
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
        arguments["workspace"] = json!(self.workspace.to_string_lossy());
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let session = self.connect().await?;
            let params = CallToolRequestParams::new(tool)
                .with_arguments(arguments.as_object().cloned().unwrap_or_default());
            match session.call_tool(params).await {
                Ok(result) => {
                    if result.is_error == Some(true) {
                        return Err(ApiError::tool_error());
                    }
                    if let Some(value) = result.structured_content {
                        return Ok(value);
                    }
                    let value = serde_json::to_value(result).map_err(|_| ApiError::upstream())?;
                    let content = value["content"].as_array().ok_or_else(ApiError::upstream)?;
                    let text = content
                        .iter()
                        .find(|item| item["type"] == "text")
                        .and_then(|item| item["text"].as_str())
                        .ok_or_else(ApiError::upstream)?;
                    serde_json::from_str(text).map_err(|_| ApiError::upstream())
                }
                Err(_) => {
                    self.invalidate(&session).await;
                    Err(ApiError::unavailable())
                }
            }
        })
        .await;
        match result {
            Ok(value) => value,
            Err(_) => {
                // Drop the old transport after a timeout; the next request reconnects.
                *self.session.lock().await = None;
                Err(ApiError::timeout())
            }
        }
    }

    async fn connect(&self) -> Result<Arc<Session>, ApiError> {
        let settings = read_settings(&self.settings).await?;
        let fingerprint: [u8; 32] =
            Sha256::digest(format!("{}:{}", settings.port, settings.token).as_bytes()).into();
        let mut cached = self.session.lock().await;
        if let Some(current) = cached.as_ref()
            && current.fingerprint == fingerprint
            && !current.service.is_transport_closed()
        {
            return Ok(current.service.clone());
        }
        *cached = None;
        // Both URL and credentials come from local settings, never from a client request.
        let config = StreamableHttpClientTransportConfig::with_uri(format!(
            "http://127.0.0.1:{}/mcp",
            settings.port
        ))
        .auth_header(settings.token);
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|_| ApiError::unavailable())?;
        let transport = StreamableHttpClientTransport::with_client(http, config);
        // Aquilum implements the classic initialize handshake, not draft discovery.
        let client = ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_06_18);
        let service = Arc::new(
            client
                .serve(transport)
                .await
                .map_err(|_| ApiError::unavailable())?,
        );
        *cached = Some(CachedSession {
            fingerprint,
            service: service.clone(),
        });
        Ok(service)
    }

    async fn invalidate(&self, session: &Arc<Session>) {
        let mut cached = self.session.lock().await;
        if cached
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(&current.service, session))
        {
            *cached = None;
        }
    }
}

async fn read_settings(path: &Path) -> Result<McpSettings, ApiError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| ApiError::unavailable())?;
    if bytes.len() > 65_536 {
        return Err(ApiError::unavailable());
    }
    let settings: Settings = serde_json::from_slice(&bytes).map_err(|_| ApiError::unavailable())?;
    if !settings.mcp.enabled || settings.mcp.port == 0 || settings.mcp.token.len() < 16 {
        return Err(ApiError::unavailable());
    }
    Ok(settings.mcp)
}
