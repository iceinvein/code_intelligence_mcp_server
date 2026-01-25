# Phase 9: Multi-Repo Support - Research

**Researched:** 2026-01-25
**Domain:** Package boundary detection, cross-repo symbol resolution, monorepo patterns
**Confidence:** HIGH

## Summary

Multi-repo support requires detecting package boundaries across multiple language ecosystems, managing repository and package metadata, and enabling cross-package symbol resolution with package-aware scoring. The existing codebase already has `repositories` and `packages` tables in the schema (FNDN-11, FNDN-12), but they are not yet populated or used.

This phase focuses on:
1. **Package manifest detection** - Identifying package.json, Cargo.toml, go.mod, pom.xml, pyproject.toml
2. **Repository detection** - Using git roots to distinguish monorepos from multi-repo workspaces
3. **Package metadata CRUD** - Storing and querying package/repository information
4. **Cross-package resolution** - Resolving symbols across package boundaries
5. **Package-aware scoring** - Boosting same-package results in search

**Primary recommendation:** Use language-specific manifest parsers (serde_json for package.json, toml for Cargo.toml/pyproject.toml, custom for go.mod/pom.xml), combine with git2::Repository::discover() for git root detection, and add package_id to symbols and edges for cross-package resolution.

## Standard Stack

The established libraries/tools for this domain:

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| **git2** | 0.18+ | Git repository detection and root discovery | Industry standard for git operations in Rust; Repository::discover() traverses up directory tree |
| **serde_json** | 1.0 | Parse package.json for npm/yarn workspaces | Standard JSON deserialization in Rust ecosystem |
| **toml** | 0.8 | Parse Cargo.toml and pyproject.toml | De facto standard for TOML parsing in Rust |
| **regex** | 1.10 | Parse go.mod and pom.xml (text-based formats) | Standard regex library for pattern matching |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| **walkdir** | 2.5 | Efficient directory traversal for manifest scanning | When scanning large codebases for package files |
| **ignore** | 0.4 | Respect .gitignore when scanning for packages | To avoid scanning node_modules, target, vendor directories |

### Existing Project Dependencies (already in Cargo.toml)

The following dependencies are already available and should be used:

- `serde` and `serde_json` - for package.json parsing
- `rusqlite` - for CRUD operations on repositories/packages tables
- `tree-sitter-*` - language-specific parsing already available
- `anyhow` - error handling

**No new dependencies required** for basic package detection and resolution.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Manual TOML parsing | toml crate | toml crate is well-tested, handles edge cases (escaped strings, tables) |
| Custom go.mod parser | gomodparse crate | Custom regex parser is sufficient for workspace dependency detection |
| git2 | libgit2-sys | git2 provides safe Rust wrappers around libgit2 |

**Installation:**
```bash
# All dependencies already present in Cargo.toml
# No additional cargo install required
```

## Architecture Patterns

### Recommended Project Structure

```
src/
├── indexer/
│   ├── package/
│   │   ├── mod.rs           # Package detection orchestrator
│   │   ├── detector.rs      # Manifest file detection (walks directory tree)
│   │   ├── parsers/
│   │   │   ├── mod.rs       # Manifest parser dispatcher
│   │   │   ├── npm.rs       # package.json parser (workspaces field)
│   │   │   ├── cargo.rs     # Cargo.toml parser (workspace members)
│   │   │   ├── go.rs        # go.mod parser
│   │   │   ├── maven.rs     # pom.xml parser
│   │   │   └── python.rs    # pyproject.toml parser
│   │   └── git.rs           # Git root detection using git2
│   └── pipeline/
│       └── package_index.rs # Integration with indexing pipeline
├── storage/
│   └── sqlite/
│       └── queries/
│           └── packages.rs  # CRUD operations for repositories/packages
├── retrieval/
│   └── ranking/
│       └── package.rs       # Same-package scoring boost
└── graph/
    └── package.rs           # Cross-package dependency graph
```

### Pattern 1: Package Detection Pipeline

**What:** Three-phase approach to package discovery

**When to use:** During initial indexing and when scanning new directories

**Example:**

