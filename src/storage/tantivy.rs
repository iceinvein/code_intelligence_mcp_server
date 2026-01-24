use crate::storage::sqlite::SymbolRow;
use crate::text;
use anyhow::{anyhow, Context, Result};
use std::{path::Path, sync::Mutex};
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::TantivyDocument,
    schema::{
        Field, IndexRecordOption, TextFieldIndexing, TextOptions, Value, INDEXED, STORED, STRING,
    },
    tokenizer::{Token, TokenStream, Tokenizer},
    Index, IndexReader, IndexWriter, ReloadPolicy, Term,
};

const TANTIVY_SCHEMA_VERSION: &str = "4";

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub score: f32,
    pub id: String,
    pub name: String,
    pub file_path: String,
    pub kind: String,
    pub exported: bool,
}

#[derive(Debug, Clone, Copy)]
struct Fields {
    id: Field,
    name: Field,
    name_ngram: Field,
    file_path: Field,
    kind: Field,
    exported: Field,
    text: Field,
    text_ngram: Field,
}

pub struct TantivyIndex {
    index: Index,
    fields: Fields,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
}

#[derive(Clone, Copy)]
struct CodeTokenizer;

struct CodeTokenStream {
    tokens: Vec<Token>,
    i: usize,
}

impl TokenStream for CodeTokenStream {
    fn advance(&mut self) -> bool {
        if self.i >= self.tokens.len() {
            return false;
        }
        self.i += 1;
        true
    }

    fn token(&self) -> &Token {
        &self.tokens[self.i - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        let idx = self.i - 1;
        &mut self.tokens[idx]
    }
}

impl Tokenizer for CodeTokenizer {
    type TokenStream<'a> = CodeTokenStream;

    fn token_stream(&mut self, text_in: &str) -> CodeTokenStream {
        let normalized = text::split_identifier_like(text_in).to_lowercase();
        let mut tokens = Vec::new();
        let mut pos = 0usize;
        let bytes = normalized.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let end = i;
            let token_text = &normalized[start..end];
            tokens.push(Token {
                offset_from: start,
                offset_to: end,
                position: pos,
                text: token_text.to_string(),
                position_length: 1,
            });
            pos += 1;
        }

        CodeTokenStream { tokens, i: 0 }
    }
}

#[derive(Clone, Copy)]
struct CodeNgramTokenizer;

impl Tokenizer for CodeNgramTokenizer {
    type TokenStream<'a> = CodeTokenStream;

    fn token_stream(&mut self, text_in: &str) -> CodeTokenStream {
        let normalized = text::split_identifier_like(text_in).to_lowercase();
        let mut tokens = Vec::new();
        let mut pos = 0usize;

        for word in normalized.split_whitespace() {
            let chars: Vec<char> = word.chars().collect();
            if chars.len() < 3 {
                tokens.push(Token {
                    offset_from: 0,
                    offset_to: 0,
                    position: pos,
                    text: word.to_string(),
                    position_length: 1,
                });
                pos += 1;
                continue;
            }

            let max_n = 5usize.min(chars.len());
            for n in 3..=max_n {
                for start in 0..=(chars.len().saturating_sub(n)) {
                    let end = start + n;
                    tokens.push(Token {
                        offset_from: 0,
                        offset_to: 0,
                        position: pos,
                        text: chars[start..end].iter().collect::<String>(),
                        position_length: 1,
                    });
                }
            }
            pos += 1;
        }

        CodeTokenStream { tokens, i: 0 }
    }
}

impl TantivyIndex {
    pub fn open_or_create(index_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_dir).with_context(|| {
            format!(
                "Failed to create tantivy index directory: {}",
                index_dir.display()
            )
        })?;

        // Clean up stale lock files from previous crashes
        // These can remain after abnormal termination and prevent writer creation
        let meta_lock = index_dir.join(".tantivy-meta.lock");
        let writer_lock = index_dir.join(".tantivy-writer.lock");
        let _ = std::fs::remove_file(&meta_lock);
        let _ = std::fs::remove_file(&writer_lock);

        let version_path = index_dir.join("schema_version");
        let existing_version = std::fs::read_to_string(&version_path)
            .ok()
            .map(|s| s.trim().to_string());
        if existing_version.as_deref() != Some(TANTIVY_SCHEMA_VERSION) && index_dir.exists() {
            std::fs::remove_dir_all(index_dir).with_context(|| {
                format!(
                    "Failed to remove existing tantivy index directory: {}",
                    index_dir.display()
                )
            })?;
            std::fs::create_dir_all(index_dir).with_context(|| {
                format!(
                    "Failed to create tantivy index directory: {}",
                    index_dir.display()
                )
            })?;
        }

        let index = match Index::open_in_dir(index_dir) {
            Ok(index) => index,
            Err(_) => {
                let schema = build_schema();
                Index::create_in_dir(index_dir, schema).context("Failed to create tantivy index")?
            }
        };

