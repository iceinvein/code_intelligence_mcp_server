use anyhow::Result;
use rusqlite::{Connection, params};
use std::time::UNIX_EPOCH;

pub struct EmbeddingCacheEntry {
    pub cache_key: String,
    pub model_name: String,
    pub text_hash: String,
    pub embedding: Vec<u8>,
    pub vector_dim: usize,
    pub created_at: i64,
    pub last_accessed_at: i64,
    pub access_count: i64,
}

/// Get cached embedding by cache key
pub fn get_cached_embedding(conn: &Connection, cache_key: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare_cached(
        "SELECT embedding FROM embedding_cache WHERE cache_key = ?1"
    )?;

    let result = stmt.query_row(params![cache_key], |row| {
        row.get(0)
    });

    match result {
        Ok(blob) => {
            // Update last_accessed_at and increment access_count
            let now = UNIX_EPOCH.elapsed().unwrap().as_secs() as i64;
            let _ = conn.execute(
                "UPDATE embedding_cache SET last_accessed_at = ?1, access_count = access_count + 1 WHERE cache_key = ?2",
                params![now, cache_key]
            );
            Ok(Some(blob))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Store embedding in cache
pub fn put_cached_embedding(
    conn: &Connection,
    cache_key: &str,
    model_name: &str,
    text_hash: &str,
    embedding: &[u8],
    vector_dim: usize,
) -> Result<()> {
    let now = UNIX_EPOCH.elapsed().unwrap().as_secs() as i64;
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO embedding_cache
         (cache_key, model_name, text_hash, embedding, vector_dim, created_at, last_accessed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
    )?;

    stmt.execute(params![cache_key, model_name, text_hash, embedding, vector_dim, now, now])?;
    Ok(())
}

/// Get cache statistics
pub struct CacheStats {
    pub total_entries: i64,
    pub total_size_bytes: i64,
    pub hit_rate: f64,
}

pub fn get_cache_stats(conn: &Connection) -> Result<CacheStats> {
    let total_entries: i64 = conn.query_row(
        "SELECT COUNT(*) FROM embedding_cache",
        [],
        |row| row.get(0)
    )?;

    let total_size_bytes: i64 = conn.query_row(
        "SELECT SUM(LENGTH(embedding)) FROM embedding_cache",
        [],
        |row| row.get(0)
    ).unwrap_or(0);

    // Note: hit_rate requires external tracking
    Ok(CacheStats {
        total_entries,
        total_size_bytes,
        hit_rate: 0.0,
    })
}

/// Lazy LRU cleanup: remove entries beyond size limit
pub fn cleanup_cache(conn: &Connection, max_size_bytes: i64) -> Result<i64> {
    let current_size: i64 = conn.query_row(
        "SELECT SUM(LENGTH(embedding)) FROM embedding_cache",
        [],
        |row| row.get(0)
    ).unwrap_or(0);

    if current_size <= max_size_bytes {
        return Ok(0);
    }

    // Delete oldest entries by last_accessed_at
    let deleted = conn.execute(
        "DELETE FROM embedding_cache
         WHERE cache_key IN (
             SELECT cache_key FROM embedding_cache
             ORDER BY last_accessed_at ASC
             LIMIT (SELECT COUNT(*) / 10 FROM embedding_cache)
         )",
        []
    )?;

    Ok(deleted as i64)
}
