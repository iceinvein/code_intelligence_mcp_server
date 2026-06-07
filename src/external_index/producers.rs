//! External index producer registry and support tiers.

use serde_json::{json, Value};

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
            producer: None,
        },
        LanguageSupport {
            language: "python",
            tier: LanguageTier::FirstClass,
            producer: None,
        },
        LanguageSupport {
            language: "go",
            tier: LanguageTier::FirstClass,
            producer: None,
        },
        LanguageSupport {
            language: "java",
            tier: LanguageTier::BuildAware,
            producer: None,
        },
        LanguageSupport {
            language: "kotlin",
            tier: LanguageTier::BuildAware,
            producer: None,
        },
        LanguageSupport {
            language: "csharp",
            tier: LanguageTier::BuildAware,
            producer: None,
        },
        LanguageSupport {
            language: "swift",
            tier: LanguageTier::BuildAware,
            producer: None,
        },
        LanguageSupport {
            language: "c",
            tier: LanguageTier::CompileDatabase,
            producer: None,
        },
        LanguageSupport {
            language: "cpp",
            tier: LanguageTier::CompileDatabase,
            producer: None,
        },
        LanguageSupport {
            language: "ruby",
            tier: LanguageTier::FallbackOnly,
            producer: None,
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

pub fn generate_and_import(producer: Option<String>, language: Option<String>) -> Value {
    let requested_producer = producer.unwrap_or_else(|| "typescript".to_string());
    let supported_producers = supported_producers();
    if !supported_producers
        .iter()
        .any(|producer| *producer == requested_producer)
    {
        return json!({
            "ok": false,
            "status": "unsupported_producer",
            "producer": requested_producer,
            "language": language,
            "supported_producers": supported_producers,
        });
    }

    json!({
        "ok": false,
        "status": "not_implemented",
        "producer": requested_producer,
        "language": language,
        "supported_producers": supported_producers,
    })
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
    fn default_generation_is_disabled() {
        let cfg = ExternalIndexConfig::default();
        assert!(!cfg.auto_enabled);
        assert_eq!(cfg.on_refresh, ExternalIndexRefreshMode::Disabled);
    }

    #[test]
    fn unknown_generation_reports_supported_producers() {
        let response = generate_and_import(Some("unknown".to_string()), Some("rust".to_string()));
        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "unsupported_producer");
        assert_eq!(response["producer"], "unknown");
        assert_eq!(response["supported_producers"][0], "typescript");
    }

    #[test]
    fn supported_generation_reports_not_implemented_until_adapter_exists() {
        let response = generate_and_import(Some("typescript".to_string()), None);
        assert_eq!(response["ok"], false);
        assert_eq!(response["status"], "not_implemented");
        assert_eq!(response["producer"], "typescript");
    }
}
