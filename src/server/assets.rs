//! Embedded single-page-app serving for the web portal.
//!
//! In release builds `rust-embed` bakes `ui/dist/` into the binary. In debug
//! builds (no `debug-embed` feature) it reads `ui/dist/` from disk relative to
//! the crate root, so `cargo run` serves whatever `bun run build` last wrote.
//!
//! Any path that is not an embedded asset and is not under `/api` falls back to
//! `index.html` so client-side routes (e.g. `/repos`) load the SPA.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/dist"]
struct Assets;

/// True for request paths the SPA fallback must NOT swallow.
pub fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// Serve an embedded asset for `uri`, falling back to `index.html` for SPA
/// routes. `/api/*` misses return a JSON 404 (the SPA never owns those).
pub async fn serve_spa(uri: Uri) -> Response {
    serve_from::<Assets>(uri.path())
}

fn serve_from<E: RustEmbed>(path: &str) -> Response {
    if is_api_path(path) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"unknown api route"}"#,
        )
            .into_response();
    }

    let trimmed = path.trim_start_matches('/');
    if let Some(content) = E::get(trimmed) {
        return asset_response(trimmed, content.data.into_owned());
    }

    // SPA fallback: serve index.html for client-side routes.
    match E::get("index.html") {
        Some(content) => asset_response("index.html", content.data.into_owned()),
        None => (StatusCode::NOT_FOUND, "ui not built").into_response(),
    }
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.as_ref())],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[derive(RustEmbed)]
    #[folder = "tests/fixtures/ui_assets"]
    struct TestAssets;

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn is_api_path_matches_only_api_routes() {
        assert!(is_api_path("/api"));
        assert!(is_api_path("/api/repos"));
        assert!(!is_api_path("/repos"));
        assert!(!is_api_path("/"));
        assert!(!is_api_path("/assets/app.js"));
    }

    #[tokio::test]
    async fn serves_known_asset_with_content_type() {
        let resp = serve_from::<TestAssets>("/assets/app.js");
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("javascript"));
        assert!(body_string(resp).await.contains("fixture-app"));
    }

    #[tokio::test]
    async fn falls_back_to_index_for_spa_routes() {
        let resp = serve_from::<TestAssets>("/repos");
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("fixture-spa"));
    }

    #[tokio::test]
    async fn unknown_api_route_returns_json_404() {
        let resp = serve_from::<TestAssets>("/api/does-not-exist");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(body_string(resp).await.contains("unknown api route"));
    }
}
