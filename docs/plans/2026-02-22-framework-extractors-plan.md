# Non-TS Framework Extractors — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add framework pattern extraction for Python (FastAPI, Flask, Django), Go (Gin, Echo, Chi), Java (Spring Boot), and Rust (Axum, Actix-web) so that `search_framework_patterns` returns results for non-TypeScript codebases.

**Architecture:** Each framework gets its own `.rs` file in `src/indexer/extract/` following the same pattern as existing extractors (elysia.rs, express.rs, etc.). Each exports `extract_<framework>_patterns(root, source) -> Vec<ExtractedFrameworkPattern>`. Language extractors (python.rs, go.rs, java.rs, rust.rs) call them and merge results into `framework_patterns`. No new `FrameworkPatternKind` variants needed — existing `Route`, `Middleware`, `Router`, `Group`, `Controller`, `Injectable`, `Module` cover all cases.

**Tech Stack:** Rust, tree-sitter (tree-sitter-python, tree-sitter-go, tree-sitter-java, tree-sitter-rust grammars)

**Critical lesson (R118):** Every framework extractor MUST require structural guards to prevent false positives. For routes, the first argument must be a string literal starting with "/". For decorators/annotations, they must be on function/method declarations.

---

## Three Detection Paradigms

1. **Decorator/Annotation** (Python FastAPI/Flask, Java Spring, Rust Actix attributes): Walk `decorated_definition`/`annotation`/`attribute_item` nodes, match decorator names against framework patterns.
2. **Builder/Method-chain** (Go Gin/Echo/Chi, Rust Axum): Walk `call_expression` nodes with method access (Go: `selector_expression`, Rust: `call_expression` on method chains), match method names against HTTP verbs.
3. **Convention-based** (Django): Look for `urlpatterns = [...]` assignments and extract `path()` calls.

## Shared Constants

All extractors reuse `FrameworkPatternKind::Route`, `::Middleware`, `::Router`, `::Group`, `::Controller`, `::Injectable` from `symbol.rs`. HTTP methods: GET, POST, PUT, DELETE, PATCH, OPTIONS, HEAD, ALL.

---

### Task 1: Python FastAPI + Flask Extractor

**Files:**
- Create: `src/indexer/extract/fastapi.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod fastapi;`
- Modify: `src/indexer/extract/python.rs` — call `extract_fastapi_patterns` and merge into `framework_patterns`

**Patterns to detect:**

FastAPI:
- `@app.get("/path")` → Route, method=GET
- `@app.post("/path")` → Route, method=POST
- `@router.get("/path")` → Route, method=GET (APIRouter)
- `@app.middleware("http")` → Middleware
- `@app.on_event("startup")` → Hook

Flask:
- `@app.route("/path", methods=["GET"])` → Route
- `@app.get("/path")` → Route (Flask 2.0+)
- `@bp.route("/path")` → Route (Blueprint)
- `@app.before_request` → Middleware
- `@app.errorhandler(404)` → ErrorHandler

**Detection approach:** Walk tree-sitter `decorated_definition` nodes. The decorator is a `decorator` child. Check if it's a `call` expression where the function is an `attribute` (e.g., `app.get`). Extract the attribute name (method) and first string argument (path).

**Structural guards:**
- Decorator must be on a `function_definition`
- Route path (first positional argument) must be a string literal starting with "/"
- Attribute must match known method names (get, post, put, delete, patch, route, middleware, before_request, etc.)

**Implementation:**

```rust
pub fn extract_fastapi_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern>
```

Walk all nodes recursively. For each `decorated_definition`:
1. Find `decorator` children
2. For each decorator, check if it's a `call` node
3. Get the `function` child — should be `attribute` (e.g., `app.get`)
4. Extract attribute name (the method after `.`)
5. Match against known FastAPI/Flask methods
6. Extract first string argument as route path
7. Verify path starts with "/"
8. Extract the function name from the decorated `function_definition`

**Tests:** 3 tests minimum:
1. FastAPI basic routes (@app.get, @app.post)
2. Flask routes with methods kwarg (@app.route("/path", methods=["GET"]))
3. Middleware detection (@app.before_request)

---

