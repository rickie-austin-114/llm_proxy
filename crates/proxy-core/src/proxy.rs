use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, Ipv6Addr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use futures_util::{stream::BoxStream, StreamExt};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder as ConnectionBuilder,
    service::TowerToHyperService,
};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::{cors, endpoint, BrowserOrigins, HttpsOptions, Provider, ProxyError, Upstream};

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_active_requests: usize,
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub response_header_timeout: Duration,
    pub stream_idle_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_requests: 8,
            max_header_bytes: 32 * 1024,
            max_body_bytes: 32 * 1024 * 1024,
            response_header_timeout: Duration::from_secs(120),
            stream_idle_timeout: Duration::from_secs(120),
        }
    }
}

pub struct ProxyOptions {
    /// Zero chooses an ephemeral port for isolated integration tests.
    pub port: u16,
    pub https: Option<HttpsOptions>,
    pub upstreams: Vec<Upstream>,
    pub browser_origins: BrowserOrigins,
    pub limits: Limits,
}

#[derive(Clone)]
struct ProxyState {
    port: u16,
    other_port: Option<u16>,
    browser_origins: BrowserOrigins,
    upstreams: Arc<RwLock<HashMap<Provider, Upstream>>>,
    concurrency: Arc<Semaphore>,
    cancellation: CancellationToken,
    limits: Limits,
}

/// Owns both loopback listeners and all serving tasks. Dropping it cancels serving.
pub struct RunningProxy {
    state: ProxyState,
    tasks: Vec<JoinHandle<()>>,
}

impl RunningProxy {
    pub async fn start(options: ProxyOptions) -> Result<Self, ProxyError> {
        let (port, listeners) = bind_loopback(options.port).await?;
        let https = match options.https {
            Some(https) => {
                let (https_port, listeners) = bind_loopback(https.port).await?;
                Some((https_port, listeners, TlsAcceptor::from(https.identity.0)))
            }
            None => None,
        };
        let https_port = https.as_ref().map(|(port, _, _)| *port);
        validate_upstreams(&options.upstreams, port, https_port)?;
        let state = ProxyState {
            port,
            other_port: https_port,
            browser_origins: options.browser_origins,
            upstreams: Arc::new(RwLock::new(
                options
                    .upstreams
                    .into_iter()
                    .map(|u| (u.provider, u))
                    .collect(),
            )),
            concurrency: Arc::new(Semaphore::new(options.limits.max_active_requests)),
            cancellation: CancellationToken::new(),
            limits: options.limits,
        };
        // Both transports share provider snapshots, the eight-request budget,
        // cancellation, and a bounded 64-connection pool (including handshakes).
        let connections = Arc::new(Semaphore::new(64));
        let mut tasks = Vec::new();
        for listener in listeners {
            tasks.push(serve(listener, state.clone(), None, connections.clone()));
        }
        if let Some((https_port, listeners, acceptor)) = https {
            let mut tls_state = state.clone();
            tls_state.port = https_port;
            tls_state.other_port = Some(port);
            for listener in listeners {
                tasks.push(serve(
                    listener,
                    tls_state.clone(),
                    Some(acceptor.clone()),
                    connections.clone(),
                ));
            }
        }
        Ok(Self { state, tasks })
    }

    pub fn port(&self) -> u16 {
        self.state.port
    }

    pub fn https_port(&self) -> Option<u16> {
        self.state.other_port
    }

    /// New requests use a new snapshot; in-flight bodies retain their old client.
    pub fn replace_upstreams(&self, upstreams: Vec<Upstream>) -> Result<(), ProxyError> {
        validate_upstreams(&upstreams, self.port(), self.https_port())?;
        *self
            .state
            .upstreams
            .write()
            .map_err(|_| ProxyError::StateUnavailable)? =
            upstreams.into_iter().map(|u| (u.provider, u)).collect();
        Ok(())
    }

