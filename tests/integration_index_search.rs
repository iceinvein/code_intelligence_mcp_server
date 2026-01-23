use code_intelligence_mcp_server::{
    config::{Config, EmbeddingsBackend, EmbeddingsDevice},
    embeddings::hash::HashEmbedder,
    indexer::pipeline::IndexPipeline,
    retrieval::Retriever,
    storage::{sqlite::SqliteStore, tantivy::TantivyIndex, vector::LanceDbStore},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

fn tmp_dir() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("code-intel-it-{nanos}-{c}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_config(base_dir: &std::path::Path) -> Config {
    let base_dir = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    Config {
        db_path: base_dir.join("code-intelligence.db"),
        vector_db_path: base_dir.join("vectors"),
        tantivy_index_path: base_dir.join("tantivy-index"),
        base_dir: base_dir.clone(),
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
        index_patterns: vec![
            "**/*.ts".to_string(),
            "**/*.tsx".to_string(),
            "**/*.rs".to_string(),
        ],
        exclude_patterns: vec![],
        watch_mode: false,
        watch_debounce_ms: 100,
        max_context_bytes: 200_000,
        index_node_modules: false,
        repo_roots: vec![base_dir],
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
        // PageRank config (FNDN-07)
        pagerank_damping: 0.85,
        pagerank_iterations: 20,
        // Query expansion config (FNDN-02)
        synonym_expansion_enabled: true,
        acronym_expansion_enabled: true,
    }
}

#[tokio::test]
async fn indexes_and_searches_with_hash_embedder() {
    let dir = tmp_dir();

    std::fs::write(
        dir.join("a.ts"),
        r#"
export function alpha() { return beta() }
export function beta() { return 123 }
"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("lib.rs"),
        r#"
pub struct Foo {
  a: i32,
}

pub fn foo() -> Foo { Foo { a: 1 } }
"#,
    )
    .unwrap();

    let config = Arc::new(test_config(&dir));

    let sqlite = SqliteStore::open(&config.db_path).unwrap();
    sqlite.init().unwrap();

    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());

    let embedder = Arc::new(Mutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let vector_dim = config.hash_embedding_dim;

    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", vector_dim)
            .await
            .unwrap(),
    );

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );

    let stats = indexer.index_all().await.unwrap();
    assert!(stats.files_indexed >= 2);
    assert!(stats.symbols_indexed >= 3);

    let resp = retriever.search("alpha", 1, true).await.unwrap();
    assert!(resp.context.contains("export function alpha"));
    assert!(resp.context.contains("export function beta"));

    let resp2 = retriever.search("Foo", 3, false).await.unwrap();
    assert!(resp2.context.contains("pub struct Foo"));

    let sqlite = SqliteStore::open(&config.db_path).unwrap();
    sqlite.init().unwrap();
    assert!(sqlite.latest_index_run().unwrap().is_some());
    assert!(sqlite.latest_search_run().unwrap().is_some());
    let beta = sqlite
        .search_symbols_by_exact_name("beta", None, 10)
        .unwrap();
    let beta = beta.first().unwrap();
    let examples = sqlite.list_usage_examples_for_symbol(&beta.id, 20).unwrap();
    assert!(examples.iter().any(|e| e.snippet.contains("beta")));

    let key = sqlite.get_similarity_cluster_key(&beta.id).unwrap();
    assert!(key.is_some());
}

#[tokio::test]
async fn incremental_index_skips_unchanged_and_removes_deleted_files() {
    let dir = tmp_dir();

    std::fs::write(
        dir.join("a.ts"),
        r#"
export function alpha() { return beta() }
export function beta() { return 123 }
"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("lib.rs"),
        r#"
pub fn gamma() -> i32 { 7 }
"#,
    )
    .unwrap();

    let config = Arc::new(test_config(&dir));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(Mutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );

    let stats1 = indexer.index_all().await.unwrap();
    assert_eq!(stats1.files_scanned, 2);
    assert_eq!(stats1.files_deleted, 0);
    assert_eq!(stats1.files_unchanged, 0);
    assert_eq!(stats1.files_indexed, 2);

    let stats2 = indexer.index_all().await.unwrap();
    assert_eq!(stats2.files_scanned, 2);
    assert_eq!(stats2.files_deleted, 0);
    assert_eq!(stats2.files_indexed, 0);
    assert_eq!(stats2.files_unchanged, 2);

    std::fs::remove_file(dir.join("a.ts")).unwrap();

    let stats3 = indexer.index_all().await.unwrap();
    assert_eq!(stats3.files_scanned, 1);
    assert_eq!(stats3.files_deleted, 1);
    assert_eq!(stats3.files_indexed, 0);
    assert_eq!(stats3.files_unchanged, 1);

    let resp = retriever.search("alpha", 5, false).await.unwrap();
    assert!(!resp.hits.iter().any(|h| h.name == "alpha"));
    assert!(!resp.context.contains("export function alpha"));
}

#[tokio::test]
async fn watch_mode_reindexes_changed_files() {
    let dir = tmp_dir();

    std::fs::write(
        dir.join("a.ts"),
        r#"
export function alpha() { return 1 }
"#,
    )
    .unwrap();

    let mut config = test_config(&dir);
    config.watch_debounce_ms = 50;
    let config = Arc::new(config);

    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(Mutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );
    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );

    let handle = indexer.spawn_watch_loop();

    sleep(Duration::from_millis(150)).await;

    std::fs::write(
        dir.join("a.ts"),
        r#"
export function alpha() { return 1 }
export function beta() { return alpha() }
"#,
    )
    .unwrap();

    let sqlite = SqliteStore::open(&config.db_path).unwrap();
    sqlite.init().unwrap();

    let mut found = false;
    for _ in 0..40 {
        let roots = sqlite
            .search_symbols_by_exact_name("beta", None, 10)
            .unwrap();
        if !roots.is_empty() {
            found = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    handle.abort();
    assert!(found);
}

#[tokio::test]
async fn multi_root_indexes_additional_repo_roots() {
    let dir = tmp_dir();
    let other = tmp_dir();

    std::fs::write(
        other.join("extra.ts"),
        r#"
export function extraRoot() { return 42 }
"#,
    )
    .unwrap();

    let mut config = test_config(&dir);
    config.repo_roots.push(other.clone());
    let config = Arc::new(config);

    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(Mutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );
    let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder);

    indexer.index_all().await.unwrap();
    let resp = retriever.search("extraRoot", 5, false).await.unwrap();
    assert!(resp.context.contains("export function extraRoot"));
}

#[tokio::test]
async fn search_uses_graph_intent_for_callers() {
    let dir = tmp_dir();

    std::fs::write(
        dir.join("logic.ts"),
        r#"
export function targetFunc() { return 1; }
"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("app.ts"),
        r#"
import { targetFunc } from "./logic";
export function callerOne() { targetFunc(); }
"#,
    )
    .unwrap();

    let config = Arc::new(test_config(&dir));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(Mutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );
    let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder);

    indexer.index_all().await.unwrap();

    // Natural language query with intent
    let resp = retriever
        .search("who calls targetFunc", 5, false)
        .await
        .unwrap();

    // Should find "callerOne" because it calls targetFunc
    assert!(resp.context.contains("callerOne"));
    assert!(resp.context.contains("targetFunc();"));

    // The hits should include callerOne
    assert!(resp.hits.iter().any(|h| h.name == "callerOne"));
}
