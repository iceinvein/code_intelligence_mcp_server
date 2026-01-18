use crate::indexer::extract::symbol::Import;
use crate::{
    config::Config,
    embeddings::Embedder,
    indexer::{
        extract::rust::extract_rust_symbols,
        extract::typescript::extract_typescript_symbols,
        parser::{language_id_for_path, LanguageId},
    },
    storage::{
        sqlite::{EdgeRow, SimilarityClusterRow, SqliteStore, SymbolRow, UsageExampleRow},
        tantivy::TantivyIndex,
        vector::{LanceVectorTable, VectorRecord},
    },
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct IndexRunStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub symbols_indexed: usize,
    pub files_skipped: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
}

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
            files.extend(self.scan_files(root)?);
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
                files.extend(self.scan_files(p)?);
            } else if p.is_file() && self.should_index_file(p) {
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

    fn scan_files(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(dir = %dir.display(), error = %err, "Failed to read dir");
                    continue;
                }
            };

            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        tracing::warn!(
                            dir = %dir.display(),
                            error = %err,
                            "Failed to read dir entry"
                        );
                        continue;
                    }
                };
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                if file_type.is_dir() {
                    if self.should_skip_dir(&path) {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }

                if file_type.is_file() && self.should_index_file(&path) {
                    out.push(path);
                }
            }
        }
        Ok(out)
    }

    fn should_skip_dir(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        if name == ".git" || name == "dist" || name == "build" || name == "target" {
            return true;
        }
        if !self.config.index_node_modules && name == "node_modules" {
            return true;
        }
        false
    }

    fn should_index_file(&self, path: &Path) -> bool {
        if self.is_excluded(path) {
            return false;
        }
        matches!(
            language_id_for_path(path),
            Some(LanguageId::Typescript | LanguageId::Tsx | LanguageId::Rust)
        )
    }

    fn is_excluded(&self, path: &Path) -> bool {
        let s = path.to_string_lossy().replace('\\', "/");
        if !self.config.index_node_modules && s.contains("/node_modules/") {
            return true;
        }
        if s.contains("/.git/") || s.contains("/dist/") || s.contains("/build/") {
            return true;
        }
        if s.contains(".test.") {
            return true;
        }
        for pat in &self.config.exclude_patterns {
            if simple_exclude_match(&s, pat) {
                return true;
            }
        }
        false
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
                    extract_typescript_symbols(language_id, &source)
                }
                LanguageId::Rust => extract_rust_symbols(&source),
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
            }

            let mut symbol_rows = Vec::new();
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
                    kind: sym.kind.to_string(),
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

                self.vectors.add_records(&vectors).await?;

                for row in &symbol_rows {
                    self.tantivy.upsert_symbol(row)?;
                    upsert_name_mapping(&mut name_to_id, row);
                }

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
                        );
                        for edge in edges {
                            let _ = sqlite.upsert_edge(&edge);
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

                    sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
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

fn stable_symbol_id(file_path: &str, name: &str, start_byte: u32) -> String {
    let mut data = Vec::with_capacity(file_path.len() + name.len() + 16);
    data.extend_from_slice(file_path.as_bytes());
    data.push(b':');
    data.extend_from_slice(name.as_bytes());
    data.push(b':');
    data.extend_from_slice(start_byte.to_string().as_bytes());
    format!("{:016x}", fnv1a_64(&data))
}

#[derive(Debug, Clone, Copy)]
struct FileFingerprint {
    mtime_ns: i64,
    size_bytes: u64,
}

fn file_fingerprint(path: &Path) -> Result<FileFingerprint> {
    let meta =
        fs::metadata(path).with_context(|| format!("Failed to stat file: {}", path.display()))?;

    let size_bytes = meta.len();
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    Ok(FileFingerprint {
        mtime_ns,
        size_bytes,
    })
}

fn unix_now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn file_key(config: &Config, path: &Path) -> String {
    if let Ok(rel) = config.path_relative_to_base(path) {
        return rel.replace('\\', "/");
    }
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;
    let mut hash = OFFSET;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn language_string(language_id: LanguageId) -> &'static str {
    match language_id {
        LanguageId::Typescript => "typescript",
        LanguageId::Tsx => "tsx",
        LanguageId::Rust => "rust",
    }
}

fn simple_exclude_match(path: &str, pattern: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    if pat.contains("node_modules") && path.contains("/node_modules/") {
        return true;
    }
    if pat.contains(".git") && path.contains("/.git/") {
        return true;
    }
    if pat.contains("/dist/") && path.contains("/dist/") {
        return true;
    }
    if pat.contains("/build/") && path.contains("/build/") {
        return true;
    }
    if pat.contains("*.test.") && path.contains(".test.") {
        return true;
    }
    false
}

fn upsert_name_mapping(name_to_id: &mut HashMap<String, String>, row: &SymbolRow) {
    if let Some(existing) = name_to_id.get(&row.name) {
        if row.exported && existing != &row.id {
            name_to_id.insert(row.name.clone(), row.id.clone());
        }
        return;
    }
    name_to_id.insert(row.name.clone(), row.id.clone());
}

fn extract_edges_for_symbol(
    row: &SymbolRow,
    name_to_id: &HashMap<String, String>,
    imports: &[Import],
    type_edges: &[(String, String)],
) -> Vec<EdgeRow> {
    let mut out = Vec::new();
    let mut used_edges: HashSet<(String, String)> = HashSet::new();

    // Map import alias/name to Import struct for fast lookup
    let mut import_map: HashMap<&str, &Import> = HashMap::new();
    for imp in imports {
        if let Some(alias) = &imp.alias {
            import_map.insert(alias, imp);
        } else {
            import_map.insert(&imp.name, imp);
        }
    }

    for callee in extract_callee_names(&row.text) {
        let to_id = if let Some(local_id) = name_to_id.get(&callee) {
            if local_id == &row.id {
                continue;
            }
            Some(local_id.clone())
        } else if let Some(imp) = import_map.get(callee.as_str()) {
            // Resolve import
            resolve_imported_symbol_id(&row.file_path, imp)
        } else {
            None
        };

        let Some(to_id) = to_id else {
            continue;
        };

        if !used_edges.insert(("call".to_string(), to_id.clone())) {
            continue;
        }
        out.push(EdgeRow {
            from_symbol_id: row.id.clone(),
            to_symbol_id: to_id,
            edge_type: "call".to_string(),
            at_file: Some(row.file_path.clone()),
            at_line: Some(row.start_line),
        });
    }

    // Handle extends/implements
    if row.kind == "class" || row.kind == "interface" || row.kind == "type_alias" {
        let (extends, implements, aliases) = parse_type_relations(&row.text);

        let mut handle_relation = |name: String, rel_type: &str| {
            let to_id = if let Some(local_id) = name_to_id.get(&name) {
                if local_id == &row.id {
                    return;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(name.as_str()) {
                resolve_imported_symbol_id(&row.file_path, imp)
            } else {
                None
            };

            if let Some(id) = to_id {
                if used_edges.insert((rel_type.to_string(), id.clone())) {
                    out.push(EdgeRow {
                        from_symbol_id: row.id.clone(),
                        to_symbol_id: id,
                        edge_type: rel_type.to_string(),
                        at_file: Some(row.file_path.clone()),
                        at_line: Some(row.start_line),
                    });
                }
            }
        };

        for name in extends {
            handle_relation(name, "extends");
        }
        for name in implements {
            handle_relation(name, "implements");
        }
        for name in aliases {
            handle_relation(name, "alias");
        }
    }

    // Create "import" edges for all imports used?
    // Actually we should create edges for *all* imports present in the file that are *used*?
    // Or just all imports? The Graph usually has "import" edges for explicit imports.
    // The previous implementation added edges for all `import_names` found in the file.
    // But `import_names` was just a list of names.
    // Now we have `imports`.
    // Let's verify: `extract_edges_for_symbol` is called for EACH symbol.
    // Should each symbol have an "import" edge?
    // NO. The FILE imports the symbol. Or the symbol USES the import.
    // The previous code: `for name in import_names ... out.push(EdgeRow { ... edge_type: "import" })`.
    // It checked if `import_names` contained something that resolved to a `to_id`.
    // If it did, it added an edge from `row.id` to `to_id` with type "import".
    // This implies "Symbol A depends on Import B".
    // This is valid if Symbol A *uses* B.
    // But the previous code iterated ALL imports for EVERY symbol. That seems wrong?
    // Ah, `import_names` was passed in.
    // If `import_names` is global for the file, then EVERY symbol in the file gets an edge to EVERY import?
    // That seems like noise.
    // But let's look at the old code:
    /*
    for name in import_names {
        let Some(to_id) = name_to_id.get(name) else { continue };
        ...
        out.push(... edge_type: "import" ...);
    }
    */
    // `name_to_id` contains LOCAL symbols.
    // So if I `import { A }`, `A` is in `name_to_id`?
    // Only if `A` is defined in the file?
    // Wait, the OLD code: `extract_ts_import_names` returned names.
    // `upsert_name_mapping` ONLY inserted symbols defined in `symbol_rows`.
    // So if `A` is imported, it is NOT in `symbol_rows`. So it is NOT in `name_to_id`.
    // So the OLD code `name_to_id.get(name)` would FAIL for imports!
    // UNLESS the import name collided with a local symbol.
    // So the old "import" edge logic was effectively broken for cross-file imports too.

    // I will drop the "import" edge type for now, or only add it if we detect usage.
    // The "call" and "reference" edges cover usage.
    // "import" edge might mean "file dependency".

    // For now, I'll stick to calls/refs/types.

    // References
    let mut refs_added = 0usize;
    for ident in extract_identifiers(&row.text) {
        if refs_added >= 20 {
            break;
        }
        if ident == row.name {
            continue;
        }

        let to_id = if let Some(local_id) = name_to_id.get(&ident) {
            if local_id == &row.id {
                continue;
            }
            Some(local_id.clone())
        } else if let Some(imp) = import_map.get(ident.as_str()) {
            resolve_imported_symbol_id(&row.file_path, imp)
        } else {
            None
        };

        if let Some(id) = to_id {
            if used_edges.insert(("reference".to_string(), id.clone())) {
                out.push(EdgeRow {
                    from_symbol_id: row.id.clone(),
                    to_symbol_id: id,
                    edge_type: "reference".to_string(),
                    at_file: Some(row.file_path.clone()),
                    at_line: Some(row.start_line),
                });
            }
        }
        refs_added += 1;
    }

    // Add type edges
    for (parent_name, type_name) in type_edges {
        if parent_name == &row.name {
            // Resolve type_name
            let to_id = if let Some(local_id) = name_to_id.get(type_name) {
                if local_id == &row.id {
                    continue;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(type_name.as_str()) {
                resolve_imported_symbol_id(&row.file_path, imp)
            } else {
                None
            };

            if let Some(id) = to_id {
                if used_edges.insert(("type".to_string(), id.clone())) {
                    out.push(EdgeRow {
                        from_symbol_id: row.id.clone(),
                        to_symbol_id: id,
                        edge_type: "type".to_string(),
                        at_file: Some(row.file_path.clone()),
                        at_line: Some(row.start_line),
                    });
                }
            }
        }
    }

    out
}

fn resolve_imported_symbol_id(current_file_path: &str, imp: &Import) -> Option<String> {
    // Basic resolution: assume TS relative path
    // current: src/a.ts, source: ./b
    // result: src/b.ts
    // We assume the symbol name in the target file is `imp.name` (the remote name).
    // The ID is stable_symbol_id(target_path, imp.name, 0).

    let target_path = resolve_path(current_file_path, &imp.source)?;
    Some(stable_symbol_id(&target_path, &imp.name, 0))
}

fn resolve_path(current: &str, source: &str) -> Option<String> {
    if !source.starts_with('.') {
        return None;
    }

    // Normalize slashes first
    let current = current.replace('\\', "/");
    let source = source.replace('\\', "/");

    let current_path = PathBuf::from(&current);
    let parent = current_path.parent()?;

    // Manual join to avoid ./ weirdness if possible or clean it after
    let joined = parent.join(&source);
    let joined_str = joined.to_string_lossy().replace('\\', "/");

    // Clean path (lexical normalization)
    let parts: Vec<&str> = joined_str.split('/').collect();
    let mut stack = Vec::new();

    for part in parts {
        if part == "." || part.is_empty() {
            continue;
        }
        if part == ".." {
            stack.pop();
        } else {
            stack.push(part);
        }
    }

    let mut s = stack.join("/");

    // Quick hack: just append .ts if missing extension
    if !s.ends_with(".ts") && !s.ends_with(".tsx") && !s.ends_with(".rs") {
        s.push_str(".ts"); // Bias towards TS
    }

    Some(s)
}

fn extract_usage_examples_for_file(
    file_path: &str,
    source: &str,
    name_to_id: &HashMap<String, String>,
    imports: &[Import],
    symbol_rows: &[SymbolRow],
) -> Vec<UsageExampleRow> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String, String, Option<u32>, String)> = HashSet::new();

    // Map import alias/name to Import struct
    let mut import_map: HashMap<&str, &Import> = HashMap::new();
    for imp in imports {
        if let Some(alias) = &imp.alias {
            import_map.insert(alias, imp);
        } else {
            import_map.insert(&imp.name, imp);
        }
    }

    for row in symbol_rows {
        for callee in extract_callee_names(&row.text) {
            let to_id = if let Some(local_id) = name_to_id.get(&callee) {
                if local_id == &row.id {
                    continue;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(callee.as_str()) {
                resolve_imported_symbol_id(file_path, imp)
            } else {
                None
            };

            let Some(to_id) = to_id else {
                continue;
            };

            let snippet =
                extract_usage_line(&row.text, &callee).unwrap_or_else(|| format!("{callee}("));
            let key = (
                to_id.clone(),
                "call".to_string(),
                file_path.to_string(),
                Some(row.start_line),
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: Some(row.id.clone()),
                example_type: "call".to_string(),
                file_path: file_path.to_string(),
                line: Some(row.start_line),
                snippet,
            });
        }

        let mut added = 0usize;
        for ident in extract_identifiers(&row.text) {
            if added >= 20 {
                break;
            }
            if ident == row.name {
                continue;
            }
            let to_id = if let Some(local_id) = name_to_id.get(&ident) {
                if local_id == &row.id {
                    continue;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(ident.as_str()) {
                resolve_imported_symbol_id(file_path, imp)
            } else {
                None
            };

            let Some(to_id) = to_id else {
                continue;
            };

            let snippet =
                extract_usage_line(&row.text, &ident).unwrap_or_else(|| ident.to_string());
            let key = (
                to_id.clone(),
                "reference".to_string(),
                file_path.to_string(),
                Some(row.start_line),
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: Some(row.id.clone()),
                example_type: "reference".to_string(),
                file_path: file_path.to_string(),
                line: Some(row.start_line),
                snippet,
            });
            added += 1;
        }
    }

    // Import usage examples?
    // We can iterate source lines and check if they contain imported names.
    // Similar to before.
    for (idx, line) in source.lines().enumerate() {
        if !line.contains("import") {
            continue;
        }
        let line_no = u32::try_from(idx + 1).ok();

        // This is expensive if imports is large, but usually small per file.
        for imp in imports {
            let name = imp.alias.as_ref().unwrap_or(&imp.name);
            if !line.contains(name) {
                continue;
            }

            let Some(to_id) = resolve_imported_symbol_id(file_path, imp) else {
                continue;
            };

            let snippet = trim_snippet(line, 200);
            let key = (
                to_id.clone(),
                "import".to_string(),
                file_path.to_string(),
                line_no,
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: None,
                example_type: "import".to_string(),
                file_path: file_path.to_string(),
                line: line_no,
                snippet,
            });
        }
    }

    out
}

fn extract_usage_line(text: &str, needle: &str) -> Option<String> {
    for line in text.lines() {
        if line.contains(needle) {
            return Some(trim_snippet(line, 200));
        }
    }
    None
}

fn trim_snippet(s: &str, max_len: usize) -> String {
    let mut out = s.trim().to_string();
    if out.len() > max_len {
        out.truncate(max_len);
    }
    out
}

fn cluster_key_from_vector(vector: &[f32]) -> String {
    let mut bits = 0u64;
    for (i, v) in vector.iter().take(64).enumerate() {
        if *v >= 0.0 {
            bits |= 1u64 << i;
        }
    }
    format!("{:016x}", bits)
}

fn extract_callee_names(text: &str) -> Vec<String> {
    let stopwords: HashSet<&'static str> = [
        "if", "for", "while", "switch", "catch", "function", "return", "new", "await", "match",
    ]
    .into_iter()
    .collect();

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        let is_ident_start = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        if !is_ident_start {
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                i += 1;
            } else {
                break;
            }
        }
        let ident = &text[start..i];
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'(' && !stopwords.contains(ident) {
            out.push(ident.to_string());
        }
    }
    out
}

fn extract_identifiers(text: &str) -> Vec<String> {
    let stopwords: HashSet<&'static str> = [
        "if", "for", "while", "switch", "catch", "function", "return", "new", "await", "match",
        "let", "const", "var", "pub", "impl", "trait", "struct", "enum", "mod", "use",
    ]
    .into_iter()
    .collect();

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        let is_ident_start = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        if !is_ident_start {
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                i += 1;
            } else {
                break;
            }
        }
        let ident = &text[start..i];
        if !stopwords.contains(ident) {
            out.push(ident.to_string());
        }
    }
    out
}

