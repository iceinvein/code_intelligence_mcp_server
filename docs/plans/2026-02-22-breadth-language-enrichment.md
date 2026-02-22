# Breadth: Language Extractor Enrichment Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring Python, Java, C, and C++ extractors to parity with the enriched Rust/Go extractors — method prefixing, type edges, data flow edges, visibility detection, const extraction, and line number consistency.

**Architecture:** Extend existing tree-sitter extractors in `src/indexer/extract/{python,java,c,cpp}.rs` to emit the same `type_edges`, `dataflow_edges`, and richer symbol kinds that Rust/Go already produce. The downstream pipeline (`src/indexer/pipeline/edges.rs`) already processes all these — no storage or pipeline changes needed.

**Tech Stack:** Rust, tree-sitter (tree-sitter-python, tree-sitter-java, tree-sitter-c, tree-sitter-cpp grammars)

---

## Current State

| Feature | TypeScript | Rust | Go | Python | Java | C | C++ |
|---------|-----------|------|----|--------|------|---|-----|
| Method prefixing | Yes | `Foo::bar` | `Server.Start` | **No** | **No** | N/A | **No** |
| Type edges (fn sigs) | Yes | Yes | Yes | **No** | **No** | **No** | **No** |
| Type edges (fields) | Yes | Yes | Yes | **No** | **No** | **No** | **No** |
| Type edges (inheritance) | Yes | Yes (impl) | Yes (embed) | **No** | **No** | N/A | **No** |
| Data flow edges | Yes | Yes | No | **No** | **No** | **No** | **No** |
| Imports | Yes | Yes | Yes | Partial | Yes | Yes | Yes |
| Const/static | Yes | Yes | N/A | **No** | **No** | **No** | **No** |
| Visibility/export | Yes | `pub` | Uppercase | `_`prefix | `public` | **All true** | **All true** |
| Line numbers | 1-indexed | 1-indexed | 1-indexed | **0-indexed** | **0-indexed** | **0-indexed** | **0-indexed** |

## Reusable Patterns (from Rust/Go)

**Method prefixing:** Track `current_class`/`current_impl` during walk. Skip methods in the flat `function_*` arm (parent-chain guard). Re-extract them inside the class/impl arm with prefixed names.

**Type edge extraction:** Walk param nodes, find type annotations, extract base type name via recursive `extract_type_name()` helper (strips generics, references, pointers, optionals). Push `(symbol_name, type_name)` pairs.

**Data flow:** Walk function body for assignment nodes. LHS identifier → `Writes` edge. RHS identifiers/call names → `Reads` edge. Keep it shallow (top-level identifiers only).

**Line numbers:** Rust/Go use `start.row as u32 + 1` (1-indexed). Python/Java/C/C++ currently use `start.row as u32` (0-indexed).

---

## Task 1: Python — Line Numbers + Dunder Export Fix

**Files:**
- Modify: `src/indexer/extract/python.rs:78-93` (symbol_from_node)
- Modify: `src/indexer/extract/python.rs:25` (dunder export check)
- Test: `src/indexer/extract/python.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_line_numbers_are_1_indexed() {
    let source = "def hello():\n    pass\n";
    let extracted = extract_python_symbols(source).unwrap();
    let hello = extracted.symbols.iter().find(|s| s.name == "hello").unwrap();
    assert_eq!(hello.lines.start, 1); // line 1, not 0
    assert_eq!(hello.lines.end, 2);
}

#[test]
fn test_dunder_methods_exported() {
    let source = "class Foo:\n    def __init__(self):\n        pass\n    def __str__(self):\n        return ''\n";
    let extracted = extract_python_symbols(source).unwrap();
    let init = extracted.symbols.iter().find(|s| s.name.contains("__init__")).unwrap();
    assert!(init.exported, "__init__ should be exported (dunder methods are public API)");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::python -- test_line_numbers test_dunder`
