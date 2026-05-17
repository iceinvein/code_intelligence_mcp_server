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

/// Long-lived session-id → repo bindings, written by every successful
/// binding path (URL `?repo=`, MCP `roots/list`, and the `bind_workspace`
/// tool). Unlike `PendingRepos` (consume-once on first tool call), entries
/// here persist for the lifetime of the session bookkeeping and are dropped
/// in lockstep with session eviction. `recover_session` reads this map when
/// the recovering request did not carry `?repo=`, so `bind_workspace`- and
/// `roots/list`-bound sessions also survive an SDK eviction.
pub type BoundRepos = Arc<DashMap<String, Utf8PathBuf>>;

pub fn new_bound_repos() -> BoundRepos {
    Arc::new(DashMap::new())
}

/// Per-stale-sid recovery future, shared across concurrent requests that
/// all hit the same dead `mcp-session-id`. The first caller into a given
/// entry runs the initialize handshake; subsequent callers await its
/// result instead of independently allocating a second backend session.
type RecoveryCell = Arc<tokio::sync::OnceCell<Option<String>>>;
type Recoveries = Arc<DashMap<String, RecoveryCell>>;

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
    /// Stable session-id → repo bindings used by `recover_session` when the
    /// recovering request has no `?repo=` URL query (i.e. the original
    /// session was bound via `roots/list` or `bind_workspace`).
    bound_repos: BoundRepos,
    /// In-flight recoveries keyed by stale session id. Lets concurrent
    /// requests with the same stale id share a single backend session
    /// allocation instead of each minting a fresh one.
    recoveries: Recoveries,
    /// Sticky client-supplied stale sid → fresh sid we minted during
    /// recovery. Repeated requests bearing the same stale id (clients that
    /// cache a session id from a prior daemon run and never update it after
    /// `mcp-session-id` rotation) get rewritten to the recovered fresh id
    /// before reaching the upstream. Without this, every tool call after a
    /// daemon restart triggers a fresh recovery, allocates a fresh upstream
    /// session, and silently drops the `bind_workspace`/`roots/list` repo
    /// binding that was recorded against the previous one. Cleared lazily:
    /// if a remapped fresh id later goes stale itself, the next request
    /// runs recovery again and overwrites the entry with the newer fresh
    /// id. No active eviction is required because the map only grows by
    /// one entry per unique client-supplied stale id.
    stale_to_fresh: Arc<DashMap<String, String>>,
}

