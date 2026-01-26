//! Web UI for code intelligence (optional feature)

use crate::handlers::{tool_internal_error, AppState};
use crate::path::Utf8PathBuf;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::{net::SocketAddr, path::Path, sync::Arc};

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    pub limit: Option<usize>,
    pub exported_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct NameParam {
    pub name: String,
}

pub async fn spawn(state: Arc<AppState>) -> anyhow::Result<()> {
    let addr: SocketAddr = std::env::var("WEB_UI_ADDR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8787)));

    let app = Router::new()
        .route("/", get(index))
        .route("/api/search", get(api_search))
        .route("/api/definition", get(api_definition))
        .route("/api/edges", get(api_edges))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Code Intelligence</title>
    <style>
      body { font-family: ui-sans-serif, system-ui, -apple-system, sans-serif; margin: 24px; }
      input, button { font-size: 14px; padding: 8px; }
      pre { white-space: pre-wrap; background: #f6f8fa; padding: 12px; border-radius: 6px; }
      .row { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; }
      .col { margin-top: 12px; }
    </style>
  </head>
  <body>
    <h1>Code Intelligence</h1>
    <div class="row">
      <input id="q" size="60" placeholder="search query or symbol name" />
      <button onclick="runSearch()">Search</button>
      <button onclick="runDef()">Definition</button>
      <button onclick="runEdges()">Edges</button>
    </div>
    <div class="col"><pre id="out"></pre></div>
    <script>
      async function getJson(url) {
        const res = await fetch(url);
        const txt = await res.text();
        try { return JSON.stringify(JSON.parse(txt), null, 2); }
        catch { return txt; }
      }
      async function runSearch() {
        const q = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/search?query=${q}`);
      }
      async function runDef() {
        const name = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/definition?name=${name}`);
      }
      async function runEdges() {
        const name = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/edges?name=${name}`);
      }
    </script>
  </body>
</html>
"#,
    )
}

async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(5).max(1);
    let exported_only = params.exported_only.unwrap_or(false);
    match state
        .retriever
        .search(&params.query, limit, exported_only)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => Json(json!({ "error": tool_internal_error(err).to_string() })).into_response(),
    }
}

async fn api_definition(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NameParam>,
) -> impl IntoResponse {
    use crate::storage::sqlite::SqliteStore;

    let sqlite = match SqliteStore::open(&state.config.db_path) {
        Ok(s) => s,
        Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
    };
    if let Err(err) = sqlite.init() {
        return Json(json!({ "error": err.to_string() })).into_response();
    }
    let rows = match sqlite.search_symbols_by_exact_name(&params.name, None, 10) {
        Ok(r) => r,
        Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
    };
    let context = state
        .retriever
        .assemble_definitions(&rows)
        .unwrap_or_default();
    Json(json!({ "symbol_name": params.name, "count": rows.len(), "definitions": rows, "context": context }))
        .into_response()
}

async fn api_edges(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NameParam>,
) -> impl IntoResponse {
    use crate::storage::sqlite::SqliteStore;

    let sqlite = match SqliteStore::open(&state.config.db_path) {
        Ok(s) => s,
        Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
    };
    if let Err(err) = sqlite.init() {
        return Json(json!({ "error": err.to_string() })).into_response();
    }
    let roots = match sqlite.search_symbols_by_exact_name(&params.name, None, 10) {
        Ok(r) => r,
        Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
    };
    let root = match roots.first() {
        Some(r) => r,
        None => return Json(json!({ "symbol_name": params.name, "edges": [] })).into_response(),
    };
    let edges = sqlite.list_edges_from(&root.id, 500).unwrap_or_default();
    Json(json!({ "symbol_name": root.name, "edges": edges })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use code_intelligence_mcp_server::{
        config::{Config, EmbeddingsBackend, EmbeddingsDevice},
        embeddings::hash::HashEmbedder,
        indexer::pipeline::IndexPipeline,
        retrieval::Retriever,
        storage::{tantivy::TantivyIndex, vector::LanceDbStore},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };
    use tokio::sync::Mutex;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-webui-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cfg(base: &Path) -> Config {
        let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
        let base_utf8 = Utf8PathBuf::from_path_buf(base.clone()).unwrap_or_else(|_| {
            Utf8PathBuf::from(base.to_string_lossy().as_ref())
        });
        Config {
            base_dir: base_utf8.clone(),
            db_path: base_utf8.join("code-intelligence.db"),
            vector_db_path: base_utf8.join("vectors"),
            tantivy_index_path: base_utf8.join("tantivy-index"),
            embeddings_backend: EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 32,
            vector_search_limit: 20,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.1,
            rank_index_file_boost: 0.05,
            rank_test_penalty: 0.1,
            rank_popularity_weight: 0.05,
            rank_popularity_cap: 50,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 50,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![base_utf8],
            // Reranker config (FNDN-03)
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            // Learning config (FNDN-04)
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            // Token config (FNDN-05)
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            // Performance config (FNDN-06)
            parallel_workers: 4,
            embedding_cache_enabled: true,
            embedding_max_threads: 0,
            // PageRank config (FNDN-07)
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            // Query expansion config (FNDN-02)
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            // RRF config (RETR-05)
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            // HyDE config (RETR-06, RETR-07)
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            // Metrics config (PERF-04)
            metrics_enabled: true,
            metrics_port: 9090,
            // Package detection config (09-04)
            package_detection_enabled: true,
        }
    }

    async fn build_state(base: &Path) -> Arc<AppState> {
        let mut config = cfg(base);

        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("a.ts"),
            r#"
export function alpha() { return 1 }
export function beta() { return alpha() }
"#,
        )
        .unwrap();

        config.repo_roots = vec![config.base_dir.clone()];
        let config = Arc::new(config);

        let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
        let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
        let vectors = Arc::new(
            lancedb
                .open_or_create_table("symbols", config.hash_embedding_dim)
                .await
                .unwrap(),
        );
        let embedder = Arc::new(Mutex::new(
            Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
        ));

        // No reranker for web UI tests (keep it simple)
        let reranker: Option<Arc<dyn code_intelligence_mcp_server::reranker::Reranker>> = None;

        let indexer = IndexPipeline::new(
            config.clone(),
            tantivy.clone(),
            vectors.clone(),
            embedder.clone(),
        );
        let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder, reranker);

        indexer.index_all().await.unwrap();

        Arc::new(AppState {
            config,
            indexer,
            retriever,
        })
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn index_page_serves_html() {
        let Html(body) = index().await;
        assert!(body.contains("<title>Code Intelligence</title>"));
    }

    #[tokio::test]
    async fn api_search_returns_json_and_context() {
        let base = tmp_dir();
        let state = build_state(&base).await;

        let resp = api_search(
            State(state),
            Query(SearchParams {
                query: "beta".to_string(),
                limit: Some(5),
                exported_only: Some(false),
            }),
        )
        .await
        .into_response();

        assert!(resp.status().is_success());
        let v = body_json(resp).await;
        assert_eq!(v.get("query").and_then(|x| x.as_str()), Some("beta"));
        assert!(v
            .get("context")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .contains("export function beta"));
    }

    #[tokio::test]
    async fn api_definition_returns_rows_and_context() {
        let base = tmp_dir();
        let state = build_state(&base).await;

        let resp = api_definition(
            State(state),
            Query(NameParam {
                name: "beta".to_string(),
            }),
        )
        .await
        .into_response();

        assert!(resp.status().is_success());
        let v = body_json(resp).await;
        assert!(v.get("count").and_then(|x| x.as_u64()).unwrap_or(0) >= 1);
        assert!(v
            .get("context")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .contains("export function beta"));
    }

    #[tokio::test]
    async fn api_edges_returns_edges_and_handles_missing_symbol() {
        let base = tmp_dir();
        let state = build_state(&base).await;

        let resp = api_edges(
            State(state.clone()),
            Query(NameParam {
                name: "beta".to_string(),
            }),
        )
        .await
        .into_response();
        assert!(resp.status().is_success());
        let v = body_json(resp).await;
        let edges = v
            .get("edges")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(!edges.is_empty());
        assert!(edges
            .iter()
            .any(|e| e.get("edge_type").and_then(|x| x.as_str()) == Some("call")));

        let resp2 = api_edges(
            State(state),
            Query(NameParam {
                name: "does_not_exist".to_string(),
            }),
        )
        .await
        .into_response();
        assert!(resp2.status().is_success());
        let v2 = body_json(resp2).await;
        let edges2 = v2
            .get("edges")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(edges2.is_empty());
    }
}