Expected: FAIL — line numbers are 0-indexed, `__init__` has `exported=false`

**Step 3: Fix line numbers and dunder export**

In `symbol_from_node`, change:
```rust
lines: LineSpan {
    start: start.row as u32 + 1,
    end: end.row as u32 + 1,
},
```

In the `function_definition` arm, change export logic:
```rust
let is_dunder = name.starts_with("__") && name.ends_with("__");
let exported = is_dunder || !name.starts_with('_');
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::python`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/python.rs
git commit -m "fix: python extractor 1-indexed lines and dunder export"
```

---

## Task 2: Python — Class Method Prefixing

**Files:**
- Modify: `src/indexer/extract/python.rs` (walk callback, new helper)
- Test: `src/indexer/extract/python.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_class_method_prefixing() {
    let source = r#"
class MyClass:
    def method(self):
        pass
    def _private(self):
        pass

class Other:
    def action(self):
        pass
"#;
    let extracted = extract_python_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "MyClass.method"));
    assert!(extracted.symbols.iter().any(|s| s.name == "MyClass._private"));
    assert!(extracted.symbols.iter().any(|s| s.name == "Other.action"));
    // standalone method should not exist
    assert!(!extracted.symbols.iter().any(|s| s.name == "method"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::python -- test_class_method_prefixing`
Expected: FAIL — methods not prefixed

**Step 3: Implement class method prefixing**

Replace the flat `walk` with a recursive function that tracks `current_class: Option<String>`:

1. In the `class_definition` arm: extract the class symbol, then walk children. For any `function_definition` child inside the class body (`block`), prefix with `ClassName.method_name`.
2. In the `function_definition` arm: add parent-chain guard — if parent is `block` whose parent is `class_definition`, skip (handled by the class arm).
3. Methods inherit export from class: `ClassName.method` is exported if `ClassName` is exported (doesn't start with `_`), unless the method itself starts with `_` (but NOT dunder).

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::python`
Expected: All PASS (update old test to expect `MyClass.method` instead of `method`)

**Step 5: Commit**

```bash
git add src/indexer/extract/python.rs
git commit -m "feat: python class method prefixing (ClassName.method)"
```

---

## Task 3: Python — Type Edge Extraction

**Files:**
- Modify: `src/indexer/extract/python.rs` (new helpers, type_edges population)
- Test: `src/indexer/extract/python.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_python_type_edges() {
    let source = r#"
def process(user: User, count: int) -> Result:
    pass

class MyClass:
    name: str
    items: List[Item]

class Child(Parent, Mixin):
    pass
"#;
    let extracted = extract_python_symbols(source).unwrap();

    // Function signature types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "User"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "Result"));

    // Class field types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "MyClass" && e.1 == "List"));

    // Inheritance
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Child" && e.1 == "Parent"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Child" && e.1 == "Mixin"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::python -- test_python_type_edges`
Expected: FAIL — `type_edges` is `Vec::new()`

**Step 3: Implement type edge extraction**

Three sources of type edges:

1. **Function signatures** (`parameters` field → `typed_parameter`/`typed_default_parameter` children with `type` field; `return_type` field). Extract base type name from `type` node (strip `Optional[...]`, `List[...]` → extract outer name).

2. **Class fields** (`expression_statement` children with `type` node inside class body, or `assignment` with type annotation). Walk class body for `typed_parameter` or `assignment` nodes with type annotations.

3. **Inheritance** (`argument_list` field on `class_definition` → each `identifier` child is a base class). Tree-sitter-python: `class Child(Parent):` has `superclasses` field or `argument_list` child.

Add helper `extract_python_type_name(node, source) -> Option<String>` that recursively finds the base type identifier (strips subscripts, attributes, optional wrappers).

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::python`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/python.rs
git commit -m "feat: python type edges (signatures, fields, inheritance)"
```

---

## Task 4: Python — Async Functions + Constants

**Files:**
- Modify: `src/indexer/extract/python.rs` (walk callback)
- Test: `src/indexer/extract/python.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_async_functions() {
    let source = "async def fetch_data():\n    pass\n";
    let extracted = extract_python_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "fetch_data" && s.kind == SymbolKind::Function));
}

#[test]
fn test_module_constants() {
    let source = r#"
MAX_RETRIES = 3
DEFAULT_TIMEOUT: int = 30
_INTERNAL = "secret"
"#;
    let extracted = extract_python_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Const));
    assert!(extracted.symbols.iter().any(|s| s.name == "DEFAULT_TIMEOUT" && s.kind == SymbolKind::Const));
    let internal = extracted.symbols.iter().find(|s| s.name == "_INTERNAL").unwrap();
    assert!(!internal.exported);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::python -- test_async test_module_constants`
Expected: FAIL

**Step 3: Implement**

- **Async functions:** Tree-sitter-python uses `function_definition` for both sync and async. The `async` keyword is a child. No change needed if already matching `function_definition` — verify this works. If tree-sitter uses a separate node kind, handle it.

- **Module-level constants:** Match `expression_statement` at module level where the expression is `assignment` with an `UPPER_CASE` left-hand-side identifier. Convention: names that are ALL_CAPS at module level = constants. Add as `SymbolKind::Const`. Export logic: `!name.starts_with('_')`.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::python`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/python.rs
git commit -m "feat: python async function support and module constant extraction"
```

---

## Task 5: Python — Data Flow Edges

**Files:**
- Modify: `src/indexer/extract/python.rs` (new `extract_python_dataflow` helper)
- Test: `src/indexer/extract/python.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_python_dataflow() {
    let source = r#"
def process(data):
    result = transform(data)
    self.output = result
    save(result)
"#;
    let extracted = extract_python_symbols(source).unwrap();
    // result = transform(data) → writes("result"), reads("transform"), reads("data")
    assert!(extracted.dataflow_edges.iter().any(|e|
        e.from_symbol == "result" && e.flow_type == DataFlowType::Writes
    ));
    assert!(extracted.dataflow_edges.iter().any(|e|
        e.from_symbol == "transform" && e.flow_type == DataFlowType::Reads
    ));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::python -- test_python_dataflow`
Expected: FAIL — `dataflow_edges` is empty

**Step 3: Implement data flow extraction**

Follow Rust extractor pattern (`extract_rust_dataflow`):
1. Find function body (`block` child of `function_definition`)
2. Walk body for `assignment` nodes: LHS identifier → Writes, RHS identifiers/call names → Reads
3. Walk body for `call` expressions outside assignments: function name → Reads, arguments → Reads
4. Set `to_symbol` to the enclosing function name, `at_line` to 1-indexed line

Python-specific: `self.field = value` → LHS uses `attribute` node (`self.field`), extract `field` part as the write target.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::python`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/python.rs
git commit -m "feat: python data flow edges (reads/writes from function bodies)"
```

---

## Task 6: Java — Line Numbers + Interface Method Export Fix

**Files:**
- Modify: `src/indexer/extract/java.rs:118-134` (symbol_from_node)
- Modify: `src/indexer/extract/java.rs:102-116` (is_public)
- Test: `src/indexer/extract/java.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_java_line_numbers_1_indexed() {
    let source = "public class Foo {\n    public void bar() {}\n}\n";
    let extracted = extract_java_symbols(source).unwrap();
    let foo = extracted.symbols.iter().find(|s| s.name == "Foo").unwrap();
    assert_eq!(foo.lines.start, 1);
}

#[test]
fn test_interface_methods_exported() {
    let source = "public interface Service {\n    void process();\n}\n";
    let extracted = extract_java_symbols(source).unwrap();
    let process = extracted.symbols.iter().find(|s| s.name == "process").unwrap();
    assert!(process.exported, "Interface methods are implicitly public");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::java -- test_java_line test_interface_methods`
Expected: FAIL

**Step 3: Fix line numbers and interface method export**

In `symbol_from_node`:
```rust
lines: LineSpan {
    start: start.row as u32 + 1,
    end: end.row as u32 + 1,
},
```

In `is_public`, add interface-parent check:
```rust
fn is_public(node: Node) -> bool {
    // Interface methods are implicitly public
    if node.kind() == "method_declaration" {
        if let Some(parent) = node.parent() {
            if parent.kind() == "interface_body" {
                return true;
            }
        }
    }
    // Existing modifier check...
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::java`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/java.rs
git commit -m "fix: java extractor 1-indexed lines and interface method export"
```

---

## Task 7: Java — Method Prefixing + Constructors

**Files:**
- Modify: `src/indexer/extract/java.rs` (walk callback restructure)
- Test: `src/indexer/extract/java.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_java_method_prefixing() {
    let source = r#"
public class UserService {
    public void save() {}
    private void validate() {}
}
"#;
    let extracted = extract_java_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "UserService.save"));
    assert!(extracted.symbols.iter().any(|s| s.name == "UserService.validate"));
    assert!(!extracted.symbols.iter().any(|s| s.name == "save"));
}