        register_tokenizers(&index);
        let _ = std::fs::write(&version_path, TANTIVY_SCHEMA_VERSION);

        let schema = index.schema();
        let fields = Fields {
            id: schema
                .get_field("id")
                .context("Missing tantivy field: id")?,
            name: schema
                .get_field("name")
                .context("Missing tantivy field: name")?,
            name_ngram: schema
                .get_field("name_ngram")
                .context("Missing tantivy field: name_ngram")?,
            file_path: schema
                .get_field("file_path")
                .context("Missing tantivy field: file_path")?,
            kind: schema
                .get_field("kind")
                .context("Missing tantivy field: kind")?,
            exported: schema
                .get_field("exported")
                .context("Missing tantivy field: exported")?,
            text: schema
                .get_field("text")
                .context("Missing tantivy field: text")?,
            text_ngram: schema
                .get_field("text_ngram")
                .context("Missing tantivy field: text_ngram")?,
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .context("Failed to create tantivy reader")?;

        let writer = index
            .writer(64 * 1024 * 1024)
            .context("Failed to create tantivy writer")?;

        Ok(Self {
            index,
            fields,
            reader,
            writer: Mutex::new(writer),
        })
    }

    pub fn recreate(index_dir: &Path) -> Result<Self> {
        if index_dir.exists() {
            std::fs::remove_dir_all(index_dir).with_context(|| {
                format!(
                    "Failed to remove existing tantivy index directory: {}",
                    index_dir.display()
                )
            })?;
        }
        Self::open_or_create(index_dir)
    }

    pub fn upsert_symbol(&self, symbol: &SymbolRow) -> Result<()> {
        let writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Tantivy writer mutex poisoned"))?;

        writer.delete_term(Term::from_field_text(self.fields.id, &symbol.id));

        let expanded_text = expand_index_text(&symbol.name, &symbol.text);

        writer.add_document(doc!(
            self.fields.id => symbol.id.as_str(),
            self.fields.name => symbol.name.as_str(),
            self.fields.name_ngram => symbol.name.as_str(),
            self.fields.file_path => symbol.file_path.as_str(),
            self.fields.kind => symbol.kind.as_str(),
            self.fields.exported => if symbol.exported { 1u64 } else { 0u64 },
            self.fields.text => expanded_text.as_str(),
            self.fields.text_ngram => expanded_text.as_str(),
        ))?;

        Ok(())
    }

    pub fn delete_symbols_by_file(&self, file_path: &str) -> Result<()> {
        let writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Tantivy writer mutex poisoned"))?;

        writer.delete_term(Term::from_field_text(self.fields.file_path, file_path));
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Tantivy writer mutex poisoned"))?;
        writer.commit().context("Failed to commit tantivy writer")?;
        self.reader
            .reload()
            .context("Failed to reload tantivy reader")?;
        Ok(())
    }

    pub fn rebuild(index_dir: &Path, symbols: impl IntoIterator<Item = SymbolRow>) -> Result<Self> {
        let fresh = TantivyIndex::recreate(index_dir)?;
        for symbol in symbols {
            fresh.upsert_symbol(&symbol)?;
        }
        fresh.commit()?;
        Ok(fresh)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();
        let mut out = self.search_in_fields(
            &searcher,
            query,
            limit,
            1.0,
            &[self.fields.name, self.fields.text],
        )?;

        if out.len() < limit && !query.contains('"') && looks_like_partial(query) {
            let remaining = limit.saturating_sub(out.len());
            if remaining > 0 {
                let extra = self.search_in_fields(
                    &searcher,
                    query,
                    remaining * 2,
                    0.35,
                    &[self.fields.name_ngram, self.fields.text_ngram],
                )?;
                let mut seen: std::collections::HashSet<String> =
                    out.iter().map(|h| h.id.clone()).collect();
                for hit in extra {
                    if out.len() >= limit {
                        break;
                    }
                    if seen.insert(hit.id.clone()) {
                        out.push(hit);
                    }
                }
            }
        }

        Ok(out)
    }

