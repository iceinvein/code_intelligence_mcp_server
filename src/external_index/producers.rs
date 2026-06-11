//! External index producer registry and support tiers.

use std::io::ErrorKind;
use std::path::Path;
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
        default_program: "code-intelligence-external-typescript",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerCommandSource {
    Override,
    Bundled,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProducerProgram {
    pub program: String,
    pub source: ProducerCommandSource,
}

fn producer_spec_by_id(id: &str) -> Option<ProducerSpec> {
    PRODUCER_SPECS.iter().copied().find(|spec| spec.id == id)
}

fn resolve_producer_program(spec: ProducerSpec) -> Option<ResolvedProducerProgram> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    resolve_producer_program_for_dir(spec, exe_dir.as_deref())
}

fn resolve_producer_program_for_dir(
    spec: ProducerSpec,
    exe_dir: Option<&std::path::Path>,
) -> Option<ResolvedProducerProgram> {
    if let Ok(program) = std::env::var(spec.command_env) {
        if !program.trim().is_empty() {
            return Some(ResolvedProducerProgram {
                program,
                source: ProducerCommandSource::Override,
            });
        }
    }

    if let Some(exe_dir) = exe_dir {
        let bundled = exe_dir.join(spec.default_program);
        if is_executable(&bundled) {
            return Some(ResolvedProducerProgram {
                program: bundled.to_string_lossy().into_owned(),
                source: ProducerCommandSource::Bundled,
            });
        }
    }

    if path_lookup(spec.default_program) {
        return Some(ResolvedProducerProgram {
            program: spec.default_program.to_string(),
            source: ProducerCommandSource::Path,
        });
    }

    None
}

fn path_lookup(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(program))))
        .unwrap_or(false)
}

fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    !metadata.permissions().readonly()
                }
            })
            .unwrap_or(false)
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
        .or_else(|| detect_producer_for_repo(Utf8Path::new(repo_root)).map(str::to_string));
    let Some(requested_producer) = requested_producer else {
        return Ok(json!({
            "ok": false,
            "status": "no_supported_producer_detected",
            "language": language,
            "supported_producers": supported_producers(),
        }));
    };
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
    let resolved_language = language.or_else(|| Some(spec.default_language.to_string()));
    generate_with_spec(store, repo_root, repo_data_dir, resolved_language, *spec)
}

fn producer_for_language(language: Option<&str>) -> Option<&'static str> {
    let language = language?;
    supported_language_tiers()
        .into_iter()
        .find(|support| support.language == language)
        .and_then(|support| support.producer)
}

pub fn detect_producer_for_repo(repo_root: &Utf8Path) -> Option<&'static str> {
    let root = repo_root.as_std_path();
    let manifest_candidates = [
        ("typescript", ["package.json", "tsconfig.json"].as_slice()),
        ("rust", ["Cargo.toml"].as_slice()),
        ("go", ["go.mod"].as_slice()),
        (
            "python",
            ["pyproject.toml", "setup.py", "requirements.txt"].as_slice(),
        ),
        (
            "java",
            ["pom.xml", "build.gradle", "settings.gradle"].as_slice(),
        ),
        ("swift", ["Package.swift"].as_slice()),
        ("ruby", ["Gemfile"].as_slice()),
    ];

    for (producer, files) in manifest_candidates {
        if files.iter().any(|file| root.join(file).exists()) {
            return Some(producer);
        }
    }

    detect_producer_from_files(root)
}

fn detect_producer_from_files(root: &Path) -> Option<&'static str> {
    let mut stack = vec![root.to_path_buf()];
    let mut counts = [0usize; FILE_PRODUCER_PRIORITY.len()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git"
                || name == "target"
                || name == "node_modules"
                || name == "dist"
                || name == "build"
                || name == ".code-intelligence"
            {
                continue;
            }

            if path.is_dir() {
                stack.push(path);
                continue;
            }

            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if let Some(candidate) = producer_for_extension(ext) {
                if let Some(index) = FILE_PRODUCER_PRIORITY
                    .iter()
                    .position(|producer| *producer == candidate)
                {
                    counts[index] += 1;
                }
            }
        }
    }

    FILE_PRODUCER_PRIORITY
        .iter()
        .enumerate()
        .filter(|(index, _)| counts[*index] > 0)
        .max_by_key(|(index, _)| (counts[*index], FILE_PRODUCER_PRIORITY.len() - index))
        .map(|(_, producer)| *producer)
}

const FILE_PRODUCER_PRIORITY: [&str; 11] = [
    "typescript",
    "rust",
    "go",
    "python",
    "java",
    "kotlin",
    "csharp",
    "swift",
    "cpp",
    "c",
    "ruby",
];