#[test]
fn test_java_constructors() {
    let source = r#"
public class User {
    public User(String name) {}
}
"#;
    let extracted = extract_java_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "User.User" && s.kind == SymbolKind::Function));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::java -- test_java_method_prefixing test_java_constructors`
Expected: FAIL

**Step 3: Implement**

- **Method prefixing:** In the `class_declaration` and `interface_declaration` arms, after extracting the class symbol, walk the class `body` for `method_declaration` children. Prefix method names with `ClassName.methodName`. Add parent-chain guard in the top-level `method_declaration` arm to skip methods inside classes.

- **Constructors:** Match `constructor_declaration` node kind (Java tree-sitter grammar). Name as `ClassName.ClassName` (constructor). Kind: `SymbolKind::Function`.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::java`
Expected: All PASS (update old test to expect prefixed names)

**Step 5: Commit**

```bash
git add src/indexer/extract/java.rs
git commit -m "feat: java method prefixing and constructor extraction"
```

---

## Task 8: Java — Type Edges (extends/implements + signatures)

**Files:**
- Modify: `src/indexer/extract/java.rs` (new helpers, type_edges population)
- Test: `src/indexer/extract/java.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_java_type_edges() {
    let source = r#"
public class UserService extends BaseService implements Serializable {
    private UserRepository repo;
    public User findById(Long id) { return null; }
}
"#;
    let extracted = extract_java_symbols(source).unwrap();

    // extends/implements
    assert!(extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "BaseService"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "Serializable"));

    // Method signature types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "UserService.findById" && e.1 == "User"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "UserService.findById" && e.1 == "Long"));

    // Field types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "UserRepository"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::java -- test_java_type_edges`
