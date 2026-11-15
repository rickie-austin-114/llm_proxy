use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use bytes::Bytes;
use futures_util::StreamExt;
use proxy_core::{
    validate_endpoint, BrowserOrigins, Limits, Provider, ProxyOptions, RunningProxy, Secret,
    Upstream,
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const CLAUDE_KEY: &str = "fake-test-claude-key";
const OPENAI_KEY: &str = "fake-test-openai-key";
const OFFICE_ORIGIN: &str = "https://pivot.claude.ai";

struct Observed {
    path: String,
    headers: HeaderMap,
    body: Bytes,
    method: String,
}

#[derive(Clone)]
struct FixtureState {
    observations: mpsc::Sender<Observed>,
    release: Arc<Notify>,
    dropped: Arc<Notify>,
}

struct Fixture {
    base: String,
    observations: mpsc::Receiver<Observed>,
    release: Arc<Notify>,
    dropped: Arc<Notify>,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let base = format!("http://{}", listener.local_addr().expect("fixture address"));
        let (sender, observations) = mpsc::channel(32);
        let release = Arc::new(Notify::new());
        let dropped = Arc::new(Notify::new());
        let state = FixtureState {
            observations: sender,
            release: release.clone(),
            dropped: dropped.clone(),
        };
        let router = Router::new()
            .fallback(any(fixture_handler))
            .with_state(state);
        let cancellation = CancellationToken::new();
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .expect("serve fixture");
        });
        Self {
            base,
            observations,
            release,
            dropped,
            cancellation,
            task,
        }
    }

    async fn observed(&mut self) -> Observed {
        tokio::time::timeout(Duration::from_secs(3), self.observations.recv())
            .await
            .expect("upstream contacted")
            .expect("observation")
    }

    fn upstream(&self, provider: Provider) -> Upstream {
        let (path, key) = match provider {
            Provider::Claude => ("/anthropic", CLAUDE_KEY),
            Provider::Openai => ("/gateway/v1", OPENAI_KEY),
        };
        Upstream::new(provider, &format!("{}{path}", self.base), secret(key))
            .expect("fixture upstream")
    }

    async fn proxy(&self, limits: Limits) -> RunningProxy {
        RunningProxy::start(ProxyOptions {
            https: None,
            port: 0,
            browser_origins: Default::default(),
            upstreams: vec![
                self.upstream(Provider::Claude),
                self.upstream(Provider::Openai),
            ],
            limits,
        })
        .await
        .expect("start proxy")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

struct NotifyOnDrop(Arc<Notify>);
impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn fixture_handler(State(state): State<FixtureState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, 64 * 1024 * 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mode = parts
        .headers
        .get("x-fixture-mode")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("json")
        .to_owned();
    state
        .observations
        .send(Observed {
            path: parts.uri.to_string(),
            headers: parts.headers,
            body,
            method: parts.method.to_string(),
        })
        .await
        .expect("observation receiver");
    let mut response = match mode.as_str() {
        "stream" => {
            let stream = async_stream::stream! {
                let _guard = NotifyOnDrop(state.dropped);
                yield Ok::<_, Infallible>(Bytes::from_static(b"data: {\"chunk\":\"first\"}\n\n"));
                state.release.notified().await;
                yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
            };
            let mut response = Response::new(Body::from_stream(stream));
            response
                .headers_mut()
                .insert("content-type", "text/event-stream".parse().expect("header"));
            response
        }
        "rate-limit" => (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "17")],
            Json(json!({"error":{"message":"fixture limit"}})),
        )
            .into_response(),
        "forbidden" => (
            StatusCode::FORBIDDEN,
            Json(json!({"error":{"message":"fixture permission denied"}})),
        )
            .into_response(),
        "redirect" => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "https://must-not-be-contacted.invalid/")],
        )
            .into_response(),
        _ => (
            [("x-fixture-response", "preserved")],
            Json(json!({"ok":true})),
        )
            .into_response(),
    };
    // An upstream's browser policy must never expand the local proxy's policy.
    for (name, value) in [
        ("access-control-allow-origin", "*"),
        ("access-control-allow-credentials", "true"),
        ("access-control-allow-methods", "*"),
        ("access-control-allow-headers", "*"),
        ("access-control-expose-headers", "*"),
        ("access-control-max-age", "86400"),
        ("vary", "accept-encoding"),
        (
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        ),
        ("alt-svc", "h3=\":443\""),
    ] {
        response
            .headers_mut()
            .insert(name, value.parse().expect("fixture header"));
    }
    response
}

