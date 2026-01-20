use anyhow::Result;
use std::path::Path;
use tree_sitter::{Language, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Typescript,
    Tsx,
    Rust,
    Python,
    Go,
    Java,
    Javascript,
    C,
    Cpp,
}

pub fn language_id_for_path(path: &Path) -> Option<LanguageId> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("ts") => Some(LanguageId::Typescript),
        Some("tsx") => Some(LanguageId::Tsx),
        Some("rs") => Some(LanguageId::Rust),
        Some("py") => Some(LanguageId::Python),
        Some("go") => Some(LanguageId::Go),
        Some("java") => Some(LanguageId::Java),
        Some("js") | Some("jsx") => Some(LanguageId::Javascript),
        Some("c") | Some("h") => Some(LanguageId::C),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => Some(LanguageId::Cpp),
        _ => None,
    }
}

pub fn language_for_id(id: LanguageId) -> Language {
    match id {
        LanguageId::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        LanguageId::Rust => tree_sitter_rust::LANGUAGE.into(),
        LanguageId::Python => tree_sitter_python::LANGUAGE.into(),
        LanguageId::Go => tree_sitter_go::LANGUAGE.into(),
        LanguageId::Java => tree_sitter_java::LANGUAGE.into(),
        LanguageId::Javascript => tree_sitter_javascript::LANGUAGE.into(),
        LanguageId::C => tree_sitter_c::LANGUAGE.into(),
        LanguageId::Cpp => tree_sitter_cpp::LANGUAGE.into(),
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
        let _ = parser_for_id(LanguageId::Python).unwrap();
        let _ = parser_for_id(LanguageId::Go).unwrap();
        let _ = parser_for_id(LanguageId::Java).unwrap();
        let _ = parser_for_id(LanguageId::Javascript).unwrap();
        let _ = parser_for_id(LanguageId::C).unwrap();
        let _ = parser_for_id(LanguageId::Cpp).unwrap();
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
        assert_eq!(
            language_id_for_path(Path::new("x.py")),
            Some(LanguageId::Python)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.go")),
            Some(LanguageId::Go)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.java")),
            Some(LanguageId::Java)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.js")),
            Some(LanguageId::Javascript)
        );
        assert_eq!(language_id_for_path(Path::new("x.c")), Some(LanguageId::C));
        assert_eq!(
            language_id_for_path(Path::new("x.cpp")),
            Some(LanguageId::Cpp)
        );
        assert_eq!(language_id_for_path(Path::new("x.rb")), None);
    }
}