Expected: FAIL — `type_edges` is `Vec::new()`

**Step 3: Implement type edge extraction**

Three sources:

1. **extends/implements:** `class_declaration` has `superclass` field (the `extends` type) and `interfaces` field (the `implements` list). Extract type names from `type_identifier` nodes.

2. **Method signatures:** Walk method `parameters` for `formal_parameter` children with `type` field. Walk `type` field (return type) of `method_declaration`. Extract base type from `type_identifier` (strip generics).

3. **Field declarations:** Match `field_declaration` children inside class body. The `type` field has the field type. Push `(class_name, field_type)`.

Add helper `extract_java_type_name(node, source) -> Option<String>` for generic-stripping.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::java`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/java.rs
git commit -m "feat: java type edges (extends/implements, signatures, fields)"
```

---

## Task 9: Java — Data Flow Edges + Annotations

**Files:**
- Modify: `src/indexer/extract/java.rs` (new helpers)
- Test: `src/indexer/extract/java.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_java_dataflow() {
    let source = r#"
public class Service {
    public void process() {
        User user = findUser();
        user.name = "test";
        save(user);
    }
}
"#;
    let extracted = extract_java_symbols(source).unwrap();
    assert!(extracted.dataflow_edges.iter().any(|e|
        e.from_symbol == "user" && e.flow_type == DataFlowType::Writes
    ));
    assert!(extracted.dataflow_edges.iter().any(|e|
        e.from_symbol == "findUser" && e.flow_type == DataFlowType::Reads
    ));
}

#[test]
fn test_java_constants() {
    let source = r#"
public class Config {
    public static final int MAX_RETRIES = 3;
    private static final String SECRET = "key";
}
"#;
    let extracted = extract_java_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "Config.MAX_RETRIES" && s.kind == SymbolKind::Const));
    assert!(extracted.symbols.iter().any(|s| s.name == "Config.SECRET" && s.kind == SymbolKind::Const));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::java -- test_java_dataflow test_java_constants`
