use anyhow::Result;
use std::path::Path;
use tree_sitter::{Language, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Typescript,
    Tsx,
    Rust,
}

pub fn language_id_for_path(path: &Path) -> Option<LanguageId> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("ts") => Some(LanguageId::Typescript),
        Some("tsx") => Some(LanguageId::Tsx),
        Some("rs") => Some(LanguageId::Rust),
        _ => None,
    }
}

pub fn language_for_id(id: LanguageId) -> Language {
    match id {
        LanguageId::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        LanguageId::Rust => tree_sitter_rust::LANGUAGE.into(),
    }
}

pub fn parser_for_id(id: LanguageId) -> Result<Parser> {
    let mut parser = Parser::new();
    parser.set_language(&language_for_id(id))?;
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_parsers_for_languages() {
        let _ = parser_for_id(LanguageId::Typescript).unwrap();
        let _ = parser_for_id(LanguageId::Tsx).unwrap();
        let _ = parser_for_id(LanguageId::Rust).unwrap();
    }

    #[test]
    fn detects_language_ids_by_extension() {
        assert_eq!(
            language_id_for_path(Path::new("x.ts")),
            Some(LanguageId::Typescript)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.tsx")),
            Some(LanguageId::Tsx)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.rs")),
            Some(LanguageId::Rust)
        );
        assert_eq!(language_id_for_path(Path::new("x.py")), None);
    }
}
