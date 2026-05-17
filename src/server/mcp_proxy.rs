//! Thin reverse proxy in front of the SDK's MCP transport.
//!
//! The rust-mcp-sdk 0.8 owns the axum router and does not expose a hook to
//! attach tower middleware or to read the request URI from inside the
//! transport pipeline. We work around this by binding the SDK to an
//! internal `127.0.0.1` port and running our own axum proxy on the public
//! port. The proxy:
//!
//! 1. Reads `?repo=/abs/path` from the request URI.
//! 2. Forwards method, headers (including `mcp-session-id`), and the body
//!    to the SDK's internal listener.
//! 3. Streams the response back to the client unmodified.
//! 4. If a `mcp-session-id` is present in the SDK's response and a `?repo=`
//!    was on the request, records `(session_id → repo)` in `pending_repos`
//!    so that `StandaloneHandler` can promote it to a bound session.
//!
//! The proxy is intentionally minimal: it does not interpret JSON-RPC, it
//! does not buffer SSE bodies, and it preserves all SDK-level semantics by
//! forwarding errors and status codes verbatim.

use crate::path::Utf8PathBuf;
use axum::{
    body::{to_bytes, Body},
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use dashmap::DashMap;
use futures::TryStreamExt;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

/// Maximum size of a buffered request body before forwarding. MCP
/// JSON-RPC payloads are small (well under 256 KiB in practice). Cap at
/// 8 MiB so a hostile client cannot fill memory through this proxy.
const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Pending session-id → repo bindings captured from `?repo=` URL query
/// parameters. Populated by the proxy on the way back; consumed by
/// `StandaloneHandler::resolve_state` (and `on_initialized`) as the
/// highest-priority binding source.
pub type PendingRepos = Arc<DashMap<String, Utf8PathBuf>>;

pub fn new_pending_repos() -> PendingRepos {
    Arc::new(DashMap::new())
}

#[derive(Clone)]
struct ProxyState {
    /// HTTP client used to talk to the internal SDK listener. Kept warm so
    /// connections are reused across requests.
    client: reqwest::Client,
    /// `http://127.0.0.1:<internal-port>` (no trailing slash).
    backend_base: String,
    /// Endpoint path on the SDK side (default `/mcp`).
    backend_path: String,
    /// Captured `?repo=` bindings, populated on every response that carries
    /// a `mcp-session-id` header.
    pending_repos: PendingRepos,
}

/// Spawn the proxy listener. Returns once it has bound to `public_port`.
pub async fn spawn_mcp_proxy(
    host: &str,
    public_port: u16,
    backend_port: u16,
    backend_path: &str,
    pending_repos: PendingRepos,
) -> anyhow::Result<()> {
    // No `.timeout()` here: MCP SSE responses are long-lived, and reqwest's
    // `timeout(Duration::ZERO)` is a zero-second deadline, not "no
    // deadline." Leaving the builder's default keeps the request open for
    // the lifetime of the underlying TCP connection.
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build proxy reqwest::Client: {e}"))?;

    let state = Arc::new(ProxyState {
        client,
        backend_base: format!("http://127.0.0.1:{backend_port}"),
        backend_path: backend_path.to_string(),
        pending_repos,
    });

    let app = Router::new()
        .route(backend_path, get(handle_get))
        .route(backend_path, post(handle_post))
        .route(backend_path, delete(handle_delete))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{public_port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid proxy address {host}:{public_port}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(
        public_port,
        backend_port,
        backend_path,
        "MCP proxy listening; forwards to internal SDK"
    );

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "MCP proxy exited with error");
        }
    });

    Ok(())
}

async fn handle_get(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    forward(state, Method::GET, params, headers, req).await
}

async fn handle_post(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    forward(state, Method::POST, params, headers, req).await
}

async fn handle_delete(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    forward(state, Method::DELETE, params, headers, req).await
}