fn secret(value: &str) -> Secret {
    Secret::new(value.to_owned()).expect("fake test secret")
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("test HTTP client")
}
fn url(proxy: &RunningProxy, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", proxy.port())
}

#[tokio::test]
async fn forwards_both_protocols_and_replaces_credentials_without_changing_payloads() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    let payload =
        r#"{"model":"future-model","messages":[{"role":"user","content":"fixture content"}]}"#;
    let response = client()
        .post(url(&proxy, "/openai/v1/responses?trace=a%2Fb"))
        .bearer_auth(OPENAI_KEY)
        .header("x-api-key", OPENAI_KEY)
        .header("connection", "x-remove-me")
        .header("x-remove-me", "must disappear")
        .header("openai-project", "fixture-project")
        .body(payload)
        .send()
        .await
        .expect("forward OpenAI");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-fixture-response"], "preserved");
    let observed = fixture.observed().await;
    assert_eq!(observed.path, "/gateway/v1/responses?trace=a%2Fb");
    assert_eq!(
        observed.headers["authorization"],
        format!("Bearer {OPENAI_KEY}")
    );
    assert!(!observed.headers.contains_key("x-api-key"));
    assert!(!observed.headers.contains_key("x-remove-me"));
    assert_eq!(observed.headers["openai-project"], "fixture-project");
    assert_eq!(observed.body, payload);

    let response = client()
        .post(url(&proxy, "/claude/v1/messages"))
        .header("x-api-key", CLAUDE_KEY)
        .header("anthropic-version", "2023-06-01")
        .body(payload)
        .send()
        .await
        .expect("forward Claude");
    assert_eq!(response.status(), 200);
    let observed = fixture.observed().await;
    assert_eq!(observed.path, "/anthropic/v1/messages");
    assert_eq!(observed.headers["x-api-key"], CLAUDE_KEY);
    assert_eq!(observed.headers["anthropic-version"], "2023-06-01");
    assert!(!observed.headers.contains_key("authorization"));
    assert_eq!(observed.body, payload);
    proxy.stop().await;
}

#[tokio::test]
async fn rejects_missing_wrong_cross_provider_and_ambiguous_keys_before_contacting_upstream() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    for (path, key, other_key) in [
        ("/claude/v1/models", CLAUDE_KEY, OPENAI_KEY),
        ("/openai/v1/models", OPENAI_KEY, CLAUDE_KEY),
    ] {
        let endpoint = url(&proxy, path);
        for request in [
            client().get(&endpoint),
            client().get(&endpoint).bearer_auth("wrong-fake-key"),
            client().get(&endpoint).bearer_auth("fixture-local-token"),
            client().get(&endpoint).bearer_auth(other_key),
            client().get(&endpoint).header("x-api-key", other_key),
            client()
                .get(&endpoint)
                .bearer_auth(key)
                .header("x-api-key", other_key),
            client()
                .get(&endpoint)
                .bearer_auth(other_key)
                .header("x-api-key", key),
            client()
                .get(&endpoint)
                .header("authorization", format!("Basic {key}")),
            client()
                .get(&endpoint)
                .bearer_auth(key)
                .header("authorization", format!("Bearer {key}")),
            client()
                .get(&endpoint)
                .header("x-api-key", key)
                .header("x-api-key", key),
        ] {
            assert_eq!(
                request
                    .send()
                    .await
                    .expect("authentication response")
                    .status(),
                401
            );
        }
    }
    assert!(fixture.observations.try_recv().is_err());
    proxy.stop().await;
}

