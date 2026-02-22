# Tier 2: Deep Rust + Go Extraction — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring Rust and Go extractors to near-TypeScript richness: imports, type edges, data flow, inheritance, and richer symbol kinds.

**Architecture:** Extend tree-sitter extractors (`src/indexer/extract/rust.rs` and `go.rs`) to populate the existing `ExtractedFile` vectors (`imports`, `type_edges`, `dataflow_edges`). No pipeline or storage changes needed — `src/indexer/pipeline/edges.rs` already processes all three.

**Tech Stack:** Rust, tree-sitter (tree-sitter-rust, tree-sitter-go)

---

## Context for the Implementer

### How the Pipeline Works

1. Extractors produce `ExtractedFile { symbols, imports, type_edges, dataflow_edges, ... }`
2. `src/indexer/pipeline/parse.rs:253-264` passes these to `extract_edges_for_symbol()`
3. `src/indexer/pipeline/edges.rs:446-536` processes `type_edges` and `dataflow_edges`:
   - Type edges: resolves type names to symbol IDs via `name_to_id` map or `import_map`, creates `EdgeRow` with `edge_type: "type"`
   - Data flow: resolves `from_symbol` to symbol ID, creates `EdgeRow` with `edge_type: "reads"` or `"writes"`
   - Imports: `build_import_map()` creates a lookup from imported name → Import struct

### Key Types (in `src/indexer/extract/symbol.rs`)

```rust
pub struct Import { pub name: String, pub source: String, pub alias: Option<String> }
pub struct DataFlowEdge { pub from_symbol: String, pub to_symbol: String, pub flow_type: DataFlowType, pub at_line: u32 }
pub enum DataFlowType { Reads, Writes }
// type_edges: Vec<(String, String)> — (parent_symbol_name, type_name)
```

### Existing Rust Extractor (`src/indexer/extract/rust.rs`, 391 lines)

Already extracts: Function, Struct, Enum, Trait, Impl, Module symbols. Has type edges for function params/returns and struct fields. Missing: imports, data flow, const/static/type, method prefixing, impl edges.

### Existing Go Extractor (`src/indexer/extract/go.rs`, 277 lines)

Already extracts: Function (incl methods), Struct, Interface, TypeAlias symbols + imports. Missing: type edges, method receiver linkage, interface methods, struct embedding.

### Running Tests

```bash
EMBEDDINGS_BACKEND=hash cargo test -- rust::tests  # Rust extractor tests only
EMBEDDINGS_BACKEND=hash cargo test -- go::tests     # Go extractor tests only
EMBEDDINGS_BACKEND=hash cargo test                  # All tests
```

---

## Task 1: Rust — Extract `use` Imports

**Files:**
- Modify: `src/indexer/extract/rust.rs`

**Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `rust.rs`:

```rust
#[test]
fn extracts_rust_use_imports() {
    let source = r#"
use std::collections::HashMap;
use crate::path::PathNormalizer;
use super::symbol::{ExtractedFile, Import};
use anyhow::Result;
use std::io::{Read, Write};
use foo as bar;
"#;
    let extracted = extract_rust_symbols(source).unwrap();

    assert!(extracted.imports.iter().any(|i| i.name == "HashMap" && i.source == "std::collections::HashMap"));
    assert!(extracted.imports.iter().any(|i| i.name == "PathNormalizer" && i.source == "crate::path::PathNormalizer"));
    assert!(extracted.imports.iter().any(|i| i.name == "ExtractedFile" && i.source == "super::symbol"));
    assert!(extracted.imports.iter().any(|i| i.name == "Import" && i.source == "super::symbol"));
    assert!(extracted.imports.iter().any(|i| i.name == "Result" && i.source == "anyhow::Result"));
    assert!(extracted.imports.iter().any(|i| i.name == "Read" && i.source == "std::io"));
    assert!(extracted.imports.iter().any(|i| i.name == "Write" && i.source == "std::io"));
    assert!(extracted.imports.iter().any(|i| i.name == "foo" && i.alias == Some("bar".to_string())));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_use_imports`
Expected: FAIL — imports vec is empty

**Step 3: Implement import extraction**

In `rust.rs`, add the `Import` import at the top:
```rust
use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};
```

Add a new match arm in the `walk` callback inside `extract_symbols_with_parser`:
```rust
"use_declaration" => {
    extract_use_imports(node, source, &mut imports);
}
```

Add `let mut imports = Vec::new();` alongside the existing `let mut symbols` and `let mut type_edges`.

Update the `ExtractedFile` return to use `imports` instead of `Vec::new()`.

