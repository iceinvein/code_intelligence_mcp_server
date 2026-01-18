use anyhow::{anyhow, Context, Result};
use arrow_array::{
    types::Float32Type, Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::{
    query::{ExecutableQuery, QueryBase},
    Connection,
};
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub exported: bool,
    pub language: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub exported: bool,
    pub language: String,
    pub distance: Option<f32>,
}

pub struct LanceDbStore {
    db: Connection,
}

impl LanceDbStore {
    pub async fn connect(path: &Path) -> Result<Self> {
        let uri = path
            .to_str()
            .ok_or_else(|| anyhow!("VECTOR_DB_PATH is not valid UTF-8"))?;

        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create VECTOR_DB_PATH: {}", path.display()))?;

        let db = lancedb::connect(uri)
            .execute()
            .await
            .context("Failed to connect to lancedb")?;
        Ok(Self { db })
    }

    pub async fn open_or_create_table(
        &self,
        table_name: &str,
        vector_dim: usize,
    ) -> Result<LanceVectorTable> {
        let existing = self
            .db
            .table_names()
            .execute()
            .await
            .context("Failed to list lancedb table names")?;

        if !existing.iter().any(|n| n == table_name) {
            let schema = Arc::new(build_schema(vector_dim));
            self.db
                .create_empty_table(table_name, schema)
                .execute()
                .await
                .context("Failed to create lancedb table")?;
        }

        let table = self
            .db
            .open_table(table_name)
            .execute()
            .await
            .context("Failed to open lancedb table")?;

        Ok(LanceVectorTable { table, vector_dim })
    }
}

pub struct LanceVectorTable {
    table: lancedb::Table,
    vector_dim: usize,
}

impl LanceVectorTable {
    pub fn vector_dim(&self) -> usize {
        self.vector_dim
    }

    pub async fn delete_records_by_file_path(&self, file_path: &str) -> Result<()> {
        let escaped = escape_lancedb_string(file_path);
        let predicate = format!("file_path = '{escaped}'");

        self.table
            .delete(&predicate)
            .await
            .context("Failed to delete lancedb records by file_path")?;

        Ok(())
    }

    pub async fn add_records(&self, records: &[VectorRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        for record in records {
            if record.vector.len() != self.vector_dim {
                return Err(anyhow!(
                    "Vector dim mismatch for id {}: expected {}, got {}",
                    record.id,
                    self.vector_dim,
                    record.vector.len()
                ));
            }
        }

        let schema = Arc::new(build_schema(self.vector_dim));
        let batch = build_record_batch(schema.clone(), records, self.vector_dim)?;
        let batches = RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema.clone());

        self.table
            .add(Box::new(batches))
            .execute()
            .await
            .context("Failed to add records to lancedb table")?;

        Ok(())
    }

    pub async fn search(&self, query_vector: &[f32], limit: usize) -> Result<Vec<VectorHit>> {
        if query_vector.len() != self.vector_dim {
            return Err(anyhow!(
                "Query vector dim mismatch: expected {}, got {}",
                self.vector_dim,
                query_vector.len()
            ));
        }

        let stream = self
            .table
            .query()
            .nearest_to(query_vector)
            .context("Failed to create lancedb nearest_to query")?
            .limit(limit)
            .execute()
            .await
            .context("Failed to execute lancedb query")?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;

        let mut out = Vec::new();
        for batch in batches {
            let id = batch
                .column_by_name("id")
                .ok_or_else(|| anyhow!("Missing id column in lancedb result"))?;
            let name = batch
                .column_by_name("name")
                .ok_or_else(|| anyhow!("Missing name column in lancedb result"))?;
            let kind = batch
                .column_by_name("kind")
                .ok_or_else(|| anyhow!("Missing kind column in lancedb result"))?;
            let file_path = batch
                .column_by_name("file_path")
                .ok_or_else(|| anyhow!("Missing file_path column in lancedb result"))?;
            let exported = batch
                .column_by_name("exported")
                .ok_or_else(|| anyhow!("Missing exported column in lancedb result"))?;
            let language = batch
                .column_by_name("language")
                .ok_or_else(|| anyhow!("Missing language column in lancedb result"))?;
            let distance_col = batch
                .column_by_name("_distance")
                .or_else(|| batch.column_by_name("distance"));

            let id = id
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("id column is not StringArray"))?;
            let name = name
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("name column is not StringArray"))?;
            let kind = kind
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("kind column is not StringArray"))?;
            let file_path = file_path
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("file_path column is not StringArray"))?;
            let exported = exported
                .as_any()
                .downcast_ref::<BooleanArray>()
                .ok_or_else(|| anyhow!("exported column is not BooleanArray"))?;
            let language = language
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("language column is not StringArray"))?;
            let distance_col = distance_col.and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for row in 0..batch.num_rows() {
                if id.is_null(row) {
                    continue;
                }
                out.push(VectorHit {
                    id: id.value(row).to_string(),
                    name: if name.is_null(row) {
                        "".to_string()
                    } else {
                        name.value(row).to_string()
                    },
                    kind: if kind.is_null(row) {
                        "".to_string()
                    } else {
                        kind.value(row).to_string()
                    },
                    file_path: if file_path.is_null(row) {
                        "".to_string()
                    } else {
                        file_path.value(row).to_string()
                    },
                    exported: !exported.is_null(row) && exported.value(row),
                    language: if language.is_null(row) {
                        "".to_string()
                    } else {
                        language.value(row).to_string()
                    },
                    distance: distance_col.and_then(|d| {
                        if d.is_null(row) {
                            None
                        } else {
                            Some(d.value(row))
                        }
                    }),
                });
            }
        }

        Ok(out)
    }
}