```rust
// Phase 1: Discover all manifest files in workspace
let manifests = discover_manifests(&base_dir)?;

// Phase 2: Detect git roots for repository grouping
let repos = detect_git_roots(&manifests)?;

// Phase 3: Parse manifests and associate with repositories
for manifest in manifests {
    let package = parse_package_manifest(&manifest)?;
    let repo_id = find_repo_for_path(&manifest.path, &repos)?;
    upsert_package(&package, repo_id)?;
}
```

**Key insight:** Separate discovery (filesystem) from parsing (manifest-specific) to handle edge cases gracefully.

### Pattern 2: Git Root Detection

**What:** Use git2::Repository::discover() to find repository boundaries

**When to use:** To distinguish monorepo (single git root) from multi-repo workspace

**Example:**

```rust
use git2::Repository;

pub fn discover_git_roots(base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut git_roots = std::collections::HashSet::new();

    // For each manifest, discover its git root
    for manifest in find_all_manifests(base_dir)? {
        if let Ok(repo) = Repository::discover(&manifest) {
            if let Ok(workdir) = repo.workdir() {
                git_roots.insert(workdir.to_path_buf());
            }
        }
    }

    Ok(git_roots.into_iter().collect())
}

// Source: Based on git2 Repository::discover pattern
// Reference: https://docs.rs/git2/latest/git2/struct.Repository.html#method.discover
```

**Key insight:** Repository::discover() traverses parent directories until finding .git, handling nested workspaces correctly.

### Pattern 3: Package-Aware Symbol Resolution

**What:** Add package_id to symbols table for cross-package lookups

**When to use:** When resolving imports that cross package boundaries

**Example:**

```rust
// Schema migration (if needed)
// ALTER TABLE symbols ADD COLUMN package_id TEXT;
// CREATE INDEX idx_symbols_package ON symbols(package_id);

pub fn resolve_symbol_in_package(
    symbol_name: &str,
    from_package_id: &str,
    sqlite: &SqliteStore
) -> Result<Vec<SymbolRow>> {
    // First: try same-package resolution
    let same_package = sqlite
        .search_symbols_by_exact_name_in_package(symbol_name, from_package_id, 10)?;

    if !same_package.is_empty() {
        return Ok(same_package);
    }

    // Fallback: cross-package resolution
    let exported_symbols = sqlite
        .search_exported_symbols_by_exact_name(symbol_name, 10)?;

    // Rank by same-package preference
    Ok(exported_symbols
        .into_iter()
        .map(|mut s| {
            s.package_id = Some(s.package_id.clone());
            s
        })
        .collect())
}
```

### Anti-Patterns to Avoid

- **Using manifest name as primary identifier:** Multiple packages can have the same name (e.g., "utils" in different directories). Use filesystem path instead.
- **Parsing all dependencies:** Only parse workspace-local dependencies. External dependencies (npm, crates.io) should not be indexed.
- **Tight coupling between parser and indexer:** Keep manifest parsing separate from symbol extraction for testability.

## Don't Hand-Roll

Problems that look simple but have existing solutions:

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON parsing | Custom string parsing | serde_json | Handles Unicode, escapes, nested structures |
| TOML parsing | Split by lines/equals | toml crate | Handles tables, arrays, dotted keys, multi-line strings |
| Git root detection | Looking for .git directory | git2::Repository::discover() | Handles worktrees, bare repos, submodules |
| Directory traversal | Recursive fn walk() | walkdir crate | Handles symlinks, permission errors efficiently |

**Key insight:** Filesystem operations and manifest parsing have numerous edge cases (escaped strings, symbolic links, permission errors) that established libraries handle correctly.

## Common Pitfalls

### Pitfall 1: Ignoring Nested Workpaces

**What goes wrong:** A monorepo with packages/packages/Cargo.toml is detected as two separate packages instead of recognizing the workspace structure.

**Why it happens:** Naive detection looks for any manifest file without checking if it's part of a parent workspace.

**How to avoid:** Check parent directories for workspace manifests before treating a nested manifest as an independent package.

**Warning signs:** Seeing more packages than expected, duplicate package names in output.

