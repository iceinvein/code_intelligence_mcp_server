use crate::storage::sqlite::SymbolRow;
use anyhow::{anyhow, Context, Result};
use std::{path::Path, sync::Mutex};
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::TantivyDocument,
    schema::{Field, Value, INDEXED, STORED, STRING, TEXT},
    Index, IndexReader, IndexWriter, ReloadPolicy, Term,
};

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
    file_path: Field,
    kind: Field,
    exported: Field,
    text: Field,
}

pub struct TantivyIndex {
    index: Index,
    fields: Fields,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
}

impl TantivyIndex {
    pub fn open_or_create(index_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_dir).with_context(|| {
            format!(
                "Failed to create tantivy index directory: {}",
                index_dir.display()
            )
        })?;

        let index = match Index::open_in_dir(index_dir) {
            Ok(index) => index,
            Err(_) => {
                let schema = build_schema();
                Index::create_in_dir(index_dir, schema).context("Failed to create tantivy index")?
            }
        };

        let schema = index.schema();
        let fields = Fields {
            id: schema
                .get_field("id")
                .context("Missing tantivy field: id")?,
            name: schema
                .get_field("name")
                .context("Missing tantivy field: name")?,
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

        // Tweak 3: Index normalization (CamelCase splitting)
        let split_name = split_camel_case(&symbol.name);
        let expanded_text = if split_name != symbol.name {
            format!("{} {}", symbol.text, split_name)
        } else {
            symbol.text.clone()
        };

        writer.add_document(doc!(
            self.fields.id => symbol.id.as_str(),
            self.fields.name => symbol.name.as_str(),
            self.fields.file_path => symbol.file_path.as_str(),
            self.fields.kind => symbol.kind.as_str(),
            self.fields.exported => if symbol.exported { 1u64 } else { 0u64 },
            self.fields.text => expanded_text.as_str(),
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
        let query_parser =
            QueryParser::for_index(&self.index, vec![self.fields.name, self.fields.text]);
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
                score,
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
    builder.add_text_field("name", TEXT | STORED);
    builder.add_text_field("file_path", STRING | STORED);
    builder.add_text_field("kind", STRING | STORED);
    builder.add_u64_field("exported", INDEXED | STORED);
    builder.add_text_field("text", TEXT | STORED);

    builder.build()
}

fn split_camel_case(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            if prev.is_lowercase() {
                result.push(' ');
            } else if i + 1 < chars.len() && chars[i + 1].is_lowercase() {
                result.push(' ');
            }
        }
        result.push(c);
    }
    result
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
}
