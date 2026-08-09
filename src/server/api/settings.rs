//! `/api/settings` endpoints: expose the daemon's effective Tier 1 + Tier 2
//! configuration as a grouped descriptor catalog, and persist edits to
//! `server.toml` (restart-only; no live apply).

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::ApiError;
use crate::config::toml_writer::{write_settings, SettingChange};
use crate::config::{get_data_dir, StandaloneConfig};
use crate::path::Utf8Path;

pub(crate) enum FieldKind {
    /// Numeric field with an inclusive range. `integer` forces whole numbers.
    Number {
        min: f64,
        max: f64,
        integer: bool,
    },
    Bool,
    Enum(&'static [&'static str]),
    /// Comma-separated string (e.g. glob pattern lists).
    Csv,
    /// Free string (read-only in this catalog).
    Str,
}

impl FieldKind {
    fn type_str(&self) -> &'static str {
        match self {
            FieldKind::Number { .. } => "number",
            FieldKind::Bool => "bool",
            FieldKind::Enum(_) => "enum",
            FieldKind::Csv => "csv",
            FieldKind::Str => "string",
        }
    }
}

pub(crate) struct FieldSpec {
    pub key: &'static str,
    pub group: &'static str,
    pub toml_path: &'static [&'static str],
    pub kind: FieldKind,
    pub needs_reindex: bool,
    pub editable: bool,
    pub description: &'static str,
}

const BACKENDS: &[&str] = &["llamacpp", "hash"];
const DEVICES: &[&str] = &["metal", "cpu"];

