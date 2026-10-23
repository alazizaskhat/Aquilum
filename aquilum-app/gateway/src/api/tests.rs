use super::*;
use crate::auth::{Client, TokenFile};
#[cfg(feature = "standalone")]
use axum::body::to_bytes;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("Пример.md"), "# Example\nhello").unwrap();
        let token = format!("aqg_{}", "ab".repeat(32));
        let tokens_file = dir.path().join("tokens.json");
        std::fs::write(
            &tokens_file,
            serde_json::to_vec(&TokenFile {
                clients: vec![Client {
                    name: "phone".into(),
                    sha256: hex::encode(Sha256::digest(token.as_bytes())),
                    enabled: true,
                }],
            })
            .unwrap(),
        )
        .unwrap();
        let config = Config {
            enabled: true,
            bind: "127.0.0.1:8790".parse().unwrap(),
            workspace: root.canonicalize().unwrap(),
            aquilum_settings: dir.path().join("settings.json"),
            tokens_file,
            allowed_origins: vec!["https://test1.salemsmart.kz".into()],
            requests_per_minute: 120,
            burst: 20,
        };
        #[cfg(feature = "standalone")]
        let backend = Arc::new(crate::upstream::Aquilum::new(
            config.aquilum_settings.clone(),
            config.workspace.clone(),
        ));
        #[cfg(not(feature = "standalone"))]
        let backend = Arc::new(Unavailable);
        Self {
            _dir: dir,
            state: Arc::new(AppState::new(config, backend)),
            token,
        }
    }
    fn request(&self, uri: &str) -> axum::http::request::Builder {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
    }
    async fn send(&self, request: axum::http::request::Builder) -> Response {
        router(self.state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

#[cfg(not(feature = "standalone"))]
struct Unavailable;
#[cfg(not(feature = "standalone"))]
impl crate::NoteBackend for Unavailable {
    fn call(&self, _: &'static str, _: Value) -> crate::NoteFuture<'_> {
        Box::pin(async { Err(ApiError::unavailable()) })
    }
}

#[tokio::test]
async fn no_token_wrong_token_duplicate_headers_and_query_token_are_rejected() {
    let f = Fixture::new();
    let missing = f.send(Request::builder().uri("/api/notes")).await;
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(missing.headers()[header::CACHE_CONTROL], "no-store");
    assert!(missing.headers().contains_key(header::WWW_AUTHENTICATE));
    assert_eq!(
        f.send(
            Request::builder()
                .uri("/api/notes")
                .header(header::AUTHORIZATION, "Bearer wrong")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(
            f.request("/api/notes")
                .header(header::AUTHORIZATION, "Bearer wrong")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(Request::builder().uri(format!("/api/notes?token={}", f.token)))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn writes_untrusted_origins_and_workspace_injection_are_rejected() {
    let f = Fixture::new();
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        assert_eq!(
            f.send(f.request("/api/note?path=example.md").method(method))
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let origin = f
        .send(
            f.request("/api/notes")
                .header(header::ORIGIN, "https://attacker.example"),
        )
        .await;
    assert_eq!(origin.status(), StatusCode::FORBIDDEN);
    assert!(
        !origin
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    );
    assert_eq!(
        f.send(f.request("/api/notes?workspace=C%3A%5Cprivate"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send(f.request("/api/note?path=..%2Fsecret.md"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send(f.request("/api/note?path=C%3A%5Csecret.md"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn revocation_and_invalid_token_file_fail_closed_without_restart() {
    let f = Fixture::new();
    assert_eq!(
        f.send(f.request("/api/notes")).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut tokens = auth::load_tokens(&f.state.config.tokens_file).unwrap();
    tokens.clients[0].enabled = false;
    std::fs::write(
        &f.state.config.tokens_file,
        serde_json::to_vec(&tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.send(f.request("/api/notes")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    std::fs::write(&f.state.config.tokens_file, "broken").unwrap();
    assert_eq!(
        f.send(f.request("/api/notes")).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn per_client_rate_limit_is_enforced() {
    let f = Fixture::new();
    for _ in 0..20 {
        assert_eq!(
            f.send(f.request("/missing")).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let response = f.send(f.request("/missing")).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key(header::RETRY_AFTER));
}

#[test]
fn paths_reject_hidden_traversal_and_windows_aliases() {
    let f = Fixture::new();
    for path in [
        "../x.md",
        "..\\x.md",
        "/x.md",
        "C:/x.md",
        "x.md:stream",
        ".trash/x.md",
        "dir/./x.md",
        "x.md.",
        "x.md ",
        "%2e%2e/x.md",
        "x.txt",
        "//server/x.md",
    ] {
        assert!(
            validate_path(&f.state.config.workspace, path, true).is_err(),
            "accepted {path}"
        );
    }
    assert!(validate_path(&f.state.config.workspace, "Пример.md", true).is_ok());
}

#[tokio::test]
#[cfg(feature = "standalone")]
async fn sdk_handshake_workspace_pinning_and_response_decoding() {
    let f = Fixture::new();
    let expected_workspace = f.state.config.workspace.to_string_lossy().into_owned();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let app = Router::new().route("/mcp", axum::routing::post(move |headers: axum::http::HeaderMap, Json(rpc): Json<Value>| {
            let workspace = expected_workspace.clone();
            async move {
                assert_eq!(headers[header::AUTHORIZATION], "Bearer upstream-test-credential");
                if rpc.get("id").is_none() { return StatusCode::ACCEPTED.into_response(); }
                let result = match rpc["method"].as_str().unwrap() {
                    "initialize" => json!({"protocolVersion":rpc["params"]["protocolVersion"], "capabilities":{"tools":{}}, "serverInfo":{"name":"aquilum-test", "version":"1"}}),
                    "tools/call" => {
                        assert_eq!(rpc["params"]["arguments"]["workspace"], workspace);
                        assert_eq!(rpc["params"]["name"], "list_notes");
                        json!({"content":[{"type":"text", "text":json!({"notes":["Пример.md", "../outside.md"], "total":2, "nextOffset":null}).to_string()}], "isError":false})
                    }
                    _ => panic!("Unexpected method"),
                };
                Json(json!({"jsonrpc":"2.0", "id":rpc["id"], "result":result})).into_response()
            }
        }));
        axum::serve(listener, app).await.unwrap();
    });
    std::fs::write(
        &f.state.config.aquilum_settings,
        json!({"mcp":{"enabled":true, "port":port, "token":"upstream-test-credential"}})
            .to_string(),
    )
    .unwrap();
    let response = f.send(f.request("/api/notes")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let data: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(data["notes"], json!(["Пример.md"]));
    server.abort();
}

#[test]
fn bucket_refills_and_does_not_allow_an_unbounded_burst() {
    let mut bucket = Bucket::new(2);
    assert!(bucket.take(2, 1.0));
    assert!(bucket.take(2, 1.0));
    assert!(!bucket.take(2, 1.0));
    bucket.updated -= Duration::from_secs(1);
    assert!(bucket.take(2, 1.0));
    assert!(!bucket.take(2, 1.0));
}