fn escape_lancedb_string(s: &str) -> String {
    s.replace('\'', "''")
}

fn build_schema(vector_dim: usize) -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                vector_dim as i32,
            ),
            true,
        ),
        Field::new("name", DataType::Utf8, true),
        Field::new("kind", DataType::Utf8, true),
        Field::new("file_path", DataType::Utf8, true),
        Field::new("exported", DataType::Boolean, true),
        Field::new("language", DataType::Utf8, true),
        Field::new("text", DataType::Utf8, true),
    ])
}

fn build_record_batch(
    schema: Arc<Schema>,
    records: &[VectorRecord],
    vector_dim: usize,
) -> Result<RecordBatch> {
    let ids = StringArray::from(records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>());
    let names = StringArray::from(records.iter().map(|r| r.name.as_str()).collect::<Vec<_>>());
    let kinds = StringArray::from(records.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>());
    let file_paths = StringArray::from(
        records
            .iter()
            .map(|r| r.file_path.as_str())
            .collect::<Vec<_>>(),
    );
    let languages = StringArray::from(
        records
            .iter()
            .map(|r| r.language.as_str())
            .collect::<Vec<_>>(),
    );
    let texts = StringArray::from(records.iter().map(|r| r.text.as_str()).collect::<Vec<_>>());
    let exported = BooleanArray::from(records.iter().map(|r| r.exported).collect::<Vec<_>>());

    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        records
            .iter()
            .map(|r| Some(r.vector.iter().copied().map(Some))),
        vector_dim as i32,
    );

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(vectors),
            Arc::new(names),
            Arc::new(kinds),
            Arc::new(file_paths),
            Arc::new(exported),
            Arc::new(languages),
            Arc::new(texts),
        ],
    )
    .context("Failed to build arrow record batch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_db_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("code-intel-lancedb-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn creates_table_adds_and_searches() {
        let dir = tmp_db_dir();
        let store = LanceDbStore::connect(&dir).await.unwrap();
        let table = store.open_or_create_table("symbols", 3).await.unwrap();

        table
            .add_records(&[
                VectorRecord {
                    id: "id1".to_string(),
                    vector: vec![1.0, 0.0, 0.0],
                    name: "alpha".to_string(),
                    kind: "function".to_string(),
                    file_path: "src/a.ts".to_string(),
                    exported: true,
                    language: "typescript".to_string(),
                    text: "export function alpha() {}".to_string(),
                },
                VectorRecord {
                    id: "id2".to_string(),
                    vector: vec![0.0, 1.0, 0.0],
                    name: "beta".to_string(),
                    kind: "function".to_string(),
                    file_path: "src/b.ts".to_string(),
                    exported: false,
                    language: "typescript".to_string(),
                    text: "function beta() {}".to_string(),
                },
            ])
            .await
            .unwrap();

        let hits = table.search(&[0.9, 0.1, 0.0], 1).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "id1");

        let reopened = store.open_or_create_table("symbols", 3).await.unwrap();
        let hits2 = reopened.search(&[0.0, 1.0, 0.0], 2).await.unwrap();
        assert!(hits2.iter().any(|h| h.id == "id2"));
    }
}
