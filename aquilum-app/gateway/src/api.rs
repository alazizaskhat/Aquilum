use crate::{NoteBackend, auth, config::Config};
use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub struct AppState {
    pub config: Config,
    backend: Arc<dyn NoteBackend>,
    rate: Mutex<HashMap<String, Bucket>>,
    global_rate: Mutex<Bucket>,
    in_flight: Semaphore,
}

impl AppState {
    pub fn new(config: Config, backend: Arc<dyn NoteBackend>) -> Self {
        Self {
            backend,
            config,
            rate: Mutex::new(HashMap::new()),
            global_rate: Mutex::new(Bucket::new(240)),
            in_flight: Semaphore::new(8),
        }
    }

    async fn call(&self, tool: &'static str, mut arguments: Value) -> Result<Value, ApiError> {
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
        arguments["workspace"] = json!(self.config.workspace.to_string_lossy());
        self.backend.call(tool, arguments).await
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/notes", get(notes))
        .route("/api/note", get(note))
        .route("/api/export", get(export))
        .route("/api/outline", get(outline))
        .route("/api/search", get(search))
        .route("/api/backlinks", get(backlinks))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "not_found", "Endpoint not found") })
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}

async fn protect(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let result = authorize(&state, &request);
    let mut response = match result {
        Err(error) => error.into_response(),
        Ok(()) => match state.in_flight.try_acquire() {
            Err(_) => ApiError::limited().into_response(),
            Ok(_permit) => {
                match tokio::time::timeout(Duration::from_secs(18), next.run(request)).await {
                    Ok(response) => response,
                    Err(_) => ApiError::timeout().into_response(),
                }
            }
        },
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    response
}

fn authorize(state: &AppState, request: &Request<Body>) -> Result<(), ApiError> {
    if !state
        .global_rate
        .lock()
        .map_err(|_| ApiError::unavailable())?
        .take(240, 20.0)
    {
        return Err(ApiError::limited());
    }
    if request.uri().to_string().len() > 4096 {
        return Err(ApiError::bad("Request URL too long"));
    }
    if request.headers().contains_key(header::TRANSFER_ENCODING)
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .is_some_and(|value| value != "0")
    {
        return Err(ApiError::bad("Request bodies are not accepted"));
    }
    let origins = request.headers().get_all(header::ORIGIN);
    if origins.iter().count() > 1 {
        return Err(ApiError::forbidden());
    }
    if let Some(origin) = origins.iter().next() {
        let origin = origin.to_str().map_err(|_| ApiError::forbidden())?;
        if !state
            .config
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
        {
            return Err(ApiError::forbidden());
        }
    }
    let authorization = request.headers().get_all(header::AUTHORIZATION);
    if authorization.iter().count() != 1 {
        return Err(ApiError::unauthorized());
    }
    let value = authorization
        .iter()
        .next()
        .unwrap()
        .to_str()
        .map_err(|_| ApiError::unauthorized())?;
    let (scheme, token) = value.split_once(' ').ok_or_else(ApiError::unauthorized)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(ApiError::unauthorized());
    }
    // Re-read a small, atomically replaced hash file: revocation needs no restart.
    let file = auth::load_tokens(&state.config.tokens_file).map_err(|_| ApiError::unavailable())?;
    let client = auth::authenticate(&file, token).ok_or_else(ApiError::unauthorized)?;
    let mut rates = state.rate.lock().map_err(|_| ApiError::unavailable())?;
    rates.retain(|_, bucket| bucket.updated.elapsed() < Duration::from_secs(300));
    if rates.len() >= 128 && !rates.contains_key(&client) {
        return Err(ApiError::limited());
    }
    if !rates
        .entry(client)
        .or_insert_with(|| Bucket::new(state.config.burst))
        .take(
            state.config.burst,
            f64::from(state.config.requests_per_minute) / 60.0,
        )
    {
        return Err(ApiError::limited());
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Err(ApiError(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "Only read operations are available",
        ));
    }
    Ok(())
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn new(capacity: u32) -> Self {
        Self {
            tokens: f64::from(capacity),
            updated: Instant::now(),
        }
    }
    fn take(&mut self, capacity: u32, per_second: f64) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.updated).as_secs_f64() * per_second)
            .min(f64::from(capacity));
        self.updated = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