### Task 2: Python Django URL Extractor

**Files:**
- Create: `src/indexer/extract/django.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod django;`
- Modify: `src/indexer/extract/python.rs` — call `extract_django_patterns` and merge

**Patterns to detect:**

URL patterns (urls.py):
- `path('api/users/', views.user_list)` → Route
- `path('api/', include('app.urls'))` → Group
- `re_path(r'^api/.*$', handler)` → Route

DRF (Django REST Framework):
- Class with `ViewSet` or `APIView` base class → Controller
- `@action(detail=True)` → Route
- `@api_view(['GET'])` → Route

**Detection approach:** Walk `assignment` nodes looking for `urlpatterns = [...]`. Inside the list, each `call` to `path()` or `re_path()` is a route. Also detect class definitions inheriting from `*ViewSet` or `*APIView`.

**Structural guards:**
- `path()` calls must have a string literal as first argument
- ViewSet/APIView detection requires `argument_list` in class superclasses containing matching name
- `include()` indicates a Group, not a Route

**Tests:** 2 tests minimum:
1. urlpatterns with path() calls
2. DRF ViewSet class detection

---

### Task 3: Go Gin/Echo/Chi Extractor (Unified)

**Files:**
- Create: `src/indexer/extract/go_frameworks.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod go_frameworks;`
- Modify: `src/indexer/extract/go.rs` — call `extract_go_framework_patterns` and merge

**Patterns to detect:**

Gin:
- `r.GET("/path", handler)` → Route, method=GET
- `r.POST("/path", handler)` → Route, method=POST
- `r.Use(middleware)` → Middleware
- `r.Group("/api")` → Group

Echo:
- `e.GET("/path", handler)` → Route, method=GET
- `e.Use(middleware)` → Middleware
- `e.Group("/api")` → Group

Chi:
- `r.Get("/path", handler)` → Route, method=GET (lowercase Get)
- `r.Use(middleware)` → Middleware
- `r.Route("/api", func(r chi.Router) { ... })` → Group
- `r.Mount("/path", subRouter)` → Group