    pub async fn stop(mut self) {
        self.state.cancellation.cancel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        for mut task in self.tasks.drain(..) {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

impl Drop for RunningProxy {
    fn drop(&mut self) {
        self.state.cancellation.cancel();
    }
}

async fn bind_loopback(port: u16) -> Result<(u16, Vec<TcpListener>), ProxyError> {
    let ipv4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| ProxyError::PortUnavailable)?;
    let port = ipv4
        .local_addr()
        .map_err(|_| ProxyError::PortUnavailable)?
        .port();
    let mut listeners = vec![ipv4];
    match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
        Ok(listener) => listeners.push(listener),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
            ) => {}
        Err(_) => return Err(ProxyError::PortUnavailable),
    }
    Ok((port, listeners))
}

fn validate_upstreams(
    upstreams: &[Upstream],
    port: u16,
    other_port: Option<u16>,
) -> Result<(), ProxyError> {
    for upstream in upstreams {
        upstream.validate_proxy_port(port)?;
        if let Some(port) = other_port {
            upstream.validate_proxy_port(port)?;
        }
    }
    Ok(())
}

fn serve(
    listener: TcpListener,
    state: ProxyState,
    tls: Option<TlsAcceptor>,
    connections: Arc<Semaphore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let router = Router::new()
            .fallback(any(forward))
            .with_state(state.clone());
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = state.cancellation.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => {},
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    };
                    let Ok(permit) = connections.clone().try_acquire_owned() else { continue };
                    let tls = tls.clone();
                    let router = router.clone();
                    let cancellation = state.cancellation.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let connection = async {
                            if let Some(acceptor) = tls {
                                // A silent TCP client cannot monopolize a TLS slot forever.
                                if let Ok(Ok(stream)) = tokio::time::timeout(
                                    Duration::from_secs(5), acceptor.accept(stream),
                                ).await {
                                    serve_connection(stream, router).await;
                                }
                            } else {
                                serve_connection(stream, router).await;
                            }
                        };
                        tokio::select! {
                            _ = cancellation.cancelled() => {},
                            _ = connection => {},
                        }
                    });
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    })
}

async fn serve_connection<I>(stream: I, router: Router)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut builder = ConnectionBuilder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(10))
        .max_buf_size(32 * 1024);
    builder
        .http2()
        .max_concurrent_streams(8)
        .max_header_list_size(32 * 1024)
        .max_send_buf_size(64 * 1024);
    let _ = builder
        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(router))
        .await;
}

async fn forward(State(state): State<ProxyState>, request: Request) -> Response {
    let browser_request = request.headers().contains_key(header::ORIGIN);
    let allowed_origin = state.browser_origins.allowed_origin(request.headers());
    let mut response = if browser_request && allowed_origin.is_none() {
        failure(
            None,
            StatusCode::FORBIDDEN,
            "browser_origin_denied",
            "This browser origin is not allowed to access the local proxy.",
        )
    } else {
        forward_request(state, request).await
    };
    cors::finish(&mut response, allowed_origin);
    response
}

