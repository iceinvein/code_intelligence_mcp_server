use axum::{
    extract::State,
    routing::get,
    Router,
    response::{Response, IntoResponse},
    http::{StatusCode, header},
};
use std::sync::Arc;
use anyhow::Result;

#[derive(Clone)]
pub struct MetricsState {
    pub registry: Arc<super::MetricsRegistry>,
}

/// Spawn metrics server on localhost only
pub async fn spawn_metrics_server(
    registry: Arc<super::MetricsRegistry>,
    port: u16,
) -> Result<tokio::task::JoinHandle<()>> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(MetricsState { registry: Arc::clone(&registry) });

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind metrics server on port {}: {}", port, e))?;

    tracing::info!("Metrics server listening on http://127.0.0.1:{}", port);

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("Metrics server error: {}", e);
        }
    });

    Ok(handle)
}

async fn metrics_handler(State(state): State<MetricsState>) -> Response {
    match render_metrics_internal(&state.registry.registry) {
        Ok(output) => {
            tracing::debug!("Served metrics, {} bytes", output.len());
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")
                .body(output.into_response().into_body())
                .unwrap()
        }
        Err(e) => {
            tracing::error!("Failed to render metrics: {}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(format!("Failed to gather metrics: {}", e).into())
                .unwrap()
        }
    }
}

fn render_metrics_internal(registry: &prometheus::Registry) -> Result<String, anyhow::Error> {
    let encoder = prometheus::TextEncoder::new();
    let metric_families = registry.gather();
    let output = encoder.encode_to_string(&metric_families)?;
    Ok(output)
}