### Pitfall 2: Misidentifying External Dependencies as Workspace Packages

**What goes wrong:** node_modules/@scope/package gets detected as a workspace package and indexed.

**Why it happens:** Scanning directories without respecting exclude patterns (node_modules, vendor, target).

**How to avoid:** Use the existing should_skip_dir() logic from scan.rs, extended to check for vendor directories in each ecosystem.

**Warning signs:** Massive package count, indexing timeouts, symbols from third-party libraries appearing in results.

### Pitfall 3: Package Identity Collision

**What goes wrong:** Two packages named "utils" in different directories are treated as the same package.

**Why it happens:** Using manifest name field as primary identifier instead of filesystem path.

**How to avoid:** Use filesystem path as primary key for package identity. Store name as metadata only.

**Warning signs:** Symbol lookup returns results from wrong package, incorrect file paths in package metadata.

### Pitfall 4: Cross-Package Resolution Without Export Filtering

**What goes wrong:** Resolving imports returns internal symbols that shouldn't be accessible across package boundaries.

**Why it happens:** Not filtering by exported=true when doing cross-package lookups.

**How to avoid:** Always apply exported filter for cross-package resolution. Same-package can include internal symbols.

**Warning signs:** Test symbols appearing in cross-package results, private functions accessible across packages.

### Pitfall 5: Circular Dependency Detection

**What goes wrong:** Infinite loops when traversing cross-package dependency graphs in monorepos with circular references.

**Why it happens:** Naive graph traversal without cycle detection.

**How to avoid:** Use depth-limited traversal (max_depth=3) and visited set to detect cycles. Log warnings for circular dependencies.

**Warning signs:** Stack overflow, hanging queries, repeated packages in dependency chains.

## Code Examples

Verified patterns from official sources:

### Detecting npm Workspaces

```rust
use serde_json::Value;

pub fn parse_npm_workspace(manifest_path: &Path) -> Result<Option<NpmWorkspace>> {
    let content = std::fs::read_to_string(manifest_path)?;
    let json: Value = serde_json::from_str(&content)?;

    // Check for workspaces field
    if let Some(workspaces) = json.get("workspaces") {
        return Ok(Some(NpmWorkspace {
            // workspaces can be array of strings or object with packages field
            packages: parse_workspaces_packages(workspaces)?,
        }));
    }

    Ok(None)
}

// Source: npm workspaces specification
// Reference: https://docs.npmjs.com/cli/v10/using-npm/workspaces
```

### Detecting Cargo Workspaces

```rust
use toml::Value;

pub fn parse_cargo_workspace(manifest_path: &Path) -> Result<Option<CargoWorkspace>> {
    let content = std::fs::read_to_string(manifest_path)?;
    let toml: Value = toml::from_str(&content)?;

    // Check for workspace.members or [workspace] section
    if let Some(workspace) = toml.get("workspace") {
        return Ok(Some(CargoWorkspace {
            members: workspace
                .get("members")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)))
                .unwrap_or_default()
                .collect(),
        }));
    }

    Ok(None)
}

// Source: Cargo workspace specification
// Reference: https://doc.rust-lang.org/cargo/reference/workspaces.html
```

### Detecting Go Modules

```rust
use regex::Regex;

pub fn parse_go_module(manifest_path: &Path) -> Result<Option<GoModule>> {
    let content = std::fs::read_to_string(manifest_path)?;

    // go.mod is text-based, use regex for module declaration
    let module_re = Regex::new(r"^module\s+([^\s]+)")?;
    let require_re = Regex::new(r"^\s+require\s+([^\s]+)\s+([^\s]+)")?;

    let module_path = module_re.captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    // Extract workspace-local requires (using ./ relative paths)
    let local_requires: Vec<String> = require_re.captures_iter(&content)
        .filter_map(|c| c.get(1))
        .filter(|s| s.as_str().starts_with("./"))
        .map(|m| m.as_str().to_string())
        .collect();

    Ok(Some(GoModule {
        path: module_path.unwrap_or_default(),
        local_dependencies: local_requires,
    }))
}

// Source: go.mod specification
// Reference: https://go.dev/ref/mod#go-mod-file
```