#[tokio::test]
async fn streams_before_completion_and_holds_capacity_until_body_finishes() {
    let fixture = Fixture::start().await;
    let limits = Limits {
        max_active_requests: 1,
        ..Limits::default()
    };
    let proxy = fixture.proxy(limits).await;
    let response = client()
        .post(url(&proxy, "/openai/v1/responses"))
        .bearer_auth(OPENAI_KEY)
        .header("origin", OFFICE_ORIGIN)
        .header("x-fixture-mode", "stream")
        .send()
        .await
        .expect("stream headers");
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("first event is not buffered")
        .expect("first chunk")
        .expect("valid bytes");
    assert!(String::from_utf8_lossy(&first).contains("first"));
    let busy = client()
        .get(url(&proxy, "/openai/v1/models"))
        .bearer_auth(OPENAI_KEY)
        .header("origin", OFFICE_ORIGIN)
        .send()
        .await
        .expect("capacity response");
    assert_eq!(busy.status(), 429);
    assert_eq!(busy.headers()["access-control-allow-origin"], OFFICE_ORIGIN);
    fixture.release.notify_one();
    while let Some(chunk) = stream.next().await {
        chunk.expect("remaining stream");
    }
    let response = client()
        .get(url(&proxy, "/openai/v1/models"))
        .bearer_auth(OPENAI_KEY)
        .send()
        .await
        .expect("released capacity");
    assert_eq!(response.status(), 200);
    proxy.stop().await;
}

#[tokio::test]
async fn dropping_downstream_cancels_the_upstream_response() {
    let fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    let response = client()
        .get(url(&proxy, "/claude/v1/messages"))
        .bearer_auth(CLAUDE_KEY)
        .header("x-fixture-mode", "stream")
        .send()
        .await
        .expect("stream response");
    let mut chunks = response.bytes_stream();
    chunks.next().await.expect("first chunk").expect("bytes");
    drop(chunks);
    tokio::time::timeout(Duration::from_secs(2), fixture.dropped.notified())
        .await
        .expect("upstream body dropped after client cancellation");
    proxy.stop().await;
}

#[tokio::test]
async fn preserves_upstream_errors_and_does_not_follow_redirects_or_retry() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    let response = client()
        .get(url(&proxy, "/openai/v1/models"))
        .bearer_auth(OPENAI_KEY)
        .header("x-fixture-mode", "rate-limit")
        .send()
        .await
        .expect("rate limit");
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "17");
    let body: Value = serde_json::from_slice(&response.bytes().await.expect("upstream bytes"))
        .expect("upstream JSON");
    assert_eq!(body["error"]["message"], "fixture limit");
    fixture.observed().await;
    assert!(fixture.observations.try_recv().is_err());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("redirect-free client");
    let response = client
        .get(url(&proxy, "/claude/v1/models"))
        .bearer_auth(CLAUDE_KEY)
        .header("x-fixture-mode", "redirect")
        .send()
        .await
        .expect("redirect response");
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        "https://must-not-be-contacted.invalid/"
    );
    fixture.observed().await;
    assert!(fixture.observations.try_recv().is_err());
    proxy.stop().await;
}

#[tokio::test]
async fn enforces_declared_and_streamed_upload_limits() {
    let fixture = Fixture::start().await;
    let proxy = fixture
        .proxy(Limits {
            max_body_bytes: 16,
            ..Limits::default()
        })
        .await;
    let response = client()
        .post(url(&proxy, "/openai/v1/responses"))
        .bearer_auth(OPENAI_KEY)
        .body("this payload exceeds sixteen bytes")
        .send()
        .await
        .expect("declared limit response");
    assert_eq!(response.status(), 413);
    let chunks = futures_util::stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(b"0123456789")),
        Ok(Bytes::from_static(b"0123456789")),
    ]);
    let response = client()
        .post(url(&proxy, "/openai/v1/responses"))
        .bearer_auth(OPENAI_KEY)
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .expect("chunked limit response");
    assert_eq!(response.status(), 413);
    proxy.stop().await;
}