/// The single source of truth for the settings surface. Both the GET descriptor
/// list and the PUT validator iterate this catalog.
pub(crate) fn catalog() -> Vec<FieldSpec> {
    vec![
        FieldSpec {
            key: "embeddings_backend",
            group: "Embeddings",
            toml_path: &["embeddings", "backend"],
            kind: FieldKind::Enum(BACKENDS),
            needs_reindex: true,
            editable: true,
            description:
                "Embedding backend. 'hash' is a fast non-semantic test backend and degrades search.",
        },
        FieldSpec {
            key: "embeddings_device",
            group: "Embeddings",
            toml_path: &["embeddings", "device"],
            kind: FieldKind::Enum(DEVICES),
            needs_reindex: true,
            editable: true,
            description: "Embedding compute device.",
        },
        FieldSpec {
            key: "index_patterns",
            group: "Indexing",
            toml_path: &["repos", "defaults", "index_patterns"],
            kind: FieldKind::Csv,
            needs_reindex: true,
            editable: true,
            description: "Comma-separated glob patterns to index.",
        },
        FieldSpec {
            key: "exclude_patterns",
            group: "Indexing",
            toml_path: &["repos", "defaults", "exclude_patterns"],
            kind: FieldKind::Csv,
            needs_reindex: true,
            editable: true,
            description: "Comma-separated glob patterns to skip.",
        },
        FieldSpec {
            key: "watch_mode",
            group: "Indexing",
            toml_path: &["repos", "defaults", "watch_mode"],
            kind: FieldKind::Bool,
            needs_reindex: false,
            editable: true,
            description: "Auto-reindex on file changes.",
        },
        FieldSpec {
            key: "consent_required",
            group: "Indexing",
            toml_path: &["indexing", "consent_required"],
            kind: FieldKind::Bool,
            needs_reindex: false,
            editable: true,
            description: "Require approval before every repository's first full index.",
        },
        FieldSpec {
            key: "reranker_enabled",
            group: "Indexing",
            toml_path: &["reranker", "enabled"],
            kind: FieldKind::Bool,
            needs_reindex: false,
            editable: true,
            description: "Load the cross-encoder reranker (~600MB GPU-resident).",
        },
        FieldSpec {
            key: "descriptions_enabled",
            group: "Indexing",
            toml_path: &["descriptions", "enabled"],
            kind: FieldKind::Bool,
            needs_reindex: true,
            editable: true,
            description: "Run the index-time LLM description backfill.",
        },
        FieldSpec {
            key: "warm_ttl_seconds",
            group: "Lifecycle",
            toml_path: &["lifecycle", "warm_ttl_seconds"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 86_400.0,
                integer: true,
            },
            needs_reindex: false,
            editable: true,
            description: "Idle seconds before a repo's in-memory state is evicted.",
        },
        FieldSpec {
            key: "missing_repo_grace_days",
            group: "Lifecycle",
            toml_path: &["lifecycle", "missing_repo_grace_days"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 365.0,
                integer: true,
            },
            needs_reindex: false,
            editable: true,
            description: "Days a repo's index survives after its folder is deleted. 0 never deletes a registered repo's index this way; it does not govern the separate sweep for unclaimed data directories.",
        },
        FieldSpec {
            key: "host",
            group: "Daemon",
            toml_path: &["server", "host"],
            kind: FieldKind::Str,
            needs_reindex: false,
            editable: false,
            description: "Bind host (install-level; read-only here).",
        },
        FieldSpec {
            key: "port",
            group: "Daemon",
            toml_path: &["server", "port"],
            kind: FieldKind::Number {
                min: 1.0,
                max: 65_535.0,
                integer: true,
            },
            needs_reindex: false,
            editable: false,
            description: "MCP port (install-level; read-only here).",
        },
        FieldSpec {
            key: "hybrid_alpha",
            group: "Retrieval",
            toml_path: &["retrieval", "hybrid_alpha"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 1.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Vector vs keyword weight (0 = keyword only, 1 = vector only).",
        },
        FieldSpec {
            key: "max_context_bytes",
            group: "Retrieval",
            toml_path: &["retrieval", "max_context_bytes"],
            kind: FieldKind::Number {
                min: 1_000.0,
                max: 10_000_000.0,
                integer: true,
            },
            needs_reindex: false,
            editable: true,
            description: "Max bytes of context assembled per query.",
        },
        FieldSpec {
            key: "rank_vector_weight",
            group: "Ranking",
            toml_path: &["ranking", "vector_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Weight of the vector signal.",
        },
        FieldSpec {
            key: "rank_keyword_weight",
            group: "Ranking",
            toml_path: &["ranking", "keyword_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Weight of the keyword signal.",
        },
        FieldSpec {
            key: "rank_exported_boost",
            group: "Ranking",
            toml_path: &["ranking", "exported_boost"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Boost for exported/public symbols.",
        },
        FieldSpec {
            key: "rank_index_file_boost",
            group: "Ranking",
            toml_path: &["ranking", "index_file_boost"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Boost for index/barrel files.",
        },
        FieldSpec {
            key: "rank_test_penalty",
            group: "Ranking",
            toml_path: &["ranking", "test_penalty"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Multiplier applied to test files.",
        },
        FieldSpec {
            key: "rank_popularity_weight",
            group: "Ranking",
            toml_path: &["ranking", "popularity_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Weight of the incoming-edge popularity signal.",
        },
        FieldSpec {
            key: "rank_popularity_cap",
            group: "Ranking",
            toml_path: &["ranking", "popularity_cap"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 1_000.0,
                integer: true,
            },
            needs_reindex: false,
            editable: true,
            description: "Cap on counted incoming edges.",
        },
        FieldSpec {
            key: "rrf_k",
            group: "RRF",
            toml_path: &["rrf", "k"],
            kind: FieldKind::Number {
                min: 1.0,
                max: 1_000.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "RRF rank constant.",
        },
        FieldSpec {
            key: "rrf_keyword_weight",
            group: "RRF",
            toml_path: &["rrf", "keyword_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "RRF weight for the keyword list.",
        },
        FieldSpec {
            key: "rrf_vector_weight",
            group: "RRF",
            toml_path: &["rrf", "vector_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "RRF weight for the vector list.",
        },
        FieldSpec {
            key: "rrf_graph_weight",
            group: "RRF",
            toml_path: &["rrf", "graph_weight"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 10.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "RRF weight for the graph list.",
        },
        FieldSpec {
            key: "learning_enabled",
            group: "Learning",
            toml_path: &["learning", "enabled"],
            kind: FieldKind::Bool,
            needs_reindex: false,
            editable: true,
            description: "Enable selection/affinity learning.",
        },
        FieldSpec {
            key: "learning_selection_boost",
            group: "Learning",
            toml_path: &["learning", "selection_boost"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 1.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Max boost from selection history.",
        },
        FieldSpec {
            key: "learning_file_affinity_boost",
            group: "Learning",
            toml_path: &["learning", "file_affinity_boost"],
            kind: FieldKind::Number {
                min: 0.0,
                max: 1.0,
                integer: false,
            },
            needs_reindex: false,
            editable: true,
            description: "Max boost from file access frequency.",
        },
    ]
}

/// Extract a field's current value from a config instance as JSON.
pub(crate) fn field_value(cfg: &StandaloneConfig, key: &str) -> Value {
    use crate::config::{EmbeddingsBackend, EmbeddingsDevice};
    match key {
        "embeddings_backend" => json!(match cfg.embeddings_backend {
            EmbeddingsBackend::LlamaCpp => "llamacpp",
            EmbeddingsBackend::Hash => "hash",
        }),
        "embeddings_device" => json!(match cfg.embeddings_device {
            EmbeddingsDevice::Metal => "metal",
            EmbeddingsDevice::Cpu => "cpu",
        }),
        "index_patterns" => json!(cfg.default_index_patterns.join(",")),
        "exclude_patterns" => json!(cfg.default_exclude_patterns.join(",")),
        "watch_mode" => json!(cfg.default_watch_mode),
        "consent_required" => json!(cfg.index_consent_required),
        "reranker_enabled" => json!(cfg.reranker_enabled),
        "descriptions_enabled" => json!(cfg.descriptions_enabled),
        "warm_ttl_seconds" => json!(cfg.warm_ttl_seconds),
        "missing_repo_grace_days" => json!(cfg.missing_repo_grace_days),
        "host" => json!(cfg.host),
        "port" => json!(cfg.port),
        "hybrid_alpha" => json!(cfg.hybrid_alpha),
        "max_context_bytes" => json!(cfg.max_context_bytes),
        "rank_vector_weight" => json!(cfg.rank_vector_weight),
        "rank_keyword_weight" => json!(cfg.rank_keyword_weight),
        "rank_exported_boost" => json!(cfg.rank_exported_boost),
        "rank_index_file_boost" => json!(cfg.rank_index_file_boost),
        "rank_test_penalty" => json!(cfg.rank_test_penalty),
        "rank_popularity_weight" => json!(cfg.rank_popularity_weight),
        "rank_popularity_cap" => json!(cfg.rank_popularity_cap),
        "rrf_k" => json!(cfg.rrf_k),
        "rrf_keyword_weight" => json!(cfg.rrf_keyword_weight),
        "rrf_vector_weight" => json!(cfg.rrf_vector_weight),
        "rrf_graph_weight" => json!(cfg.rrf_graph_weight),
        "learning_enabled" => json!(cfg.learning_enabled),
        "learning_selection_boost" => json!(cfg.learning_selection_boost),
        "learning_file_affinity_boost" => json!(cfg.learning_file_affinity_boost),
        _ => Value::Null,
    }
}

/// Build the grouped descriptor catalog with current + default values.
pub(crate) fn build_settings_response(cfg: &StandaloneConfig) -> Value {
    let defaults = StandaloneConfig::default();
    let fields: Vec<Value> = catalog()
        .iter()
        .map(|f| {
            let mut o = serde_json::Map::new();
            o.insert("key".into(), json!(f.key));
            o.insert("group".into(), json!(f.group));
            o.insert("type".into(), json!(f.kind.type_str()));
            o.insert("value".into(), field_value(cfg, f.key));
            o.insert("default".into(), field_value(&defaults, f.key));
            match &f.kind {
                FieldKind::Number { min, max, .. } => {
                    o.insert("range".into(), json!({ "min": min, "max": max }));
                }
                FieldKind::Enum(opts) => {
                    o.insert("options".into(), json!(opts));
                }
                FieldKind::Bool | FieldKind::Csv | FieldKind::Str => {}
            }
            o.insert("needs_restart".into(), json!(true));
            o.insert("needs_reindex".into(), json!(f.needs_reindex));
            o.insert("editable".into(), json!(f.editable));
            o.insert("description".into(), json!(f.description));
            Value::Object(o)
        })
        .collect();
    json!({ "fields": fields })
}

/// Validate one (spec, json value) pair and convert to a TOML change.
fn to_change(spec: &FieldSpec, v: &Value) -> Result<SettingChange, String> {
    if !spec.editable {
        return Err("read-only".to_string());
    }
    let item = match &spec.kind {
        FieldKind::Bool => toml_edit::value(v.as_bool().ok_or("expected a boolean")?),
        FieldKind::Number { min, max, integer } => {
            let n = v.as_f64().ok_or("expected a number")?;
            if n < *min || n > *max {
                return Err(format!("must be between {min} and {max}"));
            }
            if *integer {
                if n.fract() != 0.0 {
                    return Err("expected an integer".to_string());
                }
                toml_edit::value(n as i64)
            } else {
                toml_edit::value(n)
            }
        }
        FieldKind::Enum(opts) => {
            let s = v.as_str().ok_or("expected a string")?;
            if !opts.contains(&s) {
                return Err(format!("must be one of {opts:?}"));
            }
            toml_edit::value(s)
        }
        FieldKind::Csv => {
            let s = v.as_str().ok_or("expected a comma-separated string")?;
            if s.trim().is_empty() {
                return Err("must not be empty".to_string());
            }
            toml_edit::value(s)
        }
        FieldKind::Str => return Err("read-only".to_string()),
    };
    Ok(SettingChange {
        path: spec.toml_path,
        value: item,
    })
}

/// Validate all changes, then (only if every one is valid) write them. Returns
/// per-key (key, message) errors on any failure; nothing is written in that case.
pub(crate) fn apply_settings(
    toml_path: &Utf8Path,
    changes: &serde_json::Map<String, Value>,
) -> Result<(), Vec<(String, String)>> {
    let cat = catalog();
    let mut out: Vec<SettingChange> = Vec::new();
    let mut errors: Vec<(String, String)> = Vec::new();

    for (key, v) in changes {
        match cat.iter().find(|f| f.key == key.as_str()) {
            None => errors.push((key.clone(), "unknown setting".to_string())),
            Some(spec) => match to_change(spec, v) {
                Ok(c) => out.push(c),
                Err(msg) => errors.push((key.clone(), msg)),
            },
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    write_settings(toml_path, &out).map_err(|e| vec![("_".to_string(), e.to_string())])?;
    Ok(())
}

/// True if any changed key is flagged `needs_reindex` in the catalog.
pub(crate) fn changes_need_reindex(changes: &serde_json::Map<String, Value>) -> bool {
    let cat = catalog();
    changes
        .keys()
        .any(|k| cat.iter().any(|f| f.key == k.as_str() && f.needs_reindex))
}

fn server_toml_path() -> crate::path::Utf8PathBuf {
    get_data_dir().join("server.toml")
}

pub(crate) async fn handle_settings_get() -> Result<Json<Value>, ApiError> {
    let cfg = StandaloneConfig::load(None, None, None)
        .map_err(|e| ApiError::internal(format!("failed to load config: {e}")))?;
    Ok(Json(build_settings_response(&cfg)))
}

#[derive(Debug, Deserialize)]
pub(crate) struct SettingsPutRequest {
    changes: serde_json::Map<String, Value>,
}

pub(crate) async fn handle_settings_put(
    Json(req): Json<SettingsPutRequest>,
) -> Result<Response, ApiError> {
    let path = server_toml_path();
    match apply_settings(&path, &req.changes) {
        Ok(()) => {
            let cfg = StandaloneConfig::load(None, None, None)
                .map_err(|e| ApiError::internal(format!("failed to reload config: {e}")))?;
            Ok((
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "settings": build_settings_response(&cfg),
                    "needs_restart": true,
                    "needs_reindex": changes_need_reindex(&req.changes),
                })),
            )
                .into_response())
        }
        Err(errors) => {
            let errs: Vec<Value> = errors
                .iter()
                .map(|(k, m)| json!({ "key": k, "message": m }))
                .collect();
            Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "errors": errs })),
            )
                .into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_all_resolve_to_a_value() {
        let cfg = StandaloneConfig::default();
        for f in catalog() {
            assert!(
                !field_value(&cfg, f.key).is_null(),
                "catalog key {} has no field_value mapping",
                f.key
            );
        }
    }

    #[test]
    fn build_response_shapes_a_known_field() {
        let cfg = StandaloneConfig::default();
        let resp = build_settings_response(&cfg);
        let fields = resp["fields"].as_array().unwrap();
        let alpha = fields.iter().find(|f| f["key"] == "hybrid_alpha").unwrap();
        assert_eq!(alpha["group"], "Retrieval");
        assert_eq!(alpha["type"], "number");
        assert_eq!(alpha["needs_restart"], true);
        assert_eq!(alpha["needs_reindex"], false);
        assert_eq!(alpha["editable"], true);
        assert_eq!(alpha["range"]["max"], 1.0);
        let backend = fields
            .iter()
            .find(|f| f["key"] == "embeddings_backend")
            .unwrap();
        assert_eq!(backend["options"], json!(["llamacpp", "hash"]));
        assert_eq!(backend["needs_reindex"], true);
        let host = fields.iter().find(|f| f["key"] == "host").unwrap();
        assert_eq!(host["editable"], false);
    }

    #[test]
    fn catalog_exposes_the_missing_repo_grace_knob() {
        let cfg = StandaloneConfig::default();
        let fields = catalog();
        let spec = fields
            .iter()
            .find(|f| f.key == "missing_repo_grace_days")
            .expect("missing_repo_grace_days must be in the catalog");
        assert_eq!(spec.group, "Lifecycle");
        assert_eq!(spec.toml_path, &["lifecycle", "missing_repo_grace_days"]);
        assert!(spec.editable);
        assert!(!spec.needs_reindex);
        assert_eq!(field_value(&cfg, "missing_repo_grace_days"), json!(7));
    }

    #[test]
    fn apply_rejects_out_of_range_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::path::Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        let mut changes = serde_json::Map::new();
        changes.insert("hybrid_alpha".into(), json!(2.0));
        let err = apply_settings(&path, &changes).unwrap_err();
        assert_eq!(err[0].0, "hybrid_alpha");
        assert!(
            !path.exists(),
            "nothing should be written on validation failure"
        );
    }

    #[test]
    fn apply_rejects_unknown_and_readonly_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::path::Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        let mut changes = serde_json::Map::new();
        changes.insert("nope".into(), json!(1));
        assert_eq!(
            apply_settings(&path, &changes).unwrap_err()[0].1,
            "unknown setting"
        );
        let mut ro = serde_json::Map::new();
        ro.insert("port".into(), json!(1234));
        assert_eq!(apply_settings(&path, &ro).unwrap_err()[0].1, "read-only");
    }

    #[test]
    fn apply_writes_valid_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::path::Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        let mut changes = serde_json::Map::new();
        changes.insert("hybrid_alpha".into(), json!(0.85));
        changes.insert("embeddings_backend".into(), json!("hash"));
        apply_settings(&path, &changes).expect("apply ok");
        let toml = std::fs::read_to_string(path.as_std_path()).unwrap();
        let cfg = StandaloneConfig::from_toml_str(&toml).unwrap();
        assert!((cfg.hybrid_alpha - 0.85).abs() < 1e-6);
        assert_eq!(field_value(&cfg, "embeddings_backend"), json!("hash"));
    }

    #[test]
    fn apply_rejects_bad_enum_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::path::Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        let mut changes = serde_json::Map::new();
        changes.insert("embeddings_device".into(), json!("gpu"));
        assert!(apply_settings(&path, &changes).unwrap_err()[0]
            .1
            .contains("one of"));
    }

    #[test]
    fn put_then_rebuild_reflects_change() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::path::Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        let mut changes = serde_json::Map::new();
        changes.insert("hybrid_alpha".into(), json!(0.85));
        apply_settings(&path, &changes).expect("apply ok");
        let toml = std::fs::read_to_string(path.as_std_path()).unwrap();
        let cfg = StandaloneConfig::from_toml_str(&toml).unwrap();
        let resp = build_settings_response(&cfg);
        let alpha = resp["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == "hybrid_alpha")
            .unwrap();
        let v = alpha["value"].as_f64().unwrap();
        assert!(
            (v - 0.85).abs() < 1e-6,
            "value should reflect the saved change: {v}"
        );
    }
}