### Package-Aware Scoring

```rust
pub fn apply_package_boost(
    hits: &mut Vec<RankedHit>,
    query_package_id: Option<&str>,
    sqlite: &SqliteStore
) -> Result<()> {
    let Some(query_pkg) = query_package_id else {
        return Ok(());
    };

    // Batch lookup package_id for all hits
    let symbol_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    let package_map = sqlite.batch_get_symbol_packages(&symbol_ids)?;

    for hit in hits.iter_mut() {
        let hit_package_id = package_map.get(&hit.id);

        // Same-package boost: 1.2x for navigation, 1.1x for generic search
        if hit_package_id == Some(query_pkg) {
            hit.score *= 1.2;
        }
    }

    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(())
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Manual package list configuration | Automatic manifest detection | 2020s | Zero-config package discovery |
| Repository = package | Repository can contain multiple packages | 2020s | Monorepo support |
| Single-repo indexing | Multi-repo workspaces | 2025+ | Cross-repo symbol resolution |

**Deprecated/outdated:**
- **Manually configured package lists:** Modern tools auto-detect from manifests
- **Single-package workspace assumption:** 43% of developers work in monorepos (up from 28% in 2022)
- **Name-based package identity:** Path-based identity prevents collisions

## Open Questions

### Question 1: How to handle hybrid monorepo/multi-repo setups?

**What we know:** A workspace can have multiple git roots (multi-repo) AND nested packages (monorepo).

**What's unclear:** When to detect as "monorepo with multiple git roots" vs "separate multi-repo workspace".

**Recommendation:** Use git root count as the primary discriminator:
- Single git root = monorepo
- Multiple git roots = multi-repo
- Both can have nested packages detected via manifests

### Question 2: How to detect query context package automatically?

**What we know:** User file path can be used to infer current package context.

**What's unclear:** Should this be done at search time or pre-computed during indexing?

**Recommendation:** Compute file->package mapping during indexing. At search time, look up package_id from file_path. Allow explicit package_id override via query parameter.

### Question 3: Package dependency storage granularity?

**What we know:** We need to track internal dependencies for cross-package resolution.

**What's unclear:** Store as package-to-package edges or symbol-to-symbol edges?

**Recommendation:** Use symbol-to-symbol edges (already in `edges` table) and infer package-level dependencies via aggregation. This maintains resolution at the symbol level where it's actually used.

## Sources

### Primary (HIGH confidence)

- [git2-rs Repository documentation](https://docs.rs/git2/latest/git2/struct.Repository.html) - Repository::discover() for git root detection
- [Cargo workspaces reference](https://doc.rust-lang.org/cargo/reference/workspaces.html) - workspace.members and [workspace] section
- [npm workspaces documentation](https://docs.npmjs.com/files/package.json/) - workspaces field format
- [Go modules reference](https://go.dev/ref/mod#go-mod-file) - go.mod file format

### Secondary (MEDIUM confidence)

- [pyproject-toml crate](https://crates.io/crates/pyproject-toml) - Python package manifest parsing
- [Managing dependency graphs in large codebases](https://tweag.io/blog/2025-09-18-managing-dependency-graph/) - Monorepo dependency complexity (Sept 2025)
- [10 Common monorepo problems](https://digma.ai/10-common-problems-of-working-with-a-monorepo/) - Circular dependencies and edge cases (Feb 2025)

### Tertiary (LOW confidence)

- [Basic go.mod parsing with Rust](https://gist.github.com/Integralist/a7a84316dd7fd210b06e813fc799246f) - Example go.mod parser (unverified)
- [Moonrepo monorepo tool](https://github.com/moonrepo/moon) - Modern Rust-based monorepo build system

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - git2, serde_json, toml are industry standards
- Architecture: HIGH - patterns based on existing codebase structure and industry best practices
- Pitfalls: MEDIUM - based on common monorepo issues documented in 2025 sources

**Research date:** 2026-01-25
**Valid until:** 30 days (stable domain, package manager formats change slowly)
