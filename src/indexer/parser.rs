use anyhow::Result;
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};

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
    Ruby,
    Kotlin,
    CSharp,
    Swift,
    /// Markdown documentation. Parsed by the hand-written extractor in
    /// `extract/markdown.rs`, not by tree-sitter.
    Markdown,
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
        Some("rb") => Some(LanguageId::Ruby),
        Some("kt") | Some("kts") => Some(LanguageId::Kotlin),
        Some("cs") => Some(LanguageId::CSharp),
        Some("swift") => Some(LanguageId::Swift),
        Some("md") | Some("markdown") => Some(LanguageId::Markdown),
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
        LanguageId::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        LanguageId::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        LanguageId::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        LanguageId::Swift => tree_sitter_swift::LANGUAGE.into(),
        // Markdown never reaches tree-sitter; the extractor is hand-written.
        LanguageId::Markdown => {
            unreachable!("markdown is parsed by extract::markdown, not tree-sitter")
        }
    }
}

/// Deepest nesting the symbol extractors can walk before a parser worker runs
/// out of stack. They recurse at least one frame per level, measured at roughly
/// a kilobyte per level in a debug build, so this limit is a stack budget
/// expressed in AST levels and is paired with `PARSER_STACK_BYTES` in
/// `pipeline::parse`. Raising either alone is what turns a deep file into a
/// daemon-wide abort.
///
/// For scale: across 14,000 TypeScript files in one repository the deepest
/// hand-written file reached 274 levels and generated C headers reached 2,861,
/// while a generated 18,000-member union type of IAM action names reached
/// 18,634. The limit sits above real code and below what no stack can hold.
pub const MAX_AST_DEPTH: usize = 4_000;

/// Deepest nesting in `tree`, counting the root as level 1.
///
/// Walks with a cursor rather than recursion, so measuring a tree that is too
/// deep to walk recursively does not itself overflow the stack.
pub fn ast_depth(tree: &Tree) -> usize {
    let mut cursor = tree.walk();
    let mut depth = 1usize;
    let mut deepest = 1usize;

    loop {
        if cursor.goto_first_child() {
            depth += 1;
            deepest = deepest.max(depth);
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return deepest;
            }
            depth -= 1;
        }
    }
}

/// Refuse a tree the extractors cannot walk, naming the depth so the skip is
/// diagnosable from the log line alone.
pub fn ensure_walkable_depth(tree: &Tree) -> Result<()> {
    let depth = ast_depth(tree);
    if depth > MAX_AST_DEPTH {
        anyhow::bail!(
            "AST nests {depth} levels, past the {MAX_AST_DEPTH}-level limit the symbol extractors can walk"
        );
    }
    Ok(())
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
        let _ = parser_for_id(LanguageId::Ruby).unwrap();
        let _ = parser_for_id(LanguageId::Kotlin).unwrap();
        let _ = parser_for_id(LanguageId::CSharp).unwrap();
        let _ = parser_for_id(LanguageId::Swift).unwrap();
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
        assert_eq!(
            language_id_for_path(Path::new("x.rb")),
            Some(LanguageId::Ruby)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.kt")),
            Some(LanguageId::Kotlin)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.kts")),
            Some(LanguageId::Kotlin)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.cs")),
            Some(LanguageId::CSharp)
        );
        assert_eq!(
            language_id_for_path(Path::new("x.swift")),
            Some(LanguageId::Swift)
        );
    }

    fn typescript_tree(source: &str) -> tree_sitter::Tree {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn measures_the_deepest_nesting_in_a_tree() {
        let shallow = ast_depth(&typescript_tree("const a = 1;\n"));
        let nested = ast_depth(&typescript_tree("const a = [[[[1]]]];\n"));
        assert!(nested > shallow, "shallow={shallow} nested={nested}");
    }

    #[test]
    fn refuses_a_tree_past_the_depth_limit_and_names_it() {
        let mut source = String::from("type Deep =\n");
        for i in 0..(MAX_AST_DEPTH + 50) {
            source.push_str(&format!("  | \"v{i}\"\n"));
        }
        source.push_str("  | \"last\";\n");

        let error = ensure_walkable_depth(&typescript_tree(&source))
            .expect_err("a tree past the limit must be refused");
        assert!(
            error.to_string().contains(&MAX_AST_DEPTH.to_string()),
            "{error}"
        );
    }

    #[test]
    fn accepts_a_tree_within_the_depth_limit() {
        ensure_walkable_depth(&typescript_tree("const a = [[[[1]]]];\n")).unwrap();
    }
}
