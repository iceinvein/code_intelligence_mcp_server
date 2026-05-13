//! Background worker that generates LLM descriptions for indexed symbols.
//!
//! Runs asynchronously after initial indexing completes. Queries SQLite for
//! symbols without descriptions, generates them via the LLM backend, caches
//! results in SQLite, and re-upserts symbols to Tantivy with descriptions
//! appended to the text field.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::llm::{self, LlmGenerator};
use crate::storage::sqlite::queries::descriptions as desc_queries;
use crate::storage::sqlite::{SqliteStore, SymbolRow};
use crate::storage::tantivy::TantivyIndex;

/// Run the background description generation worker.
///
/// Processes all symbols that don't have descriptions yet,
/// committing to Tantivy in batches for progressive search improvement.
/// Fetches symbols in pages of 500 to limit peak memory — each page's
/// symbol text strings are freed before the next page is fetched.
/// The LLM model (~1.1 GB GPU) is explicitly dropped after all
/// descriptions are generated.
///
/// If `one_shot` is true, exits immediately when no pending symbols remain
/// (used in tests). Otherwise loops with an idle timeout checking for new work.
pub async fn run_description_worker(
    db: Arc<SqliteStore>,
    tantivy: Arc<TantivyIndex>,
    llm: Arc<dyn LlmGenerator>,
    max_tokens: u32,
    batch_commit_size: usize,
    cancel: CancellationToken,
    one_shot: bool,
) -> Result<()> {
    /// How long to sleep between idle checks for new undescribed symbols.
    /// Handles the startup race (worker starts before indexing completes)
    /// and watch-mode (new files indexed after initial pass).
    const IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

    /// Maximum consecutive idle cycles before the worker exits and frees
    /// the LLM model (~1.1 GB GPU memory). At 30s intervals, 20 cycles = 10 min.
    const MAX_IDLE_CYCLES: u32 = 20;

    let mut idle_cycles: u32 = 0;

    loop {
        if cancel.is_cancelled() {
            tracing::info!("Description worker cancelled");
            break;
        }

        // Clean up descriptions for deleted symbols
        {
            let conn = db.read().context("read conn for orphan cleanup")?;
            let orphaned = desc_queries::cleanup_orphaned_descriptions(&conn).unwrap_or(0);
            if orphaned > 0 {
                tracing::info!("Cleaned up {} orphaned descriptions", orphaned);
            }
        }

        let total = {
            let conn = db.read().context("read conn for undescribed count")?;
            desc_queries::count_undescribed_symbols(&conn)
                .context("Failed to count undescribed symbols")?
        };

        if total == 0 {
            if one_shot {
                tracing::debug!("Description worker: one-shot mode, no pending symbols, exiting");
                break;
            }
            idle_cycles += 1;
            if idle_cycles >= MAX_IDLE_CYCLES {
                tracing::info!(
                    "Description worker: idle for {}s, exiting",
                    IDLE_POLL_INTERVAL.as_secs() * MAX_IDLE_CYCLES as u64
                );
                break;
            }
            if idle_cycles == 1 {
                tracing::debug!(
                    "Description worker: no undescribed symbols, waiting for indexing..."
                );
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => continue,
            }
        }

        // Found work — reset idle counter
        idle_cycles = 0;
        tracing::info!("Description worker: {} symbols to describe", total);

        let mut generated_count = 0;
        let mut processed_count = 0;

        loop {
            if cancel.is_cancelled() {
                tracing::info!(
                    "Description worker cancelled at {}/{}",
                    processed_count,
                    total
                );
                break;
            }

            // Fetch next page. Described symbols drop out of the LEFT JOIN
            // result set, so LIMIT without OFFSET always returns the next batch.
            let page = {
                let conn = db.read().context("read conn for undescribed batch")?;
                desc_queries::get_undescribed_symbols_batch(&conn, PAGE_SIZE)?
            };

            if page.is_empty() {
                break;
            }

            for sym in &page {
                if cancel.is_cancelled() {
                    break;
                }

                if !crate::indexer::pipeline::should_generate_embedding(
                    &sym.kind,
                    &sym.name,
                    &sym.file_path,
                    sym.exported,
                    sym.start_line,
                    sym.end_line,
                ) {
                    let content_hash = llm::compute_content_hash(&sym.name, &sym.kind, &sym.text);
                    {
                        let conn = db.read().context("read conn for skip-description upsert")?;
                        let _ = desc_queries::upsert_description(&conn, &sym.id, &content_hash, "");
                    }
                    continue;
                }

                processed_count += 1;

                let content_hash = llm::compute_content_hash(&sym.name, &sym.kind, &sym.text);

                // Check if already described with matching content (race condition guard)
                {
                    let conn = db.read().context("read conn for description check")?;
                    if desc_queries::get_description(&conn, &sym.id, &content_hash)?.is_some() {
                        continue;
                    }
                }

                // Generate description
                let prompt =
                    llm::build_description_prompt(&sym.name, &sym.kind, &sym.file_path, &sym.text);
                let description = match llm.generate(&prompt, max_tokens) {
                    Ok(desc) => desc,
                    Err(e) => {
                        tracing::warn!("Failed to generate description for {}: {:#}", sym.name, e);
                        // Mark as described (empty) to prevent infinite retry on
                        // permanently-failing symbols (e.g. text too large for context window).
                        let conn = db
                            .read()
                            .context("read conn for failed-description upsert")?;
                        let _ = desc_queries::upsert_description(&conn, &sym.id, &content_hash, "");
                        continue;
                    }
                };

                // Cache in SQLite
                {
                    let conn = db.read().context("read conn for description upsert")?;
                    desc_queries::upsert_description(&conn, &sym.id, &content_hash, &description)?;
                }

                // Re-upsert to Tantivy with description
                {
                    let conn = db.read().context("read conn for symbol lookup")?;
                    if let Some(symbol_row) = get_symbol_row(&conn, &sym.id)? {
                        if let Err(e) =
                            tantivy.upsert_symbol(&symbol_row, "", "", Some(&description))
                        {
                            tracing::warn!("Failed to re-upsert {} to Tantivy: {}", sym.name, e);
                        }
                    }
                }

                generated_count += 1;

                // Batch commit for progressive search improvement
                if generated_count % batch_commit_size == 0 {
                    if let Err(e) = tantivy.commit() {
                        tracing::warn!("Failed to commit Tantivy batch: {}", e);
                    }
                    tracing::info!(
                        "Descriptions: {}/{} processed ({} generated)",
                        processed_count,
                        total,
                        generated_count
                    );
                }
            }
            // page Vec dropped here — symbol text strings freed before next fetch
        }

        // Final commit for this pass
        if generated_count > 0 {
            tantivy
                .commit()
                .context("Final Tantivy commit after descriptions")?;
        }
        tracing::info!(
            "Description worker: pass complete, {} symbols described out of {} total",
            generated_count,
            total
        );
    }

    // Explicitly drop the LLM to free ~1.1 GB of GPU memory.
    drop(llm);
    Ok(())
}