async fn forward_request(state: ProxyState, request: Request) -> Response {
    if request.method() == Method::CONNECT || request.method() == Method::TRACE {
        return failure(
            None,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "Use a native provider HTTP endpoint.",
        );
    }
    let browser_request = request.headers().contains_key(header::ORIGIN);
    if browser_request && !cors::method_allowed(request.method()) {
        return failure(
            None,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "Browser clients can use GET, POST, or OPTIONS.",
        );
    }
    let host_header = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    // HTTP/2 carries :authority in the URI; HTTP/1 absolute-form targets remain
    // forbidden. Never accept a conflicting Host header on an HTTP/2 stream.
    let authority = request
        .uri()
        .authority()
        .map(|authority| authority.as_str());
    let http2 = request.version() == axum::http::Version::HTTP_2;
    let host = if http2 {
        authority.or(host_header)
    } else {
        host_header
    };
    let valid_hosts = [
        format!("127.0.0.1:{}", state.port),
        format!("localhost:{}", state.port),
        format!("[::1]:{}", state.port),
    ];
    if (!http2 && authority.is_some())
        || (http2 && authority.is_some() && host_header.is_some() && authority != host_header)
        || request.headers().get_all(header::HOST).iter().count() > 1
        || !valid_hosts
            .iter()
            .any(|allowed| Some(allowed.as_str()) == host)
    {
        return failure(
            None,
            StatusCode::BAD_REQUEST,
            "invalid_host",
            "Use the local base URL shown in settings.",
        );
    }
    let path = request.uri().path();
    let Some((provider, suffix)) =
        [Provider::Claude, Provider::Openai]
            .into_iter()
            .find_map(|provider| {
                path.strip_prefix(provider.mount())
                    .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                    .map(|suffix| (provider, suffix.to_owned()))
            })
    else {
        return failure(
            None,
            StatusCode::NOT_FOUND,
            "unknown_endpoint",
            "Use the Claude or OpenAI base URL shown in settings.",
        );
    };
    let header_bytes: usize = request
        .headers()
        .iter()
        .map(|(k, v)| k.as_str().len() + v.len())
        .sum();
    if header_bytes > state.limits.max_header_bytes {
        return failure(
            Some(provider),
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "headers_too_large",
            "Reduce request header size.",
        );
    }
    // Preflights stay local, including when a provider has not been configured.
    // Host, namespace, origin, and header limits still apply before bypassing auth.
    if browser_request && request.method() == Method::OPTIONS {
        return cors::preflight(request.headers()).unwrap_or_else(|()| {
            failure(
                Some(provider),
                StatusCode::BAD_REQUEST,
                "invalid_cors_preflight",
                "Request a supported method and valid HTTP header names.",
            )
        });
    }
    let upstream = match state.upstreams.read() {
        Ok(upstreams) => upstreams.get(&provider).cloned(),
        Err(_) => {
            return failure(
                Some(provider),
                StatusCode::SERVICE_UNAVAILABLE,
                "state_unavailable",
                "Restart the app to restore proxy state.",
            )
        }
    };
    let Some(upstream) = upstream else {
        return failure(
            Some(provider),
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_not_configured",
            "Save this provider's endpoint and API key in settings.",
        );
    };
    if !authorized(request.headers(), upstream.key.expose()) {
        return failure(
            Some(provider),
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "Use the same API key saved for this provider in settings.",
        );
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|size| size > state.limits.max_body_bytes as u64)
    {
        return failure(
            Some(provider),
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "The request exceeds the local body size limit.",
        );
    }
    let Ok(permit) = state.concurrency.clone().try_acquire_owned() else {
        return failure(
            Some(provider),
            StatusCode::TOO_MANY_REQUESTS,
            "local_capacity",
            "The local proxy is busy. Wait for an active request to finish.",
        );
    };
    let Some(url) = endpoint::destination(&upstream.base, &suffix, request.uri().query()) else {
        return failure(
            Some(provider),
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "Use a relative provider endpoint without traversal or nested path encoding.",
        );
    };
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    strip_hop_headers(&mut headers);
    cors::strip_headers(&mut headers);
    for name in [
        "host",
        "authorization",
        "x-api-key",
        "proxy-authorization",
        "cookie",
        "origin",
        "referer",
        "content-length",
    ] {
        headers.remove(name);
    }
    let Ok(auth) = upstream.auth_value() else {
        return failure(
            Some(provider),
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid_credential",
            "Replace this provider's API key in settings.",
        );
    };
    let auth_header = match provider {
        Provider::Claude => "x-api-key",
        Provider::Openai => "authorization",
    };
    headers.insert(HeaderName::from_static(auth_header), auth);
    let exceeded = Arc::new(AtomicBool::new(false));
    let upload_exceeded = exceeded.clone();
    let max_body_bytes = state.limits.max_body_bytes;
    let upload_cancel = state.cancellation.clone();
    let upload: BoxStream<'static, Result<bytes::Bytes, io::Error>> =
        Box::pin(async_stream::try_stream! {
            let mut body = body.into_data_stream();
            let mut bytes_read = 0_usize;
            loop {
                let next = tokio::select! {
                    _ = upload_cancel.cancelled() => break,
                    next = body.next() => next,
                };
                let Some(chunk) = next else { break; };
                let chunk = chunk.map_err(|_| io::Error::other("Client upload was interrupted"))?;
                bytes_read = bytes_read.saturating_add(chunk.len());
                if bytes_read > max_body_bytes {
                    upload_exceeded.store(true, Ordering::Relaxed);
                    Err(io::Error::other("Request body exceeds the local limit"))?;
                }
                yield chunk;
            }
        });
    let outgoing = upstream
        .client
        .request(parts.method, url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(upload));
    let response = tokio::select! {
        _ = state.cancellation.cancelled() => return failure(Some(provider), StatusCode::SERVICE_UNAVAILABLE, "proxy_stopped", "The local proxy has stopped."),
        result = tokio::time::timeout(state.limits.response_header_timeout, outgoing.send()) => result,
    };
    let response = match response {
        Ok(Ok(response)) => response,
        _ if exceeded.load(Ordering::Relaxed) => {
            return failure(
                Some(provider),
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "The request exceeds the local body size limit.",
            )
        }
        Err(_) => {
            return failure(
                Some(provider),
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_timeout",
                "The upstream did not respond in time. Check its availability.",
            )
        }
        Ok(Err(error)) => {
            let (status, code, message) = if error.is_timeout() {
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream_timeout",
                    "The upstream connection timed out. Check the endpoint and network.",
                )
            } else {
                (StatusCode::BAD_GATEWAY, "upstream_unavailable", "Could not reach the upstream. Check its endpoint, TLS certificate, and network.")
            };
            return failure(Some(provider), status, code, message);
        }
    };
    let status = response.status();
    let mut headers = response.headers().clone();
    strip_hop_headers(&mut headers);
    cors::strip_headers(&mut headers);
    // The upstream's transport policy cannot configure the localhost origin.
    for name in [
        "set-cookie",
        "content-length",
        "strict-transport-security",
        "alt-svc",
    ] {
        headers.remove(name);
    }
    let stream: BoxStream<'static, Result<bytes::Bytes, io::Error>> = Box::pin(
        async_stream::try_stream! {
            // Keep the permit until the response body is complete or dropped by the client.
            let _permit = permit;
            let mut chunks = response.bytes_stream();
            loop {
                let next = tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    next = tokio::time::timeout(state.limits.stream_idle_timeout, chunks.next()) => next,
                };
                match next {
                    Ok(Some(Ok(bytes))) => yield bytes,
                    Ok(None) => break,
                    Ok(Some(Err(_))) => Err(io::Error::other("Upstream stream was interrupted"))?,
                    Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "Upstream stream timed out"))?,
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    if headers.get_all(header::AUTHORIZATION).iter().count() > 1
        || headers.get_all("x-api-key").iter().count() > 1
    {
        return false;
    }
    let bearer = match headers.get(header::AUTHORIZATION) {
        Some(value) => match value.to_str().ok().and_then(|v| v.split_once(' ')) {
            Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") => Some(token),
            _ => return false,
        },
        None => None,
    };
    let api_key = match headers.get("x-api-key") {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => return false,
        },
        None => None,
    };
    let candidates: Vec<_> = [bearer, api_key].into_iter().flatten().collect();
    !candidates.is_empty()
        && candidates
            .iter()
            .all(|token| bool::from(token.as_bytes().ct_eq(expected.as_bytes())))
}

fn strip_hop_headers(headers: &mut HeaderMap) {
    let named: Vec<_> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn failure(provider: Option<Provider>, status: StatusCode, code: &str, message: &str) -> Response {
    let body = match provider {
        Some(Provider::Claude) => json!({"type":"error", "error":{"type":code, "message":message}}),
        _ => json!({"error":{"type":"local_proxy_error", "code":code, "message":message}}),
    };
    (status, Json(body)).into_response()
}
