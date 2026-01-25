# Phase 9: Multi-Repo Support - Context

**Gathered:** 2026-01-25
**Status:** Ready for planning

<domain>
## Phase Boundary

Infrastructure for detecting package boundaries and enabling symbol resolution across multiple repositories/packages within a single workspace. This phase delivers automatic package detection (package.json, Cargo.toml, go.mod, etc.), storage and management of package/repository metadata, cross-package symbol resolution, and search ranking that prioritizes same-package results.

</domain>

<decisions>
## Implementation Decisions

### Package detection rules
- Language-based detection: TypeScript/JavaScript → package.json, Rust → Cargo.toml, Go → go.mod, Java → pom.xml, Python → requirements.txt/pyproject.toml
- Detect all nested packages: any subdirectory with a manifest file is a package
- Recognize common monorepo patterns:
  - packages/* structure (JS monorepos)
  - Any manifest in subdirectory (generic)
  - npm/Yarn workspaces field
  - Cargo workspace members
- Package identity: Use filesystem path as primary identifier (disambiguates duplicate names)
- Manifest name field stored as metadata but not used for identity

### Package metadata storage
- Store per-package attributes:
  - Package name (from manifest name field)
  - Version (from manifest version field)
  - Root directory path (filesystem location)
  - Language ecosystem (inferred from manifest type)
- Store internal dependencies only (workspace-local packages), ignore external dependencies
- Auto-detect monorepo vs multi-repo workspace based on git root differences
- All packages in same git root share repo_id; different git roots get different repo_ids

### Cross-package resolution
- Hybrid approach: Try package-aware resolution first, fall back to symbol-only if package unknown
- External packages: Resolve references but don't index them (node_modules, vendor, .target ignored)
- Workspace dependencies: Use both manifest workspace references (workspace:*, [workspace]) and filesystem-based resolution
- Prefer manifest references when available, fall back to filesystem paths for implicit dependencies
- Import edges carry package_id to enable cross-package routing

### Same-package scoring
- 1.2x multiplier for same-package results (modest preference)
- Auto-detect query context from indexed file path, allow override via package parameter
- Intent-specific boost:
  - Navigation intents (Definition, Reference, Implementation): 1.2x boost
  - Generic search (General, Error): 1.1x boost (weaker preference)
- Boost applied after other ranking signals, before final reranking

### Claude's Discretion
- Exact auto-detection logic for monorepo vs multi-repo (git root detection strategy)
- How to handle packages with missing or invalid manifests
- Graceful degradation when package information is incomplete
- Performance optimization for package resolution (caching strategy, query batching)

</decisions>

<specifics>
## Specific Ideas

No specific requirements — standard approaches for package detection and cross-package resolution are acceptable.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 09-multi-repo-support*
*Context gathered: 2026-01-25*