fn producer_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "ts" | "tsx" | "js" | "jsx" => Some("typescript"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "py" => Some("python"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "cs" => Some("csharp"),
        "swift" => Some("swift"),
        "cpp" | "cc" | "cxx" | "hpp" => Some("cpp"),
        "c" | "h" => Some("c"),
        "rb" => Some("ruby"),
        _ => None,
    }
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");
        let _env = EnvVarGuard::set(
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
    fn detects_rust_producer_from_manifest_when_no_producer_is_requested() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");
        std::fs::write(
            repo.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("write Cargo.toml");
        let _env = EnvVarGuard::set(
            "EXTERNAL_INDEX_RUST_COMMAND",
            "__missing_detected_rust_external_index__",
        );

        let response = generate_and_import(
            &store,
            repo.path().to_str().expect("utf8"),
            Utf8Path::from_path(repo_data.path()).expect("utf8"),
            None,
            None,
        )
        .expect("response");

        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "missing_toolchain");
        assert_eq!(response["producer"], "rust");
        assert_eq!(response["language"], "rust");
        assert_eq!(
            response["program"],
            "__missing_detected_rust_external_index__"
        );
    }

    #[test]
    fn detects_typescript_producer_from_package_manifest() {
        let repo = tempfile::tempdir().expect("repo");
        std::fs::write(repo.path().join("package.json"), "{}").expect("write package.json");

        let detected =
            detect_producer_for_repo(Utf8Path::from_path(repo.path()).expect("utf8 repo"));

        assert_eq!(detected, Some("typescript"));
    }

    #[test]
    fn reports_no_supported_producer_when_repo_has_no_known_signal() {
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");

        let response = generate_and_import(
            &store,
            repo.path().to_str().expect("utf8"),
            Utf8Path::from_path(repo_data.path()).expect("utf8"),
            None,
            None,
        )
        .expect("response");

        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "no_supported_producer_detected");
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
    fn producer_resolution_prefers_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let _env = EnvVarGuard::set(
            "EXTERNAL_INDEX_RUST_COMMAND",
            "/custom/code-intelligence-external-rust",
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let spec = producer_spec_by_id("rust").expect("rust spec");

        let resolved = resolve_producer_program_for_dir(spec, Some(temp.path())).expect("resolve");

        assert_eq!(resolved.program, "/custom/code-intelligence-external-rust");
        assert_eq!(resolved.source, ProducerCommandSource::Override);
    }

    #[test]
    fn producer_resolution_uses_bundled_executable_before_path() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
        let temp = tempfile::tempdir().expect("tempdir");
        let bundled = temp.path().join("code-intelligence-external-rust");
        std::fs::write(&bundled, "#!/bin/sh\nexit 0\n").expect("write bundled producer");
        make_executable(&bundled);
        let path_dir = tempfile::tempdir().expect("path tempdir");
        let path_program = path_dir.path().join("code-intelligence-external-rust");
        std::fs::write(&path_program, "#!/bin/sh\nexit 0\n").expect("write path producer");
        make_executable(&path_program);
        let _path = EnvVarGuard::set("PATH", path_dir.path().as_os_str());
        let spec = producer_spec_by_id("rust").expect("rust spec");

        let resolved = resolve_producer_program_for_dir(spec, Some(temp.path())).expect("resolve");

        assert_eq!(resolved.program, bundled.to_string_lossy());
        assert_eq!(resolved.source, ProducerCommandSource::Bundled);
    }

    #[test]
    fn producer_resolution_reports_missing_when_not_overridden_or_bundled() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
        let temp = tempfile::tempdir().expect("tempdir");
        let path_dir = tempfile::tempdir().expect("path tempdir");
        let _path = EnvVarGuard::set("PATH", path_dir.path().as_os_str());
        let spec = producer_spec_by_id("rust").expect("rust spec");

        let resolved = resolve_producer_program_for_dir(spec, Some(temp.path()));

        assert!(resolved.is_none());
    }

    #[test]
    fn producer_resolution_falls_back_to_path_when_no_bundle_exists() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
        let exe_dir = tempfile::tempdir().expect("exe tempdir");
        let path_dir = tempfile::tempdir().expect("path tempdir");
        let path_program = path_dir.path().join("code-intelligence-external-rust");
        std::fs::write(&path_program, "#!/bin/sh\nexit 0\n").expect("write path producer");
        make_executable(&path_program);
        let _path = EnvVarGuard::set("PATH", path_dir.path().as_os_str());
        let spec = producer_spec_by_id("rust").expect("rust spec");

        let resolved = resolve_producer_program_for_dir(spec, Some(exe_dir.path())).expect("resolve");

        assert_eq!(resolved.program, "code-intelligence-external-rust");
        assert_eq!(resolved.source, ProducerCommandSource::Path);
    }

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }

    #[test]
    fn generate_typescript_reports_missing_toolchain_cleanly() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let store = SqliteStore::open_in_memory().expect("sqlite");
        store.init().expect("init");
        let repo = tempfile::tempdir().expect("repo");
        let repo_data = tempfile::tempdir().expect("repo data");
        let _env = EnvVarGuard::set(
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

        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "missing_toolchain");
        assert_eq!(response["program"], "__missing_scip_typescript_binary__");
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