/// Parse the `?repo=` query parameter into a validated `Utf8PathBuf`.
/// Returns `None` for missing, non-absolute, or non-existent-directory
/// values. This is the same validation surface used by `bind_workspace`,
/// kept here so a malicious or typoed URL cannot cause us to remember
/// nonsense.
fn parse_repo_query(params: &HashMap<String, String>) -> Option<Utf8PathBuf> {
    let raw = params.get("repo")?;
    let path = Utf8PathBuf::from(raw.as_str());
    if !path.is_absolute() || !path.is_dir() {
        tracing::debug!(
            repo = %raw,
            "Ignoring ?repo= query: not an absolute existing directory"
        );
        return None;
    }
    Some(path)
}

async fn forward(
    state: Arc<ProxyState>,
    method: Method,
    params: HashMap<String, String>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    let repo_query = parse_repo_query(&params);

    let url = format!("{}{}", state.backend_base, state.backend_path);

    // Collect the request body. For SSE this is empty (GET) or a small
    // JSON-RPC POST. Buffering up to MAX_REQUEST_BODY_BYTES is fine; the
    // SDK itself has the same expectation.
    let (_parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "proxy: failed to read incoming body");
            return (StatusCode::BAD_REQUEST, format!("bad body: {e}")).into_response();
        }
    };

    // Translate axum::HeaderMap → reqwest::HeaderMap by re-parsing. axum 0.8
    // and reqwest 0.12 both use the `http` crate, so this is just a type
    // boundary, not a deep conversion. We drop hop-by-hop headers.
    let mut out_headers = reqwest::header::HeaderMap::new();
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) {
            continue;
        }
        if let Ok(hn) = reqwest::header::HeaderName::from_bytes(n.as_bytes()) {
            if let Ok(hv) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                out_headers.insert(hn, hv);
            }
        }
    }
    // Preserve the original query string on the upstream request so the
    // SDK's `Query` extractor sees exactly what the client sent. We do
    // need to drop `repo` if we want to hide it from the SDK; we keep it
    // so the backend's logs match the public request shape.
    let request_builder = state
        .client
        .request(method.clone(), &url)
        .headers(out_headers)
        .query(&params)
        .body(body_bytes);

    let response = match request_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "proxy: upstream request failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("upstream unreachable: {e}"),
            )
                .into_response();
        }
    };

    let status = response.status();
    let upstream_headers = response.headers().clone();

    // Capture the session id assigned by the SDK. Only meaningful on
    // initialize POSTs where the SDK creates a new session, but the header
    // is also echoed on subsequent requests and that is fine to record.
    if let (Some(sid_value), Some(repo)) = (upstream_headers.get("mcp-session-id"), &repo_query) {
        if let Ok(sid) = sid_value.to_str() {
            tracing::info!(
                session = %sid,
                repo = %repo,
                "proxy: captured ?repo= URL binding"
            );
            state.pending_repos.insert(sid.to_string(), repo.clone());
        }
    }

    // Stream the response body straight back. SSE responses must not be
    // buffered: clients block waiting for `event:` frames on the same
    // connection.
    let stream = response.bytes_stream();
    let mapped =
        stream.map_err(|e| std::io::Error::other(format!("proxy upstream stream error: {e}")));
    let body = Body::from_stream(mapped);

    let mut out = Response::new(body);
    *out.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    {
        let out_headers = out.headers_mut();
        for (name, value) in upstream_headers.iter() {
            let n = name.as_str();
            if is_hop_by_hop(n) {
                continue;
            }
            if let Ok(hn) = axum::http::HeaderName::from_bytes(n.as_bytes()) {
                if let Ok(hv) = HeaderValue::from_bytes(value.as_bytes()) {
                    out_headers.insert(hn, hv);
                }
            }
        }
    }
    out
}

/// Hop-by-hop headers defined by RFC 7230 §6.1. These must not be forwarded
/// across a proxy boundary; doing so can break connection reuse and confuse
/// downstream HTTP/1.1 framing.
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

/// JSON-RPC error code emitted by rust-mcp-sdk when the `mcp-session-id` is
/// either missing or refers to a session the SDK no longer tracks. See
/// `rust-mcp-sdk/src/mcp_http/mcp_http_handler.rs` (`error_response`) and
/// the constant table in `rust-mcp-sdk/src/error.rs`. We treat both the
/// wrapped JSON-RPC envelope (`{"error": {"code": -32016, ...}}`) and the
/// bare error object (which is what `error_response` actually serializes)
/// as session-expired signals.
#[allow(dead_code)]
const SESSION_NOT_FOUND_CODE: i64 = -32016;