#[tokio::test]
async fn answers_office_preflights_locally_without_credentials_or_provider_configuration() {
    let proxy = RunningProxy::start(ProxyOptions {
        https: None,
        port: 0,
        browser_origins: Default::default(),
        upstreams: Vec::new(),
        limits: Limits::default(),
    })
    .await
    .expect("unconfigured proxy");
    for path in ["/claude", "/claude/v1/messages", "/openai/v1/models"] {
        let response = client()
            .request(reqwest::Method::OPTIONS, url(&proxy, path))
            .header("origin", OFFICE_ORIGIN)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "X-API-Key, Content-Type, Anthropic-Version, Anthropic-Beta, Authorization, X-Future-SDK-Header")
            .send()
            .await
            .expect("preflight response");
        assert_eq!(response.status(), 204);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            OFFICE_ORIGIN
        );
        assert_eq!(
            response.headers()["access-control-allow-methods"],
            "GET, POST, OPTIONS"
        );
        assert_eq!(response.headers()["access-control-allow-headers"], "x-api-key, content-type, anthropic-version, anthropic-beta, authorization, x-future-sdk-header");
        assert!(!response
            .headers()
            .contains_key("access-control-allow-credentials"));
        let vary = response
            .headers()
            .get_all("vary")
            .iter()
            .map(|value| value.to_str().expect("vary"))
            .collect::<Vec<_>>()
            .join(",")
            .to_ascii_lowercase();
        for name in [
            "origin",
            "access-control-request-method",
            "access-control-request-headers",
        ] {
            assert!(vary.contains(name));
        }
        assert!(response
            .bytes()
            .await
            .expect("empty preflight body")
            .is_empty());
    }
    let response = client()
        .request(reqwest::Method::OPTIONS, url(&proxy, "/claude/v1/models"))
        .header("origin", OFFICE_ORIGIN)
        .header("access-control-request-method", "GET")
        .send()
        .await
        .expect("preflight without custom headers");
    assert_eq!(response.status(), 204);
    assert!(!response
        .headers()
        .contains_key("access-control-allow-headers"));
    let response = client()
        .get(url(&proxy, "/claude/v1/models"))
        .header("origin", OFFICE_ORIGIN)
        .send()
        .await
        .expect("unconfigured provider error");
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    proxy.stop().await;
}

#[tokio::test]
async fn preflights_preserve_host_namespace_method_and_header_limits() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    let preflight = || {
        client()
            .request(reqwest::Method::OPTIONS, url(&proxy, "/claude/v1/messages"))
            .header("origin", OFFICE_ORIGIN)
    };
    for (request, status) in [
        (preflight(), 400),
        (
            preflight().header("access-control-request-method", "DELETE"),
            400,
        ),
        (
            preflight()
                .header("access-control-request-method", "POST")
                .header("access-control-request-method", "POST"),
            400,
        ),
        (
            preflight()
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "x-api-key, invalid header",
                ),
            400,
        ),
        (
            preflight()
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "*"),
            400,
        ),
        (
            preflight()
                .header("access-control-request-method", "POST")
                .header("host", "untrusted.invalid"),
            400,
        ),
        (
            client()
                .request(reqwest::Method::OPTIONS, url(&proxy, "/unknown"))
                .header("origin", OFFICE_ORIGIN)
                .header("access-control-request-method", "POST"),
            404,
        ),
        (
            client()
                .delete(url(&proxy, "/claude/v1/messages"))
                .header("origin", OFFICE_ORIGIN)
                .header("x-api-key", CLAUDE_KEY),
            405,
        ),
        (
            client().request(reqwest::Method::OPTIONS, url(&proxy, "/claude/v1/messages")),
            401,
        ),
    ] {
        let response = request.send().await.expect("invalid preflight response");
        assert_eq!(response.status(), status);
    }
    let limited = fixture
        .proxy(Limits {
            max_header_bytes: 256,
            ..Limits::default()
        })
        .await;
    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            url(&limited, "/claude/v1/messages"),
        )
        .header("origin", OFFICE_ORIGIN)
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            format!("x-{}", "a".repeat(512)),
        )
        .send()
        .await
        .expect("bounded preflight response");
    assert_eq!(response.status(), 431);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    assert!(fixture.observations.try_recv().is_err());
    limited.stop().await;
    proxy.stop().await;
}