Expected: FAIL

**Step 3: Implement**

- **Data flow:** Follow Rust pattern. Walk method body (`block` child of `method_declaration`) for `local_variable_declaration` and `assignment_expression` nodes. LHS → Writes, RHS identifiers/calls → Reads.

- **Constants:** Match `field_declaration` inside class body where modifiers include `static` and `final`. Extract as `SymbolKind::Const` with `ClassName.FIELD_NAME` prefixed name.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::java`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/java.rs
git commit -m "feat: java data flow edges and constant extraction"
```

---

## Task 10: C — Line Numbers + Static Visibility + Unions

**Files:**
- Modify: `src/indexer/extract/c.rs` (symbol_from_node, walk callback)
- Test: `src/indexer/extract/c.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_c_line_numbers_1_indexed() {
    let source = "int add(int a, int b) {\n    return a + b;\n}\n";
    let extracted = extract_c_symbols(source).unwrap();
    let add = extracted.symbols.iter().find(|s| s.name == "add").unwrap();
    assert_eq!(add.lines.start, 1);
}

#[test]
fn test_c_static_not_exported() {
    let source = "static int helper(int x) { return x; }\nint public_fn() { return 0; }\n";
    let extracted = extract_c_symbols(source).unwrap();
    let helper = extracted.symbols.iter().find(|s| s.name == "helper").unwrap();
    assert!(!helper.exported, "static functions should not be exported");
    let public_fn = extracted.symbols.iter().find(|s| s.name == "public_fn").unwrap();
    assert!(public_fn.exported);
}

#[test]
fn test_c_union() {
    let source = "union Data {\n    int i;\n    float f;\n};\n";
    let extracted = extract_c_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "Data" && s.kind == SymbolKind::Struct));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::c -- test_c_line test_c_static test_c_union`
Expected: FAIL

**Step 3: Implement**

- **Line numbers:** `+1` in `symbol_from_node`.

- **Static visibility:** Check if `function_definition` has a `storage_class_specifier` child with text `"static"`. If so, `exported = false`. Otherwise `true`.

    ```rust
    fn is_static(node: Node, source: &str) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "storage_class_specifier" {
                if child.utf8_text(source.as_bytes()).unwrap() == "static" {
                    return true;
                }
            }
        }
        false
    }
    ```

- **Union support:** Match `union_specifier` → `SymbolKind::Struct` (reuse kind, same as Go embedding).

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::c`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/c.rs
git commit -m "fix: c extractor 1-indexed lines, static visibility, union support"
```

---

## Task 11: C — Type Edges + Global Variables

