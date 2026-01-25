pub mod operations;
pub mod queries;
pub mod schema;

use anyhow::Result;

pub use operations::SqliteStore;
pub use schema::*;

// Re-export types used in the API
pub use crate::indexer::extract::symbol::TodoEntry;

impl SqliteStore {
    pub fn upsert_symbol(&self, symbol: &SymbolRow) -> Result<()> {
        queries::symbols::upsert_symbol(&self.conn, symbol)
    }

    pub fn delete_symbols_by_file(&self, file_path: &str) -> Result<()> {
        queries::symbols::delete_symbols_by_file(&self.conn, file_path)
    }

    pub fn count_symbols(&self) -> Result<u64> {
        queries::symbols::count_symbols(&self.conn)
    }

    pub fn most_recent_symbol_update(&self) -> Result<Option<i64>> {
        queries::symbols::most_recent_symbol_update(&self.conn)
    }

    pub fn search_symbols_by_exact_name(
        &self,
        name: &str,
        file_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        queries::symbols::search_symbols_by_exact_name(&self.conn, name, file_path, limit)
    }

    pub fn search_symbols_by_text_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        queries::symbols::search_symbols_by_text_substr(&self.conn, needle, limit)
    }

    pub fn get_symbol_by_id(&self, id: &str) -> Result<Option<SymbolRow>> {
        queries::symbols::get_symbol_by_id(&self.conn, id)
    }

    pub fn list_symbol_headers_by_file(
        &self,
        file_path: &str,
        exported_only: bool,
    ) -> Result<Vec<SymbolHeaderRow>> {
        queries::symbols::list_symbol_headers_by_file(&self.conn, file_path, exported_only)
    }

    pub fn list_symbol_id_name_pairs(&self) -> Result<Vec<(String, String)>> {
        queries::symbols::list_symbol_id_name_pairs(&self.conn)
    }

    pub fn list_symbols_by_file(&self, file_path: &str) -> Result<Vec<SymbolRow>> {
        queries::symbols::list_symbols_by_file(&self.conn, file_path)
    }

    pub fn search_symbols_by_name_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        queries::symbols::search_symbols_by_name_prefix(&self.conn, prefix, limit)
    }

    pub fn search_symbols_by_name_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        queries::symbols::search_symbols_by_name_substr(&self.conn, needle, limit)
    }

    pub fn upsert_edge(&self, edge: &EdgeRow) -> Result<()> {
        queries::edges::upsert_edge(&self.conn, edge)
    }

    pub fn upsert_edge_evidence(&self, evidence: &EdgeEvidenceRow) -> Result<()> {
        queries::edges::upsert_edge_evidence(&self.conn, evidence)
    }

    pub fn list_edge_evidence(
        &self,
        from_symbol_id: &str,
        to_symbol_id: &str,
        edge_type: &str,
        limit: usize,
    ) -> Result<Vec<EdgeEvidenceRow>> {
        queries::edges::list_edge_evidence(
            &self.conn,
            from_symbol_id,
            to_symbol_id,
            edge_type,
            limit,
        )
    }

    pub fn list_edges_from(&self, from_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        queries::edges::list_edges_from(&self.conn, from_symbol_id, limit)
    }

    pub fn list_edges_to(&self, to_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        queries::edges::list_edges_to(&self.conn, to_symbol_id, limit)
    }

    pub fn count_incoming_edges(&self, to_symbol_id: &str) -> Result<u64> {
        queries::edges::count_incoming_edges(&self.conn, to_symbol_id)
    }

    pub fn count_edges(&self) -> Result<u64> {
        queries::edges::count_edges(&self.conn)
    }

    pub fn list_all_edges(&self) -> Result<Vec<(String, String)>> {
        queries::edges::list_all_edges(&self.conn)
    }

    pub fn list_all_symbol_ids(&self) -> Result<Vec<(String, String)>> {
        queries::edges::list_all_symbol_ids(&self.conn)
    }

    pub fn get_file_fingerprint(&self, file_path: &str) -> Result<Option<FileFingerprintRow>> {
        queries::files::get_file_fingerprint(&self.conn, file_path)
    }

    pub fn upsert_file_fingerprint(
        &self,
        file_path: &str,
        mtime_ns: i64,
        size_bytes: u64,
    ) -> Result<()> {
        queries::files::upsert_file_fingerprint(&self.conn, file_path, mtime_ns, size_bytes)
    }

    pub fn delete_file_fingerprint(&self, file_path: &str) -> Result<()> {
        queries::files::delete_file_fingerprint(&self.conn, file_path)
    }

    pub fn list_all_file_fingerprints(&self, limit: usize) -> Result<Vec<FileFingerprintRow>> {
        queries::files::list_all_file_fingerprints(&self.conn, limit)
    }

    pub fn insert_index_run(&self, run: &IndexRunRow) -> Result<()> {
        queries::stats::insert_index_run(&self.conn, run)
    }

    pub fn insert_search_run(&self, run: &SearchRunRow) -> Result<()> {
        queries::stats::insert_search_run(&self.conn, run)
    }

    pub fn latest_index_run(&self) -> Result<Option<IndexRunRow>> {
        queries::stats::latest_index_run(&self.conn)
    }

    pub fn latest_search_run(&self) -> Result<Option<SearchRunRow>> {
        queries::stats::latest_search_run(&self.conn)
    }

    pub fn upsert_similarity_cluster(&self, row: &SimilarityClusterRow) -> Result<()> {
        queries::misc::upsert_similarity_cluster(&self.conn, row)
    }

    pub fn get_similarity_cluster_key(&self, symbol_id: &str) -> Result<Option<String>> {
        queries::misc::get_similarity_cluster_key(&self.conn, symbol_id)
    }

    pub fn list_symbols_in_cluster(
        &self,
        cluster_key: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        queries::misc::list_symbols_in_cluster(&self.conn, cluster_key, limit)
    }

    pub fn delete_usage_examples_by_file(&self, file_path: &str) -> Result<()> {
        queries::misc::delete_usage_examples_by_file(&self.conn, file_path)
    }

    pub fn upsert_usage_example(&self, example: &UsageExampleRow) -> Result<()> {
        queries::misc::upsert_usage_example(&self.conn, example)
    }

    pub fn list_usage_examples_for_symbol(
        &self,
        to_symbol_id: &str,
        limit: usize,
    ) -> Result<Vec<UsageExampleRow>> {
        queries::misc::list_usage_examples_for_symbol(&self.conn, to_symbol_id, limit)
    }

    pub fn upsert_symbol_metrics(&self, metrics: &SymbolMetricsRow) -> Result<()> {
        queries::metrics::upsert_symbol_metrics(&self.conn, metrics)
    }

    pub fn batch_get_symbol_metrics(
        &self,
        symbol_ids: &[String],
    ) -> Result<std::collections::HashMap<String, f64>> {
        queries::metrics::batch_get_symbol_metrics(&self.conn, symbol_ids)
    }

    pub fn get_symbol_metrics(&self, symbol_id: &str) -> Result<Option<SymbolMetricsRow>> {
        queries::metrics::get_symbol_metrics(&self.conn, symbol_id)
    }

    pub fn insert_query_selection(
        &self,
        query_text: &str,
        query_normalized: &str,
        selected_symbol_id: &str,
        position: u32,
    ) -> Result<i64> {
        queries::selections::insert_query_selection(
            &self.conn,
            query_text,
            query_normalized,
            selected_symbol_id,
            position,
        )
    }

    pub fn batch_get_selection_boosts(
        &self,
        pairs: &[(String, String)],
    ) -> Result<std::collections::HashMap<String, f32>> {
        queries::selections::batch_get_selection_boosts(&self.conn, pairs)
    }

    pub fn search_todos(
        &self,
        keyword: Option<&str>,
        file_path: Option<&str>,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<schema::TodoRow>> {
        queries::todos::search_todos(&self.conn, keyword, file_path, kind, limit)
    }

    pub fn batch_upsert_todos(&self, todos: &[TodoEntry]) -> Result<()> {
        queries::todos::batch_upsert_todos(&self.conn, todos)
    }

    pub fn delete_todos_by_file(&self, file_path: &str) -> Result<()> {
        queries::todos::delete_todos_by_file(&self.conn, file_path)
    }

    pub fn batch_upsert_docstrings(&self, entries: &[crate::indexer::extract::symbol::JSDocEntry]) -> Result<()> {
        queries::docstrings::batch_upsert_docstrings(&self.conn, entries)
    }

    pub fn has_docstring(&self, symbol_id: &str) -> Result<bool> {
        queries::docstrings::has_docstring(&self.conn, symbol_id)
    }

    pub fn get_docstring_by_symbol(&self, symbol_id: &str) -> Result<Option<schema::DocstringRow>> {
        queries::docstrings::get_docstring_by_symbol(&self.conn, symbol_id)
    }

    pub fn delete_docstrings_by_file(&self, file_path: &str) -> Result<()> {
        queries::docstrings::delete_docstrings_by_file(&self.conn, file_path)
    }

    pub fn batch_upsert_decorators(&self, decorators: &[crate::storage::sqlite::schema::DecoratorRow]) -> Result<()> {
        queries::decorators::batch_upsert_decorators(&self.conn, decorators)
    }

    pub fn delete_decorators_by_file(&self, file_path: &str) -> Result<()> {
        queries::decorators::delete_decorators_by_file(&self.conn, file_path)
    }

    pub fn search_decorators_by_name(
        &self,
        name: Option<&str>,
        decorator_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DecoratorRow>> {
        queries::decorators::search_decorators_by_name_filtered(
            &self.conn,
            name,
            decorator_type,
            limit,
        )
    }

    pub fn is_test_file(&self, path: &str) -> bool {
        queries::tests::is_test_file(path)
    }

    pub fn create_test_links_for_file(&self, test_file_path: &str) -> Result<()> {
        queries::tests::create_test_links_for_file(&self.conn, test_file_path)
    }

    pub fn delete_test_links_for_file(&self, file_path: &str) -> Result<()> {
        queries::tests::delete_test_links_for_file(&self.conn, file_path)
    }

    pub fn get_tests_for_source(&self, source_path: &str) -> Result<Vec<String>> {
        queries::tests::get_tests_for_source(&self.conn, source_path)
    }

    pub fn get_symbols_with_tests(
        &self,
        file_path: &str,
    ) -> Result<Vec<(String, String)>> {
        queries::tests::get_symbols_with_tests(&self.conn, file_path)
    }

    pub fn get_cached_embedding(&self, cache_key: &str) -> Result<Option<Vec<u8>>> {
        queries::cache::get_cached_embedding(&self.conn, cache_key)
    }

    pub fn put_cached_embedding(
        &self,
        cache_key: &str,
        model_name: &str,
        text_hash: &str,
        embedding: &[u8],
        vector_dim: usize,
    ) -> Result<()> {
        queries::cache::put_cached_embedding(
            &self.conn,
            cache_key,
            model_name,
            text_hash,
            embedding,
            vector_dim,
        )
    }

    pub fn cleanup_cache(&self, max_size_bytes: i64) -> Result<i64> {
        queries::cache::cleanup_cache(&self.conn, max_size_bytes)
    }
}