/// Returns `true` when an upstream response indicates that the SDK does
/// not know the `mcp-session-id` we forwarded. Only inspects `4xx`
/// responses with a JSON content-type and a body small enough to have
/// already been buffered by `forward`; this function does not perform I/O.
#[allow(dead_code)]
fn parse_session_expired(status: StatusCode, content_type: Option<&str>, body: &[u8]) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let ct = content_type.unwrap_or("");
    if !ct.starts_with("application/json") {
        return false;
    }
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    // Wrapped JSON-RPC envelope: { "error": { "code": -32016, ... } }
    if let Some(code) = value
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
    {
        if code == SESSION_NOT_FOUND_CODE {
            return true;
        }
    }
    // Bare SdkError object: { "code": -32016, ... }
    if value.get("code").and_then(|c| c.as_i64()) == Some(SESSION_NOT_FOUND_CODE) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repo_query_accepts_absolute_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let mut params = HashMap::new();
        params.insert("repo".to_string(), dir.as_str().to_string());
        assert_eq!(parse_repo_query(&params).as_deref(), Some(dir.as_path()));
    }

    #[test]
    fn parse_repo_query_rejects_missing() {
        let params = HashMap::new();
        assert!(parse_repo_query(&params).is_none());
    }

    #[test]
    fn parse_repo_query_rejects_relative() {
        let mut params = HashMap::new();
        params.insert("repo".to_string(), "relative/path".to_string());
        assert!(parse_repo_query(&params).is_none());
    }

    #[test]
    fn parse_repo_query_rejects_nonexistent() {
        let mut params = HashMap::new();
        params.insert(
            "repo".to_string(),
            "/this/path/does/not/exist/we/hope".to_string(),
        );
        assert!(parse_repo_query(&params).is_none());
    }

    #[test]
    fn hop_by_hop_drops_connection_class() {
        assert!(is_hop_by_hop("Connection"));
        assert!(is_hop_by_hop("Transfer-Encoding"));
        assert!(is_hop_by_hop("Upgrade"));
        assert!(is_hop_by_hop("Host"));
        // Things that must pass through:
        assert!(!is_hop_by_hop("Content-Type"));
        assert!(!is_hop_by_hop("Mcp-Session-Id"));
        assert!(!is_hop_by_hop("Authorization"));
    }

    #[test]
    fn parse_session_expired_matches_sdk_envelope() {
        let body = br#"{"jsonrpc":"2.0","error":{"code":-32016,"message":"Bad Request: Session not found","data":null},"id":null}"#;
        assert!(parse_session_expired(
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            body,
        ));
    }

    #[test]
    fn parse_session_expired_matches_bare_error_object() {
        // The SDK's error_response helper serializes SdkError directly, not
        // wrapped in a JSON-RPC envelope, so we must accept that shape too.
        let body = br#"{"code":-32016,"message":"Session not found","data":null}"#;
        assert!(parse_session_expired(
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            body,
        ));
    }

    #[test]
    fn parse_session_expired_rejects_other_errors() {
        let body = br#"{"code":-32600,"message":"Invalid Request"}"#;
        assert!(!parse_session_expired(
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            body,
        ));
    }

    #[test]
    fn parse_session_expired_rejects_2xx() {
        let body = br#"{"code":-32016,"message":"Session not found"}"#;
        assert!(!parse_session_expired(
            StatusCode::OK,
            Some("application/json"),
            body
        ));
    }

    #[test]
    fn parse_session_expired_rejects_non_json() {
        let body = b"event: message\ndata: {}\n\n";
        assert!(!parse_session_expired(
            StatusCode::BAD_REQUEST,
            Some("text/event-stream"),
            body,
        ));
    }

    #[test]
    fn parse_session_expired_handles_malformed_json() {
        let body = b"not json";
        assert!(!parse_session_expired(
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            body,
        ));
    }
}
