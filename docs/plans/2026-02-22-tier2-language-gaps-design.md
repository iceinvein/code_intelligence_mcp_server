# Tier 2: Deep Rust + Go Extraction — Design

**Goal:** Bring Rust and Go extractors close to TypeScript-level richness so that `trace_data_flow`, `get_type_graph`, `explore_dependency_graph`, and `get_call_hierarchy` work well for Rust and Go codebases.

**Architecture:** Extend existing tree-sitter extractors (`src/indexer/extract/rust.rs` and `go.rs`) to emit type edges, data flow edges, imports, and richer symbol kinds. No new storage or pipeline changes needed — the existing `edges.rs` pipeline already processes `type_edges`, `dataflow_edges`, and `imports` from `ExtractedFile`.

**Tech Stack:** Rust, tree-sitter (tree-sitter-rust, tree-sitter-go grammars)

---

## Current State

| Feature | TypeScript | Rust | Go |
|---------|-----------|------|----|
| Type edges | Full | Partial (fn params, struct fields) | None |
| Data flow edges | Full | None | None |
| Imports | Yes | None | Yes |
| Inheritance/impl | Via type edges | None | None |
| const/static/type | Yes | None | N/A |
| Method→parent link | Via name | None | None |

## Rust Extractor Improvements

### R1: Import Extraction (`use` statements)

Extract `use_declaration` nodes:
- `use std::collections::HashMap;` → Import{name:"HashMap", source:"std::collections::HashMap"}
- `use crate::path::PathNormalizer;` → Import{name:"PathNormalizer", source:"crate::path::PathNormalizer"}
- `use super::symbol::*;` → Import{name:"*", source:"super::symbol"}
- `use foo as bar;` → Import{name:"foo", source:"foo", alias:"bar"}
- `use std::io::{Read, Write};` → two Import entries

Unblocks `explore_dependency_graph` for Rust.

### R2: Trait Impl Edges

From `impl_item`, emit type edges connecting the impl to both the trait and the type:
- `impl Display for Foo` → type_edge("impl Display for Foo", "Display") + type_edge("impl Display for Foo", "Foo")
- Already have `impl_display_name` — just need to emit the edges

Makes `get_type_graph` bidirectional for Rust traits.

### R3: `const`/`static`/`type` Extraction

- `const_item` → SymbolKind::Const
- `static_item` → SymbolKind::Const (reuse kind)
- `type_item` → SymbolKind::TypeAlias

### R4: Method→Struct Parent Linkage

Methods inside `impl Foo { fn bar() {} }` currently extract as standalone "bar". Changes:
- Track current impl type during walk
- Prefix method names: "Foo::bar"
- Emit type_edge("bar", "Foo") connecting method to struct

### R5: Data Flow Edges (reads/writes)

Walk function bodies (`block` nodes inside `function_item`) to extract:
- `let x = foo();` → writes("x"), reads("foo")
- `x = value;` → writes("x"), reads("value")
- `self.field = value` → writes("field"), reads("value")
- `process(data)` → reads("process"), reads("data")

Scope: top-level identifiers in expressions. Not deep analysis.

### R6: Trait Method Signature Types

Extract type edges from trait method signatures:
- `fn process(&self, input: Input) -> Output` inside a trait → type_edges to Input and Output

## Go Extractor Improvements

### G1: Type Edges for Functions and Structs

- Function params/returns: `func Process(u User) error` → type_edge("Process", "User"), type_edge("Process", "error")
- Struct fields: `type Config struct { DB *Database }` → type_edge("Config", "Database")

### G2: Method Receiver Linkage

- `func (g *GoGreeter) Greet()` → type_edge("Greet", "GoGreeter")
- Prefix name as "GoGreeter.Greet"

### G3: Interface Method Extraction

- `type Reader interface { Read(p []byte) (int, error) }` → extract "Read" as Function, type_edge("Read", "Reader")

### G4: Struct Embedding (Composition)

- `type Server struct { http.Handler }` → type_edge("Server", "Handler")
- Go's equivalent of extends/implements

## Out of Scope (YAGNI)

- No data flow for Go
- No framework patterns for Rust/Go
- No Go const/var blocks
- No Rust lifetime tracking
- No Rust where clause constraints
- No import path resolution to file paths

## Testing

Unit tests per addition in each extractor's `#[cfg(test)] mod tests`. Test real code snippets, verify symbol kinds, edge counts, edge targets. Integration: build release, index this codebase, verify tools work.