/// Spawn the proxy listener. Returns once it has bound to `public_port`.
pub async fn spawn_mcp_proxy(
    host: &str,
    public_port: u16,
    backend_port: u16,
    backend_path: &str,
    pending_repos: PendingRepos,
    bound_repos: BoundRepos,
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
        bound_repos,
        recoveries: Arc::new(DashMap::new()),
        stale_to_fresh: Arc::new(DashMap::new()),
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

    // Extract the client's stale session id (if any) before consuming the
    // headers. `recover_session` uses it to look up a `bind_workspace`- or
    // `roots/list`-recorded binding when the request did not carry `?repo=`.
    let client_sid = out_headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Sticky remap. If we have already recovered this client-supplied sid
    // to a fresh upstream sid in a previous request, rewrite the outbound
    // header now. This lets repeated requests carrying a stale id (clients
    // that cached a session id from a previous daemon run and never
    // update it) reuse the same recovered session instead of each minting
    // a brand-new one. Without this, `bind_workspace` lands on session
    // N1, the next tool call recovers onto N2 (which has no recorded
    // binding), and the agent sees "Session not bound" on every call.
    let remapped_sid = client_sid.as_deref().and_then(|s| {
        state
            .stale_to_fresh
            .get(s)
            .map(|entry| entry.value().clone())
    });
    if let Some(target) = &remapped_sid {
        if let Ok(hv) = reqwest::header::HeaderValue::from_str(target) {
            out_headers.insert(
                reqwest::header::HeaderName::from_static("mcp-session-id"),
                hv,
            );
            tracing::debug!(
                client_sid = %client_sid.as_deref().unwrap_or(""),
                upstream_sid = %target,
                "proxy: applied sticky session remap"
            );
        }
    }

    // The sid the upstream actually saw. We use this as the dedupe key for
    // concurrent recoveries and as the lookup key for `bound_repos` if the
    // remapped sid is itself stale: a `bind_workspace` recorded on a
    // previous fresh sid sits under that fresh key, not the client's
    // original sid.
    let upstream_sid = remapped_sid.clone().or_else(|| client_sid.clone());

    // Clone so we can replay on session-expired without rebuilding from
    // the original axum request (which has already been consumed).
    let replay_headers = out_headers.clone();
    let replay_body = body_bytes.clone();
    let replay_params = params.clone();

    let first = send_upstream_once(
        state.clone(),
        method.clone(),
        params,
        out_headers,
        body_bytes,
        repo_query.clone(),
    )
    .await;

    let original_response = match first {
        UpstreamOutcome::Done(resp) => return resp,
        UpstreamOutcome::RetryRequested { original_response } => original_response,
    };

    // Session expired upstream. Try once to recover. When multiple
    // requests with the same stale id arrive concurrently they share one
    // recovery via a per-sid OnceCell, so only one backend session is
    // allocated. Requests without a stale id (rare; client never sent
    // `mcp-session-id`) recover directly with no dedupe key.
    let new_sid_opt = match upstream_sid.as_deref() {
        Some(sid) => {
            let cell: RecoveryCell = state
                .recoveries
                .entry(sid.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone();
            let state_for_init = state.clone();
            let repo_for_init = repo_query.clone();
            let sid_for_init = sid.to_string();
            let result = cell
                .get_or_init(|| async move {
                    recover_session(&state_for_init, repo_for_init.as_ref(), Some(&sid_for_init))
                        .await
                })
                .await
                .clone();
            // Best-effort cleanup so a future eviction-and-recovery cycle
            // on the same upstream sid can run again. Concurrent removers
            // are harmless (DashMap.remove is idempotent).
            state.recoveries.remove(sid);
            result
        }
        None => recover_session(&state, repo_query.as_ref(), None).await,
    };
    let new_sid = match new_sid_opt {
        Some(s) => s,
        None => return original_response,
    };

    // Persist the sticky remap so future requests with this client sid
    // skip recovery entirely. Updating in place when the remapped target
    // itself went stale (`remapped_sid.is_some()`) keeps the chain pinned
    // to whichever fresh id is currently live.
    if let Some(cs) = &client_sid {
        let first_remap = state
            .stale_to_fresh
            .insert(cs.clone(), new_sid.clone())
            .is_none();
        if first_remap {
            tracing::info!(
                client_sid = %cs,
                upstream_sid = %new_sid,
                "proxy: pinned client sid to recovered upstream sid"
            );
        } else {
            tracing::debug!(
                client_sid = %cs,
                upstream_sid = %new_sid,
                "proxy: refreshed remapped upstream sid"
            );
        }
    }

    // Substitute the new session id into the replay headers.
    let mut retry_headers = replay_headers;
    if let Ok(hv) = reqwest::header::HeaderValue::from_str(&new_sid) {
        retry_headers.insert(
            reqwest::header::HeaderName::from_static("mcp-session-id"),
            hv,
        );
    }

    let second = send_upstream_once(
        state,
        method,
        replay_params,
        retry_headers,
        replay_body,
        repo_query,
    )
    .await;

    match second {
        UpstreamOutcome::Done(resp) => resp,
        // Recovery itself was rejected by upstream a second time. Do NOT
        // loop; return the second response to the client.
        UpstreamOutcome::RetryRequested { original_response } => original_response,
    }
}

/// Issue a single upstream request and translate its response back into an
/// axum `Response`. Captures the `mcp-session-id` -> `?repo=` binding on
/// the way back when both are present. Streams the body without buffering.
///
/// Extracted from `forward` so we can call it twice in a row from the
/// session-recovery path without duplicating the streaming / header-translation
/// glue. Returns `UpstreamOutcome::RetryRequested` when the upstream signals
/// session-not-found (-32016) so `forward` can run recovery and replay.
async fn send_upstream_once(
    state: Arc<ProxyState>,
    method: Method,
    params: HashMap<String, String>,
    out_headers: reqwest::header::HeaderMap,
    body_bytes: axum::body::Bytes,
    repo_query: Option<Utf8PathBuf>,
) -> UpstreamOutcome {
    let url = format!("{}{}", state.backend_base, state.backend_path);

    let request_builder = state
        .client
        .request(method, &url)
        .headers(out_headers)
        .query(&params)
        .body(body_bytes);

    let response = match request_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "proxy: upstream request failed");
            return UpstreamOutcome::Done(
                (
                    StatusCode::BAD_GATEWAY,
                    format!("upstream unreachable: {e}"),
                )
                    .into_response(),
            );
        }
    };

    let status = response.status();
    let upstream_headers = response.headers().clone();

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

    let content_type_owned: Option<String> = upstream_headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let small_json_error = status.is_client_error()
        && content_type_owned
            .as_deref()
            .map(|ct| ct.starts_with("application/json"))
            .unwrap_or(false);

    let body_for_response: Body = if small_json_error {
        // Buffer up to 64 KiB. SDK error envelopes are tiny; anything
        // larger is unexpected and we forward without inspection.
        const MAX_ERROR_BUFFER: usize = 64 * 1024;
        let bytes = match response.bytes().await {
            Ok(b) if b.len() <= MAX_ERROR_BUFFER => b,
            Ok(b) => {
                tracing::warn!(
                    len = b.len(),
                    "proxy: 4xx response exceeded inspect buffer; forwarding raw"
                );
                let mut out = Response::new(Body::from(b));
                *out.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
                copy_response_headers(&upstream_headers, out.headers_mut());
                return UpstreamOutcome::Done(out);
            }
            Err(e) => {
                tracing::error!(error = %e, "proxy: failed to read 4xx response body");
                return UpstreamOutcome::Done(
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("upstream body read failed: {e}"),
                    )
                        .into_response(),
                );
            }
        };
        // Probe for the session-expired signal. If matched, surface
        // RetryRequested with the original response embedded so the
        // caller can return it if recovery fails.
        if parse_session_expired(status, content_type_owned.as_deref(), &bytes) {
            let mut fallback = Response::new(Body::from(bytes));
            *fallback.status_mut() =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
            copy_response_headers(&upstream_headers, fallback.headers_mut());
            return UpstreamOutcome::RetryRequested {
                original_response: fallback,
            };
        }
        Body::from(bytes)
    } else {
        // Stream the response body straight back. SSE responses must not be
        // buffered: clients block waiting for `event:` frames on the same
        // connection.
        let stream = response.bytes_stream();
        let mapped =
            stream.map_err(|e| std::io::Error::other(format!("proxy upstream stream error: {e}")));
        Body::from_stream(mapped)
    };

    let mut out = Response::new(body_for_response);
    *out.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    copy_response_headers(&upstream_headers, out.headers_mut());
    UpstreamOutcome::Done(out)
}

