//! External index producer registry and support tiers.

use std::io::ErrorKind;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::path::Utf8Path;
use crate::storage::sqlite::SqliteStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageTier {
    FirstClass,
    BuildAware,
    CompileDatabase,
    FallbackOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageSupport {
    pub language: &'static str,
    pub tier: LanguageTier,
    pub producer: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProducerSpec {
    id: &'static str,
    default_language: &'static str,
    default_program: &'static str,
    command_env: &'static str,
    output_file: &'static str,
}

const PRODUCER_SPECS: &[ProducerSpec] = &[
    ProducerSpec {
        id: "typescript",
        default_language: "typescript",
        default_program: "scip-typescript",
        command_env: "EXTERNAL_INDEX_TYPESCRIPT_COMMAND",
        output_file: "typescript-normalized.json",
    },
    ProducerSpec {
        id: "rust",
        default_language: "rust",
        default_program: "code-intelligence-external-rust",
        command_env: "EXTERNAL_INDEX_RUST_COMMAND",
        output_file: "rust-normalized.json",
    },
    ProducerSpec {
        id: "python",
        default_language: "python",
        default_program: "code-intelligence-external-python",
        command_env: "EXTERNAL_INDEX_PYTHON_COMMAND",
        output_file: "python-normalized.json",
    },
    ProducerSpec {
        id: "go",
        default_language: "go",
        default_program: "code-intelligence-external-go",
        command_env: "EXTERNAL_INDEX_GO_COMMAND",
        output_file: "go-normalized.json",
    },
    ProducerSpec {
        id: "java",
        default_language: "java",
        default_program: "code-intelligence-external-java",
        command_env: "EXTERNAL_INDEX_JAVA_COMMAND",
        output_file: "java-normalized.json",
    },
    ProducerSpec {
        id: "kotlin",
        default_language: "kotlin",
        default_program: "code-intelligence-external-kotlin",
        command_env: "EXTERNAL_INDEX_KOTLIN_COMMAND",
        output_file: "kotlin-normalized.json",
    },
    ProducerSpec {
        id: "csharp",
        default_language: "csharp",
        default_program: "code-intelligence-external-csharp",
        command_env: "EXTERNAL_INDEX_CSHARP_COMMAND",
        output_file: "csharp-normalized.json",
    },
    ProducerSpec {
        id: "swift",
        default_language: "swift",
        default_program: "code-intelligence-external-swift",
        command_env: "EXTERNAL_INDEX_SWIFT_COMMAND",
        output_file: "swift-normalized.json",
    },
    ProducerSpec {
        id: "c",
        default_language: "c",
        default_program: "code-intelligence-external-c",
        command_env: "EXTERNAL_INDEX_C_COMMAND",
        output_file: "c-normalized.json",
    },
    ProducerSpec {
        id: "cpp",
        default_language: "cpp",
        default_program: "code-intelligence-external-cpp",
        command_env: "EXTERNAL_INDEX_CPP_COMMAND",
        output_file: "cpp-normalized.json",
    },
    ProducerSpec {
        id: "ruby",
        default_language: "ruby",
        default_program: "code-intelligence-external-ruby",
        command_env: "EXTERNAL_INDEX_RUBY_COMMAND",
        output_file: "ruby-normalized.json",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalIndexRefreshMode {
    Disabled,
    Explicit,
    Watch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIndexConfig {
    pub auto_enabled: bool,
    pub producer: Option<String>,
    pub on_refresh: ExternalIndexRefreshMode,
}

impl Default for ExternalIndexConfig {
    fn default() -> Self {
        Self {
            auto_enabled: false,
            producer: None,
            on_refresh: ExternalIndexRefreshMode::Disabled,
        }
    }
}

pub fn supported_language_tiers() -> Vec<LanguageSupport> {
    vec![
        LanguageSupport {
            language: "typescript",
            tier: LanguageTier::FirstClass,
            producer: Some("typescript"),
        },
        LanguageSupport {
            language: "javascript",
            tier: LanguageTier::FirstClass,
            producer: Some("typescript"),
        },
        LanguageSupport {
            language: "rust",
            tier: LanguageTier::FirstClass,
            producer: Some("rust"),
        },
        LanguageSupport {
            language: "python",
            tier: LanguageTier::FirstClass,
            producer: Some("python"),
        },
        LanguageSupport {
            language: "go",
            tier: LanguageTier::FirstClass,
            producer: Some("go"),
        },
        LanguageSupport {
            language: "java",
            tier: LanguageTier::BuildAware,
            producer: Some("java"),
        },
        LanguageSupport {
            language: "kotlin",
            tier: LanguageTier::BuildAware,
            producer: Some("kotlin"),
        },
        LanguageSupport {
            language: "csharp",
            tier: LanguageTier::BuildAware,
            producer: Some("csharp"),
        },
        LanguageSupport {
            language: "swift",
            tier: LanguageTier::BuildAware,
            producer: Some("swift"),
        },
        LanguageSupport {
            language: "c",
            tier: LanguageTier::CompileDatabase,
            producer: Some("c"),
        },
        LanguageSupport {
            language: "cpp",
            tier: LanguageTier::CompileDatabase,
            producer: Some("cpp"),
        },
        LanguageSupport {
            language: "ruby",
            tier: LanguageTier::FallbackOnly,
            producer: Some("ruby"),
        },
    ]
}

pub fn supported_producers() -> Vec<&'static str> {
    let mut producers = supported_language_tiers()
        .into_iter()
        .filter_map(|support| support.producer)
        .collect::<Vec<_>>();
    producers.sort_unstable();
    producers.dedup();
    producers
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

pub fn typescript_command(program: &str, repo: &str, output: &str) -> ProducerCommand {
    producer_command(program, repo, output)
}

fn producer_command(program: &str, repo: &str, output: &str) -> ProducerCommand {
    ProducerCommand {
        program: program.to_string(),
        cwd: repo.to_string(),
        args: vec![
            "index".to_string(),
            "--output".to_string(),
            output.to_string(),
        ],
    }
}

pub fn generate_and_import(
    store: &SqliteStore,
    repo_root: &str,
    repo_data_dir: &Utf8Path,
    producer: Option<String>,
    language: Option<String>,
) -> Result<Value> {
    let requested_producer = producer
        .or_else(|| producer_for_language(language.as_deref()).map(str::to_string))
        .unwrap_or_else(|| "typescript".to_string());
    let supported_producers = supported_producers();
    if !supported_producers
        .iter()
        .any(|producer| *producer == requested_producer)
    {
        return Ok(json!({
            "ok": false,
            "status": "unsupported_producer",
            "producer": requested_producer,
            "language": language,
            "supported_producers": supported_producers,
        }));
    }

    let spec = PRODUCER_SPECS
        .iter()
        .find(|spec| spec.id == requested_producer)
        .expect("supported producer must have a spec");
    generate_with_spec(store, repo_root, repo_data_dir, language, *spec)
}

fn producer_for_language(language: Option<&str>) -> Option<&'static str> {
    let language = language?;
    supported_language_tiers()
        .into_iter()
        .find(|support| support.language == language)
        .and_then(|support| support.producer)
}

fn generate_with_spec(
    store: &SqliteStore,
    repo_root: &str,
    repo_data_dir: &Utf8Path,
    language: Option<String>,
    spec: ProducerSpec,
) -> Result<Value> {
    let program =
        std::env::var(spec.command_env).unwrap_or_else(|_| spec.default_program.to_string());
    let external_dir = repo_data_dir.join("external");
    std::fs::create_dir_all(external_dir.as_std_path()).with_context(|| {
        format!(
            "Failed to create external index output dir: {}",
            external_dir
        )
    })?;
    let output_path = external_dir.join(spec.output_file);
    let command = producer_command(&program, repo_root, output_path.as_str());

    let output = match Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(json!({
                "ok": false,
                "status": "missing_toolchain",
                "producer": spec.id,
                "language": language,
                "program": command.program,
                "supported_producers": supported_producers(),
            }));
        }
        Err(err) => {
            return Ok(json!({
                "ok": false,
                "status": "producer_failed",
                "producer": spec.id,
                "language": language,
                "program": command.program,
                "error": err.to_string(),
                "supported_producers": supported_producers(),
            }));
        }
    };

    if !output.status.success() {
        return Ok(json!({
            "ok": false,
            "status": "producer_failed",
            "producer": spec.id,
            "language": language,
            "program": command.program,
            "exit_code": output.status.code(),
            "stderr": truncate_lossy(&output.stderr, 4_000),
            "supported_producers": supported_producers(),
        }));
    }

    if !output_path.exists() {
        return Ok(json!({
            "ok": false,
            "status": "artifact_missing",
            "producer": spec.id,
            "language": language,
            "program": command.program,
            "artifact_path": output_path,
            "supported_producers": supported_producers(),
        }));
    }

    let report = crate::external_index::importer::import_external_index(
        store,
        repo_root,
        output_path.as_std_path(),
    )?;
    Ok(json!({
        "ok": true,
        "status": "imported",
        "producer": spec.id,
        "language": language.unwrap_or_else(|| spec.default_language.to_string()),
        "program": command.program,
        "artifact_path": output_path,
        "index_id": report.index_id,
        "symbols_imported": report.symbols_imported,
        "references_imported": report.references_imported,
        "symbols_mapped": report.symbols_mapped,
        "symbols_unmapped": report.symbols_unmapped,
    }))
}

fn truncate_lossy(bytes: &[u8], max_chars: usize) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tiers_cover_existing_indexed_languages() {
        let langs = supported_language_tiers();
        for lang in [
            "typescript",
            "javascript",
            "rust",
            "python",
            "go",
            "java",
            "c",
            "cpp",
            "ruby",
            "kotlin",
            "csharp",
            "swift",
        ] {
            assert!(
                langs.iter().any(|tier| tier.language == lang),
                "missing {lang}"
            );
        }
    }

    #[test]
    fn every_supported_language_has_a_concrete_producer() {
        let langs = supported_language_tiers();
        for lang in [
            "typescript",
            "javascript",
            "rust",
            "python",
            "go",
            "java",
            "c",
            "cpp",
            "ruby",
            "kotlin",
            "csharp",
            "swift",
        ] {
            let support = langs
                .iter()
                .find(|tier| tier.language == lang)
                .unwrap_or_else(|| panic!("missing {lang}"));
            assert!(support.producer.is_some(), "missing producer for {lang}");
        }
    }

    #[test]
    fn language_selection_reports_missing_toolchain_for_non_typescript_producers() {
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");
        std::env::set_var(
            "EXTERNAL_INDEX_RUST_COMMAND",
            "__missing_rust_external_index__",
        );

        let response = generate_and_import(
            &store,
            repo.path().to_str().expect("utf8"),
            Utf8Path::from_path(repo_data.path()).expect("utf8"),
            None,
            Some("rust".to_string()),
        )
        .expect("response");

        std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "missing_toolchain");
        assert_eq!(response["producer"], "rust");
        assert_eq!(response["language"], "rust");
        assert_eq!(response["program"], "__missing_rust_external_index__");
    }

    #[test]
    fn default_generation_is_disabled() {
        let cfg = ExternalIndexConfig::default();
        assert!(!cfg.auto_enabled);
        assert_eq!(cfg.on_refresh, ExternalIndexRefreshMode::Disabled);
    }

    #[test]
    fn unknown_generation_reports_supported_producers() {
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let temp = tempfile::tempdir().expect("tempdir");
        let repo_data = tempfile::tempdir().expect("repo data");
        let response = generate_and_import(
            &store,
            temp.path().to_str().expect("utf8"),
            Utf8Path::from_path(repo_data.path()).expect("utf8"),
            Some("unknown".to_string()),
            Some("rust".to_string()),
        )
        .expect("response");
        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "unsupported_producer");
        assert_eq!(response["producer"], "unknown");
        let supported = response["supported_producers"]
            .as_array()
            .expect("supported producers array");
        assert!(supported.iter().any(|producer| producer == "typescript"));
        assert!(supported.iter().any(|producer| producer == "rust"));
    }

    #[test]
    fn typescript_producer_uses_configured_command() {
        let cmd = typescript_command(
            "custom-scip-typescript",
            "/repo",
            "/repo/.code-intelligence/external/typescript.json",
        );
        assert_eq!(cmd.program, "custom-scip-typescript");
        assert_eq!(cmd.cwd, "/repo");
        assert!(cmd.args.contains(&"index".to_string()));
        assert!(cmd.args.contains(&"--output".to_string()));
        assert!(cmd
            .args
            .contains(&"/repo/.code-intelligence/external/typescript.json".to_string()));
    }

    #[test]
    fn generate_typescript_reports_missing_toolchain_cleanly() {
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");
        std::env::set_var(
            "EXTERNAL_INDEX_TYPESCRIPT_COMMAND",
            "__missing_scip_typescript_binary__",
        );

        let response = generate_and_import(
            &store,
            repo.path().to_str().expect("utf8"),
            Utf8Path::from_path(repo_data.path()).expect("utf8"),
            Some("typescript".to_string()),
            Some("typescript".to_string()),
        )
        .expect("response");

        std::env::remove_var("EXTERNAL_INDEX_TYPESCRIPT_COMMAND");
        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "missing_toolchain");
        assert_eq!(response["program"], "__missing_scip_typescript_binary__");
    }
}
