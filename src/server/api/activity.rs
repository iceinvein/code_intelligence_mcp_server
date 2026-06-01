//! Live-activity endpoints: background jobs, bound MCP sessions, and the SSE
//! log stream.

use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        Json,
    },
};
use futures::stream::Stream;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};
use tokio::sync::broadcast::error::RecvError;

use super::ApiState;
use crate::server::jobs;

pub(crate) async fn handle_jobs(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let items = jobs::snapshot(&state.job_registry);
    let running = items
        .iter()
        .filter(|j| matches!(j.status, jobs::JobStatus::Running))
        .count();
    Json(json!({
        "count": items.len(),
        "running": running,
        "jobs": items,
    }))
}

pub(crate) async fn handle_logs_stream(
    State(state): State<Arc<ApiState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.log_broadcaster.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(line) => Some((Ok(Event::default().data(line)), rx)),
            Err(RecvError::Lagged(n)) => Some((
                Ok(Event::default()
                    .event("lagged")
                    .data(format!("{n} log messages dropped"))),
                rx,
            )),
            Err(RecvError::Closed) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(crate) async fn handle_sessions(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let now = Instant::now();
    let sessions: Vec<Value> = state
        .session_repos
        .iter()
        .map(|entry| {
            let info = entry.value();
            let initialized_at_unix_s = info
                .initialized_at
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let last_seen_secs_ago = now.saturating_duration_since(info.last_seen).as_secs();
            json!({
                "session_id": entry.key(),
                "repo": info.repo.as_ref().map(|p| p.as_str()),
                "bound": info.repo.is_some(),
                "initialized_at_unix_s": initialized_at_unix_s,
                "last_seen_secs_ago": last_seen_secs_ago,
                "bind_skipped_reason": info.bind_skipped_reason.clone(),
            })
        })
        .collect();
    let bound = sessions
        .iter()
        .filter(|v| v.get("bound").and_then(|b| b.as_bool()).unwrap_or(false))
        .count();
    Json(json!({
        "count": sessions.len(),
        "bound_count": bound,
        "connected_count": sessions.len(),
        "sessions": sessions,
    }))
}