/// Copy non-hop-by-hop headers from a reqwest response into an axum
/// response. Pulled out of `send_upstream_once` so the early-return path
/// for oversize 4xx bodies can reuse it.
fn copy_response_headers(from: &reqwest::header::HeaderMap, to: &mut axum::http::HeaderMap) {
    for (name, value) in from.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) {
            continue;
        }
        if let Ok(hn) = axum::http::HeaderName::from_bytes(n.as_bytes()) {
            if let Ok(hv) = HeaderValue::from_bytes(value.as_bytes()) {
                to.insert(hn, hv);
            }
        }
    }
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

/// Outcome of a single upstream forward attempt. `Done` carries the
/// final axum response to return to the client; `RetryRequested` means
/// the upstream signalled session-not-found and the caller should run
/// recovery + replay once.
enum UpstreamOutcome {
    Done(Response),
    RetryRequested {
        /// Original 4xx body we already consumed; surfaced if recovery
        /// fails so the client still sees the real error.
        original_response: Response,
    },
}

/// JSON-RPC error code emitted by rust-mcp-sdk when the `mcp-session-id` is
/// either missing or refers to a session the SDK no longer tracks. See
/// `rust-mcp-sdk/src/mcp_http/mcp_http_handler.rs` (`error_response`) and
/// the constant table in `rust-mcp-sdk/src/error.rs`. We treat both the
/// wrapped JSON-RPC envelope (`{"error": {"code": -32016, ...}}`) and the
/// bare error object (which is what `error_response` actually serializes)
/// as session-expired signals.
const SESSION_NOT_FOUND_CODE: i64 = -32016;

/// Returns `true` when an upstream response indicates that the SDK does
/// not know the `mcp-session-id` we forwarded. Only inspects `4xx`
/// responses with a JSON content-type and a body small enough to have
/// already been buffered by `forward`; this function does not perform I/O.
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