#[tokio::test]
async fn rejects_other_origins_lookalikes_and_duplicate_origins_before_upstream() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    for origin in [
        "https://untrusted.invalid",
        "https://claude.ai",
        "https://other.claude.ai",
        "http://pivot.claude.ai",
        "https://pivot.claude.ai:8443",
        "https://pivot.claude.ai.untrusted.invalid",
        "https://pivot.claude.ai@untrusted.invalid",
        "https://pivot.claude.ai/",
        "https://pivot.claude.ai https://untrusted.invalid",
        "null",
    ] {
        for method in [reqwest::Method::GET, reqwest::Method::OPTIONS] {
            let response = client()
                .request(method, url(&proxy, "/claude/v1/models"))
                .header("x-api-key", CLAUDE_KEY)
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .send()
                .await
                .expect("untrusted browser response");
            assert_eq!(response.status(), 403, "origin {origin}");
            assert!(!response
                .headers()
                .contains_key("access-control-allow-origin"));
        }
    }
    let response = client()
        .get(url(&proxy, "/claude/v1/models"))
        .header("x-api-key", CLAUDE_KEY)
        .header("origin", OFFICE_ORIGIN)
        .header("origin", OFFICE_ORIGIN)
        .send()
        .await
        .expect("duplicate origin response");
    assert_eq!(response.status(), 403);
    assert!(!response
        .headers()
        .contains_key("access-control-allow-origin"));
    assert!(fixture.observations.try_recv().is_err());
    proxy.stop().await;
}

#[tokio::test]
async fn office_requests_keep_provider_auth_and_local_cors_on_success_and_errors() {
    let mut fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    for (path, key, other_key) in [
        ("/claude/v1/messages", CLAUDE_KEY, OPENAI_KEY),
        ("/openai/v1/responses", OPENAI_KEY, CLAUDE_KEY),
    ] {
        for request in [
            client().post(url(&proxy, path)),
            client()
                .post(url(&proxy, path))
                .header("x-api-key", other_key),
        ] {
            let response = request
                .header("origin", OFFICE_ORIGIN)
                .send()
                .await
                .expect("browser authentication error");
            assert_eq!(response.status(), 401);
            assert_eq!(
                response.headers()["access-control-allow-origin"],
                OFFICE_ORIGIN
            );
        }
        assert!(fixture.observations.try_recv().is_err());
        for (mode, status) in [("json", 200), ("rate-limit", 429), ("forbidden", 403)] {
            let response = client()
                .post(url(&proxy, path))
                .header("origin", OFFICE_ORIGIN)
                .header("referer", "https://pivot.claude.ai/office")
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "fixture-beta")
                .header("x-future-sdk-header", "preserved")
                .header("access-control-request-headers", "x-api-key")
                .header("x-fixture-mode", mode)
                .body("opaque office payload")
                .send()
                .await
                .expect("browser API response");
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers()["access-control-allow-origin"],
                OFFICE_ORIGIN
            );
            for name in [
                "access-control-allow-credentials",
                "access-control-allow-methods",
                "access-control-allow-headers",
                "access-control-max-age",
            ] {
                assert!(!response.headers().contains_key(name));
            }
            assert_ne!(response.headers()["access-control-expose-headers"], "*");
            if mode == "rate-limit" {
                assert_eq!(response.headers()["retry-after"], "17");
                assert!(response.headers()["access-control-expose-headers"]
                    .to_str()
                    .expect("exposed headers")
                    .contains("retry-after"));
            }
            let vary = response
                .headers()
                .get_all("vary")
                .iter()
                .map(|value| value.to_str().expect("vary"))
                .collect::<Vec<_>>()
                .join(",")
                .to_ascii_lowercase();
            assert!(vary.contains("origin"));
            assert!(vary.contains("accept-encoding"));
            let observation = fixture.observed().await;
            assert_eq!(observation.body, "opaque office payload");
            assert_eq!(observation.headers["anthropic-version"], "2023-06-01");
            assert_eq!(observation.headers["anthropic-beta"], "fixture-beta");
            assert_eq!(observation.headers["x-future-sdk-header"], "preserved");
            assert!(!observation.headers.contains_key("origin"));
            assert!(!observation.headers.contains_key("referer"));
            assert!(!observation
                .headers
                .contains_key("access-control-request-headers"));
            if key == CLAUDE_KEY {
                assert_eq!(observation.headers["x-api-key"], CLAUDE_KEY);
                assert!(!observation.headers.contains_key("authorization"));
            } else {
                assert_eq!(
                    observation.headers["authorization"],
                    format!("Bearer {OPENAI_KEY}")
                );
                assert!(!observation.headers.contains_key("x-api-key"));
            }
        }
    }
    let response = client()
        .get(url(&proxy, "/claude/v1/models"))
        .header("x-api-key", CLAUDE_KEY)
        .send()
        .await
        .expect("native client response");
    assert_eq!(response.status(), 200);
    assert!(!response
        .headers()
        .keys()
        .any(|name| name.as_str().starts_with("access-control-")));
    proxy.stop().await;
}

