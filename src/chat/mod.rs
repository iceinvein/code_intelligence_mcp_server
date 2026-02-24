//! Codebase chatbot — local LLM with tool-calling RAG
//!
//! Activated by `--chat` flag in standalone mode. Spawns an axum HTTP server
//! on a separate port (default 3334) serving a chat web UI and streaming API.

pub mod agent;
pub mod llm;
pub mod tools;

use agent::{ChatEvent, ChatMessage};
use axum::{
    extract::{Json, State},
    response::{
        sse::{Event, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;

use crate::session::SessionManager;

/// Shared state for the chat server.
pub struct ChatState {
    pub session_manager: Arc<SessionManager>,
    pub llm: Arc<llm::ChatLlm>,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub repo_path: String,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub model_loaded: bool,
    pub model_name: String,
}

#[derive(Debug, Serialize)]
pub struct RepoListEntry {
    pub name: String,
    pub path: String,
    pub last_accessed: String,
}

/// Spawn the chat HTTP server on the given port.
pub async fn spawn(
    session_manager: Arc<SessionManager>,
    llm: Arc<llm::ChatLlm>,
    port: u16,
) -> anyhow::Result<()> {
    let state = Arc::new(ChatState {
        session_manager,
        llm,
    });

    let app = Router::new()
        .route("/", get(index_page))
        .route("/api/chat", post(chat_handler))
        .route("/api/status", get(status_handler))
        .route("/api/repos", get(repos_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("Chat UI available at http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });

    Ok(())
}

async fn index_page() -> Html<&'static str> {
    Html(include_str!("ui.html"))
}

async fn status_handler(
    State(_state): State<Arc<ChatState>>,
) -> impl IntoResponse {
    Json(StatusResponse {
        model_loaded: true,
        model_name: "Qwen2.5-Coder-14B-Instruct".to_string(),
    })
}

async fn repos_handler(
    State(state): State<Arc<ChatState>>,
) -> impl IntoResponse {
    match state.session_manager.registry.list_all() {
        Ok(entries) => {
            let list: Vec<RepoListEntry> = entries
                .into_iter()
                .map(|e| RepoListEntry {
                    name: e.name,
                    path: e.path,
                    last_accessed: e.last_accessed,
                })
                .collect();
            Json(list).into_response()
        }
        Err(e) => {
            tracing::warn!("Failed to list repos: {}", e);
            Json(Vec::<RepoListEntry>::new()).into_response()
        }
    }
}

async fn chat_handler(
    State(state): State<Arc<ChatState>>,
    Json(req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (event_tx, mut event_rx) = mpsc::channel::<ChatEvent>(64);

    let llm = state.llm.clone();
    let session_manager = state.session_manager.clone();
    let repo_path = req.repo_path.clone();
    let messages = req.messages;

    // Spawn agent loop in background
    tokio::spawn(async move {
        // Resolve repo path to AppState
        let repo_utf8 = crate::path::Utf8PathBuf::from(repo_path.clone());
        let app_state = match session_manager.get_or_create_repo(&repo_utf8).await {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send(ChatEvent::Error {
                    message: format!("Failed to resolve repo: {}", e),
                }).await;
                let _ = event_tx.send(ChatEvent::Done).await;
                return;
            }
        };

        // Extract repo name from path
        let repo_name = repo_path
            .rsplit('/')
            .next()
            .unwrap_or(&repo_path);

        // Run agent loop
        agent::run_agent(
            &llm,
            &app_state,
            messages,
            repo_name,
            &repo_path,
            event_tx,
        ).await;
    });

    // Convert mpsc receiver to SSE stream
    let stream = async_stream::stream! {
        while let Some(event) = event_rx.recv().await {
            let event_type = match &event {
                ChatEvent::Token { .. } => "token",
                ChatEvent::ToolCallStart { .. } => "tool_call",
                ChatEvent::ToolResult { .. } => "tool_result",
                ChatEvent::Error { .. } => "error",
                ChatEvent::Done => "done",
            };
            let data = serde_json::to_string(&event).unwrap_or_default();
            yield Ok::<_, Infallible>(Event::default().event(event_type).data(data));
        }
    };

    Sse::new(stream)
}