/// Synthesize an MCP `initialize` + `notifications/initialized` handshake
/// against the internal SDK so we obtain a fresh `mcp-session-id`. Returns
/// the new session id on success, or `None` on any failure (the caller
/// surfaces the original error to the client in that case).
///
/// We do not preserve the client's original initialize params: the SDK
/// uses them only for capability negotiation, and the proxy is acting as
/// a session-revival mechanism, not a full client. Tool calls we replay
/// afterwards do not depend on negotiated capabilities beyond the
/// JSON-RPC framing, which is identical.
async fn recover_session(
    state: &ProxyState,
    repo_query: Option<&Utf8PathBuf>,
    stale_sid: Option<&str>,
) -> Option<String> {
    let url = format!("{}{}", state.backend_base, state.backend_path);

    // Prefer the URL `?repo=` (highest-priority binding source). When that's
    // missing, fall back to a previously-recorded binding for the stale
    // session id, which covers `bind_workspace` and `roots/list` sessions.
    let bound_repo_owned: Option<Utf8PathBuf> = match repo_query {
        Some(_) => None,
        None => stale_sid.and_then(|sid| state.bound_repos.get(sid).map(|r| r.value().clone())),
    };
    let effective_repo: Option<&Utf8PathBuf> = repo_query.or(bound_repo_owned.as_ref());

    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(repo) = effective_repo {
        query.push(("repo", repo.as_str().to_string()));
    }

    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "code-intelligence-proxy-recover",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    });

    let init_resp = match state
        .client
        .post(&url)
        .query(&query)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(init_body.to_string())
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(status = %r.status(), "session recovery: initialize failed");
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e, "session recovery: initialize transport error");
            return None;
        }
    };

    let new_sid = init_resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())?;

    // Record the binding eagerly so the StandaloneHandler sees the repo on
    // the replay even before the SDK has fully completed the initialized
    // notification round-trip. Write to both maps: `pending_repos` is the
    // one-shot signal `StandaloneHandler::resolve_state` consumes on the
    // first tool call; `bound_repos` is the durable cache used by future
    // recoveries on the new session id.
    if let Some(repo) = effective_repo {
        state.pending_repos.insert(new_sid.clone(), repo.clone());
        state.bound_repos.insert(new_sid.clone(), repo.clone());
    }

    // Drain the initialize response body so the connection is reusable.
    let _ = init_resp.bytes().await;

    let initialized_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    if let Err(e) = state
        .client
        .post(&url)
        .query(&query)
        .header("mcp-session-id", &new_sid)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(initialized_body.to_string())
        .send()
        .await
    {
        tracing::warn!(error = %e, "session recovery: notifications/initialized failed");
        // Continue anyway -- the replay will surface any real failure.
    }

    tracing::info!(
        new_session = %new_sid,
        repo = %repo_query.map(|p| p.as_str()).unwrap_or("<none>"),
        "session recovery: minted fresh session and replaying"
    );
    Some(new_sid)
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

    use axum::{routing::post as axum_post, Router as AxumRouter};
    use std::net::SocketAddr;

    async fn spawn_test_upstream<F>(handler: F) -> SocketAddr
    where
        F: Fn(HeaderMap, axum::body::Bytes) -> Response + Clone + Send + Sync + 'static,
    {
        // Bind a fake SDK upstream on a random port. The handler is a plain
        // closure so each test can script its own response sequence.
        let app = AxumRouter::new().route(
            "/mcp",
            axum_post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let h = handler.clone();
                async move { h(headers, body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn forward_buffers_4xx_for_inspection() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let upstream = spawn_test_upstream(move |_h, _b| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                r#"{"code":-32600,"message":"Invalid Request"}"#,
            )
                .into_response()
        })
        .await;

        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            new_pending_repos(),
            new_bound_repos(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/mcp"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // The proxy must NOT have retried for a non-session error.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The body must be forwarded intact even after buffering.
        assert_eq!(
            resp.text().await.unwrap(),
            r#"{"code":-32600,"message":"Invalid Request"}"#
        );
    }

    #[tokio::test]
    async fn forward_recovers_from_session_not_found_and_replays_once() {
        // Fake SDK that:
        //   1. Returns -32016 the first time a tool call arrives with a
        //      stale session id.
        //   2. Accepts the proxy's synthetic initialize, returns a fresh
        //      mcp-session-id header.
        //   3. Accepts the notifications/initialized POST.
        //   4. Returns 200 + tool result on the replayed tool call.
        let state = Arc::new(std::sync::Mutex::new(0u32));
        let state_for_handler = state.clone();
        let upstream = spawn_test_upstream(move |headers: HeaderMap, body: axum::body::Bytes| {
            let body_str = std::str::from_utf8(&body).unwrap_or("");
            let mut s = state_for_handler.lock().unwrap();
            *s += 1;
            match *s {
                1 => {
                    // Stale tool call: must include the client's session id.
                    assert_eq!(
                        headers.get("mcp-session-id").map(|v| v.to_str().unwrap()),
                        Some("stale-sid")
                    );
                    (
                        StatusCode::BAD_REQUEST,
                        [("content-type", "application/json")],
                        r#"{"code":-32016,"message":"Session not found","data":null}"#,
                    )
                        .into_response()
                }
                2 => {
                    // Synthetic initialize from the proxy.
                    assert!(body_str.contains("\"method\":\"initialize\""));
                    let mut resp = (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                    resp
                }
                3 => {
                    // notifications/initialized -- fire and forget.
                    assert!(body_str.contains("notifications/initialized"));
                    assert_eq!(
                        headers.get("mcp-session-id").map(|v| v.to_str().unwrap()),
                        Some("fresh-sid")
                    );
                    StatusCode::ACCEPTED.into_response()
                }
                4 => {
                    // Replay of the original tool call, now under the new id.
                    assert_eq!(
                        headers.get("mcp-session-id").map(|v| v.to_str().unwrap()),
                        Some("fresh-sid")
                    );
                    let mut resp = (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        r#"{"jsonrpc":"2.0","id":42,"result":{"ok":true}}"#,
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                    resp
                }
                _ => unreachable!("proxy made an unexpected {}-th upstream call", *s),
            }
        })
        .await;

        let pending = new_pending_repos();
        let bound = new_bound_repos();
        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            pending.clone(),
            bound.clone(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/mcp?repo={repo}"))
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("mcp-session-id")
                .map(|v| v.to_str().unwrap()),
            Some("fresh-sid")
        );
        let body = resp.text().await.unwrap();
        assert!(body.contains("\"ok\":true"));

        // The proxy must have made exactly 4 upstream calls: the failed
        // original, the recovery initialize, the recovery initialized
        // notification, and the replay.
        assert_eq!(*state.lock().unwrap(), 4);

        // ?repo= must have been re-bound to the new session id.
        assert!(pending.contains_key("fresh-sid"));
    }

    #[tokio::test]
    async fn forward_does_not_recover_more_than_once_per_request() {
        // If recovery itself yields another -32016, we surface the second
        // failure to the client rather than looping.
        let state = Arc::new(std::sync::Mutex::new(0u32));
        let state_for_handler = state.clone();
        let upstream = spawn_test_upstream(move |_h: HeaderMap, _b: axum::body::Bytes| {
            let mut s = state_for_handler.lock().unwrap();
            *s += 1;
            (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                r#"{"code":-32016,"message":"Session not found"}"#,
            )
                .into_response()
        })
        .await;

        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            new_pending_repos(),
            new_bound_repos(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/mcp"))
            .header("mcp-session-id", "stale-sid")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // The proxy should give up after: original (1) + initialize attempt (2).
        // It MUST NOT keep looping.
        assert!(*state.lock().unwrap() <= 2, "proxy retried more than once");
    }

    #[tokio::test]
    async fn forward_passes_through_2xx_response_unchanged() {
        let upstream = spawn_test_upstream(|_h, _b| {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"ok":true}"#,
            )
                .into_response()
        })
        .await;

        // Start the proxy in front of it on another random port.
        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            new_pending_repos(),
            new_bound_repos(),
        )
        .await
        .unwrap();

        // Drive a request through the proxy.
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/mcp"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.text().await.unwrap();
        assert_eq!(body, r#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn forward_recovers_using_bound_repos_when_url_has_no_repo() {
        // Same shape as forward_recovers_from_session_not_found_and_replays_once,
        // but the client does NOT pass `?repo=` in the URL. Instead, the
        // stale session id has a prior binding in `bound_repos` (as would
        // be left by `bind_workspace` or `roots/list`). The proxy must
        // discover that binding and re-apply it on the recovered session.
        let state = Arc::new(std::sync::Mutex::new(0u32));
        let state_for_handler = state.clone();
        let tmp = tempfile::tempdir().unwrap();
        let repo_path = tmp.path().to_str().unwrap().to_string();
        let upstream = spawn_test_upstream(move |headers: HeaderMap, body: axum::body::Bytes| {
            let body_str = std::str::from_utf8(&body).unwrap_or("");
            let mut s = state_for_handler.lock().unwrap();
            *s += 1;
            match *s {
                1 => (
                    StatusCode::BAD_REQUEST,
                    [("content-type", "application/json")],
                    r#"{"code":-32016,"message":"Session not found"}"#,
                )
                    .into_response(),
                2 => {
                    assert!(body_str.contains("\"method\":\"initialize\""));
                    let mut resp = (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                    resp
                }
                3 => {
                    assert!(body_str.contains("notifications/initialized"));
                    StatusCode::ACCEPTED.into_response()
                }
                4 => {
                    assert_eq!(
                        headers.get("mcp-session-id").map(|v| v.to_str().unwrap()),
                        Some("fresh-sid")
                    );
                    let mut resp = (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        r#"{"jsonrpc":"2.0","id":42,"result":{"ok":true}}"#,
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                    resp
                }
                _ => unreachable!("proxy made an unexpected {}-th upstream call", *s),
            }
        })
        .await;

        let pending = new_pending_repos();
        let bound = new_bound_repos();
        // Pre-seed bound_repos as bind_workspace / roots-list would.
        bound.insert(
            "stale-sid".to_string(),
            Utf8PathBuf::from(repo_path.clone()),
        );

        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            pending.clone(),
            bound.clone(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let resp = client
            // NOTE: no `?repo=` on this URL — recovery must read from bound_repos.
            .post(format!("http://127.0.0.1:{proxy_port}/mcp"))
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("mcp-session-id")
                .map(|v| v.to_str().unwrap()),
            Some("fresh-sid")
        );
        assert_eq!(*state.lock().unwrap(), 4);
        // Recovery must rehydrate the binding under the new session id in
        // both maps so subsequent recoveries on `fresh-sid` (if it dies
        // later) also work.
        assert!(pending.contains_key("fresh-sid"));
        assert!(bound.contains_key("fresh-sid"));
    }

    #[tokio::test]
    async fn forward_dedupes_concurrent_recoveries_for_same_stale_sid() {
        // Two concurrent client requests carrying the same stale
        // `mcp-session-id` must share one backend recovery. The upstream's
        // initialize handler sleeps so both stale POSTs reach the
        // recovery phase before either initialize completes, then a
        // single initialize is observed.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let stale_count = Arc::new(AtomicUsize::new(0));
        let init_count = Arc::new(AtomicUsize::new(0));
        let notif_count = Arc::new(AtomicUsize::new(0));
        let replay_count = Arc::new(AtomicUsize::new(0));
        let sc = stale_count.clone();
        let ic = init_count.clone();
        let nc = notif_count.clone();
        let rc = replay_count.clone();

        let upstream = spawn_test_upstream(move |headers: HeaderMap, body: axum::body::Bytes| {
            let body_str = std::str::from_utf8(&body).unwrap_or("");
            let is_init = body_str.contains("\"method\":\"initialize\"");
            let is_notif = body_str.contains("notifications/initialized");
            let sid_header = headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if is_init {
                ic.fetch_add(1, Ordering::SeqCst);
                let _ = (&sc, &nc, &rc); // keep clones for the non-init arms
                                         // NOTE: synchronous sleep here (we're inside a sync closure
                                         // adapter in spawn_test_upstream's wrapper, which itself
                                         // wraps the call in an async block). Use a short tokio
                                         // sleep below via std::thread::sleep equivalent: just
                                         // delay enough to let the second stale-POST arrive.
                std::thread::sleep(std::time::Duration::from_millis(150));
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                resp
            } else if is_notif {
                nc.fetch_add(1, Ordering::SeqCst);
                StatusCode::ACCEPTED.into_response()
            } else if sid_header.as_deref() == Some("stale-sid") {
                sc.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_REQUEST,
                    [("content-type", "application/json")],
                    r#"{"code":-32016,"message":"Session not found"}"#,
                )
                    .into_response()
            } else {
                // Replay arrives with fresh-sid.
                rc.fetch_add(1, Ordering::SeqCst);
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                resp
            }
        })
        .await;
        // Originals stay accessible for assertions; the closure owns the clones.

        let pending = new_pending_repos();
        let bound = new_bound_repos();
        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            pending.clone(),
            bound.clone(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{proxy_port}/mcp");
        let r1 = {
            let c = client.clone();
            let u = url.clone();
            tokio::spawn(async move {
                c.post(&u)
                    .header("mcp-session-id", "stale-sid")
                    .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#)
                    .send()
                    .await
                    .unwrap()
            })
        };
        let r2 = {
            let c = client.clone();
            let u = url.clone();
            tokio::spawn(async move {
                c.post(&u)
                    .header("mcp-session-id", "stale-sid")
                    .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}"#)
                    .send()
                    .await
                    .unwrap()
            })
        };
        let (a, b) = tokio::join!(r1, r2);
        let resp_a = a.unwrap();
        let resp_b = b.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);
        assert_eq!(resp_b.status(), StatusCode::OK);

        assert_eq!(stale_count.load(Ordering::SeqCst), 2, "two stale POSTs");
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "dedupe must collapse two recoveries into one initialize"
        );
        assert_eq!(notif_count.load(Ordering::SeqCst), 1, "one initialized");
        assert_eq!(replay_count.load(Ordering::SeqCst), 2, "two replays");
    }

    #[tokio::test]
    async fn forward_sticky_remap_skips_recovery_on_repeat_requests() {
        // Two SEQUENTIAL client requests, both bearing the same stale sid.
        // The first triggers recovery (1 stale + 1 init + 1 notif + 1 replay).
        // The second must be rewritten to the recovered fresh sid BEFORE it
        // hits the upstream, so we only see one extra 200 with no further
        // -32016 nor a second initialize. Without sticky remap, every later
        // request would re-recover, allocate a new session, and silently
        // discard any `bind_workspace` recorded on the previous one.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let stale_count = Arc::new(AtomicUsize::new(0));
        let init_count = Arc::new(AtomicUsize::new(0));
        let notif_count = Arc::new(AtomicUsize::new(0));
        let ok_with_fresh = Arc::new(AtomicUsize::new(0));
        let sc = stale_count.clone();
        let ic = init_count.clone();
        let nc = notif_count.clone();
        let oc = ok_with_fresh.clone();

        let upstream = spawn_test_upstream(move |headers: HeaderMap, body: axum::body::Bytes| {
            let body_str = std::str::from_utf8(&body).unwrap_or("");
            let sid = headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if body_str.contains("\"method\":\"initialize\"") {
                ic.fetch_add(1, Ordering::SeqCst);
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                resp
            } else if body_str.contains("notifications/initialized") {
                nc.fetch_add(1, Ordering::SeqCst);
                StatusCode::ACCEPTED.into_response()
            } else if sid.as_deref() == Some("stale-sid") {
                sc.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_REQUEST,
                    [("content-type", "application/json")],
                    r#"{"code":-32016,"message":"Session not found"}"#,
                )
                    .into_response()
            } else if sid.as_deref() == Some("fresh-sid") {
                oc.fetch_add(1, Ordering::SeqCst);
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_static("fresh-sid"));
                resp
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        })
        .await;

        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            new_pending_repos(),
            new_bound_repos(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{proxy_port}/mcp");

        // First request: triggers recovery.
        let r1 = client
            .post(&url)
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);

        assert_eq!(
            stale_count.load(Ordering::SeqCst),
            1,
            "first request stale-POSTs once"
        );
        assert_eq!(init_count.load(Ordering::SeqCst), 1, "first recovery init");
        assert_eq!(
            notif_count.load(Ordering::SeqCst),
            1,
            "first recovery notif"
        );
        assert_eq!(ok_with_fresh.load(Ordering::SeqCst), 1, "first replay");

        // Second request: SAME client-supplied stale sid. Must be rewritten
        // by the proxy before hitting upstream, so the upstream sees
        // `fresh-sid` directly and never returns -32016.
        let r2 = client
            .post(&url)
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);

        assert_eq!(
            stale_count.load(Ordering::SeqCst),
            1,
            "remap must skip the stale POST entirely"
        );
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "remap must skip a second recovery initialize"
        );
        assert_eq!(notif_count.load(Ordering::SeqCst), 1, "no second notif");
        assert_eq!(
            ok_with_fresh.load(Ordering::SeqCst),
            2,
            "second request must land on fresh-sid directly"
        );
    }

    #[tokio::test]
    async fn forward_sticky_remap_refreshes_when_remapped_sid_dies() {
        // Map stale-sid -> fresh-1 via a first recovery. Then make fresh-1
        // itself go stale: the next request with stale-sid will be rewritten
        // to fresh-1, get a -32016, run recovery (keyed on fresh-1 since
        // that's what upstream saw), mint fresh-2, replay. The map must
        // advance to stale-sid -> fresh-2 so further requests with stale-sid
        // route directly to fresh-2 without another recovery.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let phase = Arc::new(std::sync::Mutex::new(1u8));
        let stale_count = Arc::new(AtomicUsize::new(0));
        let init_count = Arc::new(AtomicUsize::new(0));
        let dead_fresh_calls = Arc::new(AtomicUsize::new(0));
        let live_fresh_calls = Arc::new(AtomicUsize::new(0));
        let p = phase.clone();
        let sc = stale_count.clone();
        let ic = init_count.clone();
        let dfc = dead_fresh_calls.clone();
        let lfc = live_fresh_calls.clone();

        let upstream = spawn_test_upstream(move |headers: HeaderMap, body: axum::body::Bytes| {
            let body_str = std::str::from_utf8(&body).unwrap_or("");
            let sid = headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if body_str.contains("\"method\":\"initialize\"") {
                let n = ic.fetch_add(1, Ordering::SeqCst) + 1;
                let fresh = if n == 1 { "fresh-1" } else { "fresh-2" };
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_str(fresh).unwrap());
                resp
            } else if body_str.contains("notifications/initialized") {
                StatusCode::ACCEPTED.into_response()
            } else if sid.as_deref() == Some("stale-sid") {
                sc.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_REQUEST,
                    [("content-type", "application/json")],
                    r#"{"code":-32016,"message":"Session not found"}"#,
                )
                    .into_response()
            } else if sid.as_deref() == Some("fresh-1") {
                let phase = *p.lock().unwrap();
                if phase == 1 {
                    // Phase 1: fresh-1 is live; succeed.
                    let mut resp = (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#,
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert("mcp-session-id", HeaderValue::from_static("fresh-1"));
                    resp
                } else {
                    // Phase 2+: fresh-1 has died; force a re-recovery.
                    dfc.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        [("content-type", "application/json")],
                        r#"{"code":-32016,"message":"Session not found"}"#,
                    )
                        .into_response()
                }
            } else if sid.as_deref() == Some("fresh-2") {
                lfc.fetch_add(1, Ordering::SeqCst);
                let mut resp = (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("mcp-session-id", HeaderValue::from_static("fresh-2"));
                resp
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        })
        .await;

        let proxy_port = portpicker::pick_unused_port().expect("free port");
        spawn_mcp_proxy(
            "127.0.0.1",
            proxy_port,
            upstream.port(),
            "/mcp",
            new_pending_repos(),
            new_bound_repos(),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{proxy_port}/mcp");

        // Phase 1: first request pins stale-sid -> fresh-1.
        let r1 = client
            .post(&url)
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        assert_eq!(init_count.load(Ordering::SeqCst), 1, "one init in phase 1");

        // Phase 2: simulate fresh-1 going stale upstream.
        *phase.lock().unwrap() = 2;

        let r2 = client
            .post(&url)
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);

        // The second request should have been remapped to fresh-1 first,
        // then re-recovered onto fresh-2.
        assert_eq!(
            stale_count.load(Ordering::SeqCst),
            1,
            "stale-sid should never reach upstream after the first request"
        );
        assert_eq!(
            dead_fresh_calls.load(Ordering::SeqCst),
            1,
            "fresh-1 stale once"
        );
        assert_eq!(init_count.load(Ordering::SeqCst), 2, "second recovery init");
        assert_eq!(
            live_fresh_calls.load(Ordering::SeqCst),
            1,
            "replay on fresh-2"
        );

        // Phase 3: a follow-up request with stale-sid must now route to
        // fresh-2 directly (no third recovery).
        let r3 = client
            .post(&url)
            .header("mcp-session-id", "stale-sid")
            .body(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(r3.status(), StatusCode::OK);
        assert_eq!(init_count.load(Ordering::SeqCst), 2, "no third init");
        assert_eq!(
            live_fresh_calls.load(Ordering::SeqCst),
            2,
            "follow-up lands on fresh-2 directly"
        );
    }
}