#[tokio::test]
async fn custom_origins_are_explicit_and_an_empty_list_disables_browser_access() {
    let custom_origin = "https://future-office.example.com";
    for origins in [vec![custom_origin.to_owned()], Vec::new()] {
        let proxy = RunningProxy::start(ProxyOptions {
            https: None,
            port: 0,
            upstreams: Vec::new(),
            browser_origins: BrowserOrigins::new(&origins).expect("configured origins"),
            limits: Limits::default(),
        })
        .await
        .expect("proxy with custom browser policy");
        for origin in [custom_origin, OFFICE_ORIGIN] {
            let allowed = origins.iter().any(|allowed| allowed == origin);
            let response = client()
                .request(reqwest::Method::OPTIONS, url(&proxy, "/claude/v1/messages"))
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .send()
                .await
                .expect("configured origin preflight");
            assert_eq!(response.status(), if allowed { 204 } else { 403 });
            assert_eq!(
                response
                    .headers()
                    .get("access-control-allow-origin")
                    .map(|value| value.to_str().expect("origin")),
                allowed.then_some(origin)
            );
        }
        proxy.stop().await;
    }
}

#[tokio::test]
async fn rejects_browser_origins_bad_hosts_and_traversal() {
    let fixture = Fixture::start().await;
    let proxy = fixture.proxy(Limits::default()).await;
    let response = client()
        .get(url(&proxy, "/openai/v1/models"))
        .bearer_auth(OPENAI_KEY)
        .header("origin", "https://untrusted.invalid")
        .send()
        .await
        .expect("origin response");
    assert_eq!(response.status(), 403);
    let response = client()
        .get(url(&proxy, "/openai/v1/models"))
        .bearer_auth(OPENAI_KEY)
        .header("host", "untrusted.invalid")
        .send()
        .await
        .expect("host response");
    assert_eq!(response.status(), 400);
    let mut socket = TcpStream::connect(("127.0.0.1", proxy.port()))
        .await
        .expect("raw client");
    socket.write_all(format!("GET /openai/v1/%2e%2e/secret HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {OPENAI_KEY}\r\nConnection: close\r\n\r\n", proxy.port()).as_bytes()).await.expect("raw request");
    let mut response = String::new();
    socket
        .read_to_string(&mut response)
        .await
        .expect("raw response");
    assert!(response.starts_with("HTTP/1.1 400"));
    proxy.stop().await;
}

