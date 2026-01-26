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
        let conn = self.read()?;
        queries::symbols::upsert_symbol(&conn, symbol)
    }

    pub fn delete_symbols_by_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::symbols::delete_symbols_by_file(&conn, file_path)
    }

    pub fn count_symbols(&self) -> Result<u64> {
        let conn = self.read()?;
        queries::symbols::count_symbols(&conn)
    }

    pub fn most_recent_symbol_update(&self) -> Result<Option<i64>> {
        let conn = self.read()?;
        queries::symbols::most_recent_symbol_update(&conn)
    }

    pub fn search_symbols_by_exact_name(
        &self,
        name: &str,
        file_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::search_symbols_by_exact_name(&conn, name, file_path, limit)
    }

    pub fn search_symbols_by_text_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::search_symbols_by_text_substr(&conn, needle, limit)
    }

    pub fn get_symbol_by_id(&self, id: &str) -> Result<Option<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::get_symbol_by_id(&conn, id)
    }

    pub fn list_symbol_headers_by_file(
        &self,
        file_path: &str,
        exported_only: bool,
    ) -> Result<Vec<SymbolHeaderRow>> {
        let conn = self.read()?;
        queries::symbols::list_symbol_headers_by_file(&conn, file_path, exported_only)
    }

    pub fn list_symbol_id_name_pairs(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        queries::symbols::list_symbol_id_name_pairs(&conn)
    }

    pub fn list_symbols_by_file(&self, file_path: &str) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::list_symbols_by_file(&conn, file_path)
    }

    pub fn search_symbols_by_name_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::search_symbols_by_name_prefix(&conn, prefix, limit)
    }

    pub fn search_symbols_by_name_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::symbols::search_symbols_by_name_substr(&conn, needle, limit)
    }

    pub fn upsert_edge(&self, edge: &EdgeRow) -> Result<()> {
        let conn = self.write()?;
        queries::edges::upsert_edge(&conn, edge)
    }

    pub fn upsert_edge_evidence(&self, evidence: &EdgeEvidenceRow) -> Result<()> {
        let conn = self.write()?;
        queries::edges::upsert_edge_evidence(&conn, evidence)
    }

    pub fn list_edge_evidence(
        &self,
        from_symbol_id: &str,
        to_symbol_id: &str,
        edge_type: &str,
        limit: usize,
    ) -> Result<Vec<EdgeEvidenceRow>> {
        let conn = self.read()?;
        queries::edges::list_edge_evidence(
            &conn,
            from_symbol_id,
            to_symbol_id,
            edge_type,
            limit,
        )
    }

    pub fn list_edges_from(&self, from_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        let conn = self.read()?;
        queries::edges::list_edges_from(&conn, from_symbol_id, limit)
    }

    pub fn list_edges_to(&self, to_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        let conn = self.read()?;
        queries::edges::list_edges_to(&conn, to_symbol_id, limit)
    }

    pub fn count_incoming_edges(&self, to_symbol_id: &str) -> Result<u64> {
        let conn = self.read()?;
        queries::edges::count_incoming_edges(&conn, to_symbol_id)
    }

    pub fn count_edges(&self) -> Result<u64> {
        let conn = self.read()?;
        queries::edges::count_edges(&conn)
    }

    pub fn list_all_edges(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        queries::edges::list_all_edges(&conn)
    }

    pub fn list_all_symbol_ids(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        queries::edges::list_all_symbol_ids(&conn)
    }

    pub fn get_file_fingerprint(&self, file_path: &str) -> Result<Option<FileFingerprintRow>> {
        let conn = self.read()?;
        queries::files::get_file_fingerprint(&conn, file_path)
    }

    pub fn upsert_file_fingerprint(
        &self,
        file_path: &str,
        mtime_ns: i64,
        size_bytes: u64,
    ) -> Result<()> {
        let conn = self.write()?;
        queries::files::upsert_file_fingerprint(&conn, file_path, mtime_ns, size_bytes)
    }

    pub fn delete_file_fingerprint(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::files::delete_file_fingerprint(&conn, file_path)
    }

    pub fn list_all_file_fingerprints(&self, limit: usize) -> Result<Vec<FileFingerprintRow>> {
        let conn = self.read()?;
        queries::files::list_all_file_fingerprints(&conn, limit)
    }

    pub fn insert_index_run(&self, run: &IndexRunRow) -> Result<()> {
        let conn = self.write()?;
        queries::stats::insert_index_run(&conn, run)
    }

    pub fn insert_search_run(&self, run: &SearchRunRow) -> Result<()> {
        let conn = self.write()?;
        queries::stats::insert_search_run(&conn, run)
    }

    pub fn latest_index_run(&self) -> Result<Option<IndexRunRow>> {
        let conn = self.read()?;
        queries::stats::latest_index_run(&conn)
    }

    pub fn latest_search_run(&self) -> Result<Option<SearchRunRow>> {
        let conn = self.read()?;
        queries::stats::latest_search_run(&conn)
    }

    pub fn upsert_similarity_cluster(&self, row: &SimilarityClusterRow) -> Result<()> {
        let conn = self.write()?;
        queries::misc::upsert_similarity_cluster(&conn, row)
    }

    pub fn get_similarity_cluster_key(&self, symbol_id: &str) -> Result<Option<String>> {
        let conn = self.read()?;
        queries::misc::get_similarity_cluster_key(&conn, symbol_id)
    }

    pub fn list_symbols_in_cluster(
        &self,
        cluster_key: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        queries::misc::list_symbols_in_cluster(&conn, cluster_key, limit)
    }

    pub fn list_symbols_without_similarity_clusters(
        &self,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let conn = self.read()?;
        queries::misc::list_symbols_without_similarity_clusters(&conn, limit)
    }

    pub fn delete_usage_examples_by_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::misc::delete_usage_examples_by_file(&conn, file_path)
    }

    pub fn upsert_usage_example(&self, example: &UsageExampleRow) -> Result<()> {
        let conn = self.write()?;
        queries::misc::upsert_usage_example(&conn, example)
    }

    pub fn list_usage_examples_for_symbol(
        &self,
        to_symbol_id: &str,
        limit: usize,
    ) -> Result<Vec<UsageExampleRow>> {
        let conn = self.read()?;
        queries::misc::list_usage_examples_for_symbol(&conn, to_symbol_id, limit)
    }

    pub fn upsert_symbol_metrics(&self, metrics: &SymbolMetricsRow) -> Result<()> {
        let conn = self.write()?;
        queries::metrics::upsert_symbol_metrics(&conn, metrics)
    }

    pub fn batch_get_symbol_metrics(
        &self,
        symbol_ids: &[String],
    ) -> Result<std::collections::HashMap<String, f64>> {
        let conn = self.read()?;
        queries::metrics::batch_get_symbol_metrics(&conn, symbol_ids)
    }

    pub fn get_symbol_metrics(&self, symbol_id: &str) -> Result<Option<SymbolMetricsRow>> {
        let conn = self.read()?;
        queries::metrics::get_symbol_metrics(&conn, symbol_id)
    }

    pub fn insert_query_selection(
        &self,
        query_text: &str,
        query_normalized: &str,
        selected_symbol_id: &str,
        position: u32,
    ) -> Result<i64> {
        let conn = self.write()?;
        queries::selections::insert_query_selection(
            &conn,
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
        let conn = self.read()?;
        queries::selections::batch_get_selection_boosts(&conn, pairs)
    }

    pub fn search_todos(
        &self,
        keyword: Option<&str>,
        file_path: Option<&str>,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<schema::TodoRow>> {
        let conn = self.read()?;
        queries::todos::search_todos(&conn, keyword, file_path, kind, limit)
    }

    pub fn batch_upsert_todos(&self, todos: &[TodoEntry]) -> Result<()> {
        let conn = self.write()?;
        queries::todos::batch_upsert_todos(&conn, todos)
    }

    pub fn delete_todos_by_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::todos::delete_todos_by_file(&conn, file_path)
    }

    pub fn batch_upsert_docstrings(
        &self,
        entries: &[crate::indexer::extract::symbol::JSDocEntry],
    ) -> Result<()> {
        let conn = self.write()?;
        queries::docstrings::batch_upsert_docstrings(&conn, entries)
    }

    pub fn has_docstring(&self, symbol_id: &str) -> Result<bool> {
        let conn = self.read()?;
        queries::docstrings::has_docstring(&conn, symbol_id)
    }

    pub fn get_docstring_by_symbol(&self, symbol_id: &str) -> Result<Option<schema::DocstringRow>> {
        let conn = self.read()?;
        queries::docstrings::get_docstring_by_symbol(&conn, symbol_id)
    }

    pub fn delete_docstrings_by_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::docstrings::delete_docstrings_by_file(&conn, file_path)
    }

    pub fn batch_upsert_decorators(
        &self,
        decorators: &[crate::storage::sqlite::schema::DecoratorRow],
    ) -> Result<()> {
        let conn = self.write()?;
        queries::decorators::batch_upsert_decorators(&conn, decorators)
    }

    pub fn delete_decorators_by_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::decorators::delete_decorators_by_file(&conn, file_path)
    }

    pub fn search_decorators_by_name(
        &self,
        name: Option<&str>,
        decorator_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DecoratorRow>> {
        let conn = self.read()?;
        queries::decorators::search_decorators_by_name_filtered(
            &conn,
            name,
            decorator_type,
            limit,
        )
    }

    pub fn is_test_file(&self, path: &str) -> bool {
        queries::tests::is_test_file(path)
    }

    pub fn create_test_links_for_file(&self, test_file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::tests::create_test_links_for_file(&conn, test_file_path)
    }

    pub fn delete_test_links_for_file(&self, file_path: &str) -> Result<()> {
        let conn = self.write()?;
        queries::tests::delete_test_links_for_file(&conn, file_path)
    }

    pub fn get_tests_for_source(&self, source_path: &str) -> Result<Vec<String>> {
        let conn = self.read()?;
        queries::tests::get_tests_for_source(&conn, source_path)
    }

    pub fn get_symbols_with_tests(&self, file_path: &str) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        queries::tests::get_symbols_with_tests(&conn, file_path)
    }

    pub fn get_cached_embedding(&self, cache_key: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.read()?;
        queries::cache::get_cached_embedding(&conn, cache_key)
    }

    pub fn put_cached_embedding(
        &self,
        cache_key: &str,
        model_name: &str,
        text_hash: &str,
        embedding: &[u8],
        vector_dim: usize,
    ) -> Result<()> {
        let conn = self.write()?;
        queries::cache::put_cached_embedding(
            &conn,
            cache_key,
            model_name,
            text_hash,
            embedding,
            vector_dim,
        )
    }

    pub fn cleanup_cache(&self, max_size_bytes: i64) -> Result<i64> {
        let conn = self.write()?;
        queries::cache::cleanup_cache(&conn, max_size_bytes)
    }

    // Repository and package operations (09-03)
    pub fn upsert_repository(&self, repo: &RepositoryRow) -> Result<()> {
        let conn = self.write()?;
        queries::packages::upsert_repository(&conn, repo)
    }

    pub fn upsert_package(&self, pkg: &PackageRow) -> Result<()> {
        let conn = self.write()?;
        queries::packages::upsert_package(&conn, pkg)
    }

    pub fn get_package_for_file(&self, file_path: &str) -> Result<Option<PackageRow>> {
        let conn = self.read()?;
        queries::packages::get_package_for_file(&conn, file_path)
    }

    pub fn list_all_packages(&self) -> Result<Vec<PackageRow>> {
        let conn = self.read()?;
        queries::packages::list_all_packages(&conn)
    }

    pub fn list_all_repositories(&self) -> Result<Vec<RepositoryRow>> {
        let conn = self.read()?;
        queries::packages::list_all_repositories(&conn)
    }

    pub fn get_repository_by_id(&self, id: &str) -> Result<Option<RepositoryRow>> {
        let conn = self.read()?;
        queries::packages::get_repository_by_id(&conn, id)
    }

    pub fn get_package_by_id(&self, id: &str) -> Result<Option<PackageRow>> {
        let conn = self.read()?;
        queries::packages::get_package_by_id(&conn, id)
    }

    pub fn count_packages_in_repository(&self, repository_id: &str) -> Result<u64> {
        let conn = self.read()?;
        queries::packages::count_packages_in_repository(&conn, repository_id)
    }

    /// Batch lookup package IDs for multiple symbols.
    ///
    /// Returns a HashMap mapping symbol_id to package_id.
    pub fn batch_get_symbol_packages(
        &self,
        symbol_ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        let conn = self.read()?;
        queries::packages::batch_get_symbol_packages(&conn, symbol_ids)
    }

    /// Get the package ID for a given file path.
    ///
    /// Returns Ok(Some(package_id)) if a package contains the file, Ok(None) otherwise.
    pub fn get_package_id_for_file(&self, file_path: &str) -> Result<Option<String>> {
        let conn = self.read()?;
        queries::packages::get_package_id_for_file(&conn, file_path)
    }
}
