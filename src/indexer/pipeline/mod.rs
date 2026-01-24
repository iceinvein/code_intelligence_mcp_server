pub mod edges;
pub mod parsing;
pub mod scan;
pub mod stats;
pub mod usage;
pub mod utils;

use crate::{
    config::Config,
    embeddings::Embedder,
    graph::pagerank,
    indexer::{
        extract::c::extract_c_symbols,
        extract::cpp::extract_cpp_symbols,
        extract::go::extract_go_symbols,
        extract::java::extract_java_symbols,
        extract::javascript::extract_javascript_symbols,
        extract::python::extract_python_symbols,
        extract::rust::extract_rust_symbols,
        extract::typescript::extract_typescript_symbols_with_path,
        parser::{language_id_for_path, LanguageId},
    },
    storage::{
        sqlite::{SimilarityClusterRow, SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::{LanceVectorTable, VectorRecord},
    },
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

use self::edges::{extract_edges_for_symbol, upsert_name_mapping};
use self::parsing::symbol_kind_to_string;
use self::scan::{scan_files, should_index_file};
use self::stats::IndexRunStats;
use self::usage::extract_usage_examples_for_file;
use self::utils::{
    cluster_key_from_vector, file_fingerprint, file_key, language_string, stable_symbol_id,
    unix_now_s,
};

#[derive(Clone)]
pub struct IndexPipeline {
    config: Arc<Config>,
    db_path: PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
}

impl IndexPipeline {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    ) -> Self {
        let db_path = config.db_path.clone();
        Self {
            config,
            db_path,
            tantivy,
            vectors,
            embedder,
        }
    }

    pub async fn index_all(&self) -> Result<IndexRunStats> {
        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();
        let mut files = Vec::new();
        for root in &self.config.repo_roots {
            files.extend(scan_files(&self.config, root)?);
        }
        let stats = self.index_files(files, true).await?;
        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;
        Ok(stats)
    }

    pub async fn index_paths(&self, paths: &[PathBuf]) -> Result<IndexRunStats> {
        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                files.extend(scan_files(&self.config, p)?);
            } else if p.is_file() && should_index_file(&self.config, p) {
                files.push(p.to_path_buf());
            }
        }
        let stats = self.index_files(files, false).await?;
        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;
        Ok(stats)
    }

    pub fn spawn_watch_loop(&self) -> tokio::task::JoinHandle<()> {
        let pipeline = self.clone();
        tokio::spawn(async move {
            let interval_ms = pipeline.config.watch_debounce_ms.max(50);
            loop {
                sleep(Duration::from_millis(interval_ms)).await;
                if let Err(err) = pipeline.index_all().await {
                    tracing::warn!(error = %err, "Watch index run failed");
                }
            }
        })
    }

    fn persist_index_run_metrics(
        &self,
        started_at_unix_s: i64,
        elapsed: Duration,
        stats: &IndexRunStats,
    ) -> Result<()> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;
        let run = crate::storage::sqlite::IndexRunRow {
            started_at_unix_s,
            duration_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
            files_scanned: stats.files_scanned as u64,
            files_indexed: stats.files_indexed as u64,
            files_skipped: stats.files_skipped as u64,
            files_unchanged: stats.files_unchanged as u64,
            files_deleted: stats.files_deleted as u64,
            symbols_indexed: stats.symbols_indexed as u64,
        };
        let _ = sqlite.insert_index_run(&run);
        Ok(())
    }

    async fn index_files(
        &self,
        files: Vec<PathBuf>,
        cleanup_deleted: bool,
    ) -> Result<IndexRunStats> {
        let mut seen = HashSet::new();
        let mut uniq = Vec::new();
        for p in files {
            let abs = p.canonicalize().unwrap_or(p);
            if seen.insert(abs.clone()) {
                uniq.push(abs);
            }
        }

        let mut stats = IndexRunStats {
            files_scanned: uniq.len(),
            ..Default::default()
        };

        if cleanup_deleted {
            let mut scanned_rel: HashSet<String> = HashSet::new();
            for file in &uniq {
                scanned_rel.insert(file_key(&self.config, file));
            }

            let existing = {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.list_all_file_fingerprints(1_000_000)?
            };

            let to_delete = existing
                .into_iter()
                .filter(|fp| !scanned_rel.contains(&fp.file_path))
                .map(|fp| fp.file_path)
                .collect::<Vec<_>>();

            let mut any = false;
            for file_path in to_delete {
                {
                    let sqlite = SqliteStore::open(&self.db_path)?;
                    sqlite.init()?;
                    sqlite.delete_symbols_by_file(&file_path)?;
                    sqlite.delete_usage_examples_by_file(&file_path)?;
                    sqlite.delete_todos_by_file(&file_path)?;
                    sqlite.delete_test_links_for_file(&file_path)?;
                    sqlite.delete_file_fingerprint(&file_path)?;
                }

                self.tantivy.delete_symbols_by_file(&file_path)?;
                self.vectors.delete_records_by_file_path(&file_path).await?;

                stats.files_deleted += 1;
                any = true;
            }

            if any {
                self.tantivy.commit()?;
            }
        }

        let mut name_to_id: HashMap<String, String> = HashMap::new();

        for file in uniq {
            let rel = file_key(&self.config, &file);

            let language_id = match language_id_for_path(&file) {
                Some(id) => id,
                None => {
                    stats.files_skipped += 1;
                    continue;
                }
            };

            let fp = match file_fingerprint(&file) {
                Ok(fp) => fp,
                Err(err) => {
                    tracing::warn!(
                        file = %file.display(),
                        error = %err,
                        "Failed to fingerprint file"
                    );
                    stats.files_skipped += 1;
                    continue;
                }
            };

            let is_unchanged = {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.get_file_fingerprint(&rel)?.is_some_and(|existing| {
                    existing.mtime_ns == fp.mtime_ns && existing.size_bytes == fp.size_bytes
                })
            };

            if is_unchanged {
                stats.files_unchanged += 1;
                continue;
            }

            let source = match fs::read_to_string(&file) {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(file = %file.display(), error = %err, "Failed to read file");
                    stats.files_skipped += 1;
                    continue;
                }
            };

            let extracted = match language_id {
                LanguageId::Typescript | LanguageId::Tsx => {
                    extract_typescript_symbols_with_path(language_id, &source, &rel)
                }
                LanguageId::Rust => extract_rust_symbols(&source),
                LanguageId::Python => extract_python_symbols(&source),
                LanguageId::Go => extract_go_symbols(&source),
                LanguageId::C => extract_c_symbols(&source),
                LanguageId::Cpp => extract_cpp_symbols(&source),
                LanguageId::Java => extract_java_symbols(&source),
                LanguageId::Javascript => extract_javascript_symbols(&source),
            };

            let extracted = match extracted {
                Ok(syms) => syms,
                Err(err) => {
                    tracing::warn!(
                        file = %file.display(),
                        error = %err,
                        "Failed to extract symbols"
                    );
                    stats.files_skipped += 1;
                    continue;
                }
            };
            self.tantivy.delete_symbols_by_file(&rel)?;
            self.vectors
                .delete_records_by_file_path(&rel)
                .await
                .with_context(|| format!("Failed to delete old vectors for {rel}"))?;

            {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite
                    .delete_symbols_by_file(&rel)
                    .with_context(|| format!("Failed to delete old symbols for {rel}"))?;
                sqlite
                    .delete_usage_examples_by_file(&rel)
                    .with_context(|| format!("Failed to delete old usage examples for {rel}"))?;
                sqlite
                    .delete_todos_by_file(&rel)
                    .with_context(|| format!("Failed to delete old todos for {rel}"))?;
                sqlite
                    .delete_test_links_for_file(&rel)
                    .with_context(|| format!("Failed to delete old test links for {rel}"))?;
            }

            let mut symbol_rows = Vec::new();

            // 1. Add File-Level Symbol (Document Indexing)
            // We index the file itself as a symbol to allow retrieval of the "whole file" concept.
            let file_symbol_id = stable_symbol_id(&rel, "FILE_ROOT", 0);
            symbol_rows.push(SymbolRow {
                id: file_symbol_id,
                file_path: rel.clone(),
                language: language_string(language_id).to_string(),
                kind: "file".to_string(),
                name: rel.clone(), // Name is the relative path
                exported: false,
                start_byte: 0,
                end_byte: source.len() as u32,
                start_line: 1,
                end_line: source.lines().count() as u32,
                text: source.clone(),
            });

            for sym in extracted.symbols {
                let text = source
                    .get(sym.bytes.start..sym.bytes.end)
                    .unwrap_or("")
                    .to_string();

                if text.trim().is_empty() {
                    continue;
                }

                let start_byte_for_id = if sym.exported {
                    0
                } else {
                    sym.bytes.start as u32
                };
                let id = stable_symbol_id(&rel, &sym.name, start_byte_for_id);
                symbol_rows.push(SymbolRow {
                    id,
                    file_path: rel.clone(),
                    language: language_string(language_id).to_string(),
                    kind: symbol_kind_to_string(sym.kind),
                    name: sym.name,
                    exported: sym.exported,
                    start_byte: sym.bytes.start as u32,

                    end_byte: sym.bytes.end as u32,
                    start_line: sym.lines.start,
                    end_line: sym.lines.end,
                    text,
                });
            }

            if !symbol_rows.is_empty() {
                let vectors = self
                    .embed_and_build_vector_records(&symbol_rows)
                    .await
                    .with_context(|| format!("Failed to embed symbols for {rel}"))?;

                for row in &symbol_rows {
                    self.tantivy.upsert_symbol(row)?;
                    upsert_name_mapping(&mut name_to_id, row);
                }

                // Commit Tantivy changes immediately to ensure they are persisted
                // even if vector indexing panics (which has been observed with lance).
                self.tantivy.commit()?;

                {
                    let sqlite = SqliteStore::open(&self.db_path)?;
                    sqlite.init()?;
                    for row in &symbol_rows {
                        sqlite.upsert_symbol(row)?;
                    }
                    for row in &symbol_rows {
                        let edges = extract_edges_for_symbol(
                            row,
                            &name_to_id,
                            &extracted.imports,
                            &extracted.type_edges,
                            &extracted.dataflow_edges,
                        );
                        for (edge, evidence) in edges {
                            let _ = sqlite.upsert_edge(&edge);
                            for ev in evidence {
                                let _ = sqlite.upsert_edge_evidence(&ev);
                            }
                        }
                    }

                    let examples = extract_usage_examples_for_file(
                        &rel,
                        &source,
                        &name_to_id,
                        &extracted.imports,
                        &symbol_rows,
                    );

                    for ex in examples {
                        let _ = sqlite.upsert_usage_example(&ex);
                    }

                    for rec in &vectors {
                        let _ = sqlite.upsert_similarity_cluster(&SimilarityClusterRow {
                            symbol_id: rec.id.clone(),
                            cluster_key: cluster_key_from_vector(&rec.vector),
                        });
                    }

                    // Store TODO entries extracted from this file
                    if !extracted.todos.is_empty() {
                        let _ = sqlite.batch_upsert_todos(&extracted.todos);
                    }

                    // Create test links if this is a test file
                    if sqlite.is_test_file(&rel) {
                        let _ = sqlite.create_test_links_for_file(&rel);
                    }

                    sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
                }

                // Add vectors last, as this step is prone to panics in some environments.
                // We wrap it in a result check just in case, though panics escape this.
                if let Err(e) = self.vectors.add_records(&vectors).await {
                    tracing::error!("Failed to add vector records for {}: {}", rel, e);
                }
            } else {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
            }

            stats.symbols_indexed += symbol_rows.len();
            stats.files_indexed += 1;
            self.tantivy.commit()?;
        }

        // Compute PageRank scores after all indexing is complete
        // Only run if the graph structure changed (files indexed or deleted)
        if stats.files_indexed > 0 || stats.files_deleted > 0 {
            let sqlite = SqliteStore::open(&self.db_path)?;
            sqlite.init()?;
            pagerank::compute_and_store_pagerank(&sqlite, &self.config)
                .context("Failed to compute PageRank scores")?;
        } else {
            tracing::debug!("Skipping PageRank computation (no files indexed or deleted)");
        }

        tracing::debug!(?stats, "Index run completed");
        Ok(stats)
    }

    async fn embed_and_build_vector_records(
        &self,
        rows: &[SymbolRow],
    ) -> Result<Vec<VectorRecord>> {
        let texts = rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>();
        let vectors = {
            let mut embedder = self.embedder.lock().await;
            embedder.embed(&texts)?
        };

        let mut out = Vec::with_capacity(rows.len());
        for (row, vector) in rows.iter().zip(vectors) {
            out.push(VectorRecord {
                id: row.id.clone(),
                vector,
                name: row.name.clone(),
                kind: row.kind.clone(),
                file_path: row.file_path.clone(),
                exported: row.exported,
                language: row.language.clone(),
                text: row.text.clone(),
            });
        }

        Ok(out)
    }
}