#[tokio::test]
async fn updates_new_requests_without_interrupting_old_streams_and_releases_port_on_stop() {
    const ROTATED_KEY: &str = "fake-test-replacement-openai-key";
    let old_fixture = Fixture::start().await;
    let mut new_fixture = Fixture::start().await;
    let proxy = old_fixture.proxy(Limits::default()).await;
    let response = client()
        .get(url(&proxy, "/openai/v1/responses"))
        .bearer_auth(OPENAI_KEY)
        .header("x-fixture-mode", "stream")
        .send()
        .await
        .expect("old stream");
    let mut stream = response.bytes_stream();
    stream.next().await.expect("first event").expect("bytes");
    proxy
        .replace_upstreams(vec![Upstream::new(
            Provider::Openai,
            &format!("{}/gateway/v1", new_fixture.base),
            secret(ROTATED_KEY),
        )
        .expect("replacement upstream")])
        .expect("replace snapshot");
    assert_eq!(
        client()
            .get(url(&proxy, "/openai/v1/models"))
            .bearer_auth(OPENAI_KEY)
            .send()
            .await
            .expect("old key rejected")
            .status(),
        401
    );
    assert!(new_fixture.observations.try_recv().is_err());
    assert_eq!(
        client()
            .get(url(&proxy, "/claude/v1/models"))
            .bearer_auth(CLAUDE_KEY)
            .send()
            .await
            .expect("removed provider unavailable")
            .status(),
        503
    );
    assert_eq!(
        client()
            .get(url(&proxy, "/openai/v1/models"))
            .bearer_auth(ROTATED_KEY)
            .send()
            .await
            .expect("new destination")
            .status(),
        200
    );
    let observed = new_fixture.observed().await;
    assert_eq!(observed.path, "/gateway/v1/models");
    assert_eq!(
        observed.headers["authorization"],
        format!("Bearer {ROTATED_KEY}")
    );
    old_fixture.release.notify_one();
    while let Some(chunk) = stream.next().await {
        chunk.expect("old stream still valid");
    }
    let port = proxy.port();
    proxy.stop().await;
    let _rebound = TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("port released after shutdown");
}

#[tokio::test]
async fn connection_tests_list_models_without_generating_content() {
    let mut fixture = Fixture::start().await;
    for provider in [Provider::Claude, Provider::Openai] {
        assert_eq!(
            fixture
                .upstream(provider)
                .test_connection()
                .await
                .expect("model list test"),
            200
        );
        let request = fixture.observed().await;
        assert_eq!(request.method, "GET");
        assert!(request.body.is_empty());
        assert_eq!(
            request.path,
            if provider == Provider::Claude {
                "/anthropic/v1/models?limit=1"
            } else {
                "/gateway/v1/models"
            }
        );
    }
}

#[test]
fn refuses_insecure_ambiguous_and_self_referencing_endpoints() {
    for endpoint in [
        "http://example.com",
        "http://localhost:8080",
        "https://user:password@example.com",
        "https://example.com?api_key=fake",
        "file:///tmp/upstream",
    ] {
        assert!(
            validate_endpoint(endpoint).is_err(),
            "must reject {endpoint}"
        );
    }
    let upstream = Upstream::new(Provider::Claude, "http://[::1]:8787", secret(CLAUDE_KEY))
        .expect("IPv6 loopback endpoint");
    assert!(upstream.validate_proxy_port(8787).is_err());
    assert!(upstream.validate_proxy_port(8788).is_ok());
}

fn tls_fixture() -> (proxy_core::HttpsOptions, reqwest::Certificate) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate().expect("CA key");
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    let ca = ca_params.self_signed(&ca_key).expect("CA certificate");
    let key = KeyPair::generate().expect("server key");
    let server = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
        .expect("loopback SANs")
        .signed_by(&key, &Issuer::from_params(&ca_params, &ca_key))
        .expect("server certificate");
    let root = reqwest::Certificate::from_der(ca.der()).expect("client root");
    let identity = proxy_core::TlsIdentity::new(
        vec![server.der().to_vec(), ca.der().to_vec()],
        zeroize::Zeroizing::new(key.serialize_der()),
    )
    .expect("server TLS config");
    (proxy_core::HttpsOptions { port: 0, identity }, root)
}