#[derive(Debug)]
pub struct ApiError(StatusCode, &'static str, &'static str);
impl ApiError {
    pub fn bad(message: &'static str) -> Self {
        Self(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid client bearer token is required",
        )
    }
    pub fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "forbidden", "Access denied")
    }
    pub fn unavailable() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The notes service is unavailable",
        )
    }
    pub fn upstream() -> Self {
        Self(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            "Invalid response from notes service",
        )
    }
    pub fn timeout() -> Self {
        Self(
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "The notes service did not respond in time",
        )
    }
    pub fn limited() -> Self {
        Self(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many requests; retry later",
        )
    }
    pub fn tool_error() -> Self {
        Self(
            StatusCode::UNPROCESSABLE_ENTITY,
            "tool_error",
            "The notes service could not complete this request",
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.0,
            Json(json!({"error": {"code": self.1, "message": self.2}})),
        )
            .into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"aquilum-gateway\""),
            );
        }
        if self.0 == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        if self.0 == StatusCode::METHOD_NOT_ALLOWED {
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        }
        response
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    folder: Option<String>,
    q: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    sort: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteQuery {
    path: String,
    heading: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathQuery {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    folder: Option<String>,
    limit: Option<usize>,
}

async fn health(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let status = state.call("get_index_status", json!({})).await?;
    Ok(Json(
        json!({"status":"ok", "readOnly":true, "index":status}),
    ))
}

async fn notes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut args =
        json!({"limit": bounded(query.limit, 100, 200)?, "offset": query.offset.unwrap_or(0)});
    if let Some(folder) = query.folder {
        args["folder"] = json!(validate_path(&state.config.workspace, &folder, false)?);
    }
    if let Some(q) = query.q {
        validate_text(&q, 256)?;
        args["query"] = json!(q);
    }
    if let Some(sort) = query.sort {
        if !matches!(sort.as_str(), "path" | "links") {
            return Err(ApiError::bad("Sort must be path or links"));
        }
        args["sort"] = json!(sort);
    }
    let mut data = state.call("list_notes", args).await?;
    filter_paths(&mut data, "notes", &state.config.workspace);
    Ok(Json(data))
}

async fn note(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NoteQuery>,
) -> Result<Json<Value>, ApiError> {
    let path = validate_path(&state.config.workspace, &query.path, true)?;
    let mut args = json!({"note":path, "offset":query.offset.unwrap_or(0), "limit":bounded(query.limit, 8000, 40000)?});
    if let Some(heading) = query.heading {
        validate_text(&heading, 512)?;
        args["heading"] = json!(heading);
    }
    Ok(Json(state.call("read_note", args).await?))
}

async fn export(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> Result<Json<Value>, ApiError> {
    let path = validate_path(&state.config.workspace, &query.path, true)?;
    Ok(Json(state.call("read_note", json!({"note": path, "snapshot": true})).await?))
}

async fn outline(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> Result<Json<Value>, ApiError> {
    let path = validate_path(&state.config.workspace, &query.path, true)?;
    Ok(Json(
        state.call("note_outline", json!({"note":path})).await?,
    ))
}

async fn backlinks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> Result<Json<Value>, ApiError> {
    let path = validate_path(&state.config.workspace, &query.path, true)?;
    let mut data = state.call("backlinks", json!({"note":path})).await?;
    filter_paths(&mut data, "backlinks", &state.config.workspace);
    Ok(Json(data))
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    validate_text(&query.q, 512)?;
    let mut args = json!({"query":query.q, "limit":bounded(query.limit, 10, 50)?});
    if let Some(folder) = query.folder {
        args["folder"] = json!(validate_path(&state.config.workspace, &folder, false)?);
    }
    let mut data = state.call("search_notes", args).await?;
    filter_paths(&mut data, "results", &state.config.workspace);
    Ok(Json(data))
}

fn bounded(value: Option<usize>, default: usize, max: usize) -> Result<usize, ApiError> {
    let value = value.unwrap_or(default);
    if value == 0 || value > max {
        return Err(ApiError::bad("Limit is outside the supported range"));
    }
    Ok(value)
}
fn validate_text(value: &str, max: usize) -> Result<(), ApiError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::bad("Invalid text parameter"));
    }
    Ok(())
}

fn validate_path(root: &Path, value: &str, note: bool) -> Result<String, ApiError> {
    if !note && value.is_empty() {
        return Ok(String::new());
    }
    validate_text(value, 2048)?;
    // Refuse traversal, device paths, ADS, hidden paths, Windows aliases and URI escapes.
    let normalized = value.replace('\\', "/");
    if normalized.contains([':', '%', '?', '*', '"', '<', '>', '|'])
        || normalized.split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with(['.', ' '])
                || part != part.trim()
        })
    {
        return Err(ApiError::bad(
            "Use a relative path inside the configured workspace",
        ));
    }
    if note && !normalized.to_ascii_lowercase().ends_with(".md") {
        return Err(ApiError::bad("An explicit .md note path is required"));
    }
    let target: PathBuf = root.join(&normalized).canonicalize().map_err(|_| {
        ApiError(
            StatusCode::NOT_FOUND,
            "not_found",
            "Note or folder not found",
        )
    })?;
    if !target.starts_with(root) || (note && !target.is_file()) || (!note && !target.is_dir()) {
        return Err(ApiError::forbidden());
    }
    Ok(normalized)
}

fn filter_paths(data: &mut Value, key: &str, root: &Path) {
    if let Some(items) = data[key].as_array_mut() {
        items.retain(|item| {
            item.as_str()
                .or_else(|| item["path"].as_str())
                .is_some_and(|path| validate_path(root, path, true).is_ok())
        });
    }
}

#[cfg(test)]
mod tests;
