//! `.well-known/mcp` discovery endpoint for standalone mode.
//!
//! Exposes a lightweight HTTP endpoint that returns MCP server metadata,
//! allowing clients to auto-discover the standalone server's transport URL
//! and capabilities.

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use std::net::SocketAddr;

/// Spawn the discovery HTTP server in the background.
///
/// Binds to `discovery_port` and serves a single `GET /.well-known/mcp` route
/// returning JSON metadata about the MCP server running on `mcp_host:mcp_port`.
pub async fn spawn_discovery_server(
    mcp_host: &str,
    mcp_port: u16,
    discovery_port: u16,
) -> anyhow::Result<()> {
    let response_body = build_discovery_response(mcp_host, mcp_port);

    let app = Router::new().route(
        "/.well-known/mcp",
        get(move || {
            let body = response_body.clone();
            async move { Json(body) }
        }),
    );

    let addr: SocketAddr = format!("{}:{}", mcp_host, discovery_port)
        .parse()
        .map_err(|e| {
            anyhow::anyhow!(
                "Invalid discovery address {}:{}: {}",
                mcp_host,
                discovery_port,
                e
            )
        })?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(
        discovery_port = discovery_port,
        mcp_url = %format!("http://{}:{}/mcp", mcp_host, mcp_port),
        "Discovery endpoint available at http://{}:{}/.well-known/mcp",
        mcp_host,
        discovery_port,
    );

    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });

    Ok(())
}

/// Build the JSON response for the discovery endpoint.
pub fn build_discovery_response(mcp_host: &str, mcp_port: u16) -> Value {
    json!({
        "mcp": {
            "version": "2025-11-25",
            "transport": {
                "type": "streamable-http",
                "url": format!("http://{}:{}/mcp", mcp_host, mcp_port),
            },
            "server": {
                "name": "code-intelligence",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "tools": true,
                "roots": true,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_response_shape() {
        let response = build_discovery_response("127.0.0.1", 3333);

        // Top-level "mcp" key exists
        let mcp = response.get("mcp").expect("missing 'mcp' key");

        // Version
        assert_eq!(
            mcp.get("version").and_then(|v| v.as_str()),
            Some("2025-11-25"),
            "MCP version should be 2025-11-25"
        );

        // Transport
        let transport = mcp.get("transport").expect("missing 'transport' key");
        assert_eq!(
            transport.get("type").and_then(|v| v.as_str()),
            Some("streamable-http")
        );
        assert_eq!(
            transport.get("url").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:3333/mcp")
        );

        // Server
        let server = mcp.get("server").expect("missing 'server' key");
        assert_eq!(
            server.get("name").and_then(|v| v.as_str()),
            Some("code-intelligence")
        );
        assert!(
            server.get("version").and_then(|v| v.as_str()).is_some(),
            "server version should be present"
        );

        // Capabilities
        let capabilities = mcp.get("capabilities").expect("missing 'capabilities' key");
        assert_eq!(
            capabilities.get("tools").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            capabilities.get("roots").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn discovery_response_uses_dynamic_host_port() {
        let response = build_discovery_response("0.0.0.0", 4444);
        let url = response["mcp"]["transport"]["url"].as_str().unwrap();
        assert_eq!(url, "http://0.0.0.0:4444/mcp");
    }

    #[test]
    fn discovery_response_version_matches_cargo() {
        let response = build_discovery_response("localhost", 3333);
        let version = response["mcp"]["server"]["version"].as_str().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }
}
