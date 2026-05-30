//! Shared Origin-header guard for localhost-only HTTP surfaces.

use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};

/// Reject browser requests whose `Origin` is not the same localhost port as
/// the request. Requests without `Origin` are non-browser clients such as MCP
/// agents, curl, and lifecycle commands.
pub async fn check_origin(req: Request, next: Next) -> Result<Response, StatusCode> {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let origin = origin.to_str().map_err(|_| StatusCode::FORBIDDEN)?;
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::FORBIDDEN)?;
        if !is_same_localhost_origin(origin, host) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(next.run(req).await)
}

fn is_same_localhost_origin(origin: &str, request_host: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return false;
    }
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    if !is_localhost_name(origin_host) {
        return false;
    }
    let Some(request_port) = authority_port(request_host) else {
        return false;
    };
    url.port_or_known_default() == Some(request_port)
}

fn is_localhost_name(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
}

fn authority_port(authority: &str) -> Option<u16> {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[') {
        let after_bracket = rest.split_once(']')?.1;
        return after_bracket.strip_prefix(':')?.parse().ok();
    }
    authority.rsplit_once(':')?.1.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_localhost_origin_accepts_matching_port_variants() {
        assert!(is_same_localhost_origin(
            "http://localhost:17802",
            "127.0.0.1:17802"
        ));
        assert!(is_same_localhost_origin(
            "http://127.0.0.1:17802",
            "localhost:17802"
        ));
        assert!(is_same_localhost_origin(
            "http://[::1]:17802",
            "[::1]:17802"
        ));
    }

    #[test]
    fn same_localhost_origin_rejects_remote_or_malformed_origins() {
        assert!(!is_same_localhost_origin(
            "https://example.com",
            "127.0.0.1:17802"
        ));
        assert!(!is_same_localhost_origin(
            "http://attacker.localhost.evil:17802",
            "127.0.0.1:17802"
        ));
        assert!(!is_same_localhost_origin("127.0.0.1", "127.0.0.1:17802"));
    }

    #[test]
    fn same_localhost_origin_rejects_different_ports() {
        assert!(!is_same_localhost_origin(
            "http://localhost:3000",
            "127.0.0.1:17802"
        ));
    }
}
