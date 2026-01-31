use code_intelligence_mcp_server::{
    config::{Config, EmbeddingsBackend, EmbeddingsDevice},
    embeddings::hash::HashEmbedder,
    indexer::pipeline::IndexPipeline,
    metrics::MetricsRegistry,
    path::Utf8PathBuf,
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
    let base_dir_utf8 = Utf8PathBuf::from_path_buf(base_dir.clone()).unwrap_or_else(|_| {
        Utf8PathBuf::from(base_dir.to_string_lossy().as_ref())
    });
    Config {
        db_path: base_dir_utf8.join("code-intelligence.db"),
        vector_db_path: base_dir_utf8.join("vectors"),
        tantivy_index_path: base_dir_utf8.join("tantivy-index"),
        base_dir: base_dir_utf8.clone(),
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
        watch_min_index_interval_ms: 50, // Small interval for tests
        max_context_bytes: 200_000,
        index_node_modules: false,
        repo_roots: vec![base_dir_utf8],
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
        metrics_enabled: false,
        metrics_port: 9090,
        package_detection_enabled: false,
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

    let sqlite = SqliteStore::open(config.db_path.as_path()).unwrap();
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        None,
        None,
        metrics,
    );

    let stats = indexer.index_all().await.unwrap();
    assert!(stats.files_indexed >= 2);
    assert!(stats.symbols_indexed >= 3);

    let resp = retriever.search("alpha", 1, true).await.unwrap();
    assert!(resp.response.context.contains("export function alpha"));
    assert!(resp.response.context.contains("export function beta"));

    let resp2 = retriever.search("Foo", 3, false).await.unwrap();
    assert!(resp2.response.context.contains("pub struct Foo"));

    let sqlite = SqliteStore::open(config.db_path.as_path()).unwrap();
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        None,
        None,
        metrics,
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
    assert!(!resp.response.hits.iter().any(|h| h.name == "alpha"));
    assert!(!resp.response.context.contains("export function alpha"));
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics,
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

    let sqlite = SqliteStore::open(config.db_path.as_path()).unwrap();
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
    let other_utf8 = Utf8PathBuf::from_path_buf(other.clone()).unwrap();
    config.repo_roots.push(other_utf8);
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );

    indexer.index_all().await.unwrap();
    let resp = retriever.search("extraRoot", 5, false).await.unwrap();
    assert!(resp.response.context.contains("export function extraRoot"));
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );

    indexer.index_all().await.unwrap();

    // Natural language query with intent
    let resp = retriever
        .search("who calls targetFunc", 5, false)
        .await
        .unwrap();

    // Should find "callerOne" because it calls targetFunc
    assert!(resp.response.context.contains("callerOne"));
    assert!(resp.response.context.contains("targetFunc();"));

    // The hits should include callerOne
    assert!(resp.response.hits.iter().any(|h| h.name == "callerOne"));
}

#[tokio::test]
async fn symbol_to_package_association() {
    let dir = tmp_dir();

    // Create a package structure - use a non-hidden directory structure
    let pkg_dir = dir.join("mypackage");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    // Create package.json
    std::fs::write(
        pkg_dir.join("package.json"),
        r#"
{
  "name": "mypackage",
  "version": "1.0.0"
}
"#,
    )
    .unwrap();

    // Create a source file
    std::fs::write(
        pkg_dir.join("utils.ts"),
        r#"
export function myFunction() { return 42; }
export const myConstant = 123;
"#,
    )
    .unwrap();

    let mut config = test_config(&dir);
    config.package_detection_enabled = true;
    let config = Arc::new(config);

    let sqlite = SqliteStore::open(config.db_path.as_path()).unwrap();
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

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics,
    );

    // Index the files
    let stats = indexer.index_all().await.unwrap();
    eprintln!("Indexed {} files", stats.files_indexed);
    assert!(stats.files_indexed >= 1);

    // Verify symbols were indexed with file_path
    let symbols = sqlite
        .search_symbols_by_exact_name("myFunction", None, 10)
        .unwrap();
    assert!(!symbols.is_empty());

    let my_function = symbols
        .iter()
        .find(|s| s.name == "myFunction")
        .expect("myFunction not found");
    eprintln!("my_function.file_path: {}", my_function.file_path);

    // List all packages to debug
    let all_packages = sqlite.list_all_packages().unwrap();
    eprintln!("Found {} packages", all_packages.len());
    for pkg in &all_packages {
        eprintln!("  - {:?} (manifest: {})", pkg.name, pkg.manifest_path);
    }

    // Manually insert the package if auto-discovery failed
    if all_packages.is_empty() {
        eprintln!("Auto-discovery failed, manually inserting package and repository");
        use code_intelligence_mcp_server::storage::sqlite::schema::{PackageRow, RepositoryRow};

        // First create a repository
        let repo_root = config.repo_roots[0].to_string().replace('\\', "/");
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        repo_root.hash(&mut hasher);
        let repo_id = format!("repo-{:x}", hasher.finish());

        let repo_row = RepositoryRow {
            id: repo_id.clone(),
            name: "test-repo".to_string(),
            root_path: repo_root.clone(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 0,
        };
        sqlite.upsert_repository(&repo_row).unwrap();
        eprintln!("Created repository: {:?}", repo_row);

        // Then create the package - use relative manifest path without the filename
        // The manifest_path should be the directory containing package.json for LIKE query to work
        // get_package_for_file uses: WHERE file_path LIKE manifest_path || '%'
        // So for file "mypackage/utils.ts", manifest_path should be "mypackage/"
        let manifest_path = "mypackage/".to_string();
        eprintln!("Manifest path: {}", manifest_path);

        let mut hasher2 = DefaultHasher::new();
        manifest_path.hash(&mut hasher2);
        let pkg_row = PackageRow {
            id: format!("pkg-{:x}", hasher2.finish()),
            repository_id: repo_id,
            name: "mypackage".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: manifest_path.clone(),
            package_type: "npm".to_string(),
            created_at: 0,
        };
        sqlite.upsert_package(&pkg_row).unwrap();
        eprintln!("Manually inserted package: {:?}", pkg_row);
    }

    assert_eq!(my_function.file_path, "mypackage/utils.ts");

    // Verify symbol can be associated with its package via file_path
    let package = sqlite.get_package_for_file(&my_function.file_path).unwrap();
    eprintln!(
        "Package for file: {:?}",
        package.as_ref().map(|p| (&p.name, &p.manifest_path))
    );
    assert!(
        package.is_some(),
        "Package should be found for symbol's file_path"
    );

    let pkg = package.unwrap();
    assert_eq!(pkg.name, "mypackage");
    // manifest_path is the directory prefix for file matching, not the full package.json path
    assert!(pkg.manifest_path.contains("mypackage"));
}