**Detection approach:** Walk `call_expression` nodes. Check if function is a `selector_expression` (Go's `object.method`). Match the method name against HTTP verbs (case-insensitive for Gin/Echo: `GET`/`POST`; case-sensitive for Chi: `Get`/`Post`). First argument must be a string literal starting with "/".

**Framework discrimination:** Gin uses UPPERCASE methods (`GET`, `POST`), Echo also uppercase, Chi uses TitleCase (`Get`, `Post`). The framework name is set based on method casing:
- UPPERCASE → "gin" (could also be Echo — use "gin_echo" since they're identical)
- TitleCase → "chi"

**Structural guards:**
- Method on a `selector_expression` (not standalone function call)
- First argument is `interpreted_string_literal` starting with `"/`
- Method name matches known HTTP verb or framework method (Use, Group, Route, Mount)

**Tests:** 3 tests minimum:
1. Gin/Echo uppercase routes (r.GET, r.POST)
2. Chi titlecase routes (r.Get, r.Post) + r.Route group
3. Middleware detection (r.Use)

---

### Task 4: Java Spring Boot Extractor

**Files:**
- Create: `src/indexer/extract/spring.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod spring;`
- Modify: `src/indexer/extract/java.rs` — call `extract_spring_patterns` and merge

**Patterns to detect:**

Route annotations:
- `@GetMapping("/path")` → Route, method=GET
- `@PostMapping("/path")` → Route, method=POST
- `@PutMapping("/path")` → Route, method=PUT
- `@DeleteMapping("/path")` → Route, method=DELETE
- `@PatchMapping("/path")` → Route, method=PATCH
- `@RequestMapping(value="/path", method=RequestMethod.GET)` → Route

Class annotations:
- `@RestController` → Controller
- `@Controller` → Controller
- `@RequestMapping("/prefix")` → Group (class-level)
- `@Service` → Injectable
- `@Repository` → Injectable
- `@Component` → Injectable
- `@Configuration` → Module

**Detection approach:** Walk `annotation` nodes (tree-sitter-java). Match annotation name against Spring patterns. For `@*Mapping`, extract the string argument as route path. For class-level annotations, associate with the following class declaration.

**Structural guards:**
- Annotation must be child of `modifiers` node on a `method_declaration` or `class_declaration`
- `@*Mapping` must have string literal argument starting with "/" (or empty for root)
- Only match known Spring annotation names exactly

**Tests:** 3 tests minimum:
1. @GetMapping/@PostMapping routes
2. @RestController + @RequestMapping class-level
3. @Service/@Repository injectable detection

---

### Task 5: Rust Axum Extractor

**Files:**
- Create: `src/indexer/extract/axum.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod axum;`
- Modify: `src/indexer/extract/rust.rs` — call `extract_axum_patterns` and merge

**Patterns to detect:**

Routes:
- `.route("/path", get(handler))` → Route, method=GET
- `.route("/path", post(handler))` → Route, method=POST
- `.route("/path", get(h1).post(h2))` → Two Routes
- `Router::new().route(...)` → Router

Middleware/layers:
- `.layer(middleware)` → Middleware
- `.nest("/prefix", subrouter)` → Group

**Detection approach:** Walk `call_expression` nodes. Look for `.route(` method calls where:
1. First argument is a string literal starting with "/"
2. Second argument is a call to `get()`, `post()`, etc. or a method chain of them

**Structural guards:**
- `.route()` must be a method call (callee is `field_expression`)
- First argument must be `string_literal` starting with "/"
- Second argument handler name must match known Axum HTTP method functions

**Tests:** 2 tests minimum:
1. Basic routes (.route("/path", get(handler)))
2. Nested routes (.nest) + middleware (.layer)

---

### Task 6: Rust Actix-web Extractor

**Files:**
- Create: `src/indexer/extract/actix.rs`
- Modify: `src/indexer/extract/mod.rs` — add `pub mod actix;`
- Modify: `src/indexer/extract/rust.rs` — call `extract_actix_patterns` and merge

**Patterns to detect:**

Attribute macros:
- `#[get("/path")]` → Route, method=GET
- `#[post("/path")]` → Route, method=POST
- `#[put("/path")]`, `#[delete("/path")]`, `#[patch("/path")]`

Builder API:
- `web::resource("/path").route(web::get().to(handler))` → Route
- `web::scope("/prefix")` → Group
- `.app_data(data)` → State

**Detection approach:** Two-phase extraction:
1. Walk `attribute_item` nodes on `function_item`. Match attribute name against actix HTTP verbs.
2. Walk `call_expression` chains for `web::resource`, `web::scope`, `web::get().to()`.

**Structural guards:**
- Attribute HTTP verbs must be directly on a `function_item`
- `web::resource` first argument must be string literal starting with "/"
- Attribute argument must be a string literal

**Tests:** 2 tests minimum:
1. Attribute-based routes (#[get("/path")])
2. Builder API (web::resource, web::scope)

---

### Task 7: Integration — Wire All Extractors into Language Dispatchers

**Files:**
- Modify: `src/indexer/extract/python.rs` — import and call `fastapi::extract_fastapi_patterns` + `django::extract_django_patterns`
- Modify: `src/indexer/extract/go.rs` — import and call `go_frameworks::extract_go_framework_patterns`
- Modify: `src/indexer/extract/java.rs` — import and call `spring::extract_spring_patterns`
- Modify: `src/indexer/extract/rust.rs` — import and call `axum::extract_axum_patterns` + `actix::extract_actix_patterns`

**Pattern:** Follow exactly how `typescript.rs` does it:
```rust
let mut framework_patterns = Vec::new();
framework_patterns.extend(fastapi::extract_fastapi_patterns(root, source));
framework_patterns.extend(django::extract_django_patterns(root, source));
```

Then set `framework_patterns` in the returned `ExtractedFile` instead of `Vec::new()`.

**Tests:** 1 integration test — parse a file with framework patterns and verify they appear in `ExtractedFile.framework_patterns`.

---

### Task 8: Full Build + Test Verification

**Step 1:** Run `cargo test` — all existing + new tests must pass
**Step 2:** Run `cargo build --release` — clean compile
**Step 3:** Run `cargo test --test integration_index_search` — integration tests pass

Commit all remaining changes.
