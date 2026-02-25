//! Codebase chatbot — local LLM with tool-calling RAG
//!
//! Activated by `--chat` flag in standalone mode. Spawns an axum HTTP server
//! on a separate port (default 3334) serving a chat web UI and streaming API.

pub mod agent;
pub mod llm;
pub mod store;
pub mod tools;

use agent::{ChatEvent, ChatMessage};
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
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

use crate::config::ChatConfig;
use crate::session::SessionManager;
use store::ChatStore;

/// Shared state for the chat server.
pub struct ChatState {
    pub session_manager: Arc<SessionManager>,
    pub llm: Arc<llm::ChatLlm>,
    pub chat_store: Arc<ChatStore>,
    pub chat_config: ChatConfig,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub repo_path: String,
    /// If provided, messages are persisted to this session.
    pub session_id: Option<String>,
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

// --- Session API types ---

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub repo_path: String,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct SessionListEntry {
    pub id: String,
    pub repo_path: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct SessionDetail {
    pub id: String,
    pub repo_path: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<MessageEntry>,
}

#[derive(Debug, Serialize)]
pub struct MessageEntry {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    pub repo_path: Option<String>,
    pub limit: Option<usize>,
}

/// Spawn the chat HTTP server on the given port.
pub async fn spawn(
    session_manager: Arc<SessionManager>,
    llm: Arc<llm::ChatLlm>,
    chat_store: Arc<ChatStore>,
    chat_config: ChatConfig,
    port: u16,
) -> anyhow::Result<()> {
    let state = Arc::new(ChatState {
        session_manager,
        llm,
        chat_store,
        chat_config,
    });

    let app = Router::new()
        .route("/", get(index_page))
        .route("/api/chat", post(chat_handler))
        .route("/api/status", get(status_handler))
        .route("/api/repos", get(repos_handler))
        .route(
            "/api/sessions",
            get(list_sessions_handler).post(create_session_handler),
        )
        .route(
            "/api/sessions/{id}",
            get(get_session_handler).delete(delete_session_handler),
        )
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

// --- Session route handlers ---

async fn create_session_handler(
    State(state): State<Arc<ChatState>>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    match state
        .chat_store
        .create_session(&id, &req.repo_path, req.title.as_deref())
    {
        Ok(()) => (StatusCode::CREATED, Json(CreateSessionResponse { id })).into_response(),
        Err(e) => {
            tracing::warn!("Failed to create chat session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn list_sessions_handler(
    State(state): State<Arc<ChatState>>,
    axum::extract::Query(query): axum::extract::Query<ListSessionsQuery>,
) -> impl IntoResponse {
    let repo_path = query.repo_path.as_deref().unwrap_or("");
    let limit = query.limit.unwrap_or(50);

    match state.chat_store.list_sessions(repo_path, limit) {
        Ok(sessions) => {
            let list: Vec<SessionListEntry> = sessions
                .into_iter()
                .map(|s| SessionListEntry {
                    id: s.id,
                    repo_path: s.repo_path,
                    title: s.title,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                })
                .collect();
            Json(list).into_response()
        }
        Err(e) => {
            tracing::warn!("Failed to list chat sessions: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_session_handler(
    State(state): State<Arc<ChatState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let session = match state.chat_store.get_session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!("Failed to get chat session: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let messages = match state.chat_store.list_messages(&id) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Failed to list messages for session {}: {}", id, e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    Json(SessionDetail {
        id: session.id,
        repo_path: session.repo_path,
        title: session.title,
        created_at: session.created_at,
        updated_at: session.updated_at,
        messages: messages
            .into_iter()
            .map(|m| MessageEntry {
                id: m.id,
                role: m.role,
                content: m.content,
                tool_calls_json: m.tool_calls_json,
                created_at: m.created_at,
            })
            .collect(),
    })
    .into_response()
}

async fn delete_session_handler(
    State(state): State<Arc<ChatState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.chat_store.delete_session(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::warn!("Failed to delete chat session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- Chat handler with persistence ---

async fn chat_handler(
    State(state): State<Arc<ChatState>>,
    Json(req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (event_tx, mut event_rx) = mpsc::channel::<ChatEvent>(64);

    let llm = state.llm.clone();
    let session_manager = state.session_manager.clone();
    let chat_store = state.chat_store.clone();
    let max_tool_rounds = state.chat_config.max_tool_rounds;
    let repo_path = req.repo_path.clone();
    let messages = req.messages;
    let session_id = req.session_id.clone();

    // Persist the user message if session_id is provided
    if let Some(ref sid) = session_id {
        // Auto-create session if it doesn't exist
        if let Ok(None) = chat_store.get_session(sid) {
            let title = messages
                .last()
                .filter(|m| m.role == "user")
                .map(|m| {
                    let t = m.content.trim();
                    if t.len() > 80 {
                        format!("{}...", &t[..80])
                    } else {
                        t.to_string()
                    }
                });
            if let Err(e) =
                chat_store.create_session(sid, &repo_path, title.as_deref())
            {
                tracing::warn!("Failed to auto-create chat session: {}", e);
            }
        }

        // Persist the latest user message
        if let Some(last_user_msg) = messages.iter().rev().find(|m| m.role == "user") {
            if let Err(e) =
                chat_store.add_message(sid, "user", &last_user_msg.content, None)
            {
                tracing::warn!("Failed to persist user message: {}", e);
            }
            // Touch the session to update its timestamp
            if let Err(e) = chat_store.touch_session(sid) {
                tracing::warn!("Failed to touch session: {}", e);
            }
        }
    }

    // Spawn agent loop in background
    tokio::spawn(async move {
        // Resolve repo path to AppState
        let repo_utf8 = crate::path::Utf8PathBuf::from(repo_path.clone());
        let app_state = match session_manager.get_or_create_repo(&repo_utf8).await {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx
                    .send(ChatEvent::Error {
                        message: format!("Failed to resolve repo: {}", e),
                    })
                    .await;
                let _ = event_tx.send(ChatEvent::Done).await;
                return;
            }
        };

        // Extract repo name from path
        let repo_name = repo_path.rsplit('/').next().unwrap_or(&repo_path);

        // Interpose a forwarder between the agent and client SSE stream
        // so we can capture the full assistant response for persistence.
        let (agent_tx, mut agent_rx) = mpsc::channel::<ChatEvent>(64);

        let session_id_for_persist = session_id.clone();
        let chat_store_for_persist = chat_store.clone();
        tokio::spawn(async move {
            let mut assistant_content = String::new();
            while let Some(event) = agent_rx.recv().await {
                match &event {
                    ChatEvent::Token { content } => {
                        assistant_content.push_str(content);
                    }
                    ChatEvent::Done => {
                        // Persist the assistant response if session_id is set
                        if let Some(ref sid) = session_id_for_persist {
                            if !assistant_content.is_empty() {
                                if let Err(e) = chat_store_for_persist.add_message(
                                    sid,
                                    "assistant",
                                    &assistant_content,
                                    None,
                                ) {
                                    tracing::warn!(
                                        "Failed to persist assistant message: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
                // Forward to the real client
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        // Run agent loop — `llm` is already `Arc<ChatLlm>`, moved into the agent.
        agent::run_agent(
            llm,
            &app_state,
            messages,
            repo_name,
            &repo_path,
            agent_tx,
            max_tool_rounds,
        )
        .await;
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