#[tokio::test]
async fn https_verifies_certificates_and_preserves_cors_auth_streaming_and_shared_capacity() {
    let mut fixture = Fixture::start().await;
    let (https, root) = tls_fixture();
    let proxy = RunningProxy::start(ProxyOptions {
        port: 0,
        https: Some(https),
        upstreams: vec![fixture.upstream(Provider::Claude)],
        browser_origins: Default::default(),
        limits: Limits {
            max_active_requests: 1,
            ..Limits::default()
        },
    })
    .await
    .expect("HTTP and HTTPS listeners");
    let port = proxy.https_port().expect("HTTPS port");
    let url = format!("https://localhost:{port}/claude/v1/messages");
    let client = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(root.clone())
        .build()
        .expect("trusted client");
    let untrusted = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("untrusted client");
    let http1 = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .add_root_certificate(root)
        .build()
        .expect("HTTP/1.1 TLS client");
    let response = http1
        .get(format!("https://127.0.0.1:{port}/claude/v1/models"))
        .send()
        .await
        .expect("HTTPS with an IP SAN and HTTP/1.1");
    assert_eq!(response.version(), reqwest::Version::HTTP_11);
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        untrusted.get(&url).send().await.is_err(),
        "untrusted local CA must fail validation"
    );
    // A client that never starts a TLS handshake must not block other clients.
    let _silent = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("silent TLS connection");
    let response = client
        .request(reqwest::Method::OPTIONS, &url)
        .header("Origin", OFFICE_ORIGIN)
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "x-api-key,anthropic-version",
        )
        .send()
        .await
        .expect("HTTPS preflight");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    let response = client
        .get(&url)
        .header("Origin", OFFICE_ORIGIN)
        .send()
        .await
        .expect("missing key");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    let response = client
        .get(&url)
        .header("x-api-key", CLAUDE_KEY)
        .header("Origin", "https://untrusted.example")
        .send()
        .await
        .expect("unlisted origin");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client
        .get(&url)
        .header("Host", "example.com")
        .header("x-api-key", CLAUDE_KEY)
        .send()
        .await
        .expect("invalid host");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        fixture.observations.try_recv().is_err(),
        "local failures must not contact upstream"
    );
    let response = client
        .post(&url)
        .header("x-api-key", CLAUDE_KEY)
        .header("Origin", OFFICE_ORIGIN)
        .header("x-fixture-mode", "stream")
        .send()
        .await
        .expect("HTTPS stream headers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert!(!response.headers().contains_key("strict-transport-security"));
    assert!(!response.headers().contains_key("alt-svc"));
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        OFFICE_ORIGIN
    );
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("streaming before upstream completion")
        .expect("first chunk")
        .expect("chunk bytes");
    assert!(String::from_utf8_lossy(&first).contains("first"));
    let observation = fixture.observed().await;
    assert_eq!(observation.path, "/anthropic/v1/messages");
    assert_eq!(observation.headers["x-api-key"], CLAUDE_KEY);
    assert!(!observation.headers.contains_key("origin"));
    let response = client
        .get(format!(
            "http://127.0.0.1:{}/claude/v1/models",
            proxy.port()
        ))
        .header("x-api-key", CLAUDE_KEY)
        .send()
        .await
        .expect("shared HTTP capacity");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(proxy
        .replace_upstreams(vec![Upstream::new(
            Provider::Claude,
            &format!("https://127.0.0.1:{port}"),
            secret(CLAUDE_KEY)
        )
        .expect("upstream")])
        .is_err());
    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), fixture.dropped.notified())
        .await
        .expect("cancel upstream on HTTPS client disconnect");
    let http_port = proxy.port();
    tokio::time::timeout(Duration::from_secs(2), proxy.stop())
        .await
        .expect("cancel silent TLS handshake and all listeners");
    assert!(TcpListener::bind(("127.0.0.1", http_port)).await.is_ok());
    assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok());
}

#[tokio::test]
async fn https_port_conflict_rolls_back_http_binding() {
    let fixture = Fixture::start().await;
    let (mut https, _) = tls_fixture();
    let occupied = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("occupied HTTPS port");
    https.port = occupied.local_addr().expect("address").port();
    let http = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("available HTTP port");
    let port = http.local_addr().expect("address").port();
    drop(http);
    let result = RunningProxy::start(ProxyOptions {
        port,
        https: Some(https),
        upstreams: vec![fixture.upstream(Provider::Claude)],
        browser_origins: Default::default(),
        limits: Limits::default(),
    })
    .await;
    assert!(result.is_err());
    assert!(
        TcpListener::bind(("127.0.0.1", port)).await.is_ok(),
        "failed HTTPS binding must release HTTP"
    );
}