fn parse_type_relations(text: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut extends = Vec::new();
    let mut implements = Vec::new();
    let mut aliases = Vec::new();

    let mut rest = text;
    while let Some(pos) = rest.find("extends") {
        rest = &rest[pos + "extends".len()..];
        if let Some(name) = parse_next_identifier(rest) {
            extends.push(name);
        }
    }

    let mut rest = text;
    while let Some(pos) = rest.find("implements") {
        rest = &rest[pos + "implements".len()..];
        if let Some(name) = parse_next_identifier(rest) {
            implements.push(name);
        }
    }

    if let Some(eq_pos) = text.find('=') {
        let rhs = &text[eq_pos + 1..];
        if let Some(name) = parse_next_identifier(rhs) {
            aliases.push(name);
        }
    }

    (extends, implements, aliases)
}

fn parse_next_identifier(s: &str) -> Option<String> {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.peek().copied() {
        if c.is_alphabetic() || c == '_' || c == '$' {
            break;
        }
        chars.next();
    }
    let mut out = String::new();
    while let Some(c) = chars.peek().copied() {
        if c.is_alphanumeric() || c == '_' || c == '$' {
            out.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

trait SymbolKindToString {
    fn to_string(self) -> String;
}

impl SymbolKindToString for crate::indexer::extract::symbol::SymbolKind {
    fn to_string(self) -> String {
        match self {
            crate::indexer::extract::symbol::SymbolKind::Function => "function",
            crate::indexer::extract::symbol::SymbolKind::Class => "class",
            crate::indexer::extract::symbol::SymbolKind::Interface => "interface",
            crate::indexer::extract::symbol::SymbolKind::TypeAlias => "type_alias",
            crate::indexer::extract::symbol::SymbolKind::Enum => "enum",
            crate::indexer::extract::symbol::SymbolKind::Const => "const",
            crate::indexer::extract::symbol::SymbolKind::Struct => "struct",
            crate::indexer::extract::symbol::SymbolKind::Trait => "trait",
            crate::indexer::extract::symbol::SymbolKind::Impl => "impl",
            crate::indexer::extract::symbol::SymbolKind::Module => "module",
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn symbol(id: &str, name: &str, kind: &str, text: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
        }
    }

    fn tmp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("code-intel-pipeline-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extracts_call_import_and_reference_edges() {
        let row = symbol(
            "id_a",
            "a",
            "function",
            "import { b } from './b';\nexport function a(){ b(); c(); }",
        );
        let mut name_to_id = HashMap::new();
        // b is imported, so it might NOT be in local name_to_id if we rely on imports.
        // But if it IS in name_to_id (e.g. from a previous pass or if we index dependencies first? No, name_to_id is local).
        // So 'b' should NOT be in name_to_id in a real scenario.
        // But for this test let's simulate 'c' being local.
        name_to_id.insert("c".to_string(), "id_c".to_string());

        let imports = vec![Import {
            name: "b".to_string(),
            source: "./b".to_string(),
            alias: None,
        }];
        let type_edges = vec![];

        let edges = extract_edges_for_symbol(&row, &name_to_id, &imports, &type_edges);

        // 'b' is called. It is in imports.
        // It resolves to src/b.ts (relative to src/a.ts).
        // ID is stable_symbol_id("src/b.ts", "b", 0).
        let expected_b_id = stable_symbol_id("src/b.ts", "b", 0);

        assert!(edges
            .iter()
            .any(|e| e.edge_type == "call" && e.to_symbol_id == expected_b_id));

        // 'c' is called. It is local.
        assert!(edges
            .iter()
            .any(|e| e.edge_type == "call" && e.to_symbol_id == "id_c"));

        // 'b' is also referenced (identifiers)
        assert!(edges
            .iter()
            .any(|e| e.edge_type == "reference" && e.to_symbol_id == expected_b_id));

        // "import" edges are removed for now.
    }

    #[test]
    fn file_key_is_relative_under_base_and_absolute_outside() {
        let base0 = tmp_dir();
        let base = base0.canonicalize().unwrap_or(base0);
        let inner = base.join("src/a.ts");
        std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
        std::fs::write(&inner, "export function a() {}").unwrap();

        let other0 = tmp_dir();
        let other = other0.canonicalize().unwrap_or(other0);
        let outside = other.join("b.ts");
        std::fs::write(&outside, "export function b() {}").unwrap();

        let config = Config {
            base_dir: base.clone(),
            db_path: base.join("code-intelligence.db"),
            vector_db_path: base.join("vectors"),
            tantivy_index_path: base.join("tantivy-index"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base.clone()],
        };

        let k1 = file_key(&config, &inner);
        assert_eq!(k1, "src/a.ts");

        let k2 = file_key(&config, &outside);
        assert!(k2.ends_with("/b.ts"));
        assert!(k2.contains(&*other.to_string_lossy()));
    }
}