**Files:**
- Modify: `src/indexer/extract/c.rs` (new helpers, type_edges population)
- Test: `src/indexer/extract/c.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_c_type_edges() {
    let source = r#"
struct Config {
    Database *db;
    Logger *log;
};

int process(User *user, Config *cfg) {
    return 0;
}
"#;
    let extracted = extract_c_symbols(source).unwrap();

    // Struct field types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Config" && e.1 == "Database"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Config" && e.1 == "Logger"));

    // Function param types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "User"));
    assert!(extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "Config"));
}

#[test]
fn test_c_global_variables() {
    let source = "int global_count = 0;\nstatic char *internal_buf;\n";
    let extracted = extract_c_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "global_count" && s.kind == SymbolKind::Const));
    let internal = extracted.symbols.iter().find(|s| s.name == "internal_buf").unwrap();
    assert!(!internal.exported);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::c -- test_c_type_edges test_c_global`
Expected: FAIL

**Step 3: Implement**

- **Struct field types:** Walk `field_declaration_list` children of `struct_specifier`. Each `field_declaration` has a `type` child. Extract base type from `type_identifier` (strip pointer declarator).

- **Function param types:** Walk `parameter_list` children of `function_declarator`. Each `parameter_declaration` has a `type` child. Extract base type.

- **Global variables:** Match `declaration` at top level (not inside function). Extract name from declarator, kind=`SymbolKind::Const`. Check for `static` storage class.

- **Type name extraction helper:** `extract_c_type_name(node, source) -> Option<String>` — recurse through `type_identifier`, `struct_specifier`, `enum_specifier` to find the base name. Skip primitive types (`int`, `char`, `float`, `double`, `void`).

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::c`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/c.rs
git commit -m "feat: c type edges (struct fields, function params) and global variables"
```

---

## Task 12: C++ — Line Numbers + Access Specifier Visibility

**Files:**
- Modify: `src/indexer/extract/cpp.rs` (symbol_from_node, walk callback)
- Test: `src/indexer/extract/cpp.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[test]
fn test_cpp_line_numbers_1_indexed() {
    let source = "class Foo {};\n";
    let extracted = extract_cpp_symbols(source).unwrap();
    let foo = extracted.symbols.iter().find(|s| s.name == "Foo").unwrap();
    assert_eq!(foo.lines.start, 1);
}

#[test]
fn test_cpp_access_specifier() {
    let source = r#"
class MyClass {
public:
    void publicMethod() {}
private:
    void privateMethod() {}
protected:
    void protectedMethod() {}
};
"#;
    let extracted = extract_cpp_symbols(source).unwrap();
    let public_m = extracted.symbols.iter().find(|s| s.name.contains("publicMethod")).unwrap();
    assert!(public_m.exported);
    let private_m = extracted.symbols.iter().find(|s| s.name.contains("privateMethod")).unwrap();
    assert!(!private_m.exported);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib indexer::extract::cpp -- test_cpp_line test_cpp_access`
Expected: FAIL

**Step 3: Implement**

- **Line numbers:** `+1` in `symbol_from_node`.

- **Access specifiers:** Track `current_access: String` during walk. When encountering `access_specifier` node, update state to its text (`"public"`, `"private"`, `"protected"`). Methods/fields use current access to set `exported`. Default for `class` is `private`, for `struct` is `public`.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::cpp`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/cpp.rs
git commit -m "fix: c++ extractor 1-indexed lines and access specifier visibility"
```

---

## Task 13: C++ — Method Prefixing + Declarations in Class Bodies

**Files:**
- Modify: `src/indexer/extract/cpp.rs` (walk callback restructure)
- Test: `src/indexer/extract/cpp.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_cpp_method_prefixing() {
    let source = r#"
class Server {
public:
    void start() {}
    int getPort();
};
"#;
    let extracted = extract_cpp_symbols(source).unwrap();
    assert!(extracted.symbols.iter().any(|s| s.name == "Server.start"));
    // Method declarations (no body) should also be extracted
    assert!(extracted.symbols.iter().any(|s| s.name == "Server.getPort"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::cpp -- test_cpp_method_prefixing`
Expected: FAIL

**Step 3: Implement**