const PAGE_SIZE: usize = 500;

/// Helper to get a SymbolRow from SQLite by ID.
fn get_symbol_row(conn: &rusqlite::Connection, symbol_id: &str) -> Result<Option<SymbolRow>> {
    use rusqlite::OptionalExtension;
    let mut stmt = conn.prepare_cached(
        "SELECT id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text
         FROM symbols WHERE id = ?1"
    )?;
    let result = stmt
        .query_row(rusqlite::params![symbol_id], |row| {
            Ok(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get(5)?,
                start_byte: row.get(6)?,
                end_byte: row.get(7)?,
                start_line: row.get(8)?,
                end_line: row.get(9)?,
                text: row.get(10)?,
            })
        })
        .optional()
        .context("Failed to get symbol row")?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmGenerator;
    use crate::path::Utf8PathBuf;
    use crate::storage::sqlite::schema::SCHEMA_SQL;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn setup_test_env() -> (Arc<SqliteStore>, Arc<TantivyIndex>) {
        // Create in-memory SQLite with schema
        let db = Arc::new(SqliteStore::open_in_memory().unwrap());
        {
            let conn = db.read().unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
        }

        // Create Tantivy in temp dir
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tantivy_dir = Utf8PathBuf::from(
            std::env::temp_dir()
                .join(format!("describe-test-{}", nanos))
                .to_string_lossy()
                .to_string(),
        );
        let tantivy = Arc::new(TantivyIndex::open_or_create(&tantivy_dir).unwrap());

        (db, tantivy)
    }

    fn insert_test_symbol(db: &SqliteStore, id: &str, name: &str, kind: &str) {
        let conn = db.read().unwrap();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES (?1, 'src/test.rs', 'rust', ?2, ?3, 1, 0, 100, 1, 10, 'fn test() {}')",
            rusqlite::params![id, kind, name],
        ).unwrap();
    }

    #[tokio::test]
    async fn worker_generates_descriptions_for_undescribed_symbols() {
        let (db, tantivy) = setup_test_env();
        insert_test_symbol(&db, "s1", "my_func", "function");
        insert_test_symbol(&db, "s2", "MyStruct", "struct");

        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let cancel = CancellationToken::new();

        run_description_worker(db.clone(), tantivy, llm, 30, 10, cancel, true)
            .await
            .unwrap();

        // Both symbols should now have descriptions
        let conn = db.read().unwrap();
        let count = desc_queries::count_descriptions(&conn).unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn worker_skips_already_described_symbols() {
        let (db, tantivy) = setup_test_env();
        insert_test_symbol(&db, "s1", "my_func", "function");

        // Pre-insert a description with matching content hash
        let hash = llm::compute_content_hash("my_func", "function", "fn test() {}");
        {
            let conn = db.read().unwrap();
            desc_queries::upsert_description(&conn, "s1", &hash, "Already described").unwrap();
        }

        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let cancel = CancellationToken::new();

        run_description_worker(db.clone(), tantivy, llm, 30, 10, cancel, true)
            .await
            .unwrap();

        // Should still have exactly 1 description (the pre-existing one)
        let conn = db.read().unwrap();
        let desc = desc_queries::get_description_for_symbol(&conn, "s1").unwrap();
        assert_eq!(desc, Some("Already described".to_string()));
    }

    #[tokio::test]
    async fn worker_handles_empty_symbols() {
        let (db, tantivy) = setup_test_env();
        // No symbols inserted

        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let cancel = CancellationToken::new();

        run_description_worker(db, tantivy, llm, 30, 10, cancel, true)
            .await
            .unwrap();
        // Should complete without error
    }

    #[tokio::test]
    async fn worker_respects_cancellation() {
        let (db, tantivy) = setup_test_env();
        for i in 0..10 {
            insert_test_symbol(&db, &format!("s{}", i), &format!("func_{}", i), "function");
        }

        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        run_description_worker(db.clone(), tantivy, llm, 30, 10, cancel, true)
            .await
            .unwrap();

        // No descriptions should have been generated
        let conn = db.read().unwrap();
        let count = desc_queries::count_descriptions(&conn).unwrap();
        assert_eq!(count, 0);
    }

    /// LLM generator that always fails (simulates InsufficientSpace or other errors).
    struct FailingLlmGenerator;
    impl LlmGenerator for FailingLlmGenerator {
        fn generate(&self, _prompt: &str, _max_tokens: u32) -> Result<String> {
            anyhow::bail!("Failed to add token to batch: InsufficientSpace(512)")
        }
    }

    #[tokio::test]
    async fn worker_marks_failed_symbols_as_described() {
        let (db, tantivy) = setup_test_env();
        insert_test_symbol(&db, "s1", "my_func", "function");
        insert_test_symbol(&db, "s2", "other_func", "function");

        let llm: Arc<dyn LlmGenerator> = Arc::new(FailingLlmGenerator);
        let cancel = CancellationToken::new();

        run_description_worker(db.clone(), tantivy, llm, 30, 10, cancel, true)
            .await
            .unwrap();

        // Both symbols should have empty descriptions to prevent infinite retry
        let conn = db.read().unwrap();
        let count = desc_queries::count_descriptions(&conn).unwrap();
        assert_eq!(count, 2);
        // Descriptions should be empty (not generated)
        let desc = desc_queries::get_description_for_symbol(&conn, "s1").unwrap();
        assert_eq!(desc, Some("".to_string()));
    }
}