    fn search_in_fields(
        &self,
        searcher: &tantivy::Searcher,
        query: &str,
        limit: usize,
        score_multiplier: f32,
        fields: &[Field],
    ) -> Result<Vec<SearchHit>> {
        let query_parser = QueryParser::for_index(&self.index, fields.to_vec());
        let parsed_query = query_parser
            .parse_query(query)
            .with_context(|| format!("Failed to parse tantivy query: {query}"))?;

        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .context("Failed to search tantivy index")?;

        let mut out = Vec::with_capacity(top_docs.len());
        for (score, addr) in top_docs {
            let retrieved = searcher
                .doc::<TantivyDocument>(addr)
                .context("Failed to load tantivy doc")?;
            let id = retrieved
                .get_first(self.fields.id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = retrieved
                .get_first(self.fields.name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let file_path = retrieved
                .get_first(self.fields.file_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let kind = retrieved
                .get_first(self.fields.kind)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let exported = retrieved
                .get_first(self.fields.exported)
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                != 0;

            out.push(SearchHit {
                score: score * score_multiplier,
                id,
                name,
                file_path,
                kind,
                exported,
            });
        }
        Ok(out)
    }
}

fn build_schema() -> tantivy::schema::Schema {
    let mut builder = tantivy::schema::Schema::builder();

    builder.add_text_field("id", STRING | STORED);
    let indexing = TextFieldIndexing::default()
        .set_tokenizer("code")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text_options = TextOptions::default()
        .set_indexing_options(indexing)
        .set_stored();
    builder.add_text_field("name", text_options.clone());
    let ngram_indexing = TextFieldIndexing::default()
        .set_tokenizer("code_ngram")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let ngram_options = TextOptions::default().set_indexing_options(ngram_indexing);
    builder.add_text_field("name_ngram", ngram_options.clone());
    builder.add_text_field("file_path", STRING | STORED);
    builder.add_text_field("kind", STRING | STORED);
    builder.add_u64_field("exported", INDEXED | STORED);
    builder.add_text_field("text", text_options);
    builder.add_text_field("text_ngram", ngram_options);

    builder.build()
}

fn register_tokenizers(index: &Index) {
    index.tokenizers().register("code", CodeTokenizer);
    index
        .tokenizers()
        .register("code_ngram", CodeNgramTokenizer);
}

fn expand_index_text(name: &str, text: &str) -> String {
    let split = text::split_identifier_like(name);
    if split.is_empty() {
        return text.to_string();
    }
    if text.contains(&split) {
        return text.to_string();
    }
    format!("{text} {split}")
}

fn looks_like_partial(query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return false;
    }
    let lowered = q.to_lowercase();
    let tokens = lowered.split_whitespace().collect::<Vec<_>>();
    if tokens.len() != 1 {
        return false;
    }
    let t = tokens[0];
    (3..=12).contains(&t.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_index_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("code-intel-tantivy-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_symbol(id: &str, name: &str, text: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
        }
    }

    #[test]
    fn indexes_and_searches_persisted_docs() {
        let dir = tmp_index_dir();

        let index = TantivyIndex::open_or_create(&dir).unwrap();
        index
            .upsert_symbol(&sample_symbol("id1", "alpha", "export function alpha() {}"))
            .unwrap();
        index
            .upsert_symbol(&sample_symbol(
                "id2",
                "beta",
                "export const beta = { nested: { a: 1 } }",
            ))
            .unwrap();
        index.commit().unwrap();

        let hits = index.search("alpha", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "id1"));

        drop(index);

        let reopened = TantivyIndex::open_or_create(&dir).unwrap();
        let hits2 = reopened.search("beta", 10).unwrap();
        assert!(hits2.iter().any(|h| h.id == "id2"));
    }

    #[test]
    fn indexes_camel_case_split_tokens() {
        let dir = tmp_index_dir();
        let index = TantivyIndex::open_or_create(&dir).unwrap();

        index
            .upsert_symbol(&sample_symbol(
                "id1",
                "DBConnection",
                "class DBConnection {}",
            ))
            .unwrap();
        index.commit().unwrap();

        // Should match "connection" (split from DBConnection)
        let hits = index.search("connection", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "id1"),
            "Should match 'connection'"
        );

        // Should match "db" (split from DBConnection)
        let hits2 = index.search("db", 10).unwrap();
        assert!(hits2.iter().any(|h| h.id == "id1"), "Should match 'db'");
    }

    #[test]
    fn indexes_separator_and_digit_split_tokens() {
        let dir = tmp_index_dir();
        let index = TantivyIndex::open_or_create(&dir).unwrap();

        index
            .upsert_symbol(&sample_symbol(
                "id1",
                "HTTP2Server_v1",
                "class HTTP2Server_v1 {}",
            ))
            .unwrap();
        index.commit().unwrap();

        let hits = index.search("server", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "id1"));
        let hits2 = index.search("2", 10).unwrap();
        assert!(hits2.iter().any(|h| h.id == "id1"));
        let hits3 = index.search("v1", 10).unwrap();
        assert!(hits3.iter().any(|h| h.id == "id1"));
    }

    #[test]
    fn supports_phrase_queries() {
        let dir = tmp_index_dir();
        let index = TantivyIndex::open_or_create(&dir).unwrap();

        index
            .upsert_symbol(&sample_symbol(
                "id1",
                "alpha",
                "export function alpha() { return foo_bar(); }",
            ))
            .unwrap();
        index.commit().unwrap();

        let hits = index.search("\"return foo\"", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "id1"));
    }

    #[test]
    fn finds_partial_substring_via_ngram_fallback() {
        let dir = tmp_index_dir();
        let index = TantivyIndex::open_or_create(&dir).unwrap();

        index
            .upsert_symbol(&sample_symbol(
                "id1",
                "DBConnection",
                "class DBConnection { connect() {} }",
            ))
            .unwrap();
        index.commit().unwrap();

        let hits = index.search("nect", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "id1"));
    }
}