- **Method prefixing:** In the `class_specifier`/`struct_specifier` arms, after extracting the class/struct, walk the `field_declaration_list` body. For `function_definition` children, prefix with `ClassName.method`. For `declaration` children that contain a `function_declarator`, extract as `ClassName.method` (method declarations without body).

- **Parent-chain guard:** In the top-level `function_definition` arm, skip if parent is `field_declaration_list`.

- Use `.` separator (not `::`) to match Python/Java/Go convention for method prefixing.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::cpp`
Expected: All PASS (update old test)

**Step 5: Commit**

```bash
git add src/indexer/extract/cpp.rs
git commit -m "feat: c++ method prefixing and class member declarations"
```

---

## Task 14: C++ — Type Edges (inheritance + signatures)

**Files:**
- Modify: `src/indexer/extract/cpp.rs` (new helpers, type_edges population)
- Test: `src/indexer/extract/cpp.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_cpp_type_edges() {
    let source = r#"
class Animal {
public:
    virtual void speak() = 0;
};

class Dog : public Animal {
    std::string name;
public:
    void speak() override {}
    void fetch(Ball *ball) {}
};
"#;
    let extracted = extract_cpp_symbols(source).unwrap();

    // Inheritance
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Dog" && e.1 == "Animal"));

    // Method param types
    assert!(extracted.type_edges.iter().any(|e| e.0 == "Dog.fetch" && e.1 == "Ball"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib indexer::extract::cpp -- test_cpp_type_edges`
Expected: FAIL

**Step 3: Implement**

- **Inheritance:** `class_specifier` has `base_class_clause` child. Walk its `type_identifier` children (the base class names). Push `(class_name, base_class_name)`.

- **Method signature types:** Reuse C-style param extraction. Walk `parameter_list` of function declarators. Extract base type from `type_identifier`.

- **Field types:** Walk `field_declaration` children in class body. Extract type from `type` field. Skip primitive types.

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib indexer::extract::cpp`
Expected: All PASS

**Step 5: Commit**

```bash
git add src/indexer/extract/cpp.rs
git commit -m "feat: c++ type edges (inheritance, method params, fields)"
```

---

## Task 15: Build + Full Test Suite

**Files:**
- All modified files above

**Step 1: Run all tests**

Run: `EMBEDDINGS_BACKEND=hash cargo test`
Expected: All tests pass (unit + integration)

**Step 2: Build release**

Run: `cargo build --release`
Expected: Clean compile

**Step 3: Verify no regressions**

Run: `cargo test --test integration_index_search`
Expected: All integration tests pass

**Step 4: Commit any remaining changes**

```bash
git status
# If any unstaged fixes needed, commit them
```

---

## Out of Scope (YAGNI)

- **Framework extractors for non-TS languages** (FastAPI, Spring Boot, etc.) — separate follow-up
- **Deep data flow for C/C++** — too many pointer-level complexities; skip for now
- **Python decorator extraction** — framework-specific, not core extraction
- **Java records** — rare in practice, skip for now
- **C++ templates** — type parameterization is Tier 3 work
- **C macros** (#define) — can't reliably extract semantics from preprocessor
- **Import path resolution** to actual files — needs project-level analysis

## Files Touched Summary

| File | Changes |
|------|---------|
| `src/indexer/extract/python.rs` | Tasks 1-5: line fix, dunder export, method prefix, type edges, constants, async, data flow |
| `src/indexer/extract/java.rs` | Tasks 6-9: line fix, interface export, method prefix, constructors, type edges, data flow, constants |
| `src/indexer/extract/c.rs` | Tasks 10-11: line fix, static visibility, union, type edges, global vars |
| `src/indexer/extract/cpp.rs` | Tasks 12-14: line fix, access specifiers, method prefix, declarations, type edges |

## Testing Strategy

- Unit tests per task using inline `#[cfg(test)]` blocks with real code snippets
- Run `EMBEDDINGS_BACKEND=hash cargo test` after each language is complete
- Integration: rebuild release, index a polyglot codebase, verify tools work across languages