Implement the function:
```rust
fn extract_use_imports(node: Node<'_>, source: &str, imports: &mut Vec<Import>) {
    // Walk the use_declaration tree to find use_list, scoped_use_list, use_as_clause, etc.
    extract_use_tree(node, source, "", imports);
}

fn extract_use_tree(node: Node<'_>, source: &str, prefix: &str, imports: &mut Vec<Import>) {
    match node.kind() {
        "use_declaration" => {
            // Has an "argument" child which is the use tree
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "use" && child.kind() != ";" && child.kind() != "pub" {
                    extract_use_tree(child, source, prefix, imports);
                }
            }
        }
        "scoped_identifier" => {
            // e.g., std::collections::HashMap — this is a single import
            let full_path = text_for_node(node, source);
            let name = full_path.rsplit("::").next().unwrap_or(&full_path).to_string();
            imports.push(Import {
                name,
                source: full_path,
                alias: None,
            });
        }
        "use_as_clause" => {
            // e.g., foo as bar
            let path_node = node.child_by_field_name("path");
            let alias_node = node.child_by_field_name("alias");
            if let (Some(path), Some(alias)) = (path_node, alias_node) {
                let path_text = text_for_node(path, source);
                let full_source = if prefix.is_empty() { path_text.clone() } else { format!("{prefix}::{path_text}") };
                let name = path_text.rsplit("::").next().unwrap_or(&path_text).to_string();
                imports.push(Import {
                    name,
                    source: full_source,
                    alias: Some(text_for_node(alias, source)),
                });
            }
        }
        "use_list" => {
            // e.g., {Read, Write} inside use std::io::{Read, Write}
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "{" && child.kind() != "}" && child.kind() != "," {
                    extract_use_tree(child, source, prefix, imports);
                }
            }
        }
        "scoped_use_list" => {
            // e.g., std::io::{Read, Write}
            // Has a "path" and a "list" child
            let path_node = node.child_by_field_name("path");
            let list_node = node.child_by_field_name("list");
            let new_prefix = if let Some(p) = path_node {
                let p_text = text_for_node(p, source);
                if prefix.is_empty() { p_text } else { format!("{prefix}::{p_text}") }
            } else {
                prefix.to_string()
            };
            if let Some(list) = list_node {
                extract_use_tree(list, source, &new_prefix, imports);
            }
        }
        "identifier" => {
            // Simple identifier, e.g., a single name in a use list
            let name = text_for_node(node, source);
            let full_source = if prefix.is_empty() { name.clone() } else { format!("{prefix}::{name}") };
            imports.push(Import {
                name,
                source: full_source,
                alias: None,
            });
        }
        "use_wildcard" => {
            // use foo::*
            imports.push(Import {
                name: "*".to_string(),
                source: prefix.to_string(),
                alias: None,
            });
        }
        _ => {
            // Recurse for any other node types
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_use_tree(child, source, prefix, imports);
            }
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_use_imports`
Expected: PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/rust.rs
git commit -m "feat: extract use imports from Rust source files"
```

---

## Task 2: Rust — Extract `const`, `static`, `type` Items

**Files:**
- Modify: `src/indexer/extract/rust.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_rust_const_static_type() {
    let source = r#"
pub const MAX_SIZE: usize = 100;
static INSTANCE: Mutex<Config> = Mutex::new(Config::default());
pub type Result<T> = std::result::Result<T, Error>;
"#;
    let extracted = extract_rust_symbols(source).unwrap();

    let max_size = extracted.symbols.iter().find(|s| s.name == "MAX_SIZE").unwrap();
    assert_eq!(max_size.kind, SymbolKind::Const);
    assert!(max_size.exported);

    let instance = extracted.symbols.iter().find(|s| s.name == "INSTANCE").unwrap();
    assert_eq!(instance.kind, SymbolKind::Const);
    assert!(!instance.exported);

    let result = extracted.symbols.iter().find(|s| s.name == "Result").unwrap();
    assert_eq!(result.kind, SymbolKind::TypeAlias);
    assert!(result.exported);

    // Type edges from const/static/type
    assert!(extracted.type_edges.iter().any(|e| e.0 == "MAX_SIZE" && e.1 == "usize"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "INSTANCE" && e.1 == "Mutex"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "INSTANCE" && e.1 == "Config"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_const_static_type`
Expected: FAIL

**Step 3: Implement extraction**

Add three new match arms in the `walk` callback:

```rust
"const_item" => {
    if let Some(name) = symbol_name_from_declaration(node, source) {
        symbols.push(symbol_from_node(
            name.clone(),
            SymbolKind::Const,
            is_public(node, source),
            node,
        ));
        if let Some(type_node) = node.child_by_field_name("type") {
            extract_type_ref(type_node, source, &name, &mut type_edges);
        }
    }
}
"static_item" => {
    if let Some(name) = symbol_name_from_declaration(node, source) {
        symbols.push(symbol_from_node(
            name.clone(),
            SymbolKind::Const,
            is_public(node, source),
            node,
        ));
        if let Some(type_node) = node.child_by_field_name("type") {
            extract_type_ref(type_node, source, &name, &mut type_edges);
        }
    }
}
"type_item" => {
    if let Some(name) = symbol_name_from_declaration(node, source) {
        symbols.push(symbol_from_node(
            name.clone(),
            SymbolKind::TypeAlias,
            is_public(node, source),
            node,
        ));
        // Extract the target type
        if let Some(type_node) = node.child_by_field_name("type") {
            extract_type_ref(type_node, source, &name, &mut type_edges);
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_const_static_type`
Expected: PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/rust.rs
git commit -m "feat: extract const, static, and type alias items from Rust"
```

---

## Task 3: Rust — Impl Type Edges + Method Prefixing

**Files:**
- Modify: `src/indexer/extract/rust.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_rust_impl_edges_and_method_prefix() {
    let source = r#"
pub struct Foo { x: i32 }

pub trait Display {
    fn fmt(&self) -> String;
}

impl Display for Foo {
    fn fmt(&self) -> String {
        format!("{}", self.x)
    }
}

impl Foo {
    pub fn new(x: i32) -> Self {
        Self { x }
    }

    pub fn value(&self) -> i32 {
        self.x
    }
}
"#;
    let extracted = extract_rust_symbols(source).unwrap();

    // impl Display for Foo should have type edges to both Display and Foo
    let impl_display = extracted.symbols.iter().find(|s| s.name.contains("impl Display for Foo")).unwrap();
    assert_eq!(impl_display.kind, SymbolKind::Impl);
    assert!(extracted.type_edges.iter().any(|e| e.0 == impl_display.name && e.1 == "Display"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == impl_display.name && e.1 == "Foo"));

    // Methods inside impl should be prefixed with the type name
    assert!(extracted.symbols.iter().any(|s| s.name == "Foo::new" && s.kind == SymbolKind::Function));
    assert!(extracted.symbols.iter().any(|s| s.name == "Foo::value" && s.kind == SymbolKind::Function));
    assert!(extracted.symbols.iter().any(|s| s.name == "Foo::fmt" && s.kind == SymbolKind::Function));

    // Method type edges should use prefixed name
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Foo::new" && e.1 == "Self"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_impl_edges_and_method_prefix`
Expected: FAIL — methods named "new"/"value"/"fmt" without prefix, no impl type edges

**Step 3: Implement**

This requires a refactor of the walk approach. Currently `walk` visits all nodes via a flat callback. We need to track "current impl type" context when descending into `impl_item` children.

Replace the flat `walk` with a two-pass or context-aware approach:

1. First, change the `walk` callback to skip `function_item` nodes that are children of `impl_item` (they'll be handled by the impl handler).
2. In the `impl_item` arm, walk its `declaration_list` children to find `function_item` nodes, prefixing their names.

```rust
"impl_item" => {
    let display_name = impl_display_name(node, source);
    symbols.push(symbol_from_node(
        display_name.clone(),
        SymbolKind::Impl,
        is_public(node, source),
        node,
    ));

    // Emit type edges for impl — connect to trait and type
    let type_name = node
        .child_by_field_name("type")
        .map(|n| text_for_node(n, source));
    let trait_name = node
        .child_by_field_name("trait")
        .map(|n| text_for_node(n, source));

    if let Some(ref tn) = type_name {
        type_edges.push((display_name.clone(), tn.clone()));
    }
    if let Some(ref tr) = trait_name {
        type_edges.push((display_name.clone(), tr.clone()));
    }

    // Extract methods with type-prefixed names
    let prefix = type_name.as_deref().unwrap_or("unknown");
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        for child in body.children(&mut body_cursor) {
            if child.kind() == "function_item" {
                if let Some(method_name) = symbol_name_from_declaration(child, source) {
                    let prefixed = format!("{prefix}::{method_name}");
                    symbols.push(symbol_from_node(
                        prefixed.clone(),
                        SymbolKind::Function,
                        is_public(child, source),
                        child,
                    ));
                    extract_function_signature_types(child, source, &prefixed, &mut type_edges);
                }
            }
        }
    }
}
```

Also need to prevent the walk from re-extracting `function_item` nodes inside impl blocks. Since `walk` visits ALL descendants, the existing `"function_item"` arm will fire for methods too. Fix: check if the function's parent is a `declaration_list` inside an `impl_item`, and skip if so.

Add to the `"function_item"` arm:
```rust
"function_item" => {
    // Skip methods inside impl blocks — handled by impl_item arm
    if node.parent().map(|p| p.kind()) == Some("declaration_list") {
        // parent of declaration_list should be impl_item
        if node.parent().and_then(|p| p.parent()).map(|gp| gp.kind()) == Some("impl_item") {
            return; // or continue in the closure context
        }
    }
    // ... existing extraction code
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_impl_edges`
Expected: PASS

Also run existing tests to make sure nothing regressed:
Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests`
Expected: ALL PASS (the `extracts_rust_items_with_spans` test may need updating since `new` is now `Foo::new`)

**Step 5: Update existing test if needed**

The `extracts_rust_items_with_spans` test looks for `s.kind == SymbolKind::Function && s.name == "top"` and a few others. The `impl Foo { pub fn new() }` will now produce `Foo::new` instead of `new`. Update the assertion:

```rust
// Was: s.name == "new" — now prefixed
// The test checks for "top" (free function) which should be unchanged
```

**Step 6: Commit**

```bash
git add src/indexer/extract/rust.rs
git commit -m "feat: add impl type edges and method prefixing in Rust extractor"
```

---

## Task 4: Rust — Trait Method Signature Types

**Files:**
- Modify: `src/indexer/extract/rust.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_rust_trait_method_types() {
    let source = r#"
pub trait Processor {
    fn process(&self, input: Input) -> Output;
    fn validate(&self, data: &Data) -> Result<(), Error>;
}
"#;
    let extracted = extract_rust_symbols(source).unwrap();

    // Trait should exist
    assert!(extracted.symbols.iter().any(|s| s.name == "Processor" && s.kind == SymbolKind::Trait));

    // Type edges from trait method signatures
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Processor::process" && e.1 == "Input"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Processor::process" && e.1 == "Output"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Processor::validate" && e.1 == "Data"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Processor::validate" && e.1 == "Result"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Processor::validate" && e.1 == "Error"));

    // Trait methods should be extracted as symbols
    assert!(extracted.symbols.iter().any(|s| s.name == "Processor::process" && s.kind == SymbolKind::Function));
    assert!(extracted.symbols.iter().any(|s| s.name == "Processor::validate" && s.kind == SymbolKind::Function));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_trait_method_types`
Expected: FAIL

**Step 3: Implement**

In the `"trait_item"` match arm, after extracting the trait symbol, walk its body to find method signatures:

```rust
"trait_item" => {
    if let Some(name) = symbol_name_from_declaration(node, source) {
        symbols.push(symbol_from_node(
            name.clone(),
            SymbolKind::Trait,
            is_public(node, source),
            node,
        ));

        // Extract method signatures from trait body
        if let Some(body) = node.child_by_field_name("body") {
            let mut body_cursor = body.walk();
            for child in body.children(&mut body_cursor) {
                // function_signature_item is a trait method declaration (no body)
                // function_item is a default method (has body)
                if child.kind() == "function_signature_item" || child.kind() == "function_item" {
                    if let Some(method_name) = symbol_name_from_declaration(child, source) {
                        let prefixed = format!("{name}::{method_name}");
                        symbols.push(symbol_from_node(
                            prefixed.clone(),
                            SymbolKind::Function,
                            is_public(child, source),
                            child,
                        ));
                        extract_function_signature_types(child, source, &prefixed, &mut type_edges);
                    }
                }
            }
        }
    }
}
```

Also update the `"function_item"` arm to skip functions inside trait bodies (similar to impl skip):
```rust
// Also skip methods inside trait_item — handled by trait_item arm
if node.parent().map(|p| p.kind()) == Some("declaration_list") {
    let gp_kind = node.parent().and_then(|p| p.parent()).map(|gp| gp.kind());
    if gp_kind == Some("impl_item") || gp_kind == Some("trait_item") {
        return;
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_trait_method_types`
Expected: PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/rust.rs
git commit -m "feat: extract trait method signatures and type edges in Rust"
```

---

## Task 5: Rust — Data Flow Edges (reads/writes)

**Files:**
- Modify: `src/indexer/extract/rust.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_rust_dataflow_edges() {
    let source = r#"
fn process(data: Vec<u8>) -> String {
    let result = transform(data);
    let count = result.len();
    output = format_output(result, count);
    output
}
"#;
    let extracted = extract_rust_symbols(source).unwrap();

    let has_read = |sym: &str| extracted.dataflow_edges.iter().any(|e| {
        e.from_symbol == sym && matches!(e.flow_type, DataFlowType::Reads)
    });
    let has_write = |sym: &str| extracted.dataflow_edges.iter().any(|e| {
        e.from_symbol == sym && matches!(e.flow_type, DataFlowType::Writes)
    });

    // let result = transform(data) → writes result, reads transform, reads data
    assert!(has_write("result"));
    assert!(has_read("transform"));
    assert!(has_read("data"));

    // let count = result.len() → writes count, reads result
    assert!(has_write("count"));

    // output = format_output(result, count) → writes output, reads format_output
    assert!(has_write("output"));
    assert!(has_read("format_output"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_dataflow_edges`
Expected: FAIL — dataflow_edges is empty

**Step 3: Implement data flow extraction**

Add the `DataFlowEdge` and `DataFlowType` imports at the top of `rust.rs`:
```rust
use super::symbol::{ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};
```

Add `let mut dataflow_edges = Vec::new();` in `extract_symbols_with_parser`.

In the `"function_item"` arm (for free functions — impl/trait methods handled in their respective arms), add:
```rust
extract_rust_dataflow(node, source, &name, &mut dataflow_edges);
```

Similarly in the `"impl_item"` arm, when extracting methods, add the dataflow call with the prefixed name:
```rust
extract_rust_dataflow(child, source, &prefixed, &mut dataflow_edges);
```

Same for `"trait_item"` methods that have bodies (`function_item` kind).

Update the `ExtractedFile` return to use `dataflow_edges` instead of `Vec::new()`.

Implement the data flow functions:

```rust
fn extract_rust_dataflow(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let body = match node.child_by_field_name("body") {
        Some(b) if b.kind() == "block" => b,
        _ => return,
    };

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        extract_rust_dataflow_from_node(child, source, context_name, out);
    }
}

fn extract_rust_dataflow_from_node(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    match node.kind() {
        "let_declaration" => {
            let line = node.start_position().row as u32;
            // let x = expr;
            if let Some(pattern) = node.child_by_field_name("pattern") {
                let name = text_for_node(pattern, source);
                out.push(DataFlowEdge {
                    from_symbol: name,
                    to_symbol: context_name.to_string(),
                    flow_type: DataFlowType::Writes,
                    at_line: line,
                });
            }
            if let Some(value) = node.child_by_field_name("value") {
                extract_rust_reads_from_expr(value, source, context_name, out);
            }
        }
        "assignment_expression" => {
            let line = node.start_position().row as u32;
            if let Some(left) = node.child_by_field_name("left") {
                let name = extract_rust_lhs_identifier(left, source);
                if let Some(n) = name {
                    out.push(DataFlowEdge {
                        from_symbol: n,
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Writes,
                        at_line: line,
                    });
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                extract_rust_reads_from_expr(right, source, context_name, out);
            }
        }
        "call_expression" => {
            let line = node.start_position().row as u32;
            if let Some(func) = node.child_by_field_name("function") {
                let name = extract_rust_callee_name(func, source);
                if let Some(n) = name {
                    out.push(DataFlowEdge {
                        from_symbol: n,
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                    });
                }
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                let mut cursor = args.walk();
                for child in args.children(&mut cursor) {
                    extract_rust_reads_from_expr(child, source, context_name, out);
                }
            }
        }
        "expression_statement" | "block" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_rust_dataflow_from_node(child, source, context_name, out);
            }
        }
        _ => {
            // Recurse into children for nested expressions
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    extract_rust_dataflow_from_node(cursor.node(), source, context_name, out);
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }
}

fn extract_rust_reads_from_expr(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32;
    match node.kind() {
        "identifier" => {
            let name = text_for_node(node, source);
            // Skip keywords and common tokens
            if !matches!(name.as_str(), "self" | "Self" | "true" | "false" | "None" | "Some" | "Ok" | "Err") {
                out.push(DataFlowEdge {
                    from_symbol: name,
                    to_symbol: context_name.to_string(),
                    flow_type: DataFlowType::Reads,
                    at_line: line,
                });
            }
        }
        "call_expression" => {
            // Recurse — the call_expression handler in the parent will handle this
            extract_rust_dataflow_from_node(node, source, context_name, out);
        }
        "field_expression" => {
            // obj.field — read the object
            if let Some(obj) = node.child_by_field_name("value") {
                if obj.kind() == "identifier" {
                    let name = text_for_node(obj, source);
                    if name != "self" {
                        out.push(DataFlowEdge {
                            from_symbol: name,
                            to_symbol: context_name.to_string(),
                            flow_type: DataFlowType::Reads,
                            at_line: line,
                        });
                    }
                }
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_rust_reads_from_expr(child, source, context_name, out);
            }
        }
    }
}

fn extract_rust_lhs_identifier(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "field_expression" => {
            // self.field or obj.field — use field name
            node.child_by_field_name("field")
                .map(|f| text_for_node(f, source))
        }
        _ => None,
    }
}

fn extract_rust_callee_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "field_expression" => {
            // obj.method() — return method name
            node.child_by_field_name("field")
                .map(|f| text_for_node(f, source))
        }
        "scoped_identifier" => {
            // Type::method() — return full path
            Some(text_for_node(node, source))
        }
        _ => None,
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests::extracts_rust_dataflow_edges`
Expected: PASS

**Step 5: Run all Rust tests**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- rust::tests`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add src/indexer/extract/rust.rs
git commit -m "feat: extract data flow edges (reads/writes) from Rust function bodies"
```

---

## Task 6: Go — Type Edges for Functions and Structs

**Files:**
- Modify: `src/indexer/extract/go.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_go_type_edges() {
    let source = r#"
package main

type User struct {
    Name    string
    Age     int
    Address *Address
}

func Process(u User, count int) (string, error) {
    return "", nil
}
"#;
    let extracted = extract_go_symbols(source).unwrap();

    let has_edge = |parent: &str, ty: &str| {
        extracted.type_edges.iter().any(|e| e.0 == parent && e.1 == ty)
    };

    // Struct field types
    assert!(has_edge("User", "Address"));

    // Function param and return types
    assert!(has_edge("Process", "User"));
    assert!(has_edge("Process", "error"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_type_edges`
Expected: FAIL — type_edges is empty

**Step 3: Implement**

Add `let mut type_edges = Vec::new();` in `extract_symbols_with_parser`.

Update the `"function_declaration"` arm:
```rust
"function_declaration" => {
    if let Some(name) = symbol_name(node, source) {
        let exported = is_exported(&name);
        symbols.push(symbol_from_node(name.clone(), SymbolKind::Function, exported, node));
        extract_go_function_types(node, source, &name, &mut type_edges);
    }
}
```

Update the `"type_spec"` arm to extract struct field types:
```rust
"type_spec" => {
    if let Some(name) = symbol_name(node, source) {
        let exported = is_exported(&name);
        let type_node = node.child_by_field_name("type");
        let kind = if type_node.map(|n| n.kind()) == Some("struct_type") {
            SymbolKind::Struct
        } else if type_node.map(|n| n.kind()) == Some("interface_type") {
            SymbolKind::Interface
        } else {
            SymbolKind::TypeAlias
        };
        symbols.push(symbol_from_node(name.clone(), kind, exported, node));

        // Extract struct field types
        if let Some(tn) = type_node {
            if tn.kind() == "struct_type" {
                extract_go_struct_field_types(tn, source, &name, &mut type_edges);
            }
        }
    }
}
```

Update `ExtractedFile` return to use `type_edges`.

Implement helper functions:

```rust
fn extract_go_function_types(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    // Parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                if let Some(type_node) = child.child_by_field_name("type") {
                    extract_go_type_ref(type_node, source, parent_name, out);
                }
            }
        }
    }

    // Return type(s)
    if let Some(result) = node.child_by_field_name("result") {
        extract_go_type_ref(result, source, parent_name, out);
    }
}

fn extract_go_struct_field_types(
    struct_node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    // struct_type -> field_declaration_list -> field_declaration
    if let Some(field_list) = struct_node.child_by_field_name("body") {
        // fallback: iterate children
    }
    let mut cursor = struct_node.walk();
    for child in struct_node.children(&mut cursor) {
        if child.kind() == "field_declaration_list" {
            let mut f_cursor = child.walk();
            for field in child.children(&mut f_cursor) {
                if field.kind() == "field_declaration" {
                    if let Some(type_node) = field.child_by_field_name("type") {
                        extract_go_type_ref(type_node, source, parent_name, out);
                    }
                }
            }
        }
    }
}

fn extract_go_type_ref(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    match node.kind() {
        "type_identifier" => {
            let name = text_for_node(node, source);
            // Skip built-in types that aren't useful for graph edges
            if !matches!(name.as_str(), "string" | "int" | "bool" | "byte" | "rune"
                | "float32" | "float64" | "int8" | "int16" | "int32" | "int64"
                | "uint" | "uint8" | "uint16" | "uint32" | "uint64") {
                out.push((parent_name.to_string(), name));
            }
        }
        "pointer_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "*" {
                    extract_go_type_ref(child, source, parent_name, out);
                }
            }
        }
        "slice_type" | "array_type" | "map_type" | "channel_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_go_type_ref(child, source, parent_name, out);
            }
        }
        "qualified_type" => {
            // pkg.Type — extract the type name
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push((parent_name.to_string(), text_for_node(name_node, source)));
            }
        }
        "parameter_list" => {
            // Return type can be a parameter list: (string, error)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "parameter_declaration" {
                    if let Some(type_node) = child.child_by_field_name("type") {
                        extract_go_type_ref(type_node, source, parent_name, out);
                    }
                } else {
                    // Simple type in result list
                    extract_go_type_ref(child, source, parent_name, out);
                }
            }
        }
        _ => {}
    }
}

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}
```

Note: `go.rs` currently uses `n.utf8_text(source.as_bytes()).unwrap().to_string()` inline. Consider adding the `text_for_node` helper to DRY this up.

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_type_edges`
Expected: PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/go.rs
git commit -m "feat: extract type edges for Go functions and struct fields"
```

---

## Task 7: Go — Method Receiver Linkage

**Files:**
- Modify: `src/indexer/extract/go.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_go_method_receiver_linkage() {
    let source = r#"
package main

type Server struct {
    Port int
}

func (s *Server) Start() error {
    return nil
}

func (s Server) GetPort() int {
    return s.Port
}
"#;
    let extracted = extract_go_symbols(source).unwrap();

    // Methods should be prefixed with receiver type
    assert!(extracted.symbols.iter().any(|s| s.name == "Server.Start" && s.kind == SymbolKind::Function));
    assert!(extracted.symbols.iter().any(|s| s.name == "Server.GetPort" && s.kind == SymbolKind::Function));

    // Type edge from method to receiver type
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Server.Start" && e.1 == "Server"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Server.GetPort" && e.1 == "Server"));

    // Return type edge
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Server.Start" && e.1 == "error"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_method_receiver_linkage`
Expected: FAIL

**Step 3: Implement**

Update the `"method_declaration"` arm:

```rust
"method_declaration" => {
    if let Some(name) = symbol_name(node, source) {
        // Extract receiver type for prefixing
        let receiver_type = extract_go_receiver_type(node, source);
        let prefixed_name = if let Some(ref rt) = receiver_type {
            format!("{rt}.{name}")
        } else {
            name.clone()
        };
        let exported = is_exported(&name);
        symbols.push(symbol_from_node(prefixed_name.clone(), SymbolKind::Function, exported, node));

        // Type edge from method to receiver type
        if let Some(ref rt) = receiver_type {
            type_edges.push((prefixed_name.clone(), rt.clone()));
        }

        // Extract parameter and return types
        extract_go_function_types(node, source, &prefixed_name, &mut type_edges);
    }
}
```

Implement the receiver type extractor:

```rust
fn extract_go_receiver_type(node: Node<'_>, source: &str) -> Option<String> {
    // method_declaration has a "receiver" field containing parameter_list
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    for child in receiver.children(&mut cursor) {
        if child.kind() == "parameter_declaration" {
            if let Some(type_node) = child.child_by_field_name("type") {
                return Some(extract_go_base_type_name(type_node, source));
            }
        }
    }
    None
}

fn extract_go_base_type_name(node: Node<'_>, source: &str) -> String {
    match node.kind() {
        "type_identifier" => text_for_node(node, source),
        "pointer_type" => {
            // *Type → Type
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "*" {
                    return extract_go_base_type_name(child, source);
                }
            }
            text_for_node(node, source)
        }
        _ => text_for_node(node, source),
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_method_receiver_linkage`
Expected: PASS

**Step 5: Update existing tests**

The `test_extract_go_struct_method` test checks for `s.name == "Greet"` — this will now be `"GoGreeter.Greet"`. Update the assertion.

**Step 6: Commit**

```bash
git add src/indexer/extract/go.rs
git commit -m "feat: link Go methods to receiver types with prefixed names"
```

---

## Task 8: Go — Interface Method Extraction + Struct Embedding

**Files:**
- Modify: `src/indexer/extract/go.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extracts_go_interface_methods_and_embedding() {
    let source = r#"
package main

type Reader interface {
    Read(p []byte) (int, error)
}

type Writer interface {
    Write(p []byte) (int, error)
}

type ReadWriter interface {
    Reader
    Writer
}

type BufferedReader struct {
    Reader
    bufSize int
}
"#;
    let extracted = extract_go_symbols(source).unwrap();

    // Interface methods should be extracted
    assert!(extracted.symbols.iter().any(|s| s.name == "Reader.Read" && s.kind == SymbolKind::Function));
    assert!(extracted.symbols.iter().any(|s| s.name == "Writer.Write" && s.kind == SymbolKind::Function));

    // Type edges from interface methods
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Reader.Read" && e.1 == "error"));

    // Embedded interfaces — ReadWriter embeds Reader and Writer
    assert!(extracted.type_edges.iter().any(|e| e.0 == "ReadWriter" && e.1 == "Reader"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "ReadWriter" && e.1 == "Writer"));

    // Struct embedding — BufferedReader embeds Reader
    assert!(extracted.type_edges.iter().any(|e| e.0 == "BufferedReader" && e.1 == "Reader"));
}
```

**Step 2: Run test to verify it fails**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_interface_methods_and_embedding`
Expected: FAIL

**Step 3: Implement**

In the `"type_spec"` arm, for interface types, walk the body to find method specs and embedded types:

```rust
if tn.kind() == "interface_type" {
    extract_go_interface_members(tn, source, &name, &mut symbols, &mut type_edges);
}
```

For struct types, also detect embedding (field declarations without a name):

Update `extract_go_struct_field_types` to also handle embedded types:

```rust
fn extract_go_struct_field_types(
    struct_node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let mut cursor = struct_node.walk();
    for child in struct_node.children(&mut cursor) {
        if child.kind() == "field_declaration_list" {
            let mut f_cursor = child.walk();
            for field in child.children(&mut f_cursor) {
                if field.kind() == "field_declaration" {
                    let has_name = field.child_by_field_name("name").is_some();
                    if has_name {
                        // Named field — extract type
                        if let Some(type_node) = field.child_by_field_name("type") {
                            extract_go_type_ref(type_node, source, parent_name, out);
                        }
                    } else {
                        // Embedded type (no name field) — this is Go composition
                        if let Some(type_node) = field.child_by_field_name("type") {
                            extract_go_type_ref(type_node, source, parent_name, out);
                        }
                    }
                }
            }
        }
    }
}
```

Implement interface member extraction:

```rust
fn extract_go_interface_members(
    iface_node: Node<'_>,
    source: &str,
    iface_name: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = iface_node.walk();
    for child in iface_node.children(&mut cursor) {
        // Walk into the method_spec_list (body of interface)
        if child.kind() == "method_spec_list" || child.kind() == "interface_type" {
            let mut inner_cursor = child.walk();
            for member in child.children(&mut inner_cursor) {
                match member.kind() {
                    "method_spec" => {
                        // Interface method: Read(p []byte) (int, error)
                        if let Some(name_node) = member.child_by_field_name("name") {
                            let method_name = text_for_node(name_node, source);
                            let prefixed = format!("{iface_name}.{method_name}");
                            let exported = is_exported(&method_name);
                            symbols.push(symbol_from_node(prefixed.clone(), SymbolKind::Function, exported, member));

                            // Type edge from method to interface
                            type_edges.push((prefixed.clone(), iface_name.to_string()));

                            // Extract param and return types
                            if let Some(params) = member.child_by_field_name("parameters") {
                                let mut p_cursor = params.walk();
                                for param in params.children(&mut p_cursor) {
                                    if param.kind() == "parameter_declaration" {
                                        if let Some(type_node) = param.child_by_field_name("type") {
                                            extract_go_type_ref(type_node, source, &prefixed, type_edges);
                                        }
                                    }
                                }
                            }
                            if let Some(result) = member.child_by_field_name("result") {
                                extract_go_type_ref(result, source, &prefixed, type_edges);
                            }
                        }
                    }
                    "type_identifier" | "qualified_type" => {
                        // Embedded interface: Reader or pkg.Reader
                        let embedded_name = if member.kind() == "qualified_type" {
                            member.child_by_field_name("name")
                                .map(|n| text_for_node(n, source))
                                .unwrap_or_else(|| text_for_node(member, source))
                        } else {
                            text_for_node(member, source)
                        };
                        type_edges.push((iface_name.to_string(), embedded_name));
                    }
                    _ => {}
                }
            }
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests::extracts_go_interface_methods`
Expected: PASS

**Step 5: Run all Go tests**

Run: `EMBEDDINGS_BACKEND=hash cargo test -- go::tests`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add src/indexer/extract/go.rs
git commit -m "feat: extract Go interface methods and struct/interface embedding"
```

---

## Task 9: Build Release + Run Full Test Suite

**Files:** None (verification only)

**Step 1: Run all tests**

Run: `EMBEDDINGS_BACKEND=hash cargo test`
Expected: All tests pass, 0 failures

**Step 2: Build release binary**

Run: `cargo build --release`
Expected: Compiles without errors

**Step 3: Run doc tests**

Run: `EMBEDDINGS_BACKEND=hash cargo test --doc`
Expected: All doc tests pass

**Step 4: Verify with integration test (optional)**

Run: `EMBEDDINGS_BACKEND=hash cargo test --test integration_index_search`
Expected: PASS

**Step 5: Commit (if any fixups needed)**

```bash
git add -A
git commit -m "chore: fixups from full test suite run"
```
